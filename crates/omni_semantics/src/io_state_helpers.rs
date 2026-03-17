use super::*;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub(crate) fn resolve_init_default_ty(
    decl_ty: Option<&DeclType>,
    context_label: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    match decl_ty {
        Some(DeclType::Scalar(prim)) => Some(*prim),
        Some(DeclType::Generic(param)) => {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context_label} init section default type '[{param}]' is invalid; only primitive scalar types are allowed"
                ),
            );
            None
        }
        Some(DeclType::Tuple(_)) => {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context_label} init section default type must be a scalar primitive type"
                ),
            );
            None
        }
        Some(DeclType::Array { .. }) | Some(DeclType::ArrayGeneric { .. }) => {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context_label} init section default type must be a scalar primitive type"
                ),
            );
            None
        }
        None => None,
    }
}

/// Resolve the type for a scalar state assignment, given the priority:
/// existing > declared > init_default_ty > inferred > F32.
pub(crate) fn resolve_scalar_assignment_type(
    existing_ty: Option<PrimitiveType>,
    declared_ty: Option<PrimitiveType>,
    inferred_ty: Option<PrimitiveType>,
    init_default_ty: Option<PrimitiveType>,
) -> PrimitiveType {
    match (existing_ty, declared_ty) {
        (Some(existing), _) => existing,
        (None, Some(declared)) => declared,
        (None, None) => init_default_ty
            .or(inferred_ty)
            .unwrap_or(PrimitiveType::F32),
    }
}

pub(crate) fn check_unique_set(
    names: &[String],
    kind: &str,
    all_declared: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut local = HashSet::new();
    for name in names {
        if is_builtin_constant_name(name) {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{kind} name '{name}' is reserved as a builtin constant"),
            );
            continue;
        }
        if !local.insert(name.clone()) {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("duplicate {kind} '{name}'"),
            );
            continue;
        }
        if !all_declared.insert(name.clone()) {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("symbol '{name}' declared multiple times across blocks"),
            );
        }
    }
}

#[derive(Default)]
pub(crate) struct IoInference {
    pub(crate) max_in: usize,
    pub(crate) max_out: usize,
}

pub(crate) fn infer_numbered_io_from_sample(sample: &[Stmt]) -> IoInference {
    let mut out = IoInference::default();
    for stmt in sample {
        infer_io_from_stmt(stmt, &mut out);
    }
    out
}

pub(crate) fn infer_io_from_stmt(stmt: &Stmt, acc: &mut IoInference) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    acc.max_out = acc
                        .max_out
                        .max(parse_numbered_port_index(name, "out").unwrap_or(0));
                }
                AssignTarget::Index { base, index } => {
                    acc.max_out = acc
                        .max_out
                        .max(parse_numbered_port_index(base, "out").unwrap_or(0));
                    infer_io_from_expr(index, acc);
                }
                AssignTarget::Slice { base, start, end } => {
                    acc.max_out = acc
                        .max_out
                        .max(parse_numbered_port_index(base, "out").unwrap_or(0));
                    if let Some(start) = start {
                        infer_io_from_expr(start, acc);
                    }
                    if let Some(end) = end {
                        infer_io_from_expr(end, acc);
                    }
                }
                AssignTarget::Tuple(names) => {
                    for name in names {
                        acc.max_out = acc
                            .max_out
                            .max(parse_numbered_port_index(name, "out").unwrap_or(0));
                    }
                }
            }
            infer_io_from_expr(expr, acc);
        }
        Stmt::Expr { expr, .. } => infer_io_from_expr(expr, acc),
        Stmt::Return { expr, .. } => infer_io_from_expr(expr, acc),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            infer_io_from_expr(cond, acc);
            for nested in then_branch {
                infer_io_from_stmt(nested, acc);
            }
            for nested in else_branch {
                infer_io_from_stmt(nested, acc);
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                infer_io_from_stmt(nested, acc);
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                infer_io_from_stmt(nested, acc);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(crate) fn infer_io_from_expr(expr: &Expr, acc: &mut IoInference) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::ArrayCtor { .. } => {}
        Expr::Var { name, .. } => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(name, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(name, "out").unwrap_or(0));
        }
        Expr::Index { base, index, .. } => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(base, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(base, "out").unwrap_or(0));
            infer_io_from_expr(index, acc);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(base, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(base, "out").unwrap_or(0));
            if let Some(start) = start {
                infer_io_from_expr(start, acc);
            }
            if let Some(end) = end {
                infer_io_from_expr(end, acc);
            }
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            infer_io_from_expr(lhs, acc);
            infer_io_from_expr(rhs, acc);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            infer_io_from_expr(expr, acc)
        }
        Expr::Logical { lhs, rhs, .. } => {
            infer_io_from_expr(lhs, acc);
            infer_io_from_expr(rhs, acc);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                infer_io_from_expr(arg, acc);
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                infer_io_from_expr(value, acc);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                infer_io_from_expr(&arg.expr, acc);
            }
        }
    }
}

pub(crate) fn parse_numbered_port_index(name: &str, prefix: &str) -> Option<usize> {
    let tail = name.strip_prefix(prefix)?;
    if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let idx = tail.parse::<usize>().ok()?;
    if idx == 0 {
        return None;
    }
    Some(idx)
}

