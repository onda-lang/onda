use std::collections::{BTreeMap, BTreeSet, HashMap};

use onda_frontend::SourceLoc;

use super::*;

pub(super) fn infer_graph_source_base_value_type(
    base: &str,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
) -> Option<GraphValueType> {
    let base = resolve_graph_owner_input_name(owner, base);
    owner
        .param_value_types
        .get(base)
        .cloned()
        .or_else(|| owner.input_value_types.get(base).cloned())
        .or_else(|| {
            base.rsplit_once('.').and_then(|(node_base, field)| {
                infer_graph_proc_field_value_type(
                    &GraphNodeKey::Direct(node_base.to_owned()),
                    field,
                    nodes,
                    proc_surfaces,
                )
            })
        })
}

pub(super) fn node_ref_name(node: &GraphNodeKey) -> String {
    match node {
        GraphNodeKey::Direct(name) => name.clone(),
        GraphNodeKey::Indexed { base, index } => format!("{base}[{index}]"),
    }
}

pub(super) fn named_call_arg_expr<'a>(args: &'a [CallArg], arg_name: &str) -> Option<&'a Expr> {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(arg_name))
        .map(|arg| &arg.expr)
}

pub(super) fn collect_graph_nodes_from_init(
    init: &[Stmt],
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    options: AnalysisOptions,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) -> BTreeMap<GraphNodeKey, GraphNodeInfo> {
    let mut out = BTreeMap::<GraphNodeKey, GraphNodeInfo>::new();
    for stmt in init {
        let Stmt::Assign {
            target: AssignTarget::Var(name),
            expr,
            ..
        } = stmt
        else {
            continue;
        };
        match expr {
            Expr::UserCall {
                name: ctor_name, ..
            } => {
                if let Some(surface) = proc_surfaces.get(ctor_name) {
                    out.insert(
                        GraphNodeKey::Direct(name.clone()),
                        GraphNodeInfo {
                            proc_name: surface.proc_name.clone(),
                        },
                    );
                }
            }
            Expr::ArrayCtor { spec, .. } => {
                let ArrayElemType::Struct(proc_name) = &spec.elem else {
                    continue;
                };
                if !proc_surfaces.contains_key(proc_name) {
                    continue;
                }
                let size_context = format!("{owner_context} graph node array '{name}' size");
                let Some(len) = eval_data_size_expr(&spec.size, options, &size_context, errors)
                else {
                    continue;
                };
                for idx in 0..len {
                    out.insert(
                        GraphNodeKey::Indexed {
                            base: name.clone(),
                            index: idx,
                        },
                        GraphNodeInfo {
                            proc_name: proc_name.clone(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
    out
}

pub(super) fn expand_graph_bundle_source(
    source: &Expr,
    dest_count: usize,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Result<Option<GraphSourceExpansion>, ()> {
    if dest_count <= 1 {
        return Ok(None);
    }

    let key = match source {
        Expr::Var { name, .. }
            if !owner.param_value_types.contains_key(name)
                && !owner
                    .input_value_types
                    .contains_key(resolve_graph_owner_input_name(owner, name))
                && !owner
                    .output_value_types
                    .contains_key(resolve_graph_owner_output_name(owner, name)) =>
        {
            GraphNodeKey::Direct(name.clone())
        }
        Expr::Index { base, index, .. }
            if !owner.param_value_types.contains_key(base)
                && !owner
                    .input_value_types
                    .contains_key(resolve_graph_owner_input_name(owner, base))
                && !owner
                    .output_value_types
                    .contains_key(resolve_graph_owner_output_name(owner, base)) =>
        {
            let context = format!("{owner_context} graph source '{base}[...]'");
            let Some(idx) = eval_graph_nonnegative_int_expr(index, options, &context, errors)
            else {
                return Err(());
            };
            GraphNodeKey::Indexed {
                base: base.clone(),
                index: idx,
            }
        }
        _ => return Ok(None),
    };
    let diag = DiagCtx::new(source.loc());

    let Some(node) = nodes.get(&key) else {
        return Ok(None);
    };
    let Some(surface) = proc_surfaces.get(&node.proc_name) else {
        return Ok(None);
    };

    let output_slots = &surface.api.outs;
    if output_slots.is_empty() {
        push_semantic(
            diag,
            errors,
            format!(
                "{owner_context} graph source '{}' cannot fan out because it has no outputs",
                graph_bundle_source_label(&key)
            ),
        );
        return Err(());
    }
    if output_slots.len() == 1 {
        return Ok(Some(GraphSourceExpansion::Shared {
            expr: graph_bundle_slot_expr(&key, &output_slots[0]),
            use_shared_tmp: false,
        }));
    }
    if output_slots.len() != dest_count {
        push_semantic(
            diag,
            errors,
            format!(
                "{owner_context} graph source '{}' exposes {} output slot(s), but destination set has {} endpoint(s)",
                graph_bundle_source_label(&key),
                output_slots.len(),
                dest_count
            ),
        );
        return Err(());
    }
    Ok(Some(GraphSourceExpansion::PerDest(
        output_slots
            .iter()
            .map(|slot| graph_bundle_slot_expr(&key, slot))
            .collect(),
    )))
}

fn graph_bundle_source_label(key: &GraphNodeKey) -> String {
    match key {
        GraphNodeKey::Direct(name) => name.clone(),
        GraphNodeKey::Indexed { base, index } => format!("{base}[{index}]"),
    }
}

fn graph_bundle_slot_expr(key: &GraphNodeKey, slot: &str) -> Expr {
    match key {
        GraphNodeKey::Direct(name) => Expr::var(format!("{name}.{slot}")),
        GraphNodeKey::Indexed { base, index } => Expr::UserCall {
            loc: Default::default(),
            name: format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"),
            type_args: Vec::new(),
            args: vec![
                CallArg {
                    name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                    expr: Expr::var(base.clone()),
                },
                CallArg {
                    name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                    expr: Expr::int(*index as i64),
                },
                CallArg {
                    name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                    expr: Expr::var(slot.to_owned()),
                },
            ],
        },
    }
}

pub(super) fn resolve_graph_dest(
    dest: &GraphEndpoint,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Result<(GraphDestKind, String, GraphValueType), ()> {
    match dest {
        GraphEndpoint::Symbol { name, .. } => {
            let resolved = resolve_graph_owner_output_name(owner, name);
            if let Some(ty) = owner.output_value_types.get(resolved).cloned() {
                Ok((
                    GraphDestKind::TopOutput(resolved.to_owned()),
                    resolved.to_owned(),
                    ty,
                ))
            } else {
                push_graph_error(
                    errors,
                    dest.loc(),
                    format!("{owner_context} graph destination '{name}' is not a declared output"),
                );
                Err(())
            }
        }
        GraphEndpoint::ProcField { proc, field, .. } => {
            let key = GraphNodeKey::Direct(proc.clone());
            resolve_graph_proc_dest(
                &key,
                field,
                dest.loc(),
                nodes,
                proc_surfaces,
                owner_context,
                errors,
            )
        }
        GraphEndpoint::ProcIndexedField {
            proc, index, field, ..
        } => {
            let context = format!("{owner_context} graph proc-array destination '{proc}[...]'");
            let Some(idx) = eval_graph_nonnegative_int_expr(index, options, &context, errors)
            else {
                return Err(());
            };
            let key = GraphNodeKey::Indexed {
                base: proc.clone(),
                index: idx,
            };
            resolve_graph_proc_dest(
                &key,
                field,
                dest.loc(),
                nodes,
                proc_surfaces,
                owner_context,
                errors,
            )
        }
    }
}

fn resolve_graph_proc_dest(
    key: &GraphNodeKey,
    field: &str,
    loc: SourceLoc,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Result<(GraphDestKind, String, GraphValueType), ()> {
    let Some(node) = nodes.get(key) else {
        push_graph_error(
            errors,
            loc,
            format!(
                "{owner_context} graph destination references unknown node '{}'",
                node_ref_name(key)
            ),
        );
        return Err(());
    };
    let Some(surface) = proc_surfaces.get(&node.proc_name) else {
        return Err(());
    };
    let resolved_out = resolve_graph_proc_output_name(surface, field);
    let resolved_in = resolve_graph_proc_input_name(surface, field);
    if let Some(ty) = surface.param_value_types.get(field).cloned() {
        return Ok((
            GraphDestKind::ProcParam {
                node: key.clone(),
                param: field.to_owned(),
            },
            format!("{}.{}", node_ref_name(key), field),
            ty,
        ));
    }
    if let Some(ty) = surface.in_value_types.get(resolved_in).cloned() {
        return Ok((
            GraphDestKind::ProcInput {
                node: key.clone(),
                port: resolved_in.to_owned(),
            },
            format!("{}.{}", node_ref_name(key), resolved_in),
            ty,
        ));
    }
    if surface.api.outs.iter().any(|out| out == resolved_out) {
        push_graph_error(
            errors,
            loc,
            format!(
                "{owner_context} graph destination '{}' cannot target processor outputs",
                format!("{}.{}", node_ref_name(key), field)
            ),
        );
        return Err(());
    }
    push_graph_error(
        errors,
        loc,
        format!(
            "{owner_context} graph destination '{}' references an unknown endpoint",
            format!("{}.{}", node_ref_name(key), field)
        ),
    );
    Err(())
}

pub(super) fn validate_graph_source_expr(
    expr: &Expr,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    block_safe_only: bool,
    inferred_param_rate: bool,
    deps: &mut BTreeSet<GraphNodeKey>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                validate_graph_source_expr(
                    value,
                    owner,
                    nodes,
                    proc_surfaces,
                    block_safe_only,
                    inferred_param_rate,
                    deps,
                    owner_context,
                    options,
                    errors,
                );
            }
        }
        Expr::Var { name, .. } => {
            validate_graph_source_base(
                name,
                expr.loc(),
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                errors,
            );
        }
        Expr::UserCall { name, args, .. } => {
            if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    });
                let index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG).cloned();
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var { name, .. } => Some(name.clone()),
                            _ => None,
                        },
                    );
                if let (Some(base), Some(index), Some(field)) = (base, index, field) {
                    let context = format!("{owner_context} graph source '{}[...]'", base);
                    if let Some(idx) =
                        eval_graph_nonnegative_int_expr(&index, options, &context, errors)
                    {
                        let key = GraphNodeKey::Indexed { base, index: idx };
                        validate_graph_proc_field_source(
                            &key,
                            &field,
                            expr.loc(),
                            nodes,
                            proc_surfaces,
                            block_safe_only,
                            inferred_param_rate,
                            deps,
                            owner_context,
                            errors,
                        );
                    }
                } else {
                    push_graph_error(
                        errors,
                        expr.loc(),
                        "malformed graph indexed endpoint source",
                    );
                }
                return;
            }
            if name == GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    });
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG).cloned();
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var { name, .. } => Some(name.clone()),
                            _ => None,
                        },
                    );
                let field_index =
                    named_call_arg_expr(args, GRAPH_PROC_FIELD_INDEX_EXPR_ARG).cloned();
                if let (Some(base), Some(proc_index), Some(field), Some(field_index)) =
                    (base, proc_index, field, field_index)
                {
                    let proc_context = format!("{owner_context} graph source '{}[...]'", base);
                    if let Some(proc_idx) =
                        eval_graph_nonnegative_int_expr(&proc_index, options, &proc_context, errors)
                    {
                        let key = GraphNodeKey::Indexed {
                            base,
                            index: proc_idx,
                        };
                        validate_graph_proc_field_source(
                            &key,
                            &field,
                            expr.loc(),
                            nodes,
                            proc_surfaces,
                            block_safe_only,
                            inferred_param_rate,
                            deps,
                            owner_context,
                            errors,
                        );
                        validate_graph_source_expr(
                            &field_index,
                            owner,
                            nodes,
                            proc_surfaces,
                            block_safe_only,
                            inferred_param_rate,
                            deps,
                            owner_context,
                            options,
                            errors,
                        );
                    }
                } else {
                    push_graph_error(
                        errors,
                        expr.loc(),
                        "malformed graph indexed processor-array output source",
                    );
                }
                return;
            }
            push_graph_error(
                errors,
                expr.loc(),
                "graph source expressions do not support user-defined or processor calls",
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_graph_source_expr(
                    arg,
                    owner,
                    nodes,
                    proc_surfaces,
                    block_safe_only,
                    inferred_param_rate,
                    deps,
                    owner_context,
                    options,
                    errors,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_graph_source_expr(
                lhs,
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                options,
                errors,
            );
            validate_graph_source_expr(
                rhs,
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                options,
                errors,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_graph_source_expr(
                expr,
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                options,
                errors,
            )
        }
        Expr::Index { base, index, .. } => {
            validate_graph_source_base(
                base,
                expr.loc(),
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                errors,
            );
            validate_graph_source_expr(
                index,
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                options,
                errors,
            );
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            validate_graph_source_base(
                base,
                expr.loc(),
                owner,
                nodes,
                proc_surfaces,
                block_safe_only,
                inferred_param_rate,
                deps,
                owner_context,
                errors,
            );
            if let Some(GraphValueType::Array { len, .. }) =
                infer_graph_source_base_value_type(base, owner, nodes, proc_surfaces)
            {
                let _ = eval_graph_static_slice_bounds(
                    len,
                    start.as_deref(),
                    end.as_deref(),
                    options,
                    &format!("{owner_context} graph slice '{base}'"),
                    errors,
                );
            }
        }
        Expr::ArrayCtor { .. } => {
            push_graph_error(
                errors,
                expr.loc(),
                "constructor graph sources are not yet supported",
            );
        }
    }
}

