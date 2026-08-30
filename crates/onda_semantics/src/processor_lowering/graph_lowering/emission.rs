use super::*;

pub(super) fn assign_stmt(target: String, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(target),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

pub(super) fn graph_delay_flat_index_expr(head_name: &str, array_len: usize, slot: usize) -> Expr {
    if array_len <= 1 {
        return Expr::var(head_name.to_owned());
    }
    let base = Expr::Binary {
        loc: Default::default(),
        op: BinaryOp::Mul,
        lhs: Box::new(Expr::var(head_name.to_owned())),
        rhs: Box::new(Expr::int(array_len as i64)),
    };
    if slot == 0 {
        base
    } else {
        Expr::Binary {
            loc: Default::default(),
            op: BinaryOp::Add,
            lhs: Box::new(base),
            rhs: Box::new(Expr::int(slot as i64)),
        }
    }
}

pub(super) fn call_stmt(expr: Expr) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr,
    }
}

pub(super) fn assign_node_field_stmt(node: &GraphNodeKey, field: &str, expr: Expr) -> Stmt {
    match node {
        GraphNodeKey::Direct(name) => assign_stmt(format!("{name}.{field}"), expr),
        GraphNodeKey::Indexed { base, index } => {
            assign_stmt(format!("{base}[{index}].{field}"), expr)
        }
    }
}

pub(super) fn assign_node_array_field_stmts(
    node: &GraphNodeKey,
    field: &str,
    expr: &Expr,
    owner: &GraphOwnerSurface,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    let Some(node_info) = nodes.get(node) else {
        return Vec::new();
    };
    let Some(surface) = proc_surfaces.get(&node_info.proc_name) else {
        return Vec::new();
    };
    let Some(slot_names) = surface.param_array_slots.get(field) else {
        return vec![assign_node_field_stmt(node, field, expr.clone())];
    };
    let slot_exprs = expand_graph_expr_to_slots(
        expr,
        slot_names.len(),
        owner,
        nodes,
        proc_surfaces,
        &format!(
            "{owner_context} graph destination '{}.{}'",
            node_ref_name(node),
            field
        ),
        options,
        errors,
    );
    slot_names
        .iter()
        .zip(slot_exprs)
        .map(|(slot_name, slot_expr)| assign_node_field_stmt(node, slot_name, slot_expr))
        .collect()
}

pub(super) fn build_node_call_expr(node: &GraphNodeKey, mut args: Vec<CallArg>) -> Expr {
    match node {
        GraphNodeKey::Direct(name) => Expr::UserCall {
            loc: Default::default(),
            name: name.clone(),
            type_args: Vec::new(),
            args,
        },
        GraphNodeKey::Indexed { base, index } => {
            args.insert(
                0,
                CallArg {
                    name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                    expr: Expr::int(*index as i64),
                },
            );
            args.insert(
                0,
                CallArg {
                    name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                    expr: Expr::var(base.clone()),
                },
            );
            Expr::UserCall {
                loc: Default::default(),
                name: PROC_INDEX_CALL_SENTINEL.to_owned(),
                type_args: Vec::new(),
                args,
            }
        }
    }
}

