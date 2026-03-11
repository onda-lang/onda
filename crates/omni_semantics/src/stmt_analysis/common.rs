use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScopePolicy {
    Init,
    Runtime(ScopeKind),
    Def,
    Event,
}

impl ScopePolicy {
    pub(crate) fn scope_kind(self) -> ScopeKind {
        match self {
            Self::Init => ScopeKind::Init,
            Self::Runtime(scope) => scope,
            Self::Def => ScopeKind::Def,
            Self::Event => ScopeKind::Sample,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ScopeAnalysisCtx<'a> {
    pub(crate) policy: ScopePolicy,
    pub(crate) input_names: &'a HashSet<String>,
    pub(crate) output_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) options: AnalysisOptions,
}

impl<'a> ScopeAnalysisCtx<'a> {
    pub(crate) fn scope_kind(self) -> ScopeKind {
        self.policy.scope_kind()
    }
}

pub(crate) fn build_scope_analysis_expr_inputs<'a>(
    common: ScopeAnalysisCtx<'a>,
    locals: &'a HashSet<String>,
    state_scalars: &'a HashMap<String, PrimitiveType>,
    declared_symbols: &'a DeclaredSymbolMap,
    param_structs: &'a HashMap<String, String>,
    struct_instances: &'a HashMap<String, String>,
    expr_outputs: &'a HashSet<String>,
) -> ScopeExprInputs<'a> {
    ScopeExprInputs {
        locals,
        state_scalars,
        declared_symbols,
        param_structs,
        struct_instances,
        input_names: common.input_names,
        output_names: common.output_names,
        param_names: common.param_names,
        struct_defs: common.struct_defs,
        fn_signatures: common.fn_signatures,
        expr_outputs,
    }
}
