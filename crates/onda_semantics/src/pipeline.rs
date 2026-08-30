use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use onda_frontend::Span;

use crate::callable_validation::validate_owner_callable_bindings;
use crate::processor_lowering::{
    coerce_typed_delegates, coerce_typed_events, collect_runtime_state_roots, desugar_processors,
    guard_pinned_initializers, internal_proc_index_call_signature, lower_graph_blocks,
    nested_call_out_fn_name, nested_step_fn_name, prepare_processors_for_graph_inspection,
    proc_runtime_analysis_options, validated_sample_oversample_factor, ProcLoweringShape,
    ProcessorDesugarResult, TopLevelProcRewriteMeta, TOP_LEVEL_INIT_ALL_NAME,
};
use crate::*;

mod const_evaluation;
mod const_rewriting;
mod integer_ranges;
mod post_analysis;

use const_evaluation::*;
use const_rewriting::*;
pub(crate) use integer_ranges::*;
use post_analysis::*;
mod namespace_flattening;
use crate::task_lowering::validate_task_source_model;
use namespace_flattening::flatten_namespaces_for_semantics;

fn annotate_print_origins(program: &mut Program) {
    fn statements(body: &mut [Stmt], lexical_owner: &str, declaration: &str) {
        for statement in body {
            if let Stmt::Print { loc, origin, .. } = statement {
                *origin = Some(onda_frontend::PrintSourceOrigin {
                    source: *loc,
                    lexical_owner: lexical_owner.to_owned(),
                    declaration: declaration.to_owned(),
                });
            }
            match statement {
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    statements(then_branch, lexical_owner, declaration);
                    statements(else_branch, lexical_owner, declaration);
                }
                Stmt::For { body, .. } | Stmt::While { body, .. } => {
                    statements(body, lexical_owner, declaration);
                }
                _ => {}
            }
        }
    }

    fn when_declaration(when: &WhenDef) -> String {
        format!("when {}", when.target.delegate)
    }

    for block in &mut program.blocks {
        match block {
            Block::Init(init) => statements(&mut init.body, "program", "init"),
            Block::Block(exec) => {
                statements(&mut exec.pre, "program", "block");
                if let Some(sample) = &mut exec.sample {
                    statements(&mut sample.body, "program", "block");
                }
                statements(&mut exec.post, "program", "block");
            }
            Block::Sample(sample) => statements(&mut sample.body, "program", "sample"),
            Block::Def(def) => statements(&mut def.body, "program", &def.name),
            Block::Events(events) => {
                for event in &mut events.events {
                    statements(&mut event.body, "program", &event.name);
                }
            }
            Block::When(when) => {
                let declaration = when_declaration(when);
                statements(&mut when.body, "program", &declaration);
            }
            Block::Tasks(tasks) => {
                for task in &mut tasks.tasks {
                    statements(&mut task.body, "program", &task.name);
                }
            }
            Block::Struct(struct_def) => {
                let owner = struct_def.name.clone();
                for method in &mut struct_def.methods {
                    statements(&mut method.body, &owner, &method.name);
                }
            }
            Block::Proc(processor) => {
                let owner = processor.name.clone();
                statements(&mut processor.init.body, &owner, "init");
                statements(&mut processor.block_pre, &owner, "block");
                statements(&mut processor.sample, &owner, "sample");
                statements(&mut processor.block_post, &owner, "block");
                for event in &mut processor.events {
                    statements(&mut event.body, &owner, &event.name);
                }
                for when in &mut processor.whens {
                    let declaration = when_declaration(when);
                    statements(&mut when.body, &owner, &declaration);
                }
                for task in &mut processor.tasks {
                    statements(&mut task.body, &owner, &task.name);
                }
                for def in &mut processor.local_defs {
                    statements(&mut def.body, &owner, &def.name);
                }
            }
            _ => {}
        }
    }
}

fn validate_analysis_options(options: AnalysisOptions) -> Result<(), Vec<Diagnostic>> {
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
    Ok(())
}

fn resolves_to_processor_constructor(
    name: &str,
    current_ns: &str,
    proc_symbols: &HashSet<String>,
    proc_template_bases: &HashSet<String>,
) -> bool {
    if resolve_proc_ctor_symbol_name(name, current_ns, proc_symbols).is_some() {
        return true;
    }
    if name.contains("::") {
        proc_template_bases.contains(name)
    } else {
        resolve_unqualified_symbol_name(name, current_ns, proc_template_bases).is_some()
    }
}

fn validate_pinned_bindings(program: &Program, errors: &mut Vec<Diagnostic>) {
    fn add_occupied_names<'a>(
        occupied: &mut HashMap<String, &'static str>,
        names: impl IntoIterator<Item = &'a str>,
        kind: &'static str,
    ) {
        occupied.extend(names.into_iter().map(|name| (name.to_owned(), kind)));
    }

    fn validate_init(
        init: &InitBlock,
        occupied_names: &HashMap<String, &'static str>,
        current_ns: &str,
        proc_symbols: &HashSet<String>,
        proc_template_bases: &HashSet<String>,
        errors: &mut Vec<Diagnostic>,
    ) {
        let mut pending = init.pinned_roots.iter().collect::<HashSet<_>>();
        for stmt in &init.body {
            let Stmt::Assign {
                target: AssignTarget::Var(name),
                expr,
                ..
            } = stmt
            else {
                continue;
            };
            if !pending.remove(name) {
                continue;
            }
            if let Some(kind) = occupied_names.get(name) {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "'pin' requires a fresh state binding; '{name}' is already declared as {kind}"
                    ),
                    stmt.loc(),
                ));
                continue;
            }
            let pinned_processor_kind = match expr {
                Expr::UserCall {
                    name: constructor, ..
                } if resolves_to_processor_constructor(
                    constructor,
                    current_ns,
                    proc_symbols,
                    proc_template_bases,
                ) =>
                {
                    Some("processor instance")
                }
                Expr::ArrayCtor { spec, .. }
                    if matches!(
                        &spec.elem,
                        ArrayElemType::Struct(constructor)
                            if resolves_to_processor_constructor(
                                constructor,
                                current_ns,
                                proc_symbols,
                                proc_template_bases,
                            )
                    ) =>
                {
                    Some("processor array")
                }
                _ => None,
            };
            if let Some(kind) = pinned_processor_kind {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "'pin' cannot be applied to {kind} '{name}'; pin the processor's own init state instead"
                    ),
                    stmt.loc(),
                ));
            }
        }
    }

    let proc_symbols = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc_def) => Some(proc_def.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let proc_template_bases = specialized_proc_template_bases(&proc_symbols);
    let mut top_level_names = HashMap::new();
    for block in &program.blocks {
        match block {
            Block::Ins(decls) => {
                add_occupied_names(
                    &mut top_level_names,
                    decls.iter().map(|decl| decl.name.as_str()),
                    "an input",
                );
            }
            Block::Outs(decls) => {
                add_occupied_names(
                    &mut top_level_names,
                    decls.iter().map(|decl| decl.name.as_str()),
                    "an output",
                );
            }
            Block::KOuts(decls) => {
                add_occupied_names(
                    &mut top_level_names,
                    decls.iter().map(|decl| decl.name.as_str()),
                    "a control output",
                );
            }
            Block::Params(decls) => {
                add_occupied_names(
                    &mut top_level_names,
                    decls.iter().map(|decl| decl.name.as_str()),
                    "a param",
                );
            }
            Block::Buffers(decls) => {
                add_occupied_names(
                    &mut top_level_names,
                    decls.iter().map(|decl| decl.name.as_str()),
                    "a buffer",
                );
            }
            Block::Const(decl) => {
                top_level_names.insert(decl.name.clone(), "a constant");
            }
            _ => {}
        }
    }
    if let Some(Block::Init(init)) = program.block(BlockKind::Init) {
        validate_init(
            init,
            &top_level_names,
            "",
            &proc_symbols,
            &proc_template_bases,
            errors,
        );
    }
    for proc_def in program.blocks.iter().filter_map(|block| match block {
        Block::Proc(proc_def) => Some(proc_def),
        _ => None,
    }) {
        let mut occupied_names = HashMap::new();
        add_occupied_names(
            &mut occupied_names,
            proc_def.ins.iter().map(|decl| decl.name.as_str()),
            "an input",
        );
        add_occupied_names(
            &mut occupied_names,
            proc_def.outs.iter().map(|decl| decl.name.as_str()),
            "an output",
        );
        add_occupied_names(
            &mut occupied_names,
            proc_def.params.iter().map(|decl| decl.name.as_str()),
            "a param",
        );
        add_occupied_names(
            &mut occupied_names,
            proc_def.buffers.iter().map(|decl| decl.name.as_str()),
            "a buffer",
        );
        add_occupied_names(
            &mut occupied_names,
            proc_def.consts.iter().map(|decl| decl.name.as_str()),
            "a constant",
        );
        validate_init(
            &proc_def.init,
            &occupied_names,
            &namespace_of_symbol(&proc_def.name),
            &proc_symbols,
            &proc_template_bases,
            errors,
        );
    }
}