/// Controls which runtime-scope assignments are promoted to state scalars.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RuntimeRegistrationMode {
    /// Event-like scope: do not promote assignments to state.
    None,
    /// Block scope: register all assigned scalars (type inferred from expression).
    Block,
    /// Sample scope: register only explicitly typed declarations.
    Sample,
}

pub(crate) fn register_scope_state<'a>(
    stmts: impl Iterator<Item = &'a Stmt>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    registration_mode: RuntimeRegistrationMode,
) {
    for stmt in stmts {
        register_scope_stmt_state(
            stmt,
            state_scalars,
            declared_symbols,
            state_arrays,
            state_array_struct_roots,
            struct_instances,
            input_names,
            output_names,
            param_names,
            struct_defs,
            registration_mode,
        );
    }
}

fn register_scope_stmt_state(
    stmt: &Stmt,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    registration_mode: RuntimeRegistrationMode,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            if matches!(registration_mode, RuntimeRegistrationMode::None) {
                return;
            }
            // Sample scope: only register explicitly typed declarations
            if matches!(registration_mode, RuntimeRegistrationMode::Sample) && !*is_typed_decl {
                return;
            }
            if let AssignTarget::Var(name) = target {
                if split_simple_field_path(name).is_none()
                    && !is_builtin_constant_name(name)
                    && !input_names.contains(name)
                    && !output_names.contains(name)
                    && !param_names.contains(name)
                    && !state_scalars.contains_key(name)
                    && !state_arrays.contains_key(name)
                    && !state_array_struct_roots.contains_key(name)
                    && !struct_instances.contains_key(name)
                    && !matches!(expr, Expr::ArrayCtor { .. })
                    && generic_decl_ty.is_none()
                {
                    let resolved_ty = match registration_mode {
                        RuntimeRegistrationMode::None => return,
                        RuntimeRegistrationMode::Block => {
                            let inferred_ty = {
                                let mut infer_errors = Vec::<Diagnostic>::new();
                                let empty_locals = HashSet::<String>::new();
                                infer_expr_type_for_semantics(
                                    expr,
                                    state_scalars,
                                    declared_symbols,
                                    None,
                                    &empty_locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    struct_instances,
                                    struct_defs,
                                    &mut infer_errors,
                                )
                                .unwrap_or(PrimitiveType::F32)
                            };
                            decl_ty.unwrap_or(inferred_ty)
                        }
                        RuntimeRegistrationMode::Sample => decl_ty.unwrap_or(PrimitiveType::F32),
                    };
                    state_scalars.insert(name.clone(), resolved_ty);
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                register_scope_stmt_state(
                    nested,
                    state_scalars,
                    declared_symbols,
                    state_arrays,
                    state_array_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    registration_mode,
                );
            }
            for nested in else_branch {
                register_scope_stmt_state(
                    nested,
                    state_scalars,
                    declared_symbols,
                    state_arrays,
                    state_array_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    registration_mode,
                );
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                register_scope_stmt_state(
                    nested,
                    state_scalars,
                    declared_symbols,
                    state_arrays,
                    state_array_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    registration_mode,
                );
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                register_scope_stmt_state(
                    nested,
                    state_scalars,
                    declared_symbols,
                    state_arrays,
                    state_array_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    registration_mode,
                );
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(crate) fn normalize_numbered_ports(
    explicit: &[String],
    prefix: &str,
    inferred_max: usize,
) -> Vec<String> {
    let explicit_max = explicit
        .iter()
        .filter_map(|name| parse_numbered_port_index(name, prefix))
        .max()
        .unwrap_or(0);
    let max_idx = explicit_max.max(inferred_max);

    let mut out = Vec::new();
    if max_idx > 0 {
        for idx in 1..=max_idx {
            out.push(format!("{prefix}{idx}"));
        }
    }

    for name in explicit {
        if parse_numbered_port_index(name, prefix).is_none() && !out.contains(name) {
            out.push(name.clone());
        }
    }

    out
}

pub(crate) fn normalize_numbered_port_decls(
    explicit: &[PortDecl],
    prefix: &str,
    inferred_max: usize,
) -> Vec<PortDecl> {
    let explicit_names = explicit.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let ordered_names = normalize_numbered_ports(&explicit_names, prefix, inferred_max);
    let explicit_map = explicit
        .iter()
        .map(|p| (p.name.clone(), p.clone()))
        .collect::<HashMap<_, _>>();
    ordered_names
        .into_iter()
        .map(|name| {
            explicit_map.get(&name).cloned().unwrap_or(PortDecl {
                loc: Default::default(),
                name,
                ty: None,
                ty_loc: Default::default(),
                default: None,
                range: None,
            })
        })
        .collect()
}

pub(crate) fn check_local_port_duplicates(
    ports: &[PortDecl],
    kind: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let mut seen = HashSet::new();
    for port in ports {
        if !seen.insert(port.name.clone()) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate {kind} '{}'", port.name),
                port.loc.as_ref(),
            ));
        }
    }
}
