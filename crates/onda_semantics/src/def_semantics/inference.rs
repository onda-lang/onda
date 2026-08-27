use std::collections::{HashMap, HashSet};

use super::call_types::const_positive_usize_for_call_type;

use onda_frontend::{
    AssignTarget, BufferChannels, BufferElemType, CallArg, DiagCtx, Diagnostic, Expr, FnParamType,
    FunctionDef, PrimitiveType, Stmt,
};

mod return_inference;
pub(crate) use return_inference::*;
mod call_inference;
use call_inference::infer_stmt_calls;
pub(crate) use call_inference::{resolve_call_args, resolve_call_args_at};

use crate::builtins::{
    builtin_constant_type, eval_data_size_expr, is_builtin_constant_name,
    validate_buffer_static_channels,
};
use crate::{
    push_semantic, resolve_struct_field_decl, with_expr_diag_context, with_stmt_diag_context,
    AnalysisOptions, FnSignature, ProcNestedArrayState, TypedBufferChannels, TypedFieldType,
    TypedFnParam, TypedStructField,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StructFieldUsage {
    Scalar,
    Array,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct InferredFnParam {
    method_self_struct: Option<String>,
    saw_scalar: bool,
    saw_structs: HashSet<String>,
    saw_struct_arrays: Vec<InferredStructArrayParam>,
    saw_proc_arrays: Vec<InferredProcArrayParam>,
    saw_arrays: Vec<InferredArrayParam>,
    saw_buffers: Vec<InferredBufferParam>,
    saw_seeded_buffer: bool,
    saw_call_buffer: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InferredBufferParam {
    pub(crate) elem_ty: PrimitiveType,
    pub(crate) channels: TypedBufferChannels,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InferredBufferBinding {
    pub(crate) candidates: Vec<InferredBufferParam>,
    pub(crate) is_array: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InferredArrayParam {
    pub(crate) elem_ty: PrimitiveType,
    pub(crate) len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InferredStructArrayParam {
    pub(crate) struct_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InferredProcArrayParam {
    pub(crate) proc_name: String,
    pub(crate) len: usize,
}

pub(crate) fn infer_def_param_kinds(
    defs: &[FunctionDef],
    init: &[Stmt],
    block_stmts: &[Stmt],
    sample: &[Stmt],
    struct_instances: &HashMap<String, String>,
    struct_array_roots: &HashMap<String, String>,
    proc_array_roots: &HashMap<String, InferredProcArrayParam>,
    array_bindings: &HashMap<String, InferredArrayParam>,
    buffer_bindings: &HashMap<String, InferredBufferBinding>,
    fn_signatures: &HashMap<String, FnSignature>,
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_types: &HashSet<String>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (
    HashMap<String, Vec<TypedFnParam>>,
    HashMap<String, Vec<TypedStructField>>,
) {
    let declared_struct_params = collect_declared_struct_param_types(
        defs,
        fn_signatures,
        method_self_struct,
        struct_defs,
        errors,
    );
    let declared_buffer_params =
        collect_declared_buffer_param_types(defs, fn_signatures, options, errors);
    let field_usage = collect_def_param_field_usage(
        defs,
        fn_signatures,
        &declared_struct_params,
        struct_defs,
        errors,
    );

    let mut kinds = HashMap::new();
    for def in defs {
        kinds.insert(
            def.name.clone(),
            vec![InferredFnParam::default(); def.params.len()],
        );
    }

    for def in defs {
        if let Some(explicit) = declared_struct_params.get(&def.name) {
            if let Some(kinds_for_def) = kinds.get_mut(&def.name) {
                for (idx, explicit_struct) in explicit.iter().enumerate() {
                    if let (Some(struct_name), Some(dst)) =
                        (explicit_struct.as_ref(), kinds_for_def.get_mut(idx))
                    {
                        if idx == 0
                            && method_self_struct.get(&def.name).map(String::as_str)
                                == Some(struct_name.as_str())
                        {
                            dst.method_self_struct = Some(struct_name.clone());
                        }
                        dst.saw_structs.insert(struct_name.clone());
                    }
                }
            }
        }
    }

    let mut init_array_bindings = array_bindings.clone();
    let mut init_buffer_bindings = buffer_bindings.clone();
    for stmt in init {
        infer_stmt_calls(
            stmt,
            struct_instances,
            struct_array_roots,
            proc_array_roots,
            &mut init_array_bindings,
            &mut init_buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }
    let mut block_array_bindings = array_bindings.clone();
    let mut block_buffer_bindings = buffer_bindings.clone();
    for stmt in block_stmts {
        infer_stmt_calls(
            stmt,
            struct_instances,
            struct_array_roots,
            proc_array_roots,
            &mut block_array_bindings,
            &mut block_buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }
    let mut sample_array_bindings = array_bindings.clone();
    let mut sample_buffer_bindings = block_buffer_bindings.clone();
    for stmt in sample {
        infer_stmt_calls(
            stmt,
            struct_instances,
            struct_array_roots,
            proc_array_roots,
            &mut sample_array_bindings,
            &mut sample_buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }

    // Propagate inferred def parameter kinds through def-to-def calls.
    for _ in 0..defs.len().saturating_add(1) {
        let snapshot = kinds.clone();
        for def in defs {
            let param_index = def
                .params
                .iter()
                .enumerate()
                .map(|(idx, p)| (p.name.clone(), idx))
                .collect::<HashMap<_, _>>();
            for stmt in &def.body {
                propagate_stmt_callee_buffer_requirements_to_params(
                    stmt,
                    &def.name,
                    &param_index,
                    fn_signatures,
                    &declared_buffer_params,
                    &snapshot,
                    &mut kinds,
                );
            }

            let mut local_struct_instances = HashMap::<String, String>::new();
            let mut local_struct_array_roots = HashMap::<String, String>::new();
            let mut local_proc_array_roots = HashMap::<String, InferredProcArrayParam>::new();
            let mut local_array_bindings = HashMap::<String, InferredArrayParam>::new();
            let mut local_buffer_bindings = HashMap::<String, InferredBufferBinding>::new();

            if let Some(explicit_structs) = declared_struct_params.get(&def.name) {
                for (idx, explicit) in explicit_structs.iter().enumerate() {
                    if let (Some(struct_name), Some(param)) =
                        (explicit.as_ref(), def.params.get(idx))
                    {
                        crate::register_struct_instance_and_array_roots(
                            &param.name,
                            struct_name,
                            struct_defs,
                            &mut local_struct_instances,
                            &mut local_struct_array_roots,
                        );
                    }
                }
            }

            if let Some(explicit_buffers) = declared_buffer_params.get(&def.name) {
                for (idx, explicit) in explicit_buffers.iter().enumerate() {
                    if let (Some((elem_ty, channels)), Some(param)) =
                        (explicit.as_ref(), def.params.get(idx))
                    {
                        local_buffer_bindings.insert(
                            param.name.clone(),
                            InferredBufferBinding {
                                candidates: vec![InferredBufferParam {
                                    elem_ty: *elem_ty,
                                    channels: channels.clone(),
                                }],
                                is_array: matches!(param.ty, Some(FnParamType::BufferArray { .. })),
                            },
                        );
                    }
                }
            }

            if let Some(inferred_for_def) = kinds.get(&def.name) {
                for (idx, inferred_kind) in inferred_for_def.iter().enumerate() {
                    let Some(param) = def.params.get(idx) else {
                        continue;
                    };
                    if let Some(inferred_struct_array) =
                        infer_struct_array_observation_from_param_slot(
                            inferred_kind,
                            &def.name,
                            &param.name,
                            errors,
                        )
                    {
                        local_struct_array_roots
                            .insert(param.name.clone(), inferred_struct_array.struct_name);
                    }
                    if let Some(inferred_proc_array) = infer_proc_array_observation_from_param_slot(
                        inferred_kind,
                        &def.name,
                        &param.name,
                        errors,
                    ) {
                        local_proc_array_roots.insert(param.name.clone(), inferred_proc_array);
                    }
                    if !inferred_kind.saw_arrays.is_empty() {
                        let inferred_array = infer_untyped_array_from_observations(
                            &def.name,
                            &param.name,
                            inferred_kind,
                            false,
                            errors,
                        )
                        .unwrap_or(InferredArrayParam {
                            elem_ty: PrimitiveType::F32,
                            len: 1,
                        });
                        local_array_bindings.insert(param.name.clone(), inferred_array);
                    }
                    if local_buffer_bindings.contains_key(&param.name) {
                        continue;
                    }
                    let has_struct_observations = !inferred_kind.saw_structs.is_empty();
                    let has_effective_buffer_observations = !inferred_kind.saw_arrays.is_empty()
                        || inferred_kind.saw_call_buffer
                        || (inferred_kind.saw_seeded_buffer && !has_struct_observations);
                    if has_effective_buffer_observations {
                        if let Some(inferred_buffer) =
                            infer_buffer_observation_from_param_slot(inferred_kind)
                        {
                            local_buffer_bindings.insert(
                                param.name.clone(),
                                InferredBufferBinding {
                                    candidates: vec![inferred_buffer],
                                    is_array: false,
                                },
                            );
                        }
                    }
                }
            }

            let mut merged_struct_array_roots = struct_array_roots.clone();
            merged_struct_array_roots.extend(local_struct_array_roots);
            let mut merged_proc_array_roots = proc_array_roots.clone();
            merged_proc_array_roots.extend(local_proc_array_roots);
            for stmt in &def.body {
                infer_stmt_calls(
                    stmt,
                    &local_struct_instances,
                    &merged_struct_array_roots,
                    &merged_proc_array_roots,
                    &mut local_array_bindings,
                    &mut local_buffer_bindings,
                    fn_signatures,
                    &mut kinds,
                    errors,
                );
            }
        }
        if kinds == snapshot {
            break;
        }
    }

    let mut out = HashMap::new();
    let mut synthesized = HashMap::new();

    for def in defs {
        let display_name = fn_signatures
            .get(&def.name)
            .and_then(|signature| signature.display_name.as_deref())
            .unwrap_or(&def.name);
        let mut typed = Vec::with_capacity(def.params.len());
        let inferred = kinds.get(&def.name);
        let explicit = declared_struct_params
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![None; def.params.len()]);
        let explicit_buffers = declared_buffer_params
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![None; def.params.len()]);
        let usage = field_usage
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![HashMap::new(); def.params.len()]);

        for idx in 0..def.params.len() {
            let inferred_kind = inferred
                .and_then(|v| v.get(idx))
                .cloned()
                .unwrap_or_default();
            let explicit_struct = explicit.get(idx).and_then(|s| s.as_ref());
            let explicit_buffer = explicit_buffers.get(idx).and_then(|s| s.as_ref());
            let param_name = def
                .params
                .get(idx)
                .map(|p| p.name.as_str())
                .unwrap_or("<param>");
            let param_diag = def
                .params
                .get(idx)
                .map(|p| DiagCtx::new(p.ty_loc.or(p.loc)))
                .unwrap_or_default();
            let usage_for_param = usage.get(idx).cloned().unwrap_or_default();
            let has_struct_usage =
                !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty();
            let has_direct_struct_usage = !inferred_kind.saw_structs.is_empty()
                || usage_for_param
                    .values()
                    .any(|usage| matches!(usage, StructFieldUsage::Scalar));
            let has_effective_buffer_usage = !inferred_kind.saw_arrays.is_empty()
                || inferred_kind.saw_call_buffer
                || (inferred_kind.saw_seeded_buffer && !has_struct_usage);
            let inferred_struct_array = infer_struct_array_observation_from_param_slot(
                &inferred_kind,
                display_name,
                param_name,
                errors,
            );
            let inferred_proc_array = infer_proc_array_observation_from_param_slot(
                &inferred_kind,
                display_name,
                param_name,
                errors,
            );

            if let Some(FnParamType::BufferArray { len, .. }) =
                def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                let (elem_ty, channels) = explicit_buffer
                    .cloned()
                    .unwrap_or((PrimitiveType::F32, TypedBufferChannels::Mono));
                typed.push(TypedFnParam::BufferArray {
                    elem_ty,
                    channels,
                    len: *len,
                });
                continue;
            }

            // Handle explicitly typed tuple params (e.g. `(f32, i32)`)
            if let Some(FnParamType::Tuple(elem_tys)) =
                def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                typed.push(TypedFnParam::Tuple {
                    elem_tys: elem_tys.clone(),
                });
                continue;
            }
            // Handle explicitly typed array params (e.g. `f32[]`, `f32[4]`)
            if let Some(FnParamType::Array(Some(prim))) =
                def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                typed.push(TypedFnParam::Array { elem_ty: *prim });
                continue;
            }
            if let Some(FnParamType::SizedArray {
                elem: Some(prim), ..
            }) = def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                typed.push(TypedFnParam::Array { elem_ty: *prim });
                continue;
            }
            if let Some(FnParamType::SizedArray {
                generic_name: Some(ref param_ty),
                size,
                ..
            }) = def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                if proc_types.contains(param_ty) {
                    let Some(len) = const_positive_usize_for_call_type(size) else {
                        push_semantic(
                            param_diag,
                            errors,
                            format!(
                                "function '{}' parameter '{}' processor-array length must be a positive compile-time integer",
                                display_name, param_name
                            ),
                        );
                        typed.push(TypedFnParam::ProcArray {
                            proc_name: param_ty.clone(),
                            len: 1,
                        });
                        continue;
                    };
                    typed.push(TypedFnParam::ProcArray {
                        proc_name: param_ty.clone(),
                        len,
                    });
                    continue;
                }
                if let Some(proc_array) = inferred_proc_array
                    .as_ref()
                    .filter(|array| array.proc_name == *param_ty)
                    .cloned()
                    .or_else(|| {
                        let len = const_positive_usize_for_call_type(size)?;
                        proc_array_roots
                            .values()
                            .find(|array| array.proc_name == *param_ty && array.len == len)
                            .cloned()
                    })
                {
                    typed.push(TypedFnParam::ProcArray {
                        proc_name: proc_array.proc_name,
                        len: proc_array.len,
                    });
                    continue;
                }
                if !def.type_params.contains(param_ty) && struct_defs.contains_key(param_ty) {
                    typed.push(TypedFnParam::StructArray {
                        struct_name: param_ty.clone(),
                    });
                    continue;
                }
                push_semantic(
                    param_diag,
                    errors,
                    format!(
                        "function '{}' parameter '{}' uses unresolved generic array element type '{}'",
                        display_name, param_name, param_ty
                    ),
                );
                typed.push(TypedFnParam::Array {
                    elem_ty: PrimitiveType::F32,
                });
                continue;
            }
            if let Some(FnParamType::ArrayGeneric(param_ty)) =
                def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                if let Some(proc_array) = inferred_proc_array
                    .as_ref()
                    .filter(|array| array.proc_name == *param_ty)
                {
                    typed.push(TypedFnParam::ProcArray {
                        proc_name: proc_array.proc_name.clone(),
                        len: proc_array.len,
                    });
                    continue;
                }
                if !def.type_params.contains(param_ty) && struct_defs.contains_key(param_ty) {
                    typed.push(TypedFnParam::StructArray {
                        struct_name: param_ty.clone(),
                    });
                    continue;
                }
                push_semantic(
                    param_diag,
                    errors,
                    format!(
                        "function '{}' parameter '{}' uses unresolved generic array element type '{}'",
                        display_name, param_name, param_ty
                    ),
                );
                typed.push(TypedFnParam::Array {
                    elem_ty: PrimitiveType::F32,
                });
                continue;
            }
            // Handle bare buffer params — treat like untyped buffer for inference
            // (monomorphization resolves the concrete type at call sites)
            if let Some(FnParamType::BareBuffer) = def.params.get(idx).and_then(|p| p.ty.as_ref()) {
                // Fall through to buffer inference below by marking as buffer-like
                // If inference found buffer observations, use them; otherwise default
                let inferred_buffer = infer_untyped_buffer_from_observations(
                    display_name,
                    param_name,
                    &inferred_kind,
                    true,
                    errors,
                )
                .unwrap_or(InferredBufferParam {
                    elem_ty: PrimitiveType::F32,
                    channels: TypedBufferChannels::Mono,
                });
                typed.push(TypedFnParam::Buffer {
                    elem_ty: inferred_buffer.elem_ty,
                    channels: inferred_buffer.channels,
                });
                continue;
            }
            // Handle untyped array params (`[]`) — infer element type from usage
            if let Some(FnParamType::Array(None)) = def.params.get(idx).and_then(|p| p.ty.as_ref())
            {
                let inferred_array = infer_untyped_array_from_observations(
                    display_name,
                    param_name,
                    &inferred_kind,
                    true,
                    errors,
                )
                .unwrap_or(InferredArrayParam {
                    elem_ty: PrimitiveType::F32,
                    len: 1,
                });
                typed.push(TypedFnParam::Array {
                    elem_ty: inferred_array.elem_ty,
                });
                continue;
            }

            if let Some((elem_ty, channels)) = explicit_buffer {
                if !inferred_kind.saw_arrays.is_empty() {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as array",
                            display_name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                    );
                }
                if inferred_kind.saw_scalar {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as scalar",
                            display_name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                    );
                }
                if has_struct_usage {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as struct",
                            display_name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                    );
                }
                typed.push(TypedFnParam::Buffer {
                    elem_ty: *elem_ty,
                    channels: channels.clone(),
                });
                continue;
            }

            if has_effective_buffer_usage {
                if has_struct_usage {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as struct and buffer",
                            display_name, param_name
                        ),
                    );
                }
                let inferred_buffer = infer_untyped_buffer_from_observations(
                    display_name,
                    param_name,
                    &inferred_kind,
                    true,
                    errors,
                )
                .unwrap_or(InferredBufferParam {
                    elem_ty: PrimitiveType::F32,
                    channels: TypedBufferChannels::Mono,
                });
                typed.push(TypedFnParam::Buffer {
                    elem_ty: inferred_buffer.elem_ty,
                    channels: inferred_buffer.channels,
                });
                continue;
            }

            if let Some(struct_name) = explicit_struct {
                if inferred_kind.saw_scalar {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as scalar",
                            display_name, param_name, struct_name
                        ),
                    );
                }
                if !inferred_kind.saw_arrays.is_empty() {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as array",
                            display_name, param_name, struct_name
                        ),
                    );
                }
                for observed in &inferred_kind.saw_structs {
                    if observed != struct_name {
                        push_semantic(
                            param_diag,
                            errors,
                            format!(
                                "function '{}' parameter '{}' is explicitly '{}' but is called with '{}'",
                                display_name, param_name, struct_name, observed
                            ),
                        );
                    }
                }
                typed.push(TypedFnParam::Struct {
                    struct_name: struct_name.clone(),
                });
                continue;
            }

            if let Some(inferred_struct_array) = inferred_struct_array {
                if inferred_kind.saw_scalar {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and struct array",
                            display_name, param_name
                        ),
                    );
                }
                if !inferred_kind.saw_arrays.is_empty() {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as primitive array and struct array",
                            display_name, param_name
                        ),
                    );
                }
                if has_effective_buffer_usage {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as buffer and struct array",
                            display_name, param_name
                        ),
                    );
                }
                if has_direct_struct_usage {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as struct and struct array",
                            display_name, param_name
                        ),
                    );
                }
                typed.push(TypedFnParam::StructArray {
                    struct_name: inferred_struct_array.struct_name,
                });
                continue;
            }

            if let Some(inferred_proc_array) = inferred_proc_array {
                if inferred_kind.saw_scalar {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and processor array",
                            display_name, param_name
                        ),
                    );
                }
                if !inferred_kind.saw_arrays.is_empty() {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as primitive array and processor array",
                            display_name, param_name
                        ),
                    );
                }
                if has_effective_buffer_usage {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as buffer and processor array",
                            display_name, param_name
                        ),
                    );
                }
                if has_direct_struct_usage {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as struct and processor array",
                            display_name, param_name
                        ),
                    );
                }
                typed.push(TypedFnParam::ProcArray {
                    proc_name: inferred_proc_array.proc_name,
                    len: inferred_proc_array.len,
                });
                continue;
            }

            if has_struct_usage {
                if inferred_kind.saw_scalar {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and struct",
                            display_name, param_name
                        ),
                    );
                }

                let synthetic_name = synthetic_struct_param_name(&def.name, idx, param_name);
                let fields = build_structural_param_fields(
                    display_name,
                    param_name,
                    &usage_for_param,
                    &inferred_kind.saw_structs,
                    struct_defs,
                    errors,
                );
                synthesized.insert(synthetic_name.clone(), fields);
                typed.push(TypedFnParam::Struct {
                    struct_name: synthetic_name,
                });
            } else {
                let scalar_ty = match def.params.get(idx).and_then(|p| p.ty.as_ref()) {
                    Some(FnParamType::Primitive(prim)) => Some(*prim),
                    _ => None,
                };
                typed.push(TypedFnParam::Scalar { ty: scalar_ty });
            }
        }

        out.insert(def.name.clone(), typed);
    }

    (out, synthesized)
}

