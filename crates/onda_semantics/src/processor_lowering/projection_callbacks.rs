//! Callback handlers compose through each declared ownership boundary.

use super::*;

pub(super) fn child_when_body(
    declared: &view_catalog::DeclaredProjection,
    delegate: &str,
    procs: &HashMap<String, ProcessorDef>,
    shapes: &HashMap<String, ProcLoweringShape>,
    proc_api: &HashMap<String, ProcApi>,
    options: AnalysisOptions,
    context_args: &[String],
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    let edges = &declared.projection.edges;
    let edge = edges.last().expect("declared child edge");
    let owner_type = if edges.len() == 1 {
        &declared.projection.owner
    } else {
        &edges[edges.len() - 2].proc_name
    };
    let owner = &procs[owner_type];
    let child = &procs[&edge.proc_name];
    let captured_count = if nested_proc_captures_delegate_context(
        owner,
        child,
        &edge.declaration,
        options,
        usize::from(edge.extent.is_some()),
    ) {
        shapes[owner_type].buffer_specs.len()
    } else {
        0
    };
    let (owner_args, outer_args) = context_args.split_at(captured_count);
    let selected_index = edge
        .extent
        .and_then(|_| proc_storage::family_coordinate(declared.projection.coordinate_arity()));
    let body = proc_child_when_body(
        owner,
        child,
        &edge.declaration,
        delegate,
        options,
        owner_args,
        selected_index.as_ref(),
    );
    if edges.len() == 1 || body.is_empty() {
        return body;
    }
    let parent_path = view_catalog::DeclaredProjection::source_path_for(&edges[..edges.len() - 1]);
    let shape = &shapes[owner_type];
    let ins = shape
        .ins
        .iter()
        .chain(shape.buffer_specs.iter().map(|spec| &spec.name))
        .cloned()
        .collect();
    lower_callee_stmts_for_nested_wrapper(
        body,
        &declared.projection.owner,
        owner_type,
        &parent_path,
        shape,
        &HashMap::new(),
        &ins,
        &shape.field_array_slots,
        &shape.in_array_slots,
        &shape.nested_proc_array_slots,
        outer_args,
        proc_api,
        errors,
    )
}
