use super::*;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

#[derive(Debug, Clone)]
struct RuntimeManagedProcArray {
    proc_name: String,
    slots: Vec<String>,
    active_symbol: String,
}

fn sanitize_runtime_symbol_component(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn runtime_proc_array_active_symbol(array_base: &str) -> String {
    format!(
        "__omni_proc_block_active_{}",
        sanitize_runtime_symbol_component(array_base)
    )
}

fn specialized_proc_template_bases(proc_symbols: &HashSet<String>) -> HashSet<String> {
    proc_symbols
        .iter()
        .filter_map(|name| {
            name.rsplit_once(".__gen__")
                .map(|(base, _)| base.to_owned())
        })
        .collect()
}

fn resolve_proc_ctor_symbol_name(
    ctor_name: &str,
    current_ns: &str,
    proc_symbols: &HashSet<String>,
) -> Option<String> {
    let direct = if ctor_name.contains("::") {
        proc_symbols
            .contains(ctor_name)
            .then_some(ctor_name.to_owned())
    } else {
        resolve_unqualified_symbol_name(ctor_name, current_ns, proc_symbols)
    };
    if direct.is_some() {
        return direct;
    }

    let resolved_base = if ctor_name.contains("::") {
        ctor_name.to_owned()
    } else {
        let template_bases = specialized_proc_template_bases(proc_symbols);
        resolve_unqualified_symbol_name(ctor_name, current_ns, &template_bases)?
    };
    let prefix = format!("{resolved_base}.__gen__");
    let mut matches = proc_symbols
        .iter()
        .filter(|name| name.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn try_dynamic_proc_call_meta<'a>(
    expr: &'a Expr,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a [CallArg], &'a str, &'a Expr)> {
    let Expr::UserCall {
        name,
        args,
        type_args: _,
        ..
    } = expr
    else {
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
    if matches!(index.as_ref(), Expr::Int { .. }) {
        return None;
    }
    Some((proc_name, args, base.as_str(), index.as_ref()))
}

fn rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
    stmt: Stmt,
    proc_api: &HashMap<String, ProcApi>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    runtime_managed_arrays: &mut HashMap<String, RuntimeManagedProcArray>,
) -> Vec<Stmt> {
    fn collect_guards_from_expr(
        expr: &Expr,
        proc_api: &HashMap<String, ProcApi>,
        proc_array_slots: &HashMap<String, Vec<String>>,
        runtime_managed_arrays: &mut HashMap<String, RuntimeManagedProcArray>,
        guards: &mut Vec<Stmt>,
    ) {
        match expr {
            Expr::Index { index, .. } => collect_guards_from_expr(
                index,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                guards,
            ),
            Expr::Slice { start, end, .. } => {
                if let Some(start) = start {
                    collect_guards_from_expr(
                        start,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                    );
                }
                if let Some(end) = end {
                    collect_guards_from_expr(
                        end,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                    );
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_guards_from_expr(
                    &spec.size,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                );
                if let Some(values) = init {
                    for value in values {
                        collect_guards_from_expr(
                            value,
                            proc_api,
                            proc_array_slots,
                            runtime_managed_arrays,
                            guards,
                        );
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                collect_guards_from_expr(
                    lhs,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                );
                collect_guards_from_expr(
                    rhs,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                );
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    collect_guards_from_expr(
                        arg,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                    );
                }
            }
            Expr::UserCall {
                name: _,
                args,
                type_args: _,
                ..
            } => {
                for arg in args {
                    collect_guards_from_expr(
                        &arg.expr,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                    );
                }
                let Some((proc_name, args, array_base, index_expr)) =
                    try_dynamic_proc_call_meta(expr, proc_api)
                else {
                    return;
                };
                let Some(api) = proc_api.get(proc_name) else {
                    return;
                };
                let Some(slots) = proc_array_slots.get(array_base) else {
                    return;
                };
                runtime_managed_arrays
                    .entry(array_base.to_owned())
                    .or_insert_with(|| RuntimeManagedProcArray {
                        proc_name: proc_name.to_owned(),
                        slots: slots.clone(),
                        active_symbol: runtime_proc_array_active_symbol(array_base),
                    });
                let active_symbol = runtime_proc_array_active_symbol(array_base);
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
                collect_guards_from_expr(
                    inner,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                );
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    collect_guards_from_expr(
                        value,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                    );
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    let mut guards = Vec::<Stmt>::new();
    match &stmt {
        Stmt::Expr { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
            collect_guards_from_expr(
                expr,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut guards,
            );
        }
        _ => {}
    }
    if !guards.is_empty() {
        let mut rewritten = guards;
        rewritten.push(stmt);
        return rewritten;
    }

    match stmt {
        Stmt::Const { .. } => Vec::new(),
        Stmt::If {
            loc,
            cond,
            then_branch,
            else_branch,
        } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &cond,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut cond_guards,
            );
            let mut rewritten_then = Vec::<Stmt>::new();
            for nested in then_branch {
                rewritten_then.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                ));
            }
            let mut rewritten_else = Vec::<Stmt>::new();
            for nested in else_branch {
                rewritten_else.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                ));
            }
            cond_guards.push(Stmt::If {
                loc,
                cond,
                then_branch: rewritten_then,
                else_branch: rewritten_else,
            });
            cond_guards
        }
        Stmt::For {
            loc,
            var,
            start,
            end,
            step,
            end_inclusive,
            body,
        } => {
            let mut range_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &start,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut range_guards,
            );
            collect_guards_from_expr(
                &end,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut range_guards,
            );
            if let Some(step_expr) = &step {
                collect_guards_from_expr(
                    step_expr,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    &mut range_guards,
                );
            }
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                ));
            }
            range_guards.push(Stmt::For {
                loc,
                var,
                start,
                end,
                step,
                end_inclusive,
                body: rewritten_body,
            });
            range_guards
        }
        Stmt::While { loc, cond, body } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &cond,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut cond_guards,
            );
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                ));
            }
            cond_guards.push(Stmt::While {
                loc,
                cond,
                body: rewritten_body,
            });
            cond_guards
        }
        _ => vec![stmt],
    }
}