fn validate_graph_source_base(
    base: &str,
    loc: SourceLoc,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    block_safe_only: bool,
    inferred_param_rate: bool,
    deps: &mut BTreeSet<GraphNodeKey>,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let resolved_input = resolve_graph_owner_input_name(owner, base);
    let resolved_output = resolve_graph_owner_output_name(owner, base);
    if owner.param_value_types.contains_key(base) {
        return;
    }
    if owner.input_value_types.contains_key(resolved_input) {
        if block_safe_only {
            graph_block_source_error(
                format!("{owner_context} graph @block edge cannot read sample-rate input '{base}'"),
                inferred_param_rate,
                loc,
                errors,
            );
        }
        return;
    }
    if owner.output_value_types.contains_key(resolved_output) {
        push_graph_error(
            errors,
            loc,
            format!("{owner_context} graph source cannot read output '{base}'"),
        );
        return;
    }
    if let Some((node_base, field)) = base.rsplit_once('.') {
        let key = GraphNodeKey::Direct(node_base.to_owned());
        validate_graph_proc_field_source(
            &key,
            field,
            loc,
            nodes,
            proc_surfaces,
            block_safe_only,
            inferred_param_rate,
            deps,
            owner_context,
            errors,
        );
        return;
    }
    push_graph_error(
        errors,
        loc,
        format!("{owner_context} graph source references unknown symbol '{base}'"),
    );
}

