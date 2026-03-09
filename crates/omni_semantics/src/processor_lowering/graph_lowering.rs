use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::*;

const GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL: &str = "__omni_graph_proc_array_field_index";
const GRAPH_PROC_FIELD_INDEX_EXPR_ARG: &str = "__proc_field_index_expr";

#[derive(Debug, Clone)]
enum GraphValueType {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: usize },
}

#[derive(Debug, Clone)]
struct GraphProcSurface {
    proc_name: String,
    api: ProcApi,
    in_value_types: HashMap<String, GraphValueType>,
    param_value_types: HashMap<String, GraphValueType>,
    out_value_types: HashMap<String, GraphValueType>,
    param_array_slots: HashMap<String, Vec<String>>,
    out_array_slots: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
struct GraphOwnerSurface {
    input_value_types: HashMap<String, GraphValueType>,
    param_value_types: HashMap<String, GraphValueType>,
    output_value_types: HashMap<String, GraphValueType>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum GraphNodeKey {
    Direct(String),
    Indexed { base: String, index: usize },
}

#[derive(Debug, Clone)]
struct GraphNodeInfo {
    proc_name: String,
}

#[derive(Debug, Clone)]
enum GraphDestKind {
    TopOutput(String),
    ProcInput { node: GraphNodeKey, port: String },
    ProcParam { node: GraphNodeKey, param: String },
}

#[derive(Debug, Clone)]
struct GraphDelayState {
    buf_name: String,
    head_name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
}

#[derive(Debug, Clone)]
struct ResolvedGraphEdge {
    original_source: Expr,
    source: Expr,
    rate: GraphRate,
    delay: Option<usize>,
    dest: GraphDestKind,
    dest_value_ty: GraphValueType,
    deps: BTreeSet<GraphNodeKey>,
    delay_state: Option<GraphDelayState>,
}

#[derive(Debug, Clone)]
struct LoweredGraph {
    init_stmts: Vec<Stmt>,
    block_pre: Vec<Stmt>,
    sample: Vec<Stmt>,
}

pub(super) fn lower_graph_blocks(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let proc_surfaces = build_graph_proc_surfaces(program, options, errors);
    lower_proc_graph_blocks(program, &proc_surfaces, options, errors);
    lower_top_level_graph_block(program, &proc_surfaces, options, errors);
}

fn build_graph_proc_surfaces(
    program: &Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, GraphProcSurface> {
    let mut out = HashMap::<String, GraphProcSurface>::new();
    for proc in program.blocks.iter().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        let (_ins, _in_types, in_ports, _) =
            expand_proc_port_specs(&proc.name, &proc.ins, "ins", options, errors);
        let (outs, _, _, out_array_slots) =
            expand_proc_port_specs(&proc.name, &proc.outs, "outs", options, errors);
        let (param_specs, param_array_slots) =
            expand_proc_param_specs(&proc.name, &proc.params, options, errors);
        let params = param_specs
            .iter()
            .flat_map(|spec| spec.slots.iter().cloned())
            .map(|slot| (slot.name.clone(), slot))
            .collect::<HashMap<_, _>>();
        out.insert(
            proc.name.clone(),
            GraphProcSurface {
                proc_name: proc.name.clone(),
                api: ProcApi {
                    ins: in_ports,
                    params,
                    outs: outs.clone(),
                    events: HashMap::new(),
                    buffers: Vec::new(),
                    has_block: false,
                    sample_oversample_factor: 1,
                },
                in_value_types: value_types_from_ports(
                    &proc.ins, options, errors, &proc.name, "input",
                ),
                param_value_types: value_types_from_params(
                    &proc.params,
                    options,
                    errors,
                    &proc.name,
                ),
                out_value_types: value_types_from_ports(
                    &proc.outs, options, errors, &proc.name, "output",
                ),
                param_array_slots,
                out_array_slots,
            },
        );
    }
    out
}

fn graph_owner_surface_from_program(
    program: &Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> GraphOwnerSurface {
    GraphOwnerSurface {
        input_value_types: match program.block(BlockKind::Ins) {
            Some(Block::Ins(ports)) => {
                value_types_from_ports(ports, options, errors, "top-level", "input")
            }
            _ => HashMap::new(),
        },
        param_value_types: match program.block(BlockKind::Params) {
            Some(Block::Params(params)) => {
                value_types_from_params(params, options, errors, "top-level")
            }
            _ => HashMap::new(),
        },
        output_value_types: match program.block(BlockKind::Outs) {
            Some(Block::Outs(ports)) => {
                value_types_from_ports(ports, options, errors, "top-level", "output")
            }
            _ => HashMap::new(),
        },
    }
}

fn graph_owner_surface_from_proc(
    proc: &ProcessorDef,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> GraphOwnerSurface {
    GraphOwnerSurface {
        input_value_types: value_types_from_ports(&proc.ins, options, errors, &proc.name, "input"),
        param_value_types: value_types_from_params(&proc.params, options, errors, &proc.name),
        output_value_types: value_types_from_ports(
            &proc.outs, options, errors, &proc.name, "output",
        ),
    }
}

fn value_type_from_decl_type(
    ty: Option<&DeclType>,
    options: AnalysisOptions,
    size_context: Option<(&Expr, String)>,
    errors: &mut Vec<Diagnostic>,
) -> Option<GraphValueType> {
    match ty {
        None => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Some(DeclType::Scalar(ty)) => Some(GraphValueType::Scalar(*ty)),
        Some(DeclType::Generic(_)) => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Some(DeclType::Array { elem, .. }) => {
            let Some((size_expr, context)) = size_context else {
                return None;
            };
            let len = eval_data_size_expr(size_expr, options, &context, errors)?;
            Some(GraphValueType::Array {
                elem_ty: *elem,
                len,
            })
        }
        Some(DeclType::ArrayGeneric { .. }) => {
            let Some((size_expr, context)) = size_context else {
                return None;
            };
            let len = eval_data_size_expr(size_expr, options, &context, errors)?;
            Some(GraphValueType::Array {
                elem_ty: PrimitiveType::F32,
                len,
            })
        }
    }
}

fn value_types_from_ports(
    ports: &[PortDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    owner_context: &str,
    kind: &str,
) -> HashMap<String, GraphValueType> {
    let mut out = HashMap::<String, GraphValueType>::new();
    for port in ports {
        let ty = match port.ty.as_ref() {
            Some(DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. }) => {
                value_type_from_decl_type(
                    port.ty.as_ref(),
                    options,
                    Some((
                        size,
                        format!("{owner_context} graph {kind} '{}' size", port.name),
                    )),
                    errors,
                )
            }
            _ => value_type_from_decl_type(port.ty.as_ref(), options, None, errors),
        };
        if let Some(ty) = ty {
            out.insert(port.name.clone(), ty);
        }
    }
    out
}

fn value_types_from_params(
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    owner_context: &str,
) -> HashMap<String, GraphValueType> {
    let mut out = HashMap::<String, GraphValueType>::new();
    for param in params {
        let ty = match param.ty.as_ref() {
            Some(DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. }) => {
                value_type_from_decl_type(
                    param.ty.as_ref(),
                    options,
                    Some((
                        size,
                        format!("{owner_context} graph param '{}' size", param.name),
                    )),
                    errors,
                )
            }
            _ => value_type_from_decl_type(param.ty.as_ref(), options, None, errors),
        };
        if let Some(ty) = ty {
            out.insert(param.name.clone(), ty);
        }
    }
    out
}

fn graph_value_type_label(ty: &GraphValueType) -> String {
    match ty {
        GraphValueType::Scalar(prim) => format!("{prim:?}"),
        GraphValueType::Array { elem_ty, len } => format!("{elem_ty:?}[{len}]"),
    }
}

fn eval_graph_nonnegative_int_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        errors.push(Diagnostic::semantic(
            format!("{context} must be finite"),
            0,
            0,
        ));
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        errors.push(Diagnostic::semantic(
            format!("{context} must be an integer"),
            0,
            0,
        ));
        return None;
    }
    if rounded < 0.0 {
        errors.push(Diagnostic::semantic(
            format!("{context} must be greater than or equal to zero"),
            0,
            0,
        ));
        return None;
    }
    Some(rounded as usize)
}

fn eval_graph_static_slice_bound(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let Some(expr) = expr else {
        return Some(if default_to_len { total_len } else { 0 });
    };
    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        errors.push(Diagnostic::semantic(
            format!("{context} must be finite"),
            0,
            0,
        ));
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        errors.push(Diagnostic::semantic(
            format!("{context} must be an integer"),
            0,
            0,
        ));
        return None;
    }
    let raw = rounded as i64;
    let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
    Some(adjusted.clamp(0, total_len as i64) as usize)
}

