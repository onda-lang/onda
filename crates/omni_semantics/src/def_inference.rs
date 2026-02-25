use std::collections::{HashMap, HashSet};

use omni_frontend::{
    AssignTarget, BufferChannels, BufferElemType, BuiltinFn, CallArg, Diagnostic, Expr,
    FnParamType, FunctionDef, PrimitiveType, Stmt,
};

mod return_inference;
pub(crate) use return_inference::*;
mod call_inference;
use call_inference::infer_stmt_calls;
pub(crate) use call_inference::resolve_call_args;

use crate::builtins::{
    eval_data_size_expr, is_builtin_constant_name, is_internal_buffer_2d_fn,
    parse_buffer_chans_instance_base, parse_data_len_instance_base,
};
use crate::{
    with_stmt_diag_context, AnalysisOptions, FnSignature, TypedBufferChannels, TypedFieldType,
    TypedFnParam, TypedStructField,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StructFieldUsage {
    Scalar,
    Data,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct InferredFnParam {
    saw_scalar: bool,
    saw_structs: HashSet<String>,
    saw_buffers: Vec<InferredBufferParam>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InferredBufferParam {
    pub(crate) elem_ty: PrimitiveType,
    pub(crate) channels: TypedBufferChannels,
}

pub(crate) fn infer_def_param_kinds(
    defs: &[FunctionDef],
    init: &[Stmt],
    block_stmts: &[Stmt],
    sample: &[Stmt],
    struct_instances: &HashMap<String, String>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (
    HashMap<String, Vec<TypedFnParam>>,
    HashMap<String, Vec<TypedStructField>>,
) {
    let declared_struct_params =
        collect_declared_struct_param_types(defs, method_self_struct, struct_defs, errors);
    let declared_buffer_params = collect_declared_buffer_param_types(defs, options, errors);
    let field_usage = collect_def_param_field_usage(defs, errors);

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
                        dst.saw_structs.insert(struct_name.clone());
                    }
                }
            }
        }
    }

    for stmt in init {
        infer_stmt_calls(
            stmt,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }
    for stmt in block_stmts {
        infer_stmt_calls(
            stmt,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }
    for stmt in sample {
        infer_stmt_calls(
            stmt,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }

    // Propagate inferred def parameter kinds through def-to-def calls.
    for _ in 0..defs.len().saturating_add(1) {
        let snapshot = kinds.clone();
        for def in defs {
            let mut local_struct_instances = HashMap::<String, String>::new();
            let mut local_buffer_bindings = HashMap::<String, Vec<InferredBufferParam>>::new();

            if let Some(explicit_structs) = declared_struct_params.get(&def.name) {
                for (idx, explicit) in explicit_structs.iter().enumerate() {
                    if let (Some(struct_name), Some(param)) =
                        (explicit.as_ref(), def.params.get(idx))
                    {
                        local_struct_instances.insert(param.name.clone(), struct_name.clone());
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
                            vec![InferredBufferParam {
                                elem_ty: *elem_ty,
                                channels: channels.clone(),
                            }],
                        );
                    }
                }
            }

            if let Some(inferred_for_def) = kinds.get(&def.name) {
                for (idx, inferred_kind) in inferred_for_def.iter().enumerate() {
                    let Some(param) = def.params.get(idx) else {
                        continue;
                    };
                    if local_buffer_bindings.contains_key(&param.name) {
                        continue;
                    }
                    if !inferred_kind.saw_buffers.is_empty() {
                        local_buffer_bindings
                            .insert(param.name.clone(), inferred_kind.saw_buffers.clone());
                    }
                }
            }

            for stmt in &def.body {
                infer_stmt_calls(
                    stmt,
                    &local_struct_instances,
                    &local_buffer_bindings,
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
            let usage_for_param = usage.get(idx).cloned().unwrap_or_default();

            if let Some((elem_ty, channels)) = explicit_buffer {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as scalar",
                            def.name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                        0,
                        0,
                    ));
                }
                if !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as struct",
                            def.name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                        0,
                        0,
                    ));
                }
                typed.push(TypedFnParam::Buffer {
                    elem_ty: *elem_ty,
                    channels: channels.clone(),
                });
                continue;
            }

            if !inferred_kind.saw_buffers.is_empty() {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and buffer",
                            def.name, param_name
                        ),
                        0,
                        0,
                    ));
                }
                if !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is used both as struct and buffer",
                            def.name, param_name
                        ),
                        0,
                        0,
                    ));
                }
                let inferred_buffer = infer_untyped_buffer_from_observations(
                    &def.name,
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
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as scalar",
                            def.name, param_name, struct_name
                        ),
                        0,
                        0,
                    ));
                }
                for observed in &inferred_kind.saw_structs {
                    if observed != struct_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "function '{}' parameter '{}' is explicitly '{}' but is called with '{}'",
                                def.name, param_name, struct_name, observed
                            ),
                            0,
                            0,
                        ));
                    }
                }
                typed.push(TypedFnParam::Struct {
                    struct_name: struct_name.clone(),
                });
                continue;
            }

            if !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty() {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and struct",
                            def.name, param_name
                        ),
                        0,
                        0,
                    ));
                }

                let synthetic_name = synthetic_struct_param_name(&def.name, idx, param_name);
                let fields = build_structural_param_fields(
                    &def.name,
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
                typed.push(TypedFnParam::Scalar);
            }
        }

        out.insert(def.name.clone(), typed);
    }

    (out, synthesized)
}

