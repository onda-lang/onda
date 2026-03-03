use std::collections::{HashMap, HashSet};

use crate::*;

mod generated_blocks;
mod generic_proc_rewrite;
mod global_proc_rewrite;
mod nested_paths;
mod nested_proc_lowering;
mod shape_helpers;
use generated_blocks::*;
use generic_proc_rewrite::*;
use global_proc_rewrite::*;
use nested_paths::*;
use nested_proc_lowering::*;
use shape_helpers::*;

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
    state: ProcStateFields,
    instance_fields: HashMap<String, HashSet<String>>,
    fields: Vec<StructField>,
    field_names: HashSet<String>,
    array_field_names: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ProcLoweringShape {
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
    state: ProcStateFields,
    fields: Vec<StructField>,
    field_names: HashSet<String>,
    array_field_names: HashSet<String>,
    nested_fields: HashMap<String, HashSet<String>>,
}

struct ProcLoweringEnv {
    struct_defs_by_name: HashMap<String, StructDef>,
    proc_defs_by_name: HashMap<String, ProcessorDef>,
    proc_api: HashMap<String, ProcApi>,
    proc_order: Vec<String>,
    lowering_shapes: HashMap<String, ProcLoweringShape>,
}

pub(crate) struct ProcessorDesugarResult {
    program: Program,
    def_sample_oversample_factors: HashMap<String, usize>,
    proc_step_oversample_meta: HashMap<String, ProcStepOversampleMeta>,
}

const ALLOWED_SAMPLE_OVERSAMPLE_FACTORS: &[i64] = &[1, 2, 4, 8, 16, 32, 64];

fn validated_sample_oversample_factor(
    factor_expr: Option<&Expr>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    let Some(expr) = factor_expr else {
        return 1;
    };

    match expr {
        Expr::Int(value) => {
            if ALLOWED_SAMPLE_OVERSAMPLE_FACTORS.contains(value) {
                *value as usize
            } else {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} oversampling factor must be one of {{1,2,4,8,16,32,64}}; got {value}"
                    ),
                    0,
                    0,
                ));
                1
            }
        }
        _ => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} oversampling factor must be an integer literal in {{1,2,4,8,16,32,64}}"
                ),
                0,
                0,
            ));
            1
        }
    }
}

fn is_runtime_state_symbol_name(name: &str) -> bool {
    !name.starts_with(DECLARED_INPUT_TYPE_PREFIX)
        && !name.starts_with(DECLARED_OUTPUT_TYPE_PREFIX)
        && !name.starts_with(DECLARED_PARAM_TYPE_PREFIX)
        && !name.starts_with(DECLARED_DATA_ELEM_TYPE_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_ELEM_TYPE_PREFIX)
        && !name.starts_with(DECLARED_STRUCT_FIELD_TYPE_PREFIX)
        && !name.starts_with(DECLARED_INVALID_PLACEHOLDER_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_MULTICHANNEL_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_STATIC_CHANNELS_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_ELEM_F32_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_ELEM_F64_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_ELEM_I32_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_ELEM_I64_PREFIX)
        && !name.starts_with(DECLARED_BUFFER_ELEM_BOOL_PREFIX)
        && !name.starts_with(DECLARED_FUNCTION_RETURN_TYPE_PREFIX)
}

fn runtime_symbol_root(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

fn collect_runtime_state_roots(state_scalars: &HashMap<String, PrimitiveType>) -> HashSet<String> {
    state_scalars
        .keys()
        .filter(|name| is_runtime_state_symbol_name(name))
        .map(|name| runtime_symbol_root(name).to_owned())
        .collect::<HashSet<_>>()
}

fn internal_proc_index_call_signature(include_field_arg: bool) -> FnSignature {
    const PROC_INDEX_CALL_MAX_POSITIONAL_ARGS: usize = 16;

    let mut params = vec![
        PROC_INDEX_BASE_ARG.to_owned(),
        PROC_INDEX_EXPR_ARG.to_owned(),
    ];
    let mut defaults = vec![None, None];
    let mut param_types = vec![None, None];

    for idx in 0..PROC_INDEX_CALL_MAX_POSITIONAL_ARGS {
        params.push(format!("__proc_index_arg{idx}"));
        defaults.push(Some(Expr::Number(0.0)));
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

fn coerce_typed_events(
    events: &[EventDef],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedEvent> {
    let mut out = Vec::<TypedEvent>::new();
    let mut seen_events = HashSet::<String>::new();
    for event in events {
        if is_builtin_constant_name(&event.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "event name '{}' is reserved as a builtin constant",
                    event.name
                ),
                0,
                0,
            ));
            continue;
        }
        if !seen_events.insert(event.name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate event '{}'", event.name),
                0,
                0,
            ));
            continue;
        }
        let mut seen_params = HashSet::<String>::new();
        let mut typed_params = Vec::<TypedEventParam>::new();
        for param in &event.params {
            if !seen_params.insert(param.name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "duplicate event parameter '{}' in '{}'",
                        param.name, event.name
                    ),
                    0,
                    0,
                ));
                continue;
            }
            let typed = match &param.ty {
                EventParamType::Scalar(ty) => TypedEventParamType::Scalar(*ty),
                EventParamType::Array { elem, size } => {
                    let context = format!("event '{}.{}' array size", event.name, param.name);
                    let len = eval_data_size_expr(size, options, &context, errors).unwrap_or(1);
                    if len == 0 {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "event parameter '{}.{}' array size must be greater than zero",
                                event.name, param.name
                            ),
                            0,
                            0,
                        ));
                    }
                    TypedEventParamType::Array { elem: *elem, len }
                }
            };
            typed_params.push(TypedEventParam {
                name: param.name.clone(),
                ty: typed,
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

fn expand_proc_event_specs(
    proc: &ProcessorDef,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, ProcEventSpec> {
    let mut out = HashMap::<String, ProcEventSpec>::new();
    for event in &proc.events {
        if is_builtin_constant_name(&event.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' event name '{}' is reserved as a builtin constant",
                    proc.name, event.name
                ),
                0,
                0,
            ));
            continue;
        }
        if out.contains_key(&event.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate processor event '{}.{}'", proc.name, event.name),
                0,
                0,
            ));
            continue;
        }
        let mut params = Vec::<ProcEventParamSpec>::new();
        for param in &event.params {
            match &param.ty {
                EventParamType::Scalar(ty) => params.push(ProcEventParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcEventParamSlotSpec {
                        name: param.name.clone(),
                        ty: *ty,
                    }],
                }),
                EventParamType::Array { elem, size } => {
                    let context = format!(
                        "processor '{}.{}' event parameter '{}'",
                        proc.name, event.name, param.name
                    );
                    let len = eval_data_size_expr(size, options, &context, errors).unwrap_or(1);
                    if len == 0 {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor '{}.{}' event parameter '{}' array size must be greater than zero",
                                proc.name, event.name, param.name
                            ),
                            0,
                            0,
                        ));
                    }
                    let mut slots = Vec::<ProcEventParamSlotSpec>::new();
                    for idx in 0..len {
                        slots.push(ProcEventParamSlotSpec {
                            name: format!("{}[{idx}]", param.name),
                            ty: *elem,
                        });
                    }
                    params.push(ProcEventParamSpec {
                        name: param.name.clone(),
                        slots,
                    });
                }
            }
        }
        out.insert(event.name.clone(), ProcEventSpec { params });
    }
    out
}

fn validate_event_assign_target_restrictions(
    target: &AssignTarget,
    locals: &mut HashSet<String>,
    init_writable_roots: &HashSet<String>,
    immutable_roots: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    scalar_param_names: &HashSet<String>,
    array_param_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let (base, indexed) = match target {
        AssignTarget::Var(name) => (name.as_str(), false),
        AssignTarget::Index { base, .. } => (base.as_str(), true),
    };
    let root = runtime_symbol_root(base);

    if scalar_param_names.contains(root) {
        errors.push(Diagnostic::semantic(
            format!("cannot assign to immutable event parameter '{}'", root),
            0,
            0,
        ));
        return;
    }
    if array_param_names.contains(root) {
        errors.push(Diagnostic::semantic(
            format!(
                "cannot assign to immutable event array parameter '{}'",
                root
            ),
            0,
            0,
        ));
        return;
    }
    if input_names.contains(root) {
        errors.push(Diagnostic::semantic(
            format!("cannot assign to input symbol '{}' in event handler", root),
            0,
            0,
        ));
        return;
    }
    if output_names.contains(root) {
        errors.push(Diagnostic::semantic(
            format!("cannot assign to output symbol '{}' in event handler", root),
            0,
            0,
        ));
        return;
    }
    if immutable_roots.contains(root) {
        errors.push(Diagnostic::semantic(
            format!(
                "event handlers can only write init-root state; '{}' is not init-root",
                root
            ),
            0,
            0,
        ));
        return;
    }

    if indexed {
        if !init_writable_roots.contains(root) && !locals.contains(root) {
            errors.push(Diagnostic::semantic(
                format!(
                    "event handlers can only write init-root state; '{}' is not init-root",
                    root
                ),
                0,
                0,
            ));
        }
        return;
    }

    if base.contains('.') {
        if !init_writable_roots.contains(root) && !locals.contains(root) {
            errors.push(Diagnostic::semantic(
                format!(
                    "event handlers can only write init-root state; '{}' is not init-root",
                    root
                ),
                0,
                0,
            ));
        }
        return;
    }

    if init_writable_roots.contains(root) || locals.contains(root) {
        return;
    }
    locals.insert(base.to_owned());
}

