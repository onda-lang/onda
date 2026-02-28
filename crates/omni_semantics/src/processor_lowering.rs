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
            defs.push(FunctionDef {
                type_params: Vec::new(),
                name: fq_name,
                params: method.params.clone(),
                body: method.body.clone(),
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
        desugar_init_instance_method_calls(stmt, &mut desugar_struct_instances, &struct_defs);
    }
    for stmt in &mut block_pre {
        desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
    }
    for stmt in &mut block_post {
        desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
    }
    for stmt in &mut sample {
        desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
    }
    for event in &mut events {
        for stmt in &mut event.body {
            desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
        }
    }
    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    for def in &defs {
        if is_builtin_constant_name(&def.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "function name '{}' is reserved as a builtin constant",
                    def.name
                ),
                0,
                0,
            ));
            continue;
        }
        if is_builtin_function_name(&def.name) {
            errors.push(Diagnostic::semantic(
                format!("cannot redefine builtin function '{}'", def.name),
                0,
                0,
            ));
            continue;
        }
        if struct_defs.contains_key(&def.name) {
            errors.push(Diagnostic::semantic(
                format!("function name '{}' conflicts with struct name", def.name),
                0,
                0,
            ));
            continue;
        }
        if all_declared.contains(&def.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "function name '{}' conflicts with existing symbol",
                    def.name
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
        all_declared.insert(def.name.clone());

        if !def.type_params.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "function '{}' does not support generic type parameters; use typed/untyped parameters and call-site monomorphization",
                    def.name
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
                        p.name, def.name
                    ),
                    0,
                    0,
                ));
            }
            if let Some(default) = &p.default {
                if matches!(p.ty, Some(FnParamType::Buffer(_))) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function parameter '{}.{}' is a buffer and cannot have a default value",
                            def.name, p.name
                        ),
                        0,
                        0,
                    ));
                }
                validate_default_expr(
                    default,
                    &mut errors,
                    &format!("function parameter '{}.{}'", def.name, p.name),
                );
            }
        }
    }

    let input_names: HashSet<String> = ins.iter().cloned().collect();
    let output_names: HashSet<String> = outs.iter().cloned().collect();
    let param_names: HashSet<String> = typed_params.iter().map(|p| p.name.clone()).collect();
    let def_return_types = infer_def_return_types(&defs, &fn_signatures, &struct_defs);

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
    let mut state_arrays = HashMap::new();
    let mut state_array_struct_roots = HashMap::<String, ArrayStructRootInfo>::new();
    let mut struct_instances = HashMap::new();
    let mut init_known_scalars = param_names.clone();
    init_known_scalars.extend(state_scalars.keys().cloned());
    let init_locals = HashSet::new();
    let mut init_local_aliases = LocalAliasTypes::new();
    let mut init_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &out_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &param_arrays, false);

    let init_default_ty = match init_default_decl_ty {
        Some(DeclType::Scalar(prim)) => Some(prim),
        Some(DeclType::Generic(param)) => {
            errors.push(Diagnostic::semantic(
                format!(
                    "top-level init section default type '[{param}]' is invalid; only primitive scalar types are allowed"
                ),
                0,
                0,
            ));
            None
        }
        Some(DeclType::Array { .. }) | Some(DeclType::ArrayGeneric { .. }) => {
            errors.push(Diagnostic::semantic(
                "top-level init section default type must be a scalar primitive type",
                0,
                0,
            ));
            None
        }
        None => None,
    };

    for stmt in &init {
        analyze_init_stmt(
            stmt,
            init_default_ty,
            &mut init_known_scalars,
            &mut init_local_aliases,
            &mut init_local_data_aliases,
            &init_locals,
            &mut state_scalars,
            &mut state_arrays,
            &mut state_array_struct_roots,
            &mut struct_instances,
            &input_names,
            &output_names,
            &param_names,
            &struct_defs,
            &fn_signatures,
            options,
            0,
            &mut errors,
        );
    }
    let init_writable_roots = collect_runtime_state_roots(&state_scalars);

    register_block_assigned_scalars_as_state(
        block_pre.iter().chain(block_post.iter()),
        &mut state_scalars,
        &state_arrays,
        &state_array_struct_roots,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
        &struct_defs,
        &fn_signatures,
    );

    let mut block_known_scalars = param_names.clone();
    block_known_scalars.extend(state_scalars.keys().cloned());
    let block_locals = HashSet::new();
    let mut block_local_aliases = LocalAliasTypes::new();
    let mut block_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut block_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &param_arrays, false);
    let empty_inputs = HashSet::new();
    let empty_outputs = HashSet::new();
    let block_forbidden_assigns = output_names.clone();

    for stmt in block_pre.iter().chain(block_post.iter()) {
        analyze_sample_stmt(
            stmt,
            &mut block_known_scalars,
            &mut block_local_aliases,
            &mut block_local_data_aliases,
            &block_locals,
            &state_scalars,
            &state_arrays,
            &state_array_struct_roots,
            &struct_instances,
            &empty_inputs,
            &empty_outputs,
            &block_forbidden_assigns,
            &param_names,
            &struct_defs,
            &fn_signatures,
            options,
            0,
            &mut errors,
        );
    }

    register_sample_typed_scalar_decls_as_state(
        sample.iter(),
        &mut state_scalars,
        &state_arrays,
        &state_array_struct_roots,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
    );

    let mut sample_known_scalars = param_names.clone();
    sample_known_scalars.extend(input_names.clone());
    sample_known_scalars.extend(state_scalars.keys().cloned());
    let sample_locals = HashSet::new();
    let mut sample_local_aliases = LocalAliasTypes::new();
    let mut sample_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &param_arrays, false);
    let sample_forbidden_assigns = HashSet::new();

    for stmt in &sample {
        analyze_sample_stmt(
            stmt,
            &mut sample_known_scalars,
            &mut sample_local_aliases,
            &mut sample_local_data_aliases,
            &sample_locals,
            &state_scalars,
            &state_arrays,
            &state_array_struct_roots,
            &struct_instances,
            &input_names,
            &output_names,
            &sample_forbidden_assigns,
            &param_names,
            &struct_defs,
            &fn_signatures,
            options,
            0,
            &mut errors,
        );
    }

    let typed_events = coerce_typed_events(&events, options, &mut errors);
    let final_state_roots = collect_runtime_state_roots(&state_scalars);
    let immutable_event_roots = final_state_roots
        .difference(&init_writable_roots)
        .cloned()
        .collect::<HashSet<_>>();
    let empty_event_inputs = HashSet::<String>::new();
    let empty_event_outputs = HashSet::<String>::new();
    for event in &typed_events {
        let mut event_locals = HashSet::<String>::new();
        let mut scalar_event_params = HashSet::<String>::new();
        let mut array_event_params = HashSet::<String>::new();
        let mut event_known_scalars = param_names.clone();
        event_known_scalars.extend(state_scalars.keys().cloned());
        let mut event_local_aliases = LocalAliasTypes::new();
        let mut event_local_data_aliases = HashMap::new();
        seed_top_level_array_aliases(&mut event_local_data_aliases, &param_arrays, false);
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

        let mut event_param_immutable = param_names.clone();
        event_param_immutable.extend(scalar_event_params.iter().cloned());
        let event_loop_vars = HashSet::<String>::new();

        for stmt in &event.body {
            validate_event_stmt_restrictions(
                stmt,
                &mut event_locals,
                &init_writable_roots,
                &immutable_event_roots,
                &input_names,
                &output_names,
                &scalar_event_params,
                &array_event_params,
                &mut errors,
            );
            analyze_sample_stmt(
                stmt,
                &mut event_known_scalars,
                &mut event_local_aliases,
                &mut event_local_data_aliases,
                &event_loop_vars,
                &state_scalars,
                &state_arrays,
                &state_array_struct_roots,
                &struct_instances,
                &empty_event_inputs,
                &empty_event_outputs,
                &output_names,
                &event_param_immutable,
                &struct_defs,
                &fn_signatures,
                options,
                0,
                &mut errors,
            );
        }
    }

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

    for def in &defs {
        let mut fn_known = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_state_scalars = state_scalars.clone();
        let fn_sig = fn_signatures.get(&def.name);
        // Def parameters are function-local and must shadow global symbols.
        for (idx, param) in def.params.iter().enumerate() {
            let explicit_prim = fn_sig
                .and_then(|sig| sig.param_types.get(idx))
                .and_then(|ty| ty.as_ref())
                .and_then(|ty| match ty {
                    FnParamType::Primitive(prim) => Some(*prim),
                    FnParamType::Struct(_) | FnParamType::Buffer(_) => None,
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
                &input_names,
                &output_names,
                &param_names,
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