fn eval_graph_static_slice_bounds(
    total_len: usize,
    start: Option<&Expr>,
    end: Option<&Expr>,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(usize, usize)> {
    let start_idx = eval_graph_static_slice_bound(
        start,
        total_len,
        false,
        options,
        &format!("{context} slice start"),
        errors,
    )?;
    let end_idx = eval_graph_static_slice_bound(
        end,
        total_len,
        true,
        options,
        &format!("{context} slice end"),
        errors,
    )?;
    if end_idx <= start_idx {
        errors.push(Diagnostic::semantic(
            format!("{context} slice must have positive length"),
            0,
            0,
        ));
        return None;
    }
    Some((start_idx, end_idx))
}

fn graph_block_source_error(
    detail: String,
    inferred_param_rate: bool,
    errors: &mut Vec<Diagnostic>,
) {
    let message = if inferred_param_rate {
        format!("{detail}; add @sample to this param edge if sample-rate modulation is intended")
    } else {
        detail
    };
    errors.push(Diagnostic::semantic(message, 0, 0));
}

fn infer_graph_source_base_value_type(
    base: &str,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
) -> Option<GraphValueType> {
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

fn node_ref_name(node: &GraphNodeKey) -> String {
    match node {
        GraphNodeKey::Direct(name) => name.clone(),
        GraphNodeKey::Indexed { base, index } => format!("{base}[{index}]"),
    }
}

fn assign_stmt(target: String, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: None,
        target: AssignTarget::Var(target),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        expr,
    }
}

fn graph_delay_flat_index_expr(head_name: &str, array_len: usize, slot: usize) -> Expr {
    if array_len <= 1 {
        return Expr::Var(head_name.to_owned());
    }
    let base = Expr::Binary {
        op: BinaryOp::Mul,
        lhs: Box::new(Expr::Var(head_name.to_owned())),
        rhs: Box::new(Expr::Int(array_len as i64)),
    };
    if slot == 0 {
        base
    } else {
        Expr::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(base),
            rhs: Box::new(Expr::Int(slot as i64)),
        }
    }
}

fn call_stmt(expr: Expr) -> Stmt {
    Stmt::Expr { loc: None, expr }
}

fn assign_node_field_stmt(node: &GraphNodeKey, field: &str, expr: Expr) -> Stmt {
    match node {
        GraphNodeKey::Direct(name) => assign_stmt(format!("{name}.{field}"), expr),
        GraphNodeKey::Indexed { base, index } => Stmt::Assign {
            loc: None,
            target: AssignTarget::Index {
                base: format!("{base}.{field}"),
                index: Expr::Int(*index as i64),
            },
            decl_ty: None,
            generic_decl_ty: None,
            is_typed_decl: false,
            expr,
        },
    }
}

fn assign_node_array_field_stmts(
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

fn named_call_arg_expr<'a>(args: &'a [CallArg], arg_name: &str) -> Option<&'a Expr> {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(arg_name))
        .map(|arg| &arg.expr)
}

fn lower_proc_graph_blocks(
    program: &mut Program,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &mut program.blocks {
        let Block::Proc(proc) = block else {
            continue;
        };
        let Some(graph) = proc.graph.clone() else {
            continue;
        };
        if proc.has_sample_block || proc.has_block_block {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' graph block cannot be declared with sample or block",
                    proc.name
                ),
                0,
                0,
            ));
            continue;
        }

        let owner = graph_owner_surface_from_proc(proc, options, errors);
        let nodes = collect_graph_nodes_from_init(
            &proc.init.body,
            proc_surfaces,
            options,
            &format!("processor '{}'", proc.name),
            errors,
        );
        let lowered = lower_graph(
            &graph,
            &owner,
            &nodes,
            proc_surfaces,
            &format!("processor '{}'", proc.name),
            options,
            errors,
        );
        proc.init.body.extend(lowered.init_stmts);
        proc.block_pre = lowered.block_pre;
        proc.sample = lowered.sample;
        proc.block_post.clear();
        proc.has_block_block = !proc.block_pre.is_empty();
        proc.has_sample_block = true;
        proc.has_graph_block = false;
        proc.graph = None;
    }
}

