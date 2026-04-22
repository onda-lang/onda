use std::collections::{HashMap, HashSet};

use crate::*;

pub(crate) struct ExecutableOwnerBodies<'a> {
    pub(crate) init: &'a mut Vec<Stmt>,
    pub(crate) block_pre: &'a mut Vec<Stmt>,
    pub(crate) sample: &'a mut Vec<Stmt>,
    pub(crate) block_post: &'a mut Vec<Stmt>,
    pub(crate) events: &'a mut Vec<EventDef>,
}

pub(crate) struct ExecutableOwnerRuntimeState<'a> {
    pub(crate) state_scalars: &'a mut HashMap<String, PrimitiveType>,
    pub(crate) declared_symbols: &'a DeclaredSymbolMap,
    pub(crate) state_arrays: &'a HashMap<String, usize>,
    pub(crate) state_array_struct_roots: &'a HashMap<String, ArrayStructRootInfo>,
    pub(crate) nested_proc_instances: &'a HashMap<String, ProcNestedState>,
    pub(crate) proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
    pub(crate) struct_instances: &'a HashMap<String, String>,
    pub(crate) state_tuples: &'a HashMap<String, Vec<PrimitiveType>>,
}

pub(crate) struct RuntimeScopePlan<'a> {
    pub(crate) stmts: &'a [Stmt],
    pub(crate) common: ScopeAnalysisCtx<'a>,
    pub(crate) registration_mode: RuntimeRegistrationMode,
    pub(crate) registration_input_names: &'a HashSet<String>,
    pub(crate) registration_output_names: &'a HashSet<String>,
    pub(crate) registration_param_names: &'a HashSet<String>,
    pub(crate) runtime_locals: &'a HashSet<String>,
    pub(crate) runtime_known_scalars: HashSet<String>,
    pub(crate) runtime_local_aliases: LocalAliasTypes,
    pub(crate) runtime_local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    pub(crate) runtime_forbidden_assign_names: &'a HashSet<String>,
}

pub(crate) struct RuntimeScopeBodies<'a> {
    pub(crate) block_pre: &'a [Stmt],
    pub(crate) sample: &'a [Stmt],
    pub(crate) block_post: &'a [Stmt],
}

pub(crate) struct RuntimeScopePlanInputs<'a> {
    pub(crate) sample_input_names: &'a HashSet<String>,
    pub(crate) sample_output_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) fn_return_types: &'a HashMap<String, ReturnType>,
    pub(crate) options: AnalysisOptions,
    pub(crate) port_index_ins: Option<PortIndexInfo>,
    pub(crate) port_index_outs: Option<PortIndexInfo>,
    pub(crate) port_index_params: Option<PortIndexInfo>,
    pub(crate) registration_input_names: &'a HashSet<String>,
    pub(crate) registration_output_names: &'a HashSet<String>,
    pub(crate) registration_param_names: &'a HashSet<String>,
}

pub(crate) struct EventPlanInputs<'a> {
    pub(crate) typed_events: &'a [TypedEvent],
    pub(crate) init_writable_roots: &'a HashSet<String>,
    pub(crate) input_names: &'a HashSet<String>,
    pub(crate) output_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
    pub(crate) validation_input_names: &'a HashSet<String>,
    pub(crate) validation_output_names: &'a HashSet<String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) fn_return_types: &'a HashMap<String, ReturnType>,
    pub(crate) options: AnalysisOptions,
    pub(crate) port_index_ins: Option<PortIndexInfo>,
    pub(crate) port_index_outs: Option<PortIndexInfo>,
    pub(crate) port_index_params: Option<PortIndexInfo>,
}

pub(crate) struct EventAnalysisPlan<'a> {
    pub(crate) typed_events: &'a [TypedEvent],
    pub(crate) event_known_scalars_seed: HashSet<String>,
    pub(crate) event_array_alias_seed: HashMap<String, LocalArrayAliasInfo>,
    pub(crate) event_immutable_param_seed: HashSet<String>,
    pub(crate) init_writable_roots: &'a HashSet<String>,
    pub(crate) validation_input_names: &'a HashSet<String>,
    pub(crate) validation_output_names: &'a HashSet<String>,
    pub(crate) common: ScopeAnalysisCtx<'a>,
}

struct ExecutableOwnerRuntimePlanSeeds {
    block_locals: HashSet<String>,
    sample_locals: HashSet<String>,
    block_forbidden_assign_names: HashSet<String>,
    sample_forbidden_assign_names: HashSet<String>,
    block_pre_known_scalars: HashSet<String>,
    sample_known_scalars: HashSet<String>,
    block_post_known_scalars: HashSet<String>,
    block_pre_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    sample_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    block_post_array_aliases: HashMap<String, LocalArrayAliasInfo>,
}

