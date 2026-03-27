use std::collections::{HashMap, HashSet};

use crate::*;
use omni_frontend::EventParamDecl;

mod generated_blocks;
mod generic_proc_rewrite;
mod global_proc_rewrite;
mod graph_lowering;
mod nested_paths;
mod nested_proc_lowering;
mod proc_local_defs;
mod shape_helpers;
use generated_blocks::*;
use generic_proc_rewrite::*;
use global_proc_rewrite::*;
pub(crate) use graph_lowering::*;
use nested_paths::*;
use nested_proc_lowering::*;
use proc_local_defs::*;
use shape_helpers::*;

const BUILTIN_PROC_INIT_EVENT_NAME: &str = "init";

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

fn is_builtin_proc_init_event_name(name: &str) -> bool {
    name == BUILTIN_PROC_INIT_EVENT_NAME
}

fn inject_builtin_proc_init_events(program: &mut Program, errors: &mut Vec<Diagnostic>) {
    for block in &mut program.blocks {
        let Block::Proc(proc) = block else {
            continue;
        };
        for event in proc
            .events
            .iter()
            .filter(|event| is_builtin_proc_init_event_name(&event.name))
        {
            push_semantic(
                DiagCtx::new(event.loc),
                errors,
                format!(
                    "processor '{}' event name '{}' is reserved for the builtin initializer event",
                    proc.name, BUILTIN_PROC_INIT_EVENT_NAME
                ),
            );
        }
        proc.events
            .retain(|event| !is_builtin_proc_init_event_name(&event.name));
        proc.events.push(EventDef {
            loc: proc.loc,
            name: BUILTIN_PROC_INIT_EVENT_NAME.to_owned(),
            params: Vec::new(),
            body: Vec::new(),
        });
    }
}

fn coerce_scalar_event_default(
    default_expr: &Expr,
    ty: PrimitiveType,
    context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    validate_default_expr(default_expr, errors, context);
    eval_typed_const_expr(
        default_expr,
        ty,
        options,
        context,
        is_float_type(ty),
        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
        errors,
    )
}