fn lower_top_level_graph_block(
    program: &mut Program,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let graph_indices = program
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(idx, block)| match block {
            Block::Graph(_) => Some(idx),
            _ => None,
        })
        .collect::<Vec<_>>();
    if graph_indices.is_empty() {
        return;
    }
    if graph_indices.len() > 1 {
        errors.push(Diagnostic::semantic("duplicate block 'graph'", 0, 0));
        return;
    }
    if program.block(BlockKind::Sample).is_some() {
        errors.push(Diagnostic::semantic(
            "graph block cannot be declared with sample block",
            0,
            0,
        ));
        return;
    }
    if program.block(BlockKind::Block).is_some() {
        errors.push(Diagnostic::semantic(
            "graph block cannot be declared with block section",
            0,
            0,
        ));
        return;
    }

    let graph_idx = graph_indices[0];
    let graph = match program.blocks.remove(graph_idx) {
        Block::Graph(graph) => graph,
        _ => return,
    };
    let init_body = match program.block(BlockKind::Init) {
        Some(Block::Init(init)) => init.body.clone(),
        _ => Vec::new(),
    };
    let owner = graph_owner_surface_from_program(program, options, errors);
    let nodes =
        collect_graph_nodes_from_init(&init_body, proc_surfaces, options, "top-level", errors);
    let lowered = lower_graph(
        &graph,
        &owner,
        &nodes,
        proc_surfaces,
        "top-level",
        options,
        errors,
    );

    if !lowered.init_stmts.is_empty() {
        if let Some(Block::Init(init)) = program
            .blocks
            .iter_mut()
            .find(|block| matches!(block, Block::Init(_)))
        {
            init.body.extend(lowered.init_stmts);
        } else {
            program.blocks.push(Block::Init(InitBlock {
                default_ty: None,
                body: lowered.init_stmts,
            }));
        }
    }

    let sample_block = SampleBlock {
        oversample_factor: None,
        body: lowered.sample,
    };
    if lowered.block_pre.is_empty() {
        program.blocks.push(Block::Sample(sample_block));
    } else {
        program.blocks.push(Block::Block(BlockExec {
            pre: lowered.block_pre,
            sample: Some(sample_block),
            post: Vec::new(),
        }));
    }
}

