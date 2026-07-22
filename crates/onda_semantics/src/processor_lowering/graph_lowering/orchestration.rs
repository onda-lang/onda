use super::*;

pub(crate) fn lower_graph_blocks(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let error_count = errors.len();
    synthesize_graph_port_decls(program);
    reject_block_timed_graph_outputs(program, errors);
    if errors.len() != error_count {
        return;
    }
    let proc_surfaces = build_graph_proc_surfaces(program, options, errors);
    lower_proc_graph_blocks(program, &proc_surfaces, options, errors);
    lower_top_level_graph_block(program, &proc_surfaces, options, errors);
}

fn reject_block_timed_graph_outputs(program: &Program, errors: &mut Vec<Diagnostic>) {
    let has_top_level_graph = program.block(BlockKind::Graph).is_some();
    if has_top_level_graph {
        if let Some(Block::KOuts(ports)) = program.block(BlockKind::KOuts) {
            errors.push(Diagnostic::semantic_span(
                "top-level graph block does not support kouts",
                ports.loc.as_ref(),
            ));
        }
    }

    for block in &program.blocks {
        let Block::Proc(proc) = block else {
            continue;
        };
        if proc.graph.is_some() && proc.outs_timing == OutputTiming::Block {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "processor '{}' graph block does not support kouts",
                    proc.name
                ),
                proc.outs_timing_loc
                    .as_ref()
                    .or_else(|| proc.graph.as_ref().and_then(|graph| graph.loc.as_ref()))
                    .or(proc.loc.as_ref()),
            ));
        }
    }
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
                loc: graph.loc,
                default_ty: None,
                default_ty_loc: Default::default(),
                body: lowered.init_stmts,
            }));
        }
    }

    let sample_block = SampleBlock {
        loc: graph.loc,
        oversample_factor: None,
        body: lowered.sample,
    };
    if lowered.block_pre.is_empty() {
        program.blocks.push(Block::Sample(sample_block));
    } else {
        program.blocks.push(Block::Block(BlockExec {
            loc: graph.loc,
            pre: lowered.block_pre,
            sample: Some(sample_block),
            post: Vec::new(),
        }));
    }
}