fn statements_return_value(statements: &[Stmt]) -> bool {
    statements.iter().any(|statement| match statement {
        Stmt::Return { expr, .. } => !is_bare_return_expr(expr),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => statements_return_value(then_branch) || statements_return_value(else_branch),
        Stmt::For { body, .. } | Stmt::While { body, .. } => statements_return_value(body),
        Stmt::Const { .. }
        | Stmt::Assign { .. }
        | Stmt::Expr { .. }
        | Stmt::Print { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => false,
    })
}

fn statements_publish_print(statements: &[Stmt]) -> bool {
    statements.iter().any(|statement| match statement {
        Stmt::Print { .. } => true,
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => statements_publish_print(then_branch) || statements_publish_print(else_branch),
        Stmt::For { body, .. } | Stmt::While { body, .. } => statements_publish_print(body),
        Stmt::Const { .. }
        | Stmt::Assign { .. }
        | Stmt::Expr { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => false,
    })
}

fn collect_typed_nested_proc_arrays(
    owner_struct: &str,
    proc_name: &str,
    physical_prefix: &str,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    stack: &mut HashSet<String>,
    output: &mut Vec<TypedNestedProcArray>,
) {
    if !stack.insert(proc_name.to_owned()) {
        return;
    }
    let Some(shape) = lowering_shapes.get(proc_name) else {
        stack.remove(proc_name);
        return;
    };
    let mut arrays = shape
        .nested_proc_array_slots
        .iter()
        .filter_map(|(field_name, slots)| {
            shape
                .state
                .nested_proc_arrays
                .get(field_name)
                .map(|nested| (field_name, slots, nested.proc_name.as_str()))
        })
        .collect::<Vec<_>>();
    arrays.sort_by(|lhs, rhs| lhs.0.cmp(rhs.0));
    for (field_name, slots, child_proc) in arrays {
        let physical_slots = slots
            .iter()
            .map(|slot| format!("{physical_prefix}{slot}"))
            .collect::<Vec<_>>();
        output.push(TypedNestedProcArray {
            owner_struct: owner_struct.to_owned(),
            field_name: format!("{physical_prefix}{field_name}"),
            proc_name: child_proc.to_owned(),
            slots: physical_slots.clone(),
        });
        for physical_slot in physical_slots {
            collect_typed_nested_proc_arrays(
                owner_struct,
                child_proc,
                &format!("{physical_slot}__"),
                lowering_shapes,
                stack,
                output,
            );
        }
    }
    let mut nested_instances = shape.state.nested_procs.iter().collect::<Vec<_>>();
    nested_instances.sort_by(|lhs, rhs| lhs.0.cmp(rhs.0));
    for (field_name, nested) in nested_instances {
        collect_typed_nested_proc_arrays(
            owner_struct,
            &nested.proc_name,
            &format!("{physical_prefix}{field_name}__"),
            lowering_shapes,
            stack,
            output,
        );
    }
    stack.remove(proc_name);
}

fn preprocess_program_for_analysis_with_inputs(
    mut program: Program,
    options: AnalysisOptions,
    inputs: &CompileInputs,
) -> Result<Program, Vec<Diagnostic>> {
    validate_analysis_options(options)?;
    apply_compile_inputs(&mut program, inputs)?;
    inject_auto_std_math(&mut program)?;
    flatten_namespaces_for_semantics(&mut program, options)?;
    fold_host_sr_builtin(&mut program, options);
    Ok(program)
}

fn collect_struct_field_dependencies(def: &StructDef) -> Vec<String> {
    let mut deps = Vec::<String>::new();
    for field in &def.fields {
        match &field.ty {
            FieldType::Generic(name) => deps.push(name.clone()),
            FieldType::Array(spec) => {
                if let ArrayElemType::Struct(name) = &spec.elem {
                    deps.push(name.clone());
                }
            }
            FieldType::Scalar(_) | FieldType::Tuple(_) => {}
        }
    }
    deps
}

fn order_struct_defs_for_field_dependencies(structs: &mut Vec<StructDef>) {
    let index_by_name = structs
        .iter()
        .enumerate()
        .map(|(idx, def)| (def.name.clone(), idx))
        .collect::<HashMap<_, _>>();
    if index_by_name.is_empty() {
        return;
    }

    let deps_by_index = structs
        .iter()
        .map(collect_struct_field_dependencies)
        .collect::<Vec<_>>();
    let mut remaining = (0..structs.len()).collect::<Vec<_>>();
    let mut ordered = Vec::<StructDef>::with_capacity(structs.len());

    while !remaining.is_empty() {
        let remaining_names = remaining
            .iter()
            .map(|idx| structs[*idx].name.as_str())
            .collect::<HashSet<_>>();
        let mut ready_pos = None;
        for (pos, idx) in remaining.iter().enumerate() {
            let self_name = structs[*idx].name.as_str();
            let ready = deps_by_index[*idx].iter().all(|dep| {
                dep == self_name
                    || !index_by_name.contains_key(dep)
                    || !remaining_names.contains(dep.as_str())
            });
            if ready {
                ready_pos = Some(pos);
                break;
            }
        }

        let Some(pos) = ready_pos else {
            ordered.extend(remaining.drain(..).map(|idx| structs[idx].clone()));
            break;
        };
        let idx = remaining.remove(pos);
        ordered.push(structs[idx].clone());
    }

    *structs = ordered;
}

fn rewrite_function_overloads(
    def: &mut FunctionDef,
    seed: &crate::def_semantics::CallTypeEnv,
    context: crate::def_semantics::CallTypeContext<'_>,
    owner: crate::def_semantics::OverloadOwnerContext,
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    crate::def_semantics::rewrite_overloaded_calls_in_function(
        def, seed, context, owner, overloads, errors,
    )
}

fn register_generated_method_owners(
    method_owners: &mut HashMap<String, String>,
    mono_cache: &HashMap<(String, Vec<crate::def_semantics::MonoParamKey>), String>,
) {
    let generated = mono_cache
        .iter()
        .filter_map(|((original_name, _), generated_name)| {
            (!method_owners.contains_key(generated_name))
                .then(|| method_owners.get(original_name).cloned())
                .flatten()
                .map(|owner| (generated_name.clone(), owner))
        })
        .collect::<Vec<_>>();
    method_owners.extend(generated);
}

fn bind_event_param_call_types(env: &mut crate::def_semantics::CallTypeEnv, event: &EventDef) {
    for param in &event.params {
        env.shadow_binding(&param.name);
        match &param.ty {
            EventParamType::Scalar(ty) => {
                env.scalar_types.insert(param.name.clone(), *ty);
            }
            EventParamType::Array { elem, size } => {
                env.array_types.insert(
                    param.name.clone(),
                    crate::def_semantics::CallArrayType::primitive(
                        *elem,
                        crate::def_semantics::const_positive_usize_for_call_type(size),
                    ),
                );
            }
            EventParamType::Slice { elem } => {
                env.array_types.insert(
                    param.name.clone(),
                    crate::def_semantics::CallArrayType::primitive(*elem, None),
                );
            }
            EventParamType::GenericScalar { .. }
            | EventParamType::GenericArray { .. }
            | EventParamType::GenericSlice { .. } => {}
        }
    }
}

/// Resolves source-level call-shape expressions once before overload
/// resolution, return inference, and monomorphization inspect signatures or
/// array constructors. Those passes can then share a small literal-only shape
/// representation without each reimplementing compile-time evaluation.
fn normalize_runtime_call_shape_exprs(
    defs: &mut [FunctionDef],
    events: &mut [EventDef],
    init: &mut [Stmt],
    block_pre: &mut [Stmt],
    sample: &mut [Stmt],
    block_post: &mut [Stmt],
    options: AnalysisOptions,
) {
    fn normalize(expr: &mut Expr, options: AnalysisOptions, context: &str) {
        let mut discarded = Vec::new();
        let Some(value) = eval_data_size_expr(expr, options, context, &mut discarded) else {
            return;
        };
        let Ok(value) = i64::try_from(value) else {
            return;
        };
        let loc = expr.loc();
        *expr = Expr::int(value).with_loc(loc);
    }

    fn normalize_expr(expr: &mut Expr, options: AnalysisOptions) {
        match expr {
            Expr::ArrayCtor { spec, init, .. } => {
                normalize(&mut spec.size, options, "array constructor length");
                if let Some(values) = init {
                    for value in values {
                        normalize_expr(value, options);
                    }
                }
            }
            Expr::Index { index, .. } => normalize_expr(index, options),
            Expr::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    normalize_expr(coordinate, options);
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                normalize_expr(lhs, options);
                normalize_expr(rhs, options);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    normalize_expr(arg, options);
                }
            }
            Expr::UserCall { args, .. } => {
                for arg in args {
                    normalize_expr(&mut arg.expr, options);
                }
            }
            Expr::Cast { expr, .. }
            | Expr::UnaryNot { expr, .. }
            | Expr::UnaryBitNot { expr, .. } => normalize_expr(expr, options),
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    normalize_expr(value, options);
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    fn normalize_target(target: &mut AssignTarget, options: AnalysisOptions) {
        match target {
            AssignTarget::Index { index, .. } => normalize_expr(index, options),
            AssignTarget::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    normalize_expr(coordinate, options);
                }
            }
            AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        }
    }

    fn normalize_stmts(stmts: &mut [Stmt], options: AnalysisOptions) {
        for stmt in stmts {
            match stmt {
                Stmt::Const { decl, .. } => normalize_expr(&mut decl.expr, options),
                Stmt::Assign { target, expr, .. } => {
                    normalize_target(target, options);
                    normalize_expr(expr, options);
                }
                Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                    normalize_expr(expr, options);
                }
                Stmt::Print { values, .. } => {
                    for value in values {
                        normalize_expr(value, options);
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    normalize_expr(cond, options);
                    normalize_stmts(then_branch, options);
                    normalize_stmts(else_branch, options);
                }
                Stmt::For {
                    step,
                    start,
                    end,
                    body,
                    ..
                } => {
                    if let Some(step) = step {
                        normalize_expr(step, options);
                    }
                    normalize_expr(start, options);
                    normalize_expr(end, options);
                    normalize_stmts(body, options);
                }
                Stmt::While { cond, body, .. } => {
                    normalize_expr(cond, options);
                    normalize_stmts(body, options);
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
        }
    }

    for def in defs {
        let owner = def.name.clone();
        for param in &mut def.params {
            match param.ty.as_mut() {
                Some(FnParamType::SizedArray { size, .. }) => normalize(
                    size,
                    options,
                    &format!("function '{owner}' parameter '{}' array length", param.name),
                ),
                Some(FnParamType::Buffer(buffer))
                | Some(FnParamType::BufferArray { buffer, .. }) => {
                    if let BufferChannels::Static(channels) = &mut buffer.channels {
                        normalize(
                            channels,
                            options,
                            &format!(
                                "function '{owner}' parameter '{}' buffer channels",
                                param.name
                            ),
                        );
                    }
                }
                Some(
                    FnParamType::Primitive(_)
                    | FnParamType::Struct(_)
                    | FnParamType::Array(_)
                    | FnParamType::ArrayGeneric(_)
                    | FnParamType::BareBuffer
                    | FnParamType::Tuple(_),
                )
                | None => {}
            }
            if let Some(default) = &mut param.default {
                normalize_expr(default, options);
            }
        }
        normalize_stmts(&mut def.body, options);
    }

    for event in events {
        for param in &mut event.params {
            if let EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } =
                &mut param.ty
            {
                normalize(
                    size,
                    options,
                    &format!(
                        "event '{}' parameter '{}' array length",
                        event.name, param.name
                    ),
                );
            }
            if let Some(default) = &mut param.default {
                normalize_expr(default, options);
            }
        }
        normalize_stmts(&mut event.body, options);
    }

    for stmts in [init, block_pre, sample, block_post] {
        normalize_stmts(stmts, options);
    }
}

/// Applies a call-typing pass to every executable region using the language's
/// visibility graph. The same transformation and inference rules run in every
/// region; only bindings that are semantically visible are propagated.
fn rewrite_executable_call_scopes(
    init: &mut [Stmt],
    block_pre: &mut [Stmt],
    sample: &mut [Stmt],
    block_post: &mut [Stmt],
    events: &mut [EventDef],
    seed: &crate::def_semantics::CallTypeEnv,
    mut rewrite: impl FnMut(&mut [Stmt], &mut crate::def_semantics::CallTypeEnv),
) -> crate::def_semantics::CallTypeEnv {
    let mut init_env = seed.clone();
    rewrite(init, &mut init_env);

    let mut block_carried_env = init_env.clone();
    rewrite(block_pre, &mut block_carried_env);

    let mut sample_env = block_carried_env.clone();
    rewrite(sample, &mut sample_env);

    let mut block_post_env = block_carried_env;
    rewrite(block_post, &mut block_post_env);

    for event in events {
        let mut event_env = init_env.clone();
        bind_event_param_call_types(&mut event_env, event);
        rewrite(&mut event.body, &mut event_env);
    }

    // Callable executable regions may run outside the current block, so they
    // inherit initialized owner state but never block- or sample-local bindings.
    init_env
}

fn def_call_type_env<'a>(
    def: &FunctionDef,
    runtime_def_names: &HashSet<String>,
    lexical_env: &'a crate::def_semantics::CallTypeEnv,
    owner_env: &'a crate::def_semantics::CallTypeEnv,
) -> &'a crate::def_semantics::CallTypeEnv {
    // Tasks and delegate handlers are represented as functions only after
    // source-block lowering. Preserve their executable-owner visibility;
    // authored defs remain lexical-local.
    if runtime_def_names.contains(&def.name) {
        owner_env
    } else {
        lexical_env
    }
}

pub fn analyze(program: Program) -> Result<TypedProgram, Vec<Diagnostic>> {
    analyze_with_options(program, AnalysisOptions::default())
}

/// Resolves executable-root configuration declarations without analyzing or
/// lowering the runtime program.
pub fn inspect_compile_constants(
    program: Program,
    options: AnalysisOptions,
    inputs: &CompileInputs,
) -> Result<Vec<CompileConstDescriptor>, Vec<Diagnostic>> {
    let mut program = preprocess_program_for_analysis_with_inputs(program, options, inputs)?;
    let mut errors = Vec::new();
    let artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if errors.is_empty() {
        Ok(compile_const_descriptors(&program, &artifacts))
    } else {
        Err(errors)
    }
}

