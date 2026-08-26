use super::*;

#[derive(Debug, Clone)]
struct RuntimeManagedProcArray {
    proc_name: String,
    slots: Vec<String>,
    active_symbol: String,
}

fn rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
    mut stmt: Stmt,
    proc_api: &HashMap<String, ProcApi>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    runtime_managed_arrays: &mut HashMap<String, RuntimeManagedProcArray>,
    temp_counter: &mut usize,
) -> Vec<Stmt> {
    fn temp_name(temp_counter: &mut usize, purpose: &str) -> String {
        let id = *temp_counter;
        *temp_counter += 1;
        format!("__onda_dynamic_proc_{purpose}_{id}")
    }

    fn assign_temp(name: String, expr: Expr, ty: Option<PrimitiveType>) -> Stmt {
        Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Var(name),
            decl_ty: ty,
            generic_decl_ty: None,
            is_typed_decl: ty.is_some(),
            typed_decl_ty_loc: Default::default(),
            expr,
        }
    }

    fn collect_guards_from_expr(
        expr: &mut Expr,
        proc_api: &HashMap<String, ProcApi>,
        proc_array_slots: &HashMap<String, Vec<String>>,
        runtime_managed_arrays: &mut HashMap<String, RuntimeManagedProcArray>,
        guards: &mut Vec<Stmt>,
        temp_counter: &mut usize,
    ) {
        match expr {
            Expr::Index { index, .. } => collect_guards_from_expr(
                index,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                guards,
                temp_counter,
            ),
            Expr::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    collect_guards_from_expr(
                        coordinate,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                        temp_counter,
                    );
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_guards_from_expr(
                    &mut spec.size,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                    temp_counter,
                );
                if let Some(values) = init {
                    for value in values {
                        collect_guards_from_expr(
                            value,
                            proc_api,
                            proc_array_slots,
                            runtime_managed_arrays,
                            guards,
                            temp_counter,
                        );
                    }
                }
            }
            Expr::Logical { op, lhs, rhs, .. } => {
                collect_guards_from_expr(
                    lhs,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                    temp_counter,
                );
                let mut rhs_guards = Vec::new();
                collect_guards_from_expr(
                    rhs,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    &mut rhs_guards,
                    temp_counter,
                );
                if !rhs_guards.is_empty() {
                    // Keep hook execution behind the language's short-circuit
                    // boundary while still producing one expression result.
                    let result = temp_name(temp_counter, "condition");
                    guards.push(assign_temp(
                        result.clone(),
                        lhs.as_ref().clone(),
                        Some(PrimitiveType::Bool),
                    ));
                    let branch_cond = match op {
                        LogicalOp::And => Expr::var(result.clone()),
                        LogicalOp::Or => Expr::UnaryNot {
                            loc: Default::default(),
                            expr: Box::new(Expr::var(result.clone())),
                        },
                    };
                    rhs_guards.push(assign_temp(result.clone(), rhs.as_ref().clone(), None));
                    guards.push(Stmt::If {
                        loc: Default::default(),
                        cond: branch_cond,
                        then_branch: rhs_guards,
                        else_branch: Vec::new(),
                    });
                    *expr = Expr::var(result);
                }
            }
            Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
                collect_guards_from_expr(
                    lhs,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                    temp_counter,
                );
                collect_guards_from_expr(
                    rhs,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    guards,
                    temp_counter,
                );
            }
            Expr::Call { args, .. } => {
                for arg in args.iter_mut() {
                    collect_guards_from_expr(
                        arg,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                        temp_counter,
                    );
                }
            }
            Expr::UserCall { name: _, args, .. } => {
                for arg in args.iter_mut() {
                    collect_guards_from_expr(
                        &mut arg.expr,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                        temp_counter,
                    );
                }
                let Expr::UserCall { name, args, .. } = expr else {
                    unreachable!();
                };
                let proc_name = if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
                    step_proc
                } else if let Some((call_proc, out_idx_raw)) =
                    name.rsplit_once(PROC_CALL_OUT_FN_PREFIX)
                {
                    if out_idx_raw.parse::<usize>().is_ok() {
                        call_proc
                    } else {
                        return;
                    }
                } else {
                    return;
                };
                let Some(api) = proc_api.get(proc_name) else {
                    return;
                };
                if !proc_needs_block_hooks(api) {
                    return;
                }
                let Some(CallArg {
                    expr: Expr::Index { base, index, .. },
                    ..
                }) = args.first_mut()
                else {
                    return;
                };
                if matches!(index.as_ref(), Expr::Int { .. }) {
                    return;
                }
                let array_base = base.clone();
                let Some(slots) = proc_array_slots.get(&array_base) else {
                    return;
                };
                runtime_managed_arrays
                    .entry(array_base.clone())
                    .or_insert_with(|| RuntimeManagedProcArray {
                        proc_name: proc_name.to_owned(),
                        slots: slots.clone(),
                        active_symbol: runtime_proc_array_active_symbol(&array_base),
                    });
                let active_symbol = runtime_proc_array_active_symbol(&array_base);
                // The selector participates in the hook and the original call;
                // cache it so source evaluation still happens exactly once.
                let selector = temp_name(temp_counter, "selector");
                guards.push(assign_temp(selector.clone(), index.as_ref().clone(), None));
                **index = Expr::var(selector.clone());
                let input_slots = api.ins.iter().map(|port| port.slots.len()).sum::<usize>();
                let buffer_start = 1 + input_slots;
                let mut pre_args = Vec::<CallArg>::new();
                pre_args.push(CallArg {
                    name: None,
                    expr: Expr::Index {
                        loc: Default::default(),
                        base: array_base,
                        index: Box::new(Expr::var(selector.clone())),
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
                            index: Box::new(Expr::var(selector.clone())),
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
                                index: Expr::var(selector),
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
                    temp_counter,
                );
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values.iter_mut() {
                    collect_guards_from_expr(
                        value,
                        proc_api,
                        proc_array_slots,
                        runtime_managed_arrays,
                        guards,
                        temp_counter,
                    );
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    let mut guards = Vec::<Stmt>::new();
    match &mut stmt {
        Stmt::Expr { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
            collect_guards_from_expr(
                expr,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut guards,
                temp_counter,
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
            mut cond,
            then_branch,
            else_branch,
        } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &mut cond,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut cond_guards,
                temp_counter,
            );
            let mut rewritten_then = Vec::<Stmt>::new();
            for nested in then_branch {
                rewritten_then.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    temp_counter,
                ));
            }
            let mut rewritten_else = Vec::<Stmt>::new();
            for nested in else_branch {
                rewritten_else.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    temp_counter,
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
            var_ty,
            mut start,
            mut end,
            mut step,
            end_inclusive,
            body,
        } => {
            let mut range_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &mut start,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut range_guards,
                temp_counter,
            );
            collect_guards_from_expr(
                &mut end,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut range_guards,
                temp_counter,
            );
            if let Some(step_expr) = &mut step {
                collect_guards_from_expr(
                    step_expr,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    &mut range_guards,
                    temp_counter,
                );
            }
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    temp_counter,
                ));
            }
            range_guards.push(Stmt::For {
                loc,
                var,
                var_ty,
                start,
                end,
                step,
                end_inclusive,
                body: rewritten_body,
            });
            range_guards
        }
        Stmt::While {
            loc,
            mut cond,
            body,
        } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &mut cond,
                proc_api,
                proc_array_slots,
                runtime_managed_arrays,
                &mut cond_guards,
                temp_counter,
            );
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                    nested,
                    proc_api,
                    proc_array_slots,
                    runtime_managed_arrays,
                    temp_counter,
                ));
            }
            if cond_guards.is_empty() {
                vec![Stmt::While {
                    loc,
                    cond,
                    body: rewritten_body,
                }]
            } else {
                // Condition hooks belong to each evaluation, including the one
                // that terminates the loop.
                cond_guards.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(cond),
                    },
                    then_branch: vec![Stmt::Break {
                        loc: Default::default(),
                    }],
                    else_branch: Vec::new(),
                });
                cond_guards.extend(rewritten_body);
                vec![Stmt::While {
                    loc,
                    cond: Expr::bool(true),
                    body: cond_guards,
                }]
            }
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