pub(super) fn infer_graph_source_value_type(
    expr: &Expr,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<GraphValueType> {
    match expr {
        Expr::Number { .. } => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Expr::Int { value, .. } => Some(GraphValueType::Scalar(
            if *value >= i32::MIN as i64 && *value <= i32::MAX as i64 {
                PrimitiveType::I32
            } else {
                PrimitiveType::I64
            },
        )),
        Expr::Bool { .. } => Some(GraphValueType::Scalar(PrimitiveType::Bool)),
        Expr::ArrayLiteral { values, .. } => {
            let mut elem_ty = None::<PrimitiveType>;
            for value in values {
                let Some(GraphValueType::Scalar(value_ty)) = infer_graph_source_value_type(
                    value,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ) else {
                    push_graph_error(
                        errors,
                        value.loc(),
                        format!("{owner_context} graph array literal elements must be scalar"),
                    );
                    return None;
                };
                elem_ty = Some(match elem_ty {
                    None => value_ty,
                    Some(prev) if prev == value_ty => prev,
                    Some(prev) if can_implicitly_assign(value_ty, prev) => prev,
                    Some(prev) if can_implicitly_assign(prev, value_ty) => value_ty,
                    Some(prev) => {
                        push_graph_error(
                            errors,
                            value.loc(),
                            format!(
                                "{owner_context} graph array literal mixes incompatible element types {:?} and {:?}",
                                prev, value_ty
                            ),
                        );
                        return None;
                    }
                });
            }
            Some(GraphValueType::Array {
                elem_ty: elem_ty.unwrap_or(PrimitiveType::F32),
                len: values.len(),
            })
        }
        Expr::Var { name, .. } => {
            if let Some(ty) = builtin_constant_type(name) {
                return Some(GraphValueType::Scalar(ty));
            }
            if let Some(ty) = owner.param_value_types.get(name).cloned() {
                return Some(ty);
            }
            let resolved_input = resolve_graph_owner_input_name(owner, name);
            if let Some(ty) = owner.input_value_types.get(resolved_input).cloned() {
                return Some(ty);
            }
            if let Some((base, field)) = name.rsplit_once('.') {
                return infer_graph_proc_field_value_type(
                    &GraphNodeKey::Direct(base.to_owned()),
                    field,
                    nodes,
                    proc_surfaces,
                );
            }
            None
        }
        Expr::Index { base, index, .. } => {
            let _ = infer_graph_source_value_type(
                index,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            );
            let base_ty = if let Some((node_base, field)) = base.rsplit_once('.') {
                infer_graph_proc_field_value_type(
                    &GraphNodeKey::Direct(node_base.to_owned()),
                    field,
                    nodes,
                    proc_surfaces,
                )
            } else {
                let resolved_input = resolve_graph_owner_input_name(owner, base);
                owner
                    .param_value_types
                    .get(base)
                    .cloned()
                    .or_else(|| owner.input_value_types.get(resolved_input).cloned())
            };
            match base_ty {
                Some(GraphValueType::Array { elem_ty, .. }) => {
                    Some(GraphValueType::Scalar(elem_ty))
                }
                Some(GraphValueType::Scalar(_)) => None,
                None => None,
            }
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            let base_ty = infer_graph_source_base_value_type(base, owner, nodes, proc_surfaces)?;
            match base_ty {
                GraphValueType::Array { elem_ty, len } => {
                    let (start_idx, end_idx) = eval_graph_static_slice_bounds(
                        len,
                        start.as_deref(),
                        end.as_deref(),
                        options,
                        &format!("{owner_context} graph slice '{base}'"),
                        errors,
                    )?;
                    Some(GraphValueType::Array {
                        elem_ty,
                        len: end_idx - start_idx,
                    })
                }
                GraphValueType::Scalar(_) => {
                    push_graph_error(
                        errors,
                        expr.loc(),
                        format!("{owner_context} graph slice '{base}' requires an array source"),
                    );
                    None
                }
            }
        }
        Expr::UserCall { name, args, .. } => {
            if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    })?;
                let index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG)?;
                let _ = infer_graph_source_value_type(
                    index,
                    owner,
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                );
                let field = named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                    |expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    },
                )?;
                let idx = eval_graph_nonnegative_int_expr(
                    index,
                    options,
                    &format!("{owner_context} graph source '{}[...]'", base),
                    errors,
                )?;
                return infer_graph_proc_field_value_type(
                    &GraphNodeKey::Indexed { base, index: idx },
                    &field,
                    nodes,
                    proc_surfaces,
                );
            }
            if name == GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    })?;
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG)?;
                let field = named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                    |expr| match expr {
                        Expr::Var { name, .. } => Some(name.clone()),
                        _ => None,
                    },
                )?;
                let field_index = named_call_arg_expr(args, GRAPH_PROC_FIELD_INDEX_EXPR_ARG)?;
                let proc_idx = eval_graph_nonnegative_int_expr(
                    proc_index,
                    options,
                    &format!("{owner_context} graph source '{}[...]'", base),
                    errors,
                )?;
                let field_idx = eval_graph_nonnegative_int_expr(
                    field_index,
                    options,
                    &format!("{owner_context} graph source '{}.{}[...]'", base, field),
                    errors,
                )?;
                let ty = infer_graph_proc_field_value_type(
                    &GraphNodeKey::Indexed {
                        base,
                        index: proc_idx,
                    },
                    &field,
                    nodes,
                    proc_surfaces,
                )?;
                return match ty {
                    GraphValueType::Array { elem_ty, len } if field_idx < len => {
                        Some(GraphValueType::Scalar(elem_ty))
                    }
                    _ => None,
                };
            }
            if args.iter().any(|arg| {
                !matches!(
                    infer_graph_source_value_type(
                        &arg.expr,
                        owner,
                        nodes,
                        proc_surfaces,
                        owner_context,
                        options,
                        errors,
                    ),
                    Some(GraphValueType::Scalar(_))
                )
            }) {
                return None;
            }
            Some(GraphValueType::Scalar(PrimitiveType::F32))
        }
        Expr::Call { args, .. } => {
            if args.iter().any(|arg| {
                !matches!(
                    infer_graph_source_value_type(
                        arg,
                        owner,
                        nodes,
                        proc_surfaces,
                        owner_context,
                        options,
                        errors,
                    ),
                    Some(GraphValueType::Scalar(_))
                )
            }) {
                return None;
            }
            Some(GraphValueType::Scalar(PrimitiveType::F32))
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Logical { lhs, rhs, .. } => {
            let lhs_ty = infer_graph_source_value_type(
                lhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            );
            let rhs_ty = infer_graph_source_value_type(
                rhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            );
            if matches!(lhs_ty, Some(GraphValueType::Scalar(_)))
                && matches!(rhs_ty, Some(GraphValueType::Scalar(_)))
            {
                Some(GraphValueType::Scalar(PrimitiveType::Bool))
            } else {
                None
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            let lhs_ty = infer_graph_source_value_type(
                lhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            );
            let rhs_ty = infer_graph_source_value_type(
                rhs,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            );
            match (lhs_ty, rhs_ty) {
                (Some(GraphValueType::Scalar(lhs_ty)), Some(GraphValueType::Scalar(rhs_ty))) => {
                    Some(GraphValueType::Scalar(
                        if can_implicitly_assign(lhs_ty, rhs_ty) {
                            rhs_ty
                        } else if can_implicitly_assign(rhs_ty, lhs_ty) {
                            lhs_ty
                        } else {
                            lhs_ty
                        },
                    ))
                }
                (
                    Some(GraphValueType::Array {
                        elem_ty: lhs_elem_ty,
                        len: lhs_len,
                    }),
                    Some(GraphValueType::Array {
                        elem_ty: rhs_elem_ty,
                        len: rhs_len,
                    }),
                ) => {
                    if lhs_len != rhs_len {
                        push_graph_error(
                            errors,
                            expr.loc(),
                            format!(
                                "{owner_context} graph expression shape mismatch: cannot combine {:?}[{lhs_len}] and {:?}[{rhs_len}]",
                                lhs_elem_ty, rhs_elem_ty
                            ),
                        );
                        None
                    } else {
                        Some(GraphValueType::Array {
                            elem_ty: if can_implicitly_assign(lhs_elem_ty, rhs_elem_ty) {
                                rhs_elem_ty
                            } else if can_implicitly_assign(rhs_elem_ty, lhs_elem_ty) {
                                lhs_elem_ty
                            } else {
                                lhs_elem_ty
                            },
                            len: lhs_len,
                        })
                    }
                }
                (
                    Some(GraphValueType::Array { elem_ty, len }),
                    Some(GraphValueType::Scalar(rhs_ty)),
                ) => Some(GraphValueType::Array {
                    elem_ty: if can_implicitly_assign(elem_ty, rhs_ty) {
                        rhs_ty
                    } else {
                        elem_ty
                    },
                    len,
                }),
                (
                    Some(GraphValueType::Scalar(lhs_ty)),
                    Some(GraphValueType::Array { elem_ty, len }),
                ) => Some(GraphValueType::Array {
                    elem_ty: if can_implicitly_assign(elem_ty, lhs_ty) {
                        lhs_ty
                    } else {
                        elem_ty
                    },
                    len,
                }),
                _ => None,
            }
        }
        Expr::Cast { to, .. } => Some(GraphValueType::Scalar(*to)),
        Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            match infer_graph_source_value_type(
                expr,
                owner,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            ) {
                Some(GraphValueType::Scalar(ty)) => Some(GraphValueType::Scalar(ty)),
                _ => None,
            }
        }
        Expr::ArrayCtor { .. } => None,
        Expr::Tuple { .. } => None,
    }
}

