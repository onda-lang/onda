use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::*;

fn graph_use_point_rank(
    point: &GraphUsePoint,
    topo_positions: &HashMap<GraphNodeKey, usize>,
) -> usize {
    match point {
        GraphUsePoint::BeforeNode(node) => *topo_positions.get(node).unwrap_or(&usize::MAX),
        GraphUsePoint::BeforeOutputs => usize::MAX,
    }
}

pub(super) fn note_graph_use_point(
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

pub(super) fn reachable_nodes_from_outputs(
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

pub(super) fn topo_sort_nodes(
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