fn coerce_fixed_array_event_default(
    default_expr: &Expr,
    elem_ty: PrimitiveType,
    len: usize,
    context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<TypedConstValue>> {
    let expr_diag = DiagCtx::new(default_expr.loc());
    let Expr::ArrayLiteral { values, .. } = default_expr else {
        push_semantic(
            expr_diag,
            errors,
            format!("{context} default must be a fixed-size array literal"),
        );
        return None;
    };
    if values.len() != len {
        push_semantic(
            expr_diag,
            errors,
            format!(
                "{context} default expects {len} elements, got {}",
                values.len()
            ),
        );
        return None;
    }
    let mut coerced = Vec::with_capacity(len);
    for (idx, value) in values.iter().enumerate() {
        let Some(typed) = coerce_scalar_event_default(
            value,
            elem_ty,
            &format!("{context} default element {idx}"),
            options,
            errors,
        ) else {
            return None;
        };
        coerced.push(typed);
    }
    Some(coerced)
}

fn coerce_typed_event_default(
    param: &EventParamDecl,
    typed_ty: &TypedEventParamType,
    context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedEventParamDefault> {
    let default_expr = param.default.as_ref()?;
    match typed_ty {
        TypedEventParamType::Scalar(ty) => {
            coerce_scalar_event_default(default_expr, *ty, context, options, errors)
                .map(TypedEventParamDefault::Scalar)
        }
        TypedEventParamType::Array { elem, len } => {
            coerce_fixed_array_event_default(default_expr, *elem, *len, context, options, errors)
                .map(TypedEventParamDefault::Array)
        }
        TypedEventParamType::Slice { .. } => {
            push_semantic(
                DiagCtx::new(default_expr.loc()),
                errors,
                format!("{context} default is not supported for slice event params"),
            );
            None
        }
    }
}

fn validate_proc_event_default_expr(
    param: &EventParamDecl,
    len: Option<usize>,
    context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    let default_expr = param.default.as_ref()?;
    match &param.ty {
        EventParamType::Scalar(ty) => {
            coerce_scalar_event_default(default_expr, *ty, context, options, errors)?;
            Some(default_expr.clone())
        }
        EventParamType::Array { elem, .. } => {
            let Some(len) = len else {
                return None;
            };
            coerce_fixed_array_event_default(default_expr, *elem, len, context, options, errors)?;
            Some(default_expr.clone())
        }
        EventParamType::Slice { .. } | EventParamType::GenericSlice { .. } => {
            push_semantic(
                DiagCtx::new(default_expr.loc()),
                errors,
                format!("{context} default is not supported for slice event params"),
            );
            None
        }
    }
}

#[derive(Debug, Clone)]
struct ProcBaseShape {
    ins: Vec<String>,
    outs: Vec<String>,
    in_ports: Vec<ProcPortSpec>,
    param_specs: Vec<ProcParamSpec>,
    buffer_specs: Vec<ProcBufferSpec>,
    in_types: HashMap<String, PrimitiveType>,
    out_types: HashMap<String, PrimitiveType>,
    in_array_slots: HashMap<String, Vec<String>>,
    field_array_slots: HashMap<String, Vec<String>>,
    nested_proc_array_slots: HashMap<String, Vec<String>>,
    nested_proc_array_active_fields: HashMap<String, String>,
    state: ProcStateFields,
    fields: Vec<StructField>,
    field_names: HashSet<String>,
    array_field_names: HashSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcLoweringShape {
    pub(crate) ins: Vec<String>,
    pub(crate) outs: Vec<String>,
    pub(crate) in_ports: Vec<ProcPortSpec>,
    pub(crate) param_specs: Vec<ProcParamSpec>,
    pub(crate) buffer_specs: Vec<ProcBufferSpec>,
    pub(crate) in_types: HashMap<String, PrimitiveType>,
    pub(crate) out_types: HashMap<String, PrimitiveType>,
    pub(crate) in_array_slots: HashMap<String, Vec<String>>,
    pub(crate) field_array_slots: HashMap<String, Vec<String>>,
    pub(crate) nested_proc_array_slots: HashMap<String, Vec<String>>,
    pub(crate) nested_proc_array_active_fields: HashMap<String, String>,
    pub(crate) state: ProcStateFields,
    pub(crate) fields: Vec<StructField>,
    pub(crate) field_names: HashSet<String>,
    pub(crate) array_field_names: HashSet<String>,
    pub(crate) nested_fields: HashMap<String, HashSet<String>>,
}

struct ProcLoweringEnv {
    struct_defs_by_name: HashMap<String, StructDef>,
    proc_defs_by_name: HashMap<String, ProcessorDef>,
    proc_api: HashMap<String, ProcApi>,
    proc_order: Vec<String>,
    lowering_shapes: HashMap<String, ProcLoweringShape>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TopLevelProcRewriteMeta {
    pub(crate) global_proc_instances: HashMap<String, ProcCallInstance>,
    pub(crate) global_proc_array_slots: HashMap<String, Vec<String>>,
}

pub(crate) struct ProcessorDesugarResult {
    pub(crate) program: Program,
    pub(crate) def_sample_oversample_factors: HashMap<String, usize>,
    pub(crate) proc_step_oversample_meta: HashMap<String, ProcStepOversampleMeta>,
    pub(crate) proc_api: HashMap<String, ProcApi>,
    pub(crate) lowering_shapes: HashMap<String, ProcLoweringShape>,
    pub(crate) top_level_proc_rewrite: TopLevelProcRewriteMeta,
}

const ALLOWED_SAMPLE_OVERSAMPLE_FACTORS: &[i64] = &[1, 2, 4, 8, 16, 32, 64];

fn sanitize_runtime_symbol_component(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

pub(crate) fn runtime_proc_array_active_field_name(array_base: &str) -> String {
    format!(
        "__omni_proc_block_active_{}",
        sanitize_runtime_symbol_component(array_base)
    )
}

pub(crate) fn validated_sample_oversample_factor(
    factor_expr: Option<&Expr>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    let Some(expr) = factor_expr else {
        return 1;
    };
    let expr_diag = DiagCtx::new(expr.loc());

    match expr {
        Expr::Int { value, .. } => {
            if ALLOWED_SAMPLE_OVERSAMPLE_FACTORS.contains(value) {
                *value as usize
            } else {
                push_semantic(
                    expr_diag,
                    errors,
                    format!(
                        "{context} oversampling factor must be one of {{1,2,4,8,16,32,64}}; got {value}"
                    ),
                );
                1
            }
        }
        _ => {
            push_semantic(
                expr_diag,
                errors,
                format!(
                    "{context} oversampling factor must be an integer literal in {{1,2,4,8,16,32,64}}"
                ),
                );
            1
        }
    }
}

pub(crate) fn collect_runtime_state_roots(
    state_scalars: &HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
) -> HashSet<String> {
    state_scalars
        .keys()
        .chain(state_arrays.keys())
        .map(|name| runtime_symbol_root(name).to_owned())
        .collect::<HashSet<_>>()
}

pub(crate) fn internal_proc_index_call_signature(include_field_arg: bool) -> FnSignature {
    const PROC_INDEX_CALL_MAX_POSITIONAL_ARGS: usize = 16;

    let mut params = vec![
        PROC_INDEX_BASE_ARG.to_owned(),
        PROC_INDEX_EXPR_ARG.to_owned(),
    ];
    let mut defaults = vec![None, None];
    let mut param_types = vec![None, None];

    for idx in 0..PROC_INDEX_CALL_MAX_POSITIONAL_ARGS {
        params.push(format!("__proc_index_arg{idx}"));
        defaults.push(Some(Expr::number(0.0)));
        param_types.push(None);
    }

    if include_field_arg {
        params.push(PROC_FIELD_SENTINEL_ARG.to_owned());
        defaults.push(None);
        param_types.push(None);
    }

    FnSignature {
        params,
        defaults,
        param_types,
        type_params: Vec::new(),
    }
}

pub(crate) fn coerce_typed_events(
    events: &[EventDef],
    allow_slices: bool,
    event_owner_desc: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedEvent> {
    let mut out = Vec::<TypedEvent>::new();
    let mut seen_events = HashSet::<String>::new();
    for event in events {
        let event_diag = DiagCtx::new(event.loc);
        if is_builtin_constant_name(&event.name) {
            push_semantic(
                event_diag,
                errors,
                format!(
                    "event name '{}' is reserved as a builtin constant",
                    event.name
                ),
            );
            continue;
        }
        if !seen_events.insert(event.name.clone()) {
            push_semantic(
                event_diag,
                errors,
                format!("duplicate event '{}'", event.name),
            );
            continue;
        }
        let mut seen_params = HashSet::<String>::new();
        let mut typed_params = Vec::<TypedEventParam>::new();
        for param in &event.params {
            let param_diag = DiagCtx::new(param.ty_loc.or(param.loc));
            if !seen_params.insert(param.name.clone()) {
                push_semantic(
                    param_diag,
                    errors,
                    format!(
                        "duplicate event parameter '{}' in '{}'",
                        param.name, event.name
                    ),
                );
                continue;
            }
            let typed = match &param.ty {
                EventParamType::Scalar(ty) => TypedEventParamType::Scalar(*ty),
                EventParamType::Array { elem, size } => {
                    let context = format!("event '{}.{}' array size", event.name, param.name);
                    let len = eval_data_size_expr(size, options, &context, errors).unwrap_or(1);
                    if len == 0 {
                        push_semantic(
                            param_diag,
                            errors,
                            format!(
                                "event parameter '{}.{}' array size must be greater than zero",
                                event.name, param.name
                            ),
                        );
                    }
                    TypedEventParamType::Array { elem: *elem, len }
                }
                EventParamType::Slice { elem } => {
                    if !allow_slices {
                        push_semantic(
                            param_diag,
                            errors,
                            format!(
                                "{event_owner_desc} event parameter '{}.{}' cannot use slice type '{:?}[]'; top-level host events must stay fixed-size",
                                event.name, param.name, elem
                            ),
                        );
                    }
                    TypedEventParamType::Slice { elem: *elem }
                }
                EventParamType::GenericSlice { elem } => {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "{event_owner_desc} event parameter '{}.{}' has unresolved generic slice type '{}[]'; generic event slices must be specialized before lowering",
                            event.name, param.name, elem
                        ),
                    );
                    TypedEventParamType::Slice {
                        elem: PrimitiveType::F32,
                    }
                }
            };
            let default = coerce_typed_event_default(
                param,
                &typed,
                &format!("event '{}.{}'", event.name, param.name),
                options,
                errors,
            );
            typed_params.push(TypedEventParam {
                name: param.name.clone(),
                ty: typed,
                default,
            });
        }
        out.push(TypedEvent {
            name: event.name.clone(),
            params: typed_params,
            body: event.body.clone(),
        });
    }
    out
}

fn builtin_proc_init_event_spec(param_specs: &[ProcParamSpec]) -> ProcEventSpec {
    let params = param_specs
        .iter()
        .map(|param| {
            if param.slots.len() == 1 && param.slots[0].name == param.name {
                let slot = &param.slots[0];
                ProcEventParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcEventParamSlotSpec {
                        name: param.name.clone(),
                        ty: slot.ty,
                    }],
                    fixed_array_elem_ty: None,
                    slice_elem_ty: None,
                    default: slot.default.clone(),
                }
            } else {
                let default_values = param
                    .slots
                    .iter()
                    .map(|slot| slot.default.clone())
                    .collect::<Option<Vec<_>>>();
                ProcEventParamSpec {
                    name: param.name.clone(),
                    slots: Vec::new(),
                    fixed_array_elem_ty: param.slots.first().map(|slot| slot.ty),
                    slice_elem_ty: None,
                    default: default_values.map(|values| Expr::ArrayLiteral {
                        loc: Default::default(),
                        values,
                    }),
                }
            }
        })
        .collect();
    ProcEventSpec { params }
}

fn expand_proc_event_specs(
    proc: &ProcessorDef,
    param_specs: &[ProcParamSpec],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, ProcEventSpec> {
    let mut out = HashMap::<String, ProcEventSpec>::new();
    for event in &proc.events {
        let event_diag = DiagCtx::new(event.loc);
        if is_builtin_constant_name(&event.name) {
            push_semantic(
                event_diag,
                errors,
                format!(
                    "processor '{}' event name '{}' is reserved as a builtin constant",
                    proc.name, event.name
                ),
            );
            continue;
        }
        if out.contains_key(&event.name) {
            push_semantic(
                event_diag,
                errors,
                format!("duplicate processor event '{}.{}'", proc.name, event.name),
            );
            continue;
        }
        if is_builtin_proc_init_event_name(&event.name) {
            out.insert(
                event.name.clone(),
                builtin_proc_init_event_spec(param_specs),
            );
            continue;
        }
        let mut params = Vec::<ProcEventParamSpec>::new();
        for param in &event.params {
            let param_diag = DiagCtx::new(param.ty_loc.or(param.loc));
            match &param.ty {
                EventParamType::Scalar(ty) => params.push(ProcEventParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcEventParamSlotSpec {
                        name: param.name.clone(),
                        ty: *ty,
                    }],
                    fixed_array_elem_ty: None,
                    slice_elem_ty: None,
                    default: validate_proc_event_default_expr(
                        param,
                        None,
                        &format!(
                            "processor '{}.{}' event parameter '{}'",
                            proc.name, event.name, param.name
                        ),
                        options,
                        errors,
                    ),
                }),
                EventParamType::Array { elem, size } => {
                    let context = format!(
                        "processor '{}.{}' event parameter '{}'",
                        proc.name, event.name, param.name
                    );
                    let len = eval_data_size_expr(size, options, &context, errors).unwrap_or(1);
                    if len == 0 {
                        push_semantic(
                            param_diag,
                            errors,
                            format!(
                                "processor '{}.{}' event parameter '{}' array size must be greater than zero",
                                proc.name, event.name, param.name
                            ),
                        );
                    }
                    params.push(ProcEventParamSpec {
                        name: param.name.clone(),
                        slots: Vec::new(),
                        fixed_array_elem_ty: Some(*elem),
                        slice_elem_ty: None,
                        default: validate_proc_event_default_expr(
                            param,
                            Some(len),
                            &context,
                            options,
                            errors,
                        ),
                    });
                }
                EventParamType::Slice { elem } => {
                    params.push(ProcEventParamSpec {
                        name: param.name.clone(),
                        slots: Vec::new(),
                        fixed_array_elem_ty: None,
                        slice_elem_ty: Some(*elem),
                        default: validate_proc_event_default_expr(
                            param,
                            None,
                            &format!(
                                "processor '{}.{}' event parameter '{}'",
                                proc.name, event.name, param.name
                            ),
                            options,
                            errors,
                        ),
                    });
                }
                EventParamType::GenericSlice { elem } => {
                    push_semantic(
                        param_diag,
                        errors,
                        format!(
                            "processor '{}.{}' event parameter '{}' has unresolved generic slice type '{}[]'; generic event slices must be specialized before processor lowering",
                            proc.name, event.name, param.name, elem
                        ),
                    );
                    params.push(ProcEventParamSpec {
                        name: param.name.clone(),
                        slots: Vec::new(),
                        fixed_array_elem_ty: None,
                        slice_elem_ty: Some(PrimitiveType::F32),
                        default: validate_proc_event_default_expr(
                            param,
                            None,
                            &format!(
                                "processor '{}.{}' event parameter '{}'",
                                proc.name, event.name, param.name
                            ),
                            options,
                            errors,
                        ),
                    });
                }
            }
        }
        out.insert(event.name.clone(), ProcEventSpec { params });
    }
    out
}

fn build_proc_lowering_env(
    program: &Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcLoweringEnv> {
    let mut proc_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(p) => Some(p.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if proc_defs.is_empty() {
        return None;
    }

    let mut proc_sample_oversample_factors = HashMap::<String, usize>::new();
    for proc in &mut proc_defs {
        let factor = validated_sample_oversample_factor(
            proc.sample_oversample_factor.as_ref(),
            &format!("processor '{}' sample block", proc.name),
            errors,
        );
        proc_sample_oversample_factors.insert(proc.name.clone(), factor);
        rewrite_proc_local_defs(proc, errors);
    }

    let raw_struct_defs_by_name = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some((s.name.clone(), s.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let mut generic_struct_templates = HashMap::<String, StructDef>::new();
    for (name, def) in &raw_struct_defs_by_name {
        if !def.type_params.is_empty() {
            generic_struct_templates.insert(name.clone(), def.clone());
        }
    }
    let mut generated_struct_specializations = HashMap::<String, StructDef>::new();
    if !generic_struct_templates.is_empty() {
        for proc in &mut proc_defs {
            rewrite_generic_struct_ctor_stmt_list(
                &mut proc.init,
                &generic_struct_templates,
                &mut generated_struct_specializations,
                errors,
            );
            rewrite_generic_struct_ctor_stmt_list(
                &mut proc.block_pre,
                &generic_struct_templates,
                &mut generated_struct_specializations,
                errors,
            );
            rewrite_generic_struct_ctor_stmt_list(
                &mut proc.block_post,
                &generic_struct_templates,
                &mut generated_struct_specializations,
                errors,
            );
            rewrite_generic_struct_ctor_stmt_list(
                &mut proc.sample,
                &generic_struct_templates,
                &mut generated_struct_specializations,
                errors,
            );
            for event in &mut proc.events {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut event.body,
                    &generic_struct_templates,
                    &mut generated_struct_specializations,
                    errors,
                );
            }
            for def in &mut proc.local_defs {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut def.body,
                    &generic_struct_templates,
                    &mut generated_struct_specializations,
                    errors,
                );
            }
        }
        finalize_generated_generic_struct_specializations(
            &generic_struct_templates,
            &mut generated_struct_specializations,
            errors,
        );
    }
    let mut struct_defs_by_name = raw_struct_defs_by_name;
    for (name, def) in generated_struct_specializations {
        struct_defs_by_name.entry(name).or_insert(def);
    }
    let struct_symbols = struct_defs_by_name.keys().cloned().collect::<HashSet<_>>();
    let proc_symbols = proc_defs
        .iter()
        .map(|p| p.name.clone())
        .collect::<HashSet<_>>();
    let ctor_symbols = struct_symbols
        .iter()
        .cloned()
        .chain(proc_symbols.iter().cloned())
        .collect::<HashSet<_>>();
    let typed_struct_defs = struct_defs_for_scalar_expr_inference(&struct_defs_by_name);
    let mut pre_desugar_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(d) => Some(d.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut callable_symbols_for_method_sugar = pre_desugar_defs
        .iter()
        .map(|d| d.name.clone())
        .collect::<HashSet<_>>();
    for (struct_name, struct_def) in &struct_defs_by_name {
        for method in &struct_def.methods {
            callable_symbols_for_method_sugar.insert(format!("{struct_name}.{}", method.name));
        }
    }
    for (struct_name, struct_def) in &struct_defs_by_name {
        for method in &struct_def.methods {
            let mut desugared_method_body = method.body.clone();
            let mut method_struct_instances = HashMap::<String, String>::new();
            let mut method_struct_array_roots = HashMap::<String, String>::new();
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                register_struct_instance_and_array_roots(
                    "self",
                    struct_name,
                    &typed_struct_defs,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                );
            }
            let method_ns = namespace_of_symbol(struct_name);
            for stmt in &mut desugared_method_body {
                desugar_init_instance_method_calls(
                    stmt,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                    &typed_struct_defs,
                    &method_ns,
                    &callable_symbols_for_method_sugar,
                );
            }
            pre_desugar_defs.push(FunctionDef {
                loc: method.loc.clone(),
                type_params: Vec::new(),
                name: format!("{struct_name}.{}", method.name),
                params: method.params.clone(),
                body: desugared_method_body,
            });
        }
    }
    for proc in &proc_defs {
        for local_def in unique_proc_local_defs(proc) {
            pre_desugar_defs.push(pre_desugar_proc_local_hidden_def(&proc.name, &local_def));
        }
    }
    let mut pre_desugar_fn_signatures = HashMap::<String, FnSignature>::new();
    for def in &pre_desugar_defs {
        pre_desugar_fn_signatures
            .entry(def.name.clone())
            .or_insert_with(|| FnSignature {
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                type_params: def.type_params.clone(),
            });
    }
    let pre_desugar_def_return_types = infer_def_return_types(
        &pre_desugar_defs,
        &pre_desugar_fn_signatures,
        &HashMap::new(),
    );
    for proc in &mut proc_defs {
        desugar_processor_instance_method_calls(
            proc,
            &typed_struct_defs,
            &callable_symbols_for_method_sugar,
        );
    }
    let mut proc_defs_by_name = proc_defs
        .iter()
        .map(|p| (p.name.clone(), p.clone()))
        .collect::<HashMap<_, _>>();

    let mut base_shapes = HashMap::<String, ProcBaseShape>::new();
    let mut proc_api = HashMap::<String, ProcApi>::new();
    let mut proc_order = Vec::<String>::new();
    for proc in &mut proc_defs {
        let proc_diag = DiagCtx::new(proc.loc);
        if !proc.has_sample_block {
            push_semantic(
                proc_diag,
                errors,
                format!("processor '{}' must declare sample block", proc.name),
            );
        }
        if base_shapes.contains_key(&proc.name) {
            push_semantic(
                proc_diag,
                errors,
                format!("duplicate processor '{}'", proc.name),
            );
            continue;
        }
        let shape = compute_proc_shape(
            proc,
            proc_sample_oversample_factors
                .get(&proc.name)
                .copied()
                .unwrap_or(1),
            options,
            &proc_symbols,
            &struct_defs_by_name,
            &ctor_symbols,
            &pre_desugar_def_return_types,
            &pre_desugar_fn_signatures,
            &proc_defs_by_name,
            errors,
        );
        if shape.outs.is_empty() {
            push_semantic(
                proc_diag,
                errors,
                format!(
                    "processor '{}' must declare outs block or assign to outN in sample",
                    proc.name
                ),
            );
            continue;
        }
        proc_api.insert(
            proc.name.clone(),
            ProcApi {
                ins: shape.in_ports.clone(),
                params: shape
                    .param_specs
                    .iter()
                    .flat_map(|spec| spec.slots.iter().cloned())
                    .map(|slot| (slot.name.clone(), slot))
                    .collect::<HashMap<_, _>>(),
                outs: shape.outs.clone(),
                events: expand_proc_event_specs(proc, &shape.param_specs, options, errors),
                buffers: shape.buffer_specs.clone(),
                has_block: proc.has_block_block,
                sample_oversample_factor: proc_sample_oversample_factors
                    .get(&proc.name)
                    .copied()
                    .unwrap_or(1),
            },
        );
        base_shapes.insert(proc.name.clone(), shape);
        proc_order.push(proc.name.clone());
    }
    proc_defs_by_name = proc_defs
        .iter()
        .map(|p| (p.name.clone(), p.clone()))
        .collect::<HashMap<_, _>>();

    let mut lowering_shapes = HashMap::<String, ProcLoweringShape>::new();
    for proc_name in &proc_order {
        let mut visiting = Vec::<String>::new();
        let _ = build_proc_lowering_shape(
            proc_name,
            &base_shapes,
            &mut lowering_shapes,
            &mut visiting,
            errors,
        );
    }

    let effective_proc_blocks =
        compute_effective_proc_block_flags(&proc_order, &proc_defs_by_name, &base_shapes);
    for (proc_name, api) in &mut proc_api {
        api.has_block = *effective_proc_blocks
            .get(proc_name)
            .unwrap_or(&api.has_block);
    }
    for proc_name in &proc_order {
        let Some(proc) = proc_defs_by_name.get(proc_name) else {
            continue;
        };
        let Some(shape) = base_shapes.get(proc_name) else {
            continue;
        };
        let nested_instances = shape
            .state
            .nested_procs
            .iter()
            .map(|(name, nested)| {
                (
                    name.clone(),
                    ProcCallInstance {
                        proc_name: nested.proc_name.clone(),
                        buffer_args: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        reject_non_sample_proc_operator_calls_in_proc(
            proc,
            &nested_instances,
            &shape.nested_proc_array_slots,
            &proc_api,
            errors,
        );
    }

    Some(ProcLoweringEnv {
        struct_defs_by_name,
        proc_defs_by_name,
        proc_api,
        proc_order,
        lowering_shapes,
    })
}

pub(crate) fn desugar_processors(
    mut program: Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ProcessorDesugarResult {
    // Validate proc-local def type params BEFORE generic proc specialization
    // (which clears proc.type_params on specialized copies).
    for block in &program.blocks {
        if let Block::Proc(proc) = block {
            for local_def in &proc.local_defs {
                for tp in &local_def.type_params {
                    if proc.type_params.contains(tp) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "type parameter '{}' on def '{}' in proc '{}' shadows '{}' from proc '{}'; use a different name",
                                tp, local_def.name, proc.name, tp, proc.name
                            ),
                            local_def.loc,
                        ));
                    }
                }
                if !local_def.type_params.is_empty() {
                    let mut seen = std::collections::HashSet::new();
                    for tp in &local_def.type_params {
                        if !seen.insert(tp.clone()) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "duplicate type parameter '{}' in def '{}' of proc '{}'",
                                    tp, local_def.name, proc.name
                                ),
                                local_def.loc,
                            ));
                        }
                    }
                }
            }
        }
    }

    rewrite_and_materialize_generic_processors(&mut program, errors);
    inject_builtin_proc_init_events(&mut program, errors);
    lower_graph_blocks(&mut program, options, errors);

    // Rewrite proc-local defs into hidden ordinary def calls before proc lowering.
    for block in &mut program.blocks {
        if let Block::Proc(proc) = block {
            rewrite_proc_local_defs(proc, errors);
        }
    }

    let Some(ProcLoweringEnv {
        struct_defs_by_name,
        proc_defs_by_name,
        proc_api,
        proc_order,
        lowering_shapes,
    }) = build_proc_lowering_env(&program, options, errors)
    else {
        return ProcessorDesugarResult {
            program,
            def_sample_oversample_factors: HashMap::new(),
            proc_step_oversample_meta: HashMap::new(),
            proc_api: HashMap::new(),
            lowering_shapes: HashMap::new(),
            top_level_proc_rewrite: TopLevelProcRewriteMeta::default(),
        };
    };
    let existing_struct_names = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut proc_specialized_structs = struct_defs_by_name
        .values()
        .filter(|def| !existing_struct_names.contains(&def.name))
        .cloned()
        .collect::<Vec<_>>();
    proc_specialized_structs.sort_by(|a, b| a.name.cmp(&b.name));

    let (
        generated_structs,
        generated_defs,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
    ) = generate_lowered_proc_blocks(
        &proc_order,
        &proc_defs_by_name,
        &lowering_shapes,
        &struct_defs_by_name,
        &proc_api,
        errors,
    );
    program.blocks.retain(|b| !matches!(b, Block::Proc(_)));
    program
        .blocks
        .extend(proc_specialized_structs.into_iter().map(Block::Struct));
    program.blocks.extend(generated_structs);
    program.blocks.extend(generated_defs);

    let top_level_proc_rewrite =
        rewrite_top_level_proc_calls(&mut program, options, &lowering_shapes, &proc_api, errors);
    ProcessorDesugarResult {
        program,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
        proc_api,
        lowering_shapes,
        top_level_proc_rewrite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;
    use omni_frontend::{parse_program, AssignTarget, BinaryOp, Expr, Stmt};

    const WRAPPER_CONST_ZERO_LATENCY_REPRO: &str = r#"
import std/convolution
const MAX_IR = 100000
const FFT_SIZE = 1024

namespace convolution_wav_impulse<N = MAX_IR>:
  proc Engine:
    init:
      conv = std::convolution<FFT_SIZE, N>::ZeroLatencyConvolver<f32>()
    sample:
      out1 = 0.0

init:
  engine = convolution_wav_impulse<MAX_IR>::Engine()

sample:
  out1 = 0.0
"#;

    #[test]
    fn generic_proc_rewrite_specializes_nested_std_convolution_children_through_wrapper_namespace()
    {
        let mut program =
            parse_program(WRAPPER_CONST_ZERO_LATENCY_REPRO).expect("parse should succeed");
        let mut errors = Vec::new();
        rewrite_and_materialize_generic_processors(&mut program, &mut errors);
        assert!(
            errors.is_empty(),
            "generic proc rewrite should not emit errors: {errors:?}"
        );

        let zero_latency = program
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Proc(proc) if proc.name.contains("::ZeroLatencyConvolver.__gen__f32") => {
                    Some(proc)
                }
                _ => None,
            })
            .expect("specialized ZeroLatencyConvolver proc");

        let init_calls = zero_latency
            .init
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr:
                        Expr::UserCall {
                            name: call_name, ..
                        },
                    ..
                } => Some((name.clone(), call_name.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            init_calls.iter().any(|(name, call_name)| {
                name == "td" && call_name.contains("::TimeDomainConvolver.__gen__f32")
            }),
            "expected td ctor to be specialized, got {init_calls:?}"
        );
        assert!(
            init_calls.iter().any(|(name, call_name)| {
                name == "tail" && call_name.contains("::BlockConvolver.__gen__f32")
            }),
            "expected tail ctor to be specialized, got {init_calls:?}"
        );
    }

    #[test]
    fn build_proc_lowering_env_tracks_zero_latency_child_procs_through_wrapper_namespace() {
        let mut program =
            parse_program(WRAPPER_CONST_ZERO_LATENCY_REPRO).expect("parse should succeed");
        let mut errors = Vec::new();
        rewrite_and_materialize_generic_processors(&mut program, &mut errors);
        assert!(
            errors.is_empty(),
            "generic proc rewrite should not emit errors: {errors:?}"
        );
        let env = build_proc_lowering_env(&program, AnalysisOptions::default(), &mut errors)
            .expect("proc lowering env should exist");
        assert!(
            errors.is_empty(),
            "proc lowering env should not emit errors: {errors:?}"
        );

        let zero_latency_name = env
            .proc_defs_by_name
            .keys()
            .find(|name| name.contains("::ZeroLatencyConvolver.__gen__f32"))
            .cloned()
            .expect("specialized ZeroLatencyConvolver proc");
        let shape = env
            .lowering_shapes
            .get(&zero_latency_name)
            .expect("lowering shape for specialized ZeroLatencyConvolver");

        assert!(
            shape.state.nested_procs.contains_key("td"),
            "expected td nested proc, got {:?}",
            shape.state.nested_procs.keys().collect::<Vec<_>>()
        );
        assert!(
            shape.state.nested_procs.contains_key("tail"),
            "expected tail nested proc, got {:?}",
            shape.state.nested_procs.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn desugar_processors_accepts_wrapper_namespace_zero_latency_repro() {
        let program =
            parse_program(WRAPPER_CONST_ZERO_LATENCY_REPRO).expect("parse should succeed");
        let mut errors = Vec::new();
        let _desugared = desugar_processors(program, AnalysisOptions::default(), &mut errors);
        assert!(
            errors.is_empty(),
            "processor desugaring should not emit errors: {errors:?}"
        );
    }

    #[test]
    fn desugared_wrapper_namespace_program_has_no_raw_nested_proc_event_calls_left() {
        let program =
            parse_program(WRAPPER_CONST_ZERO_LATENCY_REPRO).expect("parse should succeed");
        let mut errors = Vec::new();
        let desugared = desugar_processors(program, AnalysisOptions::default(), &mut errors);
        assert!(
            errors.is_empty(),
            "processor desugaring should not emit errors: {errors:?}"
        );

        let mut offending = Vec::<String>::new();
        for block in desugared.program.blocks {
            match block {
                Block::Def(def) => {
                    for stmt in def.body {
                        collect_offending_proc_event_calls_in_stmt(
                            &stmt,
                            &def.name,
                            &mut offending,
                        );
                    }
                }
                Block::Events(events) => {
                    for event in events.events {
                        for stmt in event.body {
                            collect_offending_proc_event_calls_in_stmt(
                                &stmt,
                                &format!("event:{}", event.name),
                                &mut offending,
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(
            offending.is_empty(),
            "expected no raw nested proc event calls after desugaring, got {offending:?}"
        );
    }

    #[test]
    fn desugared_nested_proc_array_event_forwarding_has_no_raw_proc_event_calls_left() {
        let src = r#"
proc Voice:
  params:
    amp = 0.0
  outs:
    out1
  events:
    note_on(value: f32):
      amp = value
  sample:
    out1 = amp

proc Bank:
  outs:
    out1
  init:
    voices: Voice[2] = [Voice(), Voice()]
  events:
    note_on(value: f32):
      idx: i32 = 1
      voices[idx].note_on(value)
  sample:
    out1 = voices[1]()

outs:
  out1
events:
  note_on(value: f32):
    bank.note_on(value)
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let mut errors = Vec::new();
        let desugared = desugar_processors(program, AnalysisOptions::default(), &mut errors);
        assert!(
            errors.is_empty(),
            "processor desugaring should not emit errors: {errors:?}"
        );

        let mut offending = Vec::<String>::new();
        for block in desugared.program.blocks {
            match block {
                Block::Def(def) => {
                    for stmt in def.body {
                        collect_offending_proc_event_calls_in_stmt(
                            &stmt,
                            &def.name,
                            &mut offending,
                        );
                    }
                }
                Block::Events(events) => {
                    for event in events.events {
                        for stmt in event.body {
                            collect_offending_proc_event_calls_in_stmt(
                                &stmt,
                                &format!("event:{}", event.name),
                                &mut offending,
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(
            offending.is_empty(),
            "expected no raw nested proc-array event calls after desugaring, got {offending:?}"
        );
    }

    #[test]
    fn desugar_processors_synthesizes_builtin_proc_init_event() {
        let src = r#"
proc Voice:
  params:
    gain: i32 = 1
    mix: f32[2] = [0.25, 0.5]
  outs:
    out1
  sample:
    out1 = f32(gain) + mix[0] + mix[1]

outs:
  out1
init:
  voice = Voice()
  voice.init(3, [1.0, 2.0])
sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let mut errors = Vec::new();
        let desugared = desugar_processors(program, AnalysisOptions::default(), &mut errors);
        assert!(
            errors.is_empty(),
            "processor desugaring should not emit errors: {errors:?}"
        );

        let init_call_name = desugared
            .program
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Init(init) => init.body.iter().find_map(|stmt| match stmt {
                    Stmt::Expr {
                        expr: Expr::UserCall { name, .. },
                        ..
                    } if name.ends_with(".__proc_event_init") => Some(name.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("expected lowered top-level builtin init event call");
        assert_eq!(init_call_name, "Voice.__proc_event_init");

        let init_def = desugared
            .program
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Def(def) if def.name == "Voice.__proc_event_init" => Some(def),
                _ => None,
            })
            .expect("expected generated builtin init event def");

        assert_eq!(init_def.params.len(), 3);
        assert!(matches!(
            init_def.params[1].ty,
            Some(FnParamType::Primitive(PrimitiveType::I32))
        ));
        assert!(matches!(
            init_def.params[2].ty,
            Some(FnParamType::Array(Some(PrimitiveType::F32)))
        ));
        assert!(init_def.body.iter().any(|stmt| matches!(
            stmt,
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::Var { name: value_name, .. },
                ..
            } if name == "self.gain" && value_name == "gain"
        )));
        assert!(init_def.body.iter().any(|stmt| matches!(
            stmt,
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::Index { base, index, .. },
                ..
            } if name == "self.mix[0]"
                && base == "mix"
                && matches!(index.as_ref(), Expr::Int { value: 0, .. })
        )));
        assert!(init_def.body.iter().any(|stmt| matches!(
            stmt,
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::Index { base, index, .. },
                ..
            } if name == "self.mix[1]"
                && base == "mix"
                && matches!(index.as_ref(), Expr::Int { value: 1, .. })
        )));
    }

    #[test]
    fn desugar_processors_specializes_builtin_proc_init_event_for_generic_params() {
        let src = r#"
proc Voice<T>:
  params:
    value: T = 0
  outs:
    out1
  sample:
    out1 = f32(value)

outs:
  out1
init:
  voice = Voice<i64>()
  voice.init(42)
sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let mut errors = Vec::new();
        let desugared = desugar_processors(program, AnalysisOptions::default(), &mut errors);
        assert!(
            errors.is_empty(),
            "processor desugaring should not emit errors: {errors:?}"
        );

        let init_def = desugared
            .program
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Def(def)
                    if def.name.contains("Voice") && def.name.ends_with(".__proc_event_init") =>
                {
                    Some(def)
                }
                _ => None,
            })
            .expect("expected generated builtin init event def for specialized proc");

        assert!(matches!(
            init_def.params[1].ty,
            Some(FnParamType::Primitive(PrimitiveType::I64))
        ));
    }

    #[test]
    fn analyze_rejects_user_defined_proc_init_event() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  events:
    init(value: f32):
      gain = value
  sample:
    out1 = gain

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("builtin init redefinition should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("event name 'init' is reserved for the builtin initializer event")),
            "expected builtin init reservation diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_top_level_sample_coexistence() {
        let src = "outs { out1 }\nsample { out1 = 0.0 }\ngraph { 0.0 >> out1 }\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("graph + sample should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("graph block cannot be declared with sample block")
            })
            .expect("expected graph/sample exclusivity diagnostic");
        assert_eq!((diag.line, diag.column), (3, 1));
    }

    #[test]
    fn graph_block_requires_all_declared_outputs_to_be_driven() {
        let src = r#"
outs 2
graph {
  0.0 >> out1
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("missing graph output driver should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph must drive declared output 'out2'")),
            "expected missing output diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn analyze_accepts_simple_graph_program() {
        let src = r#"
proc Source:
  outs:
    out1
  sample:
    out1 = 0.25

proc Gain:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  src = Source()
  gain = Gain()

graph:
  src.out1 >> gain.in1
  gain.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("simple graph should analyze");
        assert!(
            typed
                .sample
                .iter()
                .any(|stmt| matches!(stmt, Stmt::Expr { .. })),
            "expected generated processor calls in sample body"
        );
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } if name == "out1"
            )),
            "expected generated out assignment in sample body"
        );
    }

    #[test]
    fn analyze_lowers_graph_fanout_to_shared_temp() {
        let src = r#"
proc Source:
  outs:
    out1
  sample:
    out1 = 0.25

proc Sum:
  ins:
    a
    b
  outs:
    out1
  sample:
    out1 = a + b

outs:
  out1

init:
  src = Source()
  sum = Sum()

graph:
  src.out1 * 2.0 >> { sum.a, sum.b }
  sum.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("graph fanout should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Binary { op: BinaryOp::Mul, .. },
                    ..
                } if name == "__graph_fanout_0"
            )),
            "expected shared graph fanout temp assignment in lowered sample: {:?}",
            typed.sample
        );
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } if name == "Sum.__proc_step"
                    && args.iter().filter(|arg| matches!(
                        arg.expr,
                        Expr::Var { name: ref tmp, .. } if tmp == "__graph_fanout_0"
                    )).count() == 2
            )),
            "expected lowered proc call to reuse shared graph fanout temp twice: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_lowers_graph_proc_bundle_source_to_ordered_outputs() {
        let src = r#"
