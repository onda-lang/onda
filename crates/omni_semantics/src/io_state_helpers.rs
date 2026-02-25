use super::*;

pub(crate) fn check_unique_set(
    names: &[String],
    kind: &str,
    all_declared: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut local = HashSet::new();
    for name in names {
        if is_builtin_constant_name(name) {
            errors.push(Diagnostic::semantic(
                format!("{kind} name '{name}' is reserved as a builtin constant"),
                0,
                0,
            ));
            continue;
        }
        if !local.insert(name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate {kind} '{name}'"),
                0,
                0,
            ));
            continue;
        }
        if !all_declared.insert(name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("symbol '{name}' declared multiple times across blocks"),
                0,
                0,
            ));
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
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::DataCtor { .. } => {}
        Expr::Var(name) => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(name, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(name, "out").unwrap_or(0));
        }
        Expr::Index { base, index } => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(base, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(base, "out").unwrap_or(0));
            infer_io_from_expr(index, acc);
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            infer_io_from_expr(lhs, acc);
            infer_io_from_expr(rhs, acc);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => infer_io_from_expr(expr, acc),
        Expr::Logical { lhs, rhs, .. } => {
            infer_io_from_expr(lhs, acc);
            infer_io_from_expr(rhs, acc);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                infer_io_from_expr(arg, acc);
            }
        }
        Expr::ArrayLiteral(values) => {
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

pub(crate) fn register_block_assigned_scalars_as_state<'a>(
    stmts: impl Iterator<Item = &'a Stmt>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
) {
    for stmt in stmts {
        register_block_stmt_assigned_scalars_as_state(
            stmt,
            state_scalars,
            state_data,
            state_data_struct_roots,
            struct_instances,
            input_names,
            output_names,
            param_names,
            struct_defs,
            fn_signatures,
        );
    }
}

pub(crate) fn register_sample_typed_scalar_decls_as_state<'a>(
    stmts: impl Iterator<Item = &'a Stmt>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
) {
    for stmt in stmts {
        register_sample_stmt_typed_scalar_decls_as_state(
            stmt,
            state_scalars,
            state_data,
            state_data_struct_roots,
            struct_instances,
            input_names,
            output_names,
            param_names,
        );
    }
}

pub(crate) fn register_sample_stmt_typed_scalar_decls_as_state(
    stmt: &Stmt,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            if !*is_typed_decl {
                return;
            }
            if let AssignTarget::Var(name) = target {
                if split_simple_field_path(name).is_none()
                    && !is_builtin_constant_name(name)
                    && !input_names.contains(name)
                    && !output_names.contains(name)
                    && !param_names.contains(name)
                    && !state_scalars.contains_key(name)
                    && !state_data.contains_key(name)
                    && !state_data_struct_roots.contains_key(name)
                    && !struct_instances.contains_key(name)
                    && !matches!(expr, Expr::DataCtor { .. })
                    && generic_decl_ty.is_none()
                {
                    state_scalars.insert(name.clone(), decl_ty.unwrap_or(PrimitiveType::F32));
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
            for nested in else_branch {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(crate) fn register_block_stmt_assigned_scalars_as_state(
    stmt: &Stmt,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl: _,
            expr,
            ..
        } => {
            if let AssignTarget::Var(name) = target {
                if split_simple_field_path(name).is_none()
                    && !is_builtin_constant_name(name)
                    && !input_names.contains(name)
                    && !output_names.contains(name)
                    && !param_names.contains(name)
                    && !state_scalars.contains_key(name)
                    && !state_data.contains_key(name)
                    && !state_data_struct_roots.contains_key(name)
                    && !struct_instances.contains_key(name)
                    && !matches!(expr, Expr::DataCtor { .. })
                    && generic_decl_ty.is_none()
                {
                    let inferred_ty = {
                        let mut infer_errors = Vec::<Diagnostic>::new();
                        let empty_locals = HashSet::<String>::new();
                        infer_expr_type_for_semantics(
                            expr,
                            state_scalars,
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
                    state_scalars.insert(name.clone(), decl_ty.unwrap_or(inferred_ty));
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                );
            }
            for nested in else_branch {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                );
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                );
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
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
                name,
                ty: None,
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
    let names = ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    check_local_duplicates(&names, kind, errors);
}

pub(crate) fn check_local_duplicates(names: &[String], kind: &str, errors: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate {kind} '{name}'"),
                0,
                0,
            ));
        }
    }
}
