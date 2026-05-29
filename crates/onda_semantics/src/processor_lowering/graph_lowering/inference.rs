use std::collections::{BTreeSet, HashSet};

use onda_frontend::ast::{OutputTiming, PortBlock};

use super::*;

#[derive(Default)]
struct GraphIoInference {
    input_names: BTreeSet<String>,
    output_names: BTreeSet<String>,
    max_in: usize,
    max_out: usize,
}

pub(super) fn synthesize_graph_port_decls(program: &mut Program) {
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
                    deferred_count: None,
                    deferred_default_ty: None,
                    deferred_prefix: String::new(),
                    output_timing: existing.output_timing,
                    output_timing_loc: existing.output_timing_loc,
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
            deferred_count: None,
            deferred_default_ty: None,
            deferred_prefix: String::new(),
            output_timing: OutputTiming::Sample,
            output_timing_loc: Default::default(),
        })),
        BlockKind::Outs => blocks.push(Block::Outs(PortBlock {
            loc: Default::default(),
            decls: ports,
            deferred_count: None,
            deferred_default_ty: None,
            deferred_prefix: String::new(),
            output_timing: OutputTiming::Sample,
            output_timing_loc: Default::default(),
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
                output_timing: None,
                output_timing_loc: Default::default(),
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
                output_timing: None,
                output_timing_loc: Default::default(),
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
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
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