proc Pair:
  outs 2
  sample:
    out1 = 0.25
    out2 = 0.75

outs 2

init:
  pair = Pair()

graph:
  pair >> { out1, out2 }
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("graph proc bundle should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Var { name: src, .. },
                    ..
                } if name == "out1" && src == "pair.out1"
            )),
            "expected out1 to read pair.out1, got {:?}",
            typed.sample
        );
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Var { name: src, .. },
                    ..
                } if name == "out2" && src == "pair.out2"
            )),
            "expected out2 to read pair.out2, got {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_broadcasts_single_graph_proc_bundle_source() {
        let src = r#"
proc Mono:
  outs:
    out1
  sample:
    out1 = 0.5

outs 2

init:
  mono = Mono()

graph:
  mono >> { out1, out2 }
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("single-output proc bundle should analyze");
        let matching = typed
            .sample
            .iter()
            .filter(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        expr: Expr::Var { name: src, .. },
                        ..
                    } if src == "mono.out1"
                )
            })
            .count();
        assert_eq!(
            matching, 2,
            "expected mono.out1 to drive both outputs, got {:?}",
            typed.sample
        );
        assert!(
            typed.sample.iter().all(|stmt| !matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } if name.starts_with("__graph_fanout_")
            )),
            "single-output proc bundle should not allocate a graph fanout temp: {:?}",
            typed.sample
        );
    }

    #[test]
    fn graph_proc_bundle_rejects_output_arity_mismatch() {
        let src = r#"
proc Pair:
  outs 2
  sample:
    out1 = 0.25
    out2 = 0.75

outs 3

init:
  pair = Pair()

graph:
  pair >> { out1, out2, out3 }
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("bundle arity mismatch should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "graph source 'pair' exposes 2 output slot(s), but destination set has 3 endpoint(s)"
            )),
            "expected proc bundle arity diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_cycle_without_delay() {
        let src = r#"
