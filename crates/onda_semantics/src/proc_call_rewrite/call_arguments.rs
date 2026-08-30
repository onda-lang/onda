use super::*;

#[derive(Debug, Clone)]
enum ProcNamedArgReceiver {
    Direct(String),
    Indexed {
        array_base: String,
        index: Expr,
        resolved_slot: Option<String>,
        access: IndexAccess,
    },
}

#[derive(Debug, Clone)]
struct ProcNamedArgCallTarget {
    display_name: String,
    receiver: ProcNamedArgReceiver,
    api: ProcApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcCallArgKind {
    Internal,
    Input,
    Param,
    PrivateParam,
    Ambiguous,
    Unknown,
}

fn proc_named_arg_is_internal(name: Option<&str>) -> bool {
    matches!(
        name,
        Some(PROC_INDEX_BASE_ARG)
            | Some(PROC_INDEX_EXPR_ARG)
            | Some(PROC_INDEX_UNCHECKED_ARG)
            | Some(PROC_FIELD_SENTINEL_ARG)
    )
}

fn proc_input_arg_names(api: &ProcApi) -> HashSet<String> {
    api.ins
        .iter()
        .flat_map(|port| std::iter::once(port.name.clone()).chain(port.slots.iter().cloned()))
        .collect()
}

fn proc_named_param_slots(
    api: &ProcApi,
    name: &str,
    include_private: bool,
) -> Option<Vec<ProcParamSlotSpec>> {
    if let Some(slot) = api.params.get(name) {
        if slot.private && !include_private {
            return None;
        }
        return Some(vec![slot.clone()]);
    }

    let prefix = format!("{name}[");
    let mut slots = api
        .params
        .values()
        .filter_map(|slot| {
            if slot.private && !include_private {
                return None;
            }
            let rest = slot.name.strip_prefix(&prefix)?;
            let idx = rest.strip_suffix(']')?.parse::<usize>().ok()?;
            Some((idx, slot.clone()))
        })
        .collect::<Vec<_>>();
    if slots.is_empty() {
        return None;
    }
    slots.sort_by_key(|(idx, _)| *idx);
    Some(slots.into_iter().map(|(_, slot)| slot).collect())
}

fn proc_param_slots_for_api<'a>(api: &'a ProcApi, name: &str) -> Vec<&'a ProcParamSlotSpec> {
    if let Some(slot) = api.params.get(name) {
        return vec![slot];
    }

    let prefix = format!("{name}[");
    let mut slots = api
        .params
        .values()
        .filter_map(|slot| {
            let rest = slot.name.strip_prefix(&prefix)?;
            let idx = rest.strip_suffix(']')?.parse::<usize>().ok()?;
            Some((idx, slot))
        })
        .collect::<Vec<_>>();
    slots.sort_by_key(|(idx, _)| *idx);
    slots.into_iter().map(|(_, slot)| slot).collect()
}

pub(super) fn proc_param_field_is_private(api: &ProcApi, field: &str) -> bool {
    proc_param_slots_for_api(api, field)
        .into_iter()
        .any(|slot| slot.private)
}

pub(super) fn proc_has_private_params(api: &ProcApi) -> bool {
    api.params.values().any(|slot| slot.private)
}

fn proc_call_arg_kind(api: &ProcApi, arg_name: Option<&str>) -> ProcCallArgKind {
    let Some(name) = arg_name else {
        return ProcCallArgKind::Input;
    };
    if proc_named_arg_is_internal(Some(name)) {
        return ProcCallArgKind::Internal;
    }
    let is_input = proc_input_arg_names(api).contains(name);
    let is_param = proc_named_param_slots(api, name, false).is_some();
    let is_private_param = !is_param && proc_named_param_slots(api, name, true).is_some();
    match (is_input, is_param, is_private_param) {
        (_, _, true) => ProcCallArgKind::PrivateParam,
        (true, true, false) => ProcCallArgKind::Ambiguous,
        (true, false, false) => ProcCallArgKind::Input,
        (false, true, false) => ProcCallArgKind::Param,
        (false, false, false) => ProcCallArgKind::Unknown,
    }
}

fn proc_named_arg_internal_indices(name: &str, args: &[CallArg]) -> HashSet<usize> {
    let is_indexed_call = name == PROC_INDEX_CALL_SENTINEL
        || name
            .strip_prefix(PROC_INDEX_CALL_SENTINEL)
            .is_some_and(|suffix| suffix.starts_with('.'))
        || name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .is_some_and(|raw| raw == PROC_INDEX_CALL_SENTINEL);
    if !is_indexed_call {
        return HashSet::new();
    }

    let mut internal = HashSet::<usize>::new();
    let has_named_index_args = args.iter().any(|arg| {
        matches!(
            arg.name.as_deref(),
            Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG)
        )
    });
    if has_named_index_args {
        for (idx, arg) in args.iter().enumerate() {
            if proc_named_arg_is_internal(arg.name.as_deref()) {
                internal.insert(idx);
            }
        }
        return internal;
    }

    let mut positional_seen = 0usize;
    for (idx, arg) in args.iter().enumerate() {
        if proc_named_arg_is_internal(arg.name.as_deref()) {
            internal.insert(idx);
            continue;
        }
        if arg.name.is_none() && positional_seen < 2 {
            internal.insert(idx);
            positional_seen += 1;
        }
    }
    internal
}

