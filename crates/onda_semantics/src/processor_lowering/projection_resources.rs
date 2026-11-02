//! Completes generated operation calls from declared resource identities.

use super::*;

type ResourceIdentity = (usize, usize);
type ResourceLayout = Vec<(ResourceIdentity, String)>;

fn layout(
    declared: &view_catalog::DeclaredProjection,
    kind: &crate::processor_views::ProcessorOperation,
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
    options: AnalysisOptions,
) -> ResourceLayout {
    view_catalog::operation_resources(declared, kind, shapes, procs, options)
        .into_iter()
        .map(|(capture, spec)| {
            let identity = (
                declared.projection.edges.len() - capture.frame_depth,
                capture.resource_index,
            );
            (identity, spec.name)
        })
        .collect()
}

pub(super) fn complete_operation_calls(
    blocks: &mut [Block],
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let mut layouts = HashMap::new();
    for owner in shapes.keys() {
        for declared in view_catalog::declared_projections(owner, shapes) {
            for (source, _, kind) in view_catalog::projection_operations(
                &declared,
                &procs[declared.proc_name()],
                &shapes[declared.proc_name()],
            ) {
                layouts.insert(source, layout(&declared, &kind, shapes, procs, options));
            }
        }
    }
    let signatures = blocks
        .iter()
        .filter_map(|block| {
            let Block::Def(function) = block else {
                return None;
            };
            Some((
                function.name.clone(),
                (
                    function.params.len(),
                    view_catalog::captured_resource_parameters(function),
                ),
            ))
        })
        .collect::<HashMap<_, _>>();
    for (name, resources) in &mut layouts {
        if let Some((_, parameters)) = signatures.get(name) {
            resources.retain(|(_, name)| parameters.contains(name));
        }
    }
    for block in blocks {
        let Block::Def(function) = block else {
            continue;
        };
        let own = function
            .params
            .first()
            .and_then(|param| match &param.ty {
                Some(FnParamType::Struct(owner)) => shapes.get(owner).map(|shape| {
                    shape
                        .buffer_specs
                        .iter()
                        .enumerate()
                        .map(|(index, spec)| ((0, index), spec.name.clone()))
                        .collect::<ResourceLayout>()
                }),
                _ => None,
            })
            .unwrap_or_default();
        let source = layouts
            .get(&function.name)
            .unwrap_or(&own)
            .iter()
            .filter(|(_, name)| function.params.iter().any(|param| param.name == *name))
            .cloned()
            .collect::<HashMap<_, _>>();
        for statement in &mut function.body {
            statement.visit_exprs_mut(|expr| {
                expr.visit_mut_postorder(|expr| {
                    let Expr::UserCall { name, args, loc, .. } = expr else { return };
                    let Some(resources) = layouts.get(name) else { return };
                    let Some((arity, _)) = signatures.get(name) else { return };
                    let arity = *arity;
                    let provided = args.len() - usize::from(proc_storage::selected_coordinate_argument(args).is_some());
                    if provided >= arity { return; }
                    let Some(nonresources) = arity.checked_sub(resources.len()) else { return };
                    let Some(bound) = provided.checked_sub(nonresources) else { return };
                    for (identity, _) in resources.iter().skip(bound) {
                        let Some(binding) = source.get(identity) else {
                            push_semantic(DiagCtx::new(loc.clone()), errors, format!("processor operation '{name}' lost the resource capture '{}.{}'", identity.0, identity.1));
                            return;
                        };
                        args.push(CallArg { name: None, expr: Expr::var(binding) });
                    }
                });
            });
        }
    }
}