struct ExecutableOwnerEventPlanSeed {
    known_scalar_base: HashSet<String>,
    known_scalar_extras: HashSet<String>,
    array_alias_seed: HashMap<String, LocalArrayAliasInfo>,
    immutable_param_seed: HashSet<String>,
}

pub(crate) struct ExecutableOwnerAnalysisPlanSeeds {
    empty_inputs: HashSet<String>,
    empty_outputs: HashSet<String>,
    runtime: ExecutableOwnerRuntimePlanSeeds,
    event: ExecutableOwnerEventPlanSeed,
}

impl ExecutableOwnerAnalysisPlanSeeds {
    pub(crate) fn runtime_scope_plans<'a>(
        &'a self,
        bodies: RuntimeScopeBodies<'a>,
        inputs: RuntimeScopePlanInputs<'a>,
    ) -> [RuntimeScopePlan<'a>; 3] {
        let block_common = ScopeAnalysisCtx {
            policy: ScopePolicy::Runtime(ScopeKind::Block),
            input_names: &self.empty_inputs,
            output_names: &self.empty_outputs,
            param_names: inputs.param_names,
            struct_defs: inputs.struct_defs,
            fn_signatures: inputs.fn_signatures,
            fn_return_types: inputs.fn_return_types,
            options: inputs.options,
            port_index_ins: inputs.port_index_ins,
            port_index_outs: inputs.port_index_outs,
            port_index_params: inputs.port_index_params,
        };
        let sample_common = ScopeAnalysisCtx {
            policy: ScopePolicy::Runtime(ScopeKind::Sample),
            input_names: inputs.sample_input_names,
            output_names: inputs.sample_output_names,
            param_names: inputs.param_names,
            struct_defs: inputs.struct_defs,
            fn_signatures: inputs.fn_signatures,
            fn_return_types: inputs.fn_return_types,
            options: inputs.options,
            port_index_ins: inputs.port_index_ins,
            port_index_outs: inputs.port_index_outs,
            port_index_params: inputs.port_index_params,
        };

        [
            RuntimeScopePlan {
                stmts: bodies.block_pre,
                common: block_common,
                registration_mode: RuntimeRegistrationMode::BlockRoot,
                registration_input_names: inputs.registration_input_names,
                registration_output_names: inputs.registration_output_names,
                registration_param_names: inputs.registration_param_names,
                runtime_locals: &self.runtime.block_locals,
                runtime_known_scalars: self.runtime.block_pre_known_scalars.clone(),
                runtime_local_aliases: LocalAliasTypes::new(),
                runtime_local_array_aliases: self.runtime.block_pre_array_aliases.clone(),
                runtime_forbidden_assign_names: &self.runtime.block_forbidden_assign_names,
            },
            RuntimeScopePlan {
                stmts: bodies.sample,
                common: sample_common,
                registration_mode: RuntimeRegistrationMode::None,
                registration_input_names: inputs.registration_input_names,
                registration_output_names: inputs.registration_output_names,
                registration_param_names: inputs.registration_param_names,
                runtime_locals: &self.runtime.sample_locals,
                runtime_known_scalars: self.runtime.sample_known_scalars.clone(),
                runtime_local_aliases: LocalAliasTypes::new(),
                runtime_local_array_aliases: self.runtime.sample_array_aliases.clone(),
                runtime_forbidden_assign_names: &self.runtime.sample_forbidden_assign_names,
            },
            RuntimeScopePlan {
                stmts: bodies.block_post,
                common: block_common,
                registration_mode: RuntimeRegistrationMode::BlockRoot,
                registration_input_names: inputs.registration_input_names,
                registration_output_names: inputs.registration_output_names,
                registration_param_names: inputs.registration_param_names,
                runtime_locals: &self.runtime.block_locals,
                runtime_known_scalars: self.runtime.block_post_known_scalars.clone(),
                runtime_local_aliases: LocalAliasTypes::new(),
                runtime_local_array_aliases: self.runtime.block_post_array_aliases.clone(),
                runtime_forbidden_assign_names: &self.runtime.block_forbidden_assign_names,
            },
        ]
    }

    pub(crate) fn event_plan<'a>(
        &self,
        state_scalars: &HashMap<String, PrimitiveType>,
        inputs: EventPlanInputs<'a>,
    ) -> EventAnalysisPlan<'a> {
        let mut event_known_scalars_seed =
            build_known_scalars_from_state(&self.event.known_scalar_base, state_scalars);
        extend_known_scalars(
            &mut event_known_scalars_seed,
            self.event.known_scalar_extras.iter(),
        );

        EventAnalysisPlan {
            typed_events: inputs.typed_events,
            event_known_scalars_seed,
            event_array_alias_seed: self.event.array_alias_seed.clone(),
            event_immutable_param_seed: self.event.immutable_param_seed.clone(),
            init_writable_roots: inputs.init_writable_roots,
            validation_input_names: inputs.validation_input_names,
            validation_output_names: inputs.validation_output_names,
            common: ScopeAnalysisCtx {
                policy: ScopePolicy::Event,
                input_names: inputs.input_names,
                output_names: inputs.output_names,
                param_names: inputs.param_names,
                struct_defs: inputs.struct_defs,
                fn_signatures: inputs.fn_signatures,
                fn_return_types: inputs.fn_return_types,
                options: inputs.options,
                port_index_ins: inputs.port_index_ins,
                port_index_outs: inputs.port_index_outs,
                port_index_params: inputs.port_index_params,
            },
        }
    }
}