fn collect_graph_nodes_from_init(
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

fn lower_graph(
    graph: &GraphBlock,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> LoweredGraph {
    let mut resolved = Vec::<ResolvedGraphEdge>::new();
    let mut driven_outputs = HashSet::<String>::new();
    let mut single_writer = HashSet::<String>::new();
    let mut delayed_edge_counter = 0usize;

    for edge in &graph.edges {
        let Ok((dest, dest_key, dest_value_ty)) = resolve_graph_dest(
            &edge.dest,
            owner,
            nodes,
            proc_surfaces,
            owner_context,
            options,
            errors,
        ) else {
            continue;
        };

        if !single_writer.insert(dest_key.clone()) {
            errors.push(Diagnostic::semantic(
                format!("graph destination '{dest_key}' has more than one driver"),
                0,
                0,
            ));
        }
        if let GraphDestKind::TopOutput(name) = &dest {
            driven_outputs.insert(name.clone());
        }

        let rate = match edge.rate {
            Some(rate) => rate,
            None => match dest {
                GraphDestKind::ProcParam { .. } => GraphRate::Block,
                _ => GraphRate::Sample,
            },
        };
        let delay = edge.delay.as_ref().and_then(|expr| {
            let context = format!("{owner_context} graph edge delay");
            eval_graph_nonnegative_int_expr(expr, options, &context, errors)
        });
        if delay.is_some() && rate != GraphRate::Sample {
            errors.push(Diagnostic::semantic(
                "delayed graph edges are only supported for sample-rate destinations",
                0,
                0,
            ));
        }

        let mut deps = BTreeSet::<GraphNodeKey>::new();
        let inferred_param_rate =
            edge.rate.is_none() && matches!(dest, GraphDestKind::ProcParam { .. });
        validate_graph_source_expr(
            &edge.source,
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
        if let Some(src_value_ty) = infer_graph_source_value_type(
            &edge.source,
            owner,
            nodes,
            proc_surfaces,
            owner_context,
            options,
            errors,
        ) {
            require_graph_assignable_type(
                &src_value_ty,
                &dest_value_ty,
                &format!("{owner_context} graph edge source for destination '{dest_key}'"),
                errors,
            );
        }

        let lowered_source = rewrite_graph_source_expr(
            &edge.source,
            nodes,
            proc_surfaces,
            owner_context,
            options,
            errors,
        );
        let mut source = lowered_source.clone();
        let delay_state = if delay.filter(|len| *len > 0).is_some() {
            let (dest_ty, array_len) = match &dest_value_ty {
                GraphValueType::Scalar(dest_ty) => (*dest_ty, 1usize),
                GraphValueType::Array { elem_ty, len } => (*elem_ty, *len),
            };
            let buf_name = format!("__graph_delay_{delayed_edge_counter}_buf");
            let head_name = format!("__graph_delay_{delayed_edge_counter}_head");
            delayed_edge_counter += 1;
            source = if array_len == 1 {
                Expr::Index {
                    base: buf_name.clone(),
                    index: Box::new(Expr::Var(head_name.clone())),
                }
            } else {
                Expr::ArrayLiteral(
                    (0..array_len)
                        .map(|slot| Expr::Index {
                            base: buf_name.clone(),
                            index: Box::new(graph_delay_flat_index_expr(
                                &head_name, array_len, slot,
                            )),
                        })
                        .collect(),
                )
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

        resolved.push(ResolvedGraphEdge {
            original_source: lowered_source,
            source,
            rate,
            delay,
            dest,
            dest_value_ty,
            deps,
            delay_state,
        });
    }

    for output in owner.output_value_types.keys() {
        if !driven_outputs.contains(output) {
            errors.push(Diagnostic::semantic(
                format!("graph must drive declared output '{output}'"),
                0,
                0,
            ));
        }
    }

    let reachable = reachable_nodes_from_outputs(&resolved);
    let topo = topo_sort_nodes(&resolved, &reachable, errors);

    let mut init_stmts = Vec::<Stmt>::new();
    for edge in &resolved {
        let Some(delay_state) = &edge.delay_state else {
            continue;
        };
        let delay_len = edge.delay.unwrap_or(0);
        if delay_len == 0 {
            continue;
        }
        init_stmts.push(Stmt::Assign {
            loc: None,
            target: AssignTarget::Var(delay_state.buf_name.clone()),
            decl_ty: None,
            generic_decl_ty: None,
            is_typed_decl: false,
            expr: Expr::ArrayCtor {
                spec: ArrayTypeSpec {
                    elem: ArrayElemType::Primitive(delay_state.elem_ty),
                    size: Box::new(Expr::Int((delay_len * delay_state.array_len) as i64)),
                },
                init: None,
            },
        });
        init_stmts.push(Stmt::Assign {
            loc: None,
            target: AssignTarget::Var(delay_state.head_name.clone()),
            decl_ty: Some(PrimitiveType::I32),
            generic_decl_ty: None,
            is_typed_decl: true,
            expr: Expr::Int(0),
        });
    }

    let mut block_pre = Vec::<Stmt>::new();
    let mut sample = Vec::<Stmt>::new();
    let mut sample_input_edges = BTreeMap::<GraphNodeKey, Vec<(String, Expr)>>::new();
    let mut sample_param_edges = BTreeMap::<GraphNodeKey, Vec<(String, Expr)>>::new();
    let mut output_edges = Vec::<(String, Expr)>::new();

    for edge in &resolved {
        match (&edge.rate, &edge.dest, &edge.dest_value_ty) {
            (
                GraphRate::Block,
                GraphDestKind::ProcParam { node, param },
                GraphValueType::Scalar(_),
            ) => {
                if reachable.contains(&node) {
                    block_pre.push(assign_node_field_stmt(&node, param, edge.source.clone()));
                }
            }
            (
                GraphRate::Block,
                GraphDestKind::ProcParam { node, param },
                GraphValueType::Array { .. },
            ) => {
                if reachable.contains(&node) {
                    block_pre.extend(assign_node_array_field_stmts(
                        node,
                        param,
                        &edge.source,
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
                if reachable.contains(&node) {
                    sample_input_edges
                        .entry(node.clone())
                        .or_default()
                        .push((port.clone(), edge.source.clone()));
                }
            }
            (
                GraphRate::Sample,
                GraphDestKind::ProcInput { node, port },
                GraphValueType::Array { len, .. },
            ) => {
                if reachable.contains(&node) {
                    let slot_exprs = expand_graph_expr_to_slots(
                        &edge.source,
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
                        .push((port.clone(), Expr::ArrayLiteral(slot_exprs)));
                }
            }
            (
                GraphRate::Sample,
                GraphDestKind::ProcParam { node, param },
                GraphValueType::Scalar(_),
            ) => {
                if reachable.contains(&node) {
                    sample_param_edges
                        .entry(node.clone())
                        .or_default()
                        .push((param.clone(), edge.source.clone()));
                }
            }
            (
                GraphRate::Sample,
                GraphDestKind::ProcParam { node, param },
                GraphValueType::Array { .. },
            ) => {
                if reachable.contains(&node) {
                    sample.extend(assign_node_array_field_stmts(
                        node,
                        param,
                        &edge.source,
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
                output_edges.push((name.clone(), edge.source.clone()));
            }
            _ => {}
        }
    }

    for node in topo {
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
                    loc: None,
                    target: AssignTarget::Index {
                        base: name.clone(),
                        index: Expr::Int(idx as i64),
                    },
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    expr: slot_expr,
                });
            }
        } else {
            sample.push(assign_stmt(name, expr));
        }
    }

    for edge in resolved {
        let Some(delay_state) = edge.delay_state else {
            continue;
        };
        if delay_state.array_len == 1 {
            sample.push(Stmt::Assign {
                loc: None,
                target: AssignTarget::Index {
                    base: delay_state.buf_name.clone(),
                    index: Expr::Var(delay_state.head_name.clone()),
                },
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: edge.original_source,
            });
        } else {
            let slot_exprs = expand_graph_expr_to_slots(
                &edge.original_source,
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
                    loc: None,
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
                    expr: slot_expr,
                });
            }
        }
        sample.push(assign_stmt(
            delay_state.head_name.clone(),
            Expr::Binary {
                op: BinaryOp::Mod,
                lhs: Box::new(Expr::Binary {
                    op: BinaryOp::Add,
                    lhs: Box::new(Expr::Var(delay_state.head_name.clone())),
                    rhs: Box::new(Expr::Int(1)),
                }),
                rhs: Box::new(Expr::Int(edge.delay.unwrap_or(1) as i64)),
            },
        ));
    }

    LoweredGraph {
        init_stmts,
        block_pre,
        sample,
    }
}

fn resolve_graph_dest(
    dest: &GraphEndpoint,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Result<(GraphDestKind, String, GraphValueType), ()> {
    match dest {
        GraphEndpoint::Symbol(name) => {
            if let Some(ty) = owner.output_value_types.get(name).cloned() {
                Ok((GraphDestKind::TopOutput(name.clone()), name.clone(), ty))
            } else {
                errors.push(Diagnostic::semantic(
                    format!("{owner_context} graph destination '{name}' is not a declared output"),
                    0,
                    0,
                ));
                Err(())
            }
        }
        GraphEndpoint::ProcField { proc, field } => {
            let key = GraphNodeKey::Direct(proc.clone());
            resolve_graph_proc_dest(&key, field, nodes, proc_surfaces, owner_context, errors)
        }
        GraphEndpoint::ProcIndexedField { proc, index, field } => {
            let context = format!("{owner_context} graph proc-array destination '{proc}[...]'");
            let Some(idx) = eval_graph_nonnegative_int_expr(index, options, &context, errors)
            else {
                return Err(());
            };
            let key = GraphNodeKey::Indexed {
                base: proc.clone(),
                index: idx,
            };
            resolve_graph_proc_dest(&key, field, nodes, proc_surfaces, owner_context, errors)
        }
    }
}

fn resolve_graph_proc_dest(
    key: &GraphNodeKey,
    field: &str,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Result<(GraphDestKind, String, GraphValueType), ()> {
    let Some(node) = nodes.get(key) else {
        errors.push(Diagnostic::semantic(
            format!(
                "{owner_context} graph destination references unknown node '{}'",
                node_ref_name(key)
            ),
            0,
            0,
        ));
        return Err(());
    };
    let Some(surface) = proc_surfaces.get(&node.proc_name) else {
        return Err(());
    };
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
    if let Some(ty) = surface.in_value_types.get(field).cloned() {
        return Ok((
            GraphDestKind::ProcInput {
                node: key.clone(),
                port: field.to_owned(),
            },
            format!("{}.{}", node_ref_name(key), field),
            ty,
        ));
    }
    if surface.api.outs.iter().any(|out| out == field) {
        errors.push(Diagnostic::semantic(
            format!(
                "{owner_context} graph destination '{}' cannot target processor outputs",
                format!("{}.{}", node_ref_name(key), field)
            ),
            0,
            0,
        ));
        return Err(());
    }
    errors.push(Diagnostic::semantic(
        format!(
            "{owner_context} graph destination '{}' references an unknown endpoint",
            format!("{}.{}", node_ref_name(key), field)
        ),
        0,
        0,
    ));
    Err(())
}

fn validate_graph_source_expr(
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
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        Expr::ArrayLiteral(values) => {
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
        Expr::Var(name) => {
            validate_graph_source_base(
                name,
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
                        Expr::Var(name) => Some(name.clone()),
                        _ => None,
                    });
                let index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG).cloned();
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var(name) => Some(name.clone()),
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
                    errors.push(Diagnostic::semantic(
                        "malformed graph indexed endpoint source",
                        0,
                        0,
                    ));
                }
                return;
            }
            if name == GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var(name) => Some(name.clone()),
                        _ => None,
                    });
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG).cloned();
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var(name) => Some(name.clone()),
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
                    errors.push(Diagnostic::semantic(
                        "malformed graph indexed processor-array output source",
                        0,
                        0,
                    ));
                }
                return;
            }
            errors.push(Diagnostic::semantic(
                "graph source expressions do not support user-defined or processor calls",
                0,
                0,
            ));
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
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } | Expr::UnaryBitNot { expr } => {
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
        Expr::Index { base, index } => {
            validate_graph_source_base(
                base,
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
        Expr::Slice { base, start, end } => {
            validate_graph_source_base(
                base,
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
            errors.push(Diagnostic::semantic(
                "constructor graph sources are not yet supported",
                0,
                0,
            ));
        }
    }
}

fn validate_graph_source_base(
    base: &str,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    block_safe_only: bool,
    inferred_param_rate: bool,
    deps: &mut BTreeSet<GraphNodeKey>,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if owner.param_value_types.contains_key(base) {
        return;
    }
    if owner.input_value_types.contains_key(base) {
        if block_safe_only {
            graph_block_source_error(
                format!("{owner_context} graph @block edge cannot read sample-rate input '{base}'"),
                inferred_param_rate,
                errors,
            );
        }
        return;
    }
    if owner.output_value_types.contains_key(base) {
        errors.push(Diagnostic::semantic(
            format!("{owner_context} graph source cannot read output '{base}'"),
            0,
            0,
        ));
        return;
    }
    if let Some((node_base, field)) = base.rsplit_once('.') {
        let key = GraphNodeKey::Direct(node_base.to_owned());
        validate_graph_proc_field_source(
            &key,
            field,
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
    errors.push(Diagnostic::semantic(
        format!("{owner_context} graph source references unknown symbol '{base}'"),
        0,
        0,
    ));
}

fn infer_graph_source_value_type(
    expr: &Expr,
    owner: &GraphOwnerSurface,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<GraphValueType> {
    match expr {
        Expr::Number(_) => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Expr::Int(v) => Some(GraphValueType::Scalar(
            if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                PrimitiveType::I32
            } else {
                PrimitiveType::I64
            },
        )),
        Expr::Bool(_) => Some(GraphValueType::Scalar(PrimitiveType::Bool)),
        Expr::ArrayLiteral(values) => {
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
                    errors.push(Diagnostic::semantic(
                        format!("{owner_context} graph array literal elements must be scalar"),
                        0,
                        0,
                    ));
                    return None;
                };
                elem_ty = Some(match elem_ty {
                    None => value_ty,
                    Some(prev) if prev == value_ty => prev,
                    Some(prev) if can_implicitly_assign(value_ty, prev) => prev,
                    Some(prev) if can_implicitly_assign(prev, value_ty) => value_ty,
                    Some(prev) => {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{owner_context} graph array literal mixes incompatible element types {:?} and {:?}",
                                prev, value_ty
                            ),
                            0,
                            0,
                        ));
                        return None;
                    }
                });
            }
            Some(GraphValueType::Array {
                elem_ty: elem_ty.unwrap_or(PrimitiveType::F32),
                len: values.len(),
            })
        }
        Expr::Var(name) => {
            if let Some(ty) = builtin_constant_type(name) {
                return Some(GraphValueType::Scalar(ty));
            }
            if let Some(ty) = owner.param_value_types.get(name).cloned() {
                return Some(ty);
            }
            if let Some(ty) = owner.input_value_types.get(name).cloned() {
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
        Expr::Index { base, index } => {
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
                owner
                    .param_value_types
                    .get(base)
                    .cloned()
                    .or_else(|| owner.input_value_types.get(base).cloned())
            };
            match base_ty {
                Some(GraphValueType::Array { elem_ty, .. }) => {
                    Some(GraphValueType::Scalar(elem_ty))
                }
                Some(GraphValueType::Scalar(_)) => None,
                None => None,
            }
        }
        Expr::Slice { base, start, end } => {
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
                    errors.push(Diagnostic::semantic(
                        format!("{owner_context} graph slice '{base}' requires an array source"),
                        0,
                        0,
                    ));
                    None
                }
            }
        }
        Expr::UserCall { name, args, .. } => {
            if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var(name) => Some(name.clone()),
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
                        Expr::Var(name) => Some(name.clone()),
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
                        Expr::Var(name) => Some(name.clone()),
                        _ => None,
                    })?;
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG)?;
                let field = named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                    |expr| match expr {
                        Expr::Var(name) => Some(name.clone()),
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
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{owner_context} graph expression shape mismatch: cannot combine {:?}[{lhs_len}] and {:?}[{rhs_len}]",
                                lhs_elem_ty, rhs_elem_ty
                            ),
                            0,
                            0,
                        ));
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
        Expr::UnaryNot { expr } | Expr::UnaryBitNot { expr } => {
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
    }
}

