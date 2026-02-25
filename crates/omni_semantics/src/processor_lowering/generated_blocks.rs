use super::*;

#[allow(clippy::too_many_arguments)]
fn generate_nested_wrapper_defs(
    proc: &ProcessorDef,
    shape: &ProcLoweringShape,
    proc_defs_by_name: &HashMap<String, ProcessorDef>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    struct_defs_by_name: &HashMap<String, StructDef>,
    proc_api: &HashMap<String, ProcApi>,
    nested_instances: &HashMap<String, ProcCallInstance>,
    ins_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Block> {
    let mut nested_defs = Vec::<Block>::new();
    let mut nested_paths = Vec::<(String, String)>::new();
    collect_nested_proc_instances(&shape, None, &lowering_shapes, &mut nested_paths);
    for (nested_path, callee_proc_name) in nested_paths {
        let Some(callee_proc) = proc_defs_by_name.get(&callee_proc_name) else {
            continue;
        };
        let Some(callee_shape) = lowering_shapes.get(&callee_proc_name).cloned() else {
            continue;
        };
        let mut callee_ins_names = callee_shape.ins.iter().cloned().collect::<HashSet<_>>();
        for port in &callee_shape.in_ports {
            callee_ins_names.insert(port.name.clone());
        }
        for buffer in &callee_shape.buffer_specs {
            callee_ins_names.insert(buffer.name.clone());
        }
        let mut callee_nested_instances = callee_shape
            .state
            .nested_procs
            .iter()
            .map(|(name, state)| {
                (
                    name.clone(),
                    ProcCallInstance {
                        proc_name: state.proc_name.clone(),
                        buffer_args: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let mut nested_init_body = Vec::<Stmt>::new();
        for stmt in &callee_proc.init {
            if let Stmt::Assign {
                target: AssignTarget::Var(var),
                expr:
                    Expr::UserCall {
                        name: ctor_name,
                        type_args,
                        args,
                        ..
                    },
                ..
            } = stmt
            {
                if let Some(nested_state) = callee_shape.state.nested_procs.get(var) {
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
                    if nested_state.proc_name != *ctor_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    }
                    let Some(nested_callee_shape) = lowering_shapes.get(&nested_state.proc_name)
                    else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                callee_proc_name, var, nested_state.proc_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    };
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        &nested_field_name(&nested_path, var),
                        ctor_name,
                        args,
                        &nested_callee_shape.param_specs,
                        &nested_callee_shape.buffer_specs,
                        errors,
                    );
                    if let Some(instance) = callee_nested_instances.get_mut(var) {
                        instance.buffer_args = bound_buffers;
                    }
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.data_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        ) {
                            nested_init_body.push(rewritten);
                        }
                    }
                    continue;
                }
                if let Some(state_struct) = callee_shape.state.struct_instances.get(var) {
                    if state_struct.struct_name != *ctor_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    }
                    let Some(struct_def) = resolve_proc_state_struct_def(
                        &callee_proc_name,
                        var,
                        state_struct,
                        &struct_defs_by_name,
                        errors,
                    ) else {
                        continue;
                    };
                    if !type_args.is_empty() {
                        let Some(resolved_type_args) = resolve_explicit_call_type_args(
                            type_args,
                            &format!("processor state constructor '{} = {}(...)'", var, ctor_name),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    let expanded = expand_nested_struct_ctor_assign(
                        &nested_field_name(&nested_path, var),
                        ctor_name,
                        args,
                        &struct_def,
                        errors,
                    );
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.data_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        ) {
                            nested_init_body.push(rewritten);
                        }
                    }
                    continue;
                }
            }
            if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                stmt,
                &proc.name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &proc_api,
                errors,
            ) {
                nested_init_body.push(rewritten);
            }
        }

        nested_defs.push(Block::Def(FunctionDef {
            type_params: Vec::new(),
            name: nested_init_fn_name(&proc.name, &nested_path),
            params: vec![omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            }],
            body: nested_init_body,
        }));

        let nested_step_body = callee_proc
            .sample
            .iter()
            .filter_map(|stmt| {
                lower_callee_stmt_for_nested_wrapper(
                    stmt,
                    &proc.name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &proc_api,
                    errors,
                )
            })
            .collect::<Vec<_>>();
        let mut nested_step_params = Vec::<omni_frontend::FnParamDecl>::new();
        nested_step_params.push(omni_frontend::FnParamDecl {
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            default: None,
        });
        for in_name in &callee_shape.ins {
            nested_step_params.push(omni_frontend::FnParamDecl {
                name: in_name.clone(),
                ty: None,
                default: None,
            });
        }
        for buffer in &callee_shape.buffer_specs {
            nested_step_params.push(omni_frontend::FnParamDecl {
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                default: None,
            });
        }
        nested_defs.push(Block::Def(FunctionDef {
            type_params: Vec::new(),
            name: nested_step_fn_name(&proc.name, &nested_path),
            params: nested_step_params.clone(),
            body: nested_step_body,
        }));

        let callee_has_effective_block = proc_api
            .get(&callee_proc_name)
            .map(|api| api.has_block)
            .unwrap_or(callee_proc.has_block_block);
        if callee_has_effective_block {
            let mut nested_block_params = Vec::<omni_frontend::FnParamDecl>::new();
            nested_block_params.push(omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            });
            for buffer in &callee_shape.buffer_specs {
                nested_block_params.push(omni_frontend::FnParamDecl {
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    default: None,
                });
            }
            let mut nested_block_pre_body = Vec::<Stmt>::new();
            for stmt in &callee_proc.block_pre {
                if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                    stmt,
                    &proc.name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &proc_api,
                    errors,
                ) {
                    nested_block_pre_body.push(rewritten);
                }
            }
            let called_callee_nested = collect_called_proc_instances_in_stmts(
                &callee_proc.sample,
                &callee_nested_instances,
            );
            let mut callee_nested_vars =
                callee_nested_instances.keys().cloned().collect::<Vec<_>>();
            callee_nested_vars.sort();
            for nested_var in &callee_nested_vars {
                if !called_callee_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = callee_nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !api.has_block {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                nested_block_pre_body.push(Stmt::Expr {
                    loc: None,
                    expr: Expr::UserCall {
                        name: nested_block_pre_fn_name(
                            &proc.name,
                            &nested_field_name(&nested_path, nested_var),
                        ),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            nested_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: nested_block_pre_fn_name(&proc.name, &nested_path),
                params: nested_block_params.clone(),
                body: nested_block_pre_body,
            }));

            let mut nested_block_post_body = Vec::<Stmt>::new();
            for nested_var in &callee_nested_vars {
                if !called_callee_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = callee_nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !api.has_block {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                nested_block_post_body.push(Stmt::Expr {
                    loc: None,
                    expr: Expr::UserCall {
                        name: nested_block_post_fn_name(
                            &proc.name,
                            &nested_field_name(&nested_path, nested_var),
                        ),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            for stmt in &callee_proc.block_post {
                if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                    stmt,
                    &proc.name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &proc_api,
                    errors,
                ) {
                    nested_block_post_body.push(rewritten);
                }
            }
            nested_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: nested_block_post_fn_name(&proc.name, &nested_path),
                params: nested_block_params,
                body: nested_block_post_body,
            }));
        }

        for (idx, out_name) in callee_shape.outs.iter().enumerate() {
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::Var("self".to_owned()),
            }];
            for in_name in &callee_shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::Var(in_name.clone()),
                });
            }
            for buffer in &callee_shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::Var(buffer.name.clone()),
                });
            }
            nested_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: nested_call_out_fn_name(&proc.name, &nested_path, idx),
                params: nested_step_params.clone(),
                body: vec![
                    Stmt::Expr {
                        loc: None,
                        expr: Expr::UserCall {
                            name: nested_step_fn_name(&proc.name, &nested_path),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    },
                    Stmt::Return {
                        loc: None,
                        expr: Expr::Var(format!(
                            "self.{}",
                            nested_field_name(&nested_path, out_name)
                        )),
                    },
                ],
            }));
        }
    }

    nested_defs
}

