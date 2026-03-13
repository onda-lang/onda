use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use omni_frontend::{ast::PortBlock, SourceLoc};

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
    in_aliases: HashMap<String, String>,
    out_aliases: HashMap<String, String>,
    param_array_slots: HashMap<String, Vec<String>>,
    out_array_slots: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
struct GraphOwnerSurface {
    input_value_types: HashMap<String, GraphValueType>,
    param_value_types: HashMap<String, GraphValueType>,
    output_value_types: HashMap<String, GraphValueType>,
    input_aliases: HashMap<String, String>,
    output_aliases: HashMap<String, String>,
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
struct ResolvedGraphSourcePlan {
    rate: GraphRate,
    delay: Option<usize>,
    deps: BTreeSet<GraphNodeKey>,
    original_source: Expr,
    source: Expr,
    delay_state: Option<GraphDelayState>,
    shared_tmp: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedGraphEdge {
    source_plan: usize,
    dest: GraphDestKind,
    dest_value_ty: GraphValueType,
}

#[derive(Debug, Clone)]
struct LoweredGraph {
    init_stmts: Vec<Stmt>,
    block_pre: Vec<Stmt>,
    sample: Vec<Stmt>,
}

#[derive(Debug, Clone)]
enum GraphSourceExpansion {
    Shared { expr: Expr, use_shared_tmp: bool },
    PerDest(Vec<Expr>),
}

#[derive(Default)]
struct GraphIoInference {
    input_names: BTreeSet<String>,
    output_names: BTreeSet<String>,
    max_in: usize,
    max_out: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum GraphUsePoint {
    BeforeNode(GraphNodeKey),
    BeforeOutputs,
}

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub(crate) fn lower_graph_blocks(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    synthesize_graph_port_decls(program);
    let proc_surfaces = build_graph_proc_surfaces(program, options, errors);
    lower_proc_graph_blocks(program, &proc_surfaces, options, errors);
    lower_top_level_graph_block(program, &proc_surfaces, options, errors);
}

fn synthesize_graph_port_decls(program: &mut Program) {
    for block in &mut program.blocks {
        if let Block::Proc(proc) = block {
            if let Some(graph) = proc.graph.as_ref() {
                let param_names = proc
                    .params
                    .iter()
                    .map(|param| param.name.clone())
                    .collect::<HashSet<_>>();
                let inferred = infer_io_from_graph(graph, &param_names);
                proc.ins = merge_graph_inferred_port_decls(
                    &proc.ins,
                    "in",
                    &inferred.input_names,
                    inferred.max_in,
                );
                proc.outs = merge_graph_inferred_port_decls(
                    &proc.outs,
                    "out",
                    &inferred.output_names,
                    inferred.max_out,
                );
            }
        }
    }

    if let Some(graph) = program
        .block(BlockKind::Graph)
        .and_then(|block| match block {
            Block::Graph(graph) => Some(graph.clone()),
            _ => None,
        })
    {
        let param_names = match program.block(BlockKind::Params) {
            Some(Block::Params(params)) => params.iter().map(|param| param.name.clone()).collect(),
            _ => HashSet::new(),
        };
        let inferred = infer_io_from_graph(&graph, &param_names);
        let explicit_ins = match program.block(BlockKind::Ins) {
            Some(Block::Ins(ports)) => ports.decls.clone(),
            _ => Vec::new(),
        };
        let explicit_outs = match program.block(BlockKind::Outs) {
            Some(Block::Outs(ports)) => ports.decls.clone(),
            _ => Vec::new(),
        };
        upsert_top_level_port_block(
            &mut program.blocks,
            BlockKind::Ins,
            merge_graph_inferred_port_decls(
                &explicit_ins,
                "in",
                &inferred.input_names,
                inferred.max_in,
            ),
        );
        upsert_top_level_port_block(
            &mut program.blocks,
            BlockKind::Outs,
            merge_graph_inferred_port_decls(
                &explicit_outs,
                "out",
                &inferred.output_names,
                inferred.max_out,
            ),
        );
    }
}

fn upsert_top_level_port_block(blocks: &mut Vec<Block>, kind: BlockKind, ports: Vec<PortDecl>) {
    if let Some(block) = blocks.iter_mut().find(|block| block.kind() == kind) {
        match block {
            Block::Ins(existing) | Block::Outs(existing) => {
                *existing = PortBlock {
                    loc: existing.loc.clone(),
                    decls: ports,
                }
            }
            _ => {}
        }
        return;
    }
    match kind {
        BlockKind::Ins => blocks.push(Block::Ins(PortBlock {
            loc: Default::default(),
            decls: ports,
        })),
        BlockKind::Outs => blocks.push(Block::Outs(PortBlock {
            loc: Default::default(),
            decls: ports,
        })),
        _ => {}
    }
}

fn merge_graph_inferred_port_decls(
    explicit: &[PortDecl],
    prefix: &str,
    inferred_named: &BTreeSet<String>,
    inferred_max: usize,
) -> Vec<PortDecl> {
    let mut out = explicit.to_vec();
    let mut seen = out
        .iter()
        .map(|port| port.name.clone())
        .collect::<HashSet<_>>();
    let positional_len = explicit.len().max(inferred_max);
    for idx in 0..positional_len {
        if explicit.get(idx).is_some() {
            continue;
        }
        let alias = format!("{prefix}{}", idx + 1);
        if seen.insert(alias.clone()) {
            out.push(PortDecl {
                loc: Default::default(),
                name: alias,
                ty: None,
                ty_loc: Default::default(),
                default: None,
                range: None,
            });
        }
    }
    for name in inferred_named {
        if parse_numbered_port_index(name, prefix).is_some() {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(PortDecl {
                loc: Default::default(),
                name: name.clone(),
                ty: None,
                ty_loc: Default::default(),
                default: None,
                range: None,
            });
        }
    }
    out
}

fn infer_io_from_graph(graph: &GraphBlock, param_names: &HashSet<String>) -> GraphIoInference {
    let mut inferred = GraphIoInference::default();
    for edge in &graph.edges {
        for dest in &edge.dests {
            infer_graph_output_endpoint(dest, &mut inferred);
        }
        infer_graph_input_expr(&edge.source, param_names, &mut inferred);
    }
    for output in &inferred.output_names {
        inferred.input_names.remove(output);
    }
    inferred
}

fn infer_graph_output_endpoint(endpoint: &GraphEndpoint, inferred: &mut GraphIoInference) {
    if let GraphEndpoint::Symbol { name, .. } = endpoint {
        if let Some(idx) = parse_numbered_port_index(name, "out") {
            inferred.output_names.insert(name.clone());
            inferred.max_out = inferred.max_out.max(idx);
        }
    }
}

fn infer_graph_input_expr(
    expr: &Expr,
    param_names: &HashSet<String>,
    inferred: &mut GraphIoInference,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::Var { name, .. } => infer_graph_input_base(name, param_names, inferred),
        Expr::Index { base, index, .. } => {
            infer_graph_input_base(base, param_names, inferred);
            infer_graph_input_expr(index, param_names, inferred);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            infer_graph_input_base(base, param_names, inferred);
            if let Some(start) = start {
                infer_graph_input_expr(start, param_names, inferred);
            }
            if let Some(end) = end {
                infer_graph_input_expr(end, param_names, inferred);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            infer_graph_input_expr(lhs, param_names, inferred);
            infer_graph_input_expr(rhs, param_names, inferred);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            infer_graph_input_expr(expr, param_names, inferred)
        }
        Expr::Call { args, .. } => {
            for arg in args {
                infer_graph_input_expr(arg, param_names, inferred);
            }
        }
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                infer_graph_input_expr(value, param_names, inferred);
            }
        }
        Expr::UserCall { .. } => {}
        Expr::ArrayCtor { spec, init, .. } => {
            infer_graph_input_expr(&spec.size, param_names, inferred);
            if let Some(init) = init {
                for value in init {
                    infer_graph_input_expr(value, param_names, inferred);
                }
            }
        }
    }
}

fn infer_graph_input_base(
    base: &str,
    param_names: &HashSet<String>,
    inferred: &mut GraphIoInference,
) {
    if builtin_constant_type(base).is_some() || param_names.contains(base) || base.contains('.') {
        return;
    }
    if let Some(idx) = parse_numbered_port_index(base, "in") {
        inferred.input_names.insert(base.to_owned());
        inferred.max_in = inferred.max_in.max(idx);
    }
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
        let inferred_io = infer_numbered_io_from_sample(&proc.sample);
        let (graph_ins, in_aliases) =
            graph_port_decls_with_numbered_aliases(&proc.ins, "in", inferred_io.max_in);
        let (graph_outs, out_aliases) =
            graph_port_decls_with_numbered_aliases(&proc.outs, "out", inferred_io.max_out);
        let (_ins, _in_types, in_ports, _) =
            expand_proc_port_specs(&proc.name, &graph_ins, "ins", options, errors);
        let (outs, _, _, out_array_slots) =
            expand_proc_port_specs(&proc.name, &graph_outs, "outs", options, errors);
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
                    &graph_ins, options, errors, &proc.name, "input",
                ),
                param_value_types: value_types_from_params(
                    &proc.params,
                    options,
                    errors,
                    &proc.name,
                ),
                out_value_types: value_types_from_ports(
                    &graph_outs,
                    options,
                    errors,
                    &proc.name,
                    "output",
                ),
                in_aliases,
                out_aliases,
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
    let sample_body = match program.block(BlockKind::Sample) {
        Some(Block::Sample(sample)) => sample.body.clone(),
        _ => Vec::new(),
    };
    let inferred_io = infer_numbered_io_from_sample(&sample_body);
    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(ports)) => ports.decls.clone(),
        _ => Vec::new(),
    };
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(ports)) => ports.decls.clone(),
        _ => Vec::new(),
    };
    let (graph_ins, input_aliases) =
        graph_port_decls_with_numbered_aliases(&raw_ins, "in", inferred_io.max_in);
    let (graph_outs, output_aliases) =
        graph_port_decls_with_numbered_aliases(&raw_outs, "out", inferred_io.max_out);
    GraphOwnerSurface {
        input_value_types: value_types_from_ports(
            &graph_ins,
            options,
            errors,
            "top-level",
            "input",
        ),
        param_value_types: match program.block(BlockKind::Params) {
            Some(Block::Params(params)) => {
                value_types_from_params(params, options, errors, "top-level")
            }
            _ => HashMap::new(),
        },
        output_value_types: value_types_from_ports(
            &graph_outs,
            options,
            errors,
            "top-level",
            "output",
        ),
        input_aliases,
        output_aliases,
    }
}

