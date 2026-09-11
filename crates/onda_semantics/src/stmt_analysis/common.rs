use super::*;

pub(crate) fn infer_data_initializer_type(
    expr: &Expr,
    declared: Option<&DeclType>,
    env: ExprEnv<'_>,
) -> Option<DataType> {
    if matches!(declared, Some(DeclType::Array { .. })) {
        infer_fixed_initializer_type(expr, env)
    } else {
        infer_fixed_data_type(expr, env)
    }
}

pub(crate) fn validate_primitive_array_literal_replacement(
    name: &str,
    expr: &Expr,
    is_declaration: bool,
    env: ExprEnv<'_>,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let (
        Expr::ArrayLiteral { values, .. },
        Some(DataType::Array {
            element: ArrayElemType::Primitive(element),
            len,
        }),
    ) = (expr, infer_fixed_data_type(&Expr::var(name), env))
    else {
        return false;
    };
    if is_declaration {
        errors.push(Diagnostic::semantic_span(
            format!("data declaration '{name}' must introduce a new name"),
            target_loc,
        ));
    }
    if env
        .local_array_aliases
        .get(name)
        .is_some_and(|alias| !alias.writable)
    {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign to immutable array alias '{name}'"),
            target_loc,
        ));
    }
    validate_primitive_array_values(values, element, len, expr, env, errors);
    true
}

pub(crate) fn validate_data_element_replacement(
    base: &str,
    index: &Expr,
    expr: &Expr,
    env: ExprEnv<'_>,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let selection = Expr::Index {
        loc: target_loc.into(),
        base: base.to_owned(),
        index: Box::new(index.clone()),
    };
    let Some(data @ DataType::Struct(_)) = infer_fixed_data_type(&selection, env) else {
        return false;
    };
    if env
        .local_array_aliases
        .get(base)
        .is_some_and(|alias| !alias.writable)
    {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign to immutable array alias '{base}'"),
            target_loc,
        ));
    }
    let actual = infer_fixed_data_type(expr, env);
    if actual.as_ref() != Some(&data) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "element replacement for '{base}[...]' {}",
                data_type_mismatch(&data, actual.as_ref())
            ),
            target_loc,
        ));
    }
    validate_expr(index, env, errors);
    validate_fixed_data_expr(expr, env, errors);
    true
}

pub(crate) fn validate_fixed_data_binding_replacement(
    name: &str,
    expr: &Expr,
    env: ExprEnv<'_>,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(expected @ (DataType::Struct(_) | DataType::Array { .. })) =
        infer_fixed_data_type(&Expr::var(name), env)
    else {
        return false;
    };
    if env
        .local_array_aliases
        .get(name)
        .is_some_and(|alias| !alias.writable)
    {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign to immutable data alias '{name}'"),
            target_loc,
        ));
    }
    let actual = infer_fixed_data_type(expr, env);
    if actual.as_ref() != Some(&expected) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "data replacement for '{name}' {}",
                data_type_mismatch(&expected, actual.as_ref())
            ),
            target_loc,
        ));
    }
    validate_fixed_data_expr(expr, env, errors);
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScopePolicy {
    Init,
    Runtime(ScopeKind),
    Task,
    Def,
    Event,
}

impl ScopePolicy {
    pub(crate) fn scope_kind(self) -> ScopeKind {
        match self {
            Self::Init => ScopeKind::Init,
            Self::Runtime(scope) => scope,
            Self::Task => ScopeKind::Block,
            Self::Def => ScopeKind::Def,
            Self::Event => ScopeKind::Sample,
        }
    }

    pub(crate) fn diagnostic_label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Event => "event handler",
            Self::Init | Self::Runtime(_) | Self::Def => self.scope_kind().label(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PortIndexInfo {
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
    uniform_port_type_from_names(names, types).map(|elem_ty| PortIndexInfo { elem_ty })
}

pub(crate) fn uniform_port_index_info_from_types(
    enabled: bool,
    count: usize,
    types: impl IntoIterator<Item = PrimitiveType>,
) -> Option<PortIndexInfo> {
    if !enabled || count == 0 {
        return None;
    }
    uniform_port_type_from_types(types).map(|elem_ty| PortIndexInfo { elem_ty })
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
    pub(crate) output_array_names: &'a HashSet<String>,
    pub(crate) io_surface_names: &'a HashSet<String>,
    pub(crate) io_surface_array_names: &'a HashSet<String>,
    pub(crate) dynamic_param_array_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) fn_return_types: &'a HashMap<String, ReturnType>,
    pub(crate) options: AnalysisOptions,
    pub(crate) port_index_ins: Option<PortIndexInfo>,
    pub(crate) port_index_outs: Option<PortIndexInfo>,
    pub(crate) port_index_params: Option<PortIndexInfo>,
    pub(crate) port_index_kins: Option<PortIndexInfo>,
    pub(crate) proc_event_names: &'a HashSet<String>,
}

impl<'a> ScopeAnalysisCtx<'a> {
    pub(crate) fn scope_kind(self) -> ScopeKind {
        self.policy.scope_kind()
    }

    pub(crate) fn diagnostic_label(self) -> &'static str {
        self.policy.diagnostic_label()
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
    struct_array_roots: &'a HashMap<String, ArrayStructRootInfo>,
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
        output_array_names: common.output_array_names,
        io_surface_names: common.io_surface_names,
        io_surface_array_names: common.io_surface_array_names,
        io_surface_access_allowed: matches!(
            common.policy,
            ScopePolicy::Runtime(_) | ScopePolicy::Task
        ),
        dynamic_param_array_names: common.dynamic_param_array_names,
        dynamic_param_indexing_allowed: matches!(
            common.policy,
            ScopePolicy::Runtime(_) | ScopePolicy::Task
        ),
        param_names: common.param_names,
        struct_defs: common.struct_defs,
        fn_signatures: common.fn_signatures,
        expr_outputs,
        port_index_ins: common.port_index_ins,
        port_index_outs: common.port_index_outs,
        port_index_params: common.port_index_params,
        port_index_kins: common.port_index_kins,
        struct_array_roots,
        proc_array_roots,
        proc_event_names: common.proc_event_names,
        diagnostic_scope: common.diagnostic_label(),
    }
}
