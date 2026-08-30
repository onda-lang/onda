use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_interface_views(
    inputs_enabled: bool,
    inputs: &[String],
    input_types: &HashMap<String, PrimitiveType>,
    input_arrays: &HashMap<String, TypedArrayInfo>,
    audio_outputs_enabled: bool,
    audio_outputs: &[String],
    audio_output_types: &HashMap<String, PrimitiveType>,
    audio_output_arrays: &HashMap<String, TypedArrayInfo>,
    control_outputs_enabled: bool,
    control_outputs: &[String],
    control_output_types: &HashMap<String, PrimitiveType>,
    control_output_arrays: &HashMap<String, TypedArrayInfo>,
    params_enabled: bool,
    params: &[String],
    param_types: &HashMap<String, PrimitiveType>,
    param_arrays: &HashMap<String, TypedArrayInfo>,
) -> Result<ResolvedInterfaceViews, String> {
    Ok(ResolvedInterfaceViews {
        inputs: resolve_interface_view(inputs_enabled, "input", inputs, input_types, input_arrays)?,
        audio_outputs: resolve_interface_view(
            audio_outputs_enabled,
            "audio output",
            audio_outputs,
            audio_output_types,
            audio_output_arrays,
        )?,
        control_outputs: resolve_interface_view(
            control_outputs_enabled,
            "control output",
            control_outputs,
            control_output_types,
            control_output_arrays,
        )?,
        params: resolve_interface_view(
            params_enabled,
            "parameter",
            params,
            param_types,
            param_arrays,
        )?,
    })
}

pub(super) fn resolve_interface_view(
    enabled: bool,
    kind: &str,
    names: &[String],
    types: &HashMap<String, PrimitiveType>,
    arrays: &HashMap<String, TypedArrayInfo>,
) -> Result<Option<ResolvedInterfaceView>, String> {
    if !enabled || names.is_empty() {
        return Ok(None);
    }

    let mut arrays = arrays.iter().collect::<Vec<_>>();
    arrays.sort_by_key(|(name, info)| (info.offset, name.as_str()));
    let mut array_index = 0usize;
    let mut cursor = 0usize;
    let mut slots = Vec::with_capacity(names.len());
    let mut element_type = None;
    let mut uniform = true;

    while cursor < names.len() {
        if let Some((root, info)) = arrays.get(array_index).copied() {
            if info.offset < cursor {
                return Err(format!(
                    "resolved {kind} array '{root}' overlaps another interface slot"
                ));
            }
            if info.offset == cursor {
                let end = cursor.checked_add(info.len).ok_or_else(|| {
                    format!("resolved {kind} array '{root}' slot range overflows")
                })?;
                if end > names.len() {
                    return Err(format!(
                        "resolved {kind} array '{root}' exceeds the flattened interface"
                    ));
                }
                for element in 0..info.len {
                    let flattened = &names[cursor + element];
                    if types.get(flattened).copied() != Some(info.elem_ty) {
                        return Err(format!(
                            "resolved {kind} array '{root}' element {element} has inconsistent type metadata"
                        ));
                    }
                    if let Some(first) = element_type {
                        uniform &= first == info.elem_ty;
                    } else {
                        element_type = Some(info.elem_ty);
                    }
                    let id = u32::try_from(slots.len()).map_err(|_| {
                        format!("resolved {kind} interface exceeds the u32 slot ID space")
                    })?;
                    slots.push(ResolvedInterfaceSlot {
                        id: InterfaceSlotId::new(id),
                        root: (*root).clone(),
                        element: Some(element),
                    });
                }
                cursor = end;
                array_index += 1;
                continue;
            }
        }

        let name = &names[cursor];
        let ty = types
            .get(name)
            .copied()
            .ok_or_else(|| format!("resolved {kind} slot '{name}' has no primitive type"))?;
        if let Some(first) = element_type {
            uniform &= first == ty;
        } else {
            element_type = Some(ty);
        }
        let id = u32::try_from(slots.len())
            .map_err(|_| format!("resolved {kind} interface exceeds the u32 slot ID space"))?;
        slots.push(ResolvedInterfaceSlot {
            id: InterfaceSlotId::new(id),
            root: name.clone(),
            element: None,
        });
        cursor += 1;
    }

    if array_index != arrays.len() {
        return Err(format!(
            "resolved {kind} interface contains an unreachable array descriptor"
        ));
    }
    if !uniform {
        return Ok(None);
    }
    Ok(Some(ResolvedInterfaceView {
        element_type: element_type.expect("non-empty interface has an element type"),
        slots,
    }))
}

pub(super) fn requires_entry_sample(program: &Program) -> bool {
    program.blocks.iter().any(|block| {
        matches!(
            block.kind(),
            BlockKind::Ins
                | BlockKind::Outs
                | BlockKind::KOuts
                | BlockKind::Params
                | BlockKind::Events
                | BlockKind::Buffers
                | BlockKind::Init
                | BlockKind::Block
                | BlockKind::Sample
                | BlockKind::Graph
        )
    })
}

pub(super) fn record_top_level_proc_arg_oversample_factor(
    instance_name: &str,
    sample_oversample_factor: usize,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(instance) = top_level_proc_rewrite
        .global_proc_instances
        .get(instance_name)
    else {
        return;
    };
    let Some(api) = proc_api.get(&instance.proc_name) else {
        return;
    };
    let proc_oversample_factor = api.sample_oversample_factor.max(1);
    if sample_oversample_factor > 1 && proc_oversample_factor > 1 {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "cannot pass explicitly oversampled processor '{}' (sample {}) into oversampled context (sample {})",
                instance.proc_name, proc_oversample_factor, sample_oversample_factor
            ),
        );
    }
    let effective_factor = if proc_oversample_factor > 1 {
        proc_oversample_factor
    } else {
        sample_oversample_factor.max(1)
    };
    if effective_factor <= 1 {
        return;
    }
    if let Some(previous) = out.get(instance_name).copied() {
        if previous != effective_factor {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "processor instance '{}' is required at both sample {} and sample {}; a physical processor instance can only have one effective oversampling rate",
                    instance_name, previous, effective_factor
                ),
            );
        }
        return;
    }
    out.insert(instance_name.to_owned(), effective_factor);
}

pub(super) fn record_top_level_proc_array_arg_oversample_factor(
    base: &str,
    sample_oversample_factor: usize,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(slots) = top_level_proc_rewrite.global_proc_array_slots.get(base) else {
        return;
    };
    for slot in slots {
        record_top_level_proc_arg_oversample_factor(
            slot,
            sample_oversample_factor,
            top_level_proc_rewrite,
            proc_api,
            out,
            errors,
        );
    }
    let mut slot_factors = slots.iter().filter_map(|slot| out.get(slot).copied());
    let Some(first) = slot_factors.next() else {
        return;
    };
    if slots
        .iter()
        .all(|slot| out.get(slot).copied() == Some(first))
    {
        out.insert(base.to_owned(), first);
    }
}

pub(super) fn resolved_def_call_arg<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_idx: usize,
) -> Option<&'a Expr> {
    let param_name = param_names.get(param_idx)?;
    if let Some(named) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(param_name.as_str()))
    {
        return Some(&named.expr);
    }
    let mut positional_idx = 0usize;
    for arg in args {
        if arg.name.is_some() {
            continue;
        }
        if positional_idx == param_idx {
            return Some(&arg.expr);
        }
        positional_idx += 1;
    }
    None
}