fn validate_event_stmt_restrictions(
    stmt: &Stmt,
    locals: &mut HashSet<String>,
    init_writable_roots: &HashSet<String>,
    immutable_roots: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    scalar_param_names: &HashSet<String>,
    array_param_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || match stmt {
        Stmt::Assign { target, .. } => validate_event_assign_target_restrictions(
            target,
            locals,
            init_writable_roots,
            immutable_roots,
            input_names,
            output_names,
            scalar_param_names,
            array_param_names,
            errors,
        ),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            let mut then_locals = locals.clone();
            for nested in then_branch {
                validate_event_stmt_restrictions(
                    nested,
                    &mut then_locals,
                    init_writable_roots,
                    immutable_roots,
                    input_names,
                    output_names,
                    scalar_param_names,
                    array_param_names,
                    errors,
                );
            }
            let mut else_locals = locals.clone();
            for nested in else_branch {
                validate_event_stmt_restrictions(
                    nested,
                    &mut else_locals,
                    init_writable_roots,
                    immutable_roots,
                    input_names,
                    output_names,
                    scalar_param_names,
                    array_param_names,
                    errors,
                );
            }
            locals.extend(then_locals);
            locals.extend(else_locals);
        }
        Stmt::For { var, body, .. } => {
            let mut loop_locals = locals.clone();
            loop_locals.insert(var.clone());
            for nested in body {
                validate_event_stmt_restrictions(
                    nested,
                    &mut loop_locals,
                    init_writable_roots,
                    immutable_roots,
                    input_names,
                    output_names,
                    scalar_param_names,
                    array_param_names,
                    errors,
                );
            }
            locals.extend(loop_locals);
        }
        Stmt::While { body, .. } => {
            let mut loop_locals = locals.clone();
            for nested in body {
                validate_event_stmt_restrictions(
                    nested,
                    &mut loop_locals,
                    init_writable_roots,
                    immutable_roots,
                    input_names,
                    output_names,
                    scalar_param_names,
                    array_param_names,
                    errors,
                );
            }
            locals.extend(loop_locals);
        }
        Stmt::Expr { .. } | Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn analyze_runtime_scope_stmts<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    locals: &HashSet<String>,
    known_scalars: HashSet<String>,
    local_aliases: LocalAliasTypes,
    local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    forbidden_assign_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let ctx = RuntimeStmtAnalysisCtx {
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        input_names,
        output_names,
        forbidden_assign_names,
        param_names,
        struct_defs,
        fn_signatures,
        options,
    };
    let mut state = RuntimeStmtAnalysisState {
        known_scalars,
        local_aliases,
        local_array_aliases,
    };
    analyze_runtime_stmts(stmts, locals, &ctx, &mut state, errors);
}

pub(super) fn build_known_scalars_from_state(
    base_names: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
) -> HashSet<String> {
    let mut known = base_names.clone();
    known.extend(state_scalars.keys().cloned());
    known
}

pub(super) fn extend_known_scalars<'a>(
    known_scalars: &mut HashSet<String>,
    names: impl IntoIterator<Item = &'a String>,
) {
    known_scalars.extend(names.into_iter().cloned());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn register_and_analyze_runtime_scope<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    registration_input_names: &HashSet<String>,
    registration_output_names: &HashSet<String>,
    registration_param_names: &HashSet<String>,
    registration_scope: &StateRegistrationScope<'_>,
    runtime_locals: &HashSet<String>,
    runtime_known_scalars: HashSet<String>,
    runtime_local_aliases: LocalAliasTypes,
    runtime_local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    runtime_input_names: &HashSet<String>,
    runtime_output_names: &HashSet<String>,
    runtime_forbidden_assign_names: &HashSet<String>,
    runtime_param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let stmts = stmts.into_iter().collect::<Vec<_>>();
    register_scope_state(
        stmts.iter().copied(),
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        registration_input_names,
        registration_output_names,
        registration_param_names,
        registration_scope,
    );
    let mut runtime_known_scalars = runtime_known_scalars;
    runtime_known_scalars.extend(state_scalars.keys().cloned());
    analyze_runtime_scope_stmts(
        stmts.iter().copied(),
        runtime_locals,
        runtime_known_scalars,
        runtime_local_aliases,
        runtime_local_array_aliases,
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        runtime_input_names,
        runtime_output_names,
        runtime_forbidden_assign_names,
        runtime_param_names,
        struct_defs,
        fn_signatures,
        options,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn analyze_runtime_events(
    typed_events: &[TypedEvent],
    event_known_scalars_seed: &HashSet<String>,
    event_array_alias_seed: &HashMap<String, LocalArrayAliasInfo>,
    event_immutable_param_seed: &HashSet<String>,
    init_writable_roots: &HashSet<String>,
    immutable_event_roots: &HashSet<String>,
    validation_input_names: &HashSet<String>,
    validation_output_names: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let empty_runtime_inputs = HashSet::<String>::new();
    let empty_runtime_outputs = HashSet::<String>::new();
    let runtime_loop_vars = HashSet::<String>::new();

    for event in typed_events {
        let mut event_locals = HashSet::<String>::new();
        let mut scalar_event_params = HashSet::<String>::new();
        let mut array_event_params = HashSet::<String>::new();
        let mut event_known_scalars = event_known_scalars_seed.clone();
        let mut event_local_aliases = LocalAliasTypes::new();
        let mut event_local_data_aliases = event_array_alias_seed.clone();
        for param in &event.params {
            match param.ty {
                TypedEventParamType::Scalar(ty) => {
                    scalar_event_params.insert(param.name.clone());
                    event_known_scalars.insert(param.name.clone());
                    event_local_aliases.insert(param.name.clone(), ty);
                }
                TypedEventParamType::Array { elem, len } => {
                    array_event_params.insert(param.name.clone());
                    event_local_data_aliases.insert(
                        param.name.clone(),
                        LocalArrayAliasInfo {
                            len,
                            elem_ty: elem,
                            elem_struct: None,
                            writable: false,
                        },
                    );
                }
            }
        }

        let mut event_param_immutable = event_immutable_param_seed.clone();
        event_param_immutable.extend(scalar_event_params.iter().cloned());

        for stmt in &event.body {
            validate_event_stmt_restrictions(
                stmt,
                &mut event_locals,
                init_writable_roots,
                immutable_event_roots,
                validation_input_names,
                validation_output_names,
                &scalar_event_params,
                &array_event_params,
                errors,
            );
        }

        analyze_runtime_scope_stmts(
            event.body.iter(),
            &runtime_loop_vars,
            event_known_scalars,
            event_local_aliases,
            event_local_data_aliases,
            state_scalars,
            state_arrays,
            state_array_struct_roots,
            struct_instances,
            &empty_runtime_inputs,
            &empty_runtime_outputs,
            validation_output_names,
            &event_param_immutable,
            struct_defs,
            fn_signatures,
            options,
            errors,
        );
    }
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
    }

    let struct_symbols = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let struct_defs_by_name = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some((s.name.clone(), s.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let proc_symbols = proc_defs
        .iter()
        .map(|p| p.name.clone())
        .collect::<HashSet<_>>();
    let ctor_symbols = struct_symbols
        .iter()
        .cloned()
        .chain(proc_symbols.iter().cloned())
        .collect::<HashSet<_>>();
    let proc_defs_by_name = proc_defs
        .iter()
        .map(|p| (p.name.clone(), p.clone()))
        .collect::<HashMap<_, _>>();
    let proc_primary_output_types = proc_defs
        .iter()
        .map(|p| (p.name.clone(), infer_primary_output_type_from_processor(p)))
        .collect::<HashMap<_, _>>();
    let pre_desugar_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(d) => Some(d.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
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

    let mut base_shapes = HashMap::<String, ProcBaseShape>::new();
    let mut proc_api = HashMap::<String, ProcApi>::new();
    let mut proc_order = Vec::<String>::new();
    for proc in &proc_defs {
        if !proc.has_sample_block {
            errors.push(Diagnostic::semantic(
                format!("processor '{}' must declare sample block", proc.name),
                0,
                0,
            ));
        }
        if base_shapes.contains_key(&proc.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate processor '{}'", proc.name),
                0,
                0,
            ));
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
            &proc_primary_output_types,
            &struct_defs_by_name,
            &ctor_symbols,
            &pre_desugar_def_return_types,
            &pre_desugar_fn_signatures,
            &proc_defs_by_name,
            errors,
        );
        if shape.outs.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' must declare outs block or assign to outN in sample",
                    proc.name
                ),
                0,
                0,
            ));
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
                events: expand_proc_event_specs(proc, options, errors),
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

    Some(ProcLoweringEnv {
        struct_defs_by_name,
        proc_defs_by_name,
        proc_api,
        proc_order,
        lowering_shapes,
    })
}

#[derive(Debug, Clone)]
struct OverloadCandidate {
    internal_name: String,
    signature: FnSignature,
}

#[derive(Debug, Clone)]
enum OverloadArgShape {
    Scalar(PrimitiveType),
    Struct(String),
    Array,
    Buffer {
        elem_ty: PrimitiveType,
        channels: TypedBufferChannels,
    },
    Unknown,
}

#[derive(Debug, Clone, Default)]
struct OverloadRewriteEnv {
    scalar_types: HashMap<String, PrimitiveType>,
    struct_instances: HashMap<String, String>,
    array_elem_types: HashMap<String, PrimitiveType>,
    buffer_types: HashMap<String, (PrimitiveType, TypedBufferChannels)>,
}

fn overload_internal_name(public_name: &str, ordinal: usize) -> String {
    format!(
        "__omni_ovl_{}_{}",
        sanitize_symbol_component(public_name),
        ordinal
    )
}

fn const_positive_usize_for_overload(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int(v) if *v > 0 => usize::try_from(*v).ok(),
        Expr::Number(v) if *v > 0.0 && v.fract() == 0.0 => usize::try_from(*v as i64).ok(),
        _ => None,
    }
}

fn primitive_type_for_number_literal(v: i64) -> PrimitiveType {
    if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
        PrimitiveType::I32
    } else {
        PrimitiveType::I64
    }
}

fn merge_numeric_types_no_diag(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
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
        _ => None,
    }
}

fn infer_scalar_expr_type_for_overload(
    expr: &Expr,
    env: &OverloadRewriteEnv,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(primitive_type_for_number_literal(*v)),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::Var(name) => {
            if let Some(ty) = builtin_constant_type(name) {
                return Some(ty);
            }
            if let Some((base, field)) = split_simple_field_path(name) {
                let flat = format!("{base}.{field}");
                if let Some(ty) = env.scalar_types.get(&flat).copied() {
                    return Some(ty);
                }
            }
            env.scalar_types.get(name).copied()
        }
        Expr::Index { base, .. } => {
            if let Some(elem_ty) = env.array_elem_types.get(base).copied() {
                return Some(elem_ty);
            }
            if let Some((elem_ty, _)) = env.buffer_types.get(base) {
                return Some(*elem_ty);
            }
            if let Some((root, field)) = split_simple_field_path(base) {
                let flat = format!("{root}.{field}");
                if let Some(elem_ty) = env.array_elem_types.get(&flat).copied() {
                    return Some(elem_ty);
                }
            }
            None
        }
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Call { args, .. } => {
            if args.is_empty() {
                return Some(PrimitiveType::F32);
            }
            let mut acc = PrimitiveType::F32;
            for arg in args {
                let Some(arg_ty) = infer_scalar_expr_type_for_overload(arg, env) else {
                    return None;
                };
                let Some(merged) = merge_numeric_types_no_diag(acc, arg_ty) else {
                    return None;
                };
                acc = merged;
            }
            Some(acc)
        }
        Expr::UserCall { .. } => None,
        Expr::Binary { lhs, rhs, .. } => {
            let lhs_ty = infer_scalar_expr_type_for_overload(lhs, env)?;
            let rhs_ty = infer_scalar_expr_type_for_overload(rhs, env)?;
            merge_numeric_types_no_diag(lhs_ty, rhs_ty)
        }
        Expr::ArrayCtor { .. } | Expr::ArrayLiteral(_) => None,
    }
}

