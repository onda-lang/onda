use super::*;

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
    let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|b| b.kind() == BlockKind::Init)
    {
        let mut rewritten_init = Vec::<Stmt>::new();
        for stmt in init.clone() {
            let mut pending_stmts = vec![stmt];
            if let Some(Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: Expr::DataCtor { spec, init },
                ..
            }) = pending_stmts.first()
            {
                let resolved_proc_ctor = match &spec.elem {
                    DataElemType::Struct(elem_name) => {
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
                    DataElemType::Primitive(_) => None,
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
                    let mut expanded = Vec::<Stmt>::with_capacity(len);
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
                            expr: Expr::DataCtor { init, .. },
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
                        if let Some(shape) = lowering_shapes.get(ctor_name) {
                            let proc_array_slot =
                                global_proc_array_broadcast_slots.get(var).copied();
                            let (ctor_assigns, buffer_args) = expand_proc_instance_ctor_assign(
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
                            rewritten_init.extend(ctor_assigns);
                            rewritten_init.push(Stmt::Expr {
                                loc: None,
                                expr: Expr::UserCall {
                                    name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                    type_args: Vec::new(),
                                    args: vec![CallArg {
                                        name: None,
                                        expr: Expr::Var(var.clone()),
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
                                        expr: Expr::Var(var.clone()),
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
        *init = rewritten_init;
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
                expr: Expr::Var(instance_name.clone()),
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
                expr: Expr::Var(instance_name.clone()),
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
                for param in &def.params {
                    if let Some(FnParamType::Struct(struct_name)) = &param.ty {
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
                    &global_proc_array_slots,
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
}