#[cfg(test)]
pub(crate) fn preprocess_const_semantics_for_lowering(
    program: Program,
    options: AnalysisOptions,
) -> Result<Program, Vec<Diagnostic>> {
    let mut program =
        preprocess_program_for_analysis_with_inputs(program, options, &CompileInputs::default())?;

    let mut errors = Vec::new();
    let const_artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    reject_const_shadowing_in_program(&program, &const_artifacts.const_values, &mut errors);
    reject_const_assignments_in_program(&program, &const_artifacts.const_values, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    program
        .blocks
        .retain(|block| !matches!(block, Block::Def(def) if def.is_const));
    Ok(program)
}

pub fn lower_graphs_for_inspection_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<Program, Vec<Diagnostic>> {
    lower_graphs_for_inspection_with_options_and_inputs(program, options, &CompileInputs::default())
}

pub fn lower_graphs_for_inspection_with_options_and_inputs(
    program: Program,
    options: AnalysisOptions,
    inputs: &CompileInputs,
) -> Result<Program, Vec<Diagnostic>> {
    let mut program = preprocess_program_for_analysis_with_inputs(program, options, inputs)?;

    let mut errors = Vec::new();
    let const_artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    reject_const_shadowing_in_program(&program, &const_artifacts.const_values, &mut errors);
    reject_const_assignments_in_program(&program, &const_artifacts.const_values, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    program
        .blocks
        .retain(|block| !matches!(block, Block::Def(def) if def.is_const));
    prepare_processors_for_graph_inspection(&mut program, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    lower_graph_blocks(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(program)
}

pub fn analyze_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<TypedProgram, Vec<Diagnostic>> {
    analyze_with_options_and_inputs(program, options, &CompileInputs::default())
}

/// Analyzes a program under one immutable set of host-selected compile
/// constants. Omitted declarations use their source initializers.
pub fn analyze_with_options_and_inputs(
    program: Program,
    options: AnalysisOptions,
    inputs: &CompileInputs,
) -> Result<TypedProgram, Vec<Diagnostic>> {
    let original_last_block_loc = program
        .blocks
        .iter()
        .rev()
        .map(Block::loc)
        .find(|loc| !loc.is_zero());
    let mut program = preprocess_program_for_analysis_with_inputs(program, options, inputs)?;
    annotate_print_origins(&mut program);

    let mut errors = Vec::new();
    validate_owner_callable_bindings(&program, &mut errors);
    let const_artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    let const_array_infos = const_array_info_map(&const_artifacts.const_arrays);
    let mut const_scalar_names = const_artifacts
        .const_values
        .iter()
        .filter_map(|(name, value)| match value {
            ConstValue::Scalar(_) => Some(name.clone()),
            ConstValue::Array { .. } => None,
        })
        .collect::<Vec<_>>();
    const_scalar_names.sort();
    let mut const_def_names = const_artifacts
        .const_defs
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    const_def_names.sort();
    reject_const_shadowing_in_program(&program, &const_artifacts.const_values, &mut errors);
    reject_const_assignments_in_program(&program, &const_artifacts.const_values, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    program
        .blocks
        .retain(|block| !matches!(block, Block::Def(def) if def.is_const));
    validate_task_source_model(&program, &mut errors);
    validate_pinned_bindings(&program, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    let const_arrays = const_artifacts.const_arrays;
    let mut declared_proc_integer_ranges = HashMap::new();
    for proc_def in program.blocks.iter_mut().filter_map(|block| match block {
        Block::Proc(proc_def) => Some(proc_def),
        _ => None,
    }) {
        // Canonicalize source processor declarations before desugaring or
        // specialization clones them into generated initializer functions.
        rewrite_integer_binding_ranges_in_list(
            &mut proc_def.init.body,
            &HashMap::new(),
            options,
            &mut errors,
        );
        let mut ranges = HashMap::new();
        collect_integer_binding_range_assignments(&proc_def.init.body, &mut ranges);
        if !ranges.is_empty() {
            declared_proc_integer_ranges.insert(proc_def.name.clone(), ranges);
        }
    }
    let ProcessorDesugarResult {
        program,
        runtime_def_names,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
        proc_instance_oversample_factors,
        proc_api,
        lowering_shapes,
        top_level_proc_rewrite,
        pinned_proc_fields,
        compiler_owned_proc_fields,
        top_level_delegates,
    } = desugar_processors(program, options, &const_array_infos, &mut errors);
    let mut pinned_state_roots = program
        .block(BlockKind::Init)
        .and_then(|block| match block {
            Block::Init(init) => Some(init.pinned_roots.iter().cloned()),
            _ => None,
        })
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
    let mut compiler_owned_state_roots = HashSet::new();
    let compiler_scratch_state_roots = program
        .block(BlockKind::Init)
        .and_then(|block| match block {
            Block::Init(init) => Some(init.compiler_scratch_roots.iter().cloned()),
            _ => None,
        })
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
    for (instance, proc_instance) in &top_level_proc_rewrite.global_proc_instances {
        if let Some(fields) = pinned_proc_fields.get(&proc_instance.proc_name) {
            pinned_state_roots.extend(fields.iter().map(|field| format!("{instance}.{field}")));
        }
        if let Some(fields) = compiler_owned_proc_fields.get(&proc_instance.proc_name) {
            compiler_owned_state_roots
                .extend(fields.iter().map(|field| format!("{instance}.{field}")));
        }
    }

    let mut seen_singleton = HashSet::new();
    for block in &program.blocks {
        let kind = block.kind();
        if matches!(
            kind,
            BlockKind::Def | BlockKind::Struct | BlockKind::Proc | BlockKind::Const
        ) {
            continue;
        }
        if !seen_singleton.insert(kind) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate block '{:?}'", kind).to_lowercase(),
                block.loc(),
            ));
        }
    }

    let ins_explicit = program.block(BlockKind::Ins).is_some();
    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let audio_outs_explicit = program.block(BlockKind::Outs).is_some();
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let control_outs_explicit = program.block(BlockKind::KOuts).is_some();
    let raw_kouts = match program.block(BlockKind::KOuts) {
        Some(Block::KOuts(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let outs_explicit = audio_outs_explicit || control_outs_explicit;
    let params_block = program
        .block(BlockKind::Params)
        .and_then(|block| match block {
            Block::Params(v) => Some(v),
            _ => None,
        });
    let params_explicit = params_block.is_some();
    let raw_params = params_block.map(|v| v.decls.clone()).unwrap_or_default();
    let explicit_param_prefix = params_block.map(|v| v.deferred_prefix.as_str());
    let params_block_is_kins = matches!(explicit_param_prefix, Some("kin"));
    let mut events = match program.block(BlockKind::Events) {
        Some(Block::Events(v)) => v.events.clone(),
        _ => Vec::new(),
    };
    let typed_delegates = coerce_typed_delegates(&top_level_delegates, options, &mut errors);
    let buffers = match program.block(BlockKind::Buffers) {
        Some(Block::Buffers(v)) => v.decls.clone(),
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

    for def in &mut defs {
        if !def.type_params.is_empty() {
            let mut seen = HashSet::new();
            for tp in &def.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate type parameter '{}' in def '{}'", tp, def.name),
                        def.loc,
                    ));
                }
            }
        }
    }

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
    }
    let block_section_without_sample =
        program.block(BlockKind::Block).is_some() && nested_block_sample.is_none();
    let top_sample = match program.block(BlockKind::Sample) {
        Some(Block::Sample(v)) => Some(v.clone()),
        _ => None,
    };
    let has_top_sample_block = top_sample.is_some();
    let sample_conflict_loc = top_sample
        .as_ref()
        .and_then(|sample| sample.loc.cloned())
        .or_else(|| {
            nested_block_sample
                .as_ref()
                .and_then(|sample| sample.loc.cloned())
        });

    let sample_block = match (nested_block_sample, top_sample) {
        (Some(_), Some(_)) => {
            errors.push(Diagnostic::semantic_span(
                "sample block cannot be declared both at top-level and inside block",
                sample_conflict_loc.as_ref(),
            ));
            SampleBlock {
                loc: Default::default(),
                oversample_factor: None,
                body: Vec::new(),
            }
        }
        (Some(v), None) => v,
        (None, Some(v)) => v,
        (None, None) => SampleBlock {
            loc: Default::default(),
            oversample_factor: None,
            body: Vec::new(),
        },
    };
    let sample_oversample_factor = validated_sample_oversample_factor(
        sample_block.oversample_factor.as_ref(),
        options,
        "sample block",
        &mut errors,
    );
    let mut sample = sample_block.body;

    let missing_sample_loc = original_last_block_loc.as_ref();
    if sample.is_empty()
        && program.block(BlockKind::Block).is_none()
        && requires_entry_sample(&program)
    {
        let missing_entry_message = if control_outs_explicit && !audio_outs_explicit {
            "missing required 'block' section"
        } else {
            "missing required 'sample' block"
        };
        errors.push(
            Diagnostic::semantic_span(missing_entry_message, missing_sample_loc).compiler_only(),
        );
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
        let nominal_symbols = struct_symbols
            .union(&callable_symbols)
            .cloned()
            .collect::<HashSet<_>>();
        let nominal_namespaces = collect_declared_namespaces(&nominal_symbols);

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
                            DiagCtx::new(field.ty_loc),
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
                        &nominal_symbols,
                        &nominal_namespaces,
                        &mut errors,
                        &format!("struct '{}.{}' default", s.name, field.name),
                        None,
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
                            DiagCtx::new(param.ty_loc),
                            &mut errors,
                        );
                    }
                    if let Some(default) = &mut param.default {
                        qualify_expr_namespaced_symbols(
                            default,
                            &struct_ns,
                            &callable_symbols,
                            &callable_namespaces,
                            &nominal_symbols,
                            &nominal_namespaces,
                            &mut errors,
                            &format!(
                                "method '{}.{}' parameter '{}' default",
                                s.name, method.name, param.name
                            ),
                            None,
                        );
                    }
                }
                for stmt in &mut method.body {
                    qualify_stmt_namespaced_symbols(
                        stmt,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &nominal_symbols,
                        &nominal_namespaces,
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
                        DiagCtx::new(param.ty_loc),
                        &mut errors,
                    );
                }
                if let Some(default) = &mut param.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &def_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &nominal_symbols,
                        &nominal_namespaces,
                        &mut errors,
                        &format!("function '{}' parameter '{}' default", def.name, param.name),
                        None,
                    );
                }
            }
            for stmt in &mut def.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    &def_ns,
                    &callable_symbols,
                    &callable_namespaces,
                    &nominal_symbols,
                    &nominal_namespaces,
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
                &nominal_symbols,
                &nominal_namespaces,
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
                &nominal_symbols,
                &nominal_namespaces,
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
                &nominal_symbols,
                &nominal_namespaces,
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
                &nominal_symbols,
                &nominal_namespaces,
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
                    &nominal_symbols,
                    &nominal_namespaces,
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
                errors.push(Diagnostic::semantic_span(
                    format!("duplicate generic struct '{}'", s.name),
                    s.loc,
                ));
                continue;
            }
            let mut seen = HashSet::new();
            for tp in &s.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate generic type parameter '{}' in struct '{}'",
                            tp, s.name
                        ),
                        s.loc,
                    ));
                }
            }
            for method in &s.methods {
                for tp in &method.type_params {
                    if s.type_params.contains(tp) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "type parameter '{}' on method '{}.{}' shadows '{}' from struct '{}'; use a different name",
                                tp, s.name, method.name, tp, s.name
                            ),
                            method.loc,
                        ));
                    }
                }
            }
            generic_templates.insert(s.name.clone(), s);
        }
        generic_struct_template_names = generic_templates.keys().cloned().collect();

        let mut generated_specializations = HashMap::<String, StructDef>::new();
        for s in &mut concrete_structs {
            rewrite_generic_struct_field_types(
                s,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
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
    order_struct_defs_for_field_dependencies(&mut struct_defs_raw);

    let init_ranges =
        rewrite_integer_binding_ranges_in_list(&mut init, &HashMap::new(), options, &mut errors);
    let mut runtime_ranges =
        rewrite_integer_binding_ranges_in_list(&mut block_pre, &init_ranges, options, &mut errors);
    rewrite_integer_binding_ranges_in_list(&mut sample, &runtime_ranges, options, &mut errors);
    rewrite_integer_binding_ranges_in_list(&mut block_post, &runtime_ranges, options, &mut errors);
    for event in &mut events {
        let inherited = integer_binding_ranges_outside_params(
            &runtime_ranges,
            event.params.iter().map(|param| &param.name),
        );
        rewrite_integer_binding_ranges_in_list(&mut event.body, &inherited, options, &mut errors);
    }
    let mut def_integer_ranges = Vec::with_capacity(defs.len());
    for def in &mut defs {
        let inherited = integer_binding_ranges_outside_params(
            &runtime_ranges,
            def.params.iter().map(|param| &param.name),
        );
        let ranges =
            rewrite_integer_binding_ranges_in_list(&mut def.body, &inherited, options, &mut errors);
        def_integer_ranges.push(ranges);
    }
    let mut proc_state_ranges = HashMap::new();
    for owner in lowering_shapes.keys() {
        let mut ranges = HashMap::new();
        collect_flattened_proc_integer_ranges(
            owner,
            "",
            &declared_proc_integer_ranges,
            &lowering_shapes,
            &mut HashSet::new(),
            &mut ranges,
        );
        if !ranges.is_empty() {
            proc_state_ranges.insert(owner.clone(), ranges);
        }
    }
    for (def, ranges) in defs.iter().zip(&def_integer_ranges) {
        let Some(owner) = def.name.strip_suffix(".__onda_proc_init") else {
            continue;
        };
        let proc_ranges = proc_state_ranges.entry(owner.to_owned()).or_default();
        proc_ranges.extend(ranges.clone());
        collect_integer_binding_range_assignments(&def.body, proc_ranges);
    }
    for (def, ranges) in defs.iter_mut().zip(&mut def_integer_ranges) {
        let Some((owner, generated_suffix)) = def.name.split_once(".__onda_proc_") else {
            continue;
        };
        if generated_suffix == "init" {
            continue;
        }
        let Some(proc_ranges) = proc_state_ranges.get(owner) else {
            continue;
        };
        let inherited = proc_integer_binding_range_aliases(proc_ranges);
        let inherited = integer_binding_ranges_outside_params(
            &inherited,
            def.params.iter().map(|param| &param.name),
        );
        *ranges =
            rewrite_integer_binding_ranges_in_list(&mut def.body, &inherited, options, &mut errors);
    }
    check_local_port_duplicates(&raw_ins, "input", &mut errors);
    check_local_port_duplicates(&raw_outs, "output", &mut errors);
    check_local_port_duplicates(&raw_kouts, "control output", &mut errors);
    check_local_param_duplicates(&raw_params, &mut errors);
    check_control_output_reserved_audio_names(&raw_kouts, "control output", &mut errors);

    let inferred_io = infer_numbered_io_from_sample(&sample);
    let mut inferred_owner_names = IoInference::default();
    for stmt in &init {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for stmt in &block_pre {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for stmt in &sample {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for stmt in &block_post {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for event in &events {
        for stmt in &event.body {
            infer_io_from_stmt(stmt, &mut inferred_owner_names);
        }
    }
    let inferred_param_prefix = match explicit_param_prefix {
        Some("kin") => {
            if inferred_owner_names.max_param > 0 {
                errors.push(Diagnostic::semantic_span(
                    "implicit paramN parameters cannot be mixed with a kins block; use kinN names with kins",
                    Span::ZERO,
                ));
            }
            "kin"
        }
        Some(_) => {
            if inferred_owner_names.max_kin > 0 {
                errors.push(Diagnostic::semantic_span(
                    "implicit kinN parameters cannot be mixed with a params block; use paramN names with params",
                    Span::ZERO,
                ));
            }
            "param"
        }
        None if inferred_owner_names.max_kin > 0 && inferred_owner_names.max_param == 0 => "kin",
        None => {
            if inferred_owner_names.max_kin > 0 && inferred_owner_names.max_param > 0 {
                errors.push(Diagnostic::semantic_span(
                    "implicit kinN and paramN parameters cannot be mixed in the same top-level program",
                    Span::ZERO,
                ));
            }
            "param"
        }
    };
    let inferred_param_max = if inferred_param_prefix == "kin" {
        inferred_owner_names.max_kin
    } else {
        inferred_owner_names.max_param
    };
    let params =
        normalize_numbered_param_decls(&raw_params, inferred_param_prefix, inferred_param_max);

    let ins_ports = normalize_numbered_port_decls(&raw_ins, "in", inferred_io.max_in);
    let mut audio_out_ports = normalize_numbered_port_decls(&raw_outs, "out", inferred_io.max_out);
    for port in &mut audio_out_ports {
        port.output_timing = Some(OutputTiming::Sample);
    }
    let mut control_out_ports =
        normalize_numbered_port_decls(&raw_kouts, "kout", inferred_owner_names.max_kout);
    for port in &mut control_out_ports {
        port.output_timing = Some(OutputTiming::Block);
    }
    check_port_name_conflicts(
        &audio_out_ports,
        "output",
        &control_out_ports,
        "control output",
        &mut errors,
    );
    let input_declared_names = ins_ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let mut output_declared_names = audio_out_ports
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    output_declared_names.extend(control_out_ports.iter().map(|p| p.name.clone()));
    let (ins, in_types, in_arrays, in_defaults, in_ranges) =
        expand_port_decls(&ins_ports, "input", options, &mut errors);
    let (outs, out_types, out_arrays, _out_defaults, _out_ranges) =
        expand_port_decls(&audio_out_ports, "output", options, &mut errors);
    let (
        control_outs,
        control_out_types,
        control_out_arrays,
        _control_out_defaults,
        _control_out_ranges,
    ) = expand_port_decls(&control_out_ports, "control output", options, &mut errors);
    if !outs.is_empty() && block_section_without_sample && !has_top_sample_block {
        let block_loc = program
            .block(BlockKind::Block)
            .map(Block::loc)
            .unwrap_or_default();
        errors.push(
            Diagnostic::semantic_span(
                "block section with sample-rate outputs must include nested 'sample' block",
                block_loc,
            )
            .compiler_only(),
        );
    }

    let (typed_params, param_arrays) = coerce_params(&params, options, &mut errors);
    let port_index_params = uniform_port_index_info_from_types(
        params_explicit,
        typed_params.len(),
        typed_params.iter().map(|param| param.ty),
    );
    let port_index_ins = uniform_port_index_info_from_names(ins_explicit, &ins, &in_types);
    let mut dynamic_param_array_names = HashSet::<String>::new();
    if port_index_params.is_some() {
        dynamic_param_array_names.insert("params".to_owned());
        if params_block_is_kins {
            dynamic_param_array_names.insert("kins".to_owned());
        }
    }
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

    let mut input_names = in_ranges.keys().cloned().collect::<Vec<_>>();
    input_names.sort();
    let (input_aliases, input_hoists) =
        build_top_level_range_clamp_entry(&input_names, &in_ranges, |name| {
            make_unique_temp(format!(
                "__onda_clamped_in__{}",
                sanitize_symbol_component(name)
            ))
        });

    let mut param_names_sorted = param_ranges.keys().cloned().collect::<Vec<_>>();
    param_names_sorted.sort();
    let (param_aliases, param_hoists) =
        build_top_level_range_clamp_entry(&param_names_sorted, &param_ranges, |name| {
            make_unique_temp(format!(
                "__onda_clamped_param__{}",
                sanitize_symbol_component(name)
            ))
        });
    let init_param_hoists = param_hoists.clone();
    let no_shadowed_names = HashSet::new();

    let mut process_clamp_usage = TopLevelRangeClampUsage::default();
    for stmt in &mut block_pre {
        rewrite_top_level_range_clamps_in_stmt(
            stmt,
            &input_aliases,
            &param_aliases,
            &no_shadowed_names,
            false,
            true,
            &mut process_clamp_usage,
        );
    }
    for stmt in &mut sample {
        rewrite_top_level_range_clamps_in_stmt(
            stmt,
            &input_aliases,
            &param_aliases,
            &no_shadowed_names,
            true,
            true,
            &mut process_clamp_usage,
        );
    }
    for stmt in &mut block_post {
        rewrite_top_level_range_clamps_in_stmt(
            stmt,
            &input_aliases,
            &param_aliases,
            &no_shadowed_names,
            false,
            true,
            &mut process_clamp_usage,
        );
    }

    let process_param_hoists =
        used_top_level_range_clamp_hoists(param_hoists, &process_clamp_usage.aliases);
    if !process_param_hoists.is_empty() {
        let mut rewritten = process_param_hoists;
        rewritten.append(&mut block_pre);
        block_pre = rewritten;
    }
    let process_input_hoists =
        used_top_level_range_clamp_hoists(input_hoists, &process_clamp_usage.aliases);
    if !process_input_hoists.is_empty() {
        let mut rewritten = process_input_hoists;
        rewritten.append(&mut sample);
        sample = rewritten;
    }

    let mut init_clamp_usage = TopLevelRangeClampUsage::default();
    for stmt in &mut init {
        rewrite_top_level_range_clamps_in_stmt(
            stmt,
            &HashMap::new(),
            &param_aliases,
            &no_shadowed_names,
            false,
            true,
            &mut init_clamp_usage,
        );
    }
    let mut used_init_hoists =
        used_top_level_range_clamp_hoists(init_param_hoists, &init_clamp_usage.aliases);
    if !used_init_hoists.is_empty() {
        used_init_hoists.append(&mut init);
        init = used_init_hoists;
    }

    for event in &mut events {
        let (event_param_aliases, event_param_hoists) =
            build_top_level_range_clamp_entry(&param_names_sorted, &param_ranges, |name| {
                make_unique_temp(format!(
                    "__onda_clamped_event_{}_param__{}",
                    sanitize_symbol_component(&event.name),
                    sanitize_symbol_component(name)
                ))
            });
        let mut event_clamp_usage = TopLevelRangeClampUsage::default();
        let shadowed_names = event
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect::<HashSet<_>>();
        for stmt in &mut event.body {
            rewrite_top_level_range_clamps_in_stmt(
                stmt,
                &HashMap::new(),
                &event_param_aliases,
                &shadowed_names,
                false,
                true,
                &mut event_clamp_usage,
            );
        }
        let mut used_event_hoists =
            used_top_level_range_clamp_hoists(event_param_hoists, &event_clamp_usage.aliases);
        if !used_event_hoists.is_empty() {
            used_event_hoists.append(&mut event.body);
            event.body = used_event_hoists;
        }
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
    check_unique_set(&const_scalar_names, "const", &mut all_declared, &mut errors);
    check_unique_set(
        &const_arrays
            .iter()
            .map(|array| array.name.clone())
            .collect::<Vec<_>>(),
        "const array",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &const_def_names,
        "const def",
        &mut all_declared,
        &mut errors,
    );

    let mut struct_defs = HashMap::new();
    let mut seen_struct_defs = HashMap::<String, StructDef>::new();
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
            errors.push(Diagnostic::semantic_span(
                format!("struct name '{}' is reserved as a builtin constant", s.name),
                s.loc,
            ));
            continue;
        }
        if let Some(existing) = seen_struct_defs.get(&s.name) {
            if existing == s {
                continue;
            }
            errors.push(Diagnostic::semantic_span(
                format!("duplicate struct '{}'", s.name),
                s.loc,
            ));
            continue;
        }
        if all_declared.contains(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("struct name '{}' conflicts with existing symbol", s.name),
                s.loc,
            ));
            continue;
        }
        if struct_defs.contains_key(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate struct '{}'", s.name),
                s.loc,
            ));
            continue;
        }
        let typed_fields = coerce_struct_fields(
            &s.name,
            &s.type_params,
            &s.fields,
            &struct_defs,
            options,
            &mut errors,
        );
        struct_defs.insert(s.name.clone(), typed_fields.clone());
        typed_structs.push(TypedStruct {
            name: s.name.clone(),
            fields: typed_fields,
        });
        all_declared.insert(s.name.clone());
        seen_struct_defs.insert(s.name.clone(), s.clone());

        for method in &s.methods {
            if is_unsafe_index_method_name(&method.name) {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "cannot redefine builtin method '{}.{}'",
                        s.name, method.name
                    ),
                    method.loc,
                ));
                continue;
            }
            for tp in &method.type_params {
                if s.type_params.contains(tp) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "type parameter '{}' on method '{}.{}' shadows '{}' from struct '{}'; use a different name",
                            tp, s.name, method.name, tp, s.name
                        ),
                        method.loc,
                    ));
                }
            }
            if !method.type_params.is_empty() {
                let mut seen = HashSet::new();
                for tp in &method.type_params {
                    if !seen.insert(tp.clone()) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "duplicate type parameter '{}' in method '{}.{}'",
                                tp, s.name, method.name
                            ),
                            method.loc,
                        ));
                    }
                }
            }
            if method.params.first().map(|p| p.name.as_str()) != Some("self") {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "method '{}.{}' must declare 'self' as first parameter",
                        s.name, method.name
                    ),
                    method.loc,
                ));
            }
            let fq_name = format!("{}.{}", s.name, method.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                method_self_struct.insert(fq_name.clone(), s.name.clone());
            }
            let mut desugared_method_body = method.body.clone();
            let mut method_struct_instances = HashMap::<String, String>::new();
            let mut method_struct_array_roots = HashMap::<String, String>::new();
            let method_ns = namespace_of_symbol(&s.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                register_struct_instance_and_array_roots(
                    "self",
                    &s.name,
                    &struct_defs,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                );
            }
            for stmt in &mut desugared_method_body {
                desugar_init_instance_method_calls(
                    stmt,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                    &struct_defs,
                    &method_ns,
                    &callable_symbols_for_method_sugar,
                );
            }
            let mut method_params = method.params.clone();
            if let Some(self_param) = method_params
                .first_mut()
                .filter(|param| param.name == "self")
            {
                self_param.ty = Some(FnParamType::Struct(s.name.clone()));
            }
            defs.push(FunctionDef {
                loc: method.loc,
                is_const: false,
                type_params: method.type_params.clone(),
                name: fq_name,
                params: method_params,
                return_ty: method.return_ty.clone(),
                return_ty_loc: method.return_ty_loc,
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

    normalize_struct_constructor_ranges_in_list(&mut init, &struct_defs);
    normalize_struct_constructor_ranges_in_list(&mut block_pre, &struct_defs);
    normalize_struct_constructor_ranges_in_list(&mut sample, &struct_defs);
    normalize_struct_constructor_ranges_in_list(&mut block_post, &struct_defs);
    for event in &mut events {
        normalize_struct_constructor_ranges_in_list(&mut event.body, &struct_defs);
    }
    for def in &mut defs {
        normalize_struct_constructor_ranges_in_list(&mut def.body, &struct_defs);
    }

    let mut desugar_struct_instances = HashMap::<String, String>::new();
    let mut desugar_struct_array_roots = HashMap::<String, String>::new();
    for stmt in &mut init {
        desugar_init_instance_method_calls(
            stmt,
            &mut desugar_struct_instances,
            &mut desugar_struct_array_roots,
            &struct_defs,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_pre {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_post {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut sample {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for event in &mut events {
        for stmt in &mut event.body {
            desugar_sample_instance_method_calls(
                stmt,
                &desugar_struct_instances,
                &desugar_struct_array_roots,
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
        let mut def_struct_array_roots = HashMap::<String, String>::new();
        for param in &def.params {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if struct_defs.contains_key(struct_name) {
                    register_struct_instance_and_array_roots(
                        &param.name,
                        struct_name,
                        &struct_defs,
                        &mut def_struct_instances,
                        &mut def_struct_array_roots,
                    );
                }
            }
        }
        let def_ns = namespace_of_symbol(&def.name);
        for stmt in &mut def.body {
            desugar_sample_instance_method_calls(
                stmt,
                &def_struct_instances,
                &def_struct_array_roots,
                &def_ns,
                &callable_symbols_for_method_sugar,
            );
        }
    }

    normalize_runtime_call_shape_exprs(
        &mut defs,
        &mut events,
        &mut init,
        &mut block_pre,
        &mut sample,
        &mut block_post,
        options,
    );

    let (overload_candidates, def_public_name_by_internal) =
        crate::def_semantics::prepare_function_overloads(&mut defs);
    let proc_type_names = proc_api.keys().cloned().collect::<HashSet<_>>();
    let mut method_self_struct_internal = defs
        .iter()
        .filter_map(|def| {
            let public_name = def_public_name_by_internal
                .get(&def.name)
                .cloned()
                .unwrap_or_else(|| def.name.clone());
            method_self_struct
                .get(&public_name)
                .cloned()
                .map(|owner| (def.name.clone(), owner))
        })
        .collect::<HashMap<_, _>>();
    let provisional_fn_signatures = defs
        .iter()
        .map(|def| (def.name.clone(), FnSignature::from_def(def)))
        .collect::<HashMap<_, _>>();

    // Authored defs are lexical-local. Keep compile-time data arrays available,
    // but do not let unrelated top-level ports, params, buffers, or state
    // influence their bodies. Compiler-generated executable defs select their
    // owner environment through `def_call_type_env` below.
    let mut function_env_seed = crate::def_semantics::CallTypeEnv::default();
    function_env_seed
        .array_types
        .extend(const_array_infos.iter().map(|(name, info)| {
            (
                name.clone(),
                crate::def_semantics::CallArrayType::primitive(info.elem_ty, Some(info.len)),
            )
        }));

    let mut pre_overload_return_types = HashMap::new();
    crate::def_semantics::refresh_monomorphized_return_types(
        &mut pre_overload_return_types,
        &defs,
        &[],
        &provisional_fn_signatures,
        &HashMap::new(),
        &function_env_seed,
        &struct_defs,
    );

    let mut top_level_env = crate::def_semantics::CallTypeEnv::default();
    top_level_env
        .scalar_types
        .extend(in_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(out_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(param_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .array_types
        .extend(in_arrays.iter().map(|(name, info)| {
            (
                name.clone(),
                crate::def_semantics::CallArrayType::primitive(info.elem_ty, Some(info.len)),
            )
        }));
    top_level_env
        .array_types
        .extend(out_arrays.iter().map(|(name, info)| {
            (
                name.clone(),
                crate::def_semantics::CallArrayType::primitive(info.elem_ty, Some(info.len)),
            )
        }));
    top_level_env
        .array_types
        .extend(param_arrays.iter().map(|(name, info)| {
            (
                name.clone(),
                crate::def_semantics::CallArrayType::primitive(info.elem_ty, Some(info.len)),
            )
        }));
    top_level_env
        .array_types
        .extend(const_array_infos.iter().map(|(name, info)| {
            (
                name.clone(),
                crate::def_semantics::CallArrayType::primitive(info.elem_ty, Some(info.len)),
            )
        }));
    if let Some(info) = port_index_ins {
        top_level_env.array_types.insert(
            "ins".to_owned(),
            crate::def_semantics::CallArrayType::primitive(info.elem_ty, None),
        );
    }
    if let Some(info) = port_index_params {
        let surface = if params_block_is_kins {
            "kins"
        } else {
            "params"
        };
        top_level_env.array_types.insert(
            surface.to_owned(),
            crate::def_semantics::CallArrayType::primitive(info.elem_ty, None),
        );
    }
    top_level_env.buffer_types.extend(
        typed_buffers
            .iter()
            .map(|b| (b.name.clone(), (b.elem_ty, b.channels.clone()))),
    );
    top_level_env.buffer_array_lens.extend(
        typed_buffers
            .iter()
            .filter(|buffer| buffer.is_array)
            .map(|buffer| (buffer.name.clone(), buffer.array_len)),
    );
    top_level_env
        .struct_instances
        .extend(desugar_struct_instances.clone());
    top_level_env.array_types.extend(
        top_level_proc_rewrite
            .global_proc_array_slots
            .iter()
            .filter_map(|(name, slots)| {
                let first = slots.first()?;
                let proc_name = &top_level_proc_rewrite
                    .global_proc_instances
                    .get(first)?
                    .proc_name;
                Some((
                    name.clone(),
                    crate::def_semantics::CallArrayType::nominal(
                        proc_name.clone(),
                        Some(slots.len()),
                    ),
                ))
            }),
    );

    let runtime_function_env = rewrite_executable_call_scopes(
        &mut init,
        &mut block_pre,
        &mut sample,
        &mut block_post,
        &mut events,
        &top_level_env,
        |stmts, env| {
            crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
                stmts,
                env,
                crate::def_semantics::CallTypeContext {
                    return_types: &pre_overload_return_types,
                    struct_defs: &struct_defs,
                },
                crate::def_semantics::OverloadOwnerContext {
                    defer_dependent_calls: true,
                },
                &overload_candidates,
                &mut errors,
            );
        },
    );
    for def in &mut defs {
        let env = def_call_type_env(
            def,
            &runtime_def_names,
            &function_env_seed,
            &runtime_function_env,
        );
        rewrite_function_overloads(
            def,
            env,
            crate::def_semantics::CallTypeContext {
                return_types: &pre_overload_return_types,
                struct_defs: &struct_defs,
            },
            crate::def_semantics::OverloadOwnerContext {
                defer_dependent_calls: true,
            },
            &overload_candidates,
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
        let def_diag = DiagCtx::new(def.loc);
        if is_builtin_constant_name(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!(
                    "function name '{}' is reserved as a builtin constant",
                    public_name
                ),
            );
            continue;
        }
        if is_builtin_function_name(&public_name) || is_internal_buffer_2d_fn(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("cannot redefine builtin function '{}'", public_name),
            );
            continue;
        }
        if struct_defs.contains_key(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("function name '{}' conflicts with struct name", public_name),
            );
            continue;
        }
        if all_declared.contains(&public_name)
            && !seen_public_function_symbols.contains(&public_name)
        {
            push_semantic(
                def_diag,
                &mut errors,
                format!(
                    "function name '{}' conflicts with existing symbol",
                    public_name
                ),
            );
            continue;
        }
        if fn_signatures.contains_key(&def.name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("duplicate function '{}'", def.name),
            );
            continue;
        }
        let mut signature = FnSignature::from_def(def);
        signature.display_name = Some(public_name.clone());
        fn_signatures.insert(def.name.clone(), signature);
        if seen_public_function_symbols.insert(public_name.clone()) {
            all_declared.insert(public_name.clone());
        }

        let mut local_params = HashSet::new();
        for p in &def.params {
            let param_diag = DiagCtx::new(p.ty_loc.or(p.loc));
            if is_builtin_constant_name(&p.name) {
                push_semantic(
                    param_diag,
                    &mut errors,
                    format!(
                        "function parameter '{}' in '{}' is reserved as a builtin constant",
                        p.name, def.name
                    ),
                );
            }
            if !local_params.insert(p.name.clone()) {
                push_semantic(
                    param_diag,
                    &mut errors,
                    format!(
                        "duplicate function parameter '{}' in '{}'",
                        p.name, public_name
                    ),
                );
            }
            if let Some(default) = &p.default {
                if matches!(
                    p.ty,
                    Some(FnParamType::Buffer(_))
                        | Some(FnParamType::Array(_))
                        | Some(FnParamType::ArrayGeneric(_))
                        | Some(FnParamType::BareBuffer)
                ) {
                    push_semantic(
                        param_diag,
                        &mut errors,
                        format!(
                            "function parameter '{}.{}' is a buffer and cannot have a default value",
                            public_name, p.name
                        ),
                    );
                }
                validate_default_expr(
                    default,
                    &mut errors,
                    &format!("function parameter '{}.{}'", public_name, p.name),
                );
            }
        }
    }

    // --- Pre-mono validation: check generic def call-site type args ---
    validate_generic_def_type_args_in_stmts(&init, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&block_pre, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&sample, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&block_post, &fn_signatures, &mut errors);
    for event in &events {
        validate_generic_def_type_args_in_stmts(&event.body, &fn_signatures, &mut errors);
    }
    for def in &defs {
        for default in def.params.iter().filter_map(|param| param.default.as_ref()) {
            validate_generic_def_type_args_in_expr(default, &fn_signatures, &mut errors);
        }
        validate_generic_def_type_args_in_stmts(&def.body, &fn_signatures, &mut errors);
    }

    // Validate templates before monomorphization: unused generic defs may be
    // removed below, but their source-level result contract must still hold.
    validate_def_return_control_flow(&defs, &fn_signatures, &mut errors);

    // --- Def monomorphization pass ---
    // Identify defs whose parameters require monomorphization (generic struct,
    // untyped array `[]`, bare `buffer`, or generic def type params `<T>`).
    {
        let mandatory_mono: HashSet<String> = fn_signatures
            .iter()
            .filter_map(|(name, sig)| {
                crate::def_semantics::signature_requires_monomorphization(
                    sig,
                    &generic_struct_template_names,
                    &proc_type_names,
                )
                .then_some(name.clone())
            })
            .collect();
        for name in &mandatory_mono {
            if let Some(signature) = fn_signatures.get_mut(name) {
                signature.requires_call_specialization = true;
            }
        }

        // Untyped scalar parameters are polymorphic in the source language.
        // Specialize them here from concrete call-site types so MIR and every
        // backend consume resolved signatures instead of independently
        // defaulting or inferring a type during code generation. Untyped
        // structural/proc-array parameters remain on the existing inference
        // path when their call argument is not a primitive or tuple.
        let scalar_mono_candidates = fn_signatures
            .iter()
            .filter_map(|(name, sig)| {
                let is_struct_method = method_self_struct_internal.contains_key(name);
                sig.param_types
                    .iter()
                    .enumerate()
                    .any(|(index, ty)| ty.is_none() && (!is_struct_method || index != 0))
                    .then_some(name.clone())
            })
            .collect::<HashSet<_>>();
        let mono_eligible = mandatory_mono
            .union(&scalar_mono_candidates)
            .cloned()
            .collect::<HashSet<_>>();

        // Also run mono pass when any def has untyped params that could be
        // inferred as tuple from call-site tuple literal args.
        let has_untyped_params = fn_signatures
            .values()
            .any(|sig| sig.param_types.iter().any(|pt| pt.is_none()));

        if !mono_eligible.is_empty() || has_untyped_params {
            let mut mono_return_types = HashMap::new();
            crate::def_semantics::refresh_monomorphized_return_types(
                &mut mono_return_types,
                &defs,
                &[],
                &fn_signatures,
                &HashMap::new(),
                &function_env_seed,
                &struct_defs,
            );
            let mut generated_defs = Vec::<FunctionDef>::new();
            let mut generated_sigs = HashMap::<String, FnSignature>::new();
            let mut mono_cache =
                HashMap::<(String, Vec<crate::def_semantics::MonoParamKey>), String>::new();
            let original_defs_snapshot = defs.clone();

            // Overload selection, specialization, and return inference form one
            // semantic fixed point. Rewriting a generated body can make its
            // return type concrete even when no overload name changed, and that
            // return type can decide an enclosing call on the next iteration.
            loop {
                let specialization_count_before = mono_cache.len();
                // Apply one specialization rule to every executable region. The
                // scope driver propagates only bindings that are visible at each
                // program point (init state, block-carried values, and event
                // parameters).
                let runtime_function_env = rewrite_executable_call_scopes(
                    &mut init,
                    &mut block_pre,
                    &mut sample,
                    &mut block_post,
                    &mut events,
                    &top_level_env,
                    |stmts, env| {
                        *env = crate::def_semantics::monomorphize_calls_in_stmts(
                            stmts,
                            env,
                            &mono_eligible,
                            &fn_signatures,
                            &original_defs_snapshot,
                            &generic_struct_template_names,
                            &struct_defs,
                            &mut generated_defs,
                            &mut generated_sigs,
                            &mut mono_cache,
                            &mut mono_return_types,
                            &mut errors,
                            crate::def_semantics::MonoOwnerContext {
                                type_params: &[],
                                proc_types: &proc_type_names,
                                return_type_env: &function_env_seed,
                            },
                        );
                    },
                );
                // Also walk def bodies (def-to-def mono calls).
                for def in &mut defs {
                    let env = def_call_type_env(
                        def,
                        &runtime_def_names,
                        &function_env_seed,
                        &runtime_function_env,
                    );
                    crate::def_semantics::monomorphize_calls_in_function(
                        def,
                        env,
                        &mono_eligible,
                        &fn_signatures,
                        &original_defs_snapshot,
                        &generic_struct_template_names,
                        &proc_type_names,
                        &struct_defs,
                        &mut generated_defs,
                        &mut generated_sigs,
                        &mut mono_cache,
                        &mut mono_return_types,
                        &mut errors,
                    );
                    if let Some(signature) = fn_signatures.get_mut(&def.name) {
                        signature.sync_defaults_from_def(def);
                    }
                }

                // Mono-rewrite generated defs' bodies (def-to-def mono calls).
                // E.g. quad.__onda_mono__g_f32 may call double(...) which also needs mono.
                // Loop until no new defs are generated.
                let mut processed_generated_defs = 0;
                loop {
                    if processed_generated_defs == generated_defs.len() {
                        break;
                    }
                    let first_unprocessed = processed_generated_defs;
                    processed_generated_defs = generated_defs.len();
                    let mut extra_defs = Vec::new();
                    let mut extra_sigs = HashMap::new();
                    register_generated_method_owners(&mut method_self_struct_internal, &mono_cache);
                    let mut combined_sigs = fn_signatures.clone();
                    for (name, signature) in &generated_sigs {
                        combined_sigs.insert(name.clone(), signature.clone());
                    }
                    for def in generated_defs.iter_mut().skip(first_unprocessed) {
                        rewrite_function_overloads(
                            def,
                            &function_env_seed,
                            crate::def_semantics::CallTypeContext {
                                return_types: &mono_return_types,
                                struct_defs: &struct_defs,
                            },
                            crate::def_semantics::OverloadOwnerContext {
                                defer_dependent_calls: true,
                            },
                            &overload_candidates,
                            &mut errors,
                        );
                        crate::def_semantics::monomorphize_calls_in_function(
                            def,
                            &function_env_seed,
                            &mono_eligible,
                            &combined_sigs,
                            &original_defs_snapshot,
                            &generic_struct_template_names,
                            &proc_type_names,
                            &struct_defs,
                            &mut extra_defs,
                            &mut extra_sigs,
                            &mut mono_cache,
                            &mut mono_return_types,
                            &mut errors,
                        );
                        if let Some(signature) = generated_sigs.get_mut(&def.name) {
                            signature.sync_defaults_from_def(def);
                        }
                    }
                    generated_defs.extend(extra_defs);
                    generated_sigs.extend(extra_sigs);
                }

                let overload_context = crate::def_semantics::CallTypeContext {
                    return_types: &mono_return_types,
                    struct_defs: &struct_defs,
                };
                let mut resolved_overloads = 0;
                let runtime_function_env = rewrite_executable_call_scopes(
                    &mut init,
                    &mut block_pre,
                    &mut sample,
                    &mut block_post,
                    &mut events,
                    &top_level_env,
                    |stmts, env| {
                        resolved_overloads +=
                            crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
                                stmts,
                                env,
                                overload_context,
                                crate::def_semantics::OverloadOwnerContext {
                                    defer_dependent_calls: true,
                                },
                                &overload_candidates,
                                &mut errors,
                            );
                    },
                );
                for def in &mut defs {
                    let env = def_call_type_env(
                        def,
                        &runtime_def_names,
                        &function_env_seed,
                        &runtime_function_env,
                    );
                    resolved_overloads += rewrite_function_overloads(
                        def,
                        env,
                        overload_context,
                        crate::def_semantics::OverloadOwnerContext {
                            defer_dependent_calls: true,
                        },
                        &overload_candidates,
                        &mut errors,
                    );
                }
                for def in &mut generated_defs {
                    resolved_overloads += rewrite_function_overloads(
                        def,
                        &function_env_seed,
                        overload_context,
                        crate::def_semantics::OverloadOwnerContext {
                            defer_dependent_calls: true,
                        },
                        &overload_candidates,
                        &mut errors,
                    );
                }
                let return_types_changed = crate::def_semantics::refresh_monomorphized_return_types(
                    &mut mono_return_types,
                    &defs,
                    &generated_defs,
                    &fn_signatures,
                    &generated_sigs,
                    &function_env_seed,
                    &struct_defs,
                );
                let specializations_changed = mono_cache.len() != specialization_count_before;
                if resolved_overloads == 0 && !return_types_changed && !specializations_changed {
                    break;
                }
            }
            // Register generated defs and signatures
            for sig in generated_sigs {
                fn_signatures.insert(sig.0, sig.1);
            }
            // Monomorphized method definitions retain the semantic owner of
            // their source method.  Keep that relationship explicit instead
            // of making later parameter inference reconstruct it from the
            // generated symbol spelling.
            register_generated_method_owners(&mut method_self_struct_internal, &mono_cache);
            defs.extend(generated_defs);

            // Shapes that intrinsically require monomorphization cannot be
            // lowered in their template form. Keep their source signatures,
            // however: a call that could not produce a specialization still
            // needs ordinary call-contract validation so diagnostics describe
            // the missing or incompatible argument instead of pretending the
            // declared function does not exist. Only concrete generated defs
            // survive into typed lowering.
            defs.retain(|d| !mandatory_mono.contains(&d.name));
        }
    }

    // Any calls left at their public overload name are genuinely
    // underconstrained after specialization reached a fixed point. Run one
    // strict pass to produce the normal ambiguity/no-match diagnostics while
    // keeping every scope on the same semantic type engine.
    let mut final_overload_return_types = HashMap::new();
    crate::def_semantics::refresh_monomorphized_return_types(
        &mut final_overload_return_types,
        &defs,
        &[],
        &fn_signatures,
        &HashMap::new(),
        &function_env_seed,
        &struct_defs,
    );
    let runtime_function_env = rewrite_executable_call_scopes(
        &mut init,
        &mut block_pre,
        &mut sample,
        &mut block_post,
        &mut events,
        &top_level_env,
        |stmts, env| {
            crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
                stmts,
                env,
                crate::def_semantics::CallTypeContext {
                    return_types: &final_overload_return_types,
                    struct_defs: &struct_defs,
                },
                crate::def_semantics::OverloadOwnerContext::default(),
                &overload_candidates,
                &mut errors,
            );
        },
    );
    for def in &mut defs {
        let is_unresolved_template = fn_signatures.get(&def.name).is_some_and(|signature| {
            crate::def_semantics::signature_has_dependent_call_types(
                signature,
                &generic_struct_template_names,
                &proc_type_names,
            )
        });
        let env = def_call_type_env(
            def,
            &runtime_def_names,
            &function_env_seed,
            &runtime_function_env,
        );
        rewrite_function_overloads(
            def,
            env,
            crate::def_semantics::CallTypeContext {
                return_types: &final_overload_return_types,
                struct_defs: &struct_defs,
            },
            crate::def_semantics::OverloadOwnerContext {
                defer_dependent_calls: is_unresolved_template,
            },
            &overload_candidates,
            &mut errors,
        );
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let input_names: HashSet<String> = ins.iter().cloned().collect();
    let output_names: HashSet<String> = outs.iter().cloned().collect();
    let control_output_names: HashSet<String> = control_outs.iter().cloned().collect();
    let all_output_names = output_names
        .union(&control_output_names)
        .cloned()
        .collect::<HashSet<_>>();
    let all_output_array_names = out_arrays
        .keys()
        .chain(control_out_arrays.keys())
        .cloned()
        .collect::<HashSet<_>>();
    let mut io_surface_array_names = in_arrays
        .keys()
        .chain(all_output_array_names.iter())
        .cloned()
        .collect::<HashSet<_>>();
    if ins_explicit && !ins.is_empty() {
        io_surface_array_names.insert("ins".to_owned());
    }
    if audio_outs_explicit && !outs.is_empty() {
        io_surface_array_names.insert("outs".to_owned());
    }
    if control_outs_explicit && !control_outs.is_empty() {
        io_surface_array_names.insert("kouts".to_owned());
    }
    let mut io_surface_names = input_names
        .union(&all_output_names)
        .cloned()
        .collect::<HashSet<_>>();
    io_surface_names.extend(io_surface_array_names.iter().cloned());
    let mut all_output_types = out_types.clone();
    all_output_types.extend(
        control_out_types
            .iter()
            .map(|(name, ty)| (name.clone(), *ty)),
    );
    let param_names: HashSet<String> = typed_params.iter().map(|p| p.name.clone()).collect();
    let def_return_types =
        infer_def_return_types(&defs, &fn_signatures, &function_env_seed, &struct_defs);
    for (name, return_type) in &def_return_types {
        if let Some(signature) = fn_signatures.get_mut(name) {
            signature.return_type = Some(return_type.clone());
        }
    }
    validate_def_return_types(
        &defs,
        &fn_signatures,
        &function_env_seed,
        &def_return_types,
        &struct_defs,
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    fn_signatures
        .entry(PROC_INDEX_CALL_SENTINEL.to_owned())
        .or_insert_with(|| internal_proc_index_call_signature(false));
    fn_signatures
        .entry(format!(
            "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
        ))
        .or_insert_with(|| internal_proc_index_call_signature(true));
    update_readonly_array_param_signatures(&defs, &mut fn_signatures);

    let mut state_scalars = HashMap::<String, PrimitiveType>::new();
    let mut declared_symbols = DeclaredSymbolMap::new();
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &input_names,
        &in_types,
        DeclaredScalarSymbolKind::Input,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &all_output_names,
        &all_output_types,
        DeclaredScalarSymbolKind::Output,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &param_names,
        &param_types,
        DeclaredScalarSymbolKind::Param,
    );
    for (fn_name, ret_ty) in &def_return_types {
        if let ReturnType::Scalar(scalar_ty) = ret_ty {
            insert_declared_symbol(
                &mut state_scalars,
                &mut declared_symbols,
                fn_name.clone(),
                DeclaredSymbolInfo::FunctionReturn { ty: *scalar_ty },
            );
        }
    }
    for buffer in &typed_buffers {
        let channels = match buffer.channels {
            TypedBufferChannels::Mono => BufferChannelInfo::Mono,
            TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(ch),
            TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
        };
        insert_declared_symbol(
            &mut state_scalars,
            &mut declared_symbols,
            buffer.name.clone(),
            DeclaredSymbolInfo::Buffer {
                elem_ty: buffer.elem_ty,
                channels,
                array_len: buffer.array_len,
                is_array: buffer.is_array,
            },
        );
    }
    let mut init_known_scalars = param_names.clone();
    init_known_scalars.extend(state_scalars.keys().cloned());
    init_known_scalars.insert(TOP_LEVEL_INIT_ALL_NAME.to_owned());
    let init_locals = HashSet::new();
    let mut init_local_aliases = LocalAliasTypes::new();
    init_local_aliases.insert(TOP_LEVEL_INIT_ALL_NAME.to_owned(), PrimitiveType::Bool);
    let mut init_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_data_aliases, &param_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &const_array_infos, false);

    let init_default_ty =
        resolve_init_default_ty(init_default_decl_ty.as_ref(), "top-level", &mut errors);
    let top_level_proc_symbols = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(proc_def) => Some(proc_def.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let no_proc_event_names = HashSet::<String>::new();
    let init_ctx = InitAnalysisCtx {
        context_label: "top-level",
        common: ScopeAnalysisCtx {
            policy: ScopePolicy::Init,
            input_names: &input_names,
            output_names: &all_output_names,
            output_array_names: &all_output_array_names,
            io_surface_names: &io_surface_names,
            io_surface_array_names: &io_surface_array_names,
            dynamic_param_array_names: &dynamic_param_array_names,
            param_names: &param_names,
            struct_defs: &struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types: &def_return_types,
            options,
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
            port_index_kins: None,
            proc_event_names: &no_proc_event_names,
        },
        init_default_ty,
        proc_resolution: None,
        top_level_proc_symbols: Some(&top_level_proc_symbols),
    };
    let mut init_st = InitAnalysisState::new(
        init_known_scalars,
        init_local_aliases,
        init_local_data_aliases,
        declared_symbols,
        state_scalars,
    );
    analyze_owner_init_stmts(&init, &init_ctx, &init_locals, &mut init_st, &mut errors);
    guard_pinned_initializers(&mut init, TOP_LEVEL_INIT_ALL_NAME);
    let InitAnalysisState {
        known_scalars: _init_known_scalars,
        local_aliases: _init_local_aliases,
        local_array_aliases: _init_local_data_aliases,
        mut declared_symbols,
        mut state_scalars,
        mut state_arrays,
        state_array_struct_roots,
        struct_instances,
        nested_proc_arrays,
        mut state_tuples,
        ..
    } = init_st;
    for (root, struct_name) in &struct_instances {
        extend_struct_field_integer_ranges(&mut runtime_ranges, root, struct_name, &struct_defs);
    }
    rewrite_integer_binding_ranges_in_list(&mut init, &runtime_ranges, options, &mut errors);
    rewrite_integer_binding_ranges_in_list(&mut block_pre, &runtime_ranges, options, &mut errors);
    rewrite_integer_binding_ranges_in_list(&mut sample, &runtime_ranges, options, &mut errors);
    rewrite_integer_binding_ranges_in_list(&mut block_post, &runtime_ranges, options, &mut errors);
    for event in &mut events {
        let inherited = integer_binding_ranges_outside_params(
            &runtime_ranges,
            event.params.iter().map(|param| &param.name),
        );
        rewrite_integer_binding_ranges_in_list(&mut event.body, &inherited, options, &mut errors);
    }
    let mut struct_array_field_ranges = HashMap::new();
    for (root, info) in &state_array_struct_roots {
        extend_struct_field_integer_ranges(
            &mut struct_array_field_ranges,
            root,
            &info.struct_name,
            &struct_defs,
        );
    }
    rewrite_indexed_integer_ranges_in_list(&mut init, &struct_array_field_ranges);
    rewrite_indexed_integer_ranges_in_list(&mut block_pre, &struct_array_field_ranges);
    rewrite_indexed_integer_ranges_in_list(&mut sample, &struct_array_field_ranges);
    rewrite_indexed_integer_ranges_in_list(&mut block_post, &struct_array_field_ranges);
    for event in &mut events {
        rewrite_indexed_integer_ranges_in_list(&mut event.body, &struct_array_field_ranges);
    }
    for (param_name, alias) in &param_aliases {
        if process_clamp_usage.dynamic_param_aliases.contains(alias) {
            let ty = param_types
                .get(param_name)
                .copied()
                .expect("range-clamped parameter must have a declared scalar type");
            state_scalars.insert(alias.clone(), ty);
        }
    }
    let control_out_array_slots = control_out_arrays
        .iter()
        .flat_map(|(name, info)| (0..info.len).map(move |idx| format!("{name}[{idx}]")))
        .collect::<HashSet<_>>();
    for name in &control_outs {
        if control_out_array_slots.contains(name) {
            continue;
        }
        let ty = *control_out_types.get(name).unwrap_or(&PrimitiveType::F32);
        state_scalars.insert(name.clone(), ty);
    }
    for (name, info) in &control_out_arrays {
        state_arrays.insert(name.clone(), info.len);
        insert_declared_symbol(
            &mut state_scalars,
            &mut declared_symbols,
            name.clone(),
            DeclaredSymbolInfo::DataArray {
                elem_ty: info.elem_ty,
            },
        );
    }
    let init_writable_roots = collect_runtime_state_roots(&state_scalars, &state_arrays);
    let empty_nested_proc_instances = HashMap::<String, ProcNestedState>::new();

    // Rewrite struct-array inline field sentinels (for example `data[0].field` and
    // `data[0].field[i]`) to flattened Index exprs before runtime analysis.
    rewrite_owner_struct_array_inline_fields(
        ExecutableOwnerBodies {
            init: &mut init,
            block_pre: &mut block_pre,
            sample: &mut sample,
            block_post: &mut block_post,
            events: &mut events,
        },
        &state_array_struct_roots,
        &struct_defs,
        &mut errors,
    );

    let sample_port_index_outs =
        uniform_port_index_info_from_names(audio_outs_explicit, &outs, &out_types);
    let block_port_index_outs = uniform_port_index_info_from_names(
        control_outs_explicit,
        &control_outs,
        &control_out_types,
    );
    let port_index_kins = if params_block_is_kins {
        port_index_params
    } else {
        None
    };

    let typed_events = coerce_typed_events(&events, true, "top-level", options, &mut errors);
    let analysis_plan_seeds = build_top_level_owner_analysis_plan_seeds(
        &param_names,
        &input_names,
        &output_names,
        &control_output_names,
        &state_scalars,
        &in_arrays,
        &out_arrays,
        &control_out_arrays,
        &param_arrays,
        &const_array_infos,
    );
    {
        let mut runtime_state = ExecutableOwnerRuntimeState {
            state_scalars: &mut state_scalars,
            declared_symbols: &declared_symbols,
            state_arrays: &state_arrays,
            state_array_struct_roots: &state_array_struct_roots,
            nested_proc_instances: &empty_nested_proc_instances,
            proc_array_roots: &nested_proc_arrays,
            struct_instances: &struct_instances,
            state_tuples: &mut state_tuples,
        };
        let mut runtime_plans = analysis_plan_seeds
            .runtime_scope_plans(
                RuntimeScopeBodies {
                    block_pre: &block_pre,
                    sample: &sample,
                    block_post: &block_post,
                },
                RuntimeScopePlanInputs {
                    sample_input_names: &input_names,
                    block_output_names: &control_output_names,
                    sample_output_names: &output_names,
                    output_array_names: &all_output_array_names,
                    io_surface_names: &io_surface_names,
                    io_surface_array_names: &io_surface_array_names,
                    dynamic_param_array_names: &dynamic_param_array_names,
                    param_names: &param_names,
                    struct_defs: &struct_defs,
                    fn_signatures: &fn_signatures,
                    fn_return_types: &def_return_types,
                    options,
                    port_index_ins,
                    block_port_index_outs,
                    sample_port_index_outs,
                    port_index_params,
                    port_index_kins,
                    registration_input_names: &input_names,
                    registration_output_names: &all_output_names,
                    registration_param_names: &param_names,
                    proc_event_names: &no_proc_event_names,
                },
            )
            .to_vec();
        let helper_plan = runtime_plans[0].clone();
        runtime_plans.extend(
            defs.iter()
                .filter(|def| runtime_def_names.contains(&def.name))
                .map(|def| {
                    let mut plan = RuntimeScopePlan {
                        stmts: &def.body,
                        ..helper_plan.clone()
                    };
                    for param in &def.params {
                        match param.ty.as_ref() {
                            Some(FnParamType::Primitive(ty)) => {
                                plan.runtime_known_scalars.insert(param.name.clone());
                                plan.runtime_local_aliases.insert(param.name.clone(), *ty);
                            }
                            Some(FnParamType::Array(Some(elem))) => {
                                plan.runtime_local_array_aliases.insert(
                                    param.name.clone(),
                                    LocalArrayAliasInfo {
                                        len: 1,
                                        static_len: None,
                                        elem_ty: *elem,
                                        elem_struct: None,
                                        writable: false,
                                    },
                                );
                            }
                            Some(FnParamType::SizedArray {
                                elem: Some(elem),
                                size,
                                ..
                            }) => {
                                let len =
                                    crate::def_semantics::const_positive_usize_for_call_type(size)
                                        .unwrap_or(1);
                                plan.runtime_local_array_aliases.insert(
                                    param.name.clone(),
                                    LocalArrayAliasInfo {
                                        len,
                                        static_len: Some(len),
                                        elem_ty: *elem,
                                        elem_struct: None,
                                        writable: false,
                                    },
                                );
                            }
                            _ => {
                                // Runtime-context defs are compiler generated. Any
                                // non-scalar payload shape is validated by its
                                // originating event/delegate declaration and by the
                                // ordinary function-call contract below.
                                plan.runtime_known_scalars.insert(param.name.clone());
                            }
                        }
                    }
                    plan
                }),
        );
        analyze_owner_runtime_scopes(&mut runtime_state, runtime_plans, &mut errors);

        analyze_owner_events(
            &runtime_state,
            analysis_plan_seeds.event_plan(
                runtime_state.state_scalars,
                EventPlanInputs {
                    typed_events: &typed_events,
                    init_writable_roots: &init_writable_roots,
                    input_names: &input_names,
                    output_names: &all_output_names,
                    output_array_names: &all_output_array_names,
                    io_surface_names: &io_surface_names,
                    io_surface_array_names: &io_surface_array_names,
                    dynamic_param_array_names: &dynamic_param_array_names,
                    param_names: &param_names,
                    validation_input_names: &input_names,
                    validation_output_names: &all_output_names,
                    struct_defs: &struct_defs,
                    fn_signatures: &fn_signatures,
                    fn_return_types: &def_return_types,
                    options,
                    port_index_ins: None,
                    port_index_outs: None,
                    port_index_params: None,
                    port_index_kins: None,
                    proc_event_names: &no_proc_event_names,
                },
            ),
            &mut errors,
        );
    }

    let mut block_exec = block_pre.clone();
    block_exec.extend(block_post.clone());
    let mut sample_and_event_exec = sample.clone();
    for event in &typed_events {
        sample_and_event_exec.extend(event.body.clone());
    }

    let current_declared_symbols = &declared_symbols;
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
    for (name, info) in &control_out_arrays {
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
    for (name, info) in &const_array_infos {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, len) in &state_arrays {
        let elem_ty = declared_symbol_scalar_type(current_declared_symbols, name)
            .unwrap_or(PrimitiveType::F32);
        inferred_array_bindings.insert(name.clone(), InferredArrayParam { elem_ty, len: *len });
    }
    let inferred_struct_array_roots = state_array_struct_roots
        .iter()
        .map(|(name, info)| (name.clone(), info.struct_name.clone()))
        .collect::<HashMap<_, _>>();
    let mut inferred_proc_array_roots = nested_proc_arrays
        .iter()
        .filter_map(|(name, info)| {
            let size_context = format!("top-level processor array '{}' size", name);
            let len = eval_data_size_expr(&info.size_expr, options, &size_context, &mut errors)?;
            Some((
                name.clone(),
                InferredProcArrayParam {
                    proc_name: info.proc_name.clone(),
                    len,
                },
            ))
        })
        .collect::<HashMap<_, _>>();
    for (name, info) in &state_array_struct_roots {
        if !proc_api.contains_key(&info.struct_name) {
            continue;
        }
        inferred_proc_array_roots
            .entry(name.clone())
            .or_insert_with(|| InferredProcArrayParam {
                proc_name: info.struct_name.clone(),
                len: info.len,
            });
    }

    let reachable_def_names =
        collect_reachable_def_names(&init, &block_exec, &sample_and_event_exec, &defs);
    let defs_requiring_param_inference = defs
        .iter()
        .filter(|def| {
            reachable_def_names.contains(&def.name)
                || def_has_concrete_param_contract(def, &method_self_struct_internal, &struct_defs)
        })
        .cloned()
        .collect::<Vec<_>>();
    let (inferred_def_params, synthesized_struct_defs) = infer_def_param_kinds(
        &defs_requiring_param_inference,
        &init,
        &block_exec,
        &sample_and_event_exec,
        &struct_instances,
        &inferred_struct_array_roots,
        &inferred_proc_array_roots,
        &inferred_array_bindings,
        &typed_buffers
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    InferredBufferBinding {
                        candidates: vec![InferredBufferParam {
                            elem_ty: b.elem_ty,
                            channels: b.channels.clone(),
                        }],
                        is_array: b.is_array,
                    },
                )
            })
            .collect::<HashMap<_, _>>(),
        &fn_signatures,
        &method_self_struct_internal,
        &struct_defs,
        &proc_type_names,
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
    let mut def_scalar_local_types = HashMap::<String, LocalAliasTypes>::new();
    for def in defs.iter_mut().filter(|def| {
        !runtime_def_names.contains(&def.name)
            && (reachable_def_names.contains(&def.name)
                || def_has_concrete_param_contract(def, &method_self_struct_internal, &struct_defs))
    }) {
        let def_error_start = errors.len();
        let def_param_names = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_io_surface_names = io_surface_names.clone();
        let mut def_io_surface_array_names = io_surface_array_names.clone();
        for param in &def_param_names {
            def_io_surface_names.remove(param);
            def_io_surface_array_names.remove(param);
        }
        let fn_known = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_state_scalars = HashMap::<String, PrimitiveType>::new();
        let mut def_declared_symbols = declared_symbols
            .iter()
            .filter_map(|(name, info)| match info {
                DeclaredSymbolInfo::FunctionReturn { .. } => Some((name.clone(), info.clone())),
                _ => None,
            })
            .collect::<DeclaredSymbolMap>();
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
                    | FnParamType::BufferArray { .. }
                    | FnParamType::Array(_)
                    | FnParamType::ArrayGeneric(_)
                    | FnParamType::SizedArray { .. }
                    | FnParamType::BareBuffer
                    | FnParamType::Tuple(_) => None,
                });
            if let Some(param_ty) = explicit_prim {
                def_state_scalars.insert(param.name.clone(), param_ty);
            } else {
                def_state_scalars.remove(&param.name);
            }
            if let Some(FnParamType::Tuple(elem_types)) = fn_sig
                .and_then(|signature| signature.param_types.get(idx))
                .and_then(Option::as_ref)
            {
                def_state_scalars.extend(elem_types.iter().enumerate().map(
                    |(element_index, elem_ty)| {
                        (format!("{}[{element_index}]", param.name), *elem_ty)
                    },
                ));
            }
        }
        let fn_locals = HashSet::new();
        let fn_local_aliases = LocalAliasTypes::new();
        let mut fn_local_data_aliases = HashMap::new();
        seed_top_level_array_aliases(&mut fn_local_data_aliases, &const_array_infos, false);
        let fn_local_proc_aliases = HashMap::new();
        let param_names_vec = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        let param_structs = inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let mut param_struct_arrays = def
            .params
            .iter()
            .filter_map(|param| match param.ty.as_ref() {
                Some(FnParamType::ArrayGeneric(struct_name))
                    if def_struct_defs.contains_key(struct_name)
                        && !def.type_params.contains(struct_name) =>
                {
                    Some((param.name.clone(), struct_name.clone()))
                }
                Some(FnParamType::SizedArray {
                    generic_name: Some(struct_name),
                    ..
                }) if def_struct_defs.contains_key(struct_name)
                    && !def.type_params.contains(struct_name) =>
                {
                    Some((param.name.clone(), struct_name.clone()))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        for (name, struct_name) in inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default()
        {
            param_struct_arrays.entry(name).or_insert(struct_name);
        }
        let param_buffers = inferred_def_params
            .get(&def.name)
            .map(|k| param_buffer_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_proc_arrays = inferred_def_params
            .get(&def.name)
            .map(|k| param_proc_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        // Processor state structs also appear in `def_struct_defs`; the final
        // inferred parameter kind is authoritative when a nominal array type
        // names a processor. Never register the same parameter as both ABIs.
        param_struct_arrays.retain(|name, _| !param_proc_arrays.contains_key(name));
        let param_arrays = inferred_def_params
            .get(&def.name)
            .map(|k| param_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_array_static_lens = def
            .params
            .iter()
            .filter_map(|param| match param.ty.as_ref() {
                Some(FnParamType::SizedArray { size, .. }) => {
                    crate::def_semantics::const_positive_usize_for_call_type(size)
                        .map(|len| (param.name.as_str(), len))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        for (param_name, elem_ty) in &param_arrays {
            fn_local_data_aliases.insert(
                param_name.clone(),
                LocalArrayAliasInfo {
                    len: 1,
                    static_len: param_array_static_lens.get(param_name.as_str()).copied(),
                    elem_ty: *elem_ty,
                    elem_struct: None,
                    writable: true,
                },
            );
        }
        for (param_name, proc_info) in &param_proc_arrays {
            let len = match &proc_info.size_expr {
                Expr::Int { value, .. } if *value >= 0 => *value as usize,
                _ => 1,
            };
            if let Some(api) = proc_api.get(&proc_info.proc_name) {
                for param in api.params.values().filter(|param| !param.private) {
                    fn_local_data_aliases.insert(
                        format!("{param_name}.{}", param.name),
                        LocalArrayAliasInfo {
                            len,
                            static_len: Some(len),
                            elem_ty: param.ty,
                            elem_struct: None,
                            writable: true,
                        },
                    );
                }
                if let Some(fields) = def_struct_defs.get(&proc_info.proc_name) {
                    for output in &api.outputs.names {
                        let Some(field) = fields.iter().find(|field| field.name == *output) else {
                            continue;
                        };
                        let TypedFieldType::Scalar(elem_ty) = field.ty else {
                            continue;
                        };
                        fn_local_data_aliases.insert(
                            format!("{param_name}.{output}"),
                            LocalArrayAliasInfo {
                                len,
                                static_len: Some(len),
                                elem_ty,
                                elem_struct: None,
                                writable: false,
                            },
                        );
                    }
                }
            }
            let has_block = proc_api
                .get(&proc_info.proc_name)
                .map(|api| api.has_block)
                .unwrap_or(false);
            if !has_block {
                continue;
            }
            let active_symbol = runtime_proc_array_active_symbol(param_name);
            fn_local_data_aliases.insert(
                active_symbol.clone(),
                LocalArrayAliasInfo {
                    len,
                    static_len: Some(len),
                    elem_ty: PrimitiveType::Bool,
                    elem_struct: None,
                    writable: true,
                },
            );
            insert_declared_symbol(
                &mut def_state_scalars,
                &mut def_declared_symbols,
                active_symbol,
                DeclaredSymbolInfo::DataArray {
                    elem_ty: PrimitiveType::Bool,
                },
            );
        }
        let mut def_param_array_struct_roots = HashMap::<String, ArrayStructRootInfo>::new();
        for (param_name, struct_name) in &param_struct_arrays {
            let declared_len = def
                .params
                .iter()
                .find(|param| param.name == *param_name)
                .and_then(|param| match param.ty.as_ref() {
                    Some(FnParamType::SizedArray { size, .. }) => {
                        crate::def_semantics::const_positive_usize_for_call_type(size)
                    }
                    _ => None,
                });
            register_struct_array_param_bindings(
                param_name,
                struct_name,
                declared_len,
                &def_struct_defs,
                &mut def_declared_symbols,
                &mut fn_local_data_aliases,
                &mut def_param_array_struct_roots,
                &mut errors,
            );
        }
        for (param_name, (elem_ty, channels, array_len, is_array)) in &param_buffers {
            let channel_info = match channels {
                TypedBufferChannels::Mono => BufferChannelInfo::Mono,
                TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(*ch),
                TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
            };
            insert_declared_symbol(
                &mut def_state_scalars,
                &mut def_declared_symbols,
                param_name.clone(),
                DeclaredSymbolInfo::Buffer {
                    elem_ty: *elem_ty,
                    channels: channel_info,
                    array_len: *array_len,
                    is_array: *is_array,
                },
            );
        }
        let mut def_proc_vars = HashMap::<String, ProcCallInstance>::new();
        let mut def_proc_array_slots = HashMap::<String, Vec<String>>::new();
        let mut def_proc_block_active_symbols = HashMap::<String, String>::new();
        if let Some(kinds) = inferred_def_params.get(&def.name) {
            for (param_name, kind) in def.params.iter().map(|p| &p.name).zip(kinds.iter()) {
                match kind {
                    TypedFnParam::Struct { struct_name } => {
                        if let Some(shape) = lowering_shapes.get(struct_name) {
                            for (base, slots) in &shape.nested_proc_array_slots {
                                def_proc_array_slots
                                    .entry(base.clone())
                                    .or_insert_with(|| slots.clone());
                                let prefixed_base = format!("{param_name}.{base}");
                                let prefixed_slots = slots
                                    .iter()
                                    .map(|slot| format!("{param_name}.{slot}"))
                                    .collect::<Vec<_>>();
                                def_proc_array_slots
                                    .entry(prefixed_base.clone())
                                    .or_insert(prefixed_slots);
                                if let Some(active_field) =
                                    shape.nested_proc_array_active_fields.get(base)
                                {
                                    def_proc_block_active_symbols
                                        .entry(prefixed_base)
                                        .or_insert_with(|| format!("{param_name}.{active_field}"));
                                }
                                for slot in slots {
                                    if let Some(nested) = shape.state.nested_procs.get(slot) {
                                        def_proc_vars.entry(slot.clone()).or_insert(
                                            ProcCallInstance {
                                                proc_name: nested.proc_name.clone(),
                                                buffer_args: Vec::new(),
                                                delegate_context_args: Vec::new(),
                                                routes_owner_delegates: false,
                                            },
                                        );
                                        let prefixed_slot = format!("{param_name}.{slot}");
                                        def_proc_vars.entry(prefixed_slot).or_insert(
                                            ProcCallInstance {
                                                proc_name: nested.proc_name.clone(),
                                                buffer_args: Vec::new(),
                                                delegate_context_args: Vec::new(),
                                                routes_owner_delegates: false,
                                            },
                                        );
                                    }
                                }
                            }
                            for (instance_name, nested) in &shape.state.nested_procs {
                                def_proc_vars.entry(instance_name.clone()).or_insert(
                                    ProcCallInstance {
                                        proc_name: nested.proc_name.clone(),
                                        buffer_args: Vec::new(),
                                        delegate_context_args: Vec::new(),
                                        routes_owner_delegates: false,
                                    },
                                );
                                let prefixed_instance = format!("{param_name}.{instance_name}");
                                def_proc_vars.entry(prefixed_instance).or_insert(
                                    ProcCallInstance {
                                        proc_name: nested.proc_name.clone(),
                                        buffer_args: Vec::new(),
                                        delegate_context_args: Vec::new(),
                                        routes_owner_delegates: false,
                                    },
                                );
                            }
                        }
                        if proc_api.contains_key(struct_name) {
                            def_proc_vars.insert(
                                param_name.clone(),
                                ProcCallInstance {
                                    proc_name: struct_name.clone(),
                                    buffer_args: Vec::new(),
                                    delegate_context_args: Vec::new(),
                                    routes_owner_delegates: false,
                                },
                            );
                        }
                    }
                    TypedFnParam::ProcArray { proc_name, len } => {
                        let slot_names = (0..*len)
                            .map(|idx| format!("{param_name}.__onda_proc_array_slot_{idx}"))
                            .collect::<Vec<_>>();
                        for slot_name in &slot_names {
                            def_proc_vars.insert(
                                slot_name.clone(),
                                ProcCallInstance {
                                    proc_name: proc_name.clone(),
                                    buffer_args: Vec::new(),
                                    delegate_context_args: Vec::new(),
                                    routes_owner_delegates: false,
                                },
                            );
                        }
                        def_proc_array_slots.insert(param_name.clone(), slot_names);
                        def_proc_block_active_symbols.insert(
                            param_name.clone(),
                            runtime_proc_array_active_symbol(param_name),
                        );
                    }
                    _ => {}
                }
            }
        }
        let mut def_proc_array_roots = param_proc_arrays.clone();
        for (base, slots) in &def_proc_array_slots {
            let Some(proc_name) = slots
                .first()
                .and_then(|slot| def_proc_vars.get(slot))
                .map(|instance| instance.proc_name.clone())
            else {
                continue;
            };
            def_proc_array_roots
                .entry(base.clone())
                .or_insert_with(|| ProcNestedArrayState {
                    proc_name,
                    size_expr: Expr::int(slots.len() as i64),
                });
        }
        if (!def_proc_vars.is_empty() || !def_proc_array_slots.is_empty())
            && !def.name.contains(".__onda_proc_")
        {
            rewrite_proc_array_param_field_reads(&mut def.body, &param_proc_arrays, &proc_api);
            rewrite_proc_calls_in_stmts(
                &mut def.body,
                &def_proc_vars,
                &def_proc_array_slots,
                &proc_api,
                &mut errors,
            );
            let mut rewritten_def_body = Vec::<Stmt>::new();
            for stmt in std::mem::take(&mut def.body) {
                rewritten_def_body.extend(rewrite_stmt_for_def_proc_block_guards(
                    stmt,
                    &proc_api,
                    &def_proc_block_active_symbols,
                ));
            }
            def.body = rewritten_def_body;
        }
        let resolved_scalar_locals = RefCell::new(LocalAliasTypes::new());
        let def_ctx = DefStmtAnalysisCtx {
            common: ScopeAnalysisCtx {
                policy: ScopePolicy::Def,
                input_names: &def_global_inputs,
                output_names: &def_global_outputs,
                output_array_names: &all_output_array_names,
                io_surface_names: &def_io_surface_names,
                io_surface_array_names: &def_io_surface_array_names,
                dynamic_param_array_names: &dynamic_param_array_names,
                param_names: &def_global_params,
                struct_defs: &def_struct_defs,
                fn_signatures: &fn_signatures,
                fn_return_types: &def_return_types,
                options,
                port_index_ins: None,
                port_index_outs: None,
                port_index_params: None,
                port_index_kins: None,
                proc_event_names: &no_proc_event_names,
            },
            locals: &fn_locals,
            declared_symbols: &def_declared_symbols,
            param_structs: &param_structs,
            struct_array_roots: &def_param_array_struct_roots,
            proc_array_roots: &def_proc_array_roots,
            state_scalars: &def_state_scalars,
            resolved_scalar_locals: &resolved_scalar_locals,
        };
        let mut def_state = DefStmtAnalysisState::from_parts(
            fn_known,
            fn_local_aliases,
            fn_local_data_aliases,
            fn_local_proc_aliases,
        );
        rewrite_struct_array_inline_field_stmts(
            &mut def.body,
            &def_param_array_struct_roots,
            &def_struct_defs,
            &mut errors,
        );
        // Tuple parameters are mutable local copies. Seed both their arity and
        // component types so reassignment preserves the declared target types.
        if let Some(kinds) = inferred_def_params.get(&def.name) {
            for (param, kind) in def.params.iter().zip(kinds.iter()) {
                if let TypedFnParam::Tuple { elem_tys } = kind {
                    set_tracked_tuple_types(
                        &mut def_state.tuple_vars,
                        &mut def_state.local_aliases,
                        &param.name,
                        elem_tys,
                    );
                }
            }
        }
        analyze_def_stmt_list(&def.body, def_ctx, &mut def_state, 0, &mut errors);
        if let Some((internal_source_name, _)) = def.name.split_once(".__onda_mono") {
            let source_name = fn_signatures
                .get(&def.name)
                .and_then(|signature| signature.display_name.as_deref())
                .unwrap_or(internal_source_name);
            for diagnostic in &mut errors[def_error_start..] {
                diagnostic.message = format!(
                    "while checking specialization of '{source_name}': {}",
                    diagnostic.message
                );
            }
        }
        def_scalar_local_types.insert(def.name.clone(), resolved_scalar_locals.into_inner());
    }

    if errors.is_empty() {
        let mut sorted_state = state_scalars.keys().cloned().collect::<Vec<_>>();
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
                let elem_ty = declared_symbol_scalar_type(current_declared_symbols, &name)
                    .unwrap_or(PrimitiveType::F32);
                TypedArrayVar { name, len, elem_ty }
            })
            .collect::<Vec<_>>();
        typed_data.sort_by(|a, b| a.name.cmp(&b.name));
        let pinned_state_roots = sorted_state
            .iter()
            .chain(typed_data.iter().map(|array| &array.name))
            .filter(|name| path_or_ancestor_is_declared(name, &pinned_state_roots))
            .cloned()
            .collect::<HashSet<_>>();
        compiler_owned_state_roots.extend(
            sorted_state
                .iter()
                .chain(typed_data.iter().map(|array| &array.name))
                .filter(|name| {
                    crate::internal_names::is_reserved_internal_identifier(runtime_symbol_root(
                        name,
                    ))
                })
                .cloned(),
        );
        let compiler_owned_state_roots = sorted_state
            .iter()
            .chain(typed_data.iter().map(|array| &array.name))
            .filter(|name| path_or_ancestor_is_declared(name, &compiler_owned_state_roots))
            .cloned()
            .collect::<HashSet<_>>();
        let compiler_scratch_state_roots = sorted_state
            .iter()
            .chain(typed_data.iter().map(|array| &array.name))
            .filter(|name| path_or_ancestor_is_declared(name, &compiler_scratch_state_roots))
            .cloned()
            .collect::<HashSet<_>>();
        let mut typed_data_roots = state_array_struct_roots
            .into_iter()
            .map(|(name, info)| TypedArrayStructRoot {
                name,
                struct_name: info.struct_name,
                len: info.len,
            })
            .collect::<Vec<_>>();
        typed_data_roots.sort_by(|a, b| a.name.cmp(&b.name));

        let mut typed_nested_proc_arrays = Vec::new();
        let mut proc_names = lowering_shapes.keys().cloned().collect::<Vec<_>>();
        proc_names.sort();
        for owner_struct in proc_names {
            collect_typed_nested_proc_arrays(
                &owner_struct,
                &owner_struct,
                "",
                &lowering_shapes,
                &mut HashSet::new(),
                &mut typed_nested_proc_arrays,
            );
        }
        typed_nested_proc_arrays.sort_by(|lhs, rhs| {
            (&lhs.owner_struct, &lhs.field_name).cmp(&(&rhs.owner_struct, &rhs.field_name))
        });

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
        let aggregate_layouts = match AggregateLayoutTable::build(&typed_structs) {
            Ok(layouts) => layouts,
            Err(layout_errors) => {
                errors.extend(
                    layout_errors
                        .into_iter()
                        .map(|error| Diagnostic::semantic(error.to_string(), 0, 0)),
                );
                AggregateLayoutTable::default()
            }
        };

        // Specialization and nested-state flattening can create additional
        // generated defs after the first range rewrite. Reapply the storage
        // contract to every final body before it crosses the TypedProgram
        // boundary; declarations make this pass idempotent by carrying
        // canonical inclusive bounds after their first rewrite.
        for def in &mut defs {
            let inherited = def
                .name
                .split_once(".__onda_proc_")
                .filter(|(_, generated_suffix)| *generated_suffix != "init")
                .and_then(|(owner, _)| proc_state_ranges.get(owner))
                .map(proc_integer_binding_range_aliases)
                .unwrap_or_else(|| runtime_ranges.clone());
            let inherited = integer_binding_ranges_outside_params(
                &inherited,
                def.params.iter().map(|param| &param.name),
            );
            let mut inherited = inherited;
            if let Some(param_kinds) = inferred_def_params.get(&def.name) {
                inherited.extend(struct_param_integer_ranges(
                    def,
                    param_kinds,
                    &def_struct_defs,
                ));
                let indexed_ranges =
                    struct_array_param_integer_ranges(def, param_kinds, &def_struct_defs);
                rewrite_indexed_integer_ranges_in_list(&mut def.body, &indexed_ranges);
            }
            rewrite_integer_binding_ranges_in_list(&mut def.body, &inherited, options, &mut errors);
        }

        reject_non_sample_proc_operator_calls(
            &init,
            &block_pre,
            &sample,
            &block_post,
            &typed_events,
            &defs,
            &proc_api,
            &lowering_shapes,
            &mut errors,
        );
        if !errors.is_empty() {
            return Err(errors);
        }

        let def_integer_range_params = defs
            .iter()
            .filter_map(|def| {
                let mut ranges = HashMap::new();
                if let Some((owner, _)) = def.name.split_once(".__onda_proc_") {
                    if let Some(proc_ranges) = proc_state_ranges.get(owner) {
                        ranges.extend(proc_ranges.iter().filter_map(|(name, range)| {
                            typed_integer_range(range).map(|range| (name.clone(), range))
                        }));
                    }
                }
                if let Some(param_kinds) = inferred_def_params.get(&def.name) {
                    ranges.extend(
                        struct_param_integer_ranges(def, param_kinds, &def_struct_defs)
                            .into_iter()
                            .filter_map(|(name, range)| {
                                typed_integer_range(&range).map(|range| (name, range))
                            }),
                    );
                }
                (!ranges.is_empty()).then(|| (def.name.clone(), ranges))
            })
            .collect::<HashMap<_, _>>();

        let all_typed_defs = defs
            .into_iter()
            .map(|d| {
                let param_kinds = inferred_def_params
                    .get(&d.name)
                    .cloned()
                    .unwrap_or_else(|| vec![TypedFnParam::Scalar { ty: None }; d.params.len()]);
                let readonly_array_params = fn_signatures
                    .get(&d.name)
                    .map(|signature| signature.readonly_array_params.clone())
                    .unwrap_or_default();
                TypedFunction {
                    runtime_context: runtime_def_names.contains(&d.name),
                    publishes_print: statements_publish_print(&d.body),
                    method_of: method_self_struct_internal.get(&d.name).cloned(),
                    type_params: d.type_params.clone(),
                    param_defaults: d.params.iter().map(|p| p.default.clone()).collect(),
                    param_kinds,
                    readonly_array_params,
                    integer_range_params: def_integer_range_params
                        .get(&d.name)
                        .cloned()
                        .unwrap_or_default(),
                    return_ty: def_return_types
                        .get(&d.name)
                        .cloned()
                        .unwrap_or(ReturnType::Scalar(PrimitiveType::F32)),
                    returns_value: statements_return_value(&d.body),
                    local_scalar_types: def_scalar_local_types.remove(&d.name).unwrap_or_default(),
                    name: d.name,
                    params: d.params.into_iter().map(|p| p.name).collect(),
                    body: d.body,
                }
            })
            .collect::<Vec<_>>();
        reject_recursive_runtime_defs(&all_typed_defs, &mut errors);
        if !errors.is_empty() {
            return Err(errors);
        }
        inject_sample_def_owner_proc_block_hooks(
            &sample,
            &mut block_pre,
            &mut block_post,
            &all_typed_defs,
            &proc_api,
            &top_level_proc_rewrite.global_proc_instances,
            &top_level_proc_rewrite.global_proc_array_slots,
            &mut errors,
        );
        let reachable_defs = collect_reachable_typed_def_names(
            &init,
            &block_pre,
            &sample,
            &block_post,
            &typed_events,
            &all_typed_defs,
        );
        let typed_defs = all_typed_defs
            .into_iter()
            .filter(|def| reachable_defs.contains(&def.name))
            .collect::<Vec<_>>();
        let mut proc_instance_oversample_factors = proc_instance_oversample_factors;
        if sample_oversample_factor > 1 {
            let defs_by_name = typed_defs
                .iter()
                .map(|def| (def.name.clone(), def))
                .collect::<HashMap<_, _>>();
            collect_def_proc_arg_oversample_factors_from_stmts(
                &sample,
                sample_oversample_factor,
                &defs_by_name,
                &top_level_proc_rewrite,
                &proc_api,
                &mut proc_instance_oversample_factors,
                &mut errors,
            );
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let interface_views = match resolve_interface_views(
            ins_explicit,
            &ins,
            &in_types,
            &in_arrays,
            audio_outs_explicit,
            &outs,
            &out_types,
            &out_arrays,
            control_outs_explicit,
            &control_outs,
            &control_out_types,
            &control_out_arrays,
            params_explicit,
            &typed_params
                .iter()
                .map(|param| param.name.clone())
                .collect::<Vec<_>>(),
            &param_types,
            &param_arrays,
        ) {
            Ok(views) => views,
            Err(message) => {
                push_semantic(DiagCtx::default(), &mut errors, message);
                return Err(errors);
            }
        };
        let param_range_state_aliases = param_aliases.clone();
        let dynamic_input_range_aliases: HashMap<String, String> = input_aliases
            .into_iter()
            .filter(|(_, alias)| process_clamp_usage.dynamic_input_aliases.contains(alias))
            .collect();
        let dynamic_param_range_aliases: HashMap<String, String> = param_aliases
            .into_iter()
            .filter(|(_, alias)| process_clamp_usage.dynamic_param_aliases.contains(alias))
            .collect();
        let mut state_integer_ranges = sorted_state
            .iter()
            .filter_map(|name| {
                runtime_ranges
                    .get(name)
                    .or_else(|| init_ranges.get(name))
                    .and_then(typed_integer_range)
                    .map(|range| (name.clone(), range))
            })
            .collect::<HashMap<_, _>>();
        for (root, struct_name) in &struct_instances {
            let Some(fields) = struct_defs.get(struct_name) else {
                continue;
            };
            for field in fields {
                let flat_name = format!("{root}.{}", field.name);
                if !state_scalars.contains_key(&flat_name) {
                    continue;
                }
                if let Some(range) = &field.integer_range {
                    state_integer_ranges.insert(flat_name, *range);
                }
            }
        }
        for (source, alias) in &param_range_state_aliases {
            let Some(ty @ (PrimitiveType::I32 | PrimitiveType::I64)) =
                param_types.get(source).copied()
            else {
                continue;
            };
            let Some(range) = param_ranges.get(source) else {
                continue;
            };
            let exact_integer = |value| match value {
                TypedConstValue::I32(value) => Some(i64::from(value)),
                TypedConstValue::I64(value) => Some(value),
                _ => None,
            };
            let Some((min, max)) = exact_integer(range.min).zip(exact_integer(range.max)) else {
                continue;
            };
            if ty == PrimitiveType::I32
                && (i32::try_from(min).is_err() || i32::try_from(max).is_err())
            {
                continue;
            }
            state_integer_ranges.insert(
                alias.clone(),
                TypedIntegerRange {
                    ty,
                    min,
                    max,
                    wrap: false,
                },
            );
        }

        Ok(TypedProgram {
            analysis_options: options,
            ins,
            outs,
            control_outs,
            in_types,
            out_types,
            control_out_types,
            param_types,
            state_integer_ranges,
            pinned_state_roots,
            compiler_owned_state_roots,
            compiler_scratch_state_roots,
            in_defaults,
            in_ranges,
            dynamic_input_range_aliases,
            dynamic_param_range_aliases,
            in_arrays,
            out_arrays,
            control_out_arrays,
            param_arrays,
            interface_views,
            const_arrays,
            params: typed_params,
            buffers: typed_buffers,
            structs: typed_structs,
            aggregate_layouts,
            defs: typed_defs,
            events: typed_events,
            delegates: typed_delegates,
            def_sample_oversample_factors,
            proc_step_oversample_meta,
            proc_instance_oversample_factors,
            init,
            block_pre,
            sample_oversample_factor,
            sample,
            block_post,
            state_vars: sorted_state,
            state_types,
            state_tuples,
            array_vars: typed_data,
            array_struct_roots: typed_data_roots,
            nested_proc_arrays: typed_nested_proc_arrays,
            ins_explicit,
            audio_outs_explicit,
            control_outs_explicit,
            outs_explicit,
            params_explicit,
        })
    } else {
        Err(errors)
    }
}