fn infer_array_elem_type_for_overload(
    expr: &Expr,
    env: &OverloadRewriteEnv,
) -> Option<PrimitiveType> {
    match expr {
        Expr::ArrayLiteral(values) => {
            let first = values.first()?;
            infer_scalar_expr_type_for_overload(first, env)
        }
        Expr::ArrayCtor { spec, .. } => match &spec.elem {
            ArrayElemType::Primitive(elem) => Some(*elem),
            ArrayElemType::Struct(_) => None,
        },
        _ => None,
    }
}

fn infer_overload_arg_shape(expr: &Expr, env: &OverloadRewriteEnv) -> OverloadArgShape {
    match expr {
        Expr::Var(name) => {
            if let Some(struct_name) = env.struct_instances.get(name) {
                return OverloadArgShape::Struct(struct_name.clone());
            }
            if env.array_elem_types.contains_key(name) {
                return OverloadArgShape::Array;
            }
            if let Some((elem_ty, channels)) = env.buffer_types.get(name) {
                return OverloadArgShape::Buffer {
                    elem_ty: *elem_ty,
                    channels: channels.clone(),
                };
            }
            if let Some(ty) = infer_scalar_expr_type_for_overload(expr, env) {
                return OverloadArgShape::Scalar(ty);
            }
            OverloadArgShape::Unknown
        }
        Expr::Index { base, .. } => {
            if let Some((elem_ty, _)) = env.buffer_types.get(base) {
                return OverloadArgShape::Scalar(*elem_ty);
            }
            if let Some(elem_ty) = env.array_elem_types.get(base).copied() {
                return OverloadArgShape::Scalar(elem_ty);
            }
            if let Some((root, field)) = split_simple_field_path(base) {
                let flat = format!("{root}.{field}");
                if let Some(elem_ty) = env.array_elem_types.get(&flat).copied() {
                    return OverloadArgShape::Scalar(elem_ty);
                }
            }
            OverloadArgShape::Unknown
        }
        _ => {
            if let Some(ty) = infer_scalar_expr_type_for_overload(expr, env) {
                OverloadArgShape::Scalar(ty)
            } else {
                OverloadArgShape::Unknown
            }
        }
    }
}

