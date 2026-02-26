use super::*;

pub(super) fn rewrite_top_level_proc_calls(
    program: &mut Program,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut global_proc_instances = HashMap::<String, ProcCallInstance>::new();
    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|b| b.kind() == BlockKind::Init)
    {
        let mut rewritten_init = Vec::<Stmt>::new();
        for mut stmt in init.clone() {
            rewrite_proc_calls_in_stmt(&mut stmt, &global_proc_instances, &proc_api, errors);
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
                        let (ctor_assigns, buffer_args) = expand_proc_instance_ctor_assign(
                            var,
                            ctor_name,
                            ctor_args,
                            &shape.param_specs,
                            &shape.buffer_specs,
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
                    ));
                }
            }
            Block::Sample(stmts) => {
                called_proc_instances.extend(collect_called_proc_instances_in_stmts(
                    stmts,
                    &global_proc_instances,
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
                    &proc_api,
                    errors,
                );
                if let Some(sample) = &mut exec.sample {
                    rewrite_proc_calls_in_stmts(sample, &global_proc_instances, &proc_api, errors);
                }
                rewrite_proc_calls_in_stmts(
                    &mut exec.post,
                    &global_proc_instances,
                    &proc_api,
                    errors,
                );
            }
            Block::Sample(stmts) => {
                rewrite_proc_calls_in_stmts(stmts, &global_proc_instances, &proc_api, errors);
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
                rewrite_proc_calls_in_stmts(&mut def.body, &proc_vars, &proc_api, errors);
            }
            Block::Events(events) => {
                for event in events {
                    rewrite_proc_calls_in_stmts(
                        &mut event.body,
                        &global_proc_instances,
                        &proc_api,
                        errors,
                    );
                }
            }
            _ => {}
        }
    }
}
