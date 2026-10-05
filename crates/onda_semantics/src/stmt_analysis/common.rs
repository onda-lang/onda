use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignmentBindingKind {
    Scalar,
    Data,
}

pub(crate) fn validate_array_write(
    name: &str,
    binding: &LocalArrayAliasInfo,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if binding.writable {
        return true;
    }
    errors.push(Diagnostic::semantic_span(
        format!("cannot assign to immutable array alias '{name}'"),
        target_loc,
    ));
    false
}

pub(crate) fn validate_array_binding_replacement(
    name: &str,
    binding: &LocalArrayAliasInfo,
    expr: &Expr,
    is_slice: bool,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let message = if is_slice || matches!(expr, Expr::Slice { .. }) {
        "slice binding cannot be rebound; use an explicit slice assignment to copy contents"
            .to_owned()
    } else {
        return validate_array_write(name, binding, target_loc, errors);
    };
    errors.push(Diagnostic::semantic_span(message, target_loc));
    false
}

/// Declarations introduce names; replacements preserve the binding's kind.
/// Callers check the concrete type, shape, and write permission separately.
pub(crate) fn validate_assignment_binding(
    name: &str,
    existing: Option<AssignmentBindingKind>,
    assigned: AssignmentBindingKind,
    is_declaration: bool,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    use AssignmentBindingKind::{Data, Scalar};
    let message = match (existing, assigned) {
        (None, _) => return true,
        (Some(Scalar), Data) => {
            format!("data declaration '{name}' conflicts with an existing binding")
        }
        (Some(_), Data) if is_declaration => {
            format!("data declaration '{name}' must introduce a new name")
        }
        (Some(_), Scalar) if is_declaration => {
            format!("typed declaration for '{name}' is only allowed on first assignment")
        }
        (Some(Data), Scalar) => format!("cannot assign a scalar value to data binding '{name}'"),
        _ => return true,
    };
    errors.push(Diagnostic::semantic_span(message, target_loc));
    false
}

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
        Expr::ArrayLiteral { .. },
        Some(DataType::Array {
            element: ArrayElemType::Primitive(element),
            len,
        }),
    ) = (expr, infer_fixed_data_type(&Expr::var(name), env))
    else {
        return false;
    };
    validate_assignment_binding(
        name,
        Some(AssignmentBindingKind::Data),
        AssignmentBindingKind::Data,
        is_declaration,
        target_loc,
        errors,
    );
    if let Some(binding) = env.local_array_aliases.get(name) {
        validate_array_binding_replacement(
            name,
            binding,
            expr,
            binding.static_len.is_none(),
            target_loc,
            errors,
        );
    }
    crate::expr_validation::validate_array_initializer(
        expr,
        Some(ArrayElemType::Primitive(element)),
        Some(len),
        &format!("array initializer for symbol '{name}'"),
        env,
        errors,
    );
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
    if let Some(binding) = env.local_array_aliases.get(base) {
        validate_array_write(base, binding, target_loc, errors);
    }
    let actual = infer_data_value_type(expr, env);
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
    let actual = infer_data_value_type(expr, env);
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