proc Pass:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  a = Pass()
  b = Pass()

graph:
  a.out1 >> b.in1
  b.out1 >> a.in1
  a.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("cycle should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph contains a cycle without sample delay")),
            "expected cycle diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn analyze_accepts_graph_proc_array_param_destinations() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs 2

init:
  voices: Voice[2] = Voice()

graph:
  0.25 >> voices[0].gain
  0.75 >> voices[1].gain
  voices[0].out1 >> out1
  voices[1].out1 >> out2
"#;
        let program = parse_program(src).expect("parse should succeed");
        let lowered = crate::lower_graphs_for_inspection_with_options(
            program.clone(),
            AnalysisOptions::default(),
        )
        .expect("proc-array param graph should lower for inspection");
        let block_pre = lowered
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Block(exec) => Some(&exec.pre),
                _ => None,
            })
            .expect("lowered top-level block exec");
        assert!(
            block_pre.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } if name == "voices[0].gain" || name == "voices[1].gain"
            )),
            "expected graph lowering to preserve proc-array slot endpoint paths in block_pre: {:?}",
            block_pre
        );

        let typed = analyze(program).expect("proc-array param graph should analyze");
        assert!(
            typed.block_pre.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Index { base, index },
                    ..
                } if base == "voices.gain"
                    && matches!(
                        index,
                        Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                    )
            )),
            "expected proc-array param assignments to normalize back to flattened storage in block_pre: {:?}",
            typed.block_pre
        );
        let out_assigns = typed
            .sample
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(out_assigns.contains(&"out1") && out_assigns.contains(&"out2"));
    }

    #[test]
    fn analyze_accepts_graph_indexed_param_sources() {
        let src = r#"
proc Take:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

params:
  gains: f32[2] = [0.25, 0.75]
outs:
  out1

init:
  take = Take()

graph:
  gains[1] >> take.in1
  take.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("indexed param graph should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } if name == "Take.__proc_step"
                    && args.iter().any(|arg| matches!(
                        arg.expr,
                        Expr::Index { ref base, ref index, .. }
                            if base == "gains"
                                && matches!(**index, Expr::Int { value: 1, .. })
                    ))
            )),
            "expected lowered sample call to read gains[1]: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_indexed_proc_output_array_sources() {
        let src = r#"
