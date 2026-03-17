use super::*;

#[derive(Debug, Clone)]
struct ManagedDynamicProcArray {
    proc_name: String,
    raw_slots: Vec<String>,
    slots: Vec<String>,
    active_field: String,
}

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
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

fn runtime_proc_array_active_field_name(array_base: &str) -> String {
    format!(
        "__omni_proc_block_active_{}",
        sanitize_runtime_symbol_component(array_base)
    )
}

fn rewrite_stmt_for_managed_dynamic_proc_block_hooks(
    stmt: Stmt,
    managed_arrays: &HashMap<String, ManagedDynamicProcArray>,
    proc_api: &HashMap<String, ProcApi>,
    used_arrays: &mut HashSet<String>,
) -> Vec<Stmt> {
    fn collect_guards_from_expr(
        expr: &Expr,
        managed_arrays: &HashMap<String, ManagedDynamicProcArray>,
        proc_api: &HashMap<String, ProcApi>,
        used_arrays: &mut HashSet<String>,
        guards: &mut Vec<Stmt>,
    ) {
        match expr {
            Expr::Index { index, .. } => {
                collect_guards_from_expr(index, managed_arrays, proc_api, used_arrays, guards);
            }
            Expr::Slice { start, end, .. } => {
                if let Some(start) = start {
                    collect_guards_from_expr(start, managed_arrays, proc_api, used_arrays, guards);
                }
                if let Some(end) = end {
                    collect_guards_from_expr(end, managed_arrays, proc_api, used_arrays, guards);
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_guards_from_expr(&spec.size, managed_arrays, proc_api, used_arrays, guards);
                if let Some(values) = init {
                    for value in values {
                        collect_guards_from_expr(
                            value,
                            managed_arrays,
                            proc_api,
                            used_arrays,
                            guards,
                        );
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                collect_guards_from_expr(lhs, managed_arrays, proc_api, used_arrays, guards);
                collect_guards_from_expr(rhs, managed_arrays, proc_api, used_arrays, guards);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    collect_guards_from_expr(arg, managed_arrays, proc_api, used_arrays, guards);
                }
            }
            Expr::UserCall {
                name,
                args,
                type_args: _,
                ..
            } => {
                for arg in args {
                    collect_guards_from_expr(
                        &arg.expr,
                        managed_arrays,
                        proc_api,
                        used_arrays,
                        guards,
                    );
                }
                let proc_name = if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
                    Some(step_proc)
                } else if let Some((call_proc, out_idx_raw)) =
                    name.rsplit_once(PROC_CALL_OUT_FN_PREFIX)
                {
                    if out_idx_raw.parse::<usize>().is_ok() {
                        Some(call_proc)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let Some(proc_name) = proc_name else {
                    return;
                };
                let Some(api) = proc_api.get(proc_name) else {
                    return;
                };
                if !api.has_block {
                    return;
                }
                let Some(CallArg {
                    expr: Expr::Index { base, index, .. },
                    ..
                }) = args.first()
                else {
                    return;
                };
                if matches!(index.as_ref(), Expr::Int { .. }) {
                    return;
                }
                let array_base = base.as_str();
                let Some(managed) = managed_arrays.get(array_base) else {
                    return;
                };
                if managed.proc_name != proc_name {
                    return;
                }
                used_arrays.insert(array_base.to_owned());
                let input_slots = api.ins.iter().map(|port| port.slots.len()).sum::<usize>();
                let buffer_start = 1 + input_slots;
                let mut pre_args = Vec::<CallArg>::new();
                pre_args.push(CallArg {
                    name: None,
                    expr: Expr::Index {
                        loc: Default::default(),
                        base: array_base.to_owned(),
                        index: Box::new(index.as_ref().clone()),
                    },
                });
                pre_args.extend(args.iter().skip(buffer_start).cloned());
                guards.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(Expr::Index {
                            loc: Default::default(),
                            base: format!("self.{}", managed.active_field),
                            index: Box::new(index.as_ref().clone()),
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
                                base: format!("self.{}", managed.active_field),
                                index: index.as_ref().clone(),
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
                collect_guards_from_expr(inner, managed_arrays, proc_api, used_arrays, guards);
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    collect_guards_from_expr(value, managed_arrays, proc_api, used_arrays, guards);
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    let mut guards = Vec::<Stmt>::new();
    match &stmt {
        Stmt::Expr { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
            collect_guards_from_expr(expr, managed_arrays, proc_api, used_arrays, &mut guards);
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
                managed_arrays,
                proc_api,
                used_arrays,
                &mut cond_guards,
            );
            let mut new_then = Vec::<Stmt>::new();
            for nested in then_branch {
                new_then.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                ));
            }
            let mut new_else = Vec::<Stmt>::new();
            for nested in else_branch {
                new_else.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                ));
            }
            cond_guards.push(Stmt::If {
                loc,
                cond,
                then_branch: new_then,
                else_branch: new_else,
            });
            cond_guards
        }
        Stmt::For {
            loc,
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
        } => {
            let mut range_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &start,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut range_guards,
            );
            collect_guards_from_expr(
                &end,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut range_guards,
            );
            if let Some(step_expr) = &step {
                collect_guards_from_expr(
                    step_expr,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    &mut range_guards,
                );
            }
            let mut new_body = Vec::<Stmt>::new();
            for nested in body {
                new_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                ));
            }
            range_guards.push(Stmt::For {
                loc,
                var,
                step,
                start,
                end,
                end_inclusive,
                body: new_body,
            });
            range_guards
        }
        Stmt::While { loc, cond, body } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &cond,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut cond_guards,
            );
            let mut new_body = Vec::<Stmt>::new();
            for nested in body {
                new_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                ));
            }
            cond_guards.push(Stmt::While {
                loc,
                cond,
                body: new_body,
            });
            cond_guards
        }
        _ => vec![stmt],
    }
}

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
    managed_active_fields: &mut HashMap<String, usize>,
    def_sample_oversample_factors: &mut HashMap<String, usize>,
    proc_step_oversample_meta: &mut HashMap<String, ProcStepOversampleMeta>,
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
        let callee_api = proc_api.get(&callee_proc_name);
        let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
        let proc_ns = namespace_of_symbol(&callee_proc_name);
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
        for local_def in unique_proc_local_defs(callee_proc) {
            let body = local_def
                .body
                .iter()
                .filter_map(|stmt| {
                    lower_callee_stmt_for_nested_wrapper(
                        stmt,
                        &proc.name,
                        &callee_proc_name,
                        &nested_path,
                        &callee_shape,
                        &callee_nested_instances,
                        &callee_ins_names,
                        &callee_shape.field_array_slots,
                        &callee_shape.in_array_slots,
                        &callee_shape.nested_proc_array_slots,
                        &proc_api,
                        errors,
                    )
                })
                .collect::<Vec<_>>();
            nested_defs.push(Block::Def(nested_wrapper_proc_local_hidden_def(
                &proc.name,
                &nested_path,
                &local_def,
                body,
            )));
        }
        for stmt in &callee_proc.init {
            if let Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: expr @ Expr::ArrayCtor { init, .. },
                ..
            } = stmt
            {
                if let Some(slot_names) = callee_shape.nested_proc_array_slots.get(array_var) {
                    let Some(array_state) = callee_shape.state.nested_proc_arrays.get(array_var)
                    else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' processor-array '{}' is missing state metadata",
                                callee_proc_name, array_var
                            ),
                        );
                        continue;
                    };

                    if let Some(values) = init {
                        if values.len() != slot_names.len() && values.len() != 1 {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{}.{}' initializer expects {} constructor entries (or a single broadcast constructor), got {}",
                                        callee_proc_name,
                                        array_var,
                                        slot_names.len(),
                                        values.len()
                                    ),
                                );
                            });
                        }
                    }

                    for (slot_idx, slot_name) in slot_names.iter().enumerate() {
                        let mut slot_ctor_args = Vec::<CallArg>::new();
                        let mut proc_array_slot = None;
                        if let Some(values) = init {
                            let value = if values.len() == 1 {
                                proc_array_slot = Some((slot_idx, slot_names.len()));
                                values.first()
                            } else {
                                values.get(slot_idx)
                            };
                            if let Some(value) = value {
                                if let Expr::UserCall {
                                    name: ctor_name,
                                    type_args,
                                    args,
                                    ..
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
                                            &proc_ns,
                                            &proc_symbols,
                                        )
                                    };
                                    if let Some(resolved_ctor) = resolved_ctor {
                                        if resolved_ctor != array_state.proc_name {
                                            with_expr_diag_context(value, |expr_diag| {
                                                push_semantic(
                                                    expr_diag,
                                                    errors,
                                                    format!(
                                                        "processor '{}.{}' initializer entry {} uses constructor '{}' but '{}' is required",
                                                        callee_proc_name,
                                                        array_var,
                                                        slot_idx,
                                                        resolved_ctor,
                                                        array_state.proc_name
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
                                                    "processor '{}.{}' initializer entry {} references unknown processor constructor '{}'",
                                                    callee_proc_name, array_var, slot_idx, ctor_name
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
                                    slot_ctor_args = args.clone();
                                } else {
                                    with_expr_diag_context(value, |expr_diag| {
                                        push_semantic(
                                            expr_diag,
                                            errors,
                                            format!(
                                                "processor '{}.{}' initializer entry {} must be a processor constructor call",
                                                callee_proc_name, array_var, slot_idx
                                            ),
                                        );
                                    });
                                }
                            }
                        }

                        let Some(slot_state) = callee_shape.state.nested_procs.get(slot_name)
                        else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' processor-array slot '{}' is missing nested processor state",
                                    callee_proc_name, slot_name
                                ),
                            );
                            continue;
                        };
                        let Some(nested_callee_shape) = lowering_shapes.get(&slot_state.proc_name)
                        else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' nested state '{}' references unknown processor '{}'",
                                    callee_proc_name, slot_name, slot_state.proc_name
                                ),
                            );
                            continue;
                        };
                        let lowered_slot_ctor_args = slot_ctor_args
                            .iter()
                            .map(|arg| CallArg {
                                name: arg.name.clone(),
                                expr: lower_callee_expr_for_nested_wrapper(
                                    &arg.expr,
                                    &proc.name,
                                    &nested_path,
                                    &callee_shape,
                                    &callee_nested_instances,
                                    &callee_shape.field_array_slots,
                                    &callee_shape.in_array_slots,
                                    &callee_shape.nested_proc_array_slots,
                                    &proc_api,
                                    errors,
                                ),
                            })
                            .collect::<Vec<_>>();
                        let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                            &proc.name,
                            &nested_field_name(&nested_path, slot_name),
                            &slot_state.proc_name,
                            &lowered_slot_ctor_args,
                            &nested_callee_shape.param_specs,
                            &nested_callee_shape.buffer_specs,
                            proc_array_slot,
                            errors,
                        );
                        if let Some(instance) = callee_nested_instances.get_mut(slot_name) {
                            instance.buffer_args = bound_buffers;
                        }
                        for expanded_stmt in expanded {
                            if let Some(rewritten) = rewrite_owner_proc_stmt(
                                expanded_stmt,
                                &proc.name,
                                &shape.field_names,
                                &shape.array_field_names,
                                &ins_names,
                                &shape.field_array_slots,
                                &shape.in_array_slots,
                                &shape.nested_proc_array_slots,
                                &shape.nested_fields,
                                &nested_instances,
                                &proc_api,
                                errors,
                            ) {
                                nested_init_body.push(rewritten);
                            }
                        }
                    }
                    continue;
                }
            }
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
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),
                        );
                    }
                    if nested_state.proc_name != *ctor_name {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                        );
                        continue;
                    }
                    let Some(nested_callee_shape) = lowering_shapes.get(&nested_state.proc_name)
                    else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                callee_proc_name, var, nested_state.proc_name
                            ),
                        );
                        continue;
                    };
                    let lowered_ctor_args = args
                        .iter()
                        .map(|arg| CallArg {
                            name: arg.name.clone(),
                            expr: lower_callee_expr_for_nested_wrapper(
                                &arg.expr,
                                &proc.name,
                                &nested_path,
                                &callee_shape,
                                &callee_nested_instances,
                                &callee_shape.field_array_slots,
                                &callee_shape.in_array_slots,
                                &callee_shape.nested_proc_array_slots,
                                &proc_api,
                                errors,
                            ),
                        })
                        .collect::<Vec<_>>();
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        &nested_field_name(&nested_path, var),
                        ctor_name,
                        &lowered_ctor_args,
                        &nested_callee_shape.param_specs,
                        &nested_callee_shape.buffer_specs,
                        None,
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
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
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
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                        );
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
                            DiagCtx::new(stmt.loc()),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                            );
                        }
                    }
                    let expanded = expand_nested_struct_ctor_assign(
                        &nested_field_name(&nested_path, var),
                        ctor_name,
                        &args
                            .iter()
                            .map(|arg| CallArg {
                                name: arg.name.clone(),
                                expr: lower_callee_expr_for_nested_wrapper(
                                    &arg.expr,
                                    &proc.name,
                                    &nested_path,
                                    &callee_shape,
                                    &callee_nested_instances,
                                    &callee_shape.field_array_slots,
                                    &callee_shape.in_array_slots,
                                    &callee_shape.nested_proc_array_slots,
                                    &proc_api,
                                    errors,
                                ),
                            })
                            .collect::<Vec<_>>(),
                        &struct_def,
                        &struct_defs_by_name,
                        errors,
                    );
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
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
                &callee_proc_name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &callee_shape.nested_proc_array_slots,
                &proc_api,
                errors,
            ) {
                nested_init_body.push(rewritten);
            }
        }

        nested_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            type_params: Vec::new(),
            name: nested_init_fn_name(&proc.name, &nested_path),
            params: vec![omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                ty_loc: Default::default(),
                default: None,
            }],
            body: nested_init_body,
        }));

        let nested_managed_dynamic_arrays = callee_shape
            .nested_proc_array_slots
            .iter()
            .filter_map(|(array_base, slots)| {
                let first_slot = slots.first()?;
                let instance = callee_nested_instances.get(first_slot)?;
                let api = proc_api.get(&instance.proc_name)?;
                if !api.has_block {
                    return None;
                }
                let prefixed_base = nested_field_name(&nested_path, array_base);
                let prefixed_slots = slots
                    .iter()
                    .map(|slot| nested_field_name(&nested_path, slot))
                    .collect::<Vec<_>>();
                Some((
                    prefixed_base,
                    ManagedDynamicProcArray {
                        proc_name: instance.proc_name.clone(),
                        raw_slots: slots.clone(),
                        slots: prefixed_slots,
                        active_field: nested_field_name(
                            &nested_path,
                            &runtime_proc_array_active_field_name(array_base),
                        ),
                    },
                ))
            })
            .collect::<HashMap<_, _>>();
        let mut used_nested_managed_dynamic_arrays = HashSet::<String>::new();
        let mut nested_step_body = Vec::<Stmt>::new();
        for stmt in &callee_proc.sample {
            let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                stmt,
                &proc.name,
                &callee_proc_name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &callee_shape.nested_proc_array_slots,
                &proc_api,
                errors,
            ) else {
                continue;
            };
            nested_step_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                rewritten,
                &nested_managed_dynamic_arrays,
                &proc_api,
                &mut used_nested_managed_dynamic_arrays,
            ));
        }
        let mut nested_step_params = Vec::<omni_frontend::FnParamDecl>::new();
        nested_step_params.push(omni_frontend::FnParamDecl {
            loc: Default::default(),
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            ty_loc: Default::default(),
            default: None,
        });
        for in_name in &callee_shape.ins {
            nested_step_params.push(omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: in_name.clone(),
                ty: None,
                ty_loc: Default::default(),
                default: None,
            });
        }
        for buffer in &callee_shape.buffer_specs {
            nested_step_params.push(omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                ty_loc: Default::default(),
                default: None,
            });
        }
        let nested_step_name = nested_step_fn_name(&proc.name, &nested_path);
        let callee_sample_oversample_factor = proc_api
            .get(&callee_proc_name)
            .map(|api| api.sample_oversample_factor)
            .unwrap_or(1);
        if callee_sample_oversample_factor > 1 {
            let stage_count = proc_os_sinc_stage_count(callee_sample_oversample_factor);
            let mut input_state_fields = HashMap::<String, ProcInputOversampleStateFields>::new();
            for in_name in &callee_shape.ins {
                let in_ty = *callee_shape
                    .in_types
                    .get(in_name)
                    .unwrap_or(&PrimitiveType::F32);
                let up_stages = if matches!(in_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a0"),
                            ),
                            a1: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a1"),
                            ),
                            a2: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a2"),
                            ),
                            a3: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a3"),
                            ),
                            b0: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b0"),
                            ),
                            b1: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b1"),
                            ),
                            b2: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b2"),
                            ),
                            b3: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b3"),
                            ),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                input_state_fields.insert(
                    in_name.clone(),
                    ProcInputOversampleStateFields { up_stages },
                );
            }
            let mut output_state_fields = HashMap::<String, ProcOutputOversampleStateFields>::new();
            for out_name in &callee_shape.outs {
                let out_ty = *callee_shape
                    .out_types
                    .get(out_name)
                    .unwrap_or(&PrimitiveType::F32);
                let output_field = nested_field_name(&nested_path, out_name);
                let down_stages = if matches!(out_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a0"),
                            ),
                            a1: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a1"),
                            ),
                            a2: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a2"),
                            ),
                            a3: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a3"),
                            ),
                            b0: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b0"),
                            ),
                            b1: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b1"),
                            ),
                            b2: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b2"),
                            ),
                            b3: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b3"),
                            ),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                output_state_fields.insert(
                    output_field,
                    ProcOutputOversampleStateFields { down_stages },
                );
            }
            proc_step_oversample_meta.insert(
                nested_step_name.clone(),
                ProcStepOversampleMeta {
                    input_state_fields,
                    output_state_fields,
                },
            );
        }
        def_sample_oversample_factors.insert(
            nested_step_name.clone(),
            callee_sample_oversample_factor.max(1),
        );
        nested_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            type_params: Vec::new(),
            name: nested_step_name.clone(),
            params: nested_step_params.clone(),
            body: nested_step_body,
        }));

        if let Some(callee_api) = callee_api {
            for event in &callee_proc.events {
                let Some(event_spec) = callee_api.events.get(&event.name) else {
                    push_semantic(
                        DiagCtx::new(event.loc),
                        errors,
                        format!(
                            "processor '{}' nested event '{}' is missing lowered metadata",
                            callee_proc_name, event.name
                        ),
                    );
                    continue;
                };
                let mut nested_event_params = Vec::<omni_frontend::FnParamDecl>::new();
                nested_event_params.push(omni_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: "self".to_owned(),
                    ty: Some(FnParamType::Struct(proc.name.clone())),
                    ty_loc: Default::default(),
                    default: None,
                });
                let mut callee_event_ins_names = callee_ins_names.clone();
                let mut callee_event_in_array_slots = HashMap::<String, Vec<String>>::new();
                for param in &event_spec.params {
                    callee_event_ins_names.insert(param.name.clone());
                    if let Some(elem_ty) = param.fixed_array_elem_ty.or(param.slice_elem_ty) {
                        nested_event_params.push(omni_frontend::FnParamDecl {
                            loc: Default::default(),
                            name: param.name.clone(),
                            ty: Some(FnParamType::Array(Some(elem_ty))),
                            ty_loc: Default::default(),
                            default: None,
                        });
                        continue;
                    }
                    let mut slot_names = Vec::<String>::new();
                    for slot in &param.slots {
                        slot_names.push(slot.name.clone());
                        callee_event_ins_names.insert(slot.name.clone());
                        nested_event_params.push(omni_frontend::FnParamDecl {
                            loc: Default::default(),
                            name: slot.name.clone(),
                            ty: Some(FnParamType::Primitive(slot.ty)),
                            ty_loc: Default::default(),
                            default: None,
                        });
                    }
                    if slot_names.len() > 1 {
                        callee_event_in_array_slots.insert(param.name.clone(), slot_names);
                    }
                }
                let nested_event_body = event
                    .body
                    .iter()
                    .filter_map(|stmt| {
                        lower_callee_stmt_for_nested_wrapper(
                            stmt,
                            &proc.name,
                            &callee_proc_name,
                            &nested_path,
                            &callee_shape,
                            &callee_nested_instances,
                            &callee_event_ins_names,
                            &callee_shape.field_array_slots,
                            &callee_event_in_array_slots,
                            &callee_shape.nested_proc_array_slots,
                            &proc_api,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                nested_defs.push(Block::Def(FunctionDef {
                    loc: Default::default(),
                    type_params: Vec::new(),
                    name: nested_event_fn_name(&proc.name, &nested_path, &event.name),
                    params: nested_event_params,
                    body: nested_event_body,
                }));
            }
        }

        let callee_has_effective_block = proc_api
            .get(&callee_proc_name)
            .map(|api| api.has_block)
            .unwrap_or(callee_proc.has_block_block);
        if callee_has_effective_block {
            let mut nested_block_params = Vec::<omni_frontend::FnParamDecl>::new();
            nested_block_params.push(omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                ty_loc: Default::default(),
                default: None,
            });
            for buffer in &callee_shape.buffer_specs {
                nested_block_params.push(omni_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    ty_loc: Default::default(),
                    default: None,
                });
            }
            let mut nested_block_pre_body = Vec::<Stmt>::new();
            for stmt in &callee_proc.block_pre {
                if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                    stmt,
                    &proc.name,
                    &callee_proc_name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &callee_shape.nested_proc_array_slots,
                    &proc_api,
                    errors,
                ) {
                    nested_block_pre_body.push(rewritten);
                }
            }
            let mut called_callee_nested = collect_called_proc_instances_in_stmts(
                &callee_proc.sample,
                &callee_nested_instances,
                &callee_shape.nested_proc_array_slots,
            );
            for array_base in &used_nested_managed_dynamic_arrays {
                if let Some(managed) = nested_managed_dynamic_arrays.get(array_base) {
                    for slot in &managed.raw_slots {
                        called_callee_nested.remove(slot);
                    }
                }
            }
            let mut callee_nested_vars =
                callee_nested_instances.keys().cloned().collect::<Vec<_>>();
            callee_nested_vars.sort();
            for array_base in &used_nested_managed_dynamic_arrays {
                let Some(managed) = nested_managed_dynamic_arrays.get(array_base) else {
                    continue;
                };
                managed_active_fields
                    .entry(managed.active_field.clone())
                    .or_insert(managed.slots.len());
                for slot_idx in 0..managed.slots.len() {
                    nested_block_pre_body.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: format!("self.{}", managed.active_field),
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
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                nested_block_pre_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
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
                loc: Default::default(),
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
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                nested_block_post_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
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
                    &callee_proc_name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &callee_shape.nested_proc_array_slots,
                    &proc_api,
                    errors,
                ) {
                    nested_block_post_body.push(rewritten);
                }
            }
            for array_base in &used_nested_managed_dynamic_arrays {
                let Some(managed) = nested_managed_dynamic_arrays.get(array_base) else {
                    continue;
                };
                for (slot_idx, slot_name) in managed.slots.iter().enumerate() {
                    let raw_slot_name = managed.raw_slots.get(slot_idx);
                    let Some(raw_slot_name) = raw_slot_name else {
                        continue;
                    };
                    let Some(instance) = callee_nested_instances.get(raw_slot_name) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&instance.proc_name) else {
                        continue;
                    };
                    let mut call_args = vec![CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: array_base.clone(),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                    }];
                    call_args.extend(expand_proc_buffer_call_args(
                        instance,
                        api,
                        raw_slot_name,
                        errors,
                    ));
                    nested_block_post_body.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::Index {
                            loc: Default::default(),
                            base: format!("self.{}", managed.active_field),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                        then_branch: vec![Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: nested_block_post_fn_name(&proc.name, slot_name),
                                type_args: Vec::new(),
                                args: call_args,
                            },
                        }],
                        else_branch: Vec::new(),
                    });
                }
            }
            nested_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                type_params: Vec::new(),
                name: nested_block_post_fn_name(&proc.name, &nested_path),
                params: nested_block_params,
                body: nested_block_post_body,
            }));
        }

        for (idx, out_name) in callee_shape.outs.iter().enumerate() {
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::var("self"),
            }];
            for in_name in &callee_shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(in_name.clone()),
                });
            }
            for buffer in &callee_shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(buffer.name.clone()),
                });
            }
            nested_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                type_params: Vec::new(),
                name: nested_call_out_fn_name(&proc.name, &nested_path, idx),
                params: nested_step_params.clone(),
                body: vec![
                    Stmt::Expr {
                        loc: Default::default(),
                        expr: Expr::UserCall {
                            loc: Default::default(),
                            name: nested_step_name.clone(),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    },
                    Stmt::Return {
                        loc: Default::default(),
                        expr: Expr::var(format!(
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
) -> (
    Vec<Block>,
    Vec<Block>,
    HashMap<String, usize>,
    HashMap<String, ProcStepOversampleMeta>,
) {
    let mut generated_structs = Vec::<Block>::new();
    let mut generated_defs = Vec::<Block>::new();
    let mut def_sample_oversample_factors = HashMap::<String, usize>::new();
    let mut proc_step_oversample_meta = HashMap::<String, ProcStepOversampleMeta>::new();
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
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "processor '{}' nested state '{}' references unknown processor '{}'",
                        proc.name, nested_var, nested_state.proc_name
                    ),
                );
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

        let struct_idx = generated_structs.len();
        generated_structs.push(Block::Struct(omni_frontend::StructDef {
            loc: Default::default(),
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
        if let Some(api) = proc_api.get(&proc.name) {
            for event in api.events.values() {
                for param in &event.params {
                    let len = param.slots.len();
                    if len > 1 {
                        read_lens.push(len);
                    }
                }
            }
        }
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

        for local_def in unique_proc_local_defs(proc) {
            let mut body = Vec::<Stmt>::new();
            for stmt in &local_def.body {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.array_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_proc_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    body.push(rewritten);
                }
            }
            generated_defs.push(Block::Def(owner_proc_local_hidden_def(
                &proc.name, &local_def, body,
            )));
        }

        let mut init_body = Vec::<Stmt>::new();
        let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
        let proc_ns = namespace_of_symbol(&proc.name);
        for stmt in &proc.init {
            if let Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: expr @ Expr::ArrayCtor { init, .. },
                ..
            } = stmt
            {
                if let Some(slot_names) = shape.nested_proc_array_slots.get(array_var) {
                    let Some(array_state) = shape.state.nested_proc_arrays.get(array_var) else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' processor-array '{}' is missing state metadata",
                                proc.name, array_var
                            ),
                        );
                        continue;
                    };

                    if let Some(values) = init {
                        if values.len() != slot_names.len() && values.len() != 1 {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{}.{}' initializer expects {} constructor entries (or a single broadcast constructor), got {}",
                                        proc.name,
                                        array_var,
                                        slot_names.len(),
                                        values.len()
                                    ),
                                );
                            });
                        }
                    }

                    for (slot_idx, slot_name) in slot_names.iter().enumerate() {
                        let mut slot_ctor_args = Vec::<CallArg>::new();
                        let mut proc_array_slot = None;
                        if let Some(values) = init {
                            let value = if values.len() == 1 {
                                proc_array_slot = Some((slot_idx, slot_names.len()));
                                values.first()
                            } else {
                                values.get(slot_idx)
                            };
                            if let Some(value) = value {
                                if let Expr::UserCall {
                                    name: ctor_name,
                                    type_args,
                                    args,
                                    ..
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
                                            &proc_ns,
                                            &proc_symbols,
                                        )
                                    };
                                    if let Some(resolved_ctor) = resolved_ctor {
                                        if resolved_ctor != array_state.proc_name {
                                            with_expr_diag_context(value, |expr_diag| {
                                                push_semantic(
                                                    expr_diag,
                                                    errors,
                                                    format!(
                                                        "processor '{}.{}' initializer entry {} uses constructor '{}' but '{}' is required",
                                                        proc.name,
                                                        array_var,
                                                        slot_idx,
                                                        resolved_ctor,
                                                        array_state.proc_name
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
                                                    "processor '{}.{}' initializer entry {} references unknown processor constructor '{}'",
                                                    proc.name, array_var, slot_idx, ctor_name
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
                                    slot_ctor_args = args.clone();
                                } else {
                                    with_expr_diag_context(value, |expr_diag| {
                                        push_semantic(
                                            expr_diag,
                                            errors,
                                            format!(
                                                "processor '{}.{}' initializer entry {} must be a processor constructor call",
                                                proc.name, array_var, slot_idx
                                            ),
                                        );
                                    });
                                }
                            }
                        }

                        let Some(slot_state) = shape.state.nested_procs.get(slot_name) else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' processor-array slot '{}' is missing nested processor state",
                                    proc.name, slot_name
                                ),
                            );
                            continue;
                        };
                        let Some(callee_shape) = lowering_shapes.get(&slot_state.proc_name) else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' nested state '{}' references unknown processor '{}'",
                                    proc.name, slot_name, slot_state.proc_name
                                ),
                            );
                            continue;
                        };
                        let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                            &proc.name,
                            slot_name,
                            &slot_state.proc_name,
                            &slot_ctor_args,
                            &callee_shape.param_specs,
                            &callee_shape.buffer_specs,
                            proc_array_slot,
                            errors,
                        );
                        if let Some(instance) = nested_instances.get_mut(slot_name) {
                            instance.buffer_args = bound_buffers;
                        }
                        for expanded_stmt in expanded {
                            if let Some(rewritten) = rewrite_owner_proc_stmt(
                                expanded_stmt,
                                &proc.name,
                                &shape.field_names,
                                &shape.array_field_names,
                                &ins_names,
                                &shape.field_array_slots,
                                &shape.in_array_slots,
                                &shape.nested_proc_array_slots,
                                &shape.nested_fields,
                                &nested_instances,
                                &proc_api,
                                errors,
                            ) {
                                init_body.push(rewritten);
                            }
                        }
                    }
                    continue;
                }
                if let Some(values) = init {
                    let mut decl_stmt = stmt.clone();
                    if let Stmt::Assign {
                        expr: Expr::ArrayCtor { init, .. },
                        ..
                    } = &mut decl_stmt
                    {
                        *init = None;
                    }
                    if let Some(rewritten) = rewrite_owner_proc_stmt(
                        decl_stmt,
                        &proc.name,
                        &shape.field_names,
                        &shape.array_field_names,
                        &ins_names,
                        &shape.field_array_slots,
                        &shape.in_array_slots,
                        &shape.nested_proc_array_slots,
                        &shape.nested_fields,
                        &nested_instances,
                        &proc_api,
                        errors,
                    ) {
                        init_body.push(rewritten);
                    }
                    for (idx, value) in values.iter().cloned().enumerate() {
                        let write_stmt = Stmt::Assign {
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
                        };
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            write_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
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
            if let Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: Expr::ArrayLiteral { values, .. },
                ..
            } = stmt
            {
                for (idx, value) in values.iter().cloned().enumerate() {
                    let write_stmt = Stmt::Assign {
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
                    };
                    if let Some(rewritten) = rewrite_owner_proc_stmt(
                        write_stmt,
                        &proc.name,
                        &shape.field_names,
                        &shape.array_field_names,
                        &ins_names,
                        &shape.field_array_slots,
                        &shape.in_array_slots,
                        &shape.nested_proc_array_slots,
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
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),
                        );
                    }
                    if nested_state.proc_name != *ctor_name {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                        );
                        continue;
                    }
                    let Some(callee_shape) = lowering_shapes.get(&nested_state.proc_name) else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                proc.name, var, nested_state.proc_name
                            ),
                        );
                        continue;
                    };
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        var,
                        ctor_name,
                        args,
                        &callee_shape.param_specs,
                        &callee_shape.buffer_specs,
                        None,
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
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
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
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                        );
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
                            DiagCtx::new(stmt.loc()),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                            );
                        }
                    }
                    let expanded = expand_nested_struct_ctor_assign(
                        var,
                        ctor_name,
                        args,
                        &struct_def,
                        &struct_defs_by_name,
                        errors,
                    );
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
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
                &shape.array_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_proc_array_slots,
                &shape.nested_fields,
                &nested_instances,
                &proc_api,
                errors,
            ) {
                init_body.push(rewritten);
            }
        }

        generated_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            type_params: Vec::new(),
            name: format!("{}{}", proc.name, PROC_INIT_FN_SUFFIX),
            params: vec![omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                ty_loc: Default::default(),
                default: None,
            }],
            body: init_body,
        }));

        let mut nested_managed_active_fields = HashMap::<String, usize>::new();
        generated_defs.extend(generate_nested_wrapper_defs(
            proc,
            &shape,
            proc_defs_by_name,
            lowering_shapes,
            struct_defs_by_name,
            proc_api,
            &nested_instances,
            &ins_names,
            &mut nested_managed_active_fields,
            &mut def_sample_oversample_factors,
            &mut proc_step_oversample_meta,
            errors,
        ));
        if let Some(owner_api) = proc_api.get(&proc.name) {
            for event in &proc.events {
                let Some(event_spec) = owner_api.events.get(&event.name) else {
                    push_semantic(
                        DiagCtx::new(event.loc),
                        errors,
                        format!(
                            "processor '{}' event '{}' is missing lowered event metadata",
                            proc.name, event.name
                        ),
                    );
                    continue;
                };
                let mut event_params = Vec::<omni_frontend::FnParamDecl>::new();
                event_params.push(omni_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: "self".to_owned(),
                    ty: Some(FnParamType::Struct(proc.name.clone())),
                    ty_loc: Default::default(),
                    default: None,
                });
                let mut event_ins_names = ins_names.clone();
                let mut event_in_array_slots = HashMap::<String, Vec<String>>::new();
                for param in &event_spec.params {
                    event_ins_names.insert(param.name.clone());
                    if let Some(elem_ty) = param.fixed_array_elem_ty.or(param.slice_elem_ty) {
                        event_params.push(omni_frontend::FnParamDecl {
                            loc: Default::default(),
                            name: param.name.clone(),
                            ty: Some(FnParamType::Array(Some(elem_ty))),
                            ty_loc: Default::default(),
                            default: None,
                        });
                        continue;
                    }
                    let mut slot_names = Vec::<String>::new();
                    for slot in &param.slots {
                        slot_names.push(slot.name.clone());
                        event_ins_names.insert(slot.name.clone());
                        event_params.push(omni_frontend::FnParamDecl {
                            loc: Default::default(),
                            name: slot.name.clone(),
                            ty: Some(FnParamType::Primitive(slot.ty)),
                            ty_loc: Default::default(),
                            default: None,
                        });
                    }
                    if slot_names.len() > 1 {
                        event_in_array_slots.insert(param.name.clone(), slot_names);
                    }
                }
                let event_body = event
                    .body
                    .iter()
                    .filter_map(|stmt| {
                        rewrite_owner_proc_stmt(
                            stmt.clone(),
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &event_ins_names,
                            &shape.field_array_slots,
                            &event_in_array_slots,
                            &shape.nested_proc_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                generated_defs.push(Block::Def(FunctionDef {
                    loc: Default::default(),
                    type_params: Vec::new(),
                    name: format!("{}{}{}", proc.name, PROC_EVENT_FN_PREFIX, event.name),
                    params: event_params,
                    body: event_body,
                }));
            }
        }

        let managed_dynamic_arrays = shape
            .nested_proc_array_slots
            .iter()
            .filter_map(|(array_base, slots)| {
                let first_slot = slots.first()?;
                let instance = nested_instances.get(first_slot)?;
                let api = proc_api.get(&instance.proc_name)?;
                if !api.has_block {
                    return None;
                }
                Some((
                    array_base.clone(),
                    ManagedDynamicProcArray {
                        proc_name: instance.proc_name.clone(),
                        raw_slots: slots.clone(),
                        slots: slots.clone(),
                        active_field: runtime_proc_array_active_field_name(array_base),
                    },
                ))
            })
            .collect::<HashMap<_, _>>();

        let mut used_managed_dynamic_arrays = HashSet::<String>::new();
        let mut step_body = Vec::<Stmt>::new();
        for stmt in &proc.sample {
            let Some(rewritten) = rewrite_owner_proc_stmt(
                stmt.clone(),
                &proc.name,
                &shape.field_names,
                &shape.array_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_proc_array_slots,
                &shape.nested_fields,
                &nested_instances,
                &proc_api,
                errors,
            ) else {
                continue;
            };
            step_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                rewritten,
                &managed_dynamic_arrays,
                &proc_api,
                &mut used_managed_dynamic_arrays,
            ));
        }

        let proc_has_effective_block = proc_api
            .get(&proc.name)
            .map(|api| api.has_block)
            .unwrap_or(proc.has_block_block);
        if proc_has_effective_block {
            let mut block_params = Vec::<omni_frontend::FnParamDecl>::new();
            block_params.push(omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                ty_loc: Default::default(),
                default: None,
            });
            for buffer in &shape.buffer_specs {
                block_params.push(omni_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    ty_loc: Default::default(),
                    default: None,
                });
            }
            let mut block_pre_body = Vec::<Stmt>::new();
            for stmt in &proc.block_pre {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.array_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_proc_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    block_pre_body.push(rewritten);
                }
            }
            let mut called_nested = collect_called_proc_instances_in_stmts(
                &proc.sample,
                &nested_instances,
                &shape.nested_proc_array_slots,
            );
            for array_base in &used_managed_dynamic_arrays {
                if let Some(managed) = managed_dynamic_arrays.get(array_base) {
                    for slot in &managed.raw_slots {
                        called_nested.remove(slot);
                    }
                }
            }
            let mut nested_vars = nested_instances.keys().cloned().collect::<Vec<_>>();
            nested_vars.sort();
            for array_base in &used_managed_dynamic_arrays {
                let Some(managed) = managed_dynamic_arrays.get(array_base) else {
                    continue;
                };
                for slot_idx in 0..managed.slots.len() {
                    block_pre_body.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: format!("self.{}", managed.active_field),
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
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_pre_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
                        name: nested_block_pre_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
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
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_post_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
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
                    &shape.array_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_proc_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    block_post_body.push(rewritten);
                }
            }
            for array_base in &used_managed_dynamic_arrays {
                let Some(managed) = managed_dynamic_arrays.get(array_base) else {
                    continue;
                };
                for (slot_idx, slot_name) in managed.slots.iter().enumerate() {
                    let Some(instance) = nested_instances.get(slot_name) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&instance.proc_name) else {
                        continue;
                    };
                    let mut call_args = vec![CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: array_base.clone(),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                    }];
                    call_args.extend(expand_proc_buffer_call_args(
                        instance, api, slot_name, errors,
                    ));
                    block_post_body.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::Index {
                            loc: Default::default(),
                            base: format!("self.{}", managed.active_field),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                        then_branch: vec![Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: format!(
                                    "{}{}",
                                    instance.proc_name, PROC_BLOCK_POST_FN_SUFFIX
                                ),
                                type_args: Vec::new(),
                                args: call_args,
                            },
                        }],
                        else_branch: Vec::new(),
                    });
                }
            }
            generated_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                type_params: Vec::new(),
                name: format!("{}{}", proc.name, PROC_BLOCK_POST_FN_SUFFIX),
                params: block_params,
                body: block_post_body,
            }));
        }
        let mut step_params = Vec::<omni_frontend::FnParamDecl>::new();
        step_params.push(omni_frontend::FnParamDecl {
            loc: Default::default(),
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            ty_loc: Default::default(),
            default: None,
        });
        for in_name in &shape.ins {
            let in_ty = *shape.in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
            step_params.push(omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: in_name.clone(),
                ty: Some(FnParamType::Primitive(in_ty)),
                ty_loc: Default::default(),
                default: None,
            });
        }
        for buffer in &shape.buffer_specs {
            step_params.push(omni_frontend::FnParamDecl {
                loc: Default::default(),
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                ty_loc: Default::default(),
                default: None,
            });
        }
        let step_fn_name = format!("{}{}", proc.name, PROC_STEP_FN_SUFFIX);
        let proc_sample_oversample_factor = proc_api
            .get(&proc.name)
            .map(|api| api.sample_oversample_factor)
            .unwrap_or(1)
            .max(1);
        def_sample_oversample_factors.insert(step_fn_name.clone(), proc_sample_oversample_factor);
        if proc_sample_oversample_factor > 1 {
            let stage_count = proc_os_sinc_stage_count(proc_sample_oversample_factor);
            let mut input_state_fields = HashMap::<String, ProcInputOversampleStateFields>::new();
            for in_name in &shape.ins {
                let in_ty = *shape.in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
                let up_stages = if matches!(in_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: proc_os_up_stage_tap_field_name(in_name, stage, "a0"),
                            a1: proc_os_up_stage_tap_field_name(in_name, stage, "a1"),
                            a2: proc_os_up_stage_tap_field_name(in_name, stage, "a2"),
                            a3: proc_os_up_stage_tap_field_name(in_name, stage, "a3"),
                            b0: proc_os_up_stage_tap_field_name(in_name, stage, "b0"),
                            b1: proc_os_up_stage_tap_field_name(in_name, stage, "b1"),
                            b2: proc_os_up_stage_tap_field_name(in_name, stage, "b2"),
                            b3: proc_os_up_stage_tap_field_name(in_name, stage, "b3"),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                input_state_fields.insert(
                    in_name.clone(),
                    ProcInputOversampleStateFields { up_stages },
                );
            }
            let mut output_state_fields = HashMap::<String, ProcOutputOversampleStateFields>::new();
            for out_name in &shape.outs {
                let out_ty = *shape.out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
                let down_stages = if matches!(out_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: proc_os_down_stage_tap_field_name(out_name, stage, "a0"),
                            a1: proc_os_down_stage_tap_field_name(out_name, stage, "a1"),
                            a2: proc_os_down_stage_tap_field_name(out_name, stage, "a2"),
                            a3: proc_os_down_stage_tap_field_name(out_name, stage, "a3"),
                            b0: proc_os_down_stage_tap_field_name(out_name, stage, "b0"),
                            b1: proc_os_down_stage_tap_field_name(out_name, stage, "b1"),
                            b2: proc_os_down_stage_tap_field_name(out_name, stage, "b2"),
                            b3: proc_os_down_stage_tap_field_name(out_name, stage, "b3"),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                output_state_fields.insert(
                    out_name.clone(),
                    ProcOutputOversampleStateFields { down_stages },
                );
            }
            proc_step_oversample_meta.insert(
                step_fn_name.clone(),
                ProcStepOversampleMeta {
                    input_state_fields,
                    output_state_fields,
                },
            );
        }
        generated_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            type_params: Vec::new(),
            name: step_fn_name.clone(),
            params: step_params.clone(),
            body: step_body,
        }));

        if !used_managed_dynamic_arrays.is_empty() || !nested_managed_active_fields.is_empty() {
            if let Some(Block::Struct(def)) = generated_structs.get_mut(struct_idx) {
                for array_base in &used_managed_dynamic_arrays {
                    let Some(managed) = managed_dynamic_arrays.get(array_base) else {
                        continue;
                    };
                    if def.fields.iter().any(|f| f.name == managed.active_field) {
                        continue;
                    }
                    def.fields.push(StructField {
                        loc: Default::default(),
                        name: managed.active_field.clone(),
                        ty: FieldType::Array(omni_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::int(managed.slots.len() as i64)),
                        }),
                        ty_loc: Default::default(),
                        default: None,
                    });
                }
                for (field_name, len) in &nested_managed_active_fields {
                    if def.fields.iter().any(|f| f.name == *field_name) {
                        continue;
                    }
                    def.fields.push(StructField {
                        loc: Default::default(),
                        name: field_name.clone(),
                        ty: FieldType::Array(omni_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::int(*len as i64)),
                        }),
                        ty_loc: Default::default(),
                        default: None,
                    });
                }
            }
        }

        for (idx, out_name) in shape.outs.iter().enumerate() {
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::var("self"),
            }];
            for in_name in &shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(in_name.clone()),
                });
            }
            for buffer in &shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(buffer.name.clone()),
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                type_params: Vec::new(),
                name: format!("{}{}{}", proc.name, PROC_CALL_OUT_FN_PREFIX, idx),
                params: step_params.clone(),
                body: vec![
                    Stmt::Expr {
                        loc: Default::default(),
                        expr: Expr::UserCall {
                            loc: Default::default(),
                            name: step_fn_name.clone(),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    },
                    Stmt::Return {
                        loc: Default::default(),
                        expr: Expr::var(format!("self.{out_name}")),
                    },
                ],
            }));
        }
    }

    (
        generated_structs,
        generated_defs,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
    )
}