fn collect_declared_struct_param_types(
    defs: &[FunctionDef],
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<Option<String>>> {
    let mut out = HashMap::new();
    for def in defs {
        let mut param_structs = vec![None; def.params.len()];
        for (idx, param) in def.params.iter().enumerate() {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if !struct_defs.contains_key(struct_name) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' references unknown struct '{}'",
                            def.name, param.name, struct_name
                        ),
                        0,
                        0,
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
                        errors.push(Diagnostic::semantic(
                            format!(
                                "method '{}' self parameter is '{}' but annotation declares '{}'",
                                def.name, method_struct, existing
                            ),
                            0,
                            0,
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
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<Option<(PrimitiveType, TypedBufferChannels)>>> {
    let mut out = HashMap::new();
    for def in defs {
        let mut param_buffers = vec![None; def.params.len()];
        for (idx, param) in def.params.iter().enumerate() {
            if let Some(FnParamType::Buffer(buffer_ty)) = &param.ty {
                let channels = match &buffer_ty.channels {
                    BufferChannels::Mono => TypedBufferChannels::Mono,
                    BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                    BufferChannels::Static(expr) => {
                        let context = format!(
                            "function '{}' parameter '{}' buffer channels",
                            def.name, param.name
                        );
                        let Some(channels) = eval_data_size_expr(expr, options, &context, errors)
                        else {
                            continue;
                        };
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
                        errors.push(Diagnostic::semantic(
                            format!(
                                "function '{}' parameter '{}' uses unresolved generic buffer element type '{}'",
                                def.name, param.name, param_ty
                            ),
                            0,
                            0,
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
        TypedBufferChannels::Mono => format!("buffer[{elem}]"),
        TypedBufferChannels::Static(ch) => format!("buffer[{elem}[{ch}]]"),
        TypedBufferChannels::Dynamic => format!("buffer[{elem}[]]"),
    }
}

fn collect_def_param_field_usage(
    defs: &[FunctionDef],
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<HashMap<String, StructFieldUsage>>> {
    let mut out = HashMap::new();
    for def in defs {
        let mut by_param = vec![HashMap::new(); def.params.len()];
        let param_index = def
            .params
            .iter()
            .enumerate()
            .map(|(idx, p)| (p.name.clone(), idx))
            .collect::<HashMap<_, _>>();
        for stmt in &def.body {
            collect_stmt_field_usage(stmt, &def.name, &param_index, &mut by_param, errors);
        }
        out.insert(def.name.clone(), by_param);
    }
    out
}

fn collect_stmt_field_usage(
    stmt: &Stmt,
    fn_name: &str,
    param_index: &HashMap<String, usize>,
    usage: &mut [HashMap<String, StructFieldUsage>],
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
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
                                StructFieldUsage::Data,
                                fn_name,
                                root,
                                errors,
                            );
                        }
                    }
                    collect_expr_field_usage(index, fn_name, param_index, usage, errors);
                }
            }
            collect_expr_field_usage(expr, fn_name, param_index, usage, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_expr_field_usage(expr, fn_name, param_index, usage, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr_field_usage(cond, fn_name, param_index, usage, errors);
            for nested in then_branch {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
            for nested in else_branch {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_expr_field_usage(start, fn_name, param_index, usage, errors);
            collect_expr_field_usage(end, fn_name, param_index, usage, errors);
            if let Some(step_expr) = step {
                collect_expr_field_usage(step_expr, fn_name, param_index, usage, errors);
            }
            for nested in body {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_field_usage(cond, fn_name, param_index, usage, errors);
            for nested in body {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn collect_expr_field_usage(
    expr: &Expr,
    fn_name: &str,
    param_index: &HashMap<String, usize>,
    usage: &mut [HashMap<String, StructFieldUsage>],
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::DataCtor { .. } => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                collect_expr_field_usage(value, fn_name, param_index, usage, errors);
            }
        }
        Expr::Var(name) => {
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
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(param_idx) = param_index.get(root).copied() {
                    mark_param_field_usage(
                        usage,
                        param_idx,
                        field,
                        StructFieldUsage::Data,
                        fn_name,
                        root,
                        errors,
                    );
                }
            }
            collect_expr_field_usage(index, fn_name, param_index, usage, errors);
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            collect_expr_field_usage(lhs, fn_name, param_index, usage, errors);
            collect_expr_field_usage(rhs, fn_name, param_index, usage, errors);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            collect_expr_field_usage(expr, fn_name, param_index, usage, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_field_usage(arg, fn_name, param_index, usage, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            if let Expr::UserCall { name, .. } = expr {
                if let Some(base) = parse_data_len_instance_base(name) {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(param_idx) = param_index.get(root).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Data,
                                fn_name,
                                root,
                                errors,
                            );
                        }
                    }
                }
            }
            for arg in args {
                collect_expr_field_usage(&arg.expr, fn_name, param_index, usage, errors);
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
            errors.push(Diagnostic::semantic(
                format!(
                    "function '{}' parameter '{}' uses field '{}' both as scalar and Data",
                    fn_name, param_name, field
                ),
                0,
                0,
            ));
        }
        return;
    }
    map.insert(field.to_owned(), kind);
}

pub(crate) fn split_simple_field_path(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let second = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((first, second))
}

fn synthetic_struct_param_name(def_name: &str, idx: usize, param_name: &str) -> String {
    fn sanitize(input: &str) -> String {
        input
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    format!(
        "__omni_struct_any_{}_{}_{}",
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
                errors.push(Diagnostic::semantic(
                    format!(
                        "function '{}' parameter '{}' requires field '{}' but struct '{}' does not define it",
                        fn_name, param_name, field_name, struct_name
                    ),
                    0,
                    0,
                ));
                continue;
            };

            let (candidate, candidate_data_elem_ty, candidate_data_elem_struct) = match (
                required_kind,
                found.ty,
            ) {
                (StructFieldUsage::Scalar, TypedFieldType::Scalar(prim)) => {
                    (TypedFieldType::Scalar(prim), None, None)
                }
                (StructFieldUsage::Scalar, TypedFieldType::Data(_)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as scalar but struct '{}' defines it as Data",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                        0,
                        0,
                    ));
                    continue;
                }
                (StructFieldUsage::Data, TypedFieldType::Data(len)) => (
                    TypedFieldType::Data(len),
                    found.data_elem_ty,
                    found.data_elem_struct.clone(),
                ),
                (StructFieldUsage::Data, TypedFieldType::Scalar(_)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as Data but struct '{}' defines it as scalar",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                        0,
                        0,
                    ));
                    continue;
                }
            };

            if let Some(existing) = resolved_ty {
                let existing_data_elem_ty = resolved_data_elem_ty.flatten();
                let existing_data_elem_struct = resolved_data_elem_struct.clone().unwrap_or(None);
                if existing != candidate
                    || existing_data_elem_ty != candidate_data_elem_ty
                    || existing_data_elem_struct != candidate_data_elem_struct
                {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' field '{}' resolves to incompatible types across structs",
                            fn_name, param_name, field_name
                        ),
                        0,
                        0,
                    ));
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
                StructFieldUsage::Data => TypedFieldType::Data(1),
            }
        };
        let data_elem_ty = resolved_data_elem_ty.flatten();
        let data_elem_struct = resolved_data_elem_struct.unwrap_or(None);

        out.push(TypedStructField {
            name: field_name,
            ty,
            default: None,
            data_elem_ty,
            data_elem_struct,
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

pub(crate) fn param_buffer_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, (PrimitiveType, TypedBufferChannels)> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::Buffer { elem_ty, channels } = kind {
            out.insert(name.clone(), (*elem_ty, channels.clone()));
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

fn infer_untyped_buffer_from_observations(
    _fn_name: &str,
    _param_name: &str,
    inferred: &InferredFnParam,
    _report_errors: bool,
    _errors: &mut Vec<Diagnostic>,
) -> Option<InferredBufferParam> {
    if inferred.saw_buffers.is_empty() {
        return None;
    }
    let first = inferred.saw_buffers[0].clone();
    let mut merged_channels = first.channels.clone();
    let mut merged_elem = first.elem_ty;
    for seen in inferred.saw_buffers.iter().skip(1) {
        merged_elem = match (merged_elem, seen.elem_ty) {
            (PrimitiveType::F32, PrimitiveType::F64) | (PrimitiveType::F64, PrimitiveType::F32) => {
                PrimitiveType::F64
            }
            (lhs, rhs) if lhs == rhs => lhs,
            (lhs, _) => lhs,
        };
        merged_channels = merge_inferred_buffer_channels(&merged_channels, &seen.channels);
    }
    Some(InferredBufferParam {
        elem_ty: merged_elem,
        channels: merged_channels,
    })
}

pub(crate) fn validate_default_expr(expr: &Expr, errors: &mut Vec<Diagnostic>, context: &str) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                validate_default_expr(value, errors, context);
            }
            errors.push(Diagnostic::semantic(
                "array literals are only allowed in typed array declarations and parameter defaults",
                0,
                0,
            ));
        }
        Expr::Var(name) => {
            if !is_builtin_constant_name(name) {
                errors.push(Diagnostic::semantic(
                    format!("{context} default expression uses non-constant symbol '{name}'"),
                    0,
                    0,
                ));
            }
        }
        Expr::Binary { lhs, rhs, .. } | Expr::Compare { lhs, rhs, .. } => {
            validate_default_expr(lhs, errors, context);
            validate_default_expr(rhs, errors, context);
        }
        _ => {
            errors.push(Diagnostic::semantic(
                format!("{context} default expression must be constant"),
                0,
                0,
            ));
        }
    }
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
        (F32, I32) | (I32, F32) | (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} requires numeric operands, got {:?} and {:?}",
                    lhs, rhs
                ),
                0,
                0,
            ));
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