proc Source:
  outs:
    pair: f32[2]
  sample:
    pair[0] = 0.25
    pair[1] = 0.75

proc Take:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  src = Source()
  take = Take()

graph:
  src.pair[1] >> take.in1
  take.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("indexed proc output array graph should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } if name == "Take.__proc_step"
                    && args.iter().any(|arg| matches!(
                        arg.expr,
                        Expr::Var { ref name, .. } if name == "src.pair[1]"
                    ))
            )),
            "expected lowered sample call to read flattened proc output slot src.pair[1]: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_whole_proc_output_array_sources() {
        let src = r#"
proc Source:
  outs:
    pair: f32[2]
  sample:
    pair[0] = 0.25
    pair[1] = 0.75

outs:
  out_st: f32[2]

init:
  src = Source()

graph:
  src.pair >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("whole proc output array graph should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Var { name: src, .. },
                        ..
                    } if base == "out_st"
                        && matches!(
                            index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                        && (src == "src.pair[0]" || src == "src.pair[1]")
                ))
                .count()
                == 2,
            "expected whole proc output array edge to lower to per-slot writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_indexed_proc_array_output_array_sources() {
        let src = r#"
proc Voice:
  outs:
    pair: f32[2]
  sample:
    pair[0] = 0.25
    pair[1] = 0.75