pub(super) fn lower_graph(
    graph: &GraphBlock,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> LoweredGraph {
    with_loc_diag_context(graph.loc.as_ref(), |graph_diag| {
        let GraphLoweringPlan {
            edges: resolved,
            source_plans,
            driven_outputs,
        } = build_graph_lowering_plan(
            graph,
            owner,
            nodes,
            proc_surfaces,
            owner_context,
            options,
            errors,
        );

        for output in owner.output_value_types.keys() {
            if !driven_outputs.contains(output) {
                push_semantic(
                    graph_diag,
                    errors,
                    format!("graph must drive declared output '{output}'"),
                );
            }
        }

        let reachable = reachable_nodes_from_outputs(&resolved, &source_plans);
        let topo = topo_sort_nodes(&resolved, &source_plans, &reachable, graph_diag, errors);
        let topo_positions = topo
            .iter()
            .enumerate()
            .map(|(idx, node)| (node.clone(), idx))
            .collect::<HashMap<_, _>>();

        let mut init_stmts = Vec::<Stmt>::new();
        for plan in &source_plans {
            let Some(delay_state) = &plan.delay_state else {
                continue;
            };
            let delay_len = plan.delay.unwrap_or(0);
            if delay_len == 0 {
                continue;
            }
            init_stmts.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(delay_state.buf_name.clone()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::ArrayCtor {
                    loc: Default::default(),
                    spec: ArrayTypeSpec {
                        elem: ArrayElemType::Primitive(delay_state.elem_ty),
                        size: Box::new(Expr::int((delay_len * delay_state.array_len) as i64)),
                    },
                    init: None,
                    initialize: true,
                },
            });
            init_stmts.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(delay_state.head_name.clone()),
                decl_ty: Some(DeclType::Scalar(PrimitiveType::I32)),
                generic_decl_ty: None,
                is_typed_decl: true,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int(0),
            });
        }

        let mut block_pre_temps = Vec::<Stmt>::new();
        let mut block_pre = Vec::<Stmt>::new();
        let mut sample = Vec::<Stmt>::new();
        let mut sample_input_edges = BTreeMap::<GraphNodeKey, Vec<(String, Expr)>>::new();
        let mut sample_param_edges = BTreeMap::<GraphNodeKey, Vec<(String, Expr)>>::new();
        let mut output_edges = Vec::<(String, Expr)>::new();
        let mut sample_temp_before_node = BTreeMap::<GraphNodeKey, Vec<Stmt>>::new();
        let mut sample_temp_before_outputs = Vec::<Stmt>::new();
        let mut sample_temp_use_points = HashMap::<usize, GraphUsePoint>::new();

        for edge in &resolved {
            let plan = &source_plans[edge.source_plan];
            let edge_source = if let Some(tmp) = &plan.shared_tmp {
                Expr::var(tmp.clone())
            } else {
                plan.source.clone()
            };
            match (&plan.rate, &edge.dest, &edge.dest_value_ty) {
                (
                    GraphRate::Block,
                    GraphDestKind::ProcParam { node, param },
                    GraphValueType::Scalar(_),
                ) => {
                    if reachable.contains(node) {
                        block_pre.push(assign_node_field_stmt(node, param, edge_source));
                    }
                }
                (
                    GraphRate::Block,
                    GraphDestKind::ProcParam { node, param },
                    GraphValueType::Array { .. },
                ) => {
                    if reachable.contains(node) {
                        block_pre.extend(assign_node_array_field_stmts(
                            node,
                            param,
                            &edge_source,
                            owner,
                            proc_surfaces,
                            nodes,
                            owner_context,
                            options,
                            errors,
                        ));
                    }
                }
                (
                    GraphRate::Sample,
                    GraphDestKind::ProcInput { node, port },
                    GraphValueType::Scalar(_),
                ) => {
                    if reachable.contains(node) {
                        note_graph_use_point(
                            &mut sample_temp_use_points,
                            edge.source_plan,
                            GraphUsePoint::BeforeNode(node.clone()),
                            &topo_positions,
                        );
                        sample_input_edges
                            .entry(node.clone())
                            .or_default()
                            .push((port.clone(), edge_source));
                    }
                }
                (
                    GraphRate::Sample,
                    GraphDestKind::ProcInput { node, port },
                    GraphValueType::Array { len, .. },
                ) => {
                    if reachable.contains(node) {
                        note_graph_use_point(
                            &mut sample_temp_use_points,
                            edge.source_plan,
                            GraphUsePoint::BeforeNode(node.clone()),
                            &topo_positions,
                        );
                        let slot_exprs = expand_graph_expr_to_slots(
                            &edge_source,
                            *len,
                            owner,
                            nodes,
                            proc_surfaces,
                            &format!(
                                "{owner_context} graph input '{}.{}'",
                                node_ref_name(node),
                                port
                            ),
                            options,
                            errors,
                        );
                        sample_input_edges
                            .entry(node.clone())
                            .or_default()
                            .push((port.clone(), Expr::array_literal(slot_exprs)));
                    }
                }
                (
                    GraphRate::Sample,
                    GraphDestKind::ProcParam { node, param },
                    GraphValueType::Scalar(_),
                ) => {
                    if reachable.contains(node) {
                        note_graph_use_point(
                            &mut sample_temp_use_points,
                            edge.source_plan,
                            GraphUsePoint::BeforeNode(node.clone()),
                            &topo_positions,
                        );
                        sample_param_edges
                            .entry(node.clone())
                            .or_default()
                            .push((param.clone(), edge_source));
                    }
                }
                (
                    GraphRate::Sample,
                    GraphDestKind::ProcParam { node, param },
                    GraphValueType::Array { .. },
                ) => {
                    if reachable.contains(node) {
                        note_graph_use_point(
                            &mut sample_temp_use_points,
                            edge.source_plan,
                            GraphUsePoint::BeforeNode(node.clone()),
                            &topo_positions,
                        );
                        sample.extend(assign_node_array_field_stmts(
                            node,
                            param,
                            &edge_source,
                            owner,
                            proc_surfaces,
                            nodes,
                            owner_context,
                            options,
                            errors,
                        ));
                    }
                }
                (_, GraphDestKind::TopOutput(name), _) => {
                    if plan.rate == GraphRate::Sample {
                        note_graph_use_point(
                            &mut sample_temp_use_points,
                            edge.source_plan,
                            GraphUsePoint::BeforeOutputs,
                            &topo_positions,
                        );
                    }
                    output_edges.push((name.clone(), edge_source));
                }
                _ => {}
            }
        }

        let mut emitted_block_temp = HashSet::<usize>::new();
        for edge in &resolved {
            let plan = &source_plans[edge.source_plan];
            if plan.rate == GraphRate::Block && plan.shared_tmp.is_some() {
                emitted_block_temp.insert(edge.source_plan);
            }
        }
        for plan_idx in emitted_block_temp {
            let plan = &source_plans[plan_idx];
            block_pre_temps.push(assign_stmt(
                plan.shared_tmp.clone().unwrap(),
                plan.source.clone(),
            ));
        }
        for (plan_idx, use_point) in sample_temp_use_points {
            let plan = &source_plans[plan_idx];
            let Some(tmp) = &plan.shared_tmp else {
                continue;
            };
            let stmt = assign_stmt(tmp.clone(), plan.source.clone());
            match use_point {
                GraphUsePoint::BeforeNode(node) => {
                    sample_temp_before_node.entry(node).or_default().push(stmt);
                }
                GraphUsePoint::BeforeOutputs => sample_temp_before_outputs.push(stmt),
            }
        }
        if !block_pre_temps.is_empty() {
            block_pre_temps.extend(block_pre);
            block_pre = block_pre_temps;
        }

        for node in topo {
            if let Some(temp_stmts) = sample_temp_before_node.remove(&node) {
                sample.extend(temp_stmts);
            }
            if let Some(param_edges) = sample_param_edges.get(&node) {
                for (param, expr) in param_edges {
                    sample.push(assign_node_field_stmt(&node, param, expr.clone()));
                }
            }
            let call_args = sample_input_edges
                .get(&node)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, expr)| CallArg {
                    name: Some(name),
                    expr,
                })
                .collect::<Vec<_>>();
            sample.push(call_stmt(build_node_call_expr(&node, call_args)));
        }

        sample.extend(sample_temp_before_outputs);
        for (name, expr) in output_edges {
            if let Some(GraphValueType::Array { len, .. }) = owner.output_value_types.get(&name) {
                let slot_exprs = expand_graph_expr_to_slots(
                    &expr,
                    *len,
                    owner,
                    nodes,
                    proc_surfaces,
                    &format!("{owner_context} graph output '{name}'"),
                    options,
                    errors,
                );
                for (idx, slot_expr) in slot_exprs.into_iter().enumerate() {
                    sample.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: name.clone(),
                            index: Expr::int(idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: slot_expr,
                    });
                }
            } else {
                sample.push(assign_stmt(name, expr));
            }
        }

        for plan in source_plans {
            let Some(delay_state) = plan.delay_state else {
                continue;
            };
            if delay_state.array_len == 1 {
                sample.push(Stmt::Assign {
                    loc: Default::default(),
                    target_loc: Default::default(),
                    target: AssignTarget::Index {
                        base: delay_state.buf_name.clone(),
                        index: Expr::var(delay_state.head_name.clone()),
                    },
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    typed_decl_ty_loc: Default::default(),
                    expr: plan.original_source,
                });
            } else {
                let slot_exprs = expand_graph_expr_to_slots(
                    &plan.original_source,
                    delay_state.array_len,
                    owner,
                    nodes,
                    proc_surfaces,
                    &format!("{owner_context} delayed graph edge writeback"),
                    options,
                    errors,
                );
                for (slot, slot_expr) in slot_exprs.into_iter().enumerate() {
                    sample.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: delay_state.buf_name.clone(),
                            index: graph_delay_flat_index_expr(
                                &delay_state.head_name,
                                delay_state.array_len,
                                slot,
                            ),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: slot_expr,
                    });
                }
            }
            sample.push(assign_stmt(
                delay_state.head_name.clone(),
                Expr::Binary {
                    loc: Default::default(),
                    op: BinaryOp::Mod,
                    lhs: Box::new(Expr::Binary {
                        loc: Default::default(),
                        op: BinaryOp::Add,
                        lhs: Box::new(Expr::var(delay_state.head_name.clone())),
                        rhs: Box::new(Expr::int(1)),
                    }),
                    rhs: Box::new(Expr::int(plan.delay.unwrap_or(1) as i64)),
                },
            ));
        }

        LoweredGraph {
            init_stmts,
            block_pre,
            sample,
        }
    })
}