fn push_buffer_observation(slot: &mut InferredFnParam, obs: InferredBufferParam, hard: bool) {
    if hard {
        slot.saw_call_buffer = true;
    } else {
        slot.saw_seeded_buffer = true;
    }
    if slot
        .saw_buffers
        .iter()
        .any(|seen| seen.elem_ty == obs.elem_ty && seen.channels == obs.channels)
    {
        return;
    }
    slot.saw_buffers.push(obs);
}

fn infer_proc_array_observation_from_param_slot(
    inferred_kind: &InferredFnParam,
    def_name: &str,
    param_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<InferredProcArrayParam> {
    let first = inferred_kind.saw_proc_arrays.first()?.clone();
    for observed in inferred_kind.saw_proc_arrays.iter().skip(1) {
        if observed.proc_name != first.proc_name || observed.len != first.len {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "function '{}' parameter '{}' is called with incompatible processor arrays ('{}'[{}] vs '{}'[{}])",
                    def_name,
                    param_name,
                    first.proc_name,
                    first.len,
                    observed.proc_name,
                    observed.len
                ),
            );
            break;
        }
    }
    Some(first)
}

fn infer_struct_array_observation_from_param_slot(
    inferred_kind: &InferredFnParam,
    def_name: &str,
    param_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<InferredStructArrayParam> {
    let first = inferred_kind.saw_struct_arrays.first()?.clone();
    for observed in inferred_kind.saw_struct_arrays.iter().skip(1) {
        if observed.struct_name != first.struct_name {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "function '{}' parameter '{}' is called with incompatible struct arrays ('{}[]' vs '{}[]')",
                    def_name, param_name, first.struct_name, observed.struct_name
                ),
            );
            break;
        }
    }
    Some(first)
}