fn top_level_sample_oversample_factor(program: &Program, options: AnalysisOptions) -> usize {
    let factor_expr = program.blocks.iter().find_map(|block| match block {
        Block::Sample(sample) => sample.oversample_factor.as_ref(),
        Block::Block(exec) => exec
            .sample
            .as_ref()
            .and_then(|sample| sample.oversample_factor.as_ref()),
        _ => None,
    });
    let mut ignored_errors = Vec::<Diagnostic>::new();
    validated_sample_oversample_factor(factor_expr, options, "sample block", &mut ignored_errors)
        .max(1)
}

fn record_proc_instance_oversample_factor(
    instance_name: &str,
    call_oversample_factor: usize,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(instance) = global_proc_instances.get(instance_name) else {
        return;
    };
    let Some(api) = proc_api.get(&instance.proc_name) else {
        return;
    };
    let proc_oversample_factor = api.sample_oversample_factor.max(1);
    if call_oversample_factor > 1 && proc_oversample_factor > 1 {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "cannot call explicitly oversampled processor '{}' (sample {}) from oversampled context (sample {}); use a non-oversampled child processor in oversampled code",
                instance.proc_name, proc_oversample_factor, call_oversample_factor
            ),
        );
    }

    let effective_factor = if proc_oversample_factor > 1 {
        proc_oversample_factor
    } else {
        call_oversample_factor.max(1)
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

fn propagate_uniform_proc_array_oversample_factors(
    proc_array_slots: &HashMap<String, Vec<String>>,
    factors: &mut HashMap<String, usize>,
) {
    for (base, slots) in proc_array_slots {
        let mut slot_factors = slots.iter().filter_map(|slot| factors.get(slot).copied());
        let Some(first) = slot_factors.next() else {
            continue;
        };
        if slots
            .iter()
            .all(|slot| factors.get(slot).copied() == Some(first))
        {
            factors.insert(base.clone(), first);
        }
    }
}

fn reject_explicit_oversampled_child_calls_in_context(
    proc_name: &str,
    context_oversample_factor: usize,
    proc_defs_by_name: &HashMap<String, ProcessorDef>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    proc_api: &HashMap<String, ProcApi>,
    visiting: &mut HashSet<(String, usize)>,
    errors: &mut Vec<Diagnostic>,
) {
    if context_oversample_factor <= 1 {
        return;
    }
    if !visiting.insert((proc_name.to_owned(), context_oversample_factor)) {
        return;
    }
    let Some(proc) = proc_defs_by_name.get(proc_name) else {
        return;
    };
    let Some(shape) = lowering_shapes.get(proc_name) else {
        return;
    };
    let nested_instances = shape
        .state
        .nested_procs
        .iter()
        .map(|(name, nested)| {
            (
                name.clone(),
                ProcCallInstance {
                    proc_name: nested.proc_name.clone(),
                    buffer_args: Vec::new(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let called_nested = collect_called_proc_instances_in_stmts(
        &proc.sample,
        &nested_instances,
        &shape.nested_proc_array_slots,
    );
    for nested_name in called_nested {
        let Some(instance) = nested_instances.get(&nested_name) else {
            continue;
        };
        let Some(api) = proc_api.get(&instance.proc_name) else {
            continue;
        };
        let child_oversample_factor = api.sample_oversample_factor.max(1);
        if child_oversample_factor > 1 {
            push_semantic(
                DiagCtx::new(proc.loc),
                errors,
                format!(
                    "processor '{}' runs at sample {} and cannot call explicitly oversampled child '{}' of type '{}' (sample {}); remove the child's explicit oversampling or call it from base-rate code",
                    proc_name,
                    context_oversample_factor,
                    nested_name,
                    instance.proc_name,
                    child_oversample_factor
                ),
            );
            continue;
        }
        reject_explicit_oversampled_child_calls_in_context(
            &instance.proc_name,
            context_oversample_factor,
            proc_defs_by_name,
            lowering_shapes,
            proc_api,
            visiting,
            errors,
        );
    }
}

fn top_level_constructor_array_symbols(program: &Program) -> HashSet<String> {
    let mut symbols = HashSet::new();
    for block in &program.blocks {
        match block {
            Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
                for port in ports.iter() {
                    if matches!(
                        port.ty,
                        Some(DeclType::Array { .. } | DeclType::ArrayGeneric { .. })
                    ) || matches!(port.default, Some(Expr::ArrayLiteral { .. }))
                    {
                        symbols.insert(port.name.clone());
                    }
                }
            }
            Block::Params(params) => {
                for param in params.iter() {
                    if matches!(
                        param.ty,
                        Some(DeclType::Array { .. } | DeclType::ArrayGeneric { .. })
                    ) || matches!(param.default, Some(Expr::ArrayLiteral { .. }))
                    {
                        symbols.insert(param.name.clone());
                    }
                }
            }
            Block::Const(decl) => {
                if matches!(
                    decl.ty,
                    Some(ConstType::Array { .. } | ConstType::Slice { .. })
                ) || matches!(decl.expr, Expr::ArrayLiteral { .. })
                {
                    symbols.insert(decl.name.clone());
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    match stmt {
                        Stmt::Const { decl, .. }
                            if matches!(
                                decl.ty,
                                Some(ConstType::Array { .. } | ConstType::Slice { .. })
                            ) || matches!(decl.expr, Expr::ArrayLiteral { .. }) =>
                        {
                            symbols.insert(decl.name.clone());
                        }
                        Stmt::Assign {
                            target: AssignTarget::Var(name),
                            expr: Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. },
                            ..
                        } => {
                            symbols.insert(name.clone());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    symbols
}

pub(super) fn rewrite_top_level_proc_calls(
    program: &mut Program,
    runtime_def_names: &HashSet<String>,
    options: AnalysisOptions,
    proc_defs_by_name: &HashMap<String, ProcessorDef>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> TopLevelProcRewriteMeta {
    let constructor_array_symbols = top_level_constructor_array_symbols(program);
    let mut global_proc_instances = HashMap::<String, ProcCallInstance>::new();
    let mut global_proc_array_slots = HashMap::<String, Vec<String>>::new();
    let mut global_proc_instance_oversample_factors = HashMap::<String, usize>::new();
    let mut global_proc_array_broadcast_slots = HashMap::<String, (usize, usize)>::new();
    let mut runtime_managed_arrays = HashMap::<String, RuntimeManagedProcArray>::new();
    let mut dynamic_hook_temp_counter = 0;
    let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|b| b.kind() == BlockKind::Init)
    {
        let mut rewritten_init = Vec::<Stmt>::new();
        let mut constructor_setup_indices = HashSet::<usize>::new();
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
                        // The aggregate declaration materializes every flattened processor
                        // field, including pinned fields owned by nested tasks. The generated
                        // processor init below handles ordinary fields on every init, so the
                        // aggregate defaults are only valid during a full init.
                        expanded.extend(mark_pinned_initializer_stmt(decl_stmt));
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
                    proc_api,
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
                            // Keep the aggregate declaration for state/type discovery, but do
                            // not let its flattened defaults overwrite pinned processor fields
                            // during an ordinary init. The processor init emitted below still
                            // reinitializes all non-pinned fields.
                            rewritten_init.extend(mark_pinned_initializer_stmt(ctor_stmt));
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
                                &constructor_array_symbols,
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
                            for assign in ctor_assigns {
                                constructor_setup_indices.insert(rewritten_init.len());
                                rewritten_init.push(assign);
                            }
                            rewritten_init.push(Stmt::Expr {
                                loc: Default::default(),
                                expr: Expr::UserCall {
                                    loc: Default::default(),
                                    name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                    type_args: Vec::new(),
                                    args: vec![
                                        CallArg {
                                            name: None,
                                            expr: proc_instance_self_expr(
                                                var,
                                                &global_proc_array_slots,
                                            ),
                                        },
                                        CallArg {
                                            name: None,
                                            expr: Expr::var(TOP_LEVEL_INIT_ALL_NAME),
                                        },
                                    ],
                                },
                            });
                        } else {
                            rewritten_init.push(Stmt::Expr {
                                loc: Default::default(),
                                expr: Expr::UserCall {
                                    loc: Default::default(),
                                    name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                    type_args: Vec::new(),
                                    args: vec![
                                        CallArg {
                                            name: None,
                                            expr: proc_instance_self_expr(
                                                var,
                                                &global_proc_array_slots,
                                            ),
                                        },
                                        CallArg {
                                            name: None,
                                            expr: Expr::var(TOP_LEVEL_INIT_ALL_NAME),
                                        },
                                    ],
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
        rewrite_proc_calls_in_stmts_without_hooks(
            &mut init.body,
            &global_proc_instances,
            &global_proc_array_slots,
            proc_api,
            errors,
        );
        inject_bound_proc_param_hooks_in_stmts_skipping_top_level(
            None,
            &mut init.body,
            &global_proc_instances,
            &global_proc_array_slots,
            proc_api,
            errors,
            &constructor_setup_indices,
        );
    }

    for (base, slots) in &global_proc_array_slots {
        let Some(first_slot) = slots.first() else {
            continue;
        };
        let Some(instance) = global_proc_instances.get(first_slot) else {
            continue;
        };
        let Some(api) = proc_api.get(&instance.proc_name) else {
            continue;
        };
        if !proc_needs_block_hooks(api) {
            continue;
        }
        runtime_managed_arrays
            .entry(base.clone())
            .or_insert_with(|| RuntimeManagedProcArray {
                proc_name: instance.proc_name.clone(),
                slots: slots.clone(),
                active_symbol: runtime_proc_array_active_symbol(base),
            });
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
        let sample_oversample_factor = top_level_sample_oversample_factor(program, options);
        for instance_name in &called_order {
            record_proc_instance_oversample_factor(
                instance_name,
                sample_oversample_factor,
                &global_proc_instances,
                proc_api,
                &mut global_proc_instance_oversample_factors,
                errors,
            );
        }
        propagate_uniform_proc_array_oversample_factors(
            &global_proc_array_slots,
            &mut global_proc_instance_oversample_factors,
        );
        let mut rate_validation_visiting = HashSet::<(String, usize)>::new();
        for instance_name in &called_order {
            let Some(instance) = global_proc_instances.get(instance_name) else {
                continue;
            };
            let Some(api) = proc_api.get(&instance.proc_name) else {
                continue;
            };
            let explicit_factor = api.sample_oversample_factor.max(1);
            let effective_factor = if explicit_factor > 1 {
                explicit_factor
            } else {
                sample_oversample_factor
            };
            reject_explicit_oversampled_child_calls_in_context(
                &instance.proc_name,
                effective_factor,
                proc_defs_by_name,
                lowering_shapes,
                proc_api,
                &mut rate_validation_visiting,
                errors,
            );
        }
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
            if !proc_needs_block_hooks(api) {
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
                    proc_api,
                    errors,
                );
                if let Some(sample) = &mut exec.sample {
                    rewrite_proc_calls_in_stmts(
                        sample,
                        &global_proc_instances,
                        &global_proc_array_slots,
                        proc_api,
                        errors,
                    );
                    let mut rewritten_sample = Vec::<Stmt>::new();
                    for stmt in std::mem::take(&mut sample.body) {
                        rewritten_sample.extend(
                            rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                                stmt,
                                proc_api,
                                &global_proc_array_slots,
                                &mut runtime_managed_arrays,
                                &mut dynamic_hook_temp_counter,
                            ),
                        );
                    }
                    sample.body = rewritten_sample;
                }
                rewrite_proc_calls_in_stmts(
                    &mut exec.post,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            Block::Sample(stmts) => {
                rewrite_proc_calls_in_stmts(
                    stmts,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    proc_api,
                    errors,
                );
                let mut rewritten_sample = Vec::<Stmt>::new();
                for stmt in std::mem::take(&mut stmts.body) {
                    rewritten_sample.extend(rewrite_stmt_for_runtime_managed_dynamic_proc_blocks(
                        stmt,
                        proc_api,
                        &global_proc_array_slots,
                        &mut runtime_managed_arrays,
                        &mut dynamic_hook_temp_counter,
                    ));
                }
                stmts.body = rewritten_sample;
            }
            Block::Def(def) if runtime_def_names.contains(&def.name) => {
                rewrite_proc_calls_in_stmts(
                    &mut def.body,
                    &global_proc_instances,
                    &global_proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            Block::Def(_) => {}
            Block::Events(events) => {
                for event in events {
                    rewrite_proc_calls_in_stmts(
                        &mut event.body,
                        &global_proc_instances,
                        &global_proc_array_slots,
                        proc_api,
                        errors,
                    );
                }
            }
            Block::When(when) => rewrite_proc_calls_in_stmts(
                &mut when.body,
                &global_proc_instances,
                &global_proc_array_slots,
                proc_api,
                errors,
            ),
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
            managed.sort_by_key(|(a, _)| *a);
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
                        spec: onda_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::int(len as i64)),
                        },
                        init: Some(vec![Expr::bool(false); len]),
                        initialize: true,
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
            managed.sort_by_key(|(a, _)| *a);
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

    TopLevelProcRewriteMeta {
        global_proc_instances,
        global_proc_array_slots,
        global_proc_instance_oversample_factors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalysisOptions;
    use onda_frontend::{parse_program, AssignTarget, Expr};

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
                .any(|array| array.name == "__onda_proc_block_active_voices"
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
                } if base == "__onda_proc_block_active_voices"
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
