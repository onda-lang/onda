use std::cell::RefCell;

use crate::*;

#[derive(Clone, Copy)]
pub(crate) struct DefStmtAnalysisCtx<'a> {
    pub common: ScopeAnalysisCtx<'a>,
    pub locals: &'a HashSet<String>,
    pub declared_symbols: &'a DeclaredSymbolMap,
    pub param_structs: &'a HashMap<String, String>,
    pub struct_array_roots: &'a HashMap<String, ArrayStructRootInfo>,
    pub proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
    pub state_scalars: &'a HashMap<String, PrimitiveType>,
    pub resolved_scalar_locals: &'a RefCell<LocalAliasTypes>,
}

pub(crate) type DefStmtAnalysisState = ScopeFlowState;

pub(crate) fn analyze_def_stmt_list(
    stmts: &[Stmt],
    ctx: DefStmtAnalysisCtx<'_>,
    state: &mut DefStmtAnalysisState,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) -> super::call_types::StatementFlow {
    debug_assert_eq!(ctx.common.policy, ScopePolicy::Def);

    let mut state_scalars = ctx.state_scalars.clone();
    let state_arrays = HashMap::new();
    let nested_proc_instances = HashMap::new();
    let registration_names = HashSet::new();
    let forbidden_assign_names = HashSet::new();
    let state_tuples = HashMap::new();
    let scope_ctx = FlowStmtAnalysisCtx {
        common: ctx.common,
        registration_mode: RuntimeRegistrationMode::None,
        declared_symbols: ctx.declared_symbols,
        state_arrays: &state_arrays,
        state_array_struct_roots: ctx.struct_array_roots,
        nested_proc_instances: &nested_proc_instances,
        struct_instances: ctx.param_structs,
        registration_input_names: &registration_names,
        registration_output_names: &registration_names,
        registration_param_names: &registration_names,
        forbidden_assign_names: &forbidden_assign_names,
        forbidden_assign_array_names: &forbidden_assign_names,
        proc_array_roots: ctx.proc_array_roots,
        event_policy: None,
        state_tuples: &state_tuples,
        resolved_scalar_locals: Some(ctx.resolved_scalar_locals),
        resolved_array_locals: None,
        resolved_tuple_locals: None,
    };

    analyze_flow_scope_stmts(
        stmts.iter(),
        ctx.locals,
        &mut state_scalars,
        &scope_ctx,
        state,
        loop_depth,
        0,
        errors,
    )
}