fn callee_buffer_param_requirement(
    callee: &str,
    param_idx: usize,
    declared_buffer_params: &HashMap<String, Vec<Option<(PrimitiveType, TypedBufferChannels)>>>,
    snapshot: &HashMap<String, Vec<InferredFnParam>>,
) -> Option<InferredBufferParam> {
    if let Some(Some((elem_ty, channels))) = declared_buffer_params
        .get(callee)
        .and_then(|params| params.get(param_idx))
    {
        return Some(InferredBufferParam {
            elem_ty: *elem_ty,
            channels: channels.clone(),
        });
    }
    snapshot
        .get(callee)
        .and_then(|params| params.get(param_idx))
        .and_then(infer_buffer_observation_from_param_slot)
}

fn propagate_stmt_callee_buffer_requirements_to_params(
    stmt: &Stmt,
    caller_name: &str,
    caller_param_index: &HashMap<String, usize>,
    fn_signatures: &HashMap<String, FnSignature>,
    declared_buffer_params: &HashMap<String, Vec<Option<(PrimitiveType, TypedBufferChannels)>>>,
    snapshot: &HashMap<String, Vec<InferredFnParam>>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                propagate_expr_callee_buffer_requirements_to_params(
                    index,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
            propagate_expr_callee_buffer_requirements_to_params(
                expr,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            propagate_expr_callee_buffer_requirements_to_params(
                expr,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
        }
        Stmt::Print { values, .. } => {
            for value in values {
                propagate_expr_callee_buffer_requirements_to_params(
                    value,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            propagate_expr_callee_buffer_requirements_to_params(
                cond,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
            for nested in then_branch {
                propagate_stmt_callee_buffer_requirements_to_params(
                    nested,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
            for nested in else_branch {
                propagate_stmt_callee_buffer_requirements_to_params(
                    nested,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            propagate_expr_callee_buffer_requirements_to_params(
                start,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
            propagate_expr_callee_buffer_requirements_to_params(
                end,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
            if let Some(step_expr) = step {
                propagate_expr_callee_buffer_requirements_to_params(
                    step_expr,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
            for nested in body {
                propagate_stmt_callee_buffer_requirements_to_params(
                    nested,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            propagate_expr_callee_buffer_requirements_to_params(
                cond,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
            for nested in body {
                propagate_stmt_callee_buffer_requirements_to_params(
                    nested,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn propagate_expr_callee_buffer_requirements_to_params(
    expr: &Expr,
    caller_name: &str,
    caller_param_index: &HashMap<String, usize>,
    fn_signatures: &HashMap<String, FnSignature>,
    declared_buffer_params: &HashMap<String, Vec<Option<(PrimitiveType, TypedBufferChannels)>>>,
    snapshot: &HashMap<String, Vec<InferredFnParam>>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
) {
    match expr {
        Expr::Number { .. }
        | Expr::Int { .. }
        | Expr::Bool { .. }
        | Expr::Var { .. }
        | Expr::ArrayCtor { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                propagate_expr_callee_buffer_requirements_to_params(
                    value,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Expr::Index { index, .. } => {
            propagate_expr_callee_buffer_requirements_to_params(
                index,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                propagate_expr_callee_buffer_requirements_to_params(
                    coordinate,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            propagate_expr_callee_buffer_requirements_to_params(
                lhs,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
            propagate_expr_callee_buffer_requirements_to_params(
                rhs,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            propagate_expr_callee_buffer_requirements_to_params(
                expr,
                caller_name,
                caller_param_index,
                fn_signatures,
                declared_buffer_params,
                snapshot,
                kinds,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                propagate_expr_callee_buffer_requirements_to_params(
                    arg,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(sig) = fn_signatures.get(name) {
                let mut bind_errors = Vec::new();
                let resolved = resolve_call_args(
                    args,
                    &sig.params,
                    &sig.defaults,
                    false,
                    false,
                    &format!("function '{name}' call"),
                    &mut bind_errors,
                );
                if bind_errors.is_empty() {
                    for (param_idx, arg) in resolved.into_iter().enumerate() {
                        let Some(Expr::Var { name: symbol, .. }) = arg else {
                            continue;
                        };
                        let Some(caller_param_idx) = caller_param_index.get(symbol).copied() else {
                            continue;
                        };
                        let Some(requirement) = callee_buffer_param_requirement(
                            name,
                            param_idx,
                            declared_buffer_params,
                            snapshot,
                        ) else {
                            continue;
                        };
                        if let Some(caller_slots) = kinds.get_mut(caller_name) {
                            if let Some(slot) = caller_slots.get_mut(caller_param_idx) {
                                push_buffer_observation(slot, requirement, true);
                            }
                        }
                    }
                }
            }
            for arg in args {
                propagate_expr_callee_buffer_requirements_to_params(
                    &arg.expr,
                    caller_name,
                    caller_param_index,
                    fn_signatures,
                    declared_buffer_params,
                    snapshot,
                    kinds,
                );
            }
        }
    }
}

fn collect_declared_struct_param_types(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<Option<String>>> {
    let mut out = HashMap::new();
    for def in defs {
        let display_name = fn_signatures
            .get(&def.name)
            .and_then(|signature| signature.display_name.as_deref())
            .unwrap_or(&def.name);
        let mut param_structs = vec![None; def.params.len()];
        for (idx, param) in def.params.iter().enumerate() {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if def.type_params.contains(struct_name) {
                    // This is a generic type parameter, not a struct — skip.
                } else if !struct_defs.contains_key(struct_name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "function '{}' parameter '{}' references unknown struct '{}'",
                            display_name, param.name, struct_name
                        ),
                        param.ty_loc.or(param.loc),
                    ));
                } else {
                    param_structs[idx] = Some(struct_name.clone());
                }
            }
        }

        if let Some(method_struct) = method_self_struct.get(&def.name) {
            if !param_structs.is_empty() {
                if let Some(existing) = param_structs[0].as_ref() {
                    if existing != method_struct {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "method '{}' self parameter is '{}' but annotation declares '{}'",
                                display_name, method_struct, existing
                            ),
                            def.params[0].ty_loc.or(def.params[0].loc),
                        ));
                    }
                }
                param_structs[0] = Some(method_struct.clone());
            }
        }

        out.insert(def.name.clone(), param_structs);
    }
    out
}

fn collect_declared_buffer_param_types(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<Option<(PrimitiveType, TypedBufferChannels)>>> {
    let mut out = HashMap::new();
    for def in defs {
        let display_name = fn_signatures
            .get(&def.name)
            .and_then(|signature| signature.display_name.as_deref())
            .unwrap_or(&def.name);
        let mut param_buffers = vec![None; def.params.len()];
        for (idx, param) in def.params.iter().enumerate() {
            if let Some(buffer_ty) = match &param.ty {
                Some(FnParamType::Buffer(buffer_ty))
                | Some(FnParamType::BufferArray {
                    buffer: buffer_ty, ..
                }) => Some(buffer_ty),
                _ => None,
            } {
                let channels = match &buffer_ty.channels {
                    BufferChannels::Mono => TypedBufferChannels::Mono,
                    BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                    BufferChannels::Static(expr) => {
                        let context = format!(
                            "function '{}' parameter '{}' buffer channels",
                            display_name, param.name
                        );
                        let Some(channels) = eval_data_size_expr(expr, options, &context, errors)
                        else {
                            continue;
                        };
                        let elem_ty = match buffer_ty.elem {
                            BufferElemType::Primitive(ty) => ty,
                            BufferElemType::Generic(_) => PrimitiveType::F32,
                        };
                        if !validate_buffer_static_channels(
                            channels,
                            elem_ty,
                            &context,
                            param.ty_loc.or(param.loc).into(),
                            errors,
                        ) {
                            continue;
                        }
                        if channels == 1 {
                            TypedBufferChannels::Mono
                        } else {
                            TypedBufferChannels::Static(channels)
                        }
                    }
                };
                let elem_ty = match buffer_ty.elem {
                    BufferElemType::Primitive(ty) => ty,
                    BufferElemType::Generic(ref param_ty) => {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "function '{}' parameter '{}' uses unresolved generic buffer element type '{}'",
                                display_name, param.name, param_ty
                            ),
                            param.ty_loc.or(param.loc),
                        ));
                        PrimitiveType::F32
                    }
                };
                param_buffers[idx] = Some((elem_ty, channels));
            }
        }
        out.insert(def.name.clone(), param_buffers);
    }
    out
}

fn format_buffer_type_name(elem_ty: PrimitiveType, channels: &TypedBufferChannels) -> String {
    let elem = match elem_ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    };
    match channels {
        TypedBufferChannels::Mono => format!("buffer<{elem}>"),
        TypedBufferChannels::Static(ch) => format!("buffer<{elem}[{ch}]>"),
        TypedBufferChannels::Dynamic => format!("buffer<{elem}[]>"),
    }
}

fn collect_def_param_field_usage(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    declared_struct_params: &HashMap<String, Vec<Option<String>>>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<HashMap<String, StructFieldUsage>>> {
    let mut out = HashMap::new();
    for def in defs {
        let display_name = fn_signatures
            .get(&def.name)
            .and_then(|signature| signature.display_name.as_deref())
            .unwrap_or(&def.name);
        let mut by_param = vec![HashMap::new(); def.params.len()];
        let param_index = def
            .params
            .iter()
            .enumerate()
            .map(|(idx, p)| (p.name.clone(), idx))
            .collect::<HashMap<_, _>>();
        let param_structs = declared_struct_params
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![None; def.params.len()]);
        for stmt in &def.body {
            collect_stmt_field_usage(
                stmt,
                display_name,
                &param_index,
                &param_structs,
                struct_defs,
                &mut by_param,
                errors,
            );
        }
        out.insert(def.name.clone(), by_param);
    }
    out
}

fn collect_stmt_field_usage(
    stmt: &Stmt,
    fn_name: &str,
    param_index: &HashMap<String, usize>,
    param_structs: &[Option<String>],
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    usage: &mut [HashMap<String, StructFieldUsage>],
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if let Some(param_idx) = param_index.get(base).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Scalar,
                                fn_name,
                                base,
                                errors,
                            );
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(param_idx) = param_index.get(root).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Array,
                                fn_name,
                                root,
                                errors,
                            );
                        }
                    }
                    collect_expr_field_usage(
                        index,
                        fn_name,
                        param_index,
                        param_structs,
                        struct_defs,
                        usage,
                        errors,
                    );
                }
                AssignTarget::Slice {
                    base,
                    selector,
                    channel,
                    start,
                    end,
                } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(param_idx) = param_index.get(root).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Array,
                                fn_name,
                                root,
                                errors,
                            );
                        }
                    }
                    for coordinate in [selector, channel, start, end].into_iter().flatten() {
                        collect_expr_field_usage(
                            coordinate,
                            fn_name,
                            param_index,
                            param_structs,
                            struct_defs,
                            usage,
                            errors,
                        );
                    }
                }
                AssignTarget::Tuple(names) => {
                    for name in names {
                        if let Some((base, field)) = split_simple_field_path(name) {
                            if let Some(param_idx) = param_index.get(base).copied() {
                                mark_param_field_usage(
                                    usage,
                                    param_idx,
                                    field,
                                    StructFieldUsage::Scalar,
                                    fn_name,
                                    base,
                                    errors,
                                );
                            }
                        }
                    }
                }
            }
            collect_expr_field_usage(
                expr,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_expr_field_usage(
                expr,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
        }
        Stmt::Print { values, .. } => {
            for value in values {
                collect_expr_field_usage(
                    value,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr_field_usage(
                cond,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
            for nested in then_branch {
                collect_stmt_field_usage(
                    nested,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
            for nested in else_branch {
                collect_stmt_field_usage(
                    nested,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_expr_field_usage(
                start,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
            collect_expr_field_usage(
                end,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
            if let Some(step_expr) = step {
                collect_expr_field_usage(
                    step_expr,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
            for nested in body {
                collect_stmt_field_usage(
                    nested,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_field_usage(
                cond,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
            for nested in body {
                collect_stmt_field_usage(
                    nested,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn collect_expr_field_usage(
    expr: &Expr,
    fn_name: &str,
    param_index: &HashMap<String, usize>,
    param_structs: &[Option<String>],
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    usage: &mut [HashMap<String, StructFieldUsage>],
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::ArrayCtor { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_expr_field_usage(
                    value,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Expr::Var { name, .. } => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if let Some(param_idx) = param_index.get(base).copied() {
                    let kind = param_structs
                        .get(param_idx)
                        .and_then(|s| s.as_deref())
                        .and_then(|struct_name| {
                            resolve_struct_field_decl(struct_name, field, struct_defs)
                        })
                        .map(|decl| {
                            if matches!(decl.ty, TypedFieldType::Array(_)) {
                                StructFieldUsage::Array
                            } else {
                                StructFieldUsage::Scalar
                            }
                        })
                        .unwrap_or(StructFieldUsage::Scalar);
                    mark_param_field_usage(usage, param_idx, field, kind, fn_name, base, errors);
                }
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(param_idx) = param_index.get(root).copied() {
                    mark_param_field_usage(
                        usage,
                        param_idx,
                        field,
                        StructFieldUsage::Array,
                        fn_name,
                        root,
                        errors,
                    );
                }
            }
            collect_expr_field_usage(
                index,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(param_idx) = param_index.get(root).copied() {
                    mark_param_field_usage(
                        usage,
                        param_idx,
                        field,
                        StructFieldUsage::Array,
                        fn_name,
                        root,
                        errors,
                    );
                }
            }
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_expr_field_usage(
                    coordinate,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            collect_expr_field_usage(
                lhs,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
            collect_expr_field_usage(
                rhs,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_expr_field_usage(
                expr,
                fn_name,
                param_index,
                param_structs,
                struct_defs,
                usage,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_field_usage(
                    arg,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                collect_expr_field_usage(
                    &arg.expr,
                    fn_name,
                    param_index,
                    param_structs,
                    struct_defs,
                    usage,
                    errors,
                );
            }
        }
    }
}

fn mark_param_field_usage(
    usage: &mut [HashMap<String, StructFieldUsage>],
    param_idx: usize,
    field: &str,
    kind: StructFieldUsage,
    fn_name: &str,
    param_name: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(map) = usage.get_mut(param_idx) else {
        return;
    };
    if let Some(existing) = map.get(field).copied() {
        if existing != kind {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "function '{}' parameter '{}' uses field '{}' both as scalar and array",
                    fn_name, param_name, field
                ),
            );
        }
        return;
    }
    map.insert(field.to_owned(), kind);
}

pub(crate) fn split_simple_field_path(name: &str) -> Option<(&str, &str)> {
    crate::split_root_field_path(name)
}

fn synthetic_struct_param_name(def_name: &str, idx: usize, param_name: &str) -> String {
    fn sanitize(input: &str) -> String {
        input
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    format!(
        "__onda_struct_any_{}_{}_{}",
        sanitize(def_name),
        idx,
        sanitize(param_name)
    )
}

fn build_structural_param_fields(
    fn_name: &str,
    param_name: &str,
    usage: &HashMap<String, StructFieldUsage>,
    observed_structs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedStructField> {
    let mut field_names = usage.keys().cloned().collect::<Vec<_>>();
    field_names.sort();

    let mut observed = observed_structs.iter().cloned().collect::<Vec<_>>();
    observed.sort();

    let mut out = Vec::with_capacity(field_names.len());

    for field_name in field_names {
        let required_kind = usage
            .get(&field_name)
            .copied()
            .unwrap_or(StructFieldUsage::Scalar);
        let mut resolved_ty: Option<TypedFieldType> = None;
        let mut resolved_data_elem_ty: Option<Option<PrimitiveType>> = None;
        let mut resolved_data_elem_struct: Option<Option<String>> = None;

        for struct_name in &observed {
            let Some(fields) = struct_defs.get(struct_name) else {
                continue;
            };
            let Some(found) = fields.iter().find(|f| f.name == field_name) else {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "function '{}' parameter '{}' requires field '{}' but struct '{}' does not define it",
                        fn_name, param_name, field_name, struct_name
                    ),
                );
                continue;
            };

            let (candidate, candidate_data_elem_ty, candidate_data_elem_struct) = match (
                required_kind,
                &found.ty,
            ) {
                (StructFieldUsage::Scalar, TypedFieldType::Scalar(prim)) => {
                    (TypedFieldType::Scalar(*prim), None, None)
                }
                (StructFieldUsage::Scalar, TypedFieldType::Array(_)) => {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as scalar but struct '{}' defines it as array",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                    );
                    continue;
                }
                (StructFieldUsage::Array, TypedFieldType::Array(len)) => (
                    TypedFieldType::Array(*len),
                    found.array_elem_ty,
                    found.array_elem_struct.clone(),
                ),
                (StructFieldUsage::Array, TypedFieldType::Scalar(_)) => {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as array but struct '{}' defines it as scalar",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                    );
                    continue;
                }
                (_, TypedFieldType::Struct) => {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as value but struct '{}' defines it as nested struct",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                    );
                    continue;
                }
                (_, TypedFieldType::Tuple(_)) => {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' directly but struct '{}' defines it as tuple",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                    );
                    continue;
                }
            };

            if let Some(existing) = &resolved_ty {
                let existing_data_elem_ty = resolved_data_elem_ty.flatten();
                let existing_data_elem_struct = resolved_data_elem_struct.clone().unwrap_or(None);
                if *existing != candidate
                    || existing_data_elem_ty != candidate_data_elem_ty
                    || existing_data_elem_struct != candidate_data_elem_struct
                {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "function '{}' parameter '{}' field '{}' resolves to incompatible types across structs",
                            fn_name, param_name, field_name
                        ),
                    );
                }
            } else {
                resolved_ty = Some(candidate);
                resolved_data_elem_ty = Some(candidate_data_elem_ty);
                resolved_data_elem_struct = Some(candidate_data_elem_struct);
            }
        }

        let ty = if let Some(resolved) = resolved_ty {
            resolved
        } else {
            match required_kind {
                StructFieldUsage::Scalar => TypedFieldType::Scalar(PrimitiveType::F32),
                StructFieldUsage::Array => TypedFieldType::Array(1),
            }
        };
        let array_elem_ty = resolved_data_elem_ty.flatten();
        let array_elem_struct = resolved_data_elem_struct.unwrap_or(None);

        out.push(TypedStructField {
            name: field_name,
            ty,
            default: None,
            struct_name: None,
            array_elem_ty,
            array_elem_struct,
        });
    }

    out
}

pub(crate) fn param_struct_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::Struct { struct_name } = kind {
            out.insert(name.clone(), struct_name.clone());
        }
    }
    out
}

pub(crate) fn param_struct_array_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::StructArray { struct_name } = kind {
            out.insert(name.clone(), struct_name.clone());
        }
    }
    out
}

pub(crate) fn param_proc_array_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, ProcNestedArrayState> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::ProcArray { proc_name, len } = kind {
            out.insert(
                name.clone(),
                ProcNestedArrayState {
                    proc_name: proc_name.clone(),
                    size_expr: Expr::int(*len as i64),
                },
            );
        }
    }
    out
}

pub(crate) fn param_buffer_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, (PrimitiveType, TypedBufferChannels, usize, bool)> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::Buffer { elem_ty, channels } = kind {
            out.insert(name.clone(), (*elem_ty, channels.clone(), 1, false));
        } else if let TypedFnParam::BufferArray {
            elem_ty,
            channels,
            len,
        } = kind
        {
            out.insert(name.clone(), (*elem_ty, channels.clone(), *len, true));
        }
    }
    out
}

pub(crate) fn param_array_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, PrimitiveType> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::Array { elem_ty } = kind {
            out.insert(name.clone(), *elem_ty);
        }
    }
    out
}

fn merge_inferred_buffer_channels(
    lhs: &TypedBufferChannels,
    rhs: &TypedBufferChannels,
) -> TypedBufferChannels {
    use TypedBufferChannels::{Dynamic, Mono, Static};
    match (lhs, rhs) {
        (Mono, Mono) => Mono,
        (Static(a), Static(b)) if a == b => {
            if *a == 1 {
                Mono
            } else {
                Static(*a)
            }
        }
        (Mono, Static(1)) | (Static(1), Mono) => Mono,
        (Dynamic, _) | (_, Dynamic) => Dynamic,
        _ => Dynamic,
    }
}

fn infer_buffer_observation_from_param_slot(
    inferred: &InferredFnParam,
) -> Option<InferredBufferParam> {
    if inferred.saw_buffers.is_empty() && inferred.saw_arrays.is_empty() {
        return None;
    }
    let mut merged_channels = TypedBufferChannels::Mono;
    let mut merged_elem = PrimitiveType::F32;
    let mut initialized = false;

    for seen in &inferred.saw_buffers {
        if !initialized {
            merged_channels = seen.channels.clone();
            merged_elem = seen.elem_ty;
            initialized = true;
            continue;
        }
        merged_elem = match (merged_elem, seen.elem_ty) {
            (PrimitiveType::F32, PrimitiveType::F64) | (PrimitiveType::F64, PrimitiveType::F32) => {
                PrimitiveType::F64
            }
            (lhs, rhs) if lhs == rhs => lhs,
            (lhs, _) => lhs,
        };
        merged_channels = merge_inferred_buffer_channels(&merged_channels, &seen.channels);
    }

    for seen in &inferred.saw_arrays {
        if !initialized {
            merged_channels = TypedBufferChannels::Mono;
            merged_elem = seen.elem_ty;
            initialized = true;
            continue;
        }
        merged_elem = match (merged_elem, seen.elem_ty) {
            (PrimitiveType::F32, PrimitiveType::F64) | (PrimitiveType::F64, PrimitiveType::F32) => {
                PrimitiveType::F64
            }
            (lhs, rhs) if lhs == rhs => lhs,
            (lhs, _) => lhs,
        };
        merged_channels =
            merge_inferred_buffer_channels(&merged_channels, &TypedBufferChannels::Mono);
    }

    Some(InferredBufferParam {
        elem_ty: merged_elem,
        channels: merged_channels,
    })
}

fn infer_untyped_buffer_from_observations(
    _fn_name: &str,
    _param_name: &str,
    inferred: &InferredFnParam,
    _report_errors: bool,
    _errors: &mut Vec<Diagnostic>,
) -> Option<InferredBufferParam> {
    infer_buffer_observation_from_param_slot(inferred)
}

fn infer_untyped_array_from_observations(
    _fn_name: &str,
    _param_name: &str,
    inferred: &InferredFnParam,
    _report_errors: bool,
    _errors: &mut Vec<Diagnostic>,
) -> Option<InferredArrayParam> {
    if inferred.saw_arrays.is_empty() {
        return None;
    }
    let first = inferred.saw_arrays[0].clone();
    let mut merged_elem = first.elem_ty;
    let mut merged_len = first.len;
    for seen in inferred.saw_arrays.iter().skip(1) {
        merged_elem = match (merged_elem, seen.elem_ty) {
            (PrimitiveType::F32, PrimitiveType::F64) | (PrimitiveType::F64, PrimitiveType::F32) => {
                PrimitiveType::F64
            }
            (lhs, rhs) if lhs == rhs => lhs,
            (lhs, _) => lhs,
        };
        merged_len = merged_len.max(seen.len);
    }
    Some(InferredArrayParam {
        elem_ty: merged_elem,
        len: merged_len,
    })
}

pub(crate) fn validate_default_expr(expr: &Expr, errors: &mut Vec<Diagnostic>, context: &str) {
    with_expr_diag_context(expr, |expr_diag| match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::Tuple { values, .. } => {
            for value in values {
                validate_default_expr(value, errors, context);
            }
        }
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                validate_default_expr(value, errors, context);
            }
            push_semantic(
                expr_diag,
                errors,
                "array literals are only allowed in typed array declarations and parameter defaults",
            );
        }
        Expr::Var { name, .. } => {
            if !is_builtin_constant_name(name) {
                push_semantic(
                    expr_diag,
                    errors,
                    format!("{context} default expression uses non-constant symbol '{name}'"),
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_default_expr(expr, errors, context);
        }
        Expr::Logical { lhs, rhs, .. } => {
            validate_default_expr(lhs, errors, context);
            validate_default_expr(rhs, errors, context);
        }
        Expr::Binary { lhs, rhs, .. } | Expr::Compare { lhs, rhs, .. } => {
            validate_default_expr(lhs, errors, context);
            validate_default_expr(rhs, errors, context);
        }
        Expr::UserCall { args, .. } => {
            // Allow T(constant) in defaults — will be resolved to a cast during mono.
            for arg in args {
                validate_default_expr(&arg.expr, errors, context);
            }
        }
        _ => {
            push_semantic(
                expr_diag,
                errors,
                format!("{context} default expression must be constant"),
            );
        }
    })
}

pub(crate) fn can_implicitly_assign(src: PrimitiveType, dst: PrimitiveType) -> bool {
    if src == dst {
        return true;
    }
    matches!(
        (src, dst),
        (PrimitiveType::I32, PrimitiveType::I64)
            | (PrimitiveType::I32, PrimitiveType::F32)
            | (PrimitiveType::I32, PrimitiveType::F64)
            | (PrimitiveType::I64, PrimitiveType::F64)
            | (PrimitiveType::F32, PrimitiveType::F64)
    )
}

pub(crate) fn merge_numeric_types(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (F64, I32)
        | (I32, F64)
        | (F64, I64)
        | (I64, F64)
        | (F64, F32)
        | (F32, F64)
        | (F64, F64) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) | (F32, I64) | (I64, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context} requires numeric operands, got {:?} and {:?}",
                    lhs, rhs
                ),
            );
            None
        }
    }
}

pub(crate) fn merge_inferred_return_types(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (a, b) if a == b => Some(a),
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, I64) | (I64, F32) => Some(F64),
        (F32, I32) | (I32, F32) => Some(F32),
        (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}
