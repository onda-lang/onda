//! Select ordinary call dependencies, then materialize their constant reads.
//! Selection does not execute const calls or predict runtime control flow.
use super::*;
use crate::compile_context::{CallContexts, CompileContext};
use crate::processor_views::ProcessorBoundary;

type FunctionKey = (usize, CompileContext);

pub(super) struct MaterializedRuntime {
    pub(super) defs: Vec<TypedFunction>,
    pub(super) aliases: Vec<(String, String)>,
    pub(super) const_arrays: Vec<TypedConstArray>,
}

pub(super) fn materialize_reachable_typed_defs(
    mut roots: Vec<(&mut Vec<Stmt>, AnalysisOptions, ProcessorBoundary)>,
    mut defs: Vec<TypedFunction>,
    artifacts: &SemanticConstArtifacts,
    contexts: CallContexts<'_>,
    errors: &mut Vec<Diagnostic>,
    defaults: &super::runtime_defaults::RuntimeDefaults<'_>,
    processor_views: &mut crate::processor_views::ProcessorViewCatalog,
) -> MaterializedRuntime {
    processor_views.register_operation_uses(&defs);
    let indices = defs
        .iter()
        .enumerate()
        .map(|(index, def)| (def.name.clone(), index))
        .collect::<HashMap<_, _>>();
    for (body, _, _) in &mut roots {
        for stmt in &mut **body {
            stmt.visit_exprs_mut(|expr| defaults.expand(expr, errors));
        }
    }
    let mut expanded = prune_inactive_scalar_post_hooks(
        &roots,
        &mut defs,
        &indices,
        processor_views,
        defaults,
        errors,
    );
    let mut catalog = defs.into_iter().map(Some).collect::<Vec<_>>();
    // Checked processor dependencies are shared by every compile context of a
    // source specialization. Keep this cache local to the frozen view metadata.
    let mut processor_dependencies = vec![None; catalog.len()];
    let mut selected = std::collections::BTreeSet::<FunctionKey>::new();
    let mut pending = Vec::new();
    let mut root_routes = Vec::new();
    for (body, options, boundary) in &mut roots {
        select_calls(
            body,
            CompileContext::new(*options),
            &indices,
            &contexts,
            &mut selected,
            &mut pending,
        );
        for dependency in processor_views.root_operation_dependencies(body, *boundary) {
            if let Some(&index) = indices.get(dependency) {
                let context = CompileContext::new(*options);
                let key = (index, contexts.callee(dependency, None, context));
                if selected.insert(key) {
                    pending.push(key);
                }
                root_routes.push((context, dependency.to_owned(), key));
            }
        }
    }
    while let Some((index, context)) = pending.pop() {
        let def = catalog[index].as_mut().unwrap();
        if expanded.insert(index) {
            for stmt in &mut def.body {
                stmt.visit_exprs_mut(|expr| defaults.expand(expr, errors));
            }
        }
        select_calls(
            &def.body,
            context,
            &indices,
            &contexts,
            &mut selected,
            &mut pending,
        );
        // Processor adapters and deferred resource helpers follow the same
        // context selection as ordinary calls.
        let dependencies = processor_dependencies[index].get_or_insert_with(|| {
            processor_views
                .function_dependencies(&def.name)
                .into_iter()
                .filter_map(|dependency| indices.get(dependency).copied())
                .collect::<Vec<_>>()
        });
        for &target in dependencies.iter() {
            let dependency = &catalog[target].as_ref().unwrap().name;
            let key = (target, contexts.callee(dependency, None, context));
            if selected.insert(key) {
                pending.push(key);
            }
        }
    }
    let mut remaining = HashMap::<usize, usize>::new();
    for (index, _) in &selected {
        *remaining.entry(*index).or_default() += 1;
    }
    let names = selected
        .iter()
        .map(|&key @ (index, context)| {
            let name = &catalog[index].as_ref().unwrap().name;
            (
                key,
                if remaining[&index] == 1 {
                    name.clone()
                } else {
                    context.specialized_name(name)
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let aliases = selected
        .iter()
        .filter_map(|key| {
            let source = &catalog[key.0].as_ref().unwrap().name;
            (source != &names[key]).then(|| (source.clone(), names[key].clone()))
        })
        .collect();
    for &(index, context) in &selected {
        for &target in processor_dependencies[index].as_ref().unwrap() {
            let dependency = &catalog[target].as_ref().unwrap().name;
            let target_context = contexts.callee(dependency, None, context);
            processor_views.materialized_targets.insert(
                (context, dependency.clone()),
                names[&(target, target_context)].clone(),
            );
        }
    }
    for (context, source_target, key) in root_routes {
        processor_views
            .materialized_targets
            .insert((context, source_target), names[&key].clone());
    }
    let mut array_names = HashSet::new();
    let mut rewrite = |body: &mut [Stmt], context: CompileContext| {
        for stmt in body {
            stmt.visit_exprs_mut(|expr| {
                expr.visit_mut(|expr| {
                    if let Some(name) = referenced_const_name(expr) {
                        array_names.insert(name.to_owned());
                    }
                    if let Expr::UserCall { name, args, .. } = expr {
                        if let Some(&index) = indices.get(name.as_str()) {
                            let selected =
                                contexts.callee(name, args.first().map(|arg| &arg.expr), context);
                            *name = names[&(index, selected)].clone();
                        }
                    }
                    true
                });
            });
        }
    };
    for (body, options, _) in &mut roots {
        materialize_runtime_stmts(body, artifacts, *options, errors, *options);
        rewrite(body, CompileContext::new(*options));
    }
    let defs = selected
        .into_iter()
        .map(|key @ (index, context)| {
            let count = remaining.get_mut(&index).unwrap();
            *count -= 1;
            // Move the last variant; clone only when a declaration is used in
            // multiple contexts. Array payloads and checked metadata remain shared.
            let mut def = if *count == 0 {
                catalog[index].take().unwrap()
            } else {
                catalog[index].as_ref().unwrap().clone()
            };
            materialize_runtime_stmts(
                &mut def.body,
                artifacts,
                context.options(),
                errors,
                contexts
                    .callee(&def.name, None, CompileContext::new(contexts.host))
                    .options(),
            );
            rewrite(&mut def.body, context);
            def.compile_context = Some(context);
            def.name = names[&key].clone();
            def
        })
        .collect::<Vec<_>>();
    // Lifecycle queries during MIR lowering follow the actual executable bodies,
    // including expanded defaults and pruned scalar completion edges.
    processor_views.register_operation_uses(&defs);
    MaterializedRuntime {
        defs,
        aliases,
        const_arrays: artifacts.const_values.runtime_arrays(&array_names),
    }
}

/// A parent may finish a child activated through a borrowed helper. Those
/// guarded post calls do not themselves activate children or demand their
/// constants. Follow executable calls and view adapters first, then discard
/// completion edges whose processor is never stepped.
fn prune_inactive_scalar_post_hooks(
    roots: &[(&mut Vec<Stmt>, AnalysisOptions, ProcessorBoundary)],
    defs: &mut [TypedFunction],
    indices: &HashMap<String, usize>,
    views: &crate::processor_views::ProcessorViewCatalog,
    defaults: &super::runtime_defaults::RuntimeDefaults<'_>,
    errors: &mut Vec<Diagnostic>,
) -> HashSet<usize> {
    use crate::processor_views::ProcessorOperation;
    if !views
        .declarations()
        .iter()
        .any(|declaration| declaration.len.is_none() && declaration.active_field.is_some())
    {
        return HashSet::new();
    }
    let scalar_post = |name: &str| {
        views.operation(name).is_some_and(|operation| {
            operation.kind == ProcessorOperation::BlockPost
                && operation
                    .projection
                    .edges
                    .last()
                    .is_some_and(|edge| edge.extent.is_none())
        })
    };
    let mut pending = Vec::new();
    let mut selected = HashSet::new();
    let enqueue = |name: &str, pending: &mut Vec<usize>, selected: &mut HashSet<usize>| {
        if let Some(&index) = indices.get(name) {
            if selected.insert(index) {
                pending.push(index);
            }
        }
    };
    let calls = |body: &[Stmt], pending: &mut Vec<usize>, selected: &mut HashSet<usize>| {
        for statement in body {
            statement.visit_exprs(|expression| {
                for expression in expression.walk() {
                    if let Expr::UserCall { name, .. } = expression {
                        if !scalar_post(name) {
                            enqueue(name, pending, selected);
                        }
                    }
                }
            });
        }
    };
    for (body, _, boundary) in roots {
        calls(body, &mut pending, &mut selected);
        for dependency in views.root_operation_dependencies(body, *boundary) {
            enqueue(dependency, &mut pending, &mut selected);
        }
    }
    let mut expanded = HashSet::new();
    while let Some(index) = pending.pop() {
        let def = &mut defs[index];
        for statement in &mut def.body {
            statement.visit_exprs_mut(|expression| defaults.expand(expression, errors));
        }
        expanded.insert(index);
        calls(&def.body, &mut pending, &mut selected);
        for dependency in views.function_dependencies(&def.name) {
            enqueue(dependency, &mut pending, &mut selected);
        }
        if let Some(operation) = views.operation(&def.name).filter(|operation| {
            matches!(
                operation.kind,
                ProcessorOperation::Step | ProcessorOperation::CallOutput(_)
            ) && operation
                .projection
                .edges
                .last()
                .is_some_and(|edge| edge.extent.is_none())
        }) {
            for kind in [ProcessorOperation::BlockPre, ProcessorOperation::BlockPost] {
                if let Some(target) = views.operation_target(&operation.projection, &kind) {
                    enqueue(target, &mut pending, &mut selected);
                }
            }
        }
    }
    for def in defs {
        for statement in &mut def.body {
            statement.visit_statements_mut(|statement| {
                let Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } = statement
                else {
                    return;
                };
                let [Stmt::Expr {
                    expr: Expr::UserCall { name, .. },
                    ..
                }] = then_branch.as_slice()
                else {
                    return;
                };
                if else_branch.is_empty()
                    && scalar_post(name)
                    && !indices
                        .get(name)
                        .is_some_and(|index| selected.contains(index))
                {
                    then_branch.clear();
                }
            });
        }
    }
    expanded
}

fn select_calls(
    body: &[Stmt],
    context: CompileContext,
    indices: &HashMap<String, usize>,
    contexts: &CallContexts<'_>,
    selected: &mut std::collections::BTreeSet<FunctionKey>,
    pending: &mut Vec<FunctionKey>,
) {
    for stmt in body {
        stmt.visit_exprs(|expr| {
            for expr in expr.walk() {
                if let Expr::UserCall { name, args, .. } = expr {
                    if let Some(&index) = indices.get(name) {
                        let key = (
                            index,
                            contexts.callee(name, args.first().map(|arg| &arg.expr), context),
                        );
                        if selected.insert(key) {
                            pending.push(key);
                        }
                    }
                }
            }
        });
    }
}