pub(super) fn generate_lowered_proc_blocks(
    proc_order: &[String],
    proc_defs_by_name: &HashMap<String, ProcessorDef>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    struct_defs_by_name: &HashMap<String, StructDef>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<Block>, Vec<Block>) {
    let mut generated_structs = Vec::<Block>::new();
    let mut generated_defs = Vec::<Block>::new();
    for proc_name in proc_order {
        let Some(proc) = proc_defs_by_name.get(proc_name) else {
            continue;
        };
        let Some(shape) = lowering_shapes.get(proc_name).cloned() else {
            continue;
        };

        let mut nested_vars = shape.state.nested_procs.keys().cloned().collect::<Vec<_>>();
        nested_vars.sort();
        let mut nested_instances = HashMap::<String, ProcCallInstance>::new();
        for nested_var in &nested_vars {
            let Some(nested_state) = shape.state.nested_procs.get(nested_var) else {
                continue;
            };
            if !lowering_shapes.contains_key(&nested_state.proc_name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' nested state '{}' references unknown processor '{}'",
                        proc.name, nested_var, nested_state.proc_name
                    ),
                    0,
                    0,
                ));
                continue;
            }
            nested_instances.insert(
                nested_var.clone(),
                ProcCallInstance {
                    proc_name: nested_state.proc_name.clone(),
                    buffer_args: Vec::new(),
                },
            );
        }

        generated_structs.push(Block::Struct(omni_frontend::StructDef {
            name: proc.name.clone(),
            type_params: Vec::new(),
            fields: shape.fields.clone(),
            methods: Vec::new(),
        }));

        let mut read_lens = shape
            .in_array_slots
            .values()
            .chain(shape.field_array_slots.values())
            .map(|slots| slots.len())
            .filter(|len| *len > 1)
            .collect::<Vec<_>>();
        read_lens.sort_unstable();
        read_lens.dedup();
        for len in read_lens {
            generated_defs.push(Block::Def(build_proc_read_helper(&proc.name, len, false)));
            generated_defs.push(Block::Def(build_proc_read_helper(&proc.name, len, true)));
        }

        let mut generated_write_helpers = HashSet::<String>::new();
        let mut write_slots = shape
            .field_array_slots
            .values()
            .cloned()
            .collect::<Vec<Vec<String>>>();
        write_slots.sort();
        write_slots.dedup();
        for slots in write_slots {
            let clamp_name = proc_write_helper_name(&proc.name, &slots, false);
            if generated_write_helpers.insert(clamp_name) {
                generated_defs.push(Block::Def(build_proc_write_helper(
                    &proc.name, &slots, false,
                )));
            }
            let unsafe_name = proc_write_helper_name(&proc.name, &slots, true);
            if generated_write_helpers.insert(unsafe_name) {
                generated_defs.push(Block::Def(build_proc_write_helper(
                    &proc.name, &slots, true,
                )));
            }
        }

        let mut ins_names = shape.ins.iter().cloned().collect::<HashSet<_>>();
        for port in &shape.in_ports {
            ins_names.insert(port.name.clone());
        }
        for buffer in &shape.buffer_specs {
            ins_names.insert(buffer.name.clone());
        }

        let mut init_body = Vec::<Stmt>::new();
        for stmt in &proc.init {
            if let Stmt::Assign {
                target: AssignTarget::Var(var),
                expr:
                    Expr::UserCall {
                        name: ctor_name,
                        type_args,
                        args,
                        ..
                    },
                ..
            } = stmt
            {
                if let Some(nested_state) = shape.state.nested_procs.get(var) {
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
                    if nested_state.proc_name != *ctor_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    }
                    let Some(callee_shape) = lowering_shapes.get(&nested_state.proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                proc.name, var, nested_state.proc_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    };
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        var,
                        ctor_name,
                        args,
                        &callee_shape.param_specs,
                        &callee_shape.buffer_specs,
                        errors,
                    );
                    if let Some(instance) = nested_instances.get_mut(var) {
                        instance.buffer_args = bound_buffers;
                    }
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.data_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        ) {
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
                if let Some(state_struct) = shape.state.struct_instances.get(var) {
                    if state_struct.struct_name != *ctor_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    }
                    let Some(struct_def) = resolve_proc_state_struct_def(
                        &proc.name,
                        var,
                        state_struct,
                        &struct_defs_by_name,
                        errors,
                    ) else {
                        continue;
                    };
                    if !type_args.is_empty() {
                        let Some(resolved_type_args) = resolve_explicit_call_type_args(
                            type_args,
                            &format!("processor state constructor '{} = {}(...)'", var, ctor_name),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    let expanded =
                        expand_nested_struct_ctor_assign(var, ctor_name, args, &struct_def, errors);
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.data_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        ) {
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
            }
            if let Some(rewritten) = rewrite_owner_proc_stmt(
                stmt.clone(),
                &proc.name,
                &shape.field_names,
                &shape.data_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_fields,
                &nested_instances,
                &proc_api,
                errors,
            ) {
                init_body.push(rewritten);
            }
        }

        generated_defs.push(Block::Def(FunctionDef {
            type_params: Vec::new(),
            name: format!("{}{}", proc.name, PROC_INIT_FN_SUFFIX),
            params: vec![omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            }],
            body: init_body,
        }));

        generated_defs.extend(generate_nested_wrapper_defs(
            proc,
            &shape,
            proc_defs_by_name,
            lowering_shapes,
            struct_defs_by_name,
            proc_api,
            &nested_instances,
            &ins_names,
            errors,
        ));
        let proc_has_effective_block = proc_api
            .get(&proc.name)
            .map(|api| api.has_block)
            .unwrap_or(proc.has_block_block);
        if proc_has_effective_block {
            let mut block_params = Vec::<omni_frontend::FnParamDecl>::new();
            block_params.push(omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            });
            for buffer in &shape.buffer_specs {
                block_params.push(omni_frontend::FnParamDecl {
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    default: None,
                });
            }
            let mut block_pre_body = Vec::<Stmt>::new();
            for stmt in &proc.block_pre {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.data_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    block_pre_body.push(rewritten);
                }
            }
            let called_nested =
                collect_called_proc_instances_in_stmts(&proc.sample, &nested_instances);
            let mut nested_vars = nested_instances.keys().cloned().collect::<Vec<_>>();
            nested_vars.sort();
            for nested_var in &nested_vars {
                if !called_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !api.has_block {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_pre_body.push(Stmt::Expr {
                    loc: None,
                    expr: Expr::UserCall {
                        name: nested_block_pre_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: format!("{}{}", proc.name, PROC_BLOCK_PRE_FN_SUFFIX),
                params: block_params.clone(),
                body: block_pre_body,
            }));

            let mut block_post_body = Vec::<Stmt>::new();
            for nested_var in &nested_vars {
                if !called_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !api.has_block {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_post_body.push(Stmt::Expr {
                    loc: None,
                    expr: Expr::UserCall {
                        name: nested_block_post_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            for stmt in &proc.block_post {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.data_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    block_post_body.push(rewritten);
                }
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: format!("{}{}", proc.name, PROC_BLOCK_POST_FN_SUFFIX),
                params: block_params,
                body: block_post_body,
            }));
        }

        let step_body = proc
            .sample
            .iter()
            .filter_map(|stmt| {
                rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.data_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                )
            })
            .collect::<Vec<_>>();
        let mut step_params = Vec::<omni_frontend::FnParamDecl>::new();
        step_params.push(omni_frontend::FnParamDecl {
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            default: None,
        });
        for in_name in &shape.ins {
            let in_ty = *shape.in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
            step_params.push(omni_frontend::FnParamDecl {
                name: in_name.clone(),
                ty: Some(FnParamType::Primitive(in_ty)),
                default: None,
            });
        }
        for buffer in &shape.buffer_specs {
            step_params.push(omni_frontend::FnParamDecl {
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                default: None,
            });
        }
        generated_defs.push(Block::Def(FunctionDef {
            type_params: Vec::new(),
            name: format!("{}{}", proc.name, PROC_STEP_FN_SUFFIX),
            params: step_params.clone(),
            body: step_body,
        }));

        for (idx, out_name) in shape.outs.iter().enumerate() {
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::Var("self".to_owned()),
            }];
            for in_name in &shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::Var(in_name.clone()),
                });
            }
            for buffer in &shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::Var(buffer.name.clone()),
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: format!("{}{}{}", proc.name, PROC_CALL_OUT_FN_PREFIX, idx),
                params: step_params.clone(),
                body: vec![
                    Stmt::Expr {
                        loc: None,
                        expr: Expr::UserCall {
                            name: format!("{}{}", proc.name, PROC_STEP_FN_SUFFIX),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    },
                    Stmt::Return {
                        loc: None,
                        expr: Expr::Var(format!("self.{out_name}")),
                    },
                ],
            }));
        }
    }

    (generated_structs, generated_defs)
}
