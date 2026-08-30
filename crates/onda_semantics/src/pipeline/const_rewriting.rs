use super::*;

pub(super) fn fold_decl_type_const_arrays(
    ty: &mut Option<DeclType>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            fold_const_array_expr(size, const_values, options, errors, false);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

pub(super) fn fixed_array_default_target(
    ty: &Option<DeclType>,
    options: AnalysisOptions,
) -> Option<(Option<PrimitiveType>, usize)> {
    match ty {
        Some(DeclType::Array { elem, size }) => {
            eval_data_size_expr_silent(size, options).map(|len| (Some(*elem), len))
        }
        Some(DeclType::ArrayGeneric { size, .. }) => {
            eval_data_size_expr_silent(size, options).map(|len| (None, len))
        }
        _ => None,
    }
}

pub(super) fn event_array_default_target(
    ty: &EventParamType,
    options: AnalysisOptions,
) -> Option<(Option<PrimitiveType>, usize)> {
    match ty {
        EventParamType::Array { elem, size } => {
            eval_data_size_expr_silent(size, options).map(|len| (Some(*elem), len))
        }
        EventParamType::GenericArray { size, .. } => {
            eval_data_size_expr_silent(size, options).map(|len| (None, len))
        }
        _ => None,
    }
}

pub(super) fn eval_data_size_expr_silent(expr: &Expr, options: AnalysisOptions) -> Option<usize> {
    let mut ignored = Vec::new();
    eval_data_size_expr(expr, options, "array size", &mut ignored)
}

pub(super) fn fixed_array_type_label(elem_ty: PrimitiveType, len: usize) -> String {
    format!("{}[{len}]", primitive_type_label(elem_ty))
}

pub(super) fn const_array_default_incompatible(
    default_expr: &Expr,
    expected: (Option<PrimitiveType>, usize),
    context: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Expr::Var { name, .. } = default_expr else {
        return false;
    };
    let Some(ConstValue::Array { elem_ty, len, .. }) = const_values.get(name) else {
        return false;
    };
    let (expected_elem, expected_len) = expected;
    match expected_elem {
        Some(expected_elem) => {
            if *elem_ty == expected_elem && *len == expected_len {
                return false;
            }
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context} default const array '{name}' has type {}, expected {}",
                    fixed_array_type_label(*elem_ty, *len),
                    fixed_array_type_label(expected_elem, expected_len)
                ),
                default_expr.loc(),
            ));
            true
        }
        None => {
            if *len == expected_len {
                return false;
            }
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context} default const array '{name}' has length {len}, expected {expected_len}"
                ),
                default_expr.loc(),
            ));
            true
        }
    }
}

pub(super) fn fold_fixed_array_default_const_arrays(
    default: &mut Option<Expr>,
    target: Option<(Option<PrimitiveType>, usize)>,
    context: &str,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let (Some(default_expr), Some(target)) = (default.as_ref(), target) {
        if const_array_default_incompatible(default_expr, target, context, const_values, errors) {
            *default = None;
            return;
        }
    }

    if let Some(default_expr) = default {
        fold_const_array_expr(
            default_expr,
            const_values,
            options,
            errors,
            target.is_some(),
        );
    }
}

pub(super) fn fold_decl_range_const_arrays(
    range: &mut Option<DeclRange>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            fold_const_array_expr(min, const_values, options, errors, false);
        }
        fold_const_array_expr(&mut range.max, const_values, options, errors, false);
    }
}

pub(super) fn fold_port_decl_const_arrays(
    decl: &mut PortDecl,
    kind: &str,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    fold_decl_type_const_arrays(&mut decl.ty, const_values, options, errors);
    let target = fixed_array_default_target(&decl.ty, options);
    fold_fixed_array_default_const_arrays(
        &mut decl.default,
        target,
        &format!("{kind} '{}'", decl.name),
        const_values,
        options,
        errors,
    );
    fold_decl_range_const_arrays(&mut decl.range, const_values, options, errors);
}

pub(super) fn fold_param_decl_const_arrays(
    decl: &mut ParamDecl,
    context: &str,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    fold_decl_type_const_arrays(&mut decl.ty, const_values, options, errors);
    let target = fixed_array_default_target(&decl.ty, options);
    fold_fixed_array_default_const_arrays(
        &mut decl.default,
        target,
        context,
        const_values,
        options,
        errors,
    );
    fold_decl_range_const_arrays(&mut decl.range, const_values, options, errors);
    for expr in [&mut decl.control.curve, &mut decl.control.step]
        .into_iter()
        .flatten()
    {
        fold_const_array_expr(expr, const_values, options, errors, false);
    }
}

pub(super) fn fold_buffer_type_const_arrays(
    ty: &mut Option<BufferType>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        fold_const_array_expr(expr, const_values, options, errors, false);
    }
}

pub(super) fn fold_fn_param_type_const_arrays(
    ty: &mut Option<FnParamType>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            fold_const_array_expr(size, const_values, options, errors, false);
        }
        Some(FnParamType::Buffer(buffer_ty))
        | Some(FnParamType::BufferArray {
            buffer: buffer_ty, ..
        }) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                fold_const_array_expr(expr, const_values, options, errors, false);
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
}

pub(super) fn fold_event_param_type_const_arrays(
    ty: &mut EventParamType,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            fold_const_array_expr(size, const_values, options, errors, false);
        }
        _ => {}
    }
}

pub(super) fn fold_field_type_const_arrays(
    ty: &mut FieldType,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        fold_const_array_expr(&mut spec.size, const_values, options, errors, false);
    }
}

pub(super) fn fold_stmt_const_arrays(
    stmt: &mut Stmt,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            fold_const_array_expr(&mut decl.expr, const_values, options, errors, false);
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                fold_const_array_expr(size, const_values, options, errors, false);
            }
        }
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Index { index, .. } => {
                    fold_const_array_expr(index, const_values, options, errors, false);
                }
                AssignTarget::Slice {
                    selector,
                    channel,
                    start,
                    end,
                    ..
                } => {
                    for coordinate in [selector, channel, start, end].into_iter().flatten() {
                        fold_const_array_expr(coordinate, const_values, options, errors, false);
                    }
                }
                AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
            }
            fold_const_array_expr(expr, const_values, options, errors, false);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_const_array_expr(expr, const_values, options, errors, false);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                fold_const_array_expr(value, const_values, options, errors, false);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_const_array_expr(cond, const_values, options, errors, false);
            for nested in then_branch {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
            for nested in else_branch {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                fold_const_array_expr(step, const_values, options, errors, false);
            }
            fold_const_array_expr(start, const_values, options, errors, false);
            fold_const_array_expr(end, const_values, options, errors, false);
            for nested in body {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            fold_const_array_expr(cond, const_values, options, errors, false);
            for nested in body {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn fold_function_const_arrays(
    def: &mut FunctionDef,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        fold_fn_param_type_const_arrays(&mut param.ty, const_values, options, errors);
        if let Some(default) = &mut param.default {
            fold_const_array_expr(default, const_values, options, errors, false);
        }
    }
    for stmt in &mut def.body {
        fold_stmt_const_arrays(stmt, const_values, options, errors);
    }
}

pub(super) fn fold_event_const_arrays(
    event: &mut EventDef,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        fold_event_param_type_const_arrays(&mut param.ty, const_values, options, errors);
        let target = event_array_default_target(&param.ty, options);
        fold_fixed_array_default_const_arrays(
            &mut param.default,
            target,
            &format!("event '{}.{}'", event.name, param.name),
            const_values,
            options,
            errors,
        );
    }
    for stmt in &mut event.body {
        fold_stmt_const_arrays(stmt, const_values, options, errors);
    }
}

pub(super) fn fold_delegate_const_arrays(
    delegate: &mut DelegateDef,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut delegate.params {
        fold_event_param_type_const_arrays(&mut param.ty, const_values, options, errors);
        let target = event_array_default_target(&param.ty, options);
        fold_fixed_array_default_const_arrays(
            &mut param.default,
            target,
            &format!("delegate '{}.{}'", delegate.name, param.name),
            const_values,
            options,
            errors,
        );
    }
}

pub(super) fn fold_when_const_arrays(
    when: &mut WhenDef,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(index) = &mut when.target.index {
        fold_const_array_expr(index, const_values, options, errors, false);
    }
    for stmt in &mut when.body {
        fold_stmt_const_arrays(stmt, const_values, options, errors);
    }
}

pub(super) fn fold_graph_const_arrays(
    graph: &mut GraphBlock,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &mut graph.edges {
        fold_const_array_expr(&mut edge.source, const_values, options, errors, false);
        if let Some(delay) = &mut edge.delay {
            fold_const_array_expr(delay, const_values, options, errors, false);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_const_array_expr(index, const_values, options, errors, false);
            }
        }
    }
}

pub(super) fn reject_forward_const_ref_name(
    name: &str,
    loc: SourceLoc,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if future_consts.contains(name) && !visible_consts.contains_key(name) {
        errors.push(Diagnostic::semantic_span(
            format!("constant '{name}' is not visible before its declaration"),
            loc,
        ));
    }
}

