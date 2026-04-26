use std::collections::{HashMap, HashSet};

use crate::stmt_analysis::PortIndexInfo;
use crate::*;

#[derive(Debug, Clone)]
pub(crate) struct FnSignature {
    pub(crate) params: Vec<String>,
    pub(crate) defaults: Vec<Option<Expr>>,
    pub(crate) param_types: Vec<Option<FnParamType>>,
    pub(crate) type_params: Vec<String>,
    pub(crate) readonly_array_params: HashSet<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct ExprEnv<'a> {
    pub(crate) known_scalars: &'a HashSet<String>,
    pub(crate) locals: &'a HashSet<String>,
    pub(crate) local_aliases: &'a LocalAliasTypes,
    pub(crate) outputs: &'a HashSet<String>,
    pub(crate) array_vars: &'a HashMap<String, usize>,
    pub(crate) local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    pub(crate) declared_symbols: &'a DeclaredSymbolMap,
    pub(crate) param_structs: &'a HashMap<String, String>,
    pub(crate) struct_instances: &'a HashMap<String, String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) allow_array_ctor: bool,
    pub(crate) scope: ScopeKind,
    pub(crate) port_index_ins: Option<PortIndexInfo>,
    pub(crate) port_index_outs: Option<PortIndexInfo>,
    pub(crate) port_index_params: Option<PortIndexInfo>,
    pub(crate) tuple_vars: &'a HashMap<String, usize>,
    pub(crate) proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
}

#[derive(Clone, Copy)]
pub(crate) struct ScopeExprInputs<'a> {
    pub(crate) locals: &'a HashSet<String>,
    pub(crate) state_scalars: &'a HashMap<String, PrimitiveType>,
    pub(crate) declared_symbols: &'a DeclaredSymbolMap,
    pub(crate) param_structs: &'a HashMap<String, String>,
    pub(crate) struct_instances: &'a HashMap<String, String>,
    pub(crate) input_names: &'a HashSet<String>,
    pub(crate) output_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) expr_outputs: &'a HashSet<String>,
    pub(crate) port_index_ins: Option<PortIndexInfo>,
    pub(crate) port_index_outs: Option<PortIndexInfo>,
    pub(crate) port_index_params: Option<PortIndexInfo>,
    pub(crate) proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
}

#[allow(clippy::too_many_arguments)]
static EMPTY_TUPLE_VARS: std::sync::LazyLock<HashMap<String, usize>> =
    std::sync::LazyLock::new(HashMap::new);
static EMPTY_LOCAL_ALIASES: std::sync::LazyLock<LocalAliasTypes> =
    std::sync::LazyLock::new(HashMap::new);
static EMPTY_LOCAL_ARRAY_ALIASES: std::sync::LazyLock<HashMap<String, LocalArrayAliasInfo>> =
    std::sync::LazyLock::new(HashMap::new);
static EMPTY_PROC_ARRAY_ROOTS: std::sync::LazyLock<HashMap<String, ProcNestedArrayState>> =
    std::sync::LazyLock::new(HashMap::new);

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_expr_env<'a>(
    known_scalars: &'a HashSet<String>,
    locals: &'a HashSet<String>,
    outputs: &'a HashSet<String>,
    array_vars: &'a HashMap<String, usize>,
    declared_symbols: &'a DeclaredSymbolMap,
    param_structs: &'a HashMap<String, String>,
    struct_instances: &'a HashMap<String, String>,
    struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &'a HashMap<String, FnSignature>,
    scope: ScopeKind,
) -> ExprEnv<'a> {
    ExprEnv {
        known_scalars,
        locals,
        local_aliases: &EMPTY_LOCAL_ALIASES,
        outputs,
        array_vars,
        local_array_aliases: &EMPTY_LOCAL_ARRAY_ALIASES,
        declared_symbols,
        param_structs,
        struct_instances,
        struct_defs,
        fn_signatures,
        allow_array_ctor: false,
        scope,
        port_index_ins: None,
        port_index_outs: None,
        port_index_params: None,
        tuple_vars: &EMPTY_TUPLE_VARS,
        proc_array_roots: &EMPTY_PROC_ARRAY_ROOTS,
    }
}

pub(crate) fn build_scope_expr_env<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
) -> ExprEnv<'a> {
    let mut env = build_expr_env(
        known_scalars,
        inputs.locals,
        inputs.expr_outputs,
        array_vars,
        inputs.declared_symbols,
        inputs.param_structs,
        inputs.struct_instances,
        inputs.struct_defs,
        inputs.fn_signatures,
        scope,
    );
    env.local_aliases = local_aliases;
    env.port_index_ins = inputs.port_index_ins;
    env.port_index_outs = inputs.port_index_outs;
    env.port_index_params = inputs.port_index_params;
    env.proc_array_roots = inputs.proc_array_roots;
    env
}