proc Take:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  voices: Voice[2] = Voice()
  take = Take()

graph:
  voices[0].pair[1] >> take.in1
  take.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("indexed proc-array output array graph should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } if name == "Take.__proc_step"
                    && args.iter().any(|arg| matches!(
                        arg.expr,
                        Expr::UserCall { ref name, ref args, .. }
                            if name == "Voice.__proc_call_out1"
                                && args.iter().any(|inner| matches!(
                                    inner.expr,
                                    Expr::Index { ref base, ref index, .. }
                                        if base == "voices"
                                            && matches!(**index, Expr::Int { value: 0, .. })
                                ))
                    ))
            )),
            "expected lowered sample call to read proc-array output slot voices[0].pair[1]: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_whole_proc_array_output_array_sources() {
        let src = r#"
proc Voice:
  outs:
    pair: f32[2]
  sample:
    pair[0] = 0.25
    pair[1] = 0.75

outs:
  out_st: f32[2]

init:
  voices: Voice[2] = Voice()

graph:
  voices[0].pair >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("whole proc-array output array graph should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::UserCall { name, .. },
                        ..
                    } if base == "out_st"
                        && matches!(
                            index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                        && name.starts_with("Voice.__proc_call_out")
                ))
                .count()
                == 2,
            "expected whole proc-array output array edge to lower to per-slot writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_array_param_destinations() {
        let src = r#"
proc Voice:
  params:
    pair: f32[2] = [0.0, 0.0]
  outs:
    out1
  sample:
    out1 = pair[0] + pair[1]

params:
  gains: f32[2] = [0.25, 0.75]
outs:
  out1

init:
  voice = Voice()

graph:
  gains >> voice.pair
  voice.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array param destination graph should analyze");
        assert!(
            typed
                .block_pre
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        expr: Expr::Index { base, index, .. },
                        ..
                    } if (name == "voice.pair[0]" || name == "voice.pair[1]")
                        && base == "gains"
                        && matches!(
                            &**index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                ))
                .count()
                == 2,
            "expected array param graph edge to lower to per-slot param writes: {:?}",
            typed.block_pre
        );
    }

    #[test]
    fn analyze_accepts_graph_array_input_to_array_output_edges() {
        let src = r#"
ins:
  in_st: f32[2]
outs:
  out_st: f32[2]

graph:
  in_st >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array input/output graph should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Index { base: src, .. },
                        ..
                    } if base == "out_st"
                        && src == "in_st"
                        && matches!(
                            index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                ))
                .count()
                == 2,
            "expected lowered per-slot sample assignments from in_st to out_st: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_array_binary_expressions() {
        let src = r#"
ins:
  a: f32[2]
  b: f32[2]
outs:
  out_st: f32[2]

graph:
  a * 0.5 + b * 0.25 >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array binary graph should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Binary { op: BinaryOp::Add, .. },
                        ..
                    } if base == "out_st"
                        && matches!(
                            index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                ))
                .count()
                == 2,
            "expected lowered per-slot binary assignments for out_st: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_slice_sources() {
        let src = r#"
ins:
  in_bus: f32[4]
outs:
  out_st: f32[2]

graph:
  in_bus[1:3] >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("graph slice source should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Index {
                            base: src_base,
                            index: src_index,
                            ..
                        },
                        ..
                    } if base == "out_st"
                        && src_base == "in_bus"
                        && matches!(
                            (&index, &**src_index),
                            (
                                Expr::Int { value: 0, .. },
                                Expr::Int { value: 1, .. }
                            ) | (
                                Expr::Int { value: 1, .. },
                                Expr::Int { value: 2, .. }
                            )
                        )
                ))
                .count()
                == 2,
            "expected graph slice source to lower to shifted per-slot writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_array_literal_sources() {
        let src = r#"
ins:
  in1
  in2
outs:
  out_st: f32[2]

graph:
  [in1, in2] >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("graph array literal source should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Var { name: src, .. },
                        ..
                    } if base == "out_st"
                        && ((matches!(index, Expr::Int { value: 0, .. }) && src == "in1")
                            || (matches!(index, Expr::Int { value: 1, .. }) && src == "in2"))
                ))
                .count()
                == 2,
            "expected graph array literal source to lower to per-slot writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_array_delayed_edges() {
        let src = r#"
ins:
  in_st: f32[2]
outs:
  out_st: f32[2]