fn build_top_level_runtime_array_aliases(
    in_arrays: &HashMap<String, TypedArrayInfo>,
    out_arrays: &HashMap<String, TypedArrayInfo>,
    param_arrays: &HashMap<String, TypedArrayInfo>,
) -> HashMap<String, LocalArrayAliasInfo> {
    let mut aliases = HashMap::new();
    seed_top_level_array_aliases(&mut aliases, in_arrays, false);
    seed_top_level_array_aliases(&mut aliases, out_arrays, true);
    seed_top_level_array_aliases(&mut aliases, param_arrays, false);
    aliases
}

fn build_top_level_event_array_aliases(
    param_arrays: &HashMap<String, TypedArrayInfo>,
) -> HashMap<String, LocalArrayAliasInfo> {
    let mut aliases = HashMap::new();
    seed_top_level_array_aliases(&mut aliases, param_arrays, false);
    aliases
}

fn collect_proc_owner_known_scalar_extras(
    struct_instances: &HashMap<String, String>,
    nested_proc_instances: &HashMap<String, ProcNestedState>,
    state_arrays: &HashMap<String, usize>,
) -> HashSet<String> {
    let mut extras = HashSet::new();
    extend_known_scalars(&mut extras, struct_instances.keys());
    extend_known_scalars(&mut extras, nested_proc_instances.keys());
    extend_known_scalars(&mut extras, state_arrays.keys());
    extras
}

pub(crate) fn build_top_level_owner_analysis_plan_seeds(
    param_names: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    in_arrays: &HashMap<String, TypedArrayInfo>,
    out_arrays: &HashMap<String, TypedArrayInfo>,
    param_arrays: &HashMap<String, TypedArrayInfo>,
) -> ExecutableOwnerAnalysisPlanSeeds {
    let runtime_array_aliases =
        build_top_level_runtime_array_aliases(in_arrays, out_arrays, param_arrays);
    let block_pre_known_scalars = build_known_scalars_from_state(param_names, state_scalars);
    let mut sample_base = param_names.clone();
    sample_base.extend(input_names.iter().cloned());
    let sample_known_scalars = build_known_scalars_from_state(&sample_base, state_scalars);
    let block_post_known_scalars = build_known_scalars_from_state(param_names, state_scalars);

    ExecutableOwnerAnalysisPlanSeeds {
        empty_inputs: HashSet::new(),
        empty_outputs: HashSet::new(),
        runtime: ExecutableOwnerRuntimePlanSeeds {
            block_locals: HashSet::new(),
            sample_locals: HashSet::new(),
            block_forbidden_assign_names: output_names.clone(),
            sample_forbidden_assign_names: HashSet::new(),
            block_pre_known_scalars,
            sample_known_scalars,
            block_post_known_scalars,
            block_pre_array_aliases: runtime_array_aliases.clone(),
            sample_array_aliases: runtime_array_aliases.clone(),
            block_post_array_aliases: runtime_array_aliases,
        },
        event: ExecutableOwnerEventPlanSeed {
            known_scalar_base: param_names.clone(),
            known_scalar_extras: HashSet::new(),
            array_alias_seed: build_top_level_event_array_aliases(param_arrays),
            immutable_param_seed: param_names.clone(),
        },
    }
}

