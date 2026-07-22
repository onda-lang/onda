use std::collections::{BTreeMap, HashMap, HashSet};

use super::*;

pub(super) struct GraphLoweringPlan {
    pub(super) edges: Vec<ResolvedGraphEdge>,
    pub(super) source_plans: Vec<ResolvedGraphSourcePlan>,
    pub(super) driven_outputs: HashSet<String>,
}

pub(super) fn build_graph_lowering_plan(
    graph: &GraphBlock,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> GraphLoweringPlan {
    fn inferred_edge_rate(dest: &GraphDestKind) -> GraphRate {
        match dest {
            GraphDestKind::ProcParam { .. } => GraphRate::Block,
            _ => GraphRate::Sample,
        }
    }

    let mut resolved = Vec::<ResolvedGraphEdge>::new();
    let mut source_plans = Vec::<ResolvedGraphSourcePlan>::new();
    let mut driven_outputs = HashSet::<String>::new();
    let mut single_writer = HashSet::<String>::new();
    let mut delayed_edge_counter = 0usize;
    let mut shared_tmp_counter = 0usize;

    for edge in &graph.edges {
        with_graph_edge_diag_context(edge, |diag| {
            let mut edge_dests = Vec::<(GraphDestKind, String, GraphValueType)>::new();
            let mut inferred_rates = Vec::<GraphRate>::new();
            let mut edge_failed = false;
            for dest in &edge.dests {
                let Ok((resolved_dest, dest_key, dest_value_ty)) = resolve_graph_dest(
                    dest,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ) else {
                    edge_failed = true;
                    continue;
                };
                inferred_rates.push(
                    edge.rate
                        .unwrap_or_else(|| inferred_edge_rate(&resolved_dest)),
                );
                edge_dests.push((resolved_dest, dest_key, dest_value_ty));
            }
            if edge_failed || edge_dests.is_empty() {
                return;
            }
            let rate = if let Some(rate) = edge.rate {
                rate
            } else {
                let inferred = inferred_rates[0];
                if inferred_rates.iter().any(|other| *other != inferred) {
                    push_semantic(
                        diag,
                        errors,
                        "graph fanout edge destinations require an explicit rate when their default rates differ",
                    );
                    return;
                }
                inferred
            };
            for (dest, dest_key, _) in &edge_dests {
                if !single_writer.insert(dest_key.clone()) {
                    push_semantic(
                        diag,
                        errors,
                        format!("graph destination '{dest_key}' has more than one driver"),
                    );
                }
                if let GraphDestKind::TopOutput(name) = dest {
                    driven_outputs.insert(name.clone());
                }
            }

            let delay = edge.delay.as_ref().and_then(|expr| {
                let context = format!("{owner_context} graph edge delay");
                eval_graph_nonnegative_int_expr(expr, options, &context, errors)
            });
            if delay.is_some() && rate != GraphRate::Sample {
                push_semantic(
                    diag,
                    errors,
                    "delayed graph edges are only supported for sample-rate destinations",
                );
            }

            let inferred_param_rate = edge.rate.is_none()
                && rate == GraphRate::Block
                && edge_dests
                    .iter()
                    .all(|(dest, _, _)| matches!(dest, GraphDestKind::ProcParam { .. }));
            let expansion = match expand_graph_bundle_source(
                &edge.source,
                edge_dests.len(),
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            ) {
                Ok(Some(expansion)) => expansion,
                Ok(None) => GraphSourceExpansion::Shared {
                    expr: edge.source.clone(),
                    use_shared_tmp: edge_dests.len() > 1,
                },
                Err(()) => return,
            };

            match expansion {
                GraphSourceExpansion::Shared {
                    expr,
                    use_shared_tmp,
                } => {
                    let source_plan = push_graph_source_plan(
                        &expr,
                        &edge_dests,
                        rate,
                        delay,
                        use_shared_tmp,
                        owner,
                        nodes,
                        proc_surfaces,
                        owner_context,
                        options,
                        inferred_param_rate,
                        &mut delayed_edge_counter,
                        &mut shared_tmp_counter,
                        &mut source_plans,
                        errors,
                    );
                    for (dest, _, dest_value_ty) in edge_dests {
                        resolved.push(ResolvedGraphEdge {
                            source_plan,
                            dest,
                            dest_value_ty,
                        });
                    }
                }
                GraphSourceExpansion::PerDest(exprs) => {
                    for (expr, (dest, dest_key, dest_value_ty)) in exprs.into_iter().zip(edge_dests)
                    {
                        let single_dest = vec![(dest.clone(), dest_key, dest_value_ty.clone())];
                        let source_plan = push_graph_source_plan(
                            &expr,
                            &single_dest,
                            rate,
                            delay,
                            false,
                            owner,
                            nodes,
                            proc_surfaces,
                            owner_context,
                            options,
                            inferred_param_rate,
                            &mut delayed_edge_counter,
                            &mut shared_tmp_counter,
                            &mut source_plans,
                            errors,
                        );
                        resolved.push(ResolvedGraphEdge {
                            source_plan,
                            dest,
                            dest_value_ty,
                        });
                    }
                }
            }
        });
    }

    GraphLoweringPlan {
        edges: resolved,
        source_plans,
        driven_outputs,
    }
}

fn push_graph_source_plan(
    source_expr: &Expr,
    edge_dests: &[(GraphDestKind, String, GraphValueType)],
    rate: GraphRate,
    delay: Option<usize>,
    use_shared_tmp: bool,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    inferred_param_rate: bool,
    delayed_edge_counter: &mut usize,
    shared_tmp_counter: &mut usize,
    source_plans: &mut Vec<ResolvedGraphSourcePlan>,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    let mut deps = BTreeSet::<GraphNodeKey>::new();
    validate_graph_source_expr(
        source_expr,
        owner,
        nodes,
        proc_surfaces,
        rate == GraphRate::Block,
        inferred_param_rate,
        &mut deps,
        owner_context,
        options,
        errors,
    );
    let src_value_ty = infer_graph_source_value_type(
        source_expr,
        owner,
        nodes,
        proc_surfaces,
        owner_context,
        options,
        errors,
    );
    if let Some(src_value_ty) = &src_value_ty {
        for (_, dest_key, dest_value_ty) in edge_dests {
            require_graph_assignable_type(
                src_value_ty,
                dest_value_ty,
                source_expr.loc(),
                &format!("{owner_context} graph edge source for destination '{dest_key}'"),
                errors,
            );
        }
    }

    let lowered_source = rewrite_graph_source_expr(
        source_expr,
        owner,
        nodes,
        proc_surfaces,
        owner_context,
        options,
        errors,
    );
    let mut source = lowered_source.clone();
    let delay_state = if delay.filter(|len| *len > 0).is_some() {
        let (dest_ty, array_len) = match &edge_dests[0].2 {
            GraphValueType::Scalar(dest_ty) => (*dest_ty, 1usize),
            GraphValueType::Array { elem_ty, len } => (*elem_ty, *len),
        };
        let buf_name = format!("__graph_delay_{}_buf", *delayed_edge_counter);
        let head_name = format!("__graph_delay_{}_head", *delayed_edge_counter);
        *delayed_edge_counter += 1;
        source = if array_len == 1 {
            Expr::Index {
                loc: Default::default(),
                base: buf_name.clone(),
                index: Box::new(Expr::var(head_name.clone())),
            }
        } else {
            Expr::ArrayLiteral {
                loc: Default::default(),
                values: (0..array_len)
                    .map(|slot| Expr::Index {
                        loc: Default::default(),
                        base: buf_name.clone(),
                        index: Box::new(graph_delay_flat_index_expr(&head_name, array_len, slot)),
                    })
                    .collect(),
            }
        };
        Some(GraphDelayState {
            buf_name,
            head_name,
            elem_ty: dest_ty,
            array_len,
        })
    } else {
        None
    };
    let shared_tmp = if use_shared_tmp && edge_dests.len() > 1 {
        let name = format!("__graph_fanout_{}", *shared_tmp_counter);
        *shared_tmp_counter += 1;
        Some(name)
    } else {
        None
    };
    let source_plan = source_plans.len();
    source_plans.push(ResolvedGraphSourcePlan {
        rate,
        delay,
        deps,
        original_source: lowered_source,
        source,
        delay_state,
        shared_tmp,
    });
    source_plan
}