pub(super) fn collect_def_proc_arg_oversample_factors_from_expr(
    expr: &Expr,
    sample_oversample_factor: usize,
    defs_by_name: &HashMap<String, &TypedFunction>,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if let Some(def) = defs_by_name.get(name) {
                for (param_idx, kind) in def.param_kinds.iter().enumerate() {
                    let Some(arg_expr) = resolved_def_call_arg(args, &def.params, param_idx) else {
                        continue;
                    };
                    match (kind, arg_expr) {
                        (TypedFnParam::ProcArray { .. }, Expr::Var { name: base, .. }) => {
                            record_top_level_proc_array_arg_oversample_factor(
                                base,
                                sample_oversample_factor,
                                top_level_proc_rewrite,
                                proc_api,
                                out,
                                errors,
                            );
                        }
                        (TypedFnParam::Struct { struct_name }, Expr::Var { name, .. }) => {
                            let Some(instance) =
                                top_level_proc_rewrite.global_proc_instances.get(name)
                            else {
                                continue;
                            };
                            if &instance.proc_name == struct_name {
                                record_top_level_proc_arg_oversample_factor(
                                    name,
                                    sample_oversample_factor,
                                    top_level_proc_rewrite,
                                    proc_api,
                                    out,
                                    errors,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            for arg in args {
                collect_def_proc_arg_oversample_factors_from_expr(
                    &arg.expr,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => collect_def_proc_arg_oversample_factors_from_expr(
            index,
            sample_oversample_factor,
            defs_by_name,
            top_level_proc_rewrite,
            proc_api,
            out,
            errors,
        ),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_def_proc_arg_oversample_factors_from_expr(
                    coordinate,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_def_proc_arg_oversample_factors_from_expr(
                &spec.size,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    collect_def_proc_arg_oversample_factors_from_expr(
                        value,
                        sample_oversample_factor,
                        defs_by_name,
                        top_level_proc_rewrite,
                        proc_api,
                        out,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_def_proc_arg_oversample_factors_from_expr(
                lhs,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
            collect_def_proc_arg_oversample_factors_from_expr(
                rhs,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
        }
        Expr::Call { args, .. }
        | Expr::ArrayLiteral { values: args, .. }
        | Expr::Tuple { values: args, .. } => {
            for arg in args {
                collect_def_proc_arg_oversample_factors_from_expr(
                    arg,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_def_proc_arg_oversample_factors_from_expr(
                expr,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(super) fn collect_def_proc_arg_oversample_factors_from_stmts(
    stmts: &[Stmt],
    sample_oversample_factor: usize,
    defs_by_name: &HashMap<String, &TypedFunction>,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Print { values, .. } => {
                for value in values {
                    collect_def_proc_arg_oversample_factors_from_expr(
                        value,
                        sample_oversample_factor,
                        defs_by_name,
                        top_level_proc_rewrite,
                        proc_api,
                        out,
                        errors,
                    );
                }
            }
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    expr,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    cond,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_stmts(
                    then_branch,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_stmts(
                    else_branch,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    start,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_expr(
                    end,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                if let Some(step) = step {
                    collect_def_proc_arg_oversample_factors_from_expr(
                        step,
                        sample_oversample_factor,
                        defs_by_name,
                        top_level_proc_rewrite,
                        proc_api,
                        out,
                        errors,
                    );
                }
                collect_def_proc_arg_oversample_factors_from_stmts(
                    body,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    cond,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_stmts(
                    body,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
    }
}

/// Pre-monomorphization validation of generic def call-site type arguments.
/// Checks for bool type args and type arg count mismatches BEFORE mono rewrites calls.
pub(super) fn validate_generic_def_type_args_in_stmts(
    stmts: &[Stmt],
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        validate_generic_def_type_args_in_stmt(stmt, fn_signatures, errors);
    }
}

pub(super) fn validate_generic_def_type_args_in_stmt(
    stmt: &Stmt,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            validate_generic_def_type_args_in_assign_target(target, fn_signatures, errors);
            validate_generic_def_type_args_in_expr(expr, fn_signatures, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            validate_generic_def_type_args_in_expr(expr, fn_signatures, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            validate_generic_def_type_args_in_expr(cond, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(then_branch, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(else_branch, fn_signatures, errors);
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            validate_generic_def_type_args_in_expr(start, fn_signatures, errors);
            validate_generic_def_type_args_in_expr(end, fn_signatures, errors);
            if let Some(step) = step {
                validate_generic_def_type_args_in_expr(step, fn_signatures, errors);
            }
            validate_generic_def_type_args_in_stmts(body, fn_signatures, errors);
        }
        Stmt::While { cond, body, .. } => {
            validate_generic_def_type_args_in_expr(cond, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(body, fn_signatures, errors);
        }
        _ => {}
    }
}

pub(super) fn validate_generic_def_type_args_in_assign_target(
    target: &AssignTarget,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match target {
        AssignTarget::Index { index, .. } => {
            validate_generic_def_type_args_in_expr(index, fn_signatures, errors);
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                validate_generic_def_type_args_in_expr(coordinate, fn_signatures, errors);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

pub(super) fn validate_generic_def_type_args_in_expr(
    expr: &Expr,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            if let Some(sig) = fn_signatures.get(name.as_str()) {
                let display_name = sig.display_name.as_deref().unwrap_or(name);
                if !type_args.is_empty() && !sig.type_params.is_empty() {
                    if type_args.len() != sig.type_params.len() {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "function '{}' expects {} type arguments, got {}",
                                display_name,
                                sig.type_params.len(),
                                type_args.len()
                            ),
                            expr.loc(),
                        ));
                    }
                    for ta in type_args {
                        if matches!(ta, CallTypeArg::Primitive(PrimitiveType::Bool)) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "'bool' is not valid as a generic type argument for '{}'; use f32, f64, i32, or i64",
                                    display_name
                                ),
                                expr.loc(),
                            ));
                        }
                    }
                }
            }
            for arg in args {
                validate_generic_def_type_args_in_expr(&arg.expr, fn_signatures, errors);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_generic_def_type_args_in_expr(arg, fn_signatures, errors);
            }
        }
        Expr::Binary { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            validate_generic_def_type_args_in_expr(lhs, fn_signatures, errors);
            validate_generic_def_type_args_in_expr(rhs, fn_signatures, errors);
        }
        Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. }
        | Expr::Cast { expr: inner, .. } => {
            validate_generic_def_type_args_in_expr(inner, fn_signatures, errors);
        }
        Expr::Tuple { values, .. } | Expr::ArrayLiteral { values, .. } => {
            for v in values {
                validate_generic_def_type_args_in_expr(v, fn_signatures, errors);
            }
        }
        Expr::Index { index, .. } => {
            validate_generic_def_type_args_in_expr(index, fn_signatures, errors);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                validate_generic_def_type_args_in_expr(coordinate, fn_signatures, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            validate_generic_def_type_args_in_expr(&spec.size, fn_signatures, errors);
            if let Some(values) = init {
                for value in values {
                    validate_generic_def_type_args_in_expr(value, fn_signatures, errors);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn collect_reachable_typed_def_names(
    init: &[Stmt],
    block_pre: &[Stmt],
    sample: &[Stmt],
    block_post: &[Stmt],
    events: &[TypedEvent],
    defs: &[TypedFunction],
) -> HashSet<String> {
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let def_names = def_map.keys().cloned().collect::<HashSet<_>>();
    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();

    seed_called_typed_defs_from_stmts(init, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(block_pre, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(sample, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(block_post, &def_names, &mut pending, &mut seen_pending);
    for event in events {
        seed_called_typed_defs_from_stmts(&event.body, &def_names, &mut pending, &mut seen_pending);
    }

    let mut reachable = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(def) = def_map.get(&name) else {
            continue;
        };
        seed_called_typed_defs_from_stmts(&def.body, &def_names, &mut pending, &mut seen_pending);
        seed_called_typed_defs_from_defaults(
            &def.param_defaults,
            &def_names,
            &mut pending,
            &mut seen_pending,
        );
    }
    reachable
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum RuntimeCallVisit {
    Visiting,
    Complete,
}

/// Runtime recursion is incompatible with Onda's bounded realtime execution
/// contract and with fixed per-instance aggregate scratch. Reject direct and
/// mutual call cycles after overload resolution and specialization, when every
/// call already names its concrete callee.
pub(super) fn reject_recursive_runtime_defs(defs: &[TypedFunction], errors: &mut Vec<Diagnostic>) {
    let def_names = defs
        .iter()
        .map(|def| def.name.clone())
        .collect::<HashSet<_>>();
    let mut edges = HashMap::<String, Vec<String>>::with_capacity(defs.len());
    for def in defs {
        let mut callees = Vec::new();
        let mut seen = HashSet::new();
        seed_called_typed_defs_from_stmts(&def.body, &def_names, &mut callees, &mut seen);
        seed_called_typed_defs_from_defaults(
            &def.param_defaults,
            &def_names,
            &mut callees,
            &mut seen,
        );
        callees.sort();
        edges.insert(def.name.clone(), callees);
    }

    let mut names = def_names.into_iter().collect::<Vec<_>>();
    names.sort();
    let mut visits = HashMap::<String, RuntimeCallVisit>::new();
    let mut path = Vec::<String>::new();
    for name in names {
        if visits.contains_key(&name) {
            continue;
        }
        if let Some(cycle) = find_runtime_call_cycle(&name, &edges, &mut visits, &mut path) {
            let diagnostic = defs
                .iter()
                .find(|def| def.name == cycle[0])
                .and_then(|def| def.body.first())
                .map(|statement| DiagCtx::new(statement.loc()))
                .unwrap_or_default();
            push_semantic(
                diagnostic,
                errors,
                format!(
                    "recursive runtime def cycle is not realtime-safe: {}",
                    cycle.join(" -> ")
                ),
            );
            return;
        }
    }
}

pub(super) fn find_runtime_call_cycle(
    name: &str,
    edges: &HashMap<String, Vec<String>>,
    visits: &mut HashMap<String, RuntimeCallVisit>,
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    if visits.get(name) == Some(&RuntimeCallVisit::Complete) {
        return None;
    }
    if visits.get(name) == Some(&RuntimeCallVisit::Visiting) {
        let start = path.iter().position(|entry| entry == name).unwrap_or(0);
        let mut cycle = path[start..].to_vec();
        cycle.push(name.to_owned());
        return Some(cycle);
    }

    visits.insert(name.to_owned(), RuntimeCallVisit::Visiting);
    path.push(name.to_owned());
    for callee in edges.get(name).into_iter().flatten() {
        if let Some(cycle) = find_runtime_call_cycle(callee, edges, visits, path) {
            return Some(cycle);
        }
    }
    path.pop();
    visits.insert(name.to_owned(), RuntimeCallVisit::Complete);
    None
}

pub(super) fn collect_reachable_def_names(
    init: &[Stmt],
    block_exec: &[Stmt],
    sample_and_event_exec: &[Stmt],
    defs: &[FunctionDef],
) -> HashSet<String> {
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let def_names = def_map.keys().cloned().collect::<HashSet<_>>();
    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();

    seed_called_typed_defs_from_stmts(init, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(block_exec, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(
        sample_and_event_exec,
        &def_names,
        &mut pending,
        &mut seen_pending,
    );

    let mut reachable = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(def) = def_map.get(&name) else {
            continue;
        };
        seed_called_typed_defs_from_stmts(&def.body, &def_names, &mut pending, &mut seen_pending);
        let defaults = def
            .params
            .iter()
            .map(|param| param.default.clone())
            .collect::<Vec<_>>();
        seed_called_typed_defs_from_defaults(
            &defaults,
            &def_names,
            &mut pending,
            &mut seen_pending,
        );
    }
    reachable
}

pub(super) fn def_is_block_generated_root(name: &str) -> bool {
    name.ends_with(PROC_BLOCK_PRE_FN_SUFFIX) || name.ends_with(PROC_BLOCK_POST_FN_SUFFIX)
}

pub(super) fn def_is_neither_phase_generated_root(name: &str) -> bool {
    name.ends_with(PROC_INIT_FN_SUFFIX) || name.contains(PROC_EVENT_FN_PREFIX)
}

pub(super) fn proc_name_for_lowered_proc_call(name: &str) -> Option<&str> {
    if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
        return Some(step_proc);
    }
    let (call_proc, out_idx_raw) = name.rsplit_once(PROC_CALL_OUT_FN_PREFIX)?;
    out_idx_raw.parse::<usize>().ok()?;
    Some(call_proc)
}

pub(super) fn lowered_proc_call_timing(
    name: &str,
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
) -> Option<OutputTiming> {
    if let Some(timing) = generated_proc_call_timing.get(name).copied() {
        return Some(timing);
    }
    let proc_name = proc_name_for_lowered_proc_call(name)?;
    proc_api.get(proc_name).map(|api| api.outputs.timing)
}

pub(super) fn generated_proc_call_timing_map(
    proc_api: &HashMap<String, ProcApi>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
) -> HashMap<String, OutputTiming> {
    let mut out = HashMap::<String, OutputTiming>::new();
    for (owner_proc, shape) in lowering_shapes {
        for (nested_var, nested) in &shape.state.nested_procs {
            let Some(api) = proc_api.get(&nested.proc_name) else {
                continue;
            };
            out.insert(
                nested_step_fn_name(owner_proc, nested_var),
                api.outputs.timing,
            );
            for out_idx in 0..api.outputs.names.len() {
                out.insert(
                    nested_call_out_fn_name(owner_proc, nested_var, out_idx),
                    api.outputs.timing,
                );
            }
        }
    }
    out
}

pub(super) fn collect_proc_call_diags_from_expr(
    expr: &Expr,
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_proc_call_diags_from_expr(value, proc_api, generated_proc_call_timing, out);
            }
        }
        Expr::Index { index, .. } => {
            collect_proc_call_diags_from_expr(index, proc_api, generated_proc_call_timing, out)
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_proc_call_diags_from_expr(
                    coordinate,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_proc_call_diags_from_expr(
                &spec.size,
                proc_api,
                generated_proc_call_timing,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_proc_call_diags_from_expr(
                        value,
                        proc_api,
                        generated_proc_call_timing,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_proc_call_diags_from_expr(lhs, proc_api, generated_proc_call_timing, out);
            collect_proc_call_diags_from_expr(rhs, proc_api, generated_proc_call_timing, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_proc_call_diags_from_expr(arg, proc_api, generated_proc_call_timing, out);
            }
        }
        Expr::UserCall {
            name, args, loc, ..
        } => {
            if let Some(timing) =
                lowered_proc_call_timing(name, proc_api, generated_proc_call_timing)
            {
                out.push((DiagCtx::new(*loc), timing));
            }
            for arg in args {
                collect_proc_call_diags_from_expr(
                    &arg.expr,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_proc_call_diags_from_expr(expr, proc_api, generated_proc_call_timing, out);
        }
    }
}

pub(super) fn collect_proc_call_diags_from_stmts(
    stmts: &[Stmt],
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => collect_proc_call_diags_from_expr(
                &decl.expr,
                proc_api,
                generated_proc_call_timing,
                out,
            ),
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Print { values, .. } => {
                for value in values {
                    collect_proc_call_diags_from_expr(
                        value,
                        proc_api,
                        generated_proc_call_timing,
                        out,
                    );
                }
            }
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_proc_call_diags_from_expr(expr, proc_api, generated_proc_call_timing, out);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_proc_call_diags_from_expr(cond, proc_api, generated_proc_call_timing, out);
                collect_proc_call_diags_from_stmts(
                    then_branch,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
                collect_proc_call_diags_from_stmts(
                    else_branch,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_proc_call_diags_from_expr(start, proc_api, generated_proc_call_timing, out);
                collect_proc_call_diags_from_expr(end, proc_api, generated_proc_call_timing, out);
                if let Some(step) = step {
                    collect_proc_call_diags_from_expr(
                        step,
                        proc_api,
                        generated_proc_call_timing,
                        out,
                    );
                }
                collect_proc_call_diags_from_stmts(body, proc_api, generated_proc_call_timing, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_proc_call_diags_from_expr(cond, proc_api, generated_proc_call_timing, out);
                collect_proc_call_diags_from_stmts(body, proc_api, generated_proc_call_timing, out);
            }
        }
    }
}

pub(super) fn push_proc_call_phase_errors(
    stmts: &[Stmt],
    context: &str,
    allowed: Option<OutputTiming>,
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut diags = Vec::<(DiagCtx, OutputTiming)>::new();
    collect_proc_call_diags_from_stmts(stmts, proc_api, generated_proc_call_timing, &mut diags);
    for (diag, timing) in diags {
        if Some(timing) == allowed {
            continue;
        }
        let required = match timing {
            OutputTiming::Sample => "sample",
            OutputTiming::Block => "block",
        };
        push_semantic(
            diag,
            errors,
            format!("proc operator '()' for {required}-rate proc is only allowed in {required}; found use in {context}"),
        );
    }
}

pub(super) fn collect_reachable_defs_for_phase(
    roots: &[&[Stmt]],
    generated_root: impl Fn(&str) -> bool,
    defs: &[FunctionDef],
    def_names: &HashSet<String>,
    def_map: &HashMap<String, &FunctionDef>,
) -> HashSet<String> {
    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();
    for root in roots {
        seed_called_typed_defs_from_stmts(root, def_names, &mut pending, &mut seen_pending);
    }
    for def in defs {
        if generated_root(&def.name) && seen_pending.insert(def.name.clone()) {
            pending.push(def.name.clone());
        }
    }

    let mut reachable = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(def) = def_map.get(&name) else {
            continue;
        };
        seed_called_typed_defs_from_stmts(&def.body, def_names, &mut pending, &mut seen_pending);
        let defaults = def
            .params
            .iter()
            .map(|param| param.default.clone())
            .collect::<Vec<_>>();
        seed_called_typed_defs_from_defaults(&defaults, def_names, &mut pending, &mut seen_pending);
    }
    reachable
}

pub(super) fn reject_non_sample_proc_operator_calls(
    init: &[Stmt],
    block_pre: &[Stmt],
    sample: &[Stmt],
    block_post: &[Stmt],
    events: &[TypedEvent],
    defs: &[FunctionDef],
    proc_api: &HashMap<String, ProcApi>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    errors: &mut Vec<Diagnostic>,
) {
    let generated_proc_call_timing = generated_proc_call_timing_map(proc_api, lowering_shapes);
    push_proc_call_phase_errors(
        init,
        "init",
        None,
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    push_proc_call_phase_errors(
        block_pre,
        "block pre",
        Some(OutputTiming::Block),
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    push_proc_call_phase_errors(
        sample,
        "sample",
        Some(OutputTiming::Sample),
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    push_proc_call_phase_errors(
        block_post,
        "block post",
        Some(OutputTiming::Block),
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    for event in events {
        push_proc_call_phase_errors(
            &event.body,
            &format!("event '{}'", event.name),
            None,
            proc_api,
            &generated_proc_call_timing,
            errors,
        );
    }

    let def_names = defs
        .iter()
        .map(|def| def.name.clone())
        .collect::<HashSet<_>>();
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let mut defs_with_proc_calls = HashMap::<String, Vec<(DiagCtx, OutputTiming)>>::new();
    for def in defs {
        let mut proc_call_diags = Vec::<(DiagCtx, OutputTiming)>::new();
        collect_proc_call_diags_from_stmts(
            &def.body,
            proc_api,
            &generated_proc_call_timing,
            &mut proc_call_diags,
        );
        if !proc_call_diags.is_empty() {
            defs_with_proc_calls.insert(def.name.clone(), proc_call_diags);
        }
    }

    let block_reachable_defs = collect_reachable_defs_for_phase(
        &[block_pre, block_post],
        |name| {
            def_is_block_generated_root(name)
                || lowered_proc_call_timing(name, proc_api, &generated_proc_call_timing)
                    == Some(OutputTiming::Block)
        },
        defs,
        &def_names,
        &def_map,
    );
    let sample_reachable_defs = collect_reachable_defs_for_phase(
        &[sample],
        |name| {
            lowered_proc_call_timing(name, proc_api, &generated_proc_call_timing)
                == Some(OutputTiming::Sample)
        },
        defs,
        &def_names,
        &def_map,
    );
    let mut neither_roots = Vec::<&[Stmt]>::new();
    neither_roots.push(init);
    for event in events {
        neither_roots.push(&event.body);
    }
    let neither_reachable_defs = collect_reachable_defs_for_phase(
        &neither_roots,
        def_is_neither_phase_generated_root,
        defs,
        &def_names,
        &def_map,
    );

    for (def_name, proc_call_diags) in defs_with_proc_calls {
        for (diag, timing) in proc_call_diags {
            let allowed = match timing {
                OutputTiming::Sample => {
                    sample_reachable_defs.contains(&def_name)
                        && !block_reachable_defs.contains(&def_name)
                        && !neither_reachable_defs.contains(&def_name)
                }
                OutputTiming::Block => {
                    block_reachable_defs.contains(&def_name)
                        && !sample_reachable_defs.contains(&def_name)
                        && !neither_reachable_defs.contains(&def_name)
                }
            };
            if allowed {
                continue;
            }
            let required = match timing {
                OutputTiming::Sample => "sample",
                OutputTiming::Block => "block",
            };
            push_semantic(
                diag,
                errors,
                format!("proc operator '()' for {required}-rate proc is only allowed in {required}; call in '{def_name}' is not provably {required}-only"),
            );
        }
    }
}

pub(super) fn is_array_param_type(ty: Option<&FnParamType>) -> bool {
    matches!(
        ty,
        Some(FnParamType::Array(_))
            | Some(FnParamType::ArrayGeneric(_))
            | Some(FnParamType::SizedArray { .. })
    )
}

pub(super) fn initial_readonly_array_param_candidates(def: &FunctionDef) -> HashSet<String> {
    def.params
        .iter()
        .filter(|param| is_array_param_type(param.ty.as_ref()))
        .map(|param| param.name.clone())
        .collect()
}

pub(super) fn readonly_alias_source(
    expr: &Expr,
    aliases: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Var { name, .. } => aliases.get(name).cloned(),
        Expr::Slice { base, .. } => aliases.get(base).cloned(),
        _ => None,
    }
}

pub(super) fn mark_readonly_param_expr_uses_as_mutable(
    expr: &Expr,
    aliases: &HashMap<String, String>,
    fn_signatures: &HashMap<String, FnSignature>,
    readonly_params: &HashMap<String, HashSet<String>>,
    mutable_params: &mut HashSet<String>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if let Some(sig) = fn_signatures.get(name) {
                let mut ignored = Vec::new();
                let resolved = resolve_call_args_at(
                    args,
                    &sig.params,
                    &sig.defaults,
                    sig.params.first().map(String::as_str) == Some("self"),
                    false,
                    &format!("function '{name}' call"),
                    expr.loc(),
                    &mut ignored,
                );
                if ignored.is_empty() {
                    for (idx, arg) in resolved.into_iter().enumerate() {
                        let Some(arg) = arg else {
                            continue;
                        };
                        let Some(source_param) = readonly_alias_source(arg, aliases) else {
                            continue;
                        };
                        let callee_param_name = sig.params.get(idx).map(String::as_str);
                        let callee_param_ty = sig.param_types.get(idx).and_then(|ty| ty.as_ref());
                        let callee_param_readonly = callee_param_name.is_some_and(|param| {
                            sig.readonly_array_params.contains(param)
                                || readonly_params
                                    .get(name)
                                    .is_some_and(|params| params.contains(param))
                        });
                        if is_array_param_type(callee_param_ty) && !callee_param_readonly {
                            mutable_params.insert(source_param.to_owned());
                        }
                    }
                }
            }

            for arg in args {
                mark_readonly_param_expr_uses_as_mutable(
                    &arg.expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Expr::Call { args, .. }
        | Expr::ArrayLiteral { values: args, .. }
        | Expr::Tuple { values: args, .. } => {
            for arg in args {
                mark_readonly_param_expr_uses_as_mutable(
                    arg,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                lhs,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            mark_readonly_param_expr_uses_as_mutable(
                rhs,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                expr,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Expr::Index { index, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                index,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                mark_readonly_param_expr_uses_as_mutable(
                    coordinate,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                &spec.size,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            if let Some(init) = init {
                for value in init {
                    mark_readonly_param_expr_uses_as_mutable(
                        value,
                        aliases,
                        fn_signatures,
                        readonly_params,
                        mutable_params,
                    );
                }
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(super) fn mark_readonly_param_stmt_uses_as_mutable(
    stmt: &Stmt,
    aliases: &mut HashMap<String, String>,
    fn_signatures: &HashMap<String, FnSignature>,
    readonly_params: &HashMap<String, HashSet<String>>,
    mutable_params: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => match target {
            AssignTarget::Var(name) => {
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
                if let Some(source_param) = readonly_alias_source(expr, aliases) {
                    aliases.insert(name.clone(), source_param.to_owned());
                } else {
                    aliases.remove(name);
                }
            }
            AssignTarget::Index { base, index } => {
                if let Some(source_param) = aliases.get(base) {
                    mutable_params.insert(source_param.clone());
                }
                mark_readonly_param_expr_uses_as_mutable(
                    index,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            AssignTarget::Slice {
                base,
                selector,
                channel,
                start,
                end,
            } => {
                if let Some(source_param) = aliases.get(base) {
                    mutable_params.insert(source_param.clone());
                }
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    mark_readonly_param_expr_uses_as_mutable(
                        coordinate,
                        aliases,
                        fn_signatures,
                        readonly_params,
                        mutable_params,
                    );
                }
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            AssignTarget::Tuple(_) => {
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        },
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                expr,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Stmt::Print { values, .. } => {
            for value in values {
                mark_readonly_param_expr_uses_as_mutable(
                    value,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Stmt::Const { decl, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                &decl.expr,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            mark_readonly_param_expr_uses_as_mutable(
                cond,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            let mut then_aliases = aliases.clone();
            for stmt in then_branch {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut then_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            let mut else_aliases = aliases.clone();
            for stmt in else_branch {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut else_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            aliases.extend(then_aliases);
            aliases.extend(else_aliases);
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                mark_readonly_param_expr_uses_as_mutable(
                    step,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            mark_readonly_param_expr_uses_as_mutable(
                start,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            mark_readonly_param_expr_uses_as_mutable(
                end,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            let mut loop_aliases = aliases.clone();
            for stmt in body {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut loop_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            aliases.extend(loop_aliases);
        }
        Stmt::While { cond, body, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                cond,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            let mut loop_aliases = aliases.clone();
            for stmt in body {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut loop_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            aliases.extend(loop_aliases);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn infer_readonly_array_params_for_def(
    def: &FunctionDef,
    fn_signatures: &HashMap<String, FnSignature>,
    readonly_params: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let candidates = initial_readonly_array_param_candidates(def);
    if candidates.is_empty() {
        return candidates;
    }
    let mut aliases = candidates
        .iter()
        .map(|name| (name.clone(), name.clone()))
        .collect::<HashMap<_, _>>();
    let mut mutable_params = HashSet::<String>::new();
    for stmt in &def.body {
        mark_readonly_param_stmt_uses_as_mutable(
            stmt,
            &mut aliases,
            fn_signatures,
            readonly_params,
            &mut mutable_params,
        );
    }
    candidates
        .into_iter()
        .filter(|param| !mutable_params.contains(param))
        .collect()
}

pub(super) fn update_readonly_array_param_signatures(
    defs: &[FunctionDef],
    fn_signatures: &mut HashMap<String, FnSignature>,
) {
    let mut readonly_params = defs
        .iter()
        .map(|def| {
            (
                def.name.clone(),
                initial_readonly_array_param_candidates(def),
            )
        })
        .collect::<HashMap<_, _>>();

    loop {
        let mut changed = false;
        for def in defs {
            let inferred =
                infer_readonly_array_params_for_def(def, fn_signatures, &readonly_params);
            let entry = readonly_params.entry(def.name.clone()).or_default();
            if *entry != inferred {
                *entry = inferred;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for (name, params) in readonly_params {
        if let Some(sig) = fn_signatures.get_mut(&name) {
            sig.readonly_array_params = params;
        }
    }
}

pub(super) fn def_has_concrete_param_contract(
    def: &FunctionDef,
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> bool {
    def.params.iter().enumerate().all(|(idx, param)| {
        if idx == 0 && method_self_struct.contains_key(&def.name) {
            return true;
        }
        match param.ty.as_ref() {
            Some(FnParamType::Primitive(_)) | Some(FnParamType::Tuple(_)) => true,
            Some(FnParamType::Struct(struct_name)) => {
                !def.type_params.contains(struct_name) && struct_defs.contains_key(struct_name)
            }
            Some(FnParamType::Buffer(buffer_ty)) => {
                matches!(buffer_ty.elem, BufferElemType::Primitive(_))
            }
            Some(FnParamType::BufferArray { buffer, .. }) => {
                matches!(buffer.elem, BufferElemType::Primitive(_))
            }
            Some(FnParamType::Array(Some(_))) => true,
            Some(FnParamType::SizedArray { elem: Some(_), .. }) => true,
            Some(FnParamType::ArrayGeneric(struct_name)) => {
                !def.type_params.contains(struct_name) && struct_defs.contains_key(struct_name)
            }
            Some(FnParamType::SizedArray {
                generic_name: Some(struct_name),
                ..
            }) => !def.type_params.contains(struct_name) && struct_defs.contains_key(struct_name),
            Some(FnParamType::Array(None))
            | Some(FnParamType::BareBuffer)
            | Some(FnParamType::SizedArray { .. })
            | None => false,
        }
    })
}

pub(super) fn try_indexed_proc_call_meta_in_def<'a>(
    expr: &'a Expr,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a [CallArg], &'a str, &'a Expr)> {
    let Expr::UserCall { name, args, .. } = expr else {
        return None;
    };
    let proc_name = if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
        step_proc
    } else if let Some((call_proc, out_idx_raw)) = name.rsplit_once(PROC_CALL_OUT_FN_PREFIX) {
        if out_idx_raw.parse::<usize>().is_ok() {
            call_proc
        } else {
            return None;
        }
    } else {
        return None;
    };
    let api = proc_api.get(proc_name)?;
    if !api.has_block {
        return None;
    }
    let self_arg = args.first()?;
    let Expr::Index { base, index, .. } = &self_arg.expr else {
        return None;
    };
    Some((proc_name, args, base.as_str(), index.as_ref()))
}

pub(super) fn rewrite_stmt_for_def_proc_block_guards(
    stmt: Stmt,
    proc_api: &HashMap<String, ProcApi>,
    proc_block_active_symbols: &HashMap<String, String>,
) -> Vec<Stmt> {
    fn collect_guards(
        expr: &Expr,
        proc_api: &HashMap<String, ProcApi>,
        proc_block_active_symbols: &HashMap<String, String>,
        guards: &mut Vec<Stmt>,
    ) {
        match expr {
            Expr::Index { index, .. } => {
                collect_guards(index, proc_api, proc_block_active_symbols, guards)
            }
            Expr::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    collect_guards(coordinate, proc_api, proc_block_active_symbols, guards);
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_guards(&spec.size, proc_api, proc_block_active_symbols, guards);
                if let Some(values) = init {
                    for value in values {
                        collect_guards(value, proc_api, proc_block_active_symbols, guards);
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                collect_guards(lhs, proc_api, proc_block_active_symbols, guards);
                collect_guards(rhs, proc_api, proc_block_active_symbols, guards);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    collect_guards(arg, proc_api, proc_block_active_symbols, guards);
                }
            }
            Expr::UserCall { args, .. } => {
                for arg in args {
                    collect_guards(&arg.expr, proc_api, proc_block_active_symbols, guards);
                }
                let Some((proc_name, args, array_base, index_expr)) =
                    try_indexed_proc_call_meta_in_def(expr, proc_api)
                else {
                    return;
                };
                let Some(api) = proc_api.get(proc_name) else {
                    return;
                };
                let Some(active_symbol) = proc_block_active_symbols.get(array_base).cloned() else {
                    return;
                };
                let input_slots = api.ins.iter().map(|port| port.slots.len()).sum::<usize>();
                let buffer_start = 1 + input_slots;
                let mut pre_args = Vec::<CallArg>::new();
                pre_args.push(CallArg {
                    name: None,
                    expr: Expr::Index {
                        loc: Default::default(),
                        base: array_base.to_owned(),
                        index: Box::new(index_expr.clone()),
                    },
                });
                pre_args.extend(args.iter().skip(buffer_start).cloned());
                guards.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(Expr::Index {
                            loc: Default::default(),
                            base: active_symbol.clone(),
                            index: Box::new(index_expr.clone()),
                        }),
                    },
                    then_branch: vec![
                        Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: format!("{proc_name}{PROC_BLOCK_PRE_FN_SUFFIX}"),
                                type_args: Vec::new(),
                                args: pre_args,
                            },
                        },
                        Stmt::Assign {
                            loc: Default::default(),
                            target_loc: Default::default(),
                            target: AssignTarget::Index {
                                base: active_symbol,
                                index: index_expr.clone(),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: Expr::bool(true),
                        },
                    ],
                    else_branch: Vec::new(),
                });
            }
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner, .. }
            | Expr::UnaryBitNot { expr: inner, .. } => {
                collect_guards(inner, proc_api, proc_block_active_symbols, guards)
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    collect_guards(value, proc_api, proc_block_active_symbols, guards);
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    let mut guards = Vec::<Stmt>::new();
    match &stmt {
        Stmt::Expr { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
            collect_guards(expr, proc_api, proc_block_active_symbols, &mut guards);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                collect_guards(value, proc_api, proc_block_active_symbols, &mut guards);
            }
        }
        Stmt::If { cond, .. } | Stmt::While { cond, .. } => {
            collect_guards(cond, proc_api, proc_block_active_symbols, &mut guards);
        }
        Stmt::For {
            start, end, step, ..
        } => {
            collect_guards(start, proc_api, proc_block_active_symbols, &mut guards);
            collect_guards(end, proc_api, proc_block_active_symbols, &mut guards);
            if let Some(step) = step {
                collect_guards(step, proc_api, proc_block_active_symbols, &mut guards);
            }
        }
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    if guards.is_empty() {
        return match stmt {
            Stmt::If {
                loc,
                cond,
                then_branch,
                else_branch,
            } => {
                let mut rewritten_then = Vec::<Stmt>::new();
                for nested in then_branch {
                    rewritten_then.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                let mut rewritten_else = Vec::<Stmt>::new();
                for nested in else_branch {
                    rewritten_else.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                vec![Stmt::If {
                    loc,
                    cond,
                    then_branch: rewritten_then,
                    else_branch: rewritten_else,
                }]
            }
            Stmt::For {
                loc,
                var,
                var_ty,
                start,
                end,
                end_inclusive,
                step,
                body,
            } => {
                let mut rewritten_body = Vec::<Stmt>::new();
                for nested in body {
                    rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                vec![Stmt::For {
                    loc,
                    var,
                    var_ty,
                    start,
                    end,
                    end_inclusive,
                    step,
                    body: rewritten_body,
                }]
            }
            Stmt::While { loc, cond, body } => {
                let mut rewritten_body = Vec::<Stmt>::new();
                for nested in body {
                    rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                vec![Stmt::While {
                    loc,
                    cond,
                    body: rewritten_body,
                }]
            }
            other => vec![other],
        };
    }
    let mut rewritten = guards;
    match stmt {
        Stmt::If {
            loc,
            cond,
            then_branch,
            else_branch,
        } => {
            let mut rewritten_then = Vec::<Stmt>::new();
            for nested in then_branch {
                rewritten_then.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            let mut rewritten_else = Vec::<Stmt>::new();
            for nested in else_branch {
                rewritten_else.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            rewritten.push(Stmt::If {
                loc,
                cond,
                then_branch: rewritten_then,
                else_branch: rewritten_else,
            });
        }
        Stmt::For {
            loc,
            var,
            var_ty,
            start,
            end,
            end_inclusive,
            step,
            body,
        } => {
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            rewritten.push(Stmt::For {
                loc,
                var,
                var_ty,
                start,
                end,
                end_inclusive,
                step,
                body: rewritten_body,
            });
        }
        Stmt::While { loc, cond, body } => {
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            rewritten.push(Stmt::While {
                loc,
                cond,
                body: rewritten_body,
            });
        }
        other => rewritten.push(other),
    }
    rewritten
}

pub(super) fn typed_def_owner_proc_param_index_for_symbol(
    symbol: &str,
    def: &TypedFunction,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<usize> {
    let root = symbol.split('.').next().unwrap_or(symbol);
    let param_idx = def.params.iter().position(|param| param == root)?;
    let TypedFnParam::Struct { struct_name } = def.param_kinds.get(param_idx)? else {
        return None;
    };
    let api = proc_api.get(struct_name)?;
    if api.has_block {
        Some(param_idx)
    } else {
        None
    }
}

pub(super) fn typed_def_owner_proc_param_index_for_expr(
    expr: &Expr,
    def: &TypedFunction,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<usize> {
    match expr {
        Expr::Var { name, .. } => typed_def_owner_proc_param_index_for_symbol(name, def, proc_api),
        Expr::Index { base, .. } => {
            typed_def_owner_proc_param_index_for_symbol(base, def, proc_api)
        }
        _ => None,
    }
}

pub(super) fn collect_typed_def_owner_proc_hook_params_from_expr(
    expr: &Expr,
    def: &TypedFunction,
    def_map: &HashMap<String, &TypedFunction>,
    known_requirements: &HashMap<String, HashSet<usize>>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashSet<usize>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    value,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
        Expr::Index { index, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                index,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    coordinate,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                &spec.size,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_typed_def_owner_proc_hook_params_from_expr(
                        value,
                        def,
                        def_map,
                        known_requirements,
                        proc_api,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                lhs,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
            collect_typed_def_owner_proc_hook_params_from_expr(
                rhs,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    arg,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    &arg.expr,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            if let Some((_proc_name, _args, array_base, _index_expr)) =
                try_indexed_proc_call_meta_in_def(expr, proc_api)
            {
                if let Some(param_idx) =
                    typed_def_owner_proc_param_index_for_symbol(array_base, def, proc_api)
                {
                    out.insert(param_idx);
                }
            }
            let Some(callee) = def_map.get(name) else {
                return;
            };
            let Some(required_params) = known_requirements.get(name) else {
                return;
            };
            if required_params.is_empty() {
                return;
            }
            let mut call_errors = Vec::new();
            let resolved = resolve_call_args(
                args,
                &callee.params,
                &callee.param_defaults,
                false,
                false,
                &format!("call '{}(...)'", callee.name),
                &mut call_errors,
            );
            for required_idx in required_params {
                let Some(Some(arg_expr)) = resolved.get(*required_idx) else {
                    continue;
                };
                if let Some(param_idx) =
                    typed_def_owner_proc_param_index_for_expr(arg_expr, def, proc_api)
                {
                    out.insert(param_idx);
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                inner,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
        }
    }
}

pub(super) fn collect_typed_def_owner_proc_hook_params_from_stmts(
    stmts: &[Stmt],
    def: &TypedFunction,
    def_map: &HashMap<String, &TypedFunction>,
    known_requirements: &HashMap<String, HashSet<usize>>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashSet<usize>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Print { values, .. } => {
                for value in values {
                    collect_typed_def_owner_proc_hook_params_from_expr(
                        value,
                        def,
                        def_map,
                        known_requirements,
                        proc_api,
                        out,
                    );
                }
            }
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    expr,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    cond,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    then_branch,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    else_branch,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    start,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_expr(
                    end,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                if let Some(step) = step {
                    collect_typed_def_owner_proc_hook_params_from_expr(
                        step,
                        def,
                        def_map,
                        known_requirements,
                        proc_api,
                        out,
                    );
                }
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    body,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    cond,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    body,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
    }
}

pub(super) fn collect_typed_def_owner_proc_hook_requirements(
    defs: &[TypedFunction],
    proc_api: &HashMap<String, ProcApi>,
) -> HashMap<String, HashSet<usize>> {
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let mut requirements = HashMap::<String, HashSet<usize>>::new();

    loop {
        let mut changed = false;
        for def in defs {
            let mut direct = HashSet::<usize>::new();
            collect_typed_def_owner_proc_hook_params_from_stmts(
                &def.body,
                def,
                &def_map,
                &requirements,
                proc_api,
                &mut direct,
            );
            let entry = requirements.entry(def.name.clone()).or_default();
            for param_idx in direct {
                if entry.insert(param_idx) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    requirements
}

pub(super) fn expr_global_proc_instance_name(
    expr: &Expr,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
) -> Option<String> {
    match expr {
        Expr::Var { name, .. } if global_proc_instances.contains_key(name) => Some(name.clone()),
        Expr::Index { base, index, .. } => {
            let Expr::Int { value, .. } = index.as_ref() else {
                return None;
            };
            let slot_name = format!("{base}[{value}]");
            global_proc_instances
                .contains_key(&slot_name)
                .then_some(slot_name)
        }
        _ => None,
    }
}

pub(super) fn stmt_has_proc_block_hook_for_instance(
    stmt: &Stmt,
    proc_name: &str,
    suffix: &str,
    instance_name: &str,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
) -> bool {
    let Stmt::Expr {
        expr: Expr::UserCall { name, args, .. },
        ..
    } = stmt
    else {
        return false;
    };
    if name != &format!("{proc_name}{suffix}") {
        return false;
    }
    let Some(self_arg) = args.first() else {
        return false;
    };
    expr_global_proc_instance_name(&self_arg.expr, global_proc_instances).as_deref()
        == Some(instance_name)
}

pub(super) fn collect_sample_owner_proc_hook_instances_from_expr(
    expr: &Expr,
    def_map: &HashMap<String, &TypedFunction>,
    requirements: &HashMap<String, HashSet<usize>>,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    out: &mut HashSet<String>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_sample_owner_proc_hook_instances_from_expr(
                    value,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
        Expr::Index { index, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                index,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_sample_owner_proc_hook_instances_from_expr(
                    coordinate,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                &spec.size,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_sample_owner_proc_hook_instances_from_expr(
                        value,
                        def_map,
                        requirements,
                        global_proc_instances,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                lhs,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
            collect_sample_owner_proc_hook_instances_from_expr(
                rhs,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_sample_owner_proc_hook_instances_from_expr(
                    arg,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                collect_sample_owner_proc_hook_instances_from_expr(
                    &arg.expr,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            let Some(callee) = def_map.get(name) else {
                return;
            };
            let Some(required_params) = requirements.get(name) else {
                return;
            };
            if required_params.is_empty() {
                return;
            }
            let mut call_errors = Vec::new();
            let resolved = resolve_call_args(
                args,
                &callee.params,
                &callee.param_defaults,
                false,
                false,
                &format!("call '{}(...)'", callee.name),
                &mut call_errors,
            );
            for required_idx in required_params {
                let Some(Some(arg_expr)) = resolved.get(*required_idx) else {
                    continue;
                };
                if let Some(instance_name) =
                    expr_global_proc_instance_name(arg_expr, global_proc_instances)
                {
                    out.insert(instance_name);
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                inner,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
        }
    }
}

pub(super) fn collect_sample_owner_proc_hook_instances_from_stmts(
    stmts: &[Stmt],
    def_map: &HashMap<String, &TypedFunction>,
    requirements: &HashMap<String, HashSet<usize>>,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    out: &mut HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Print { values, .. } => {
                for value in values {
                    collect_sample_owner_proc_hook_instances_from_expr(
                        value,
                        def_map,
                        requirements,
                        global_proc_instances,
                        out,
                    );
                }
            }
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    expr,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    cond,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_stmts(
                    then_branch,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_stmts(
                    else_branch,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    start,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_expr(
                    end,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                if let Some(step) = step {
                    collect_sample_owner_proc_hook_instances_from_expr(
                        step,
                        def_map,
                        requirements,
                        global_proc_instances,
                        out,
                    );
                }
                collect_sample_owner_proc_hook_instances_from_stmts(
                    body,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    cond,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_stmts(
                    body,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
    }
}

pub(super) fn inject_sample_def_owner_proc_block_hooks(
    sample: &[Stmt],
    block_pre: &mut Vec<Stmt>,
    block_post: &mut Vec<Stmt>,
    defs: &[TypedFunction],
    proc_api: &HashMap<String, ProcApi>,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    global_proc_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) {
    if defs.is_empty() || global_proc_instances.is_empty() {
        return;
    }
    let requirements = collect_typed_def_owner_proc_hook_requirements(defs, proc_api);
    if requirements.values().all(HashSet::is_empty) {
        return;
    }
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let mut instance_names = HashSet::<String>::new();
    collect_sample_owner_proc_hook_instances_from_stmts(
        sample,
        &def_map,
        &requirements,
        global_proc_instances,
        &mut instance_names,
    );
    if instance_names.is_empty() {
        return;
    }

    let mut ordered_instances = instance_names.into_iter().collect::<Vec<_>>();
    ordered_instances.sort();
    let mut injected_pre = Vec::<Stmt>::new();
    let mut injected_post = Vec::<Stmt>::new();
    for instance_name in ordered_instances {
        let Some(instance) = global_proc_instances.get(&instance_name) else {
            continue;
        };
        let Some(api) = proc_api.get(&instance.proc_name) else {
            continue;
        };
        if !api.has_block {
            continue;
        }
        let has_existing_pre = block_pre.iter().any(|stmt| {
            stmt_has_proc_block_hook_for_instance(
                stmt,
                &instance.proc_name,
                PROC_BLOCK_PRE_FN_SUFFIX,
                &instance_name,
                global_proc_instances,
            )
        });
        if !has_existing_pre {
            let mut pre_args = vec![CallArg {
                name: None,
                expr: proc_instance_self_expr(&instance_name, global_proc_array_slots),
            }];
            pre_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_pre.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_PRE_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: pre_args,
                },
            });
        }

        let has_existing_post = block_post.iter().any(|stmt| {
            stmt_has_proc_block_hook_for_instance(
                stmt,
                &instance.proc_name,
                PROC_BLOCK_POST_FN_SUFFIX,
                &instance_name,
                global_proc_instances,
            )
        });
        if !has_existing_post {
            let mut post_args = vec![CallArg {
                name: None,
                expr: proc_instance_self_expr(&instance_name, global_proc_array_slots),
            }];
            post_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_post.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_POST_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: post_args,
                },
            });
        }
    }
    if !injected_pre.is_empty() {
        let mut new_block_pre = injected_pre;
        new_block_pre.append(block_pre);
        *block_pre = new_block_pre;
    }
    if !injected_post.is_empty() {
        block_post.extend(injected_post);
    }
}

pub(super) fn seed_called_typed_defs_from_stmts(
    stmts: &[Stmt],
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    for stmt in stmts {
        collect_called_typed_defs_in_stmt(stmt, def_names, pending, seen_pending);
    }
}

pub(super) fn seed_called_typed_defs_from_defaults(
    defaults: &[Option<Expr>],
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    for default in defaults.iter().flatten() {
        collect_called_typed_defs_in_expr(default, def_names, pending, seen_pending);
    }
}

pub(super) fn collect_called_typed_defs_in_stmt(
    stmt: &Stmt,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Print { values, .. } => {
            for value in values {
                collect_called_typed_defs_in_expr(value, def_names, pending, seen_pending);
            }
        }
        Stmt::Assign { target, expr, .. } => {
            collect_called_typed_defs_in_assign_target(target, def_names, pending, seen_pending);
            collect_called_typed_defs_in_expr(expr, def_names, pending, seen_pending);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_called_typed_defs_in_expr(expr, def_names, pending, seen_pending);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_called_typed_defs_in_expr(cond, def_names, pending, seen_pending);
            seed_called_typed_defs_from_stmts(then_branch, def_names, pending, seen_pending);
            seed_called_typed_defs_from_stmts(else_branch, def_names, pending, seen_pending);
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_called_typed_defs_in_expr(start, def_names, pending, seen_pending);
            collect_called_typed_defs_in_expr(end, def_names, pending, seen_pending);
            if let Some(step) = step {
                collect_called_typed_defs_in_expr(step, def_names, pending, seen_pending);
            }
            seed_called_typed_defs_from_stmts(body, def_names, pending, seen_pending);
        }
        Stmt::While { cond, body, .. } => {
            collect_called_typed_defs_in_expr(cond, def_names, pending, seen_pending);
            seed_called_typed_defs_from_stmts(body, def_names, pending, seen_pending);
        }
    }
}

pub(super) fn collect_called_typed_defs_in_assign_target(
    target: &AssignTarget,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match target {
        AssignTarget::Index { index, .. } => {
            collect_called_typed_defs_in_expr(index, def_names, pending, seen_pending);
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_called_typed_defs_in_expr(coordinate, def_names, pending, seen_pending);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

pub(super) fn collect_called_typed_defs_in_expr(
    expr: &Expr,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_called_typed_defs_in_expr(value, def_names, pending, seen_pending);
            }
        }
        Expr::Index { index, .. } => {
            collect_called_typed_defs_in_expr(index, def_names, pending, seen_pending);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_called_typed_defs_in_expr(coordinate, def_names, pending, seen_pending);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_called_typed_defs_in_expr(&spec.size, def_names, pending, seen_pending);
            if let Some(values) = init {
                for value in values {
                    collect_called_typed_defs_in_expr(value, def_names, pending, seen_pending);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_called_typed_defs_in_expr(lhs, def_names, pending, seen_pending);
            collect_called_typed_defs_in_expr(rhs, def_names, pending, seen_pending);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_called_typed_defs_in_expr(arg, def_names, pending, seen_pending);
            }
        }
        Expr::UserCall { name, args, .. } => {
            if def_names.contains(name) && seen_pending.insert(name.clone()) {
                pending.push(name.clone());
            }
            for arg in args {
                collect_called_typed_defs_in_expr(&arg.expr, def_names, pending, seen_pending);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_called_typed_defs_in_expr(expr, def_names, pending, seen_pending);
        }
    }
}