fn infer_graph_proc_field_value_type(
    key: &GraphNodeKey,
    field: &str,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
) -> Option<GraphValueType> {
    let node = nodes.get(key)?;
    let surface = proc_surfaces.get(&node.proc_name)?;
    surface
        .out_value_types
        .get(field)
        .cloned()
        .or_else(|| surface.param_value_types.get(field).cloned())
        .or_else(|| surface.in_value_types.get(field).cloned())
        .or_else(|| {
            surface.out_array_slots.iter().find_map(|(base, slots)| {
                if slots.iter().any(|slot| slot == field) {
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

fn expand_graph_expr_to_slots(
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
    if let Expr::ArrayLiteral(values) = expr {
        if values.len() != slot_count {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: expected array expression with {slot_count} elements, got {}",
                    values.len()
                ),
                0,
                0,
            ));
            return vec![expr.clone(); slot_count];
        }
        return values.clone();
    }

    match infer_graph_source_value_type(expr, owner, nodes, proc_surfaces, context, options, errors)
    {
        Some(GraphValueType::Scalar(_)) => return vec![expr.clone(); slot_count],
        Some(GraphValueType::Array { len, .. }) if len != slot_count => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: expected array expression with {slot_count} elements, got {len}"
                ),
                0,
                0,
            ));
        }
        Some(GraphValueType::Array { .. }) => {}
        None => {
            errors.push(Diagnostic::semantic(
                format!("{context}: array expression could not be expanded element-wise"),
                0,
                0,
            ));
            return vec![expr.clone(); slot_count];
        }
    }

    match expr {
        Expr::ArrayLiteral(values) => (0..slot_count)
            .map(|i| values.get(i).cloned().unwrap_or(Expr::Number(0.0)))
            .collect(),
        Expr::Var(base) => (0..slot_count)
            .map(|i| Expr::Index {
                base: base.clone(),
                index: Box::new(Expr::Int(i as i64)),
            })
            .collect(),
        Expr::Binary { op, lhs, rhs } => {
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
                    op: *op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                })
                .collect()
        }
        Expr::Cast { to, expr } => expand_graph_expr_to_slots(
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
            to: *to,
            expr: Box::new(expr),
        })
        .collect(),
        Expr::UnaryNot { expr } => expand_graph_expr_to_slots(
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
            expr: Box::new(expr),
        })
        .collect(),
        Expr::UnaryBitNot { expr } => expand_graph_expr_to_slots(
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
            expr: Box::new(expr),
        })
        .collect(),
        Expr::Slice { base, start, end } => {
            let Some(GraphValueType::Array { len, .. }) =
                infer_graph_source_base_value_type(base, owner, nodes, proc_surfaces)
            else {
                errors.push(Diagnostic::semantic(
                    format!("{context}: sliced graph source '{base}' requires an array base"),
                    0,
                    0,
                ));
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
                    base: base.clone(),
                    index: Box::new(Expr::Int((start_idx + i) as i64)),
                })
                .collect()
        }
        _ => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: array expression requires array literals, array symbols, slices, or element-wise expressions"
                ),
                0,
                0,
            ));
            vec![expr.clone(); slot_count]
        }
    }
}