graph:
  in_st >>[1] out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array delayed graph edge should analyze");
        assert!(
            typed.init.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::ArrayCtor { spec, .. },
                    ..
                } if name == "__graph_delay_0_buf"
                    && matches!(
                        (&spec.elem, spec.size.as_ref()),
                        (
                            ArrayElemType::Primitive(PrimitiveType::F32),
                            Expr::Int { value: 2, .. }
                        )
                    )
            )),
            "expected flattened array delay buffer init: {:?}",
            typed.init
        );
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Index { base: src_base, .. },
                        ..
                    } if base == "out_st"
                        && src_base == "__graph_delay_0_buf"
                        && matches!(
                            index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                ))
                .count()
                == 2,
            "expected per-slot delayed output reads: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_delay_expressions_with_consts_and_namespace_generics() {
        let src = r#"
const TAP = 2

namespace DelayCfg<Base = 1>:
  const LEN = Base + TAP

outs:
  out1

graph:
  0.5 >>[DelayCfg<1>::LEN + 1] out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("graph delay expression should analyze");
        assert!(
            typed.init.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::ArrayCtor { spec, .. },
                    ..
                } if name == "__graph_delay_0_buf"
                    && matches!(
                        (&spec.elem, spec.size.as_ref()),
                        (
                            ArrayElemType::Primitive(PrimitiveType::F32),
                            Expr::Int { value: 4, .. }
                        )
                    )
            )),
            "expected delay buffer sized from const/namespace expression: {:?}",
            typed.init
        );
    }

    #[test]
    fn graph_block_rejects_nonconstant_slice_bounds() {
        let src = r#"
params:
  start = 1.0
ins:
  in_bus: f32[4]
outs:
  out_st: f32[2]

graph:
  in_bus[start:3] >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("dynamic graph slice bounds should fail");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("graph slice 'in_bus' slice start")),
            "expected static graph slice bound diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn analyze_accepts_graph_receiver_syntax() {
        let src = r#"
proc Gain:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  g = Gain()

graph:
  g.in1 << 0.25
  out1 << g.out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("receiver graph should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } if name == "Gain.__proc_step"
                    && args.len() == 2
                    && matches!(args[0].expr, Expr::Var { name: ref node, .. } if node == "g")
                    && args.iter().any(|arg| matches!(
                        arg.expr,
                        Expr::Number { value: v, .. } if (v - 0.25).abs() <= f64::EPSILON
                    ))
            )),
            "expected lowered proc step driven by receiver edge: {:?}",
            typed.sample
        );
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Var { name: src, .. },
                    ..
                } if name == "out1" && src == "g.out1"
            )),
            "expected lowered output assignment driven by receiver edge: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_positive_delay_cycles() {
        let src = r#"
proc Pass:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  a = Pass()
  b = Pass()

graph:
  a.out1 >> b.in1
  b.out1 >>[1] a.in1
  a.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("positive-delay cycle graph should analyze");
        assert!(
            typed.init.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } if name == "__graph_delay_0_buf"
            )),
            "expected delay state init for positive-delay cycle: {:?}",
            typed.init
        );
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Expr {
                        expr: Expr::UserCall { name, .. },
                        ..
                    } if name == "Pass.__proc_step"
                ))
                .count()
                == 2,
            "expected both proc nodes to be stepped: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_proc_local_graph_programs() {
        let src = r#"
proc Swap:
  ins:
    in1
    in2
  outs:
    out1
    out2
  graph:
    in2 >> out1
    in1 >> out2

ins:
  in1
  in2
outs:
  out1
  out2

init:
  swap = Swap()

graph:
  in1 >> swap.in1
  in2 >> swap.in2
  swap.out1 >> out1
  swap.out2 >> out2
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-local graph program should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, .. },
                    ..
                } if name == "Swap.__proc_step"
            )),
            "expected proc-local graph proc to lower to a proc step call: {:?}",
            typed.sample
        );
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(base),
                        expr: Expr::Var { name: src, .. },
                        ..
                    } if (base == "out1" && src == "swap.out1")
                        || (base == "out2" && src == "swap.out2")
                ))
                .count()
                == 2,
            "expected proc-local graph outputs to lower to scalar output writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn lower_graphs_accepts_proc_local_delay_expressions_from_namespace_generics() {
        let src = r#"
namespace DelayCfg<Tap = 2>:
  proc DelayProc:
    ins:
      in1
    outs:
      out1
    graph:
      in1 >>[Tap + 1] out1

outs:
  out1

init:
  p = DelayCfg<2>::DelayProc()

sample:
  out1 = p(0.5)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let lowered =
            crate::lower_graphs_for_inspection_with_options(program, AnalysisOptions::default())
                .expect("proc-local namespaced graph delay should lower");
        let proc = lowered
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Proc(proc)
                    if proc.name.starts_with("DelayCfg__nsinst")
                        && proc.name.ends_with("::DelayProc") =>
                {
                    Some(proc)
                }
                _ => None,
            })
            .expect("instantiated proc block");

        assert!(
            proc.graph.is_none(),
            "proc graph should be lowered away: {proc:?}"
        );
        assert!(
            proc.init.body.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::ArrayCtor { spec, .. },
                    ..
                } if name == "__graph_delay_0_buf"
                    && matches!(
                        (&spec.elem, spec.size.as_ref()),
                        (
                            ArrayElemType::Primitive(PrimitiveType::F32),
                            Expr::Int { value: 3, .. }
                        )
                    )
            )),
            "expected proc-local delay buffer sized from namespace generic expression: {:?}",
            proc.init.body
        );
    }

    #[test]
    fn analyze_accepts_graph_scalar_broadcast_to_array_outputs() {
        let src = r#"
outs:
  out_st: f32[2]

graph:
  0.5 >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("scalar to array graph edge should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Number { value: v, .. },
                        ..
                    } if base == "out_st"
                        && matches!(
                            index,
                            Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. }
                        )
                        && (*v - 0.5).abs() <= f64::EPSILON
                ))
                .count()
                == 2,
            "expected scalar broadcast to lower to per-slot writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_scalar_broadcast_to_proc_array_inputs() {
        let src = r#"
proc Sum2:
  ins:
    in_st: f32[2]
  outs:
    out1
  sample:
    out1 = in_st[0] + in_st[1]

outs:
  out1

init:
  sum = Sum2()

graph:
  0.5 >> sum.in_st
  sum.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("scalar broadcast to proc input array should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } if name == "Sum2.__proc_step"
                    && args
                        .iter()
                        .filter(|arg| matches!(
                            arg.expr,
                            Expr::Number { value: v, .. } if (v - 0.5).abs() <= f64::EPSILON
                        ))
                        .count()
                        == 2
            )),
            "expected scalar broadcast to expand into per-slot proc input args: {:?}",
            typed.sample
        );
    }

    #[test]
    fn analyze_accepts_graph_scalar_broadcast_to_proc_array_params() {
        let src = r#"
proc Sum2:
  params:
    gains: f32[2] = [0.0, 0.0]
  outs:
    out1
  sample:
    out1 = gains[0] + gains[1]

outs:
  out1

init:
  sum = Sum2()

graph:
  0.5 >> sum.gains
  sum.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("scalar broadcast to proc param array should analyze");
        assert!(
            typed
                .block_pre
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        expr: Expr::Number { value: v, .. },
                        ..
                    } if (name == "sum.gains[0]" || name == "sum.gains[1]")
                        && (*v - 0.5).abs() <= f64::EPSILON
                ))
                .count()
                == 2,
            "expected scalar broadcast to lower to per-slot param writes: {:?}",
            typed.block_pre
        );
    }

    #[test]
    fn analyze_accepts_graph_negative_slice_sources() {
        let src = r#"
ins:
  in_bus: f32[4]
outs:
  out_st: f32[2]

graph:
  in_bus[:-2] >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("negative slice graph should analyze");
        assert!(
            typed
                .sample
                .iter()
                .filter(|stmt| matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index { base, index },
                        expr: Expr::Index {
                            base: src_base,
                            index: src_index,
                            ..
                        },
                        ..
                    } if base == "out_st"
                        && matches!(
                            (&index, &**src_index),
                            (
                                Expr::Int { value: 0, .. },
                                Expr::Int { value: 0, .. }
                            ) | (
                                Expr::Int { value: 1, .. },
                                Expr::Int { value: 1, .. }
                            )
                        )
                        && src_base == "in_bus"
                ))
                .count()
                == 2,
            "expected negative graph slice source to lower to shifted per-slot writes: {:?}",
            typed.sample
        );
    }

    #[test]
    fn graph_block_rejects_array_length_mismatch_edges() {
        let src = r#"
params:
  a: f32[2] = [0.25, 0.75]
outs:
  out_st: f32[3]

graph:
  a >> out_st
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("array length mismatch graph edge should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("shape mismatch: cannot assign F32[2] to F32[3]")),
            "expected array length mismatch diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_block_rate_indexed_input_sources() {
        let src = r#"
proc Gain:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

ins:
  in_st: f32[2]
outs:
  out1

init:
  g = Gain()

graph:
  @block in_st[0] >> g.gain
  g.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("indexed @block input source should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph @block edge cannot read sample-rate input 'in_st'")),
            "expected indexed input block-rate diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_duplicate_drivers() {
        let src = r#"
outs:
  out1

graph:
  0.0 >> out1
  1.0 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("duplicate graph driver should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph destination 'out1' has more than one driver")),
            "expected duplicate-driver diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_fanout_with_mixed_implicit_rates() {
        let src = r#"
