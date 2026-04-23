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
#[allow(dead_code)]
pub(crate) struct PortIndexInfo {
    pub(crate) count: usize,
    pub(crate) elem_ty: PrimitiveType,
}

pub(crate) fn uniform_port_index_info_from_names(
    enabled: bool,
    names: &[String],
    types: &HashMap<String, PrimitiveType>,
) -> Option<PortIndexInfo> {
    if !enabled || names.is_empty() {
        return None;
    }
    uniform_port_type_from_names(names, types).map(|elem_ty| PortIndexInfo {
        count: names.len(),
        elem_ty,
    })
}

pub(crate) fn uniform_port_index_info_from_types(
    enabled: bool,
    count: usize,
    types: impl IntoIterator<Item = PrimitiveType>,
) -> Option<PortIndexInfo> {
    if !enabled || count == 0 {
        return None;
    }
    uniform_port_type_from_types(types).map(|elem_ty| PortIndexInfo { count, elem_ty })
}

fn uniform_port_type_from_names(
    names: &[String],
    types: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    uniform_port_type_from_types(names.iter().filter_map(|name| types.get(name).copied()))
}

fn uniform_port_type_from_types(
    types: impl IntoIterator<Item = PrimitiveType>,
) -> Option<PrimitiveType> {
    let mut it = types.into_iter();
    let first = it.next().unwrap_or(PrimitiveType::F32);
    if it.all(|ty| ty == first) {
        Some(first)
    } else {
        None
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
    pub(crate) fn_return_types: &'a HashMap<String, ReturnType>,
    pub(crate) options: AnalysisOptions,
    pub(crate) port_index_ins: Option<PortIndexInfo>,
    pub(crate) port_index_outs: Option<PortIndexInfo>,
    pub(crate) port_index_params: Option<PortIndexInfo>,
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
    proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
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
        port_index_ins: common.port_index_ins,
        port_index_outs: common.port_index_outs,
        port_index_params: common.port_index_params,
        proc_array_roots,
    }
}