fn require_graph_assignable_type(
    src: &GraphValueType,
    dst: &GraphValueType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match (src, dst) {
        (GraphValueType::Scalar(src_ty), GraphValueType::Scalar(dst_ty)) => {
            require_assignable_type(Some(*src_ty), *dst_ty, context, errors);
        }
        (
            GraphValueType::Scalar(src_ty),
            GraphValueType::Array {
                elem_ty: dst_elem, ..
            },
        ) => {
            require_assignable_type(Some(*src_ty), *dst_elem, context, errors);
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
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} shape mismatch: cannot assign {} to {}",
                        graph_value_type_label(src),
                        graph_value_type_label(dst)
                    ),
                    0,
                    0,
                ));
            } else {
                require_assignable_type(Some(*src_elem), *dst_elem, context, errors);
            }
        }
        _ => errors.push(Diagnostic::semantic(
            format!(
                "{context} shape mismatch: cannot assign {} to {}",
                graph_value_type_label(src),
                graph_value_type_label(dst)
            ),
            0,
            0,
        )),
    }
}

fn validate_graph_proc_field_source(
    key: &GraphNodeKey,
    field: &str,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    block_safe_only: bool,
    inferred_param_rate: bool,
    deps: &mut BTreeSet<GraphNodeKey>,
    owner_context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(node) = nodes.get(key) else {
        errors.push(Diagnostic::semantic(
            format!(
                "{owner_context} graph source references unknown node '{}'",
                node_ref_name(key)
            ),
            0,
            0,
        ));
        return;
    };
    let Some(surface) = proc_surfaces.get(&node.proc_name) else {
        return;
    };
    if surface.api.outs.iter().any(|out| out == field) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read sample-rate processor output '{}.{}'",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
                errors,
            );
        } else {
            deps.insert(key.clone());
        }
        return;
    }
    if surface.out_array_slots.contains_key(field) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read sample-rate processor output '{}.{}'",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
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
                errors,
            );
        }
        return;
    }
    if surface.api.ins.iter().any(|port| port.name == field) {
        if block_safe_only {
            graph_block_source_error(
                format!(
                    "{owner_context} graph @block edge cannot read processor input '{}.{}'",
                    node_ref_name(key),
                    field
                ),
                inferred_param_rate,
                errors,
            );
        }
        return;
    }
    errors.push(Diagnostic::semantic(
        format!(
            "{owner_context} graph source '{}' references an unknown endpoint",
            format!("{}.{}", node_ref_name(key), field)
        ),
        0,
        0,
    ));
}