fn proc_named_arg_assignment_stmt(
    receiver: &ProcNamedArgReceiver,
    slot_name: &str,
    expr: Expr,
) -> Stmt {
    if let ProcNamedArgReceiver::Indexed {
        array_base,
        index,
        resolved_slot: None,
        access: IndexAccess::Unchecked,
    } = receiver
    {
        return Stmt::Expr {
            loc: Default::default(),
            expr: Expr::UserCall {
                loc: Default::default(),
                name: WRITE_UNSAFE_FN.to_owned(),
                type_args: Vec::new(),
                args: vec![
                    CallArg {
                        name: None,
                        expr: Expr::var(format!("{array_base}.{slot_name}")),
                    },
                    CallArg {
                        name: None,
                        expr: index.clone(),
                    },
                    CallArg { name: None, expr },
                ],
            },
        };
    }
    let target = match receiver {
        ProcNamedArgReceiver::Direct(receiver) => {
            AssignTarget::Var(format!("{receiver}.{slot_name}"))
        }
        ProcNamedArgReceiver::Indexed {
            array_base,
            index,
            resolved_slot,
            ..
        } => resolved_slot.as_ref().map_or_else(
            || AssignTarget::Index {
                base: format!("{array_base}.{slot_name}"),
                index: index.clone(),
            },
            |slot| AssignTarget::Var(format!("{slot}.{slot_name}")),
        ),
    };
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target,
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn proc_named_arg_array_slot_expr(
    expr: &Expr,
    slot_idx: usize,
    slot_count: usize,
    call_display_name: &str,
    arg_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    expand_expr_to_slots(
        expr,
        slot_count,
        &format!("processor call '{call_display_name}(...)' argument '{arg_name}'"),
        errors,
    )
    .get(slot_idx)
    .cloned()
    .unwrap_or_else(|| expr.clone())
}

fn proc_named_arg_call_target_from_parts(
    name: &str,
    args: &[CallArg],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcNamedArgCallTarget> {
    if let Some(instance) = proc_vars.get(name) {
        let api = proc_api.get(&instance.proc_name)?.clone();
        return Some(ProcNamedArgCallTarget {
            display_name: name.to_owned(),
            receiver: ProcNamedArgReceiver::Direct(name.to_owned()),
            api,
        });
    }

    if let Some(proc_var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
        if proc_var_raw != PROC_INDEX_CALL_SENTINEL {
            let instance = proc_vars.get(proc_var_raw)?;
            let api = proc_api.get(&instance.proc_name)?.clone();
            return Some(ProcNamedArgCallTarget {
                display_name: proc_var_raw.to_owned(),
                receiver: ProcNamedArgReceiver::Direct(proc_var_raw.to_owned()),
                api,
            });
        }
    }

    let is_indexed_call = name == PROC_INDEX_CALL_SENTINEL
        || name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .is_some_and(|raw| raw == PROC_INDEX_CALL_SENTINEL);
    if !is_indexed_call {
        return None;
    }

    let (array_base, index_expr, access) =
        proc_index_base_and_expr_from_args(args, "processor indexed call", errors)?;
    let slots = proc_array_slots.get(&array_base)?;
    let (proc_name, api, _) = resolve_proc_array_dispatch_context(
        slots,
        proc_vars,
        proc_api,
        "processor indexed call",
        errors,
    )?;
    let resolved_slot = try_constant_index_i64(&index_expr).and_then(|raw_idx| {
        if raw_idx < 0 || raw_idx >= slots.len() as i64 {
            None
        } else {
            slots.get(raw_idx as usize).cloned()
        }
    });
    Some(ProcNamedArgCallTarget {
        display_name: format!("{array_base}[...]"),
        receiver: ProcNamedArgReceiver::Indexed {
            array_base,
            index: index_expr,
            resolved_slot,
            access,
        },
        api: proc_api.get(&proc_name).cloned().unwrap_or(api),
    })
}

fn proc_index_base_and_expr_from_args(
    args: &[CallArg],
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, Expr, IndexAccess)> {
    let access = if args
        .iter()
        .any(|arg| arg.name.as_deref() == Some(PROC_INDEX_UNCHECKED_ARG))
    {
        IndexAccess::Unchecked
    } else {
        IndexAccess::Clamp
    };
    let named_base = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG))
        .map(|arg| arg.expr.clone());
    let named_index = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG))
        .map(|arg| arg.expr.clone());
    let (base_expr, index_expr) = if let (Some(base), Some(index)) = (named_base, named_index) {
        (base, index)
    } else {
        let mut positional = args.iter().filter(|arg| arg.name.is_none());
        let Some(base) = positional.next() else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: missing processor array base/index"),
            );
            return None;
        };
        let Some(index) = positional.next() else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: missing processor array base/index"),
            );
            return None;
        };
        (base.expr.clone(), index.expr.clone())
    };
    let Expr::Var {
        name: array_base, ..
    } = base_expr
    else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context}: processor array base must be a compile-time identifier"),
        );
        return None;
    };
    Some((array_base, index_expr, access))
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_calls_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &ProcArrayAliases,
    prelude: &mut Vec<Stmt>,
    temp_counter: &mut usize,
    replace_call_with_temp: bool,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => lower_named_proc_param_calls_in_expr(
            index,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            prelude,
            temp_counter,
            true,
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
                lower_named_proc_param_calls_in_expr(
                    coordinate,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    prelude,
                    temp_counter,
                    true,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            lower_named_proc_param_calls_in_expr(
                &mut spec.size,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    lower_named_proc_param_calls_in_expr(
                        value,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        aliases,
                        prelude,
                        temp_counter,
                        true,
                        errors,
                    );
                }
            }
        }
        Expr::Logical { .. } => {
            reject_named_proc_param_calls_in_expr(
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                "logical expressions",
                errors,
            );
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            lower_named_proc_param_calls_in_expr(
                lhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
            lower_named_proc_param_calls_in_expr(
                rhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                lower_named_proc_param_calls_in_expr(
                    arg,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    prelude,
                    temp_counter,
                    true,
                    errors,
                );
            }
        }
        Expr::UserCall { .. } => lower_named_proc_param_call_expr(
            expr,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            prelude,
            temp_counter,
            replace_call_with_temp,
            errors,
        ),
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => lower_named_proc_param_calls_in_expr(
            inner,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            prelude,
            temp_counter,
            true,
            errors,
        ),
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                lower_named_proc_param_calls_in_expr(
                    value,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    prelude,
                    temp_counter,
                    true,
                    errors,
                );
            }
        }
        Expr::Var { .. } | Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_call_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &ProcArrayAliases,
    prelude: &mut Vec<Stmt>,
    temp_counter: &mut usize,
    replace_call_with_temp: bool,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_proc_alias_call_sites_in_expr(expr, aliases);

    let Expr::UserCall { name, args, .. } = expr else {
        return;
    };
    // Proc-array slots are known here, so classify receiver syntax before the
    // named-argument pass can interpret the internal receiver marker.
    canonicalize_indexed_proc_receiver_call(name, args, proc_array_slots);

    let Some(mut target) = proc_named_arg_call_target_from_parts(
        name,
        args,
        proc_vars,
        proc_array_slots,
        proc_api,
        errors,
    ) else {
        for arg in args {
            lower_named_proc_param_calls_in_expr(
                &mut arg.expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }
        return;
    };

    // Dynamic proc-array receivers feed the proc step/call itself, optional
    // block hooks, active-slot bookkeeping, and buffer selection. Snapshot the
    // source index before any of those rewrites so an effectful index expression
    // is evaluated exactly once regardless of how many ABI consumers it has.
    normalize_indexed_receiver_for_named_proc_args(
        &mut target,
        args,
        proc_vars,
        proc_array_slots,
        proc_api,
        aliases,
        prelude,
        temp_counter,
        errors,
    );

    let has_named_param_arg = args.iter().any(|arg| {
        matches!(
            proc_call_arg_kind(&target.api, arg.name.as_deref()),
            ProcCallArgKind::Param | ProcCallArgKind::PrivateParam | ProcCallArgKind::Ambiguous
        )
    });
    if !has_named_param_arg {
        for arg in args {
            lower_named_proc_param_calls_in_expr(
                &mut arg.expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }
        if replace_call_with_temp {
            let temp_name = proc_named_arg_result_temp_name(temp_counter);
            prelude.push(assign_temp_stmt(temp_name.clone(), expr.clone()));
            *expr = Expr::var(temp_name);
        }
        return;
    }

    let internal_arg_indices = proc_named_arg_internal_indices(name, args);
    let mut lowered_args = Vec::<CallArg>::with_capacity(args.len());
    let mut seen_param_args = HashSet::<String>::new();
    for (arg_idx, mut arg) in std::mem::take(args).into_iter().enumerate() {
        let arg_is_internal = internal_arg_indices.contains(&arg_idx)
            || proc_named_arg_is_internal(arg.name.as_deref());
        let arg_kind = if arg_is_internal {
            ProcCallArgKind::Internal
        } else {
            proc_call_arg_kind(&target.api, arg.name.as_deref())
        };
        if !arg_is_internal {
            lower_named_proc_param_calls_in_expr(
                &mut arg.expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }

        match arg_kind {
            ProcCallArgKind::Param => {
                let arg_name = arg.name.clone().unwrap_or_default();
                if !seen_param_args.insert(arg_name.clone()) {
                    push_semantic(
                        DiagCtx::new(arg.expr.loc()),
                        errors,
                        format!(
                            "processor call '{}(...)': duplicate named argument '{}'",
                            target.display_name, arg_name
                        ),
                    );
                    continue;
                }
                let Some(slots) = proc_named_param_slots(&target.api, &arg_name, false) else {
                    continue;
                };
                for (slot_idx, slot) in slots.iter().enumerate() {
                    let mut value = proc_named_arg_array_slot_expr(
                        &arg.expr,
                        slot_idx,
                        slots.len(),
                        &target.display_name,
                        &arg_name,
                        errors,
                    );
                    if let Some(range) = slot.range {
                        value = cast_expr_to_primitive(clamp_expr_to_range(value, range), slot.ty);
                    }
                    prelude.push(proc_named_arg_assignment_stmt(
                        &target.receiver,
                        &slot.name,
                        value,
                    ));
                }
            }
            ProcCallArgKind::Ambiguous => {
                let arg_name = arg.name.clone().unwrap_or_default();
                push_semantic(
                    DiagCtx::new(arg.expr.loc()),
                    errors,
                    format!(
                        "processor call '{}(...)' named argument '{}' matches both an input and a param",
                        target.display_name, arg_name
                    ),
                );
                lowered_args.push(arg);
            }
            ProcCallArgKind::PrivateParam => {
                let arg_name = arg.name.clone().unwrap_or_default();
                push_semantic(
                    DiagCtx::new(arg.expr.loc()),
                    errors,
                    format!(
                        "processor call '{}(...)' named argument '{}' is private and cannot be used as a live param argument; pass it to the constructor or builtin init(...), or expose an event",
                        target.display_name, arg_name
                    ),
                );
                lowered_args.push(arg);
            }
            ProcCallArgKind::Input | ProcCallArgKind::Unknown => {
                let temp_name = proc_named_arg_temp_name(temp_counter);
                prelude.push(assign_temp_stmt(temp_name.clone(), arg.expr));
                arg.expr = Expr::var(temp_name);
                lowered_args.push(arg);
            }
            ProcCallArgKind::Internal => lowered_args.push(arg),
        }
    }
    *args = lowered_args;

    if replace_call_with_temp {
        let temp_name = proc_named_arg_result_temp_name(temp_counter);
        prelude.push(assign_temp_stmt(temp_name.clone(), expr.clone()));
        *expr = Expr::var(temp_name);
    }
}

#[allow(clippy::too_many_arguments)]
fn normalize_indexed_receiver_for_named_proc_args(
    target: &mut ProcNamedArgCallTarget,
    args: &mut [CallArg],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &ProcArrayAliases,
    prelude: &mut Vec<Stmt>,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) {
    let ProcNamedArgReceiver::Indexed {
        index,
        resolved_slot,
        ..
    } = &mut target.receiver
    else {
        return;
    };
    if resolved_slot.is_some() {
        return;
    }

    lower_named_proc_param_calls_in_expr(
        index,
        proc_vars,
        proc_array_slots,
        proc_api,
        aliases,
        prelude,
        temp_counter,
        true,
        errors,
    );
    let temp_name = proc_named_arg_temp_name(temp_counter);
    prelude.push(assign_temp_stmt(temp_name.clone(), index.clone()));
    *index = Expr::var(temp_name.clone());
    for arg in &mut *args {
        if arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG) {
            arg.expr = Expr::var(temp_name.clone());
            return;
        }
    }
    let mut positional_seen = 0usize;
    for arg in &mut *args {
        if arg.name.is_none() {
            if positional_seen == 1 {
                arg.expr = Expr::var(temp_name.clone());
                return;
            }
            positional_seen += 1;
        }
    }
}

fn reject_named_proc_param_calls_in_expr(
    expr: &Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if let Some(target) = proc_named_arg_call_target_from_parts(
                name,
                args,
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                let has_named_param_arg = args.iter().any(|arg| {
                    matches!(
                        proc_call_arg_kind(&target.api, arg.name.as_deref()),
                        ProcCallArgKind::Param
                            | ProcCallArgKind::PrivateParam
                            | ProcCallArgKind::Ambiguous
                    )
                });
                if has_named_param_arg {
                    push_semantic(
                        DiagCtx::new(expr.loc()),
                        errors,
                        format!(
                            "processor named param arguments are not supported in {context}; assign the param explicitly before the proc call"
                        ),
                    );
                }
            }
            for arg in args {
                reject_named_proc_param_calls_in_expr(
                    &arg.expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => reject_named_proc_param_calls_in_expr(
            index,
            proc_vars,
            proc_array_slots,
            proc_api,
            context,
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
                reject_named_proc_param_calls_in_expr(
                    coordinate,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            reject_named_proc_param_calls_in_expr(
                &spec.size,
                proc_vars,
                proc_array_slots,
                proc_api,
                context,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    reject_named_proc_param_calls_in_expr(
                        value,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        context,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            reject_named_proc_param_calls_in_expr(
                lhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                context,
                errors,
            );
            reject_named_proc_param_calls_in_expr(
                rhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                context,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                reject_named_proc_param_calls_in_expr(
                    arg,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => reject_named_proc_param_calls_in_expr(
            inner,
            proc_vars,
            proc_array_slots,
            proc_api,
            context,
            errors,
        ),
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                reject_named_proc_param_calls_in_expr(
                    value,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::Var { .. } | Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_calls_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &mut ProcArrayAliases,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    let mut prelude = Vec::<Stmt>::new();
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            lower_named_proc_param_calls_in_expr(
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            update_proc_array_aliases_from_assignment(target, expr, proc_array_slots, aliases);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            lower_named_proc_param_calls_in_expr(
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
        }
        Stmt::Print { values, .. } => {
            for value in values {
                rewrite_proc_alias_calls_in_expr(value, aliases);
                lower_named_proc_param_calls_in_expr(
                    value,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    &mut prelude,
                    temp_counter,
                    false,
                    errors,
                );
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_proc_alias_calls_in_expr(cond, aliases);
            lower_named_proc_param_calls_in_expr(
                cond,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            let mut then_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                then_branch,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut then_aliases,
                temp_counter,
                errors,
            );
            let mut else_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                else_branch,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut else_aliases,
                temp_counter,
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
            rewrite_proc_alias_calls_in_expr(start, aliases);
            rewrite_proc_alias_calls_in_expr(end, aliases);
            lower_named_proc_param_calls_in_expr(
                start,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            lower_named_proc_param_calls_in_expr(
                end,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            if let Some(step_expr) = step {
                rewrite_proc_alias_calls_in_expr(step_expr, aliases);
                lower_named_proc_param_calls_in_expr(
                    step_expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    &mut prelude,
                    temp_counter,
                    false,
                    errors,
                );
            }
            let mut body_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                body,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut body_aliases,
                temp_counter,
                errors,
            );
        }
        Stmt::While { cond, body, .. } => {
            rewrite_proc_alias_calls_in_expr(cond, aliases);
            reject_named_proc_param_calls_in_expr(
                cond,
                proc_vars,
                proc_array_slots,
                proc_api,
                "while conditions",
                errors,
            );
            let mut body_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                body,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut body_aliases,
                temp_counter,
                errors,
            );
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    prelude
}

pub(super) fn update_proc_array_aliases_from_assignment(
    target: &AssignTarget,
    expr: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
    aliases: &mut ProcArrayAliases,
) {
    let AssignTarget::Var(name) = target else {
        return;
    };
    if let Some(alias) = proc_array_alias_from_index_expr(expr, proc_array_slots) {
        aliases.insert(name.clone(), alias);
    } else {
        aliases.remove(name);
    }
}

pub(super) fn proc_array_alias_from_index_expr(
    expr: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<ProcArrayAliasInfo> {
    let source = indexed_read_source(expr)?;
    let array_base = resolve_proc_array_base_key(source.base, proc_array_slots)?;
    Some(ProcArrayAliasInfo {
        array_base,
        index_expr: source.index.clone(),
        access: source.access,
    })
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_calls_in_stmts_with_aliases(
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &mut ProcArrayAliases,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) {
    let mut rewritten = Vec::<Stmt>::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        let prelude = lower_named_proc_param_calls_in_stmt(
            &mut stmt,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            temp_counter,
            errors,
        );
        rewritten.extend(prelude);
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

pub(crate) fn lower_named_proc_param_calls_in_stmts(
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut aliases = ProcArrayAliases::new();
    let mut temp_counter = 0usize;
    lower_named_proc_param_calls_in_stmts_with_aliases(
        stmts,
        proc_vars,
        proc_array_slots,
        proc_api,
        &mut aliases,
        &mut temp_counter,
        errors,
    );
}

pub(crate) fn expand_proc_event_call_args(
    call_args: &[CallArg],
    event: &ProcEventSpec,
    call_display_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let param_names = event
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let binding_defaults = param_names
        .iter()
        .map(|_| Some(Expr::number(0.0)))
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        call_args,
        &param_names,
        &binding_defaults,
        false,
        false,
        &format!("processor event call '{call_display_name}(...)'"),
        errors,
    );
    let mut expanded = Vec::<CallArg>::new();
    for (idx, param) in event.params.iter().enumerate() {
        let resolved_expr = match resolved.get(idx).and_then(|a| *a) {
            Some(arg_expr) => arg_expr.clone(),
            None if param.default.is_some() => {
                param.default.clone().unwrap_or_else(|| Expr::number(0.0))
            }
            None => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor event call '{call_display_name}(...)' is missing required argument '{}'",
                        param.name
                    ),
                );
                continue;
            }
        };
        match param.ty {
            ProcEventParamTypeSpec::FixedArray { len, .. } => {
                validate_fixed_array_event_arg(
                    &resolved_expr,
                    len,
                    &format!(
                        "processor event call '{call_display_name}(...)' argument '{}'",
                        param.name
                    ),
                    errors,
                );
                expanded.push(CallArg {
                    name: None,
                    expr: resolved_expr,
                });
                continue;
            }
            ProcEventParamTypeSpec::Slice { .. } => {
                expanded.push(CallArg {
                    name: None,
                    expr: resolved_expr,
                });
                continue;
            }
            ProcEventParamTypeSpec::Scalar { .. } => {}
        }
        let slot_exprs = expand_expr_to_slots(
            &resolved_expr,
            param.slots.len(),
            &format!(
                "processor event call '{call_display_name}(...)' argument '{}'",
                param.name
            ),
            errors,
        );
        for expr in slot_exprs {
            expanded.push(CallArg { name: None, expr });
        }
    }
    expanded
}

type ExpandedProcPortSpecs = (
    Vec<String>,
    HashMap<String, PrimitiveType>,
    Vec<ProcPortSpec>,
    HashMap<String, Vec<String>>,
);

pub(crate) fn expand_proc_port_specs(
    proc_name: &str,
    ports: &[PortDecl],
    kind: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ExpandedProcPortSpecs {
    let (flat, flat_types, arrays, defaults, ranges) = expand_port_decls(
        ports,
        &format!("processor '{proc_name}' {kind}"),
        options,
        errors,
    );
    let mut port_specs = Vec::<ProcPortSpec>::new();
    let mut array_slots = HashMap::<String, Vec<String>>::new();
    for port in ports {
        match port.ty.as_ref() {
            Some(DeclType::Array { .. }) | Some(DeclType::ArrayGeneric { .. }) => {
                let len = arrays.get(&port.name).map(|i| i.len).unwrap_or(0);
                let slots = (0..len)
                    .map(|idx| format!("{}[{idx}]", port.name))
                    .collect::<Vec<_>>();
                let slot_defaults = slots
                    .iter()
                    .map(|slot| defaults.get(slot).copied().map(typed_const_expr))
                    .collect::<Vec<_>>();
                array_slots.insert(port.name.clone(), slots.clone());
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots,
                    defaults: slot_defaults,
                    ranges: vec![None; len],
                });
            }
            _ => {
                let default = if port.default.is_some() {
                    defaults.get(&port.name).copied().map(typed_const_expr)
                } else {
                    None
                };
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots: vec![port.name.clone()],
                    defaults: vec![default],
                    ranges: vec![ranges.get(&port.name).copied()],
                });
            }
        }
    }
    (flat, flat_types, port_specs, array_slots)
}

pub(crate) fn expand_proc_param_specs(
    proc_name: &str,
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<ProcParamSpec>, HashMap<String, Vec<String>>) {
    let mut specs = Vec::<ProcParamSpec>::new();
    let mut field_array_slots = HashMap::<String, Vec<String>>::new();

    for param in params {
        match param.ty.as_ref() {
            None | Some(DeclType::Scalar(_)) => {
                let ty = match param.ty.as_ref() {
                    Some(DeclType::Scalar(ty)) => *ty,
                    None => param
                        .default
                        .as_ref()
                        .and_then(|expr| {
                            with_expr_diag_context(expr, |_diag| {
                                let expr_ty = infer_const_expr_type(
                                    expr,
                                    options,
                                    &format!(
                                        "processor '{proc_name}' param '{}' default",
                                        param.name
                                    ),
                                    errors,
                                );
                                effective_untyped_assignment_type(expr, expr_ty)
                            })
                        })
                        .unwrap_or(PrimitiveType::F32),
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("processor '{proc_name}' param '{}' default", param.name),
                        is_float_type(ty),
                        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                        errors,
                    )
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("processor '{proc_name}' param '{}'", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcParamSlotSpec {
                        name: param.name.clone(),
                        private: param.private,
                        ty,
                        default: Some(typed_const_expr(default)),
                        range,
                        bind: param.bind.clone(),
                    }],
                });
            }
            Some(DeclType::Generic(param_ty)) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic type '{}'",
                        param.name, param_ty
                    ),
                    param.ty_loc.or(param.loc),
                ));
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        PrimitiveType::F32,
                        options,
                        &format!("processor '{proc_name}' param '{}' default", param.name),
                        true,
                        false,
                        errors,
                    )
                    .unwrap_or(TypedConstValue::F32(0.0)),
                    None => TypedConstValue::F32(0.0),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        PrimitiveType::F32,
                        options,
                        &format!("processor '{proc_name}' param '{}'", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcParamSlotSpec {
                        name: param.name.clone(),
                        private: param.private,
                        ty: PrimitiveType::F32,
                        default: Some(typed_const_expr(default)),
                        range,
                        bind: param.bind.clone(),
                    }],
                });
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if let Some(bind) = &param.bind {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' uses bind hook '=> {bind}', but binds are only supported on primitive scalar params",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic array element type '{}'",
                        param.name, elem
                    ),
                    param.loc.as_ref(),
                ));
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = with_expr_diag_context(size, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::number(0.0)));
                        }
                    }
                    Some(default_expr @ Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            with_expr_diag_context(default_expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                        param.name,
                                        values.len()
                                    ),
                                );
                            });
                        }
                        for idx in 0..len {
                            slot_defaults.push(values.get(idx).cloned());
                        }
                    }
                    Some(expr) => {
                        for _ in 0..len {
                            slot_defaults.push(Some(expr.clone()));
                        }
                    }
                }
                let mut slots = Vec::<ProcParamSlotSpec>::with_capacity(len);
                let mut slot_names = Vec::<String>::with_capacity(len);
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", param.name);
                    slot_names.push(slot_name.clone());
                    slots.push(ProcParamSlotSpec {
                        name: slot_name,
                        private: param.private,
                        ty: PrimitiveType::F32,
                        default: slot_defaults.get(idx).cloned().unwrap_or(None),
                        range: None,
                        bind: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
            Some(DeclType::Tuple(_)) => {
                if let Some(bind) = &param.bind {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' uses bind hook '=> {bind}', but binds are only supported on primitive scalar params",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{proc_name}' param '{}' tuple type is not supported",
                        param.name
                    ),
                    param.ty_loc.or(param.loc),
                ));
                continue;
            }
            Some(DeclType::Array { elem, size }) => {
                if let Some(bind) = &param.bind {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' uses bind hook '=> {bind}', but binds are only supported on primitive scalar params",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = with_expr_diag_context(size, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::number(0.0)));
                        }
                    }
                    Some(default_expr @ Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            with_expr_diag_context(default_expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                        param.name,
                                        values.len()
                                    ),
                                );
                            });
                        }
                        for idx in 0..len {
                            slot_defaults
                                .push(values.get(idx).cloned().or(Some(Expr::number(0.0))));
                        }
                    }
                    Some(expr) => {
                        for _ in 0..len {
                            slot_defaults.push(Some(expr.clone()));
                        }
                    }
                }
                let mut slots = Vec::<ProcParamSlotSpec>::with_capacity(len);
                let mut slot_names = Vec::<String>::with_capacity(len);
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", param.name);
                    slot_names.push(slot_name.clone());
                    slots.push(ProcParamSlotSpec {
                        name: slot_name,
                        private: param.private,
                        ty: *elem,
                        default: slot_defaults
                            .get(idx)
                            .cloned()
                            .unwrap_or(Some(Expr::number(0.0))),
                        range: None,
                        bind: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
        }
    }

    (specs, field_array_slots)
}