pub(super) fn reject_forward_const_refs_expr(
    expr: &Expr,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var { name, .. } => {
            reject_forward_const_ref_name(name, expr.loc(), visible_consts, future_consts, errors);
        }
        Expr::Index { base, index, .. } => {
            reject_forward_const_ref_name(base, expr.loc(), visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            reject_forward_const_ref_name(base, expr.loc(), visible_consts, future_consts, errors);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                reject_forward_const_refs_expr(coordinate, visible_consts, future_consts, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            reject_forward_const_refs_expr(&spec.size, visible_consts, future_consts, errors);
            if let Some(init) = init {
                for value in init {
                    reject_forward_const_refs_expr(value, visible_consts, future_consts, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            reject_forward_const_refs_expr(lhs, visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(rhs, visible_consts, future_consts, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                reject_forward_const_refs_expr(arg, visible_consts, future_consts, errors);
            }
        }
        Expr::UserCall { name, args, .. } => {
            if args.is_empty() {
                if let Some(base) = parse_array_len_instance_base(name) {
                    reject_forward_const_ref_name(
                        base,
                        expr.loc(),
                        visible_consts,
                        future_consts,
                        errors,
                    );
                }
            }
            if let Some((base, _method)) = name.rsplit_once('.') {
                reject_forward_const_ref_name(
                    base,
                    expr.loc(),
                    visible_consts,
                    future_consts,
                    errors,
                );
            }
            for arg in args {
                reject_forward_const_refs_expr(&arg.expr, visible_consts, future_consts, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                reject_forward_const_refs_expr(value, visible_consts, future_consts, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

pub(super) fn reject_forward_const_refs_decl_type(
    ty: &Option<DeclType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

pub(super) fn reject_forward_const_refs_decl_range(
    range: &Option<DeclRange>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &range.min {
            reject_forward_const_refs_expr(min, visible_consts, future_consts, errors);
        }
        reject_forward_const_refs_expr(&range.max, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_port_decl(
    decl: &PortDecl,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    reject_forward_const_refs_decl_type(&decl.ty, visible_consts, future_consts, errors);
    if let Some(default) = &decl.default {
        reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
    }
    reject_forward_const_refs_decl_range(&decl.range, visible_consts, future_consts, errors);
}

pub(super) fn reject_forward_const_refs_param_decl(
    decl: &ParamDecl,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    reject_forward_const_refs_decl_type(&decl.ty, visible_consts, future_consts, errors);
    if let Some(default) = &decl.default {
        reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
    }
    reject_forward_const_refs_decl_range(&decl.range, visible_consts, future_consts, errors);
    for expr in [&decl.control.curve, &decl.control.step]
        .into_iter()
        .flatten()
    {
        reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_buffer_type(
    ty: &Option<BufferType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_fn_param_type(
    ty: &Option<FnParamType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        Some(FnParamType::Buffer(buffer_ty))
        | Some(FnParamType::BufferArray {
            buffer: buffer_ty, ..
        }) => {
            if let BufferChannels::Static(expr) = &buffer_ty.channels {
                reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
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
}

pub(super) fn reject_forward_const_refs_event_param_type(
    ty: &EventParamType,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        _ => {}
    }
}

pub(super) fn reject_forward_const_refs_field_type(
    ty: &FieldType,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        reject_forward_const_refs_expr(&spec.size, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_return_type(
    ty: &Option<FnReturnType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

pub(super) fn reject_forward_const_refs_assign_target(
    target: &AssignTarget,
    target_loc: SourceLoc,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match target {
        AssignTarget::Index { base, index } => {
            reject_forward_const_ref_name(base, target_loc, visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
        }
        AssignTarget::Slice {
            base,
            selector,
            channel,
            start,
            end,
        } => {
            reject_forward_const_ref_name(base, target_loc, visible_consts, future_consts, errors);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                reject_forward_const_refs_expr(coordinate, visible_consts, future_consts, errors);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

pub(super) fn reject_forward_const_refs_stmt(
    stmt: &Stmt,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            reject_forward_const_refs_expr(&decl.expr, visible_consts, future_consts, errors);
            if let Some(ConstType::Array { size, .. }) = &decl.ty {
                reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
            }
        }
        Stmt::Assign {
            target_loc,
            target,
            expr,
            ..
        } => {
            reject_forward_const_refs_assign_target(
                target,
                target_loc.as_ref().into(),
                visible_consts,
                future_consts,
                errors,
            );
            reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                reject_forward_const_refs_expr(value, visible_consts, future_consts, errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            reject_forward_const_refs_expr(cond, visible_consts, future_consts, errors);
            for nested in then_branch {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
            for nested in else_branch {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                reject_forward_const_refs_expr(step, visible_consts, future_consts, errors);
            }
            reject_forward_const_refs_expr(start, visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(end, visible_consts, future_consts, errors);
            for nested in body {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            reject_forward_const_refs_expr(cond, visible_consts, future_consts, errors);
            for nested in body {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn reject_forward_const_refs_function(
    def: &FunctionDef,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_forward_const_refs_fn_param_type(&param.ty, visible_consts, future_consts, errors);
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
        }
    }
    reject_forward_const_refs_return_type(&def.return_ty, visible_consts, future_consts, errors);
    for stmt in &def.body {
        reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_event(
    event: &EventDef,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &event.params {
        reject_forward_const_refs_event_param_type(
            &param.ty,
            visible_consts,
            future_consts,
            errors,
        );
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
        }
    }
    for stmt in &event.body {
        reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_delegate(
    delegate: &DelegateDef,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &delegate.params {
        reject_forward_const_refs_event_param_type(
            &param.ty,
            visible_consts,
            future_consts,
            errors,
        );
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
        }
    }
}

pub(super) fn reject_forward_const_refs_when(
    when: &WhenDef,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(index) = &when.target.index {
        reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
    }
    for stmt in &when.body {
        reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_graph(
    graph: &GraphBlock,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &graph.edges {
        reject_forward_const_refs_expr(&edge.source, visible_consts, future_consts, errors);
        if let Some(delay) = &edge.delay {
            reject_forward_const_refs_expr(delay, visible_consts, future_consts, errors);
        }
        for dest in &edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
            }
        }
    }
}

pub(super) fn reject_forward_const_refs_in_block(
    block: &Block,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &ports.deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            for decl in &ports.decls {
                reject_forward_const_refs_port_decl(decl, visible_consts, future_consts, errors);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &params.deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            for decl in &params.decls {
                reject_forward_const_refs_param_decl(decl, visible_consts, future_consts, errors);
            }
        }
        Block::Events(events) => {
            for event in &events.events {
                reject_forward_const_refs_event(event, visible_consts, future_consts, errors);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &delegates.delegates {
                reject_forward_const_refs_delegate(delegate, visible_consts, future_consts, errors);
            }
        }
        Block::When(when) => {
            reject_forward_const_refs_when(when, visible_consts, future_consts, errors)
        }
        Block::Tasks(tasks) => {
            for task in &tasks.tasks {
                for stmt in &task.body {
                    reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
                }
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &buffers.deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            for decl in &buffers.decls {
                reject_forward_const_refs_buffer_type(
                    &decl.ty,
                    visible_consts,
                    future_consts,
                    errors,
                );
            }
        }
        Block::Init(init) => {
            for stmt in &init.body {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &block_exec.pre {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            if let Some(sample) = &block_exec.sample {
                if let Some(factor) = &sample.oversample_factor {
                    reject_forward_const_refs_expr(factor, visible_consts, future_consts, errors);
                }
                for stmt in &sample.body {
                    reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
                }
            }
            for stmt in &block_exec.post {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &sample.oversample_factor {
                reject_forward_const_refs_expr(factor, visible_consts, future_consts, errors);
            }
            for stmt in &sample.body {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
        }
        Block::Graph(graph) => {
            reject_forward_const_refs_graph(graph, visible_consts, future_consts, errors);
        }
        Block::Assert(assert_decl) => {
            reject_forward_const_refs_expr(
                &assert_decl.expr,
                visible_consts,
                future_consts,
                errors,
            );
        }
        Block::Def(def) if !def.is_const => {
            reject_forward_const_refs_function(def, visible_consts, future_consts, errors);
        }
        Block::Struct(struct_def) => {
            for field in &struct_def.fields {
                reject_forward_const_refs_field_type(
                    &field.ty,
                    visible_consts,
                    future_consts,
                    errors,
                );
                if let Some(default) = &field.default {
                    reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
                }
            }
            for method in &struct_def.methods {
                reject_forward_const_refs_function(method, visible_consts, future_consts, errors);
            }
        }
        Block::Proc(proc) => {
            if let Some(count) = &proc.ins_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            if let Some(default_ty) = &proc.ins_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(count) = &proc.outs_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            if let Some(default_ty) = &proc.outs_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(count) = &proc.params_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            if let Some(default_ty) = &proc.params_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(count) = &proc.buffers_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            reject_forward_const_refs_buffer_type(
                &proc.buffers_deferred_default_ty,
                visible_consts,
                future_consts,
                errors,
            );
            for decl in &proc.ins {
                reject_forward_const_refs_port_decl(decl, visible_consts, future_consts, errors);
            }
            for decl in &proc.outs {
                reject_forward_const_refs_port_decl(decl, visible_consts, future_consts, errors);
            }
            for decl in &proc.params {
                reject_forward_const_refs_param_decl(decl, visible_consts, future_consts, errors);
            }
            for decl in &proc.buffers {
                reject_forward_const_refs_buffer_type(
                    &decl.ty,
                    visible_consts,
                    future_consts,
                    errors,
                );
            }
            if let Some(default_ty) = &proc.init.default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(factor) = &proc.sample_oversample_factor {
                reject_forward_const_refs_expr(factor, visible_consts, future_consts, errors);
            }
            for event in &proc.events {
                reject_forward_const_refs_event(event, visible_consts, future_consts, errors);
            }
            for delegate in &proc.delegates {
                reject_forward_const_refs_delegate(delegate, visible_consts, future_consts, errors);
            }
            for when in &proc.whens {
                reject_forward_const_refs_when(when, visible_consts, future_consts, errors);
            }
            for stmt in &proc.init.body {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            for stmt in &proc.block_pre {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            for stmt in &proc.sample {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            for stmt in &proc.block_post {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            if let Some(graph) = &proc.graph {
                reject_forward_const_refs_graph(graph, visible_consts, future_consts, errors);
            }
            for task in &proc.tasks {
                for stmt in &task.body {
                    reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
                }
            }
            for def in &proc.local_defs {
                reject_forward_const_refs_function(def, visible_consts, future_consts, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

pub(super) fn fold_const_array_exprs_in_block(
    block: &mut Block,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    structural_options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) => {
            if let Some(count) = &mut ports.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut ports.decls {
                fold_port_decl_const_arrays(decl, "input", const_values, options, errors);
            }
        }
        Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &mut ports.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut ports.decls {
                fold_port_decl_const_arrays(decl, "output", const_values, options, errors);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &mut params.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut params.decls {
                fold_param_decl_const_arrays(
                    decl,
                    &format!("param '<top-level>.{}'", decl.name),
                    const_values,
                    options,
                    errors,
                );
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                fold_event_const_arrays(event, const_values, options, errors);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &mut delegates.delegates {
                fold_delegate_const_arrays(delegate, const_values, options, errors);
            }
        }
        Block::When(when) => {
            fold_when_const_arrays(when, const_values, options, errors);
        }
        Block::Tasks(tasks) => {
            for task in &mut tasks.tasks {
                for stmt in &mut task.body {
                    fold_stmt_const_arrays(stmt, const_values, options, errors);
                }
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &mut buffers.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut buffers.decls {
                fold_buffer_type_const_arrays(&mut decl.ty, const_values, options, errors);
            }
        }
        Block::Init(init) => {
            for stmt in &mut init.body {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &mut block_exec.pre {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_const_array_expr(factor, const_values, options, errors, false);
                }
                for stmt in &mut sample.body {
                    fold_stmt_const_arrays(stmt, const_values, options, errors);
                }
            }
            for stmt in &mut block_exec.post {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_const_array_expr(factor, const_values, options, errors, false);
            }
            for stmt in &mut sample.body {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
        }
        Block::Graph(graph) => {
            fold_graph_const_arrays(graph, const_values, options, errors);
        }
        Block::Assert(assert_decl) => {
            fold_const_array_expr(&mut assert_decl.expr, const_values, options, errors, false);
        }
        Block::Def(def) if !def.is_const => {
            fold_function_const_arrays(def, const_values, options, errors);
        }
        Block::Struct(struct_def) => {
            for field in &mut struct_def.fields {
                fold_field_type_const_arrays(&mut field.ty, const_values, options, errors);
                if let Some(default) = &mut field.default {
                    fold_const_array_expr(default, const_values, options, errors, false);
                }
            }
            for method in &mut struct_def.methods {
                fold_function_const_arrays(method, const_values, options, errors);
            }
        }
        Block::Proc(proc) => {
            for decl in &mut proc.ins {
                fold_port_decl_const_arrays(
                    decl,
                    &format!("processor '{}' input", proc.name),
                    const_values,
                    options,
                    errors,
                );
            }
            if let Some(count) = &mut proc.ins_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut proc.outs {
                fold_port_decl_const_arrays(
                    decl,
                    &format!("processor '{}' output", proc.name),
                    const_values,
                    options,
                    errors,
                );
            }
            if let Some(count) = &mut proc.outs_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut proc.params {
                fold_param_decl_const_arrays(
                    decl,
                    &format!("processor '{}' param '{}'", proc.name, decl.name),
                    const_values,
                    options,
                    errors,
                );
            }
            if let Some(count) = &mut proc.params_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut proc.buffers {
                fold_buffer_type_const_arrays(&mut decl.ty, const_values, options, errors);
            }
            if let Some(count) = &mut proc.buffers_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            if let Some(factor) = &mut proc.sample_oversample_factor {
                fold_const_array_expr(factor, const_values, structural_options, errors, false);
            }
            for event in &mut proc.events {
                fold_event_const_arrays(event, const_values, options, errors);
            }
            for delegate in &mut proc.delegates {
                fold_delegate_const_arrays(delegate, const_values, options, errors);
            }
            for when in &mut proc.whens {
                fold_when_const_arrays(when, const_values, options, errors);
            }
            for stmt in &mut proc.init.body {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            for stmt in &mut proc.block_pre {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            for stmt in &mut proc.sample {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            for stmt in &mut proc.block_post {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            if let Some(graph) = &mut proc.graph {
                fold_graph_const_arrays(graph, const_values, options, errors);
            }
            for task in &mut proc.tasks {
                for stmt in &mut task.body {
                    fold_stmt_const_arrays(stmt, const_values, options, errors);
                }
            }
            for def in &mut proc.local_defs {
                fold_function_const_arrays(def, const_values, options, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

pub(super) fn reject_const_assignment_target(
    target: &AssignTarget,
    target_loc: &Span,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if let AssignTarget::Var(name) = target {
        if const_values.contains_key(name) {
            errors.push(Diagnostic::semantic_span(
                format!("cannot assign to constant '{name}'"),
                target_loc.as_ref(),
            ));
        }
    }
}

pub(super) fn reject_const_shadowing_name(
    symbol_kind: &str,
    name: &str,
    loc: Span,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(const_name) = visible_const_symbol_for_local_name(name, scope_ns, const_values) {
        errors.push(Diagnostic::semantic_span(
            format!("{symbol_kind} '{name}' conflicts with constant '{const_name}'"),
            loc.as_ref(),
        ));
    }
}

pub(super) fn reject_const_shadowing_stmt(
    stmt: &Stmt,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target, target_loc, ..
        } => {
            if let AssignTarget::Tuple(names) = target {
                for name in names.iter().filter_map(|target| target.binding()) {
                    reject_const_shadowing_name(
                        "tuple assignment target",
                        name,
                        *target_loc,
                        scope_ns,
                        const_values,
                        errors,
                    );
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
            for nested in else_branch {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
        }
        Stmt::For { var, loc, body, .. } => {
            reject_const_shadowing_name("loop variable", var, *loc, scope_ns, const_values, errors);
            for nested in body {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
        }
        Stmt::Const { .. }
        | Stmt::Expr { .. }
        | Stmt::Print { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => {}
    }
}

pub(super) fn reject_const_shadowing_function(
    def: &FunctionDef,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_const_shadowing_name(
            "function parameter",
            &param.name,
            param.loc,
            scope_ns,
            const_values,
            errors,
        );
    }
    for stmt in &def.body {
        reject_const_shadowing_stmt(stmt, scope_ns, const_values, errors);
    }
}

pub(super) fn reject_const_shadowing_event(
    event: &EventDef,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &event.params {
        reject_const_shadowing_name(
            "event parameter",
            &param.name,
            param.loc,
            scope_ns,
            const_values,
            errors,
        );
    }
    for stmt in &event.body {
        reject_const_shadowing_stmt(stmt, scope_ns, const_values, errors);
    }
}

pub(super) fn reject_const_shadowing_proc_decl(
    proc_name: &str,
    symbol_kind: &str,
    name: &str,
    loc: Span,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(const_name) = visible_const_symbol_for_local_name(name, scope_ns, const_values) {
        errors.push(Diagnostic::semantic_span(
            format!("{symbol_kind} '{name}' in processor '{proc_name}' conflicts with constant '{const_name}'"),
            loc.as_ref(),
        ));
    }
}

pub(super) fn reject_const_shadowing_in_program(
    program: &Program,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        match block {
            Block::Events(events) => {
                for event in &events.events {
                    let scope_ns = symbol_namespace(&event.name);
                    reject_const_shadowing_event(event, &scope_ns, const_values, errors);
                }
            }
            Block::Delegates(delegates) => {
                for delegate in &delegates.delegates {
                    let scope_ns = symbol_namespace(&delegate.name);
                    for param in &delegate.params {
                        if let Some(const_name) = visible_const_symbol_for_local_name(
                            &param.name,
                            &scope_ns,
                            const_values,
                        ) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "delegate parameter '{}' in '{}' conflicts with constant '{}'",
                                    param.name, delegate.name, const_name
                                ),
                                param.loc.as_ref(),
                            ));
                        }
                    }
                }
            }
            Block::When(when) => {
                for stmt in &when.body {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Tasks(tasks) => {
                for task in &tasks.tasks {
                    for stmt in &task.body {
                        reject_const_shadowing_stmt(stmt, "", const_values, errors);
                    }
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Block(block_exec) => {
                for stmt in &block_exec.pre {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
                if let Some(sample) = &block_exec.sample {
                    for stmt in &sample.body {
                        reject_const_shadowing_stmt(stmt, "", const_values, errors);
                    }
                }
                for stmt in &block_exec.post {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Sample(sample) => {
                for stmt in &sample.body {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Def(def) if !def.is_const => {
                let scope_ns = symbol_namespace(&def.name);
                reject_const_shadowing_function(def, &scope_ns, const_values, errors);
            }
            Block::Struct(struct_def) => {
                let scope_ns = symbol_namespace(&struct_def.name);
                for method in &struct_def.methods {
                    reject_const_shadowing_function(method, &scope_ns, const_values, errors);
                }
            }
            Block::Proc(proc) => {
                let scope_ns = symbol_namespace(&proc.name);
                for decl in &proc.ins {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor input",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for decl in &proc.outs {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor output",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for decl in &proc.params {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor parameter",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for decl in &proc.buffers {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor buffer",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for event in &proc.events {
                    reject_const_shadowing_event(event, &scope_ns, const_values, errors);
                }
                for delegate in &proc.delegates {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "delegate",
                        &delegate.name,
                        delegate.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for when in &proc.whens {
                    for stmt in &when.body {
                        reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                    }
                }
                for stmt in &proc.init.body {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for stmt in &proc.block_pre {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for stmt in &proc.sample {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for stmt in &proc.block_post {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for task in &proc.tasks {
                    for stmt in &task.body {
                        reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                    }
                }
                for def in &proc.local_defs {
                    reject_const_shadowing_function(def, &scope_ns, const_values, errors);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Graph(_)
            | Block::Const(_)
            | Block::Def(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
}

pub(super) fn reject_const_assignments_stmt(
    stmt: &Stmt,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target_loc, target, ..
        } => reject_const_assignment_target(target, target_loc, const_values, errors),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                reject_const_assignments_stmt(nested, const_values, errors);
            }
            for nested in else_branch {
                reject_const_assignments_stmt(nested, const_values, errors);
            }
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            for nested in body {
                reject_const_assignments_stmt(nested, const_values, errors);
            }
        }
        Stmt::Const { .. }
        | Stmt::Expr { .. }
        | Stmt::Print { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => {}
    }
}

pub(super) fn reject_const_assignments_function(
    def: &FunctionDef,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in &def.body {
        reject_const_assignments_stmt(stmt, const_values, errors);
    }
}

pub(super) fn reject_const_assignments_event(
    event: &EventDef,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in &event.body {
        reject_const_assignments_stmt(stmt, const_values, errors);
    }
}

pub(super) fn reject_const_assignments_in_program(
    program: &Program,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        match block {
            Block::Events(events) => {
                for event in &events.events {
                    reject_const_assignments_event(event, const_values, errors);
                }
            }
            Block::Delegates(_) => {}
            Block::When(when) => {
                for stmt in &when.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Tasks(tasks) => {
                for task in &tasks.tasks {
                    for stmt in &task.body {
                        reject_const_assignments_stmt(stmt, const_values, errors);
                    }
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Block(block_exec) => {
                for stmt in &block_exec.pre {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                if let Some(sample) = &block_exec.sample {
                    for stmt in &sample.body {
                        reject_const_assignments_stmt(stmt, const_values, errors);
                    }
                }
                for stmt in &block_exec.post {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Sample(sample) => {
                for stmt in &sample.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Def(def) => reject_const_assignments_function(def, const_values, errors),
            Block::Struct(struct_def) => {
                for method in &struct_def.methods {
                    reject_const_assignments_function(method, const_values, errors);
                }
            }
            Block::Proc(proc) => {
                for event in &proc.events {
                    reject_const_assignments_event(event, const_values, errors);
                }
                for when in &proc.whens {
                    for stmt in &when.body {
                        reject_const_assignments_stmt(stmt, const_values, errors);
                    }
                }
                for stmt in &proc.init.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for stmt in &proc.block_pre {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for stmt in &proc.sample {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for stmt in &proc.block_post {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for task in &proc.tasks {
                    for stmt in &task.body {
                        reject_const_assignments_stmt(stmt, const_values, errors);
                    }
                }
                for def in &proc.local_defs {
                    reject_const_assignments_function(def, const_values, errors);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Graph(_)
            | Block::Const(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
}

pub(super) fn const_def_registry(artifacts: &SemanticConstArtifacts) -> ConstDefRegistry<'_> {
    ConstDefRegistry {
        defs: &artifacts.const_defs,
        order: &artifacts.const_def_order,
    }
}

pub(super) fn fold_direct_const_def_call_expr(
    expr: &mut Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let direct_call = match expr {
        Expr::UserCall {
            name,
            args,
            type_args,
            ..
        } if artifacts.const_defs.contains_key(name) => Some((
            name.clone(),
            args.clone(),
            !type_args.is_empty(),
            expr.loc(),
        )),
        _ => None,
    };
    if let Some((name, args, has_type_args, loc)) = direct_call {
        if has_type_args {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: const def calls cannot use explicit type arguments"),
                loc,
            ));
            return;
        }
        let locals = HashMap::new();
        let local_arrays = HashMap::new();
        if let Some(value) = eval_const_def_call(
            &name,
            &args,
            &locals,
            &local_arrays,
            &artifacts.const_values,
            const_def_registry(artifacts),
            options,
            context,
            &mut Vec::new(),
            errors,
            loc,
        ) {
            *expr = match value {
                ConstEvalValue::Scalar(value) => typed_const_expr_with_loc(value, loc),
                ConstEvalValue::Array(array) => const_array_literal_expr(&array.values, loc),
            };
        }
        return;
    }

    match expr {
        Expr::Index { index, .. } => {
            fold_direct_const_def_call_expr(index, artifacts, options, context, errors);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                fold_direct_const_def_call_expr(coordinate, artifacts, options, context, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            fold_direct_const_def_call_expr(&mut spec.size, artifacts, options, context, errors);
            if let Some(init) = init {
                for value in init {
                    fold_direct_const_def_call_expr(value, artifacts, options, context, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            fold_direct_const_def_call_expr(lhs, artifacts, options, context, errors);
            fold_direct_const_def_call_expr(rhs, artifacts, options, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                fold_direct_const_def_call_expr(arg, artifacts, options, context, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                fold_direct_const_def_call_expr(&mut arg.expr, artifacts, options, context, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                fold_direct_const_def_call_expr(value, artifacts, options, context, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(super) fn fold_direct_const_def_decl_type(
    ty: &mut Option<DeclType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

pub(super) fn fold_direct_const_def_buffer_type(
    ty: &mut Option<BufferType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
    }
}

pub(super) fn fold_direct_const_def_fn_param_type(
    ty: &mut Option<FnParamType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        Some(FnParamType::Buffer(buffer_ty))
        | Some(FnParamType::BufferArray {
            buffer: buffer_ty, ..
        }) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
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
}

pub(super) fn fold_direct_const_def_event_param_type(
    ty: &mut EventParamType,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        _ => {}
    }
}

pub(super) fn fold_direct_const_def_field_type(
    ty: &mut FieldType,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        fold_direct_const_def_call_expr(&mut spec.size, artifacts, options, context, errors);
    }
}

pub(super) fn fold_direct_const_def_return_type(
    ty: &mut Option<FnReturnType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

pub(super) fn fold_direct_const_def_decl_range(
    range: &mut Option<DeclRange>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            fold_direct_const_def_call_expr(min, artifacts, options, context, errors);
        }
        fold_direct_const_def_call_expr(&mut range.max, artifacts, options, context, errors);
    }
}

pub(super) fn fold_direct_const_def_port_decl(
    decl: &mut PortDecl,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    fold_direct_const_def_decl_type(&mut decl.ty, artifacts, options, context, errors);
    if let Some(default) = &mut decl.default {
        fold_direct_const_def_call_expr(default, artifacts, options, context, errors);
    }
    fold_direct_const_def_decl_range(&mut decl.range, artifacts, options, context, errors);
}

pub(super) fn fold_direct_const_def_param_decl(
    decl: &mut ParamDecl,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    fold_direct_const_def_decl_type(&mut decl.ty, artifacts, options, context, errors);
    if let Some(default) = &mut decl.default {
        fold_direct_const_def_call_expr(default, artifacts, options, context, errors);
    }
    fold_direct_const_def_decl_range(&mut decl.range, artifacts, options, context, errors);
    for expr in [&mut decl.control.curve, &mut decl.control.step]
        .into_iter()
        .flatten()
    {
        fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
    }
}

pub(super) fn fold_direct_const_def_stmt(
    stmt: &mut Stmt,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                fold_direct_const_def_call_expr(
                    size,
                    artifacts,
                    options,
                    &format!("local const '{}' size", decl.name),
                    errors,
                );
            }
            fold_direct_const_def_call_expr(
                &mut decl.expr,
                artifacts,
                options,
                &format!("local const '{}'", decl.name),
                errors,
            );
        }
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Index { index, .. } => {
                    fold_direct_const_def_call_expr(
                        index,
                        artifacts,
                        options,
                        "assignment target index",
                        errors,
                    );
                }
                AssignTarget::Slice {
                    selector,
                    channel,
                    start,
                    end,
                    ..
                } => {
                    for coordinate in [selector, channel, start, end].into_iter().flatten() {
                        fold_direct_const_def_call_expr(
                            coordinate,
                            artifacts,
                            options,
                            "assignment target slice coordinate",
                            errors,
                        );
                    }
                }
                AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
            }
            fold_direct_const_def_call_expr(expr, artifacts, options, "assignment", errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_direct_const_def_call_expr(expr, artifacts, options, "expression", errors);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                fold_direct_const_def_call_expr(value, artifacts, options, "print value", errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_direct_const_def_call_expr(cond, artifacts, options, "if condition", errors);
            for nested in then_branch {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
            for nested in else_branch {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                fold_direct_const_def_call_expr(step, artifacts, options, "for loop step", errors);
            }
            fold_direct_const_def_call_expr(start, artifacts, options, "for loop start", errors);
            fold_direct_const_def_call_expr(end, artifacts, options, "for loop end", errors);
            for nested in body {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            fold_direct_const_def_call_expr(cond, artifacts, options, "while condition", errors);
            for nested in body {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn fold_direct_const_def_function(
    def: &mut FunctionDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        fold_direct_const_def_fn_param_type(
            &mut param.ty,
            artifacts,
            options,
            &format!("function '{}' parameter '{}'", def.name, param.name),
            errors,
        );
        if let Some(default) = &mut param.default {
            fold_direct_const_def_call_expr(
                default,
                artifacts,
                options,
                &format!("function '{}' parameter '{}'", def.name, param.name),
                errors,
            );
        }
    }
    fold_direct_const_def_return_type(
        &mut def.return_ty,
        artifacts,
        options,
        &format!("function '{}' return type", def.name),
        errors,
    );
    for stmt in &mut def.body {
        fold_direct_const_def_stmt(stmt, artifacts, options, errors);
    }
}

pub(super) fn fold_direct_const_def_event(
    event: &mut EventDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        fold_direct_const_def_event_param_type(
            &mut param.ty,
            artifacts,
            options,
            &format!("event '{}.{}'", event.name, param.name),
            errors,
        );
        if let Some(default) = &mut param.default {
            fold_direct_const_def_call_expr(
                default,
                artifacts,
                options,
                &format!("event '{}.{}'", event.name, param.name),
                errors,
            );
        }
    }
    for stmt in &mut event.body {
        fold_direct_const_def_stmt(stmt, artifacts, options, errors);
    }
}

pub(super) fn fold_direct_const_def_delegate(
    delegate: &mut DelegateDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut delegate.params {
        fold_direct_const_def_event_param_type(
            &mut param.ty,
            artifacts,
            options,
            &format!("delegate '{}.{}'", delegate.name, param.name),
            errors,
        );
        if let Some(default) = &mut param.default {
            fold_direct_const_def_call_expr(
                default,
                artifacts,
                options,
                &format!("delegate '{}.{}'", delegate.name, param.name),
                errors,
            );
        }
    }
}

pub(super) fn fold_direct_const_def_when(
    when: &mut WhenDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(index) = &mut when.target.index {
        fold_direct_const_def_call_expr(index, artifacts, options, "when target index", errors);
    }
    for stmt in &mut when.body {
        fold_direct_const_def_stmt(stmt, artifacts, options, errors);
    }
}

pub(super) fn fold_direct_const_def_graph(
    graph: &mut GraphBlock,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &mut graph.edges {
        fold_direct_const_def_call_expr(&mut edge.source, artifacts, options, "graph edge", errors);
        if let Some(delay) = &mut edge.delay {
            fold_direct_const_def_call_expr(delay, artifacts, options, "graph edge delay", errors);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_direct_const_def_call_expr(
                    index,
                    artifacts,
                    options,
                    "graph endpoint index",
                    errors,
                );
            }
        }
    }
}

pub(super) fn fold_direct_const_def_calls_in_block(
    block: &mut Block,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    structural_options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(default_ty) = &mut ports.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    "section default type",
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut ports.decls {
                fold_direct_const_def_port_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("port '{}'", decl.name),
                    errors,
                );
            }
        }
        Block::Params(params) => {
            if let Some(default_ty) = &mut params.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    "params default type",
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut params.decls {
                fold_direct_const_def_param_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("param '{}'", decl.name),
                    errors,
                );
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                fold_direct_const_def_event(event, artifacts, options, errors);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &mut delegates.delegates {
                fold_direct_const_def_delegate(delegate, artifacts, options, errors);
            }
        }
        Block::When(when) => {
            fold_direct_const_def_when(when, artifacts, options, errors);
        }
        Block::Tasks(tasks) => {
            for task in &mut tasks.tasks {
                for stmt in &mut task.body {
                    fold_direct_const_def_stmt(stmt, artifacts, options, errors);
                }
            }
        }
        Block::Buffers(buffers) => {
            if let Some(default_ty) = &mut buffers.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_buffer_type(
                    &mut ty,
                    artifacts,
                    options,
                    "buffers default type",
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut buffers.decls {
                fold_direct_const_def_buffer_type(
                    &mut decl.ty,
                    artifacts,
                    options,
                    &format!("buffer '{}'", decl.name),
                    errors,
                );
            }
        }
        Block::Init(init) => {
            for stmt in &mut init.body {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &mut block_exec.pre {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_direct_const_def_call_expr(
                        factor,
                        artifacts,
                        options,
                        "sample oversample factor",
                        errors,
                    );
                }
                for stmt in &mut sample.body {
                    fold_direct_const_def_stmt(stmt, artifacts, options, errors);
                }
            }
            for stmt in &mut block_exec.post {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_direct_const_def_call_expr(
                    factor,
                    artifacts,
                    options,
                    "sample oversample factor",
                    errors,
                );
            }
            for stmt in &mut sample.body {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
        }
        Block::Graph(graph) => {
            fold_direct_const_def_graph(graph, artifacts, options, errors);
        }
        Block::Assert(assert_decl) => {
            fold_direct_const_def_call_expr(
                &mut assert_decl.expr,
                artifacts,
                options,
                "assert condition",
                errors,
            );
        }
        Block::Def(def) if !def.is_const => {
            fold_direct_const_def_function(def, artifacts, options, errors);
        }
        Block::Struct(struct_def) => {
            for field in &mut struct_def.fields {
                fold_direct_const_def_field_type(
                    &mut field.ty,
                    artifacts,
                    options,
                    &format!("struct '{}' field '{}'", struct_def.name, field.name),
                    errors,
                );
                if let Some(default) = &mut field.default {
                    fold_direct_const_def_call_expr(
                        default,
                        artifacts,
                        options,
                        &format!("struct '{}' field '{}'", struct_def.name, field.name),
                        errors,
                    );
                }
            }
            for method in &mut struct_def.methods {
                fold_direct_const_def_function(method, artifacts, options, errors);
            }
        }
        Block::Proc(proc) => {
            if let Some(default_ty) = &mut proc.ins_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' input default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(default_ty) = &mut proc.outs_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' output default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(default_ty) = &mut proc.params_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' param default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(default_ty) = &mut proc.buffers_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_buffer_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' buffer default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut proc.ins {
                fold_direct_const_def_port_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("processor '{}' input '{}'", proc.name, decl.name),
                    errors,
                );
            }
            for decl in &mut proc.outs {
                fold_direct_const_def_port_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("processor '{}' output '{}'", proc.name, decl.name),
                    errors,
                );
            }
            for decl in &mut proc.params {
                fold_direct_const_def_param_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("processor '{}' param '{}'", proc.name, decl.name),
                    errors,
                );
            }
            for decl in &mut proc.buffers {
                fold_direct_const_def_buffer_type(
                    &mut decl.ty,
                    artifacts,
                    options,
                    &format!("processor '{}' buffer '{}'", proc.name, decl.name),
                    errors,
                );
            }
            if let Some(default_ty) = &mut proc.init.default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' init default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(factor) = &mut proc.sample_oversample_factor {
                fold_direct_const_def_call_expr(
                    factor,
                    artifacts,
                    structural_options,
                    &format!("processor '{}' sample oversample factor", proc.name),
                    errors,
                );
            }
            for event in &mut proc.events {
                fold_direct_const_def_event(event, artifacts, options, errors);
            }
            for delegate in &mut proc.delegates {
                fold_direct_const_def_delegate(delegate, artifacts, options, errors);
            }
            for when in &mut proc.whens {
                fold_direct_const_def_when(when, artifacts, options, errors);
            }
            for stmt in &mut proc.init.body {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            for stmt in &mut proc.block_pre {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            for stmt in &mut proc.sample {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            for stmt in &mut proc.block_post {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            if let Some(graph) = &mut proc.graph {
                fold_direct_const_def_graph(graph, artifacts, options, errors);
            }
            for def in &mut proc.local_defs {
                fold_direct_const_def_function(def, artifacts, options, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

pub(super) fn fold_local_scalar_const_expr(
    expr: &mut Expr,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    let loc = expr.loc();
    if let Expr::Var { name, .. } = expr {
        if let Some(value) = local_consts.get(name).copied() {
            *expr = typed_const_expr_with_loc(value, loc);
            return;
        }
    }

    match expr {
        Expr::Index { index, .. } => {
            fold_local_scalar_const_expr(index, local_consts);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                fold_local_scalar_const_expr(coordinate, local_consts);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            fold_local_scalar_const_expr(&mut spec.size, local_consts);
            if let Some(init) = init {
                for value in init {
                    fold_local_scalar_const_expr(value, local_consts);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            fold_local_scalar_const_expr(lhs, local_consts);
            fold_local_scalar_const_expr(rhs, local_consts);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                fold_local_scalar_const_expr(arg, local_consts);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                fold_local_scalar_const_expr(&mut arg.expr, local_consts);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            fold_local_scalar_const_expr(expr, local_consts);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                fold_local_scalar_const_expr(value, local_consts);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(super) fn fold_local_scalar_const_decl_type(
    ty: &mut Option<DeclType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

pub(super) fn fold_local_scalar_const_buffer_type(
    ty: &mut Option<BufferType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        fold_local_scalar_const_expr(expr, local_consts);
    }
}

pub(super) fn fold_local_scalar_const_fn_param_type(
    ty: &mut Option<FnParamType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        Some(FnParamType::Buffer(buffer_ty))
        | Some(FnParamType::BufferArray {
            buffer: buffer_ty, ..
        }) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                fold_local_scalar_const_expr(expr, local_consts);
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
}

pub(super) fn fold_local_scalar_const_event_param_type(
    ty: &mut EventParamType,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        _ => {}
    }
}

pub(super) fn fold_local_scalar_const_field_type(
    ty: &mut FieldType,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let FieldType::Array(spec) = ty {
        fold_local_scalar_const_expr(&mut spec.size, local_consts);
    }
}

pub(super) fn fold_local_scalar_const_return_type(
    ty: &mut Option<FnReturnType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

pub(super) fn fold_local_scalar_const_decl_range(
    range: &mut Option<DeclRange>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            fold_local_scalar_const_expr(min, local_consts);
        }
        fold_local_scalar_const_expr(&mut range.max, local_consts);
    }
}

pub(super) fn fold_local_scalar_const_port_decl(
    decl: &mut PortDecl,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    fold_local_scalar_const_decl_type(&mut decl.ty, local_consts);
    if let Some(default) = &mut decl.default {
        fold_local_scalar_const_expr(default, local_consts);
    }
    fold_local_scalar_const_decl_range(&mut decl.range, local_consts);
}

pub(super) fn fold_local_scalar_const_param_decl(
    decl: &mut ParamDecl,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    fold_local_scalar_const_decl_type(&mut decl.ty, local_consts);
    if let Some(default) = &mut decl.default {
        fold_local_scalar_const_expr(default, local_consts);
    }
    fold_local_scalar_const_decl_range(&mut decl.range, local_consts);
    for expr in [&mut decl.control.curve, &mut decl.control.step]
        .into_iter()
        .flatten()
    {
        fold_local_scalar_const_expr(expr, local_consts);
    }
}

pub(super) fn eval_local_scalar_const_decl(
    decl: &onda_frontend::ConstDecl,
    local_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context_prefix: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    if is_builtin_constant_name(&decl.name) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "constant name '{}' is reserved as a builtin constant",
                decl.name
            ),
            decl.loc.as_ref(),
        ));
        return None;
    }

    if is_const_array_decl(decl) {
        errors.push(Diagnostic::semantic_span(
            "const arrays are only supported at top-level and namespace scope",
            decl.loc.as_ref(),
        ));
        return None;
    }

    let expected_ty = match &decl.ty {
        Some(ConstType::Scalar(ty)) => Some(*ty),
        Some(ConstType::Array { .. } | ConstType::Slice { .. }) => {
            errors.push(Diagnostic::semantic_span(
                "const arrays are only supported at top-level and namespace scope",
                decl.loc.as_ref(),
            ));
            return None;
        }
        None => None,
    };

    let local_arrays = HashMap::new();
    let context = format!("{context_prefix} local const '{}'", decl.name);
    let ty = match expected_ty {
        Some(ty) => ty,
        None => infer_const_decl_scalar_type_with_defs(
            &decl.expr,
            local_consts,
            &local_arrays,
            &artifacts.const_values,
            const_def_registry(artifacts),
            options,
            &context,
            &mut Vec::new(),
            errors,
        )?,
    };
    eval_const_scalar_expr_with_defs(
        &decl.expr,
        ty,
        local_consts,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        &context,
        &mut Vec::new(),
        errors,
    )
}

pub(super) fn proc_sample_oversample_factor_for_proc_context(
    proc: &ProcessorDef,
    proc_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
) -> usize {
    let Some(factor) = proc.sample_oversample_factor.as_ref() else {
        return 1;
    };
    let local_arrays = HashMap::new();
    let mut scratch = Vec::<Diagnostic>::new();
    let Some(folded) = fold_const_eval_expr(
        factor,
        proc_consts,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        &format!("processor '{}' sample oversample factor", proc.name),
        &mut Vec::new(),
        &mut scratch,
    ) else {
        return 1;
    };
    validated_sample_oversample_factor(
        Some(&folded),
        options,
        &format!("processor '{}' sample oversample factor", proc.name),
        &mut scratch,
    )
}

pub(super) fn preprocess_local_const_stmt(
    stmt: &mut Stmt,
    local_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target_loc,
            target,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if local_consts.contains_key(name) {
                        errors.push(Diagnostic::semantic_span(
                            format!("cannot assign to constant '{name}'"),
                            target_loc.as_ref(),
                        ));
                    }
                }
                AssignTarget::Index { index, .. } => {
                    fold_local_scalar_const_expr(index, local_consts);
                }
                AssignTarget::Slice {
                    selector,
                    channel,
                    start,
                    end,
                    ..
                } => {
                    for coordinate in [selector, channel, start, end].into_iter().flatten() {
                        fold_local_scalar_const_expr(coordinate, local_consts);
                    }
                }
                AssignTarget::Tuple(names) => {
                    for name in names.iter().filter_map(|target| target.binding()) {
                        if local_consts.contains_key(name) {
                            errors.push(Diagnostic::semantic_span(
                                format!("cannot assign to constant '{name}'"),
                                target_loc.as_ref(),
                            ));
                        }
                    }
                }
            }
            fold_local_scalar_const_expr(expr, local_consts);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_local_scalar_const_expr(expr, local_consts);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                fold_local_scalar_const_expr(value, local_consts);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_local_scalar_const_expr(cond, local_consts);
            preprocess_local_const_stmts(
                then_branch,
                local_consts,
                artifacts,
                options,
                "if branch",
                errors,
            );
            preprocess_local_const_stmts(
                else_branch,
                local_consts,
                artifacts,
                options,
                "else branch",
                errors,
            );
        }
        Stmt::For {
            loc,
            var,
            step,
            start,
            end,
            body,
            ..
        } => {
            if local_consts.contains_key(var) {
                errors.push(Diagnostic::semantic_span(
                    format!("loop variable '{var}' conflicts with local constant '{var}'"),
                    loc.as_ref(),
                ));
            }
            if let Some(step) = step {
                fold_local_scalar_const_expr(step, local_consts);
            }
            fold_local_scalar_const_expr(start, local_consts);
            fold_local_scalar_const_expr(end, local_consts);
            preprocess_local_const_stmts(
                body,
                local_consts,
                artifacts,
                options,
                "for loop",
                errors,
            );
        }
        Stmt::While { cond, body, .. } => {
            fold_local_scalar_const_expr(cond, local_consts);
            preprocess_local_const_stmts(
                body,
                local_consts,
                artifacts,
                options,
                "while loop",
                errors,
            );
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn preprocess_local_const_stmts(
    stmts: &mut Vec<Stmt>,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context_prefix: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let mut scope_consts = inherited_consts.clone();
    let mut local_names = HashSet::<String>::new();
    let mut rewritten = Vec::<Stmt>::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        if let Stmt::Const { decl, .. } = &stmt {
            if !local_names.insert(decl.name.clone()) {
                errors.push(Diagnostic::semantic_span(
                    format!("duplicate constant '{}' in scope", decl.name),
                    decl.loc.as_ref(),
                ));
                continue;
            }
            if let Some(value) = eval_local_scalar_const_decl(
                decl,
                &scope_consts,
                artifacts,
                options,
                context_prefix,
                errors,
            ) {
                scope_consts.insert(decl.name.clone(), value);
            }
            continue;
        }
        preprocess_local_const_stmt(&mut stmt, &scope_consts, artifacts, options, errors);
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

pub(super) fn preprocess_local_const_function(
    def: &mut FunctionDef,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        if inherited_consts.contains_key(&param.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "function parameter '{}' in '{}' conflicts with local constant '{}'",
                    param.name, def.name, param.name
                ),
                param.loc.as_ref(),
            ));
        }
        fold_local_scalar_const_fn_param_type(&mut param.ty, inherited_consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, inherited_consts);
        }
    }
    fold_local_scalar_const_return_type(&mut def.return_ty, inherited_consts);
    preprocess_local_const_stmts(
        &mut def.body,
        inherited_consts,
        artifacts,
        options,
        &format!("function '{}'", def.name),
        errors,
    );
}

pub(super) fn preprocess_local_const_event(
    event: &mut EventDef,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        if inherited_consts.contains_key(&param.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "event parameter '{}' in '{}' conflicts with local constant '{}'",
                    param.name, event.name, param.name
                ),
                param.loc.as_ref(),
            ));
        }
        fold_local_scalar_const_event_param_type(&mut param.ty, inherited_consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, inherited_consts);
        }
    }
    preprocess_local_const_stmts(
        &mut event.body,
        inherited_consts,
        artifacts,
        options,
        &format!("event '{}'", event.name),
        errors,
    );
}

pub(super) fn preprocess_local_const_delegate(
    delegate: &mut DelegateDef,
    inherited_consts: &HashMap<String, TypedConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut delegate.params {
        if inherited_consts.contains_key(&param.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "delegate parameter '{}' in '{}' conflicts with local constant '{}'",
                    param.name, delegate.name, param.name
                ),
                param.loc.as_ref(),
            ));
        }
        fold_local_scalar_const_event_param_type(&mut param.ty, inherited_consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, inherited_consts);
        }
    }
}

pub(super) fn preprocess_local_const_when(
    when: &mut WhenDef,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(index) = &mut when.target.index {
        fold_local_scalar_const_expr(index, inherited_consts);
    }
    preprocess_local_const_stmts(
        &mut when.body,
        inherited_consts,
        artifacts,
        options,
        "when handler",
        errors,
    );
}

pub(super) fn preprocess_local_const_graph(
    graph: &mut GraphBlock,
    inherited_consts: &HashMap<String, TypedConstValue>,
) {
    for edge in &mut graph.edges {
        fold_local_scalar_const_expr(&mut edge.source, inherited_consts);
        if let Some(delay) = &mut edge.delay {
            fold_local_scalar_const_expr(delay, inherited_consts);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_local_scalar_const_expr(index, inherited_consts);
            }
        }
    }
}

pub(super) fn reject_proc_local_const_decl_name(
    proc_name: &str,
    symbol_kind: &str,
    name: &str,
    loc: Span,
    proc_const_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if proc_const_names.contains(name) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{symbol_kind} '{name}' in processor '{proc_name}' conflicts with local constant '{name}'"
            ),
            loc.as_ref(),
        ));
    }
}

pub(super) fn reject_proc_local_const_decl_conflicts(
    proc: &ProcessorDef,
    proc_const_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for task in &proc.tasks {
        reject_proc_local_const_decl_name(
            &proc.name,
            "task",
            &task.name,
            task.loc,
            proc_const_names,
            errors,
        );
    }
    for decl in &proc.ins {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor input",
            &decl.name,
            decl.loc,
            proc_const_names,
            errors,
        );
    }
    for decl in &proc.outs {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor output",
            &decl.name,
            decl.loc,
            proc_const_names,
            errors,
        );
    }
    for decl in &proc.params {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor parameter",
            &decl.name,
            decl.loc,
            proc_const_names,
            errors,
        );
    }
    for decl in &proc.buffers {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor buffer",
            &decl.name,
            decl.loc,
            proc_const_names,
            errors,
        );
    }
}

pub(super) fn preprocess_proc_local_const_decls(
    proc_name: &str,
    consts: &[onda_frontend::ConstDecl],
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ProcLocalConstArtifacts {
    let mut out = ProcLocalConstArtifacts::default();
    for decl in consts {
        if !out.names.insert(decl.name.clone()) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate proc constant '{}'", decl.name),
                decl.loc.as_ref(),
            ));
            continue;
        }
        if let Some(value) = eval_local_scalar_const_decl(
            decl,
            &out.values,
            artifacts,
            options,
            &format!("processor '{proc_name}'"),
            errors,
        ) {
            out.values.insert(decl.name.clone(), value);
        }
    }
    out
}

pub(super) fn preprocess_proc_local_consts(
    proc: &mut ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ProcLocalConstArtifacts {
    let out =
        preprocess_proc_local_const_decls(&proc.name, &proc.consts, artifacts, options, errors);
    proc.consts.clear();
    out
}

pub(super) fn preprocess_local_consts_in_block(
    block: &mut Block,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let empty_consts = HashMap::<String, TypedConstValue>::new();
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(default_ty) = &mut ports.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut ports.decls {
                fold_local_scalar_const_port_decl(decl, &empty_consts);
            }
        }
        Block::Params(params) => {
            if let Some(default_ty) = &mut params.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut params.decls {
                fold_local_scalar_const_param_decl(decl, &empty_consts);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(default_ty) = &mut buffers.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_buffer_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut buffers.decls {
                fold_local_scalar_const_buffer_type(&mut decl.ty, &empty_consts);
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                preprocess_local_const_event(event, &empty_consts, artifacts, options, errors);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &mut delegates.delegates {
                preprocess_local_const_delegate(delegate, &empty_consts, errors);
            }
        }
        Block::When(when) => {
            preprocess_local_const_when(when, &empty_consts, artifacts, options, errors)
        }
        Block::Tasks(tasks) => {
            for task in &mut tasks.tasks {
                preprocess_local_const_stmts(
                    &mut task.body,
                    &empty_consts,
                    artifacts,
                    options,
                    &format!("top-level task '{}'", task.name),
                    errors,
                );
            }
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            preprocess_local_const_stmts(
                &mut init.body,
                &empty_consts,
                artifacts,
                options,
                "init block",
                errors,
            );
        }
        Block::Block(block_exec) => {
            preprocess_local_const_stmts(
                &mut block_exec.pre,
                &empty_consts,
                artifacts,
                options,
                "block pre",
                errors,
            );
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_local_scalar_const_expr(factor, &empty_consts);
                }
                preprocess_local_const_stmts(
                    &mut sample.body,
                    &empty_consts,
                    artifacts,
                    options,
                    "sample block",
                    errors,
                );
            }
            preprocess_local_const_stmts(
                &mut block_exec.post,
                &empty_consts,
                artifacts,
                options,
                "block post",
                errors,
            );
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_local_scalar_const_expr(factor, &empty_consts);
            }
            preprocess_local_const_stmts(
                &mut sample.body,
                &empty_consts,
                artifacts,
                options,
                "sample block",
                errors,
            );
        }
        Block::Graph(graph) => {
            preprocess_local_const_graph(graph, &empty_consts);
        }
        Block::Assert(assert_decl) => {
            fold_local_scalar_const_expr(&mut assert_decl.expr, &empty_consts);
        }
        Block::Def(def) if !def.is_const => {
            preprocess_local_const_function(def, &empty_consts, artifacts, options, errors);
        }
        Block::Struct(struct_def) => {
            for field in &mut struct_def.fields {
                fold_local_scalar_const_field_type(&mut field.ty, &empty_consts);
                if let Some(default) = &mut field.default {
                    fold_local_scalar_const_expr(default, &empty_consts);
                }
            }
            for method in &mut struct_def.methods {
                preprocess_local_const_function(method, &empty_consts, artifacts, options, errors);
            }
        }
        Block::Proc(proc) => {
            let factor_proc_consts = {
                let mut scratch_errors = Vec::new();
                preprocess_proc_local_const_decls(
                    &proc.name,
                    &proc.consts,
                    artifacts,
                    options,
                    &mut scratch_errors,
                )
            };
            let sample_oversample_factor = proc_sample_oversample_factor_for_proc_context(
                proc,
                &factor_proc_consts.values,
                artifacts,
                options,
            );
            let proc_options = proc_runtime_analysis_options(options, sample_oversample_factor);
            let proc_consts = preprocess_proc_local_consts(proc, artifacts, proc_options, errors);
            reject_proc_local_const_decl_conflicts(proc, &proc_consts.names, errors);
            if let Some(count) = &mut proc.ins_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.ins_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(count) = &mut proc.outs_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.outs_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(count) = &mut proc.params_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.params_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(count) = &mut proc.buffers_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.buffers_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_buffer_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut proc.ins {
                fold_local_scalar_const_port_decl(decl, &proc_consts.values);
            }
            for decl in &mut proc.outs {
                fold_local_scalar_const_port_decl(decl, &proc_consts.values);
            }
            for decl in &mut proc.params {
                fold_local_scalar_const_param_decl(decl, &proc_consts.values);
            }
            for decl in &mut proc.buffers {
                fold_local_scalar_const_buffer_type(&mut decl.ty, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.init.default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(factor) = &mut proc.sample_oversample_factor {
                fold_local_scalar_const_expr(factor, &factor_proc_consts.values);
            }
            for event in &mut proc.events {
                preprocess_local_const_event(
                    event,
                    &proc_consts.values,
                    artifacts,
                    proc_options,
                    errors,
                );
            }
            for delegate in &mut proc.delegates {
                preprocess_local_const_delegate(delegate, &proc_consts.values, errors);
            }
            for when in &mut proc.whens {
                preprocess_local_const_when(
                    when,
                    &proc_consts.values,
                    artifacts,
                    proc_options,
                    errors,
                );
            }
            preprocess_local_const_stmts(
                &mut proc.init.body,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' init", proc.name),
                errors,
            );
            preprocess_local_const_stmts(
                &mut proc.block_pre,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' block pre", proc.name),
                errors,
            );
            preprocess_local_const_stmts(
                &mut proc.sample,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' sample", proc.name),
                errors,
            );
            preprocess_local_const_stmts(
                &mut proc.block_post,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' block post", proc.name),
                errors,
            );
            for task in &mut proc.tasks {
                preprocess_local_const_stmts(
                    &mut task.body,
                    &proc_consts.values,
                    artifacts,
                    proc_options,
                    &format!("task '{}' in processor '{}'", task.name, proc.name),
                    errors,
                );
            }
            if let Some(graph) = &mut proc.graph {
                preprocess_local_const_graph(graph, &proc_consts.values);
            }
            for def in &mut proc.local_defs {
                preprocess_local_const_function(
                    def,
                    &proc_consts.values,
                    artifacts,
                    proc_options,
                    errors,
                );
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

pub(super) fn eval_count_shorthand(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let folded = fold_const_eval_expr(
        expr,
        &locals,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        context,
        &mut Vec::new(),
        errors,
    )?;
    eval_data_size_expr(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_port_count_shorthand(
    decls: &mut Vec<PortDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(PortDecl {
                loc,
                name: format!("{prefix}{idx}"),
                output_timing: None,
                output_timing_loc: Span::ZERO,
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_param_count_shorthand(
    decls: &mut Vec<ParamDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(ParamDecl {
                loc,
                name: format!("{prefix}{idx}"),
                private: false,
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
                control: Default::default(),
                bind: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_buffer_count_shorthand(
    decls: &mut Vec<BufferDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<BufferType>,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(BufferDecl {
                loc,
                name: format!("buf{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                array_size: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

pub(super) fn expand_proc_count_shorthand(
    proc: &mut ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let loc = proc.loc;
    let proc_name = proc.name.clone();
    expand_port_count_shorthand(
        &mut proc.ins,
        &mut proc.ins_deferred_count,
        &mut proc.ins_deferred_default_ty,
        "in",
        &format!("processor '{proc_name}' ins"),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_port_count_shorthand(
        &mut proc.outs,
        &mut proc.outs_deferred_count,
        &mut proc.outs_deferred_default_ty,
        match proc.outs_timing {
            OutputTiming::Sample => "out",
            OutputTiming::Block => "kout",
        },
        &format!(
            "processor '{proc_name}' {}",
            match proc.outs_timing {
                OutputTiming::Sample => "outs",
                OutputTiming::Block => "kouts",
            }
        ),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_param_count_shorthand(
        &mut proc.params,
        &mut proc.params_deferred_count,
        &mut proc.params_deferred_default_ty,
        "param",
        &format!("processor '{proc_name}' params"),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_buffer_count_shorthand(
        &mut proc.buffers,
        &mut proc.buffers_deferred_count,
        &mut proc.buffers_deferred_default_ty,
        &format!("processor '{proc_name}' buffers"),
        loc,
        artifacts,
        options,
        errors,
    );
}

pub(super) fn proc_options_for_count_expansion(
    proc: &ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
) -> AnalysisOptions {
    let empty_proc_consts = HashMap::new();
    let sample_oversample_factor = proc_sample_oversample_factor_for_proc_context(
        proc,
        &empty_proc_consts,
        artifacts,
        options,
    );
    proc_runtime_analysis_options(options, sample_oversample_factor)
}

pub(super) fn coerce_consts_and_expand_counts(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> SemanticConstArtifacts {
    let mut artifacts = SemanticConstArtifacts::default();
    let mut seen = HashSet::<String>::new();
    let ordinary_symbols = ordinary_top_level_symbol_names(program);
    let mut future_const_symbols = top_level_const_symbol_names(program);
    for name in &ordinary_symbols {
        future_const_symbols.remove(name);
    }
    for block in &mut program.blocks {
        if let Block::Const(decl) = block {
            future_const_symbols.remove(&decl.name);
        }
        let preprocess_local_consts = match block {
            Block::Const(_) => false,
            Block::Def(def) if def.is_const => false,
            _ => true,
        };
        if preprocess_local_consts {
            preprocess_local_consts_in_block(block, &artifacts, options, errors);
        }
        match block {
            Block::Def(def) if def.is_const => {
                if is_builtin_constant_name(&def.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def name '{}' is reserved as a builtin constant",
                            def.name
                        ),
                        def.loc,
                    ));
                    continue;
                }
                if ordinary_symbols.contains(&def.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def name '{}' conflicts with existing symbol",
                            def.name
                        ),
                        def.loc,
                    ));
                    continue;
                }
                if !seen.insert(def.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate const symbol '{}'", def.name),
                        def.loc,
                    ));
                    continue;
                }
                artifacts
                    .const_def_order
                    .insert(def.name.clone(), artifacts.const_def_order.len());
                artifacts.const_defs.insert(def.name.clone(), def.clone());
                validate_const_def_declaration(def, options, &artifacts, errors);
            }
            Block::Const(decl) => {
                if is_builtin_constant_name(&decl.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "constant name '{}' is reserved as a builtin constant",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                if ordinary_symbols.contains(&decl.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "constant name '{}' conflicts with existing symbol",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                if !seen.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate const symbol '{}'", decl.name),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                let force_const_array = is_const_array_decl(decl)
                    || (decl.ty.is_none()
                        && is_known_const_array_initializer(
                            &decl.expr,
                            &artifacts.const_values,
                            &artifacts.const_defs,
                        ));
                if force_const_array {
                    if let Some(array) = coerce_const_array(
                        decl,
                        options,
                        &artifacts.const_values,
                        &artifacts.const_defs,
                        &artifacts.const_def_order,
                        errors,
                    ) {
                        record_const_array_artifact(&mut artifacts, array);
                    }
                } else {
                    let inferred_const_array = if decl.ty.is_none() {
                        let mut probe_errors = Vec::new();
                        coerce_const_array(
                            decl,
                            options,
                            &artifacts.const_values,
                            &artifacts.const_defs,
                            &artifacts.const_def_order,
                            &mut probe_errors,
                        )
                    } else {
                        None
                    };
                    if let Some(array) = inferred_const_array {
                        record_const_array_artifact(&mut artifacts, array);
                    } else if let Some(value) = coerce_const_scalar(
                        decl,
                        options,
                        &artifacts.const_values,
                        &artifacts.const_defs,
                        &artifacts.const_def_order,
                        errors,
                    ) {
                        artifacts
                            .const_values
                            .insert(decl.name.clone(), ConstValue::Scalar(value));
                    }
                }
            }
            Block::Ins(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "ins",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Outs(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "outs",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::KOuts(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "kouts",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Params(params) => {
                let prefix = params.deferred_prefix.clone();
                let block_label = if prefix == "kin" { "kins" } else { "params" };
                expand_param_count_shorthand(
                    &mut params.decls,
                    &mut params.deferred_count,
                    &mut params.deferred_default_ty,
                    &prefix,
                    block_label,
                    params.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Buffers(buffers) => {
                expand_buffer_count_shorthand(
                    &mut buffers.decls,
                    &mut buffers.deferred_count,
                    &mut buffers.deferred_default_ty,
                    "buffers",
                    buffers.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Proc(proc) => {
                let proc_options = proc_options_for_count_expansion(proc, &artifacts, options);
                expand_proc_count_shorthand(proc, &artifacts, proc_options, errors);
                fold_direct_const_def_calls_in_block(
                    block,
                    &artifacts,
                    proc_options,
                    options,
                    errors,
                );
            }
            _ => fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors),
        }
        let const_array_options = match block {
            Block::Proc(proc) => proc_options_for_count_expansion(proc, &artifacts, options),
            _ => options,
        };
        fold_const_array_exprs_in_block(
            block,
            &artifacts.const_values,
            const_array_options,
            options,
            errors,
        );
        reject_forward_const_refs_in_block(
            block,
            &artifacts.const_values,
            &future_const_symbols,
            errors,
        );
    }
    artifacts
}

pub(super) fn compile_const_descriptors(
    program: &Program,
    artifacts: &SemanticConstArtifacts,
) -> Vec<CompileConstDescriptor> {
    program
        .blocks
        .iter()
        .filter_map(|block| {
            let Block::Const(decl) = block else {
                return None;
            };
            if !decl.configurable {
                return None;
            }
            let value = artifacts.const_values.get(&decl.name)?.clone();
            let kind = match (&decl.ty, &value) {
                (Some(ConstType::Scalar(_)), ConstValue::Scalar(_)) => CompileConstKind::Scalar,
                (Some(ConstType::Array { .. }), ConstValue::Array { .. }) => {
                    CompileConstKind::FixedArray
                }
                (Some(ConstType::Slice { .. }), ConstValue::Array { .. }) => {
                    CompileConstKind::Array
                }
                _ => return None,
            };
            Some(CompileConstDescriptor {
                name: decl.name.clone(),
                kind,
                value,
            })
        })
        .collect()
}

pub(super) fn coerce_const_array(
    decl: &onda_frontend::ConstDecl,
    options: AnalysisOptions,
    const_values: &HashMap<String, ConstValue>,
    const_defs: &HashMap<String, FunctionDef>,
    const_def_order: &HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstArray> {
    let (decl_elem_ty, decl_len) = match &decl.ty {
        Some(ConstType::Array { elem, size }) => {
            let context = format!("const array '{}' size", decl.name);
            let locals = HashMap::new();
            let local_arrays = HashMap::new();
            let const_defs = ConstDefRegistry {
                defs: const_defs,
                order: const_def_order,
            };
            let len = eval_const_array_size_with_defs(
                size,
                &locals,
                &local_arrays,
                const_values,
                const_defs,
                options,
                &context,
                &mut Vec::new(),
                errors,
            )?;
            (Some(*elem), Some(len))
        }
        Some(ConstType::Slice { elem }) => (Some(*elem), None),
        Some(ConstType::Scalar(_)) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const array '{}' cannot use a scalar type annotation",
                    decl.name
                ),
                decl.loc.as_ref(),
            ));
            (None, None)
        }
        None => (None, None),
    };

    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let const_defs = ConstDefRegistry {
        defs: const_defs,
        order: const_def_order,
    };
    let expected = match (decl_elem_ty, decl_len) {
        (Some(elem_ty), Some(len)) => ConstArrayExpectation::fixed(elem_ty, len),
        (Some(elem_ty), None) => ConstArrayExpectation::elem(elem_ty),
        (None, Some(len)) => ConstArrayExpectation {
            elem_ty: None,
            len: Some(len),
        },
        (None, None) => ConstArrayExpectation::any(),
    };
    let context = format!("const array '{}'", decl.name);
    let array = eval_const_array_expr_with_defs(
        &decl.expr,
        expected,
        &locals,
        &local_arrays,
        const_values,
        const_defs,
        options,
        &context,
        &mut Vec::new(),
        errors,
    )?;

    Some(TypedConstArray {
        name: decl.name.clone(),
        elem_ty: array.elem_ty,
        len: array.len(),
        values: array.values,
    })
}

pub(super) fn coerce_const_scalar(
    decl: &onda_frontend::ConstDecl,
    options: AnalysisOptions,
    const_values: &HashMap<String, ConstValue>,
    const_defs: &HashMap<String, FunctionDef>,
    const_def_order: &HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let expected_ty = match &decl.ty {
        Some(ConstType::Scalar(ty)) => Some(*ty),
        Some(ConstType::Array { .. } | ConstType::Slice { .. }) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const scalar '{}' cannot use an array type annotation",
                    decl.name
                ),
                decl.loc.as_ref(),
            ));
            None
        }
        None => None,
    };

    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let const_defs = ConstDefRegistry {
        defs: const_defs,
        order: const_def_order,
    };
    let context = format!("const scalar '{}'", decl.name);
    let ty = match expected_ty {
        Some(ty) => ty,
        None => infer_const_decl_scalar_type_with_defs(
            &decl.expr,
            &locals,
            &local_arrays,
            const_values,
            const_defs,
            options,
            &context,
            &mut Vec::new(),
            errors,
        )?,
    };
    eval_const_scalar_expr_with_defs(
        &decl.expr,
        ty,
        &locals,
        &local_arrays,
        const_values,
        const_defs,
        options,
        &context,
        &mut Vec::new(),
        errors,
    )
}
