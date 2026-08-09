use super::*;

pub(super) fn rewrite_graph_source_expr(
    expr: &Expr,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let expr_loc = expr.loc().cloned();
    match expr {
        Expr::Var { name, .. } => {
            if let Some((node_name, field)) = name.rsplit_once('.') {
                let key = GraphNodeKey::Direct(node_name.to_owned());
                if let Some(node) = nodes.get(&key) {
                    if let Some(surface) = proc_surfaces.get(&node.proc_name) {
                        let resolved_out = resolve_graph_proc_output_name(surface, field);
                        if let Some(slots) = surface.out_array_slots.get(resolved_out) {
                            return Expr::ArrayLiteral {
                                loc: expr_loc.into(),
                                values: slots
                                    .iter()
                                    .map(|slot_name| {
                                        Expr::var(format!("{node_name}.{slot_name}"))
                                            .with_loc(expr_loc)
                                    })
                                    .collect(),
                            };
                        }
                    }
                }
            }
            if let Some((node_name, field)) = name.rsplit_once('.') {
                let key = GraphNodeKey::Direct(node_name.to_owned());
                if let Some(node) = nodes.get(&key) {
                    if let Some(surface) = proc_surfaces.get(&node.proc_name) {
                        let resolved_out = resolve_graph_proc_output_name(surface, field);
                        if resolved_out != field {
                            return Expr::var(format!("{node_name}.{resolved_out}"))
                                .with_loc(expr_loc);
                        }
                    }
                }
            }
            let resolved_input = resolve_graph_owner_input_name(owner, name);
            if resolved_input != name {
                return Expr::var(resolved_input.to_owned()).with_loc(expr_loc);
            }
            expr.clone()
        }
        Expr::Index { base, index, .. } => {
            let rewritten_index = rewrite_graph_source_expr(
                index,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            );
            if let Some((node_name, field)) = base.rsplit_once('.') {
                let key = GraphNodeKey::Direct(node_name.to_owned());
                if let Some(node) = nodes.get(&key) {
                    if let Some(surface) = proc_surfaces.get(&node.proc_name) {
                        let resolved_out = resolve_graph_proc_output_name(surface, field);
                        if let Some(slots) = surface.out_array_slots.get(resolved_out) {
                            let context = format!("{owner_context} graph source '{}[...]'", base);
                            if let Some(idx) =
                                eval_graph_nonnegative_int_expr(index, options, &context, errors)
                            {
                                if let Some(slot_name) = slots.get(idx) {
                                    return Expr::var(format!("{node_name}.{slot_name}"))
                                        .with_loc(expr_loc);
                                }
                                errors.push(Diagnostic::semantic_span(
                                    format!(
                                        "{context} index {idx} is out of range (expected 0..{})",
                                        slots.len().saturating_sub(1)
                                    ),
                                    index.loc(),
                                ));
                            }
                        }
                    }
                }
            }
            Expr::Index {
                loc: expr_loc.into(),
                base: base.clone(),
                index: Box::new(rewritten_index),
            }
        }
        Expr::ArrayLiteral { values, .. } => Expr::ArrayLiteral {
            loc: expr_loc.into(),
            values: values
                .iter()
                .map(|value| {
                    rewrite_graph_source_expr(
                        value,
                        owner,
                        nodes,
                        proc_surfaces,
                        owner_context,
                        options,
                        errors,
                    )
                })
                .collect(),
        },
        Expr::Compare { op, lhs, rhs, .. } => Expr::Compare {
            loc: expr_loc.into(),
            op: *op,
            lhs: Box::new(rewrite_graph_source_expr(
                lhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
            rhs: Box::new(rewrite_graph_source_expr(
                rhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Logical { op, lhs, rhs, .. } => Expr::Logical {
            loc: expr_loc.into(),
            op: *op,
            lhs: Box::new(rewrite_graph_source_expr(
                lhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
            rhs: Box::new(rewrite_graph_source_expr(
                rhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Binary { op, lhs, rhs, .. } => Expr::Binary {
            loc: expr_loc.into(),
            op: *op,
            lhs: Box::new(rewrite_graph_source_expr(
                lhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
            rhs: Box::new(rewrite_graph_source_expr(
                rhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Call { func, args, .. } => Expr::Call {
            loc: expr_loc.into(),
            func: *func,
            args: args
                .iter()
                .map(|arg| {
                    rewrite_graph_source_expr(
                        arg,
                        owner,
                        nodes,
                        proc_surfaces,
                        owner_context,
                        options,
                        errors,
                    )
                })
                .collect(),
        },
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    });
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG);
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var { name, .. } => Some(name.clone()),
                            _ => None,
                        },
                    );
                if let (Some(base), Some(proc_index), Some(field)) = (base, proc_index, field) {
                    let proc_context = format!("{owner_context} graph source '{}[...]'", base);
                    if let Some(proc_idx) =
                        eval_graph_nonnegative_int_expr(proc_index, options, &proc_context, errors)
                    {
                        let key = GraphNodeKey::Indexed {
                            base: base.clone(),
                            index: proc_idx,
                        };
                        if let Some(node) = nodes.get(&key) {
                            if let Some(surface) = proc_surfaces.get(&node.proc_name) {
                                let resolved_out = resolve_graph_proc_output_name(surface, &field);
                                if let Some(slots) = surface.out_array_slots.get(resolved_out) {
                                    return Expr::ArrayLiteral {
                                        loc: expr_loc.into(),
                                        values: slots
                                            .iter()
                                            .map(|slot_name| Expr::UserCall {
                                                loc: expr_loc.into(),
                                                name: format!(
                                                    "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
                                                ),
                                                type_args: Vec::new(),
                                                args: vec![
                                                    CallArg {
                                                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                                        expr: Expr::var(base.clone())
                                                            .with_loc(expr_loc),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                                        expr: Expr::int(proc_idx as i64)
                                                            .with_loc(proc_index.loc().cloned()),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                                        expr: Expr::var(slot_name.clone())
                                                            .with_loc(expr_loc),
                                                    },
                                                ],
                                            })
                                            .collect(),
                                    };
                                }
                            }
                        }
                    }
                }
            }
            if name == GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    });
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG);
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var { name, .. } => Some(name.clone()),
                            _ => None,
                        },
                    );
                let field_index = named_call_arg_expr(args, GRAPH_PROC_FIELD_INDEX_EXPR_ARG);
                if let (Some(base), Some(proc_index), Some(field), Some(field_index)) =
                    (base, proc_index, field, field_index)
                {
                    let proc_context = format!("{owner_context} graph source '{}[...]'", base);
                    if let Some(proc_idx) =
                        eval_graph_nonnegative_int_expr(proc_index, options, &proc_context, errors)
                    {
                        let key = GraphNodeKey::Indexed {
                            base: base.clone(),
                            index: proc_idx,
                        };
                        if let Some(node) = nodes.get(&key) {
                            if let Some(surface) = proc_surfaces.get(&node.proc_name) {
                                let resolved_out = resolve_graph_proc_output_name(surface, &field);
                                if let Some(slots) = surface.out_array_slots.get(resolved_out) {
                                    let field_context = format!(
                                        "{owner_context} graph source '{}.{}[...]'",
                                        node_ref_name(&key),
                                        field
                                    );
                                    if let Some(field_idx) = eval_graph_nonnegative_int_expr(
                                        field_index,
                                        options,
                                        &field_context,
                                        errors,
                                    ) {
                                        if let Some(slot_name) = slots.get(field_idx) {
                                            return Expr::UserCall {
                                                loc: expr_loc.into(),
                                                name: format!(
                                                    "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
                                                ),
                                                type_args: Vec::new(),
                                                args: vec![
                                                    CallArg {
                                                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                                        expr: Expr::var(base.clone())
                                                            .with_loc(expr_loc),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                                        expr: Expr::int(proc_idx as i64)
                                                            .with_loc(proc_index.loc().cloned()),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                                        expr: Expr::var(slot_name.clone())
                                                            .with_loc(field_index.loc().cloned()),
                                                    },
                                                ],
                                            };
                                        }
                                        errors.push(Diagnostic::semantic_span(
                                            format!(
                                                "{field_context} index {field_idx} is out of range (expected 0..{})",
                                                slots.len().saturating_sub(1)
                                            ),
                                            field_index.loc(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Expr::UserCall {
                loc: expr_loc.into(),
                name: name.clone(),
                type_args: type_args.clone(),
                args: args
                    .iter()
                    .map(|arg| CallArg {
                        name: arg.name.clone(),
                        expr: rewrite_graph_source_expr(
                            &arg.expr,
                            owner,
                            nodes,
                            proc_surfaces,
                            owner_context,
                            options,
                            errors,
                        ),
                    })
                    .collect(),
            }
        }
        Expr::Cast { to, expr, .. } => Expr::Cast {
            loc: expr_loc.into(),
            to: *to,
            expr: Box::new(rewrite_graph_source_expr(
                expr,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::UnaryNot { expr, .. } => Expr::UnaryNot {
            loc: expr_loc.into(),
            expr: Box::new(rewrite_graph_source_expr(
                expr,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::UnaryBitNot { expr, .. } => Expr::UnaryBitNot {
            loc: expr_loc.into(),
            expr: Box::new(rewrite_graph_source_expr(
                expr,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => Expr::Slice {
            loc: expr_loc.into(),
            base: base.clone(),
            selector: selector.as_ref().map(|expr| {
                Box::new(rewrite_graph_source_expr(
                    expr,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ))
            }),
            channel: channel.as_ref().map(|expr| {
                Box::new(rewrite_graph_source_expr(
                    expr,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ))
            }),
            start: start.as_ref().map(|expr| {
                Box::new(rewrite_graph_source_expr(
                    expr,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ))
            }),
            end: end.as_ref().map(|expr| {
                Box::new(rewrite_graph_source_expr(
                    expr,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ))
            }),
        },
        Expr::ArrayCtor { spec, init, .. } => Expr::ArrayCtor {
            loc: expr_loc.into(),
            spec: ArrayTypeSpec {
                elem: spec.elem.clone(),
                size: Box::new(rewrite_graph_source_expr(
                    &spec.size,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                )),
            },
            init: init.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| {
                        rewrite_graph_source_expr(
                            value,
                            owner,
                            nodes,
                            proc_surfaces,
                            owner_context,
                            options,
                            errors,
                        )
                    })
                    .collect()
            }),
        },
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Tuple { .. } => {
            expr.clone()
        }
    }
}