pub(super) fn infer_graph_proc_field_value_type(
    key: &GraphNodeKey,
    field: &str,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
) -> Option<GraphValueType> {
    let node = nodes.get(key)?;
    let surface = proc_surfaces.get(&node.proc_name)?;
    let resolved_out = resolve_graph_proc_output_name(surface, field);
    let resolved_in = resolve_graph_proc_input_name(surface, field);
    surface
        .out_value_types
        .get(resolved_out)
        .cloned()
        .or_else(|| surface.param_value_types.get(field).cloned())
        .or_else(|| surface.in_value_types.get(resolved_in).cloned())
        .or_else(|| {
            surface.out_array_slots.iter().find_map(|(base, slots)| {
                if slots.iter().any(|slot| slot == resolved_out) {
                    match surface.out_value_types.get(base) {
                        Some(GraphValueType::Array { elem_ty, .. }) => {
                            Some(GraphValueType::Scalar(*elem_ty))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            })
        })
        .or_else(|| {
            surface.param_array_slots.iter().find_map(|(base, slots)| {
                if slots.iter().any(|slot| slot == field) {
                    match surface.param_value_types.get(base) {
                        Some(GraphValueType::Array { elem_ty, .. }) => {
                            Some(GraphValueType::Scalar(*elem_ty))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            })
        })
}

pub(super) fn expand_graph_expr_to_slots(
    expr: &Expr,
    slot_count: usize,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Expr> {
    if slot_count == 0 {
        return Vec::new();
    }
    if slot_count == 1 {
        return vec![expr.clone()];
    }
    if let Expr::ArrayLiteral { values, .. } = expr {
        if values.len() != slot_count {
            push_graph_error(
                errors,
                expr.loc(),
                format!(
                    "{context}: expected array expression with {slot_count} elements, got {}",
                    values.len()
                ),
            );
            return vec![expr.clone(); slot_count];
        }
        return values.clone();
    }

    match infer_graph_source_value_type(expr, owner, nodes, proc_surfaces, context, options, errors)
    {
        Some(GraphValueType::Scalar(_)) => return vec![expr.clone(); slot_count],
        Some(GraphValueType::Array { len, .. }) if len != slot_count => {
            push_graph_error(
                errors,
                expr.loc(),
                format!(
                    "{context}: expected array expression with {slot_count} elements, got {len}"
                ),
            );
        }
        Some(GraphValueType::Array { .. }) => {}
        None => {
            push_graph_error(
                errors,
                expr.loc(),
                format!("{context}: array expression could not be expanded element-wise"),
            );
            return vec![expr.clone(); slot_count];
        }
    }

    match expr {
        Expr::ArrayLiteral { values, .. } => (0..slot_count)
            .map(|i| values.get(i).cloned().unwrap_or(Expr::number(0.0)))
            .collect(),
        Expr::Var { name: base, .. } => (0..slot_count)
            .map(|i| Expr::Index {
                loc: Default::default(),
                base: base.clone(),
                index: Box::new(Expr::int(i as i64)),
            })
            .collect(),
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs_slots = expand_graph_expr_to_slots(
                lhs,
                slot_count,
                owner,
                nodes,
                proc_surfaces,
                context,
                options,
                errors,
            );
            let rhs_slots = expand_graph_expr_to_slots(
                rhs,
                slot_count,
                owner,
                nodes,
                proc_surfaces,
                context,
                options,
                errors,
            );
            lhs_slots
                .into_iter()
                .zip(rhs_slots)
                .map(|(lhs, rhs)| Expr::Binary {
                    loc: Default::default(),
                    op: *op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                })
                .collect()
        }
        Expr::Cast { to, expr, .. } => expand_graph_expr_to_slots(
            expr,
            slot_count,
            owner,
            nodes,
            proc_surfaces,
            context,
            options,
            errors,
        )
        .into_iter()
        .map(|expr| Expr::Cast {
            loc: Default::default(),
            to: *to,
            expr: Box::new(expr),
        })
        .collect(),
        Expr::UnaryNot { expr, .. } => expand_graph_expr_to_slots(
            expr,
            slot_count,
            owner,
            nodes,
            proc_surfaces,
            context,
            options,
            errors,
        )
        .into_iter()
        .map(|expr| Expr::UnaryNot {
            loc: Default::default(),
            expr: Box::new(expr),
        })
        .collect(),
        Expr::UnaryBitNot { expr, .. } => expand_graph_expr_to_slots(
            expr,
            slot_count,
            owner,
            nodes,
            proc_surfaces,
            context,
            options,
            errors,
        )
        .into_iter()
        .map(|expr| Expr::UnaryBitNot {
            loc: Default::default(),
            expr: Box::new(expr),
        })
        .collect(),
        Expr::Slice {
            base, start, end, ..
        } => {
            let Some(GraphValueType::Array { len, .. }) =
                infer_graph_source_base_value_type(base, owner, nodes, proc_surfaces)
            else {
                push_graph_error(
                    errors,
                    expr.loc(),
                    format!("{context}: sliced graph source '{base}' requires an array base"),
                );
                return vec![expr.clone(); slot_count];
            };
            let Some((start_idx, _)) = eval_graph_static_slice_bounds(
                len,
                start.as_deref(),
                end.as_deref(),
                options,
                &format!("{context} slice '{base}'"),
                errors,
            ) else {
                return vec![expr.clone(); slot_count];
            };
            (0..slot_count)
                .map(|i| Expr::Index {
                    loc: Default::default(),
                    base: base.clone(),
                    index: Box::new(Expr::int((start_idx + i) as i64)),
                })
                .collect()
        }
        _ => {
            push_graph_error(
                errors,
                expr.loc(),
                format!(
                    "{context}: array expression requires array literals, array symbols, slices, or element-wise expressions"
                ),
            );
            vec![expr.clone(); slot_count]
        }
    }
}

pub(super) fn require_graph_assignable_type(
    src: &GraphValueType,
    dst: &GraphValueType,
    loc: SourceLoc,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let mut push_scalar_mismatch = |src_ty: PrimitiveType, dst_ty: PrimitiveType| {
        if src_ty != dst_ty && !can_implicitly_assign(src_ty, dst_ty) {
            push_graph_error(
                errors,
                loc,
                format!(
                    "{context} type mismatch: cannot assign {:?} to {:?}",
                    src_ty, dst_ty
                ),
            );
        }
    };
    match (src, dst) {
        (GraphValueType::Scalar(src_ty), GraphValueType::Scalar(dst_ty)) => {
            push_scalar_mismatch(*src_ty, *dst_ty);
        }
        (
            GraphValueType::Scalar(src_ty),
            GraphValueType::Array {
                elem_ty: dst_elem, ..
            },
        ) => {
            push_scalar_mismatch(*src_ty, *dst_elem);
        }
        (
            GraphValueType::Array {
                elem_ty: src_elem,
                len: src_len,
            },
            GraphValueType::Array {
                elem_ty: dst_elem,
                len: dst_len,
            },
        ) => {
            if src_len != dst_len {
                push_graph_error(
                    errors,
                    loc,
                    format!(
                        "{context} shape mismatch: cannot assign {} to {}",
                        graph_value_type_label(src),
                        graph_value_type_label(dst)
                    ),
                );
            } else {
                push_scalar_mismatch(*src_elem, *dst_elem);
            }
        }
        _ => push_graph_error(
            errors,
            loc,
            format!(
                "{context} shape mismatch: cannot assign {} to {}",
                graph_value_type_label(src),
                graph_value_type_label(dst)
            ),
        ),
    }
}

fn validate_graph_proc_field_source(
    key: &GraphNodeKey,
    field: &str,
    loc: SourceLoc,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    block_safe_only: bool,
    inferred_param_rate: bool,
    deps: &mut BTreeSet<GraphNodeKey>,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(node) = nodes.get(key) else {
        push_graph_error(
            errors,
            loc,
            format!(
                "{owner_context} graph source references unknown node '{}'",
                node_ref_name(key)
            ),
        );
        return;
    };
    let Some(surface) = proc_surfaces.get(&node.proc_name) else {
        return;
    };
    let resolved_out = resolve_graph_proc_output_name(surface, field);
    let resolved_in = resolve_graph_proc_input_name(surface, field);
    if surface.api.outs.iter().any(|out| out == resolved_out) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read sample-rate processor output '{}.{}'",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
                loc,
                errors,
            );
        } else {
            deps.insert(key.clone());
        }
        return;
    }
    if surface.out_array_slots.contains_key(resolved_out) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read sample-rate processor output '{}.{}'",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
                loc,
                errors,
            );
        } else {
            deps.insert(key.clone());
        }
        return;
    }
    if surface.api.params.contains_key(field) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read processor param '{}.{}' in MVP",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
                loc,
                errors,
            );
        }
        return;
    }
    if surface.api.ins.iter().any(|port| port.name == resolved_in) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read processor input '{}.{}'",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
                loc,
                errors,
            );
        }
        return;
    }
    push_graph_error(
        errors,
        loc,
        format!(
            "{owner_context} graph source '{}' references an unknown endpoint",
            format!("{}.{}", node_ref_name(key), field)
        ),
    );
}