pub(crate) fn build_proc_owner_analysis_plan_seeds(
    reserved: &HashSet<String>,
    output_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    nested_proc_instances: &HashMap<String, ProcNestedState>,
    state_arrays: &HashMap<String, usize>,
) -> ExecutableOwnerAnalysisPlanSeeds {
    let known_scalar_extras = collect_proc_owner_known_scalar_extras(
        struct_instances,
        nested_proc_instances,
        state_arrays,
    );
    let mut runtime_known_scalars = reserved.clone();
    extend_known_scalars(&mut runtime_known_scalars, known_scalar_extras.iter());

    ExecutableOwnerAnalysisPlanSeeds {
        empty_inputs: HashSet::new(),
        empty_outputs: HashSet::new(),
        runtime: ExecutableOwnerRuntimePlanSeeds {
            block_locals: HashSet::new(),
            sample_locals: HashSet::new(),
            block_forbidden_assign_names: output_names.clone(),
            sample_forbidden_assign_names: HashSet::new(),
            block_pre_known_scalars: runtime_known_scalars.clone(),
            sample_known_scalars: runtime_known_scalars.clone(),
            block_post_known_scalars: runtime_known_scalars,
            block_pre_array_aliases: HashMap::new(),
            sample_array_aliases: HashMap::new(),
            block_post_array_aliases: HashMap::new(),
        },
        event: ExecutableOwnerEventPlanSeed {
            known_scalar_base: reserved.clone(),
            known_scalar_extras,
            array_alias_seed: HashMap::new(),
            immutable_param_seed: HashSet::new(),
        },
    }
}

pub(crate) fn analyze_owner_init_stmts(
    init_stmts: &[Stmt],
    init_ctx: &InitAnalysisCtx<'_>,
    init_locals: &HashSet<String>,
    init_state: &mut InitAnalysisState,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in init_stmts {
        analyze_init_stmt(
            stmt,
            InitStmtAnalysisCtx {
                init: init_ctx,
                locals: init_locals,
            },
            init_state,
            0,
            0,
            errors,
        );
    }
}

pub(crate) fn rewrite_owner_struct_array_inline_fields(
    owner: ExecutableOwnerBodies<'_>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_struct_array_inline_field_stmts(
        owner.init,
        state_array_struct_roots,
        struct_defs,
        errors,
    );
    rewrite_struct_array_inline_field_stmts(
        owner.block_pre,
        state_array_struct_roots,
        struct_defs,
        errors,
    );
    rewrite_struct_array_inline_field_stmts(
        owner.block_post,
        state_array_struct_roots,
        struct_defs,
        errors,
    );
    rewrite_struct_array_inline_field_stmts(
        owner.sample,
        state_array_struct_roots,
        struct_defs,
        errors,
    );
    for event in owner.events {
        rewrite_struct_array_inline_field_stmts(
            &mut event.body,
            state_array_struct_roots,
            struct_defs,
            errors,
        );
    }
}

pub(crate) fn analyze_owner_runtime_scopes<'a>(
    runtime_state: &mut ExecutableOwnerRuntimeState<'a>,
    plans: impl IntoIterator<Item = RuntimeScopePlan<'a>>,
    errors: &mut Vec<Diagnostic>,
) {
    for plan in plans {
        register_and_analyze_runtime_scope(
            plan.stmts.iter(),
            plan.common,
            plan.registration_mode,
            runtime_state.state_scalars,
            runtime_state.declared_symbols,
            runtime_state.state_arrays,
            runtime_state.state_array_struct_roots,
            runtime_state.nested_proc_instances,
            runtime_state.proc_array_roots,
            runtime_state.struct_instances,
            plan.registration_input_names,
            plan.registration_output_names,
            plan.registration_param_names,
            plan.runtime_locals,
            plan.runtime_known_scalars,
            plan.runtime_local_aliases,
            plan.runtime_local_array_aliases,
            plan.runtime_forbidden_assign_names,
            runtime_state.state_tuples,
            errors,
        );
    }
}

pub(crate) fn analyze_owner_events<'a>(
    runtime_state: &ExecutableOwnerRuntimeState<'a>,
    plan: EventAnalysisPlan<'a>,
    errors: &mut Vec<Diagnostic>,
) {
    let final_state_roots = crate::processor_lowering::collect_runtime_state_roots(
        runtime_state.state_scalars,
        runtime_state.state_arrays,
    );
    let immutable_event_roots = final_state_roots
        .difference(plan.init_writable_roots)
        .cloned()
        .collect::<HashSet<_>>();

    analyze_runtime_events(
        plan.typed_events,
        &plan.event_known_scalars_seed,
        &plan.event_array_alias_seed,
        &plan.event_immutable_param_seed,
        plan.init_writable_roots,
        &immutable_event_roots,
        plan.validation_input_names,
        plan.validation_output_names,
        runtime_state.state_scalars,
        runtime_state.declared_symbols,
        runtime_state.state_arrays,
        runtime_state.state_array_struct_roots,
        runtime_state.nested_proc_instances,
        runtime_state.proc_array_roots,
        runtime_state.struct_instances,
        plan.common,
        errors,
    );
}