pub(crate) fn proc_buffer_fn_param_type(spec: &ProcBufferSpec) -> FnParamType {
    let channels = match spec.channels {
        TypedBufferChannels::Mono => BufferChannels::Mono,
        TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
        TypedBufferChannels::Static(ch) => BufferChannels::Static(Expr::int(ch as i64)),
    };
    let buffer = onda_frontend::BufferType {
        elem: BufferElemType::Primitive(spec.elem_ty),
        channels,
    };
    if spec.is_array {
        FnParamType::BufferArray {
            buffer,
            len: spec.array_len,
        }
    } else {
        FnParamType::Buffer(buffer)
    }
}

pub(crate) fn rewrite_proc_calls_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_loc = expr.loc();
    let expr_diag = DiagCtx::new(expr_loc);
    match expr {
        Expr::Index { base, index, .. } => {
            rewrite_proc_calls_in_expr(index, proc_vars, proc_array_slots, proc_api, errors);
            if reject_private_proc_param_read_path(
                base,
                expr_loc.into(),
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                return;
            }
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_proc_calls_in_expr(
                    coordinate,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            if reject_private_proc_param_read_path(
                base,
                expr_loc.into(),
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                return;
            }
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_proc_calls_in_expr(
                &mut spec.size,
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_proc_calls_in_expr(
                        value,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_proc_calls_in_expr(lhs, proc_vars, proc_array_slots, proc_api, errors);
            rewrite_proc_calls_in_expr(rhs, proc_vars, proc_array_slots, proc_api, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_proc_calls_in_expr(arg, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_proc_calls_in_expr(inner, proc_vars, proc_array_slots, proc_api, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_proc_calls_in_expr(value, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Expr::UserCall { name, args, .. } => {
            canonicalize_indexed_proc_receiver_call(name, args, proc_array_slots);
            for arg in args.iter_mut() {
                rewrite_proc_calls_in_expr(
                    &mut arg.expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }

            if *name == PROC_INDEX_CALL_SENTINEL {
                if !can_resolve_proc_index_base(args, proc_array_slots) {
                    return;
                }
                let Some(index_target) = resolve_proc_index_target_mut(
                    args,
                    proc_array_slots,
                    "processor indexed call",
                    errors,
                ) else {
                    return;
                };
                match index_target {
                    ProcIndexResolution::Slot(resolved_slot) => {
                        *name = resolved_slot;
                    }
                    ProcIndexResolution::Dynamic {
                        array_base,
                        index_expr,
                        slots,
                        access,
                    } => {
                        let Some((proc_name, api, slot_instances)) =
                            resolve_proc_array_dispatch_context(
                                &slots,
                                proc_vars,
                                proc_api,
                                "processor indexed call",
                                errors,
                            )
                        else {
                            return;
                        };
                        if api.outputs.names.len() != 1 {
                            push_semantic(
                                expr_diag,
                                errors,
                                format!(
                                    "processor call '{}[...]' has {} outputs; use '{}[...]().<endpoint>'/{} or call as statement then read fields",
                                    array_base,
                                    api.outputs.names.len(),
                                    array_base,
                                    proc_output_alias_label(api.outputs.timing)
                                ),
                            );
                            return;
                        }
                        let rewritten = build_dynamic_proc_array_dispatch_args(
                            args,
                            &api,
                            &slot_instances,
                            &array_base,
                            &index_expr,
                            access,
                            errors,
                        );
                        *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                        *args = rewritten;
                        return;
                    }
                }
            }

            if let Some(proc_var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>, IndexAccess)>;
                let proc_var = if proc_var_raw == PROC_INDEX_CALL_SENTINEL {
                    if !can_resolve_proc_index_base(args, proc_array_slots) {
                        return;
                    }
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "processor indexed field call",
                        errors,
                    ) else {
                        return;
                    };
                    match index_target {
                        ProcIndexResolution::Slot(resolved_slot) => resolved_slot,
                        ProcIndexResolution::Dynamic {
                            array_base,
                            index_expr,
                            slots,
                            access,
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots, access));
                            String::new()
                        }
                    }
                } else {
                    proc_var_raw.to_owned()
                };
                let field_pos = args.iter().position(|a| {
                    a.name
                        .as_ref()
                        .map(|s| s == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                });
                let Some(field_pos) = field_pos else {
                    push_semantic(
                        expr_diag,
                        errors,
                        "processor call field selection is missing endpoint name",
                    );
                    return;
                };
                let field_arg = args.remove(field_pos);
                let Expr::Var {
                    name: field_name, ..
                } = field_arg.expr
                else {
                    push_semantic(
                        expr_diag,
                        errors,
                        "processor call field selection must be a compile-time endpoint identifier",
                    );
                    return;
                };

                if let Some((array_base, index_expr, slots, access)) = dynamic_index {
                    let Some((proc_name, api, slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            &slots,
                            proc_vars,
                            proc_api,
                            "processor indexed field call",
                            errors,
                        )
                    else {
                        return;
                    };
                    if proc_param_field_is_private(&api, &field_name) {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor '{proc_name}' param '{field_name}' is private and cannot be read through '{array_base}.{field_name}'"
                            ),
                        );
                        return;
                    }
                    let Some(out_idx) = resolve_proc_output_field_index(
                        &api,
                        &field_name,
                        &array_base,
                        expr_diag,
                        errors,
                    ) else {
                        return;
                    };
                    let rewritten = build_dynamic_proc_array_dispatch_args(
                        args,
                        &api,
                        &slot_instances,
                        &array_base,
                        &index_expr,
                        access,
                        errors,
                    );
                    *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                    *args = rewritten;
                    return;
                }

                let Some(instance) = proc_vars.get(proc_var.as_str()) else {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("processor call target '{}' is not an instance", proc_var),
                    );
                    return;
                };
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("unknown processor type '{proc_name}'"),
                    );
                    return;
                };
                if let Some(param_slot) = api.params.get(&field_name) {
                    if param_slot.private {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor '{proc_name}' param '{field_name}' is private and cannot be read through '{proc_var}.{field_name}'"
                            ),
                        );
                        return;
                    }
                    *expr = Expr::var(format!("{proc_var}.{}", param_slot.name));
                    return;
                }
                let Some(out_idx) = resolve_proc_output_field_index(
                    api,
                    &field_name,
                    proc_var.as_str(),
                    expr_diag,
                    errors,
                ) else {
                    return;
                };
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: proc_instance_self_expr(&proc_var, proc_array_slots),
                });
                let expanded_args = expand_proc_call_args(args, api, proc_var.as_str(), errors);
                rewritten.extend(expanded_args);
                let expanded_buffers =
                    expand_proc_buffer_call_args(instance, api, proc_var.as_str(), errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                *args = rewritten;
                return;
            }

            if let Some(instance) = proc_vars.get(name) {
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("unknown processor type '{proc_name}'"),
                    );
                    return;
                };
                if api.outputs.names.len() != 1 {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...).<endpoint>'/{} or call as statement then read fields",
                            name,
                            api.outputs.names.len(),
                            name,
                            proc_output_alias_label(api.outputs.timing)
                        ),
                    );
                    return;
                }
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: proc_instance_self_expr(name, proc_array_slots),
                });
                let expanded_args = expand_proc_call_args(args, api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers = expand_proc_buffer_call_args(instance, api, name, errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                *args = rewritten;
                return;
            }

            if let Some((base_raw, event_name)) = split_dot_path(name) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>, IndexAccess)>;
                let base = if base_raw == PROC_INDEX_CALL_SENTINEL {
                    if !can_resolve_proc_index_base(args, proc_array_slots) {
                        return;
                    }
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "processor indexed event call",
                        errors,
                    ) else {
                        return;
                    };
                    match index_target {
                        ProcIndexResolution::Slot(resolved_slot) => resolved_slot,
                        ProcIndexResolution::Dynamic {
                            array_base,
                            index_expr,
                            slots,
                            access,
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots, access));
                            String::new()
                        }
                    }
                } else {
                    base_raw.to_owned()
                };

                if let Some((array_base, _, slots, _)) = &dynamic_index {
                    let Some((_proc_name, api, _slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            slots,
                            proc_vars,
                            proc_api,
                            "processor indexed event call",
                            errors,
                        )
                    else {
                        return;
                    };
                    if api.events.contains_key(event_name) {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor event call '{}[...].{}(...)' is statement-only",
                                array_base, event_name
                            ),
                        );
                        return;
                    }
                }

                if let Some(instance) = proc_vars.get(base.as_str()) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!("unknown processor type '{proc_name}'"),
                        );
                        return;
                    };
                    if api.events.contains_key(event_name) {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor event call '{}.{}(...)' is statement-only",
                                base, event_name
                            ),
                        );
                        return;
                    }
                }

                if let Some((array_base, index_expr, slots, _access)) = dynamic_index {
                    let Some((proc_name, api, _slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            &slots,
                            proc_vars,
                            proc_api,
                            "processor indexed event call",
                            errors,
                        )
                    else {
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                array_base,
                                event_name,
                                known_events.join(", ")
                            ),
                        );
                        return;
                    };
                    let expanded = expand_proc_event_call_args(
                        args,
                        event_spec,
                        &format!("{array_base}[...].{event_name}"),
                        expr_diag,
                        errors,
                    );
                    let mut rewritten = Vec::<CallArg>::with_capacity(1 + expanded.len());
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: array_base.clone(),
                            index: Box::new(index_expr.clone()),
                        },
                    });
                    rewritten.extend(expanded);
                    *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    *args = rewritten;
                    return;
                }

                if let Some(instance) = proc_vars.get(base.as_str()) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!("unknown processor type '{proc_name}'"),
                        );
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                base,
                                event_name,
                                known_events.join(", ")
                            ),
                        );
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: proc_instance_self_expr(&base, proc_array_slots),
                    });
                    let expanded = expand_proc_event_call_args(
                        args,
                        event_spec,
                        &format!("{base}.{event_name}"),
                        expr_diag,
                        errors,
                    );
                    rewritten.extend(expanded);
                    *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    *args = rewritten;
                }
            }
        }
        Expr::Var { name, .. } => {
            if reject_private_proc_param_read_path(
                name,
                expr_loc.into(),
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                return;
            }
            normalize_proc_output_alias_path(name, proc_vars, proc_api);
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}