proc Gain:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1

init:
  g = Gain()

graph:
  0.5 >> { g.gain, out1 }
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mixed implicit-rate fanout should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph fanout edge destinations require an explicit rate when their default rates differ")),
            "expected mixed implicit-rate fanout diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_proc_output_destinations() {
        let src = r#"
proc Gain:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

init:
  g = Gain()

graph:
  0.0 >> g.out1
  g.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("proc output destination should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph destination 'g.out1' cannot target processor outputs")),
            "expected proc-output destination diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_top_level_output_sources() {
        let src = r#"
outs:
  out1

graph:
  out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("top-level output source should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph source cannot read output 'out1'")),
            "expected top-level output source diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_unknown_source_nodes() {
        let src = r#"
proc Gain:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  g = Gain()

graph:
  ghost.out1 >> g.in1
  g.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unknown graph node should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph source references unknown node 'ghost'")),
            "expected unknown-node diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_unknown_destination_endpoints() {
        let src = r#"
proc Gain:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

init:
  g = Gain()

graph:
  0.0 >> g.not_real
  g.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unknown graph endpoint should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph destination 'g.not_real' references an unknown endpoint")),
            "expected unknown-endpoint diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_block_rejects_inferred_block_param_edges_from_sample_sources() {
        let src = r#"
proc Mod:
  outs:
    out1
  sample:
    out1 = 0.5

proc Gain:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1

init:
  mod = Mod()
  g = Gain()

graph:
  mod.out1 >> g.gain
  g.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("inferred @block param edge should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph @block edge cannot read sample-rate processor output 'mod.out1'")),
            "expected inferred-@block sample-source diagnostic, got {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("add @sample to this param edge")),
            "expected inferred-@block @sample hint, got {errors:?}"
        );
    }

    #[test]
    fn analyze_accepts_graph_sample_override_for_param_destinations() {
        let src = r#"
proc Gain:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

ins:
  in1
outs:
  out1

init:
  g = Gain()

graph:
  @sample in1 >> g.gain
  g.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("@sample param override graph should analyze");
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Var { name: src, .. },
                    ..
                } if name == "g.gain" && src == "in1"
            )),
            "expected lowered sample assignment to drive g.gain from in1: {:?}",
            typed.sample
        );
        assert!(
            typed.sample.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::UserCall { name, .. },
                    ..
                } if name == "Gain.__proc_step"
            )),
            "expected lowered sample proc step for g: {:?}",
            typed.sample
        );
    }

    #[test]
    fn graph_block_rejects_explicit_zero_delay_cycles() {
        let src = r#"
proc Pass:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  a = Pass()
  b = Pass()

graph:
  a.out1 >>[0] b.in1
  b.out1 >> a.in1
  a.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("zero-delay cycle should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("graph contains a cycle without sample delay")),
            "expected zero-delay cycle diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn proc_graph_block_rejects_sample_coexistence() {
        let src = r#"
proc Main:
  outs:
    out1
  sample:
    out1 = 0.0
  graph:
    0.0 >> out1
"#;
        let errors = parse_program(src).expect_err("proc graph + sample should fail at parse");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("proc graph block cannot be declared with sample or block")),
            "expected proc graph/sample exclusivity diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn proc_graph_block_rejects_block_coexistence() {
        let src = r#"
proc Main:
  outs:
    out1
  block:
    sample:
      out1 = 0.0
  graph:
    0.0 >> out1
"#;
        let errors = parse_program(src).expect_err("proc graph + block should fail at parse");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("proc graph block cannot be declared with sample or block")),
            "expected proc graph/block exclusivity diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn graph_cycle_diagnostic_reports_cycle_path() {
        let src = r#"
proc Pass:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

outs:
  out1

init:
  a = Pass()
  b = Pass()
  c = Pass()

graph:
  a.out1 >> b.in1
  b.out1 >> c.in1
  c.out1 >> a.in1
  a.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("cycle should fail");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("a -> b -> c -> a")),
            "expected explicit cycle path, got {errors:?}"
        );
    }

    #[test]
    fn graph_type_mismatch_reports_graph_edge_location() {
        let src = r#"
proc Pass:
  ins:
    in1
  outs:
    out1
  sample:
    out1 = in1

params:
  wide: f64 = 0.5

outs:
  out1

init:
  p = Pass()

graph:
  wide >> p.in1
  p.out1 >> out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("graph type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("graph edge source for destination 'p.in1' type mismatch")
            })
            .expect("expected graph type mismatch diagnostic");
        assert_eq!(diag.line, 20, "expected graph-edge line, got {diag:?}");
        assert_eq!(diag.column, 3, "expected graph-edge column, got {diag:?}");
    }

    fn collect_offending_proc_event_calls_in_stmt(
        stmt: &Stmt,
        owner: &str,
        offending: &mut Vec<String>,
    ) {
        match stmt {
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } if name == "td.set_impulse_window"
                || name == "tail.set_impulse_window"
                || name == "td.reset"
                || name == "tail.reset" =>
            {
                offending.push(format!("{owner}:{name}"));
            }
            Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
                collect_offending_proc_event_calls_in_expr(expr, owner, offending);
            }
            Stmt::Expr { expr, .. } => {
                collect_offending_proc_event_calls_in_expr(expr, owner, offending)
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_offending_proc_event_calls_in_expr(cond, owner, offending);
                for stmt in then_branch {
                    collect_offending_proc_event_calls_in_stmt(stmt, owner, offending);
                }
                for stmt in else_branch {
                    collect_offending_proc_event_calls_in_stmt(stmt, owner, offending);
                }
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_offending_proc_event_calls_in_expr(start, owner, offending);
                collect_offending_proc_event_calls_in_expr(end, owner, offending);
                if let Some(step) = step {
                    collect_offending_proc_event_calls_in_expr(step, owner, offending);
                }
                for stmt in body {
                    collect_offending_proc_event_calls_in_stmt(stmt, owner, offending);
                }
            }
            Stmt::While { cond, body, .. } => {
                collect_offending_proc_event_calls_in_expr(cond, owner, offending);
                for stmt in body {
                    collect_offending_proc_event_calls_in_stmt(stmt, owner, offending);
                }
            }
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }

    fn collect_offending_proc_event_calls_in_expr(
        expr: &Expr,
        owner: &str,
        offending: &mut Vec<String>,
    ) {
        match expr {
            Expr::UserCall { name, args, .. } => {
                if name == "td.set_impulse_window"
                    || name == "tail.set_impulse_window"
                    || name == "td.reset"
                    || name == "tail.reset"
                {
                    offending.push(format!("{owner}:{name}"));
                }
                for arg in args {
                    collect_offending_proc_event_calls_in_expr(&arg.expr, owner, offending);
                }
            }
            Expr::Index { index, .. } => {
                collect_offending_proc_event_calls_in_expr(index, owner, offending);
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_offending_proc_event_calls_in_expr(&spec.size, owner, offending);
                if let Some(init) = init {
                    for expr in init {
                        collect_offending_proc_event_calls_in_expr(expr, owner, offending);
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                collect_offending_proc_event_calls_in_expr(lhs, owner, offending);
                collect_offending_proc_event_calls_in_expr(rhs, owner, offending);
            }
            Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => {
                for expr in args {
                    collect_offending_proc_event_calls_in_expr(expr, owner, offending);
                }
            }
            Expr::Cast { expr, .. }
            | Expr::UnaryNot { expr, .. }
            | Expr::UnaryBitNot { expr, .. } => {
                collect_offending_proc_event_calls_in_expr(expr, owner, offending);
            }
            Expr::Slice { start, end, .. } => {
                if let Some(s) = start {
                    collect_offending_proc_event_calls_in_expr(s, owner, offending);
                }
                if let Some(e) = end {
                    collect_offending_proc_event_calls_in_expr(e, owner, offending);
                }
            }
            Expr::Tuple { values, .. } => {
                for v in values {
                    collect_offending_proc_event_calls_in_expr(v, owner, offending);
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }
}