fn graph_owner_surface_from_proc(
    proc: &ProcessorDef,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> GraphOwnerSurface {
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let (graph_ins, input_aliases) =
        graph_port_decls_with_numbered_aliases(&proc.ins, "in", inferred_io.max_in);
    let (graph_outs, output_aliases) =
        graph_port_decls_with_numbered_aliases(&proc.outs, "out", inferred_io.max_out);
    GraphOwnerSurface {
        input_value_types: value_types_from_ports(&graph_ins, options, errors, &proc.name, "input"),
        param_value_types: value_types_from_params(&proc.params, options, errors, &proc.name),
        output_value_types: value_types_from_ports(
            &graph_outs,
            options,
            errors,
            &proc.name,
            "output",
        ),
        input_aliases,
        output_aliases,
    }
}

fn graph_port_decls_with_numbered_aliases(
    explicit: &[PortDecl],
    prefix: &str,
    inferred_max: usize,
) -> (Vec<PortDecl>, HashMap<String, String>) {
    let mut ports = explicit.to_vec();
    let mut aliases = HashMap::<String, String>::new();
    let target_len = explicit.len().max(inferred_max);
    for idx in 0..target_len {
        let alias = format!("{prefix}{}", idx + 1);
        if let Some(port) = explicit.get(idx) {
            if port.name != alias {
                aliases.insert(alias, port.name.clone());
            }
        } else {
            ports.push(PortDecl {
                loc: Default::default(),
                name: alias,
                ty: None,
                ty_loc: Default::default(),
                default: None,
                range: None,
            });
        }
    }
    (ports, aliases)
}

fn resolve_graph_owner_input_name<'a>(owner: &'a GraphOwnerSurface, name: &'a str) -> &'a str {
    owner
        .input_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

fn resolve_graph_owner_output_name<'a>(owner: &'a GraphOwnerSurface, name: &'a str) -> &'a str {
    owner
        .output_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

fn resolve_graph_proc_input_name<'a>(surface: &'a GraphProcSurface, name: &'a str) -> &'a str {
    surface
        .in_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

fn resolve_graph_proc_output_name<'a>(surface: &'a GraphProcSurface, name: &'a str) -> &'a str {
    surface
        .out_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
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

fn push_graph_error(errors: &mut Vec<Diagnostic>, loc: SourceLoc, message: impl Into<String>) {
    errors.push(Diagnostic::semantic_span(message, loc));
}

fn eval_graph_nonnegative_int_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        push_graph_error(errors, expr.loc(), format!("{context} must be finite"));
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        push_graph_error(errors, expr.loc(), format!("{context} must be an integer"));
        return None;
    }
    if rounded < 0.0 {
        push_graph_error(
            errors,
            expr.loc(),
            format!("{context} must be greater than or equal to zero"),
        );
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
        push_graph_error(errors, expr.loc(), format!("{context} must be finite"));
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        push_graph_error(errors, expr.loc(), format!("{context} must be an integer"));
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
        let loc = SourceLoc::spanning(
            start.and_then(|expr| expr.loc().cloned()),
            end.and_then(|expr| expr.loc().cloned()),
        );
        push_graph_error(
            errors,
            loc,
            format!("{context} slice must have positive length"),
        );
        return None;
    }
    Some((start_idx, end_idx))
}

fn graph_block_source_error(
    detail: String,
    inferred_param_rate: bool,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let message = if inferred_param_rate {
        format!("{detail}; add @sample to this param edge if sample-rate modulation is intended")
    } else {
        detail
    };
    push_graph_error(errors, loc, message);
}

fn infer_graph_source_base_value_type(
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

fn node_ref_name(node: &GraphNodeKey) -> String {
    match node {
        GraphNodeKey::Direct(name) => name.clone(),
        GraphNodeKey::Indexed { base, index } => format!("{base}[{index}]"),
    }
}

fn assign_stmt(target: String, expr: Expr) -> Stmt {
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

fn graph_delay_flat_index_expr(head_name: &str, array_len: usize, slot: usize) -> Expr {
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

fn call_stmt(expr: Expr) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr,
    }
}

fn assign_node_field_stmt(node: &GraphNodeKey, field: &str, expr: Expr) -> Stmt {
    match node {
        GraphNodeKey::Direct(name) => assign_stmt(format!("{name}.{field}"), expr),
        GraphNodeKey::Indexed { base, index } => Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Index {
                base: format!("{base}.{field}"),
                index: Expr::int(*index as i64),
            },
            decl_ty: None,
            generic_decl_ty: None,
            is_typed_decl: false,
            typed_decl_ty_loc: Default::default(),
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
            errors.push(Diagnostic::semantic_span(
                format!(
                    "processor '{}' graph block cannot be declared with sample or block",
                    proc.name
                ),
                graph.loc.as_ref().or(proc.loc.as_ref()),
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
        let duplicate_loc = program.blocks[graph_indices[1]].loc();
        errors.push(Diagnostic::semantic_span(
            "duplicate block 'graph'",
            duplicate_loc,
        ));
        return;
    }
    let graph_loc = program.blocks[graph_indices[0]].loc().cloned();
    if program.block(BlockKind::Sample).is_some() {
        errors.push(Diagnostic::semantic_span(
            "graph block cannot be declared with sample block",
            graph_loc.as_ref(),
        ));
        return;
    }
    if program.block(BlockKind::Block).is_some() {
        errors.push(Diagnostic::semantic_span(
            "graph block cannot be declared with block section",
            graph_loc.as_ref(),
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
                loc: graph.loc.clone(),
                default_ty: None,
                default_ty_loc: Default::default(),
                body: lowered.init_stmts,
            }));
        }
    }

    let sample_block = SampleBlock {
        loc: graph.loc.clone(),
        oversample_factor: None,
        body: lowered.sample,
    };
    if lowered.block_pre.is_empty() {
        program.blocks.push(Block::Sample(sample_block));
    } else {
        program.blocks.push(Block::Block(BlockExec {
            loc: graph.loc.clone(),
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
    with_loc_diag_context(graph.loc.as_ref(), |graph_diag| {
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
                        for (expr, (dest, dest_key, dest_value_ty)) in
                            exprs.into_iter().zip(edge_dests.into_iter())
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
                },
            });
            init_stmts.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(delay_state.head_name.clone()),
                decl_ty: Some(PrimitiveType::I32),
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
                    if reachable.contains(&node) {
                        block_pre.push(assign_node_field_stmt(&node, param, edge_source));
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
                    if reachable.contains(&node) {
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
                    if reachable.contains(&node) {
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
                    if reachable.contains(&node) {
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
                    if reachable.contains(&node) {
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

fn expand_graph_bundle_source(
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

fn graph_use_point_rank(
    point: &GraphUsePoint,
    topo_positions: &HashMap<GraphNodeKey, usize>,
) -> usize {
    match point {
        GraphUsePoint::BeforeNode(node) => *topo_positions.get(node).unwrap_or(&usize::MAX),
        GraphUsePoint::BeforeOutputs => usize::MAX,
    }
}

fn note_graph_use_point(
    use_points: &mut HashMap<usize, GraphUsePoint>,
    source_plan: usize,
    point: GraphUsePoint,
    topo_positions: &HashMap<GraphNodeKey, usize>,
) {
    match use_points.get(&source_plan) {
        Some(existing)
            if graph_use_point_rank(existing, topo_positions)
                <= graph_use_point_rank(&point, topo_positions) => {}
        _ => {
            use_points.insert(source_plan, point);
        }
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
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::ArrayLiteral { values, .. } => {
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

fn require_graph_assignable_type(
    src: &GraphValueType,
    dst: &GraphValueType,
    loc: SourceLoc,
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
                require_assignable_type(Some(*src_elem), *dst_elem, context, errors);
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

fn rewrite_graph_source_expr(
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
                                loc: expr_loc.clone().into(),
                                values: slots
                                    .iter()
                                    .map(|slot_name| {
                                        Expr::var(format!("{node_name}.{slot_name}"))
                                            .with_loc(expr_loc.clone())
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
                                .with_loc(expr_loc.clone());
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
                                        .with_loc(expr_loc.clone());
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
                                        loc: expr_loc.clone().into(),
                                        values: slots
                                            .iter()
                                            .map(|slot_name| Expr::UserCall {
                                                loc: expr_loc.clone().into(),
                                                name: format!(
                                                    "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
                                                ),
                                                type_args: Vec::new(),
                                                args: vec![
                                                    CallArg {
                                                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                                        expr: Expr::var(base.clone())
                                                            .with_loc(expr_loc.clone()),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                                        expr: Expr::int(proc_idx as i64)
                                                            .with_loc(proc_index.loc().cloned()),
                                                    },
                                                    CallArg {
                                                        name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                                        expr: Expr::var(slot_name.clone())
                                                            .with_loc(expr_loc.clone()),
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
                                                loc: expr_loc.clone().into(),
                                                name: format!(
                                                    "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
                                                ),
                                                type_args: Vec::new(),
                                                args: vec![
                                                    CallArg {
                                                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                                        expr: Expr::var(base.clone())
                                                            .with_loc(expr_loc.clone()),
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
            base, start, end, ..
        } => Expr::Slice {
            loc: expr_loc.into(),
            base: base.clone(),
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
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => expr.clone(),
    }
}

fn reachable_nodes_from_outputs(
    edges: &[ResolvedGraphEdge],
    source_plans: &[ResolvedGraphSourcePlan],
) -> BTreeSet<GraphNodeKey> {
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
            for dep in &source_plans[edge.source_plan].deps {
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
                for dep in &source_plans[edge.source_plan].deps {
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
    source_plans: &[ResolvedGraphSourcePlan],
    reachable: &BTreeSet<GraphNodeKey>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Vec<GraphNodeKey> {
    let mut incoming = BTreeMap::<GraphNodeKey, usize>::new();
    let mut outgoing = BTreeMap::<GraphNodeKey, BTreeSet<GraphNodeKey>>::new();
    for node in reachable {
        incoming.insert(node.clone(), 0);
    }
    for edge in edges {
        let source_plan = &source_plans[edge.source_plan];
        if source_plan.delay.unwrap_or(0) > 0 || source_plan.rate != GraphRate::Sample {
            continue;
        }
        let dest_node = match &edge.dest {
            GraphDestKind::ProcInput { node, .. } | GraphDestKind::ProcParam { node, .. } => node,
            GraphDestKind::TopOutput(_) => continue,
        };
        if !reachable.contains(dest_node) {
            continue;
        }
        for dep in &source_plan.deps {
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
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "graph contains a cycle without sample delay: {}",
                        path.into_iter()
                            .map(|node| node_ref_name(&node))
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                );
            } else {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "graph contains a cycle without sample delay involving {}",
                        cycle_nodes
                            .into_iter()
                            .map(|node| node_ref_name(&node))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
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