fn score_buffer_match(
    expected: &BufferType,
    arg_elem_ty: PrimitiveType,
    arg_channels: &TypedBufferChannels,
) -> Option<i32> {
    let expected_elem = match expected.elem {
        BufferElemType::Primitive(ty) => ty,
        BufferElemType::Generic(_) => return Some(3),
    };
    if expected_elem != arg_elem_ty {
        return None;
    }
    match &expected.channels {
        BufferChannels::Mono => match arg_channels {
            TypedBufferChannels::Mono => Some(0),
            TypedBufferChannels::Static(ch) if *ch == 1 => Some(0),
            _ => None,
        },
        BufferChannels::Dynamic => match arg_channels {
            TypedBufferChannels::Mono => None,
            TypedBufferChannels::Static(ch) if *ch > 1 => Some(1),
            TypedBufferChannels::Dynamic => Some(0),
            _ => None,
        },
        BufferChannels::Static(expr) => {
            let expected_ch = const_positive_usize_for_overload(expr);
            match expected_ch {
                Some(ch) if ch <= 1 => match arg_channels {
                    TypedBufferChannels::Mono => Some(0),
                    TypedBufferChannels::Static(actual) if *actual == 1 => Some(0),
                    _ => None,
                },
                Some(ch) => match arg_channels {
                    TypedBufferChannels::Static(actual) if *actual == ch => Some(0),
                    TypedBufferChannels::Dynamic => Some(1),
                    _ => None,
                },
                None => match arg_channels {
                    TypedBufferChannels::Mono => None,
                    TypedBufferChannels::Static(ch) if *ch > 1 => Some(1),
                    TypedBufferChannels::Dynamic => Some(1),
                    _ => None,
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Def monomorphization — generic struct, untyped array [], bare buffer params
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum MonoParamKey {
    /// Non-generic param — keep as-is.
    Passthrough,
    /// Resolved concrete struct name (e.g. "Voice.__gen__f32").
    ResolvedStruct(String),
    /// Resolved array element type.
    ResolvedArray(PrimitiveType),
    /// Resolved buffer element type + channels.
    ResolvedBuffer(PrimitiveType, TypedBufferChannels),
}

fn mono_def_name(base: &str, keys: &[MonoParamKey]) -> String {
    let mut suffix = String::new();
    for key in keys {
        match key {
            MonoParamKey::Passthrough => {}
            MonoParamKey::ResolvedStruct(s) => {
                suffix.push_str("__");
                suffix.push_str(&sanitize_symbol_component(s));
            }
            MonoParamKey::ResolvedArray(prim) => {
                suffix.push_str("__arr_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
            }
            MonoParamKey::ResolvedBuffer(prim, ch) => {
                suffix.push_str("__buf_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
                match ch {
                    TypedBufferChannels::Mono => {}
                    TypedBufferChannels::Static(n) => {
                        suffix.push_str(&format!("_{n}ch"));
                    }
                    TypedBufferChannels::Dynamic => suffix.push_str("_dyn"),
                }
            }
        }
    }
    format!("{base}.__mono{suffix}")
}

fn infer_mono_arg_key(
    arg_expr: &Expr,
    param_ty: Option<&FnParamType>,
    env: &OverloadRewriteEnv,
    generic_templates: &HashSet<String>,
) -> Option<MonoParamKey> {
    match param_ty {
        Some(FnParamType::Struct(struct_name)) if generic_templates.contains(struct_name) => {
            // Arg must be a variable whose concrete struct type is known.
            if let Expr::Var(var_name) = arg_expr {
                if let Some(concrete) = env.struct_instances.get(var_name) {
                    // Check if this concrete name is a specialization of the template.
                    if concrete.starts_with(struct_name) || concrete.contains(".__gen__") {
                        return Some(MonoParamKey::ResolvedStruct(concrete.clone()));
                    }
                    // Could be a different struct that happens to match.
                    return Some(MonoParamKey::ResolvedStruct(concrete.clone()));
                }
            }
            None // Can't determine concrete struct type
        }
        Some(FnParamType::Array(None)) => {
            if let Expr::Var(var_name) = arg_expr {
                if let Some(elem_ty) = env.array_elem_types.get(var_name) {
                    return Some(MonoParamKey::ResolvedArray(*elem_ty));
                }
            }
            // Default to f32 if we can't infer
            Some(MonoParamKey::ResolvedArray(PrimitiveType::F32))
        }
        Some(FnParamType::BareBuffer) => {
            if let Expr::Var(var_name) = arg_expr {
                if let Some((elem_ty, channels)) = env.buffer_types.get(var_name) {
                    return Some(MonoParamKey::ResolvedBuffer(*elem_ty, channels.clone()));
                }
            }
            // Default to f32 mono
            Some(MonoParamKey::ResolvedBuffer(
                PrimitiveType::F32,
                TypedBufferChannels::Mono,
            ))
        }
        _ => Some(MonoParamKey::Passthrough),
    }
}

fn generate_mono_def(
    original: &FunctionDef,
    original_sig: &FnSignature,
    keys: &[MonoParamKey],
    mono_name: &str,
    _generic_templates: &HashSet<String>,
) -> (FunctionDef, FnSignature) {
    let mut new_def = original.clone();
    new_def.name = mono_name.to_owned();
    let mut new_sig = original_sig.clone();

    for (idx, key) in keys.iter().enumerate() {
        match key {
            MonoParamKey::Passthrough => {}
            MonoParamKey::ResolvedStruct(concrete_name) => {
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Struct(concrete_name.clone()));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Struct(concrete_name.clone()));
                }
            }
            MonoParamKey::ResolvedArray(elem_ty) => {
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Array(Some(*elem_ty)));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Array(Some(*elem_ty)));
                }
            }
            MonoParamKey::ResolvedBuffer(elem_ty, channels) => {
                let buf_ty = BufferType {
                    elem: BufferElemType::Primitive(*elem_ty),
                    channels: match channels {
                        TypedBufferChannels::Mono => BufferChannels::Mono,
                        TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
                        TypedBufferChannels::Static(n) => {
                            BufferChannels::Static(Expr::Int(*n as i64))
                        }
                    },
                };
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Buffer(buf_ty.clone()));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Buffer(buf_ty));
                }
            }
        }
    }

    // Also need to desugar method calls in the mono body if we resolved struct params
    // This happens when the original def body calls methods on a generic struct param.
    // The method desugaring already happened on the original, so we just need the param
    // type to be correct for inference to work.

    (new_def, new_sig)
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_stmts(
    stmts: &mut Vec<Stmt>,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    _struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
) {
    for stmt in stmts.iter_mut() {
        monomorphize_calls_in_stmt(
            stmt,
            env,
            mono_eligible,
            fn_signatures,
            original_defs,
            generic_templates,
            generated_defs,
            generated_sigs,
            mono_cache,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_stmt(
    stmt: &mut Stmt,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
) {
    match stmt {
        Stmt::Assign { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
        }
        Stmt::Expr { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
        }
        Stmt::Return { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            monomorphize_calls_in_expr(
                cond,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
            for s in then_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
            for s in else_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            for s in body.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_expr(
    expr: &mut Expr,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            // Recurse into arg expressions first
            for arg in args.iter_mut() {
                monomorphize_calls_in_expr(
                    &mut arg.expr,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }

            if !mono_eligible.contains(name.as_str()) {
                return;
            }

            let Some(sig) = fn_signatures.get(name.as_str()) else {
                return;
            };

            // Build monomorphization key from each argument
            let resolved_args = crate::def_inference::resolve_call_args(
                args,
                &sig.params,
                &sig.defaults,
                false,
                false,
                &format!("mono call '{name}'"),
                &mut Vec::new(),
            );

            let mut keys = Vec::with_capacity(sig.params.len());
            let mut all_resolved = true;
            for (idx, _param_name) in sig.params.iter().enumerate() {
                let param_ty = sig.param_types.get(idx).and_then(|t| t.as_ref());
                let arg_expr = resolved_args.get(idx).and_then(|a| a.as_ref());
                if let Some(arg_expr) = arg_expr {
                    if let Some(key) =
                        infer_mono_arg_key(arg_expr, param_ty, env, generic_templates)
                    {
                        keys.push(key);
                    } else {
                        all_resolved = false;
                        break;
                    }
                } else {
                    // Use default — passthrough
                    keys.push(MonoParamKey::Passthrough);
                }
            }

            if !all_resolved {
                return;
            }

            // Check if all keys are passthrough (nothing to monomorphize)
            if keys.iter().all(|k| matches!(k, MonoParamKey::Passthrough)) {
                return;
            }

            let cache_key = (name.clone(), keys.clone());
            let mono_name = if let Some(cached) = mono_cache.get(&cache_key) {
                cached.clone()
            } else {
                let new_name = mono_def_name(name, &keys);

                // Find original def
                let original = original_defs
                    .iter()
                    .find(|d| d.name == *name)
                    .or_else(|| generated_defs.iter().find(|d| d.name == *name));

                if let Some(original) = original {
                    let (gen_def, gen_sig) =
                        generate_mono_def(original, sig, &keys, &new_name, generic_templates);
                    generated_defs.push(gen_def);
                    generated_sigs.insert(new_name.clone(), gen_sig);
                }

                mono_cache.insert(cache_key, new_name.clone());
                new_name
            };

            *name = mono_name;
        }
        Expr::Binary { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            monomorphize_calls_in_expr(
                lhs,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
            monomorphize_calls_in_expr(
                rhs,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args.iter_mut() {
                monomorphize_calls_in_expr(
                    arg,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            monomorphize_calls_in_expr(
                inner,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
        }
        Expr::ArrayLiteral(elems) => {
            for elem in elems.iter_mut() {
                monomorphize_calls_in_expr(
                    elem,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
        }
        _ => {}
    }
}

fn score_overload_param_match(
    arg_shape: &OverloadArgShape,
    param_ty: Option<&FnParamType>,
) -> Option<i32> {
    match param_ty {
        Some(FnParamType::Primitive(expected)) => match arg_shape {
            OverloadArgShape::Scalar(src) => {
                if *src == *expected {
                    Some(0)
                } else if can_implicitly_assign(*src, *expected) {
                    Some(1)
                } else {
                    None
                }
            }
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Struct(expected_struct)) => match arg_shape {
            OverloadArgShape::Struct(actual_struct) if actual_struct == expected_struct => Some(0),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Buffer(expected_buffer)) => match arg_shape {
            OverloadArgShape::Buffer { elem_ty, channels } => {
                score_buffer_match(expected_buffer, *elem_ty, channels)
            }
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Array(Some(_expected_elem))) => match arg_shape {
            OverloadArgShape::Array => Some(0),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Array(None)) => match arg_shape {
            OverloadArgShape::Array => Some(1),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::BareBuffer) => match arg_shape {
            OverloadArgShape::Buffer { .. } => Some(1),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        None => Some(3),
    }
}

fn format_fn_param_for_overload(name: &str, ty: Option<&FnParamType>, has_default: bool) -> String {
    let typed = match ty {
        Some(FnParamType::Primitive(prim)) => format!("{name}: {prim:?}").to_lowercase(),
        Some(FnParamType::Struct(struct_name)) => format!("{name}: {struct_name}"),
        Some(FnParamType::Buffer(buffer_ty)) => format!("{name}: {:?}", buffer_ty),
        Some(FnParamType::Array(Some(prim))) => format!("{name}: {prim:?}[]").to_lowercase(),
        Some(FnParamType::Array(None)) => format!("{name}: []"),
        Some(FnParamType::BareBuffer) => format!("{name}: buffer"),
        None => name.to_owned(),
    };
    if has_default {
        format!("{typed} = ...")
    } else {
        typed
    }
}

fn format_overload_signature(name: &str, signature: &FnSignature) -> String {
    let params = signature
        .params
        .iter()
        .enumerate()
        .map(|(idx, param_name)| {
            format_fn_param_for_overload(
                param_name,
                signature.param_types.get(idx).and_then(|p| p.as_ref()),
                signature
                    .defaults
                    .get(idx)
                    .and_then(|d| d.as_ref())
                    .is_some(),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({params})")
}

fn resolve_overloaded_call_name(
    public_name: &str,
    args: &[CallArg],
    env: &OverloadRewriteEnv,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let candidates = overloads.get(public_name)?;
    if candidates.len() == 1 {
        return Some(candidates[0].internal_name.clone());
    }

    let mut scored = Vec::<(i32, usize)>::new();
    for (cand_idx, cand) in candidates.iter().enumerate() {
        let mut bind_errors = Vec::new();
        let resolved = resolve_call_args(
            args,
            &cand.signature.params,
            &cand.signature.defaults,
            false,
            false,
            &format!("function '{public_name}' call"),
            &mut bind_errors,
        );
        if !bind_errors.is_empty() {
            continue;
        }

        let mut total_score = 0_i32;
        let mut viable = true;
        for (param_idx, arg_expr) in resolved.into_iter().enumerate() {
            if let Some(arg_expr) = arg_expr {
                let arg_shape = infer_overload_arg_shape(arg_expr, env);
                let param_ty = cand
                    .signature
                    .param_types
                    .get(param_idx)
                    .and_then(|t| t.as_ref());
                let Some(score) = score_overload_param_match(&arg_shape, param_ty) else {
                    viable = false;
                    break;
                };
                total_score += score;
            } else {
                // Slight preference for overloads requiring fewer defaulted params.
                total_score += 1;
            }
        }
        if viable {
            scored.push((total_score, cand_idx));
        }
    }

    if scored.is_empty() {
        let overload_list = candidates
            .iter()
            .map(|c| format_overload_signature(public_name, &c.signature))
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(Diagnostic::semantic(
            format!(
                "no matching overload for function '{}' (candidates: {})",
                public_name, overload_list
            ),
            0,
            0,
        ));
        return Some(candidates[0].internal_name.clone());
    }

    let best_score = scored
        .iter()
        .map(|(score, _)| *score)
        .min()
        .unwrap_or(i32::MAX);
    let best = scored
        .into_iter()
        .filter(|(score, _)| *score == best_score)
        .map(|(_, idx)| idx)
        .collect::<Vec<_>>();
    if best.len() > 1 {
        let overload_list = best
            .iter()
            .filter_map(|idx| candidates.get(*idx))
            .map(|c| format_overload_signature(public_name, &c.signature))
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(Diagnostic::semantic(
            format!(
                "ambiguous overload for function '{}'; matching candidates: {}",
                public_name, overload_list
            ),
            0,
            0,
        ));
    }

    best.first()
        .and_then(|idx| candidates.get(*idx))
        .map(|cand| cand.internal_name.clone())
}

fn update_overload_env_after_assign(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    env: &mut OverloadRewriteEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    let AssignTarget::Var(name) = target else {
        return;
    };

    if let Some(declared) = decl_ty {
        env.scalar_types.insert(name.clone(), declared);
        env.struct_instances.remove(name);
        return;
    }

    if let Some(elem_ty) = infer_array_elem_type_for_overload(expr, env) {
        env.array_elem_types.insert(name.clone(), elem_ty);
        env.scalar_types.remove(name);
        env.struct_instances.remove(name);
        return;
    }

    if let Expr::UserCall { name: callee, .. } = expr {
        if struct_defs.contains_key(callee) {
            env.struct_instances.insert(name.clone(), callee.clone());
            env.scalar_types.remove(name);
            return;
        }
    }

    if let Expr::Var(src) = expr {
        if let Some(struct_name) = env.struct_instances.get(src).cloned() {
            env.struct_instances.insert(name.clone(), struct_name);
            env.scalar_types.remove(name);
            return;
        }
    }

    if let Some(ty) = infer_scalar_expr_type_for_overload(expr, env) {
        env.scalar_types.insert(name.clone(), ty);
        env.struct_instances.remove(name);
    }
}

fn rewrite_overloaded_calls_in_expr(
    expr: &mut Expr,
    env: &OverloadRewriteEnv,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_overloaded_calls_in_expr(index, env, overloads, errors);
        }
        Expr::ArrayCtor { spec, init } => {
            rewrite_overloaded_calls_in_expr(&mut spec.size, env, overloads, errors);
            if let Some(values) = init {
                for value in values {
                    rewrite_overloaded_calls_in_expr(value, env, overloads, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_overloaded_calls_in_expr(lhs, env, overloads, errors);
            rewrite_overloaded_calls_in_expr(rhs, env, overloads, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_overloaded_calls_in_expr(arg, env, overloads, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            rewrite_overloaded_calls_in_expr(expr, env, overloads, errors);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_overloaded_calls_in_expr(value, env, overloads, errors);
            }
        }
        Expr::UserCall {
            name,
            type_args: _,
            args,
        } => {
            for arg in args.iter_mut() {
                rewrite_overloaded_calls_in_expr(&mut arg.expr, env, overloads, errors);
            }
            if let Some(resolved_name) =
                resolve_overloaded_call_name(name, args, env, overloads, errors)
            {
                *name = resolved_name;
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn rewrite_overloaded_calls_in_stmt_list(
    stmts: &mut [Stmt],
    env: &mut OverloadRewriteEnv,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        with_stmt_diag_context_mut(stmt, |stmt| match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                expr,
                ..
            } => {
                if let AssignTarget::Index { index, .. } = target {
                    rewrite_overloaded_calls_in_expr(index, env, overloads, errors);
                }
                rewrite_overloaded_calls_in_expr(expr, env, overloads, errors);
                update_overload_env_after_assign(target, *decl_ty, expr, env, struct_defs);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                rewrite_overloaded_calls_in_expr(expr, env, overloads, errors);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_overloaded_calls_in_expr(cond, env, overloads, errors);
                let mut then_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    then_branch,
                    &mut then_env,
                    overloads,
                    struct_defs,
                    errors,
                );
                let mut else_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    else_branch,
                    &mut else_env,
                    overloads,
                    struct_defs,
                    errors,
                );
                env.scalar_types = then_env
                    .scalar_types
                    .iter()
                    .filter_map(|(name, ty)| {
                        else_env
                            .scalar_types
                            .get(name)
                            .copied()
                            .filter(|other| other == ty)
                            .map(|_| (name.clone(), *ty))
                    })
                    .collect::<HashMap<_, _>>();
                env.struct_instances = then_env
                    .struct_instances
                    .iter()
                    .filter_map(|(name, ty)| {
                        else_env
                            .struct_instances
                            .get(name)
                            .filter(|other| *other == ty)
                            .map(|_| (name.clone(), ty.clone()))
                    })
                    .collect::<HashMap<_, _>>();
                env.array_elem_types = then_env
                    .array_elem_types
                    .iter()
                    .filter_map(|(name, ty)| {
                        else_env
                            .array_elem_types
                            .get(name)
                            .copied()
                            .filter(|other| other == ty)
                            .map(|_| (name.clone(), *ty))
                    })
                    .collect::<HashMap<_, _>>();
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                rewrite_overloaded_calls_in_expr(start, env, overloads, errors);
                rewrite_overloaded_calls_in_expr(end, env, overloads, errors);
                if let Some(step_expr) = step {
                    rewrite_overloaded_calls_in_expr(step_expr, env, overloads, errors);
                }
                let mut body_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    body,
                    &mut body_env,
                    overloads,
                    struct_defs,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                rewrite_overloaded_calls_in_expr(cond, env, overloads, errors);
                let mut body_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    body,
                    &mut body_env,
                    overloads,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        });
    }
}

fn prepare_function_overloads(
    defs: &mut [FunctionDef],
    method_self_struct: &HashMap<String, String>,
) -> (
    HashMap<String, Vec<OverloadCandidate>>,
    HashMap<String, String>,
) {
    let mut by_public_top_level = HashMap::<String, Vec<usize>>::new();
    for (idx, def) in defs.iter().enumerate() {
        if method_self_struct.contains_key(&def.name) {
            continue;
        }
        by_public_top_level
            .entry(def.name.clone())
            .or_default()
            .push(idx);
    }

    let mut internal_to_public = HashMap::<String, String>::new();
    for (public_name, indices) in &by_public_top_level {
        if indices.len() <= 1 {
            continue;
        }
        for (ordinal, idx) in indices.iter().enumerate() {
            let internal_name = overload_internal_name(public_name, ordinal + 1);
            defs[*idx].name = internal_name.clone();
            internal_to_public.insert(internal_name, public_name.clone());
        }
    }

    let mut overloads = HashMap::<String, Vec<OverloadCandidate>>::new();
    for def in defs.iter() {
        let public_name = internal_to_public
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| def.name.clone());
        internal_to_public
            .entry(def.name.clone())
            .or_insert_with(|| public_name.clone());
        overloads
            .entry(public_name)
            .or_default()
            .push(OverloadCandidate {
                internal_name: def.name.clone(),
                signature: FnSignature {
                    params: def.params.iter().map(|p| p.name.clone()).collect(),
                    defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                    param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                    type_params: def.type_params.clone(),
                },
            });
    }

    (overloads, internal_to_public)
}

pub(crate) fn desugar_processors(
    mut program: Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ProcessorDesugarResult {
    rewrite_and_materialize_generic_processors(&mut program, errors);

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
        };
    };

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
    program.blocks.extend(generated_structs);
    program.blocks.extend(generated_defs);

    rewrite_top_level_proc_calls(&mut program, options, &lowering_shapes, &proc_api, errors);
    ProcessorDesugarResult {
        program,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
    }
}

pub fn analyze(program: Program) -> Result<TypedProgram, Vec<Diagnostic>> {
    analyze_with_options(program, AnalysisOptions::default())
}

pub fn analyze_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<TypedProgram, Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'block_size' must be greater than zero",
        )]);
    }

    let mut program = program;
    inject_auto_std_math(&mut program)?;

    let mut errors = Vec::new();
    let ProcessorDesugarResult {
        program,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
    } = desugar_processors(program, options, &mut errors);

    let mut seen_singleton = HashSet::new();
    for block in &program.blocks {
        let kind = block.kind();
        if matches!(kind, BlockKind::Def | BlockKind::Struct | BlockKind::Proc) {
            continue;
        }
        if !seen_singleton.insert(kind) {
            errors.push(Diagnostic::semantic(
                format!("duplicate block '{:?}'", kind).to_lowercase(),
                0,
                0,
            ));
        }
    }

    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(v)) => v.clone(),
        _ => Vec::new(),
    };
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(v)) => v.clone(),
        _ => Vec::new(),
    };
    let params = match program.block(BlockKind::Params) {
        Some(Block::Params(v)) => v.clone(),
        _ => Vec::new(),
    };
    let mut events = match program.block(BlockKind::Events) {
        Some(Block::Events(v)) => v.clone(),
        _ => Vec::new(),
    };
    let buffers = match program.block(BlockKind::Buffers) {
        Some(Block::Buffers(v)) => v.clone(),
        _ => Vec::new(),
    };
    let mut struct_defs_raw = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(def) => Some(def.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (init_default_decl_ty, mut init) = match program.block(BlockKind::Init) {
        Some(Block::Init(v)) => (v.default_ty.clone(), v.body.clone()),
        _ => (None, Vec::new()),
    };
    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut nested_block_sample = None;
    if let Some(Block::Block(exec)) = program.block(BlockKind::Block) {
        block_pre = exec.pre.clone();
        block_post = exec.post.clone();
        nested_block_sample = exec.sample.clone();
        if nested_block_sample.is_none() {
            errors.push(Diagnostic::semantic(
                "block section must include nested 'sample' block",
                0,
                0,
            ));
        }
    }
    let top_sample = match program.block(BlockKind::Sample) {
        Some(Block::Sample(v)) => Some(v.clone()),
        _ => None,
    };

    let sample_block = match (nested_block_sample, top_sample) {
        (Some(_), Some(_)) => {
            errors.push(Diagnostic::semantic(
                "sample block cannot be declared both at top-level and inside block",
                0,
                0,
            ));
            SampleBlock {
                oversample_factor: None,
                body: Vec::new(),
            }
        }
        (Some(v), None) => v,
        (None, Some(v)) => v,
        (None, None) => SampleBlock {
            oversample_factor: None,
            body: Vec::new(),
        },
    };
    let sample_oversample_factor = validated_sample_oversample_factor(
        sample_block.oversample_factor.as_ref(),
        "sample block",
        &mut errors,
    );
    let mut sample = sample_block.body;

    if sample.is_empty() {
        errors.push(Diagnostic::semantic(
            "missing required 'sample' block",
            0,
            0,
        ));
    }

    {
        let struct_symbols = struct_defs_raw
            .iter()
            .map(|s| s.name.clone())
            .collect::<HashSet<_>>();
        let mut callable_symbols = defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
        for p in program.blocks.iter().filter_map(|b| match b {
            Block::Proc(proc_def) => Some(proc_def),
            _ => None,
        }) {
            callable_symbols.insert(p.name.clone());
        }
        for s in &struct_defs_raw {
            callable_symbols.insert(s.name.clone());
            for method in &s.methods {
                callable_symbols.insert(format!("{}.{}", s.name, method.name));
            }
        }
        let struct_namespaces = collect_declared_namespaces(&struct_symbols);
        let callable_namespaces = collect_declared_namespaces(&callable_symbols);

        for s in &mut struct_defs_raw {
            let struct_ns = namespace_of_symbol(&s.name);
            for field in &mut s.fields {
                if let FieldType::Array(spec) = &mut field.ty {
                    if let ArrayElemType::Struct(name) = &mut spec.elem {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!("struct '{}.{}' array element type", s.name, field.name),
                            &mut errors,
                        );
                    }
                }
                if let Some(default) = &mut field.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("struct '{}.{}' default", s.name, field.name),
                    );
                }
            }
            for method in &mut s.methods {
                for param in &mut method.params {
                    if let Some(FnParamType::Struct(name)) = &mut param.ty {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!(
                                "method '{}.{}' parameter '{}'",
                                s.name, method.name, param.name
                            ),
                            &mut errors,
                        );
                    }
                    if let Some(default) = &mut param.default {
                        qualify_expr_namespaced_symbols(
                            default,
                            &struct_ns,
                            &callable_symbols,
                            &callable_namespaces,
                            &struct_symbols,
                            &struct_namespaces,
                            &mut errors,
                            &format!(
                                "method '{}.{}' parameter '{}' default",
                                s.name, method.name, param.name
                            ),
                        );
                    }
                }
                for stmt in &mut method.body {
                    qualify_stmt_namespaced_symbols(
                        stmt,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("method '{}.{}' body", s.name, method.name),
                    );
                }
            }
        }

        for def in &mut defs {
            let def_ns = namespace_of_symbol(&def.name);
            for param in &mut def.params {
                if let Some(FnParamType::Struct(name)) = &mut param.ty {
                    qualify_struct_type_name(
                        name,
                        &def_ns,
                        &struct_symbols,
                        &struct_namespaces,
                        &format!("function '{}' parameter '{}'", def.name, param.name),
                        &mut errors,
                    );
                }
                if let Some(default) = &mut param.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &def_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("function '{}' parameter '{}' default", def.name, param.name),
                    );
                }
            }
            for stmt in &mut def.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    &def_ns,
                    &callable_symbols,
                    &callable_namespaces,
                    &struct_symbols,
                    &struct_namespaces,
                    &mut errors,
                    &format!("function '{}' body", def.name),
                );
            }
        }

        for stmt in &mut init {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "init",
            );
        }
        for stmt in &mut block_pre {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "block pre",
            );
        }
        for stmt in &mut block_post {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "block post",
            );
        }
        for stmt in &mut sample {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "sample",
            );
        }
        for event in &mut events {
            for stmt in &mut event.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    "",
                    &callable_symbols,
                    &callable_namespaces,
                    &struct_symbols,
                    &struct_namespaces,
                    &mut errors,
                    &format!("event '{}'", event.name),
                );
            }
        }
    }

    let generic_struct_template_names: HashSet<String>;
    {
        let mut concrete_structs = Vec::<StructDef>::new();
        let mut generic_templates = HashMap::<String, StructDef>::new();
        for s in struct_defs_raw.drain(..) {
            if s.type_params.is_empty() {
                concrete_structs.push(s);
                continue;
            }
            if generic_templates.contains_key(&s.name) {
                errors.push(Diagnostic::semantic(
                    format!("duplicate generic struct '{}'", s.name),
                    0,
                    0,
                ));
                continue;
            }
            let mut seen = HashSet::new();
            for tp in &s.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "duplicate generic type parameter '{}' in struct '{}'",
                            tp, s.name
                        ),
                        0,
                        0,
                    ));
                }
            }
            generic_templates.insert(s.name.clone(), s);
        }
        generic_struct_template_names = generic_templates.keys().cloned().collect();

        let mut generated_specializations = HashMap::<String, StructDef>::new();
        for s in &mut concrete_structs {
            for field in &mut s.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = GenericInferenceLocals::default();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        &generic_templates,
                        &mut generated_specializations,
                        &mut errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut s.methods {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    &generic_templates,
                    &mut generated_specializations,
                    &mut errors,
                );
            }
        }
        for def in &mut defs {
            rewrite_generic_struct_ctor_stmt_list(
                &mut def.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }
        rewrite_generic_struct_ctor_stmt_list(
            &mut init,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_pre,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_post,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut sample,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        for event in &mut events {
            rewrite_generic_struct_ctor_stmt_list(
                &mut event.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }

        finalize_generated_generic_struct_specializations(
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );

        struct_defs_raw = concrete_structs;
        let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
        generated.sort_by(|a, b| a.name.cmp(&b.name));
        struct_defs_raw.extend(generated);
    }

    check_local_port_duplicates(&raw_ins, "input", &mut errors);
    check_local_port_duplicates(&raw_outs, "output", &mut errors);

    let inferred_io = infer_numbered_io_from_sample(&sample);
    let ins_ports = normalize_numbered_port_decls(&raw_ins, "in", inferred_io.max_in);
    let outs_ports = normalize_numbered_port_decls(&raw_outs, "out", inferred_io.max_out);
    let input_declared_names = ins_ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let output_declared_names = outs_ports
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let (ins, in_types, in_arrays, in_defaults, in_ranges) =
        expand_port_decls(&ins_ports, "input", options, &mut errors);
    let (outs, out_types, out_arrays, _out_defaults, _out_ranges) =
        expand_port_decls(&outs_ports, "output", options, &mut errors);

    let (typed_params, param_arrays) = coerce_params(&params, options, &mut errors);
    let typed_buffers = coerce_buffers(&buffers, options, &mut errors);
    let param_types = typed_params
        .iter()
        .map(|p| (p.name.clone(), p.ty))
        .collect::<HashMap<_, _>>();
    let param_ranges = typed_params
        .iter()
        .filter_map(|p| p.range.map(|r| (p.name.clone(), r)))
        .collect::<HashMap<_, _>>();
    let mut occupied_temp_names = HashSet::<String>::new();
    occupied_temp_names.extend(input_declared_names.iter().cloned());
    occupied_temp_names.extend(output_declared_names.iter().cloned());
    occupied_temp_names.extend(params.iter().map(|p| p.name.clone()));
    occupied_temp_names.extend(typed_buffers.iter().map(|b| b.name.clone()));

    let mut make_unique_temp = |base: String| -> String {
        if occupied_temp_names.insert(base.clone()) {
            return base;
        }
        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}_{idx}");
            if occupied_temp_names.insert(candidate.clone()) {
                return candidate;
            }
            idx += 1;
        }
    };

    let mut input_aliases = HashMap::<String, String>::new();
    let mut input_hoists = Vec::<Stmt>::new();
    let mut input_names = in_ranges.keys().cloned().collect::<Vec<_>>();
    input_names.sort();
    for name in input_names {
        let Some(range) = in_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *in_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__omni_clamped_in__{}",
            sanitize_symbol_component(&name)
        ));
        input_aliases.insert(name.clone(), alias.clone());
        input_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    let mut param_aliases = HashMap::<String, String>::new();
    let mut param_hoists = Vec::<Stmt>::new();
    let mut param_names_sorted = param_ranges.keys().cloned().collect::<Vec<_>>();
    param_names_sorted.sort();
    for name in param_names_sorted {
        let Some(range) = param_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *param_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__omni_clamped_param__{}",
            sanitize_symbol_component(&name)
        ));
        param_aliases.insert(name.clone(), alias.clone());
        param_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    for stmt in &mut block_pre {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }
    for stmt in &mut sample {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, true, true);
    }
    for stmt in &mut block_post {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }

    if !param_hoists.is_empty() {
        let mut rewritten = param_hoists;
        rewritten.append(&mut block_pre);
        block_pre = rewritten;
    }
    if !input_hoists.is_empty() {
        let mut rewritten = input_hoists;
        rewritten.append(&mut sample);
        sample = rewritten;
    }

    let mut all_declared = HashSet::new();
    check_unique_set(
        &input_declared_names,
        "input",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &output_declared_names,
        "output",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "param",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &typed_buffers
            .iter()
            .map(|b| b.name.clone())
            .collect::<Vec<_>>(),
        "buffer",
        &mut all_declared,
        &mut errors,
    );

    let mut struct_defs = HashMap::new();
    let mut typed_structs = Vec::new();
    let mut method_self_struct = HashMap::<String, String>::new();
    let mut callable_symbols_for_method_sugar =
        defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
    for s in &struct_defs_raw {
        for method in &s.methods {
            callable_symbols_for_method_sugar.insert(format!("{}.{}", s.name, method.name));
        }
    }
    for s in &struct_defs_raw {
        if is_builtin_constant_name(&s.name) {
            errors.push(Diagnostic::semantic(
                format!("struct name '{}' is reserved as a builtin constant", s.name),
                0,
                0,
            ));
            continue;
        }
        if all_declared.contains(&s.name) {
            errors.push(Diagnostic::semantic(
                format!("struct name '{}' conflicts with existing symbol", s.name),
                0,
                0,
            ));
            continue;
        }
        if struct_defs.contains_key(&s.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate struct '{}'", s.name),
                0,
                0,
            ));
            continue;
        }
        let typed_fields = coerce_struct_fields(&s.name, &s.fields, options, &mut errors);
        struct_defs.insert(s.name.clone(), typed_fields.clone());
        typed_structs.push(TypedStruct {
            name: s.name.clone(),
            fields: typed_fields,
        });
        all_declared.insert(s.name.clone());

        let mut local_method_names = HashSet::new();
        for method in &s.methods {
            if !local_method_names.insert(method.name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!("duplicate method '{}.{}'", s.name, method.name),
                    0,
                    0,
                ));
                continue;
            }
            if method.params.first().map(|p| p.name.as_str()) != Some("self") {
                errors.push(Diagnostic::semantic(
                    format!(
                        "method '{}.{}' must declare 'self' as first parameter",
                        s.name, method.name
                    ),
                    0,
                    0,
                ));
            }
            let fq_name = format!("{}.{}", s.name, method.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                method_self_struct.insert(fq_name.clone(), s.name.clone());
            }
            let mut desugared_method_body = method.body.clone();
            let mut method_struct_instances = HashMap::<String, String>::new();
            let method_ns = namespace_of_symbol(&s.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                method_struct_instances.insert("self".to_owned(), s.name.clone());
            }
            for stmt in &mut desugared_method_body {
                desugar_init_instance_method_calls(
                    stmt,
                    &mut method_struct_instances,
                    &struct_defs,
                    &method_ns,
                    &callable_symbols_for_method_sugar,
                );
            }
            defs.push(FunctionDef {
                type_params: Vec::new(),
                name: fq_name,
                params: method.params.clone(),
                body: desugared_method_body,
            });
        }
    }

    for (struct_name, fields) in &struct_defs {
        for field in fields {
            if let Some(elem_struct) = &field.array_elem_struct {
                let context = format!("field '{}.{}' array element", struct_name, field.name);
                let _ =
                    validate_data_struct_layout(elem_struct, &struct_defs, &context, &mut errors);
            }
        }
    }

    let mut desugar_struct_instances = HashMap::<String, String>::new();
    for stmt in &mut init {
        desugar_init_instance_method_calls(
            stmt,
            &mut desugar_struct_instances,
            &struct_defs,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_pre {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_post {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut sample {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for event in &mut events {
        for stmt in &mut event.body {
            desugar_sample_instance_method_calls(
                stmt,
                &desugar_struct_instances,
                "",
                &callable_symbols_for_method_sugar,
            );
        }
    }
    for def in &mut defs {
        if method_self_struct.contains_key(&def.name) {
            continue;
        }
        let mut def_struct_instances = HashMap::<String, String>::new();
        for param in &def.params {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if struct_defs.contains_key(struct_name) {
                    def_struct_instances.insert(param.name.clone(), struct_name.clone());
                }
            }
        }
        let def_ns = namespace_of_symbol(&def.name);
        for stmt in &mut def.body {
            desugar_sample_instance_method_calls(
                stmt,
                &def_struct_instances,
                &def_ns,
                &callable_symbols_for_method_sugar,
            );
        }
    }

    let (overload_candidates, def_public_name_by_internal) =
        prepare_function_overloads(&mut defs, &method_self_struct);

    let mut top_level_env = OverloadRewriteEnv::default();
    top_level_env
        .scalar_types
        .extend(in_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(out_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(param_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env.array_elem_types.extend(
        in_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        out_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        param_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.buffer_types.extend(
        typed_buffers
            .iter()
            .map(|b| (b.name.clone(), (b.elem_ty, b.channels.clone()))),
    );
    top_level_env
        .struct_instances
        .extend(desugar_struct_instances.clone());

    let mut init_rewrite_env = top_level_env.clone();
    rewrite_overloaded_calls_in_stmt_list(
        &mut init,
        &mut init_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut block_pre_rewrite_env = top_level_env.clone();
    rewrite_overloaded_calls_in_stmt_list(
        &mut block_pre,
        &mut block_pre_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut sample_rewrite_env = top_level_env.clone();
    rewrite_overloaded_calls_in_stmt_list(
        &mut sample,
        &mut sample_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut block_post_rewrite_env = top_level_env.clone();
    rewrite_overloaded_calls_in_stmt_list(
        &mut block_post,
        &mut block_post_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    for event in &mut events {
        let mut event_env = top_level_env.clone();
        rewrite_overloaded_calls_in_stmt_list(
            &mut event.body,
            &mut event_env,
            &overload_candidates,
            &struct_defs,
            &mut errors,
        );
    }
    for def in &mut defs {
        let mut def_env = top_level_env.clone();
        for param in &mut def.params {
            match &param.ty {
                Some(FnParamType::Primitive(prim)) => {
                    def_env.scalar_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::Struct(struct_name)) => {
                    def_env
                        .struct_instances
                        .insert(param.name.clone(), struct_name.clone());
                }
                Some(FnParamType::Buffer(buffer_ty)) => {
                    let channels = match &buffer_ty.channels {
                        BufferChannels::Mono => TypedBufferChannels::Mono,
                        BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                        BufferChannels::Static(expr) => const_positive_usize_for_overload(expr)
                            .map(TypedBufferChannels::Static)
                            .unwrap_or(TypedBufferChannels::Dynamic),
                    };
                    let elem_ty = match &buffer_ty.elem {
                        BufferElemType::Primitive(ty) => *ty,
                        BufferElemType::Generic(_) => PrimitiveType::F32,
                    };
                    def_env
                        .buffer_types
                        .insert(param.name.clone(), (elem_ty, channels));
                }
                Some(FnParamType::Array(Some(prim))) => {
                    def_env.array_elem_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::Array(None)) | Some(FnParamType::BareBuffer) | None => {}
            }
            if let Some(default_expr) = &mut param.default {
                rewrite_overloaded_calls_in_expr(
                    default_expr,
                    &def_env,
                    &overload_candidates,
                    &mut errors,
                );
            }
        }
        rewrite_overloaded_calls_in_stmt_list(
            &mut def.body,
            &mut def_env,
            &overload_candidates,
            &struct_defs,
            &mut errors,
        );
    }

    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    let mut seen_public_function_symbols = HashSet::<String>::new();
    for def in &defs {
        let public_name = def_public_name_by_internal
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| def.name.clone());
        if is_builtin_constant_name(&public_name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "function name '{}' is reserved as a builtin constant",
                    public_name
                ),
                0,
                0,
            ));
            continue;
        }
        if is_builtin_function_name(&public_name) {
            errors.push(Diagnostic::semantic(
                format!("cannot redefine builtin function '{}'", public_name),
                0,
                0,
            ));
            continue;
        }
        if struct_defs.contains_key(&public_name) {
            errors.push(Diagnostic::semantic(
                format!("function name '{}' conflicts with struct name", public_name),
                0,
                0,
            ));
            continue;
        }
        if all_declared.contains(&public_name)
            && !seen_public_function_symbols.contains(&public_name)
        {
            errors.push(Diagnostic::semantic(
                format!(
                    "function name '{}' conflicts with existing symbol",
                    public_name
                ),
                0,
                0,
            ));
            continue;
        }
        if fn_signatures.contains_key(&def.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate function '{}'", def.name),
                0,
                0,
            ));
            continue;
        }
        fn_signatures.insert(
            def.name.clone(),
            FnSignature {
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                type_params: def.type_params.clone(),
            },
        );
        if seen_public_function_symbols.insert(public_name.clone()) {
            all_declared.insert(public_name.clone());
        }

        if !def.type_params.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "function '{}' does not support generic type parameters; use typed/untyped parameters and call-site monomorphization",
                    public_name
                ),
                0,
                0,
            ));
        }
        let mut local_params = HashSet::new();
        for p in &def.params {
            if is_builtin_constant_name(&p.name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "function parameter '{}' in '{}' is reserved as a builtin constant",
                        p.name, def.name
                    ),
                    0,
                    0,
                ));
            }
            if !local_params.insert(p.name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "duplicate function parameter '{}' in '{}'",
                        p.name, public_name
                    ),
                    0,
                    0,
                ));
            }
            if let Some(default) = &p.default {
                if matches!(
                    p.ty,
                    Some(FnParamType::Buffer(_))
                        | Some(FnParamType::Array(_))
                        | Some(FnParamType::BareBuffer)
                ) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function parameter '{}.{}' is a buffer and cannot have a default value",
                            public_name, p.name
                        ),
                        0,
                        0,
                    ));
                }
                validate_default_expr(
                    default,
                    &mut errors,
                    &format!("function parameter '{}.{}'", public_name, p.name),
                );
            }
        }
    }

    // --- Def monomorphization pass ---
    // Identify defs whose parameters require monomorphization (generic struct,
    // untyped array `[]`, or bare `buffer`).
    {
        let mono_eligible: HashSet<String> = fn_signatures
            .iter()
            .filter_map(|(name, sig)| {
                let needs_mono = sig.param_types.iter().any(|pt| match pt {
                    Some(FnParamType::Struct(s)) if generic_struct_template_names.contains(s) => {
                        true
                    }
                    Some(FnParamType::Array(None)) | Some(FnParamType::BareBuffer) => true,
                    _ => false,
                });
                if needs_mono {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        if !mono_eligible.is_empty() {
            let mut generated_defs = Vec::<FunctionDef>::new();
            let mut generated_sigs = HashMap::<String, FnSignature>::new();
            let mut mono_cache = HashMap::<(String, Vec<MonoParamKey>), String>::new();

            // Rewrite calls in-place across all scopes.
            monomorphize_calls_in_stmts(
                &mut init,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
            );
            monomorphize_calls_in_stmts(
                &mut block_pre,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
            );
            monomorphize_calls_in_stmts(
                &mut sample,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
            );
            monomorphize_calls_in_stmts(
                &mut block_post,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
            );
            for event in &mut events {
                monomorphize_calls_in_stmts(
                    &mut event.body,
                    &top_level_env,
                    &mono_eligible,
                    &fn_signatures,
                    &defs,
                    &generic_struct_template_names,
                    &struct_defs,
                    &mut generated_defs,
                    &mut generated_sigs,
                    &mut mono_cache,
                );
            }
            // Also walk def bodies (def-to-def mono calls)
            for def in &mut defs {
                let mut def_env = top_level_env.clone();
                for param in &def.params {
                    match &param.ty {
                        Some(FnParamType::Primitive(prim)) => {
                            def_env.scalar_types.insert(param.name.clone(), *prim);
                        }
                        Some(FnParamType::Struct(struct_name)) => {
                            def_env
                                .struct_instances
                                .insert(param.name.clone(), struct_name.clone());
                        }
                        Some(FnParamType::Array(Some(prim))) => {
                            def_env.array_elem_types.insert(param.name.clone(), *prim);
                        }
                        Some(FnParamType::Buffer(buffer_ty)) => {
                            let channels = match &buffer_ty.channels {
                                BufferChannels::Mono => TypedBufferChannels::Mono,
                                BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                                BufferChannels::Static(expr) => {
                                    const_positive_usize_for_overload(expr)
                                        .map(TypedBufferChannels::Static)
                                        .unwrap_or(TypedBufferChannels::Dynamic)
                                }
                            };
                            let elem_ty = match &buffer_ty.elem {
                                BufferElemType::Primitive(ty) => *ty,
                                BufferElemType::Generic(_) => PrimitiveType::F32,
                            };
                            def_env
                                .buffer_types
                                .insert(param.name.clone(), (elem_ty, channels));
                        }
                        _ => {}
                    }
                }
                monomorphize_calls_in_stmts(
                    &mut def.body,
                    &def_env,
                    &mono_eligible,
                    &fn_signatures,
                    &[],
                    &generic_struct_template_names,
                    &struct_defs,
                    &mut generated_defs,
                    &mut generated_sigs,
                    &mut mono_cache,
                );
            }

            // Register generated defs and signatures
            for sig in generated_sigs {
                fn_signatures.insert(sig.0, sig.1);
            }
            defs.extend(generated_defs);

            // Remove original mono-eligible defs — only specialized copies should
            // be processed by inference.  Also remove their fn_signatures.
            for name in &mono_eligible {
                fn_signatures.remove(name);
            }
            defs.retain(|d| !mono_eligible.contains(&d.name));
        }
    }

    let input_names: HashSet<String> = ins.iter().cloned().collect();
    let output_names: HashSet<String> = outs.iter().cloned().collect();
    let param_names: HashSet<String> = typed_params.iter().map(|p| p.name.clone()).collect();
    let def_return_types = infer_def_return_types(&defs, &fn_signatures, &struct_defs);
    fn_signatures
        .entry(PROC_INDEX_CALL_SENTINEL.to_owned())
        .or_insert_with(|| internal_proc_index_call_signature(false));
    fn_signatures
        .entry(format!(
            "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
        ))
        .or_insert_with(|| internal_proc_index_call_signature(true));

    let mut state_scalars = HashMap::<String, PrimitiveType>::new();
    set_declared_symbol_types(
        &mut state_scalars,
        &input_names,
        &in_types,
        DECLARED_INPUT_TYPE_PREFIX,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &output_names,
        &out_types,
        DECLARED_OUTPUT_TYPE_PREFIX,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &param_names,
        &param_types,
        DECLARED_PARAM_TYPE_PREFIX,
    );
    for (fn_name, ret_ty) in &def_return_types {
        state_scalars.insert(
            declared_type_key(DECLARED_FUNCTION_RETURN_TYPE_PREFIX, fn_name),
            *ret_ty,
        );
    }
    for buffer in &typed_buffers {
        state_scalars.insert(
            declared_type_key(DECLARED_BUFFER_ELEM_TYPE_PREFIX, &buffer.name),
            buffer.elem_ty,
        );
        state_scalars.insert(
            declared_type_key(buffer_elem_decl_prefix(buffer.elem_ty), &buffer.name),
            PrimitiveType::Bool,
        );
        let is_multi = match buffer.channels {
            TypedBufferChannels::Mono => false,
            TypedBufferChannels::Static(ch) => ch > 1,
            TypedBufferChannels::Dynamic => true,
        };
        match buffer.channels {
            TypedBufferChannels::Dynamic => {
                state_scalars.insert(
                    declared_type_key(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX, &buffer.name),
                    PrimitiveType::Bool,
                );
            }
            TypedBufferChannels::Static(ch) if ch > 1 => {
                state_scalars.insert(
                    declared_buffer_static_channels_key(&buffer.name, ch),
                    PrimitiveType::Bool,
                );
            }
            _ => {}
        }
        if is_multi {
            state_scalars.insert(
                declared_type_key(DECLARED_BUFFER_MULTICHANNEL_PREFIX, &buffer.name),
                PrimitiveType::Bool,
            );
        }
    }
    let state_arrays = HashMap::new();
    let state_array_struct_roots = HashMap::<String, ArrayStructRootInfo>::new();
    let struct_instances = HashMap::new();
    let mut init_known_scalars = param_names.clone();
    init_known_scalars.extend(state_scalars.keys().cloned());
    let init_locals = HashSet::new();
    let init_local_aliases = LocalAliasTypes::new();
    let mut init_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &out_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &param_arrays, false);

    let init_default_ty =
        resolve_init_default_ty(init_default_decl_ty.as_ref(), "top-level", &mut errors);

    let init_ctx = InitAnalysisCtx {
        context_label: "top-level",
        init_default_ty,
        input_names: &input_names,
        output_names: &output_names,
        param_names: &param_names,
        struct_defs: &struct_defs,
        fn_signatures: &fn_signatures,
        options,
        proc_resolution: None,
    };
    let mut init_st = InitAnalysisState {
        known_scalars: init_known_scalars,
        local_aliases: init_local_aliases,
        local_array_aliases: init_local_data_aliases,
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        state_array_specs: HashMap::new(),
        struct_instance_type_args: HashMap::new(),
        nested_procs: HashMap::new(),
        nested_proc_arrays: HashMap::new(),
    };
    for stmt in &init {
        analyze_init_stmt(stmt, &init_ctx, &mut init_st, &init_locals, 0, &mut errors);
    }
    let InitAnalysisState {
        known_scalars: _init_known_scalars,
        local_aliases: _init_local_aliases,
        local_array_aliases: _init_local_data_aliases,
        mut state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        ..
    } = init_st;
    let init_writable_roots = collect_runtime_state_roots(&state_scalars);

    let block_known_scalars = build_known_scalars_from_state(&param_names, &state_scalars);
    let block_locals = HashSet::new();
    let mut block_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut block_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &param_arrays, false);
    let empty_inputs = HashSet::new();
    let empty_outputs = HashSet::new();
    let block_forbidden_assigns = output_names.clone();
    let block_registration_scope = StateRegistrationScope::Block {
        struct_defs: &struct_defs,
    };
    register_and_analyze_runtime_scope(
        block_pre.iter().chain(block_post.iter()),
        &mut state_scalars,
        &state_arrays,
        &state_array_struct_roots,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
        &block_registration_scope,
        &block_locals,
        block_known_scalars,
        LocalAliasTypes::new(),
        block_local_data_aliases,
        &empty_inputs,
        &empty_outputs,
        &block_forbidden_assigns,
        &param_names,
        &struct_defs,
        &fn_signatures,
        options,
        &mut errors,
    );

    let mut sample_base = param_names.clone();
    sample_base.extend(input_names.iter().cloned());
    let sample_known_scalars = build_known_scalars_from_state(&sample_base, &state_scalars);
    let sample_locals = HashSet::new();
    let mut sample_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &param_arrays, false);
    let sample_forbidden_assigns = HashSet::new();
    let sample_registration_scope = StateRegistrationScope::Sample;
    register_and_analyze_runtime_scope(
        sample.iter(),
        &mut state_scalars,
        &state_arrays,
        &state_array_struct_roots,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
        &sample_registration_scope,
        &sample_locals,
        sample_known_scalars,
        LocalAliasTypes::new(),
        sample_local_data_aliases,
        &input_names,
        &output_names,
        &sample_forbidden_assigns,
        &param_names,
        &struct_defs,
        &fn_signatures,
        options,
        &mut errors,
    );

    let typed_events = coerce_typed_events(&events, options, &mut errors);
    let final_state_roots = collect_runtime_state_roots(&state_scalars);
    let immutable_event_roots = final_state_roots
        .difference(&init_writable_roots)
        .cloned()
        .collect::<HashSet<_>>();
    let event_known_scalars_seed = build_known_scalars_from_state(&param_names, &state_scalars);
    let mut event_array_alias_seed = HashMap::new();
    seed_top_level_array_aliases(&mut event_array_alias_seed, &param_arrays, false);
    analyze_runtime_events(
        &typed_events,
        &event_known_scalars_seed,
        &event_array_alias_seed,
        &param_names,
        &init_writable_roots,
        &immutable_event_roots,
        &input_names,
        &output_names,
        &state_scalars,
        &state_arrays,
        &state_array_struct_roots,
        &struct_instances,
        &struct_defs,
        &fn_signatures,
        options,
        &mut errors,
    );

    let mut block_exec = block_pre.clone();
    block_exec.extend(block_post.clone());
    let mut sample_and_event_exec = sample.clone();
    for event in &typed_events {
        sample_and_event_exec.extend(event.body.clone());
    }

    let mut inferred_array_bindings = HashMap::<String, InferredArrayParam>::new();
    for (name, info) in &out_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, info) in &param_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, len) in &state_arrays {
        let elem_ty =
            get_declared_symbol_type(&state_scalars, name, DECLARED_DATA_ELEM_TYPE_PREFIX)
                .unwrap_or(PrimitiveType::F32);
        inferred_array_bindings.insert(name.clone(), InferredArrayParam { elem_ty, len: *len });
    }

    let (inferred_def_params, synthesized_struct_defs) = infer_def_param_kinds(
        &defs,
        &init,
        &block_exec,
        &sample_and_event_exec,
        &struct_instances,
        &inferred_array_bindings,
        &typed_buffers
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    vec![InferredBufferParam {
                        elem_ty: b.elem_ty,
                        channels: b.channels.clone(),
                    }],
                )
            })
            .collect::<HashMap<_, _>>(),
        &fn_signatures,
        &method_self_struct,
        &struct_defs,
        options,
        &mut errors,
    );

    let mut def_struct_defs = struct_defs.clone();
    for (name, fields) in &synthesized_struct_defs {
        def_struct_defs.insert(name.clone(), fields.clone());
    }

    let def_global_inputs = HashSet::<String>::new();
    let def_global_outputs = HashSet::<String>::new();
    let def_global_params = HashSet::<String>::new();
    for def in &defs {
        let mut fn_known = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_state_scalars = state_scalars
            .iter()
            .filter(|(name, _)| !is_runtime_state_symbol_name(name))
            .map(|(name, ty)| (name.clone(), *ty))
            .collect::<HashMap<_, _>>();
        let fn_sig = fn_signatures.get(&def.name);
        // Def parameters are function-local and should be visible for local
        // type inference even though top-level runtime symbols are not.
        for (idx, param) in def.params.iter().enumerate() {
            let explicit_prim = fn_sig
                .and_then(|sig| sig.param_types.get(idx))
                .and_then(|ty| ty.as_ref())
                .and_then(|ty| match ty {
                    FnParamType::Primitive(prim) => Some(*prim),
                    FnParamType::Struct(_)
                    | FnParamType::Buffer(_)
                    | FnParamType::Array(_)
                    | FnParamType::BareBuffer => None,
                });
            if let Some(param_ty) = explicit_prim {
                def_state_scalars.insert(param.name.clone(), param_ty);
            } else {
                def_state_scalars.remove(&param.name);
            }
        }
        let fn_locals = HashSet::new();
        let mut fn_local_aliases = LocalAliasTypes::new();
        let mut fn_local_data_aliases = HashMap::new();
        let param_names_vec = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        let param_structs = inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_buffers = inferred_def_params
            .get(&def.name)
            .map(|k| param_buffer_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_arrays = inferred_def_params
            .get(&def.name)
            .map(|k| param_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        for (param_name, elem_ty) in &param_arrays {
            fn_local_data_aliases.insert(
                param_name.clone(),
                LocalArrayAliasInfo {
                    len: 1,
                    elem_ty: *elem_ty,
                    elem_struct: None,
                    writable: true,
                },
            );
        }
        for (param_name, (elem_ty, channels)) in &param_buffers {
            let elem_key = declared_type_key(DECLARED_BUFFER_ELEM_TYPE_PREFIX, param_name);
            let typed_key = declared_type_key(buffer_elem_decl_prefix(*elem_ty), param_name);
            def_state_scalars.insert(elem_key.clone(), *elem_ty);
            def_state_scalars.insert(typed_key.clone(), PrimitiveType::Bool);
            fn_known.insert(elem_key);
            fn_known.insert(typed_key);
            match channels {
                TypedBufferChannels::Mono => {}
                TypedBufferChannels::Static(ch) => {
                    if *ch > 1 {
                        let key =
                            declared_type_key(DECLARED_BUFFER_MULTICHANNEL_PREFIX, param_name);
                        let st_key = declared_buffer_static_channels_key(param_name, *ch);
                        def_state_scalars.insert(key.clone(), PrimitiveType::Bool);
                        def_state_scalars.insert(st_key.clone(), PrimitiveType::Bool);
                        fn_known.insert(key);
                        fn_known.insert(st_key);
                    }
                }
                TypedBufferChannels::Dynamic => {
                    let key = declared_type_key(DECLARED_BUFFER_MULTICHANNEL_PREFIX, param_name);
                    let dyn_key =
                        declared_type_key(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX, param_name);
                    def_state_scalars.insert(key.clone(), PrimitiveType::Bool);
                    def_state_scalars.insert(dyn_key.clone(), PrimitiveType::Bool);
                    fn_known.insert(key);
                    fn_known.insert(dyn_key);
                }
            }
        }
        for stmt in &def.body {
            analyze_def_stmt(
                stmt,
                &mut fn_known,
                &mut fn_local_aliases,
                &mut fn_local_data_aliases,
                &fn_locals,
                &param_structs,
                &def_state_scalars,
                &def_global_inputs,
                &def_global_outputs,
                &def_global_params,
                &def_struct_defs,
                &fn_signatures,
                options,
                0,
                &mut errors,
            );
        }
    }

    if errors.is_empty() {
        let mut sorted_state = state_scalars
            .keys()
            .filter(|name| {
                !name.starts_with(DECLARED_INPUT_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_OUTPUT_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_PARAM_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_DATA_ELEM_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_STRUCT_FIELD_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_INVALID_PLACEHOLDER_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_MULTICHANNEL_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_STATIC_CHANNELS_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_F32_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_F64_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_I32_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_I64_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_BOOL_PREFIX)
                    && !name.starts_with(DECLARED_FUNCTION_RETURN_TYPE_PREFIX)
            })
            .cloned()
            .collect::<Vec<_>>();
        sorted_state.sort();
        let state_types = sorted_state
            .iter()
            .map(|name| {
                state_scalars
                    .get(name)
                    .copied()
                    .unwrap_or(PrimitiveType::F32)
            })
            .collect::<Vec<_>>();

        let mut typed_data = state_arrays
            .into_iter()
            .map(|(name, len)| {
                let elem_ty =
                    get_declared_symbol_type(&state_scalars, &name, DECLARED_DATA_ELEM_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32);
                TypedArrayVar { name, len, elem_ty }
            })
            .collect::<Vec<_>>();
        typed_data.sort_by(|a, b| a.name.cmp(&b.name));
        let mut typed_data_roots = state_array_struct_roots
            .into_iter()
            .map(|(name, info)| TypedArrayStructRoot {
                name,
                struct_name: info.struct_name,
                len: info.len,
            })
            .collect::<Vec<_>>();
        typed_data_roots.sort_by(|a, b| a.name.cmp(&b.name));

        let mut synth_names = synthesized_struct_defs.keys().cloned().collect::<Vec<_>>();
        synth_names.sort();
        for name in synth_names {
            if let Some(fields) = synthesized_struct_defs.get(&name) {
                typed_structs.push(TypedStruct {
                    name,
                    fields: fields.clone(),
                });
            }
        }

        let typed_defs = defs
            .into_iter()
            .map(|d| {
                let param_kinds = inferred_def_params
                    .get(&d.name)
                    .cloned()
                    .unwrap_or_else(|| vec![TypedFnParam::Scalar; d.params.len()]);
                TypedFunction {
                    method_of: method_self_struct.get(&d.name).cloned(),
                    type_params: d.type_params.clone(),
                    param_defaults: d.params.iter().map(|p| p.default.clone()).collect(),
                    param_kinds,
                    return_ty: def_return_types
                        .get(&d.name)
                        .copied()
                        .unwrap_or(PrimitiveType::F32),
                    name: d.name,
                    params: d.params.into_iter().map(|p| p.name).collect(),
                    body: d.body,
                }
            })
            .collect::<Vec<_>>();

        Ok(TypedProgram {
            ins,
            outs,
            in_types,
            out_types,
            param_types,
            in_defaults,
            in_ranges,
            in_arrays,
            out_arrays,
            param_arrays,
            params: typed_params,
            buffers: typed_buffers,
            structs: typed_structs,
            defs: typed_defs,
            events: typed_events,
            def_sample_oversample_factors,
            proc_step_oversample_meta,
            init,
            block_pre,
            sample_oversample_factor,
            sample,
            block_post,
            state_vars: sorted_state,
            state_types,
            array_vars: typed_data,
            array_struct_roots: typed_data_roots,
        })
    } else {
        Err(errors)
    }
}
