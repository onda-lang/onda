use super::*;

pub(super) fn rewrite_nested_proc_calls_in_expr(
    expr: &mut Expr,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => rewrite_nested_proc_calls_in_expr(
            index,
            owner_proc,
            nested_instances,
            proc_array_slots,
            proc_api,
            errors,
        ),
        Expr::ArrayCtor { spec, init } => {
            rewrite_nested_proc_calls_in_expr(
                &mut spec.size,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_nested_proc_calls_in_expr(
                        value,
                        owner_proc,
                        nested_instances,
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
            rewrite_nested_proc_calls_in_expr(
                lhs,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            rewrite_nested_proc_calls_in_expr(
                rhs,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_nested_proc_calls_in_expr(
                    arg,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_nested_proc_calls_in_expr(
                    value,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                rewrite_nested_proc_calls_in_expr(
                    &mut arg.expr,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            if *name == PROC_INDEX_CALL_SENTINEL {
                let Some(index_target) = resolve_proc_index_target_mut(
                    args,
                    proc_array_slots,
                    "nested processor indexed call",
                    errors,
                ) else {
                    return;
                };
                match index_target {
                    ProcIndexResolution::Slot(resolved_slot) => {
                        let Some(instance) = nested_instances.get(resolved_slot.as_str()) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "nested processor indexed call target '{}' is not an instance",
                                    resolved_slot
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        let proc_name = instance.proc_name.clone();
                        let Some(api) = proc_api.get(&proc_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown processor type '{proc_name}'"),
                                0,
                                0,
                            ));
                            return;
                        };
                        if api.outs.len() != 1 {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor call '{}(...)' has {} outputs; use '{}(...).<endpoint>'/outN",
                                    resolved_slot,
                                    api.outs.len(),
                                    resolved_slot
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var(resolved_slot.clone()),
                        });
                        let expanded_inputs =
                            expand_proc_call_args(args, api, resolved_slot.as_str(), errors);
                        rewritten.extend(expanded_inputs);
                        let expanded_buffers = expand_proc_buffer_call_args(
                            instance,
                            api,
                            resolved_slot.as_str(),
                            errors,
                        );
                        rewritten.extend(expanded_buffers);
                        *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                        *args = rewritten;
                        return;
                    }
                    ProcIndexResolution::Dynamic {
                        array_base,
                        index_expr,
                        slots,
                    } => {
                        let Some((proc_name, api, slot_instances)) =
                            resolve_proc_array_dispatch_context(
                                &slots,
                                nested_instances,
                                proc_api,
                                "nested processor indexed call",
                                errors,
                            )
                        else {
                            return;
                        };
                        if api.outs.len() != 1 {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor call '{}[...]' has {} outputs; use '{}[...]().<endpoint>'/outN",
                                    array_base,
                                    api.outs.len(),
                                    array_base
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        let rewritten = build_dynamic_proc_array_dispatch_args(
                            args,
                            &api,
                            &slot_instances,
                            &array_base,
                            &index_expr,
                            errors,
                        );
                        *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                        *args = rewritten;
                        return;
                    }
                }
            }
            if let Some(var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>)>;
                let var = if var_raw == PROC_INDEX_CALL_SENTINEL {
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "nested processor indexed field call",
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
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots));
                            String::new()
                        }
                    }
                } else {
                    var_raw.to_owned()
                };
                let field_pos = args.iter().position(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                });
                let Some(field_pos) = field_pos else {
                    errors.push(Diagnostic::semantic(
                        "processor call field selection is missing endpoint name",
                        0,
                        0,
                    ));
                    return;
                };
                let field_arg = args.remove(field_pos);
                let Expr::Var(field_name) = field_arg.expr else {
                    errors.push(Diagnostic::semantic(
                        "processor call field selection must be a compile-time endpoint identifier",
                        0,
                        0,
                    ));
                    return;
                };

                if let Some((array_base, index_expr, slots)) = dynamic_index {
                    let Some((proc_name, api, slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            &slots,
                            nested_instances,
                            proc_api,
                            "nested processor indexed field call",
                            errors,
                        )
                    else {
                        return;
                    };
                    let Some(out_idx) =
                        resolve_proc_output_field_index(&api, &field_name, &array_base, errors)
                    else {
                        return;
                    };
                    let rewritten = build_dynamic_proc_array_dispatch_args(
                        args,
                        &api,
                        &slot_instances,
                        &array_base,
                        &index_expr,
                        errors,
                    );
                    *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                    *args = rewritten;
                    return;
                }

                if let Some(instance) = nested_instances.get(var.as_str()) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(out_idx) =
                        resolve_proc_output_field_index(api, &field_name, var.as_str(), errors)
                    else {
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    let is_array_slot =
                        find_proc_array_slot(var.as_str(), proc_array_slots).is_some();
                    if is_array_slot {
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var(var.to_owned()),
                        });
                    } else {
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var("self".to_owned()),
                        });
                    }
                    let expanded_args = expand_proc_call_args(args, api, var.as_str(), errors);
                    rewritten.extend(expanded_args);
                    let expanded_buffers =
                        expand_proc_buffer_call_args(instance, api, var.as_str(), errors);
                    rewritten.extend(expanded_buffers);
                    if is_array_slot {
                        *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                    } else {
                        *name = nested_call_out_fn_name(owner_proc, var.as_str(), out_idx);
                    }
                    *args = rewritten;
                }
                return;
            }
            if let Some(instance) = nested_instances.get(name) {
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown processor type '{proc_name}'"),
                        0,
                        0,
                    ));
                    return;
                };
                if api.outs.len() != 1 {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...).<endpoint>'/outN",
                            name,
                            api.outs.len(),
                            name
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                let nested_var = name.clone();
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                let is_array_slot =
                    find_proc_array_slot(nested_var.as_str(), proc_array_slots).is_some();
                if is_array_slot {
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var(nested_var.clone()),
                    });
                } else {
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var("self".to_owned()),
                    });
                }
                let expanded_args = expand_proc_call_args(args, api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers =
                    expand_proc_buffer_call_args(instance, api, &nested_var, errors);
                rewritten.extend(expanded_buffers);
                if is_array_slot {
                    *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                } else {
                    *name = nested_call_out_fn_name(owner_proc, &nested_var, 0);
                }
                *args = rewritten;
                return;
            }

            if let Some((base_raw, event_name)) = split_dot_path(name) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>)>;
                let base = if base_raw == PROC_INDEX_CALL_SENTINEL {
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "nested processor indexed event call",
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
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots));
                            String::new()
                        }
                    }
                } else {
                    base_raw.to_owned()
                };

                if let Some((array_base, index_expr, slots)) = dynamic_index {
                    let Some((proc_name, api, _slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            &slots,
                            nested_instances,
                            proc_api,
                            "nested processor indexed event call",
                            errors,
                        )
                    else {
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        errors.push(Diagnostic::semantic(
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                array_base,
                                event_name,
                                known_events.join(", ")
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    let expanded = expand_proc_event_call_args(
                        args,
                        event_spec,
                        &format!("{array_base}[...].{event_name}"),
                        errors,
                    );
                    let mut rewritten = Vec::<CallArg>::with_capacity(1 + expanded.len());
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Index {
                            base: array_base.clone(),
                            index: Box::new(index_expr.clone()),
                        },
                    });
                    rewritten.extend(expanded);
                    *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    *args = rewritten;
                    return;
                }

                if let Some(instance) = nested_instances.get(base.as_str()) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        errors.push(Diagnostic::semantic(
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                base,
                                event_name,
                                known_events.join(", ")
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    let is_array_slot = find_proc_array_slot(&base, proc_array_slots).is_some();
                    if is_array_slot {
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var(base.clone()),
                        });
                    } else {
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var("self".to_owned()),
                        });
                    }
                    let expanded = expand_proc_event_call_args(
                        args,
                        event_spec,
                        &format!("{base}.{event_name}"),
                        errors,
                    );
                    rewritten.extend(expanded);
                    if is_array_slot {
                        *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    } else {
                        *name = nested_event_fn_name(owner_proc, base.as_str(), event_name);
                    }
                    *args = rewritten;
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner }
        | Expr::UnaryBitNot { expr: inner } => rewrite_nested_proc_calls_in_expr(
            inner,
            owner_proc,
            nested_instances,
            proc_array_slots,
            proc_api,
            errors,
        ),
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(super) fn rewrite_nested_proc_calls_in_stmt(
    stmt: &mut Stmt,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { expr, .. } => rewrite_nested_proc_calls_in_expr(
            expr,
            owner_proc,
            nested_instances,
            proc_array_slots,
            proc_api,
            errors,
        ),
        Stmt::Expr { expr, .. } => {
            rewrite_nested_proc_calls_in_expr(
                expr,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            if let Expr::UserCall { name, args, .. } = expr {
                if *name == PROC_INDEX_CALL_SENTINEL {
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "nested processor indexed statement call",
                        errors,
                    ) else {
                        return;
                    };
                    match index_target {
                        ProcIndexResolution::Slot(resolved_slot) => {
                            let Some(instance) = nested_instances.get(resolved_slot.as_str())
                            else {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "nested processor indexed statement call target '{}' is not an instance",
                                        resolved_slot
                                    ),
                                    0,
                                    0,
                                ));
                                return;
                            };
                            let proc_name = instance.proc_name.clone();
                            let Some(api) = proc_api.get(&proc_name) else {
                                errors.push(Diagnostic::semantic(
                                    format!("unknown processor type '{proc_name}'"),
                                    0,
                                    0,
                                ));
                                return;
                            };
                            let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                            rewritten.push(CallArg {
                                name: None,
                                expr: Expr::Var(resolved_slot.clone()),
                            });
                            let expanded_args =
                                expand_proc_call_args(args, api, resolved_slot.as_str(), errors);
                            rewritten.extend(expanded_args);
                            let expanded_buffers = expand_proc_buffer_call_args(
                                instance,
                                api,
                                resolved_slot.as_str(),
                                errors,
                            );
                            rewritten.extend(expanded_buffers);
                            *name = format!("{proc_name}{PROC_STEP_FN_SUFFIX}");
                            *args = rewritten;
                            return;
                        }
                        ProcIndexResolution::Dynamic {
                            array_base,
                            index_expr,
                            slots,
                        } => {
                            let Some((proc_name, api, slot_instances)) =
                                resolve_proc_array_dispatch_context(
                                    &slots,
                                    nested_instances,
                                    proc_api,
                                    "nested processor indexed statement call",
                                    errors,
                                )
                            else {
                                return;
                            };
                            let rewritten = build_dynamic_proc_array_dispatch_args(
                                args,
                                &api,
                                &slot_instances,
                                &array_base,
                                &index_expr,
                                errors,
                            );
                            *name = format!("{proc_name}{PROC_STEP_FN_SUFFIX}");
                            *args = rewritten;
                            return;
                        }
                    }
                }
                if let Some(instance) = nested_instances.get(name) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let nested_var = name.clone();
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    let is_array_slot =
                        find_proc_array_slot(nested_var.as_str(), proc_array_slots).is_some();
                    if is_array_slot {
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var(nested_var.clone()),
                        });
                    } else {
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var("self".to_owned()),
                        });
                    }
                    let expanded_args = expand_proc_call_args(args, api, &nested_var, errors);
                    rewritten.extend(expanded_args);
                    let expanded_buffers =
                        expand_proc_buffer_call_args(instance, api, &nested_var, errors);
                    rewritten.extend(expanded_buffers);
                    if is_array_slot {
                        *name = format!("{proc_name}{PROC_STEP_FN_SUFFIX}");
                    } else {
                        *name = nested_step_fn_name(owner_proc, &nested_var);
                    }
                    *args = rewritten;
                }
            }
        }
        Stmt::Return { expr, .. } => rewrite_nested_proc_calls_in_expr(
            expr,
            owner_proc,
            nested_instances,
            proc_array_slots,
            proc_api,
            errors,
        ),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_nested_proc_calls_in_expr(
                cond,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            for s in then_branch {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            for s in else_branch {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_nested_proc_calls_in_expr(
                start,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            rewrite_nested_proc_calls_in_expr(
                end,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            if let Some(step_expr) = step {
                rewrite_nested_proc_calls_in_expr(
                    step_expr,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            for s in body {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_nested_proc_calls_in_expr(
                cond,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                errors,
            );
            for s in body {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(super) fn rewrite_owner_proc_stmt(
    mut stmt: Stmt,
    owner_proc: &str,
    field_names: &HashSet<String>,
    array_field_names: &HashSet<String>,
    ins_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    nested_fields: &HashMap<String, HashSet<String>>,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    normalize_proc_output_aliases_in_stmt(&mut stmt, nested_instances, proc_api);
    rewrite_nested_field_paths_in_stmt(&mut stmt, nested_fields);
    rewrite_nested_proc_calls_in_stmt(
        &mut stmt,
        owner_proc,
        nested_instances,
        proc_array_slots,
        proc_api,
        errors,
    );
    rewrite_proc_stmt_symbols(
        &stmt,
        owner_proc,
        field_names,
        array_field_names,
        ins_names,
        field_array_slots,
        in_array_slots,
        errors,
    )
}

pub(super) fn expand_nested_struct_ctor_assign(
    instance_var: &str,
    ctor_name: &str,
    ctor_args: &[CallArg],
    struct_def: &omni_frontend::StructDef,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    if !struct_def.type_params.is_empty() {
        errors.push(Diagnostic::semantic(
            format!(
                "processor state constructor '{instance_var} = {ctor_name}(...)' does not support generic struct templates"
            ),
            0,
            0,
        ));
        return Vec::new();
    }

    let scalar_fields = struct_def
        .fields
        .iter()
        .filter(|f| matches!(f.ty, FieldType::Scalar(_)))
        .collect::<Vec<_>>();
    let scalar_param_names = scalar_fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let scalar_defaults = scalar_fields
        .iter()
        .map(|f| {
            f.default.clone().or(Some(match f.ty {
                FieldType::Scalar(PrimitiveType::F32 | PrimitiveType::F64) => Expr::Number(0.0),
                FieldType::Scalar(PrimitiveType::I32 | PrimitiveType::I64) => Expr::Int(0),
                FieldType::Scalar(PrimitiveType::Bool) => Expr::Bool(false),
                _ => Expr::Number(0.0),
            }))
        })
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        ctor_args,
        &scalar_param_names,
        &scalar_defaults,
        false,
        false,
        &format!("processor state struct constructor '{instance_var} = {ctor_name}(...)'"),
        errors,
    );

    let mut out = Vec::<Stmt>::new();
    let mut scalar_idx = 0usize;
    for field in &struct_def.fields {
        let field_path = format!("{instance_var}.{}", field.name);
        match &field.ty {
            FieldType::Scalar(_) => {
                let value = resolved
                    .get(scalar_idx)
                    .copied()
                    .flatten()
                    .cloned()
                    .or_else(|| scalar_defaults.get(scalar_idx).cloned().flatten())
                    .unwrap_or(Expr::Number(0.0));
                out.push(Stmt::Assign {
                    loc: None,
                    target: AssignTarget::Var(field_path),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    expr: value,
                });
                scalar_idx += 1;
            }
            FieldType::Array(_) => {
                if let Some(default) = &field.default {
                    out.push(Stmt::Assign {
                        loc: None,
                        target: AssignTarget::Var(field_path),
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        expr: default.clone(),
                    });
                }
            }
            FieldType::Generic(param) => {
                if let Some(nested_struct_def) = struct_defs.get(param) {
                    out.extend(expand_nested_struct_ctor_assign(
                        &field_path,
                        param,
                        &[],
                        nested_struct_def,
                        struct_defs,
                        errors,
                    ));
                } else {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor state struct field '{}.{}' uses unresolved generic parameter '{}'",
                            instance_var, field.name, param
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
    }
    out
}

pub(super) fn expand_nested_proc_ctor_assign(
    owner_proc: &str,
    nested_var: &str,
    ctor_name: &str,
    ctor_args: &[CallArg],
    callee_param_specs: &[ProcParamSpec],
    callee_buffer_specs: &[ProcBufferSpec],
    proc_array_slot: Option<(usize, usize)>,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<Stmt>, Vec<Expr>) {
    let mut param_names = callee_param_specs
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let mut param_defaults = callee_param_specs
        .iter()
        .map(|p| {
            if p.slots.iter().all(|s| s.default.is_some()) {
                Some(Expr::Number(0.0))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for buffer in callee_buffer_specs {
        param_names.push(buffer.name.clone());
        param_defaults.push(None);
    }
    let resolved = resolve_call_args(
        ctor_args,
        &param_names,
        &param_defaults,
        false,
        true,
        &format!(
            "processor constructor '{}(...)' for nested state '{}'",
            ctor_name, nested_var
        ),
        errors,
    );

    let mut out = Vec::<Stmt>::new();
    let mut bound_buffers = Vec::<Expr>::new();
    for (idx, param) in callee_param_specs.iter().enumerate() {
        let values = match resolved.get(idx).copied().flatten() {
            Some(expr) => {
                let expr = if let Some((array_slot_idx, array_len)) = proc_array_slot {
                    select_proc_array_initializer_expr_for_slot(
                        expr,
                        array_slot_idx,
                        array_len,
                        &format!(
                            "processor constructor '{}(...)' argument '{}' for nested state '{}'",
                            ctor_name, param.name, nested_var
                        ),
                        true,
                        errors,
                    )
                } else {
                    expr.clone()
                };
                expand_expr_to_slots(
                    &expr,
                    param.slots.len(),
                    &format!(
                        "processor constructor '{}(...)' argument '{}'",
                        ctor_name, param.name
                    ),
                    errors,
                )
            }
            None => param
                .slots
                .iter()
                .map(|slot| slot.default.clone().unwrap_or(Expr::Number(0.0)))
                .collect::<Vec<_>>(),
        };
        for (slot_idx, slot) in param.slots.iter().enumerate() {
            let value = values
                .get(slot_idx)
                .cloned()
                .unwrap_or_else(|| slot.default.clone().unwrap_or(Expr::Number(0.0)));
            let value = if let Some(range) = slot.range {
                clamp_expr_to_range(value, range)
            } else {
                value
            };
            out.push(Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(nested_field_name(nested_var, &slot.name)),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: value,
            });
        }
    }
    for (buffer_idx, buffer_spec) in callee_buffer_specs.iter().enumerate() {
        let resolved_idx = callee_param_specs.len() + buffer_idx;
        let Some(mut expr) = resolved.get(resolved_idx).copied().flatten().cloned() else {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' for nested state '{nested_var}' is missing required buffer argument '{}'",
                    buffer_spec.name
                ),
                0,
                0,
            ));
            continue;
        };
        if let Some((array_slot_idx, array_len)) = proc_array_slot {
            expr = select_proc_array_initializer_expr_for_slot(
                &expr,
                array_slot_idx,
                array_len,
                &format!(
                    "processor constructor '{}(...)' buffer argument '{}' for nested state '{}'",
                    ctor_name, buffer_spec.name, nested_var
                ),
                false,
                errors,
            );
        }
        if !matches!(expr, Expr::Var(_)) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' buffer argument '{}' for nested state '{nested_var}' must be a buffer symbol",
                    buffer_spec.name
                ),
                0,
                0,
            ));
        }
        bound_buffers.push(expr);
    }
    out.push(Stmt::Expr {
        loc: None,
        expr: Expr::UserCall {
            name: nested_init_fn_name(owner_proc, nested_var),
            type_args: Vec::new(),
            args: vec![CallArg {
                name: None,
                expr: Expr::Var("self".to_owned()),
            }],
        },
    });
    (out, bound_buffers)
}

pub(super) fn expand_proc_instance_ctor_assign(
    instance_var: &str,
    ctor_name: &str,
    ctor_args: &[CallArg],
    param_specs: &[ProcParamSpec],
    buffer_specs: &[ProcBufferSpec],
    proc_array_slot: Option<(usize, usize)>,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<Stmt>, Vec<Expr>) {
    let mut param_names = param_specs
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let mut param_defaults = param_specs
        .iter()
        .map(|p| {
            if p.slots.iter().all(|s| s.default.is_some()) {
                Some(Expr::Number(0.0))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for buffer in buffer_specs {
        param_names.push(buffer.name.clone());
        param_defaults.push(None);
    }
    let resolved = resolve_call_args(
        ctor_args,
        &param_names,
        &param_defaults,
        false,
        true,
        &format!("processor constructor '{ctor_name}(...)' for instance '{instance_var}'"),
        errors,
    );

    let mut out = Vec::<Stmt>::new();
    let mut bound_buffers = Vec::<Expr>::new();
    for (idx, param) in param_specs.iter().enumerate() {
        let values = match resolved.get(idx).copied().flatten() {
            Some(expr) => {
                let expr = if let Some((array_slot_idx, array_len)) = proc_array_slot {
                    select_proc_array_initializer_expr_for_slot(
                        expr,
                        array_slot_idx,
                        array_len,
                        &format!(
                            "processor constructor '{}(...)' argument '{}' for instance '{}'",
                            ctor_name, param.name, instance_var
                        ),
                        true,
                        errors,
                    )
                } else {
                    expr.clone()
                };
                expand_expr_to_slots(
                    &expr,
                    param.slots.len(),
                    &format!(
                        "processor constructor '{}(...)' argument '{}'",
                        ctor_name, param.name
                    ),
                    errors,
                )
            }
            None => param
                .slots
                .iter()
                .map(|slot| slot.default.clone().unwrap_or(Expr::Number(0.0)))
                .collect::<Vec<_>>(),
        };
        for (slot_idx, slot) in param.slots.iter().enumerate() {
            let value = values
                .get(slot_idx)
                .cloned()
                .unwrap_or_else(|| slot.default.clone().unwrap_or(Expr::Number(0.0)));
            let value = if let Some(range) = slot.range {
                clamp_expr_to_range(value, range)
            } else {
                value
            };
            out.push(Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(format!("{instance_var}.{}", slot.name)),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: value,
            });
        }
    }
    for (buffer_idx, buffer_spec) in buffer_specs.iter().enumerate() {
        let resolved_idx = param_specs.len() + buffer_idx;
        let Some(mut expr) = resolved.get(resolved_idx).copied().flatten().cloned() else {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' for instance '{instance_var}' is missing required buffer argument '{}'",
                    buffer_spec.name
                ),
                0,
                0,
            ));
            continue;
        };
        if let Some((array_slot_idx, array_len)) = proc_array_slot {
            expr = select_proc_array_initializer_expr_for_slot(
                &expr,
                array_slot_idx,
                array_len,
                &format!(
                    "processor constructor '{}(...)' buffer argument '{}' for instance '{}'",
                    ctor_name, buffer_spec.name, instance_var
                ),
                false,
                errors,
            );
        }
        if !matches!(expr, Expr::Var(_)) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' buffer argument '{}' must be a buffer symbol",
                    buffer_spec.name
                ),
                0,
                0,
            ));
        }
        bound_buffers.push(expr);
    }
    (out, bound_buffers)
}

pub(super) fn collect_nested_proc_instances(
    shape: &ProcLoweringShape,
    path_prefix: Option<&str>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    out: &mut Vec<(String, String)>,
) {
    let mut nested_vars = shape.state.nested_procs.keys().cloned().collect::<Vec<_>>();
    nested_vars.sort();
    for nested_var in nested_vars {
        let Some(nested_state) = shape.state.nested_procs.get(&nested_var) else {
            continue;
        };
        let full_path = if let Some(prefix) = path_prefix {
            nested_field_name(prefix, &nested_var)
        } else {
            nested_var.clone()
        };
        out.push((full_path.clone(), nested_state.proc_name.clone()));
        if let Some(child_shape) = lowering_shapes.get(&nested_state.proc_name) {
            collect_nested_proc_instances(child_shape, Some(&full_path), lowering_shapes, out);
        }
    }
}

pub(super) fn lower_callee_stmt_for_nested_wrapper(
    stmt: &Stmt,
    owner_proc: &str,
    nested_path: &str,
    callee_shape: &ProcLoweringShape,
    callee_nested_instances: &HashMap<String, ProcCallInstance>,
    callee_ins_names: &HashSet<String>,
    callee_field_array_slots: &HashMap<String, Vec<String>>,
    callee_in_array_slots: &HashMap<String, Vec<String>>,
    callee_proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    with_stmt_diag_context(stmt, || {
        let mut stmt = stmt.clone();
        let mut remap = callee_shape
            .state
            .nested_procs
            .keys()
            .map(|name| (name.clone(), nested_field_name(nested_path, name)))
            .collect::<HashMap<_, _>>();
        for array_base in callee_shape.state.nested_proc_arrays.keys() {
            remap.insert(
                array_base.clone(),
                nested_field_name(nested_path, array_base),
            );
        }
        remap_nested_symbols_in_stmt(&mut stmt, &remap);

        let nested_fields = callee_shape
            .nested_fields
            .iter()
            .map(|(name, fields)| (nested_field_name(nested_path, name), fields.clone()))
            .collect::<HashMap<_, _>>();
        let nested_instances = callee_nested_instances
            .iter()
            .map(|(name, instance)| {
                let mapped_name = nested_field_name(nested_path, name);
                let mut mapped_instance = instance.clone();
                for expr in &mut mapped_instance.buffer_args {
                    remap_nested_symbols_in_expr(expr, &remap);
                }
                (mapped_name, mapped_instance)
            })
            .collect::<HashMap<_, _>>();
        let mapped_proc_array_slots = callee_proc_array_slots
            .iter()
            .map(|(base, slots)| {
                (
                    nested_field_name(nested_path, base),
                    slots
                        .iter()
                        .map(|slot| nested_field_name(nested_path, slot))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        normalize_proc_output_aliases_in_stmt(&mut stmt, &nested_instances, proc_api);
        rewrite_nested_field_paths_in_stmt(&mut stmt, &nested_fields);
        rewrite_nested_proc_calls_in_stmt(
            &mut stmt,
            owner_proc,
            &nested_instances,
            &mapped_proc_array_slots,
            proc_api,
            errors,
        );

        let mapped_field_array_slots = callee_field_array_slots
            .iter()
            .map(|(base, slots)| {
                (
                    base.clone(),
                    slots
                        .iter()
                        .map(|slot| nested_field_name(nested_path, slot))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();

        let lowered = rewrite_proc_stmt_symbols(
            &stmt,
            owner_proc,
            &callee_shape.field_names,
            &callee_shape.array_field_names,
            callee_ins_names,
            &mapped_field_array_slots,
            callee_in_array_slots,
            errors,
        )?;
        let mut lowered = lowered;
        prefix_self_fields_in_stmt(&mut lowered, nested_path, &callee_shape.field_names);
        Some(lowered)
    })
}

pub(super) fn lower_callee_expr_for_nested_wrapper(
    expr: &Expr,
    owner_proc: &str,
    nested_path: &str,
    callee_shape: &ProcLoweringShape,
    callee_nested_instances: &HashMap<String, ProcCallInstance>,
    callee_field_array_slots: &HashMap<String, Vec<String>>,
    callee_in_array_slots: &HashMap<String, Vec<String>>,
    callee_proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let mut expr = expr.clone();
    let mut remap = callee_shape
        .state
        .nested_procs
        .keys()
        .map(|name| (name.clone(), nested_field_name(nested_path, name)))
        .collect::<HashMap<_, _>>();
    for array_base in callee_shape.state.nested_proc_arrays.keys() {
        remap.insert(
            array_base.clone(),
            nested_field_name(nested_path, array_base),
        );
    }
    remap_nested_symbols_in_expr(&mut expr, &remap);

    let nested_fields = callee_shape
        .nested_fields
        .iter()
        .map(|(name, fields)| (nested_field_name(nested_path, name), fields.clone()))
        .collect::<HashMap<_, _>>();
    let nested_instances = callee_nested_instances
        .iter()
        .map(|(name, instance)| {
            let mapped_name = nested_field_name(nested_path, name);
            let mut mapped_instance = instance.clone();
            for arg_expr in &mut mapped_instance.buffer_args {
                remap_nested_symbols_in_expr(arg_expr, &remap);
            }
            (mapped_name, mapped_instance)
        })
        .collect::<HashMap<_, _>>();
    let mapped_proc_array_slots = callee_proc_array_slots
        .iter()
        .map(|(base, slots)| {
            (
                nested_field_name(nested_path, base),
                slots
                    .iter()
                    .map(|slot| nested_field_name(nested_path, slot))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    normalize_proc_output_aliases_in_expr(&mut expr, &nested_instances, proc_api);
    rewrite_nested_field_paths_in_expr(&mut expr, &nested_fields);
    rewrite_nested_proc_calls_in_expr(
        &mut expr,
        owner_proc,
        &nested_instances,
        &mapped_proc_array_slots,
        proc_api,
        errors,
    );

    let mapped_field_array_slots = callee_field_array_slots
        .iter()
        .map(|(base, slots)| {
            (
                base.clone(),
                slots
                    .iter()
                    .map(|slot| nested_field_name(nested_path, slot))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    rewrite_proc_expr_symbols(
        &mut expr,
        owner_proc,
        &callee_shape.field_names,
        &mapped_field_array_slots,
        callee_in_array_slots,
        errors,
    );
    prefix_self_fields_in_expr(&mut expr, nested_path, &callee_shape.field_names);
    expr
}
