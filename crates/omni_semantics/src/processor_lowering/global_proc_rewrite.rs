use super::*;

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

fn try_dynamic_proc_call_meta<'a>(
    expr: &'a Expr,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a [CallArg], &'a str, &'a Expr)> {
    let Expr::UserCall {
        name,
        args,
        type_args: _,
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
    let Expr::Index { base, index } = &self_arg.expr else {
        return None;
    };
    if matches!(index.as_ref(), Expr::Int(_)) {
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
            Expr::ArrayCtor { spec, init } => {
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
                        base: array_base.to_owned(),
                        index: Box::new(index_expr.clone()),
                    },
                });
                pre_args.extend(args.iter().skip(buffer_start).cloned());
                guards.push(Stmt::If {
                    loc: None,
                    cond: Expr::UnaryNot {
                        expr: Box::new(Expr::Index {
                            base: active_symbol.clone(),
                            index: Box::new(index_expr.clone()),
                        }),
                    },
                    then_branch: vec![
                        Stmt::Expr {
                            loc: None,
                            expr: Expr::UserCall {
                                name: format!("{proc_name}{PROC_BLOCK_PRE_FN_SUFFIX}"),
                                type_args: Vec::new(),
                                args: pre_args,
                            },
                        },
                        Stmt::Assign {
                            loc: None,
                            target: AssignTarget::Index {
                                base: active_symbol,
                                index: index_expr.clone(),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            expr: Expr::Bool(true),
                        },
                    ],
                    else_branch: Vec::new(),
                });
            }
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner }
            | Expr::UnaryBitNot { expr: inner } => {
                collect_guards_from_expr(
                    inner,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                );
            }
            Expr::ArrayLiteral(values) => {
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
            Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
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
        Expr::Index { base, index } if base == array_base && matches!(index.as_ref(), Expr::Int(_))
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
                index: Expr::Int(slot_idx as i64),
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
                expr: Expr::ArrayCtor { spec, init },
                ..
            }) = pending_stmts.first()
            {
                let resolved_proc_ctor = match &spec.elem {
                    ArrayElemType::Struct(elem_name) => {
                        if elem_name.contains("::") {
                            if proc_symbols.contains(elem_name) {
                                Some(elem_name.clone())
                            } else {
                                None
                            }
                        } else {
                            resolve_unqualified_symbol_name(elem_name, "", &proc_symbols)
                        }
                    }
                    ArrayElemType::Primitive(_) => None,
                };
                if let Some(proc_ctor) = resolved_proc_ctor {
                    let size_context =
                        format!("top-level processor array '{}' size", array_var.as_str());
                    let Some(len) = eval_data_size_expr(&spec.size, options, &size_context, errors)
                    else {
                        pending_stmts.clear();
                        continue;
                    };
                    if let Some(values) = init {
                        if values.len() != len && values.len() != 1 {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "top-level processor array '{}' initializer expects {} constructor entries (or a single broadcast constructor), got {}",
                                    array_var,
                                    len,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
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
                                } = value
                                {
                                    let resolved_ctor = if ctor_name.contains("::") {
                                        if proc_symbols.contains(ctor_name) {
                                            Some(ctor_name.clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        resolve_unqualified_symbol_name(
                                            ctor_name,
                                            "",
                                            &proc_symbols,
                                        )
                                    };
                                    if let Some(resolved_ctor) = resolved_ctor {
                                        if resolved_ctor != proc_ctor {
                                            errors.push(Diagnostic::semantic(
                                                format!(
                                                    "top-level processor array '{}' initializer entry {} uses constructor '{}' but '{}' is required",
                                                    array_var, idx, resolved_ctor, proc_ctor
                                                ),
                                                0,
                                                0,
                                            ));
                                        }
                                    } else {
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "top-level processor array '{}' initializer entry {} references unknown processor constructor '{}'",
                                                array_var, idx, ctor_name
                                            ),
                                            0,
                                            0,
                                        ));
                                    }
                                    if !type_args.is_empty() {
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor '{}' is not generic and cannot take type arguments",
                                                ctor_name
                                            ),
                                            0,
                                            0,
                                        ));
                                    }
                                    args = ctor_args.clone();
                                } else {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "top-level processor array '{}' initializer entry {} must be a processor constructor call",
                                            array_var, idx
                                        ),
                                        0,
                                        0,
                                    ));
                                }
                            }
                        }
                        expanded.push(Stmt::Assign {
                            loc: None,
                            target: AssignTarget::Var(slot_var),
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            expr: Expr::UserCall {
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
                            loc: None,
                            target: AssignTarget::Index {
                                base: array_var.clone(),
                                index: Expr::Int(idx as i64),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
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
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{}' is not generic and cannot take type arguments",
                                    ctor_name
                                ),
                                0,
                                0,
                            ));
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
                                loc: None,
                                expr: Expr::UserCall {
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
                                loc: None,
                                expr: Expr::UserCall {
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
                errors.push(Diagnostic::semantic(
                    format!("unknown processor type '{}'", instance.proc_name),
                    0,
                    0,
                ));
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
                loc: None,
                expr: Expr::UserCall {
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
                loc: None,
                expr: Expr::UserCall {
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
                        oversample_factor: None,
                        body: Vec::new(),
                    },
                };
                program.blocks.insert(
                    sample_idx,
                    Block::Block(BlockExec {
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
                    loc: None,
                    target: AssignTarget::Var(info.active_symbol.clone()),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: true,
                    expr: Expr::ArrayCtor {
                        spec: omni_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::Int(len as i64)),
                        },
                        init: Some(vec![Expr::Bool(false); len]),
                    },
                });
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
                        loc: None,
                        target: AssignTarget::Index {
                            base: info.active_symbol.clone(),
                            index: Expr::Int(slot_idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        expr: Expr::Bool(false),
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
                            base: (*base).clone(),
                            index: Box::new(Expr::Int(slot_idx as i64)),
                        },
                    });
                    post_args.extend(expand_proc_buffer_call_args(
                        instance, api, slot_name, errors,
                    ));
                    exec.post.push(Stmt::If {
                        loc: None,
                        cond: Expr::Index {
                            base: info.active_symbol.clone(),
                            index: Box::new(Expr::Int(slot_idx as i64)),
                        },
                        then_branch: vec![Stmt::Expr {
                            loc: None,
                            expr: Expr::UserCall {
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