fn rewrite_graph_source_expr(
    expr: &Expr,
    nodes: &BTreeMap<GraphNodeKey, GraphNodeInfo>,
    proc_surfaces: &HashMap<String, GraphProcSurface>,
    owner_context: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expr {
        Expr::Var(name) => {
            if let Some((node_name, field)) = name.rsplit_once('.') {
                let key = GraphNodeKey::Direct(node_name.to_owned());
                if let Some(node) = nodes.get(&key) {
                    if let Some(surface) = proc_surfaces.get(&node.proc_name) {
                        if let Some(slots) = surface.out_array_slots.get(field) {
                            return Expr::ArrayLiteral(
                                slots
                                    .iter()
                                    .map(|slot_name| Expr::Var(format!("{node_name}.{slot_name}")))
                                    .collect(),
                            );
                        }
                    }
                }
            }
            expr.clone()
        }
        Expr::Index { base, index } => {
            let rewritten_index = rewrite_graph_source_expr(
                index,
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
                        if let Some(slots) = surface.out_array_slots.get(field) {
                            let context = format!("{owner_context} graph source '{}[...]'", base);
                            if let Some(idx) =
                                eval_graph_nonnegative_int_expr(index, options, &context, errors)
                            {
                                if let Some(slot_name) = slots.get(idx) {
                                    return Expr::Var(format!("{node_name}.{slot_name}"));
                                }
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "{context} index {idx} is out of range (expected 0..{})",
                                        slots.len().saturating_sub(1)
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        }
                    }
                }
            }
            Expr::Index {
                base: base.clone(),
                index: Box::new(rewritten_index),
            }
        }
        Expr::ArrayLiteral(values) => Expr::ArrayLiteral(
            values
                .iter()
                .map(|value| {
                    rewrite_graph_source_expr(
                        value,
                        nodes,
                        proc_surfaces,
                        owner_context,
                        options,
                        errors,
                    )
                })
                .collect(),
        ),
        Expr::Compare { op, lhs, rhs } => Expr::Compare {
            op: *op,
            lhs: Box::new(rewrite_graph_source_expr(
                lhs,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
            rhs: Box::new(rewrite_graph_source_expr(
                rhs,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Logical { op, lhs, rhs } => Expr::Logical {
            op: *op,
            lhs: Box::new(rewrite_graph_source_expr(
                lhs,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
            rhs: Box::new(rewrite_graph_source_expr(
                rhs,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: *op,
            lhs: Box::new(rewrite_graph_source_expr(
                lhs,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
            rhs: Box::new(rewrite_graph_source_expr(
                rhs,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Call { func, args } => Expr::Call {
            func: *func,
            args: args
                .iter()
                .map(|arg| {
                    rewrite_graph_source_expr(
                        arg,
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
        } => {
            if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var(name) => Some(name.clone()),
                        _ => None,
                    });
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG);
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var(name) => Some(name.clone()),
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
                                if let Some(slots) = surface.out_array_slots.get(&field) {
                                    return Expr::ArrayLiteral(
                                        slots
                                            .iter()
                                            .map(|slot_name| Expr::UserCall {
                                                name: format!(
                                                    "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
                                                ),
                                                type_args: Vec::new(),
                                                args: vec![
                                                    CallArg {
                                                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                                        expr: Expr::Var(base.clone()),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                                        expr: Expr::Int(proc_idx as i64),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                                        expr: Expr::Var(slot_name.clone()),
                                                    },
                                                ],
                                            })
                                            .collect(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            if name == GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL {
                let base =
                    named_call_arg_expr(args, PROC_INDEX_BASE_ARG).and_then(|expr| match expr {
                        Expr::Var(name) => Some(name.clone()),
                        _ => None,
                    });
                let proc_index = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG);
                let field =
                    named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG).and_then(
                        |expr| match expr {
                            Expr::Var(name) => Some(name.clone()),
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
                                if let Some(slots) = surface.out_array_slots.get(&field) {
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
                                                name: format!(
                                                    "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
                                                ),
                                                type_args: Vec::new(),
                                                args: vec![
                                                    CallArg {
                                                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                                        expr: Expr::Var(base.clone()),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                                        expr: Expr::Int(proc_idx as i64),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                                        expr: Expr::Var(slot_name.clone()),
                                                    },
                                                ],
                                            };
                                        }
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "{field_context} index {field_idx} is out of range (expected 0..{})",
                                                slots.len().saturating_sub(1)
                                            ),
                                            0,
                                            0,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Expr::UserCall {
                name: name.clone(),
                type_args: type_args.clone(),
                args: args
                    .iter()
                    .map(|arg| CallArg {
                        name: arg.name.clone(),
                        expr: rewrite_graph_source_expr(
                            &arg.expr,
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
        Expr::Cast { to, expr } => Expr::Cast {
            to: *to,
            expr: Box::new(rewrite_graph_source_expr(
                expr,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::UnaryNot { expr } => Expr::UnaryNot {
            expr: Box::new(rewrite_graph_source_expr(
                expr,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::UnaryBitNot { expr } => Expr::UnaryBitNot {
            expr: Box::new(rewrite_graph_source_expr(
                expr,
                nodes,
                proc_surfaces,
                owner_context,
                options,
                errors,
            )),
        },
        Expr::Slice { base, start, end } => Expr::Slice {
            base: base.clone(),
            start: start.as_ref().map(|expr| {
                Box::new(rewrite_graph_source_expr(
                    expr,
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
                    nodes,
                    proc_surfaces,
                    owner_context,
                    options,
                    errors,
                ))
            }),
        },
        Expr::ArrayCtor { spec, init } => Expr::ArrayCtor {
            spec: ArrayTypeSpec {
                elem: spec.elem.clone(),
                size: Box::new(rewrite_graph_source_expr(
                    &spec.size,
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
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => expr.clone(),
    }
}

fn reachable_nodes_from_outputs(edges: &[ResolvedGraphEdge]) -> BTreeSet<GraphNodeKey> {
    let mut by_dest = BTreeMap::<GraphNodeKey, Vec<&ResolvedGraphEdge>>::new();
    let mut reachable = BTreeSet::<GraphNodeKey>::new();
    let mut work = Vec::<GraphNodeKey>::new();

    for edge in edges {
        if let GraphDestKind::ProcInput { node, .. } | GraphDestKind::ProcParam { node, .. } =
            &edge.dest
        {
            by_dest.entry(node.clone()).or_default().push(edge);
        }
    }
    for edge in edges {
        if matches!(edge.dest, GraphDestKind::TopOutput(_)) {
            for dep in &edge.deps {
                if reachable.insert(dep.clone()) {
                    work.push(dep.clone());
                }
            }
        }
    }
    while let Some(node) = work.pop() {
        if let Some(incoming) = by_dest.get(&node) {
            for edge in incoming {
                reachable.insert(node.clone());
                for dep in &edge.deps {
                    if reachable.insert(dep.clone()) {
                        work.push(dep.clone());
                    }
                }
            }
        }
    }
    reachable
}

fn topo_sort_nodes(
    edges: &[ResolvedGraphEdge],
    reachable: &BTreeSet<GraphNodeKey>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<GraphNodeKey> {
    let mut incoming = BTreeMap::<GraphNodeKey, usize>::new();
    let mut outgoing = BTreeMap::<GraphNodeKey, BTreeSet<GraphNodeKey>>::new();
    for node in reachable {
        incoming.insert(node.clone(), 0);
    }
    for edge in edges {
        if edge.delay.unwrap_or(0) > 0 || edge.rate != GraphRate::Sample {
            continue;
        }
        let dest_node = match &edge.dest {
            GraphDestKind::ProcInput { node, .. } | GraphDestKind::ProcParam { node, .. } => node,
            GraphDestKind::TopOutput(_) => continue,
        };
        if !reachable.contains(dest_node) {
            continue;
        }
        for dep in &edge.deps {
            if !reachable.contains(dep) || dep == dest_node {
                continue;
            }
            if outgoing
                .entry(dep.clone())
                .or_default()
                .insert(dest_node.clone())
            {
                *incoming.entry(dest_node.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut ready = incoming
        .iter()
        .filter_map(|(node, count)| {
            if *count == 0 {
                Some(node.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    ready.sort();

    let mut out = Vec::<GraphNodeKey>::new();
    while let Some(node) = ready.first().cloned() {
        ready.remove(0);
        out.push(node.clone());
        if let Some(nexts) = outgoing.get(&node) {
            for next in nexts {
                if let Some(count) = incoming.get_mut(next) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.push(next.clone());
                    }
                }
            }
            ready.sort();
        }
    }

    if out.len() != incoming.len() && !incoming.is_empty() {
        let cycle_nodes = incoming
            .iter()
            .filter_map(|(node, count)| if *count > 0 { Some(node.clone()) } else { None })
            .collect::<BTreeSet<_>>();
        if !cycle_nodes.is_empty() {
            if let Some(path) = find_graph_cycle_path(&outgoing, &cycle_nodes) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "graph contains a cycle without sample delay: {}",
                        path.into_iter()
                            .map(|node| node_ref_name(&node))
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                    0,
                    0,
                ));
            } else {
                errors.push(Diagnostic::semantic(
                    format!(
                        "graph contains a cycle without sample delay involving {}",
                        cycle_nodes
                            .into_iter()
                            .map(|node| node_ref_name(&node))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    0,
                    0,
                ));
            }
        }
    }

    for node in reachable {
        if !out.contains(node) {
            out.push(node.clone());
        }
    }
    out
}

fn find_graph_cycle_path(
    outgoing: &BTreeMap<GraphNodeKey, BTreeSet<GraphNodeKey>>,
    cycle_nodes: &BTreeSet<GraphNodeKey>,
) -> Option<Vec<GraphNodeKey>> {
    #[derive(Copy, Clone, Eq, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }

    fn dfs(
        node: &GraphNodeKey,
        outgoing: &BTreeMap<GraphNodeKey, BTreeSet<GraphNodeKey>>,
        cycle_nodes: &BTreeSet<GraphNodeKey>,
        marks: &mut BTreeMap<GraphNodeKey, Mark>,
        stack: &mut Vec<GraphNodeKey>,
    ) -> Option<Vec<GraphNodeKey>> {
        marks.insert(node.clone(), Mark::Visiting);
        stack.push(node.clone());
        if let Some(nexts) = outgoing.get(node) {
            for next in nexts {
                if !cycle_nodes.contains(next) {
                    continue;
                }
                match marks.get(next).copied() {
                    Some(Mark::Visiting) => {
                        let start = stack.iter().position(|entry| entry == next)?;
                        let mut path = stack[start..].to_vec();
                        path.push(next.clone());
                        return Some(path);
                    }
                    Some(Mark::Done) => {}
                    None => {
                        if let Some(path) = dfs(next, outgoing, cycle_nodes, marks, stack) {
                            return Some(path);
                        }
                    }
                }
            }
        }
        stack.pop();
        marks.insert(node.clone(), Mark::Done);
        None
    }

    let mut marks = BTreeMap::<GraphNodeKey, Mark>::new();
    let mut stack = Vec::<GraphNodeKey>::new();
    for node in cycle_nodes {
        if marks.contains_key(node) {
            continue;
        }
        if let Some(path) = dfs(node, outgoing, cycle_nodes, &mut marks, &mut stack) {
            return Some(path);
        }
    }
    None
}

fn build_node_call_expr(node: &GraphNodeKey, mut args: Vec<CallArg>) -> Expr {
    match node {
        GraphNodeKey::Direct(name) => Expr::UserCall {
            name: name.clone(),
            type_args: Vec::new(),
            args,
        },
        GraphNodeKey::Indexed { base, index } => {
            args.insert(
                0,
                CallArg {
                    name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                    expr: Expr::Int(*index as i64),
                },
            );
            args.insert(
                0,
                CallArg {
                    name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                    expr: Expr::Var(base.clone()),
                },
            );
            Expr::UserCall {
                name: PROC_INDEX_CALL_SENTINEL.to_owned(),
                type_args: Vec::new(),
                args,
            }
        }
    }
}