fn is_static_block_hook_for_managed_array(
    stmt: &Stmt,
    proc_name: &str,
    array_base: &str,
    suffix: &str,
) -> bool {
    let Stmt::Expr {
        expr:
            Expr::UserCall {
                name,
                args,
                type_args: _,
                ..
            },
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
    matches!(
        &self_arg.expr,
        Expr::Index { base, index, .. } if base == array_base && matches!(index.as_ref(), Expr::Int { .. })
    )
}

fn remap_proc_ctor_assign_for_array_slot(
    mut stmt: Stmt,
    slot_var: &str,
    array_base: &str,
    slot_idx: usize,
) -> Stmt {
    if let Stmt::Assign { target, .. } = &mut stmt {
        let AssignTarget::Var(name) = target else {
            return stmt;
        };
        let prefix = format!("{slot_var}.");
        if let Some(field_path) = name.strip_prefix(&prefix) {
            *target = AssignTarget::Index {
                base: format!("{array_base}.{field_path}"),
                index: Expr::int(slot_idx as i64),
            };
        }
    }
    stmt
}

pub(super) fn rewrite_top_level_proc_calls(
    program: &mut Program,
    options: AnalysisOptions,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut global_proc_instances = HashMap::<String, ProcCallInstance>::new();
    let mut global_proc_array_slots = HashMap::<String, Vec<String>>::new();
    let mut global_proc_array_broadcast_slots = HashMap::<String, (usize, usize)>::new();
    let mut runtime_managed_arrays = HashMap::<String, RuntimeManagedProcArray>::new();
    let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|b| b.kind() == BlockKind::Init)
    {
        let mut rewritten_init = Vec::<Stmt>::new();
        for stmt in init.body.clone() {
            let mut pending_stmts = vec![stmt];
            if let Some(Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: expr @ Expr::ArrayCtor { spec, init, .. },
                ..
            }) = pending_stmts.first()
            {
                let resolved_proc_ctor = match &spec.elem {
                    ArrayElemType::Struct(elem_name) => {
                        resolve_proc_ctor_symbol_name(elem_name, "", &proc_symbols)
                    }
                    ArrayElemType::Primitive(_) => None,
                };
                if let Some(proc_ctor) = resolved_proc_ctor {
                    let size_context =
                        format!("top-level processor array '{}' size", array_var.as_str());
                    let Some(len) = with_expr_diag_context(&spec.size, |_diag| {
                        eval_data_size_expr(&spec.size, options, &size_context, errors)
                    }) else {
                        pending_stmts.clear();
                        continue;
                    };
                    if let Some(values) = init {
                        if values.len() != len && values.len() != 1 {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "top-level processor array '{}' initializer expects {} constructor entries (or a single broadcast constructor), got {}",
                                        array_var,
                                        len,
                                        values.len()
                                    ),
                                );
                            });
                        }
                    }
                    let slot_names = (0..len)
                        .map(|idx| format!("{array_var}[{idx}]"))
                        .collect::<Vec<_>>();
                    global_proc_array_slots.insert(array_var.clone(), slot_names.clone());
                    let mut expanded = Vec::<Stmt>::with_capacity(len + 1);
                    if let Some(mut decl_stmt) = pending_stmts.first().cloned() {
                        if let Stmt::Assign {
                            expr: Expr::ArrayCtor { init, .. },
                            ..
                        } = &mut decl_stmt
                        {
                            *init = None;
                        }
                        expanded.push(decl_stmt);
                    }
                    for idx in 0..len {
                        let mut args = Vec::<CallArg>::new();
                        let slot_var = format!("{array_var}[{idx}]");
                        if let Some(values) = init {
                            let value = if values.len() == 1 {
                                global_proc_array_broadcast_slots
                                    .insert(slot_var.clone(), (idx, len));
                                values.first()
                            } else {
                                values.get(idx)
                            };
                            if let Some(value) = value {
                                if let Expr::UserCall {
                                    name: ctor_name,
                                    type_args,
                                    args: ctor_args,
                                    ..
                                } = value
                                {
                                    let resolved_ctor =
                                        resolve_proc_ctor_symbol_name(ctor_name, "", &proc_symbols);
                                    if let Some(resolved_ctor) = resolved_ctor {
                                        if resolved_ctor != proc_ctor {
                                            with_expr_diag_context(value, |expr_diag| {
                                                push_semantic(
                                                    expr_diag,
                                                    errors,
                                                    format!(
                                                        "top-level processor array '{}' initializer entry {} uses constructor '{}' but '{}' is required",
                                                        array_var, idx, resolved_ctor, proc_ctor
                                                    ),
                                                );
                                            });
                                        }
                                    } else {
                                        with_expr_diag_context(value, |expr_diag| {
                                            push_semantic(
                                                expr_diag,
                                                errors,
                                                format!(
                                                    "top-level processor array '{}' initializer entry {} references unknown processor constructor '{}'",
                                                    array_var, idx, ctor_name
                                                ),
                                            );
                                        });
                                    }
                                    if !type_args.is_empty() {
                                        with_expr_diag_context(value, |expr_diag| {
                                            push_semantic(
                                                expr_diag,
                                                errors,
                                                format!(
                                                    "processor '{}' is not generic and cannot take type arguments",
                                                    ctor_name
                                                ),
                                            );
                                        });
                                    }
                                    args = ctor_args.clone();
                                } else {
                                    with_expr_diag_context(value, |expr_diag| {
                                        push_semantic(
                                            expr_diag,
                                            errors,
                                            format!(
                                                "top-level processor array '{}' initializer entry {} must be a processor constructor call",
                                                array_var, idx
                                            ),
                                        );
                                    });
                                }
                            }
                        }
                        expanded.push(Stmt::Assign {
                            loc: Default::default(),
                            target_loc: Default::default(),
                            target: AssignTarget::Var(slot_var),
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: proc_ctor.clone(),
                                type_args: Vec::new(),
                                args,
                            },
                        });
                    }
                    pending_stmts = expanded;
                } else if let Some(values) = init {
                    let mut expanded = Vec::<Stmt>::with_capacity(values.len() + 1);
                    if let Some(mut decl_stmt) = pending_stmts.first().cloned() {
                        if let Stmt::Assign {
                            expr: Expr::ArrayCtor { init, .. },
                            ..
                        } = &mut decl_stmt
                        {
                            *init = None;
                        }
                        expanded.push(decl_stmt);
                    }
                    for (idx, value) in values.iter().cloned().enumerate() {
                        expanded.push(Stmt::Assign {
                            loc: Default::default(),
                            target_loc: Default::default(),
                            target: AssignTarget::Index {
                                base: array_var.clone(),
                                index: Expr::int(idx as i64),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: value,
                        });
                    }
                    pending_stmts = expanded;
                }
            }
            for mut stmt in pending_stmts {
                rewrite_proc_calls_in_stmt(
                    &mut stmt,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    &proc_api,
                    errors,
                );
                if let Stmt::Assign {
                    target: AssignTarget::Var(var),
                    expr:
                        Expr::UserCall {
                            name: ctor_name,
                            type_args: ctor_type_args,
                            args: ctor_args,
                            ..
                        },
                    ..
                } = &stmt
                {
                    if proc_api.contains_key(ctor_name) {
                        if !ctor_type_args.is_empty() {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' is not generic and cannot take type arguments",
                                    ctor_name
                                ),
                            );
                        }
                        let array_slot = find_proc_array_slot(var, &global_proc_array_slots);
                        if array_slot.is_none() {
                            let mut ctor_stmt = stmt.clone();
                            if let Stmt::Assign {
                                expr:
                                    Expr::UserCall {
                                        type_args, args, ..
                                    },
                                ..
                            } = &mut ctor_stmt
                            {
                                type_args.clear();
                                args.clear();
                            }
                            rewritten_init.push(ctor_stmt);
                        }
                        if let Some(shape) = lowering_shapes.get(ctor_name) {
                            let proc_array_slot =
                                global_proc_array_broadcast_slots.get(var).copied();
                            let (mut ctor_assigns, buffer_args) = expand_proc_instance_ctor_assign(
                                var,
                                ctor_name,
                                ctor_args,
                                &shape.param_specs,
                                &shape.buffer_specs,
                                proc_array_slot,
                                errors,
                            );
                            global_proc_instances.insert(
                                var.clone(),
                                ProcCallInstance {
                                    proc_name: ctor_name.clone(),
                                    buffer_args: buffer_args.clone(),
                                },
                            );
                            if let Some((array_base, slot_idx)) = array_slot.as_ref() {
                                ctor_assigns = ctor_assigns
                                    .into_iter()
                                    .map(|assign| {
                                        remap_proc_ctor_assign_for_array_slot(
                                            assign,
                                            var,
                                            array_base.as_str(),
                                            *slot_idx,
                                        )
                                    })
                                    .collect::<Vec<_>>();
                            }
                            rewritten_init.extend(ctor_assigns);
                            rewritten_init.push(Stmt::Expr {
                                loc: Default::default(),
                                expr: Expr::UserCall {
                                    loc: Default::default(),
                                    name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                    type_args: Vec::new(),
                                    args: vec![CallArg {
                                        name: None,
                                        expr: proc_instance_self_expr(
                                            var,
                                            &global_proc_array_slots,
                                        ),
                                    }],
                                },
                            });
                        } else {
                            rewritten_init.push(Stmt::Expr {
                                loc: Default::default(),
                                expr: Expr::UserCall {
                                    loc: Default::default(),
                                    name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                    type_args: Vec::new(),
                                    args: vec![CallArg {
                                        name: None,
                                        expr: proc_instance_self_expr(
                                            var,
                                            &global_proc_array_slots,
                                        ),
                                    }],
                                },
                            });
                        }
                        continue;
                    }
                }
                rewritten_init.push(stmt);
            }
        }
        init.body = rewritten_init;
        rewrite_proc_calls_in_stmts(
            &mut init.body,
            &global_proc_instances,
            &global_proc_array_slots,
            &proc_api,
            errors,
        );
    }

    let mut called_proc_instances = HashSet::<String>::new();
    for block in &program.blocks {
        match block {
            Block::Block(exec) => {
                if let Some(sample) = &exec.sample {
                    called_proc_instances.extend(collect_called_proc_instances_in_stmts(
                        sample,
                        &global_proc_instances,
                        &global_proc_array_slots,
                    ));
                }
            }
            Block::Sample(stmts) => {
                called_proc_instances.extend(collect_called_proc_instances_in_stmts(
                    stmts,
                    &global_proc_instances,
                    &global_proc_array_slots,
                ));
            }
            _ => {}
        }
    }
    if !called_proc_instances.is_empty() {
        let mut called_order = called_proc_instances.into_iter().collect::<Vec<_>>();
        called_order.sort();
        let mut injected_block_pre = Vec::<Stmt>::new();
        let mut injected_block_post = Vec::<Stmt>::new();
        for instance_name in called_order {
            let Some(instance) = global_proc_instances.get(&instance_name) else {
                continue;
            };
            let Some(api) = proc_api.get(&instance.proc_name) else {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!("unknown processor type '{}'", instance.proc_name),
                );
                continue;
            };
            if !api.has_block {
                continue;
            }
            let mut pre_args = Vec::<CallArg>::new();
            pre_args.push(CallArg {
                name: None,
                expr: proc_instance_self_expr(&instance_name, &global_proc_array_slots),
            });
            pre_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_block_pre.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_PRE_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: pre_args,
                },
            });

            let mut post_args = Vec::<CallArg>::new();
            post_args.push(CallArg {
                name: None,
                expr: proc_instance_self_expr(&instance_name, &global_proc_array_slots),
            });
            post_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_block_post.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_POST_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: post_args,
                },
            });
        }

        if !injected_block_pre.is_empty() || !injected_block_post.is_empty() {
            if let Some(block_idx) = program
                .blocks
                .iter()
                .position(|b| matches!(b, Block::Block(_)))
            {
                if let Block::Block(exec) = &mut program.blocks[block_idx] {
                    let mut pre = injected_block_pre;
                    pre.append(&mut exec.pre);
                    exec.pre = pre;
                    exec.post.extend(injected_block_post);
                }
            } else if let Some(sample_idx) = program
                .blocks
                .iter()
                .position(|b| matches!(b, Block::Sample(_)))
            {
                let sample_body = match program.blocks.remove(sample_idx) {
                    Block::Sample(stmts) => stmts,
                    _ => SampleBlock {
                        loc: Default::default(),
                        oversample_factor: None,
                        body: Vec::new(),
                    },
                };
                program.blocks.insert(
                    sample_idx,
                    Block::Block(BlockExec {
                        loc: Default::default(),
                        pre: injected_block_pre,
                        sample: Some(sample_body),
                        post: injected_block_post,
                    }),
                );
            }
        }
    }

    for block in &mut program.blocks {
        match block {
            Block::Block(exec) => {
                rewrite_proc_calls_in_stmts(
                    &mut exec.pre,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    &proc_api,
                    errors,
                );
                if let Some(sample) = &mut exec.sample {
                    rewrite_proc_calls_in_stmts(
                        sample,
                        &global_proc_instances,
                        &global_proc_array_slots,
                        &proc_api,
                        errors,
                    );
                    let mut rewritten_sample = Vec::<Stmt>::new();
                    for stmt in std::mem::take(&mut sample.body) {
                        rewritten_sample.extend(
                            rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                                stmt,
                                &proc_api,
                                &global_proc_array_slots,
                                &mut runtime_managed_arrays,
                            ),
                        );
                    }
                    sample.body = rewritten_sample;
                }
                rewrite_proc_calls_in_stmts(
                    &mut exec.post,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    &proc_api,
                    errors,
                );
            }
            Block::Sample(stmts) => {
                rewrite_proc_calls_in_stmts(
                    stmts,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    &proc_api,
                    errors,
                );
                let mut rewritten_sample = Vec::<Stmt>::new();
                for stmt in std::mem::take(&mut stmts.body) {
                    rewritten_sample.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                        stmt,
                        &proc_api,
                        &global_proc_array_slots,
                        &mut runtime_managed_arrays,
                    ));
                }
                stmts.body = rewritten_sample;
            }
            Block::Def(def) => {
                let mut proc_vars = HashMap::<String, ProcCallInstance>::new();
                let mut proc_array_slots = global_proc_array_slots.clone();
                for param in &def.params {
                    if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                        if let Some(shape) = lowering_shapes.get(struct_name) {
                            for (base, slots) in &shape.nested_proc_array_slots {
                                proc_array_slots
                                    .entry(base.clone())
                                    .or_insert_with(|| slots.clone());
                                let prefixed_base = format!("{}.{}", param.name, base);
                                let prefixed_slots = slots
                                    .iter()
                                    .map(|slot| format!("{}.{}", param.name, slot))
                                    .collect::<Vec<_>>();
                                proc_array_slots
                                    .entry(prefixed_base)
                                    .or_insert(prefixed_slots);
                                for slot in slots {
                                    if let Some(nested) = shape.state.nested_procs.get(slot) {
                                        proc_vars.entry(slot.clone()).or_insert(ProcCallInstance {
                                            proc_name: nested.proc_name.clone(),
                                            buffer_args: Vec::new(),
                                        });
                                        let prefixed_slot = format!("{}.{}", param.name, slot);
                                        proc_vars.entry(prefixed_slot).or_insert(
                                            ProcCallInstance {
                                                proc_name: nested.proc_name.clone(),
                                                buffer_args: Vec::new(),
                                            },
                                        );
                                    }
                                }
                            }
                            for (instance_name, nested) in &shape.state.nested_procs {
                                proc_vars.entry(instance_name.clone()).or_insert(
                                    ProcCallInstance {
                                        proc_name: nested.proc_name.clone(),
                                        buffer_args: Vec::new(),
                                    },
                                );
                                let prefixed_instance = format!("{}.{}", param.name, instance_name);
                                proc_vars
                                    .entry(prefixed_instance)
                                    .or_insert(ProcCallInstance {
                                        proc_name: nested.proc_name.clone(),
                                        buffer_args: Vec::new(),
                                    });
                            }
                        }
                        if proc_api.contains_key(struct_name) {
                            proc_vars.insert(
                                param.name.clone(),
                                ProcCallInstance {
                                    proc_name: struct_name.clone(),
                                    buffer_args: Vec::new(),
                                },
                            );
                        }
                    }
                }
                rewrite_proc_calls_in_stmts(
                    &mut def.body,
                    &proc_vars,
                    &proc_array_slots,
                    &proc_api,
                    errors,
                );
            }
            Block::Events(events) => {
                for event in events {
                    rewrite_proc_calls_in_stmts(
                        &mut event.body,
                        &global_proc_instances,
                        &global_proc_array_slots,
                        &proc_api,
                        errors,
                    );
                }
            }
            _ => {}
        }
    }

    if !runtime_managed_arrays.is_empty() {
        if let Some(Block::Init(init)) = program
            .blocks
            .iter_mut()
            .find(|b| b.kind() == BlockKind::Init)
        {
            let mut managed = runtime_managed_arrays.iter().collect::<Vec<_>>();
            managed.sort_by(|(a, _), (b, _)| a.cmp(b));
            for (_base, info) in managed {
                let len = info.slots.len();
                init.body.push(Stmt::Assign {
                    loc: Default::default(),
                    target_loc: Default::default(),
                    target: AssignTarget::Var(info.active_symbol.clone()),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: true,
                    typed_decl_ty_loc: Default::default(),
                    expr: Expr::ArrayCtor {
                        loc: Default::default(),
                        spec: omni_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::int(len as i64)),
                        },
                        init: Some(vec![Expr::bool(false); len]),
                    },
                });
            }
        }

        if !program.blocks.iter().any(|b| matches!(b, Block::Block(_))) {
            if let Some(sample_idx) = program
                .blocks
                .iter()
                .position(|b| matches!(b, Block::Sample(_)))
            {
                let sample_body = match program.blocks.remove(sample_idx) {
                    Block::Sample(sample) => sample,
                    _ => SampleBlock {
                        loc: Default::default(),
                        oversample_factor: None,
                        body: Vec::new(),
                    },
                };
                program.blocks.insert(
                    sample_idx,
                    Block::Block(BlockExec {
                        loc: sample_body.loc,
                        pre: Vec::new(),
                        sample: Some(sample_body),
                        post: Vec::new(),
                    }),
                );
            }
        }

        if let Some(Block::Block(exec)) = program
            .blocks
            .iter_mut()
            .find(|b| matches!(b, Block::Block(_)))
        {
            let mut reset_prefix = Vec::<Stmt>::new();
            let mut managed = runtime_managed_arrays.iter().collect::<Vec<_>>();
            managed.sort_by(|(a, _), (b, _)| a.cmp(b));
            for (_base, info) in &managed {
                for slot_idx in 0..info.slots.len() {
                    reset_prefix.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: info.active_symbol.clone(),
                            index: Expr::int(slot_idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: Expr::bool(false),
                    });
                }
            }

            exec.pre.retain(|stmt| {
                !managed.iter().any(|(base, info)| {
                    is_static_block_hook_for_managed_array(
                        stmt,
                        &info.proc_name,
                        base,
                        PROC_BLOCK_PRE_FN_SUFFIX,
                    )
                })
            });
            exec.post.retain(|stmt| {
                !managed.iter().any(|(base, info)| {
                    is_static_block_hook_for_managed_array(
                        stmt,
                        &info.proc_name,
                        base,
                        PROC_BLOCK_POST_FN_SUFFIX,
                    )
                })
            });

            let mut old_pre = std::mem::take(&mut exec.pre);
            reset_prefix.append(&mut old_pre);
            exec.pre = reset_prefix;

            for (base, info) in managed {
                for (slot_idx, slot_name) in info.slots.iter().enumerate() {
                    let Some(instance) = global_proc_instances.get(slot_name) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&info.proc_name) else {
                        continue;
                    };
                    let mut post_args = Vec::<CallArg>::new();
                    post_args.push(CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: (*base).clone(),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                    });
                    post_args.extend(expand_proc_buffer_call_args(
                        instance, api, slot_name, errors,
                    ));
                    exec.post.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::Index {
                            loc: Default::default(),
                            base: info.active_symbol.clone(),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                        then_branch: vec![Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: format!("{}{}", info.proc_name, PROC_BLOCK_POST_FN_SUFFIX),
                                type_args: Vec::new(),
                                args: post_args,
                            },
                        }],
                        else_branch: Vec::new(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalysisOptions;
    use omni_frontend::{parse_program, AssignTarget, Expr};

    #[test]
    fn sample_only_dynamic_proc_array_calls_gain_runtime_block_hooks() {
        let src = r#"
proc Voice:
  outs:
    out1
  init:
    phase = 0.0
  block:
    step = 0.125
    sample:
      phase = phase + step
      out1 = phase

outs:
  out1

init:
  voices: Voice[2] = Voice()

sample:
  mix = 0.0
  for i in 0..2:
    mix = mix + voices[i]()
  out1 = mix
"#;
        let typed = analyze_with_options(
            parse_program(src).expect("parse should succeed"),
            AnalysisOptions::default(),
        )
        .expect("dynamic proc-array sample should analyze");

        assert!(
            typed
                .array_vars
                .iter()
                .any(|array| array.name == "__omni_proc_block_active_voices"
                    && array.elem_ty == PrimitiveType::Bool
                    && array.len == 2),
            "expected runtime managed active array in typed program: {:?}",
            typed.array_vars
        );
        assert!(
            typed.block_pre.iter().any(|stmt| matches!(
                stmt,
                Stmt::Assign {
                    target: AssignTarget::Index { base, index },
                    expr: Expr::Bool { value: false, .. },
                    ..
                } if base == "__omni_proc_block_active_voices"
                    && matches!(index, Expr::Int { value: 0, .. } | Expr::Int { value: 1, .. })
            )),
            "expected managed active-slot reset in block_pre: {:?}",
            typed.block_pre
        );
        assert!(
            !typed.block_post.is_empty(),
            "expected managed proc-array post hooks in block_post"
        );
    }
}
