//! Select ordinary call dependencies, then materialize their constant reads.
//! Selection does not execute const calls or predict runtime control flow.
use super::*;
use crate::compile_context::{CallContexts, CompileContext};

type FunctionKey = (usize, CompileContext);

pub(super) struct MaterializedRuntime {
    pub(super) defs: Vec<TypedFunction>,
    pub(super) aliases: Vec<(String, String)>,
    pub(super) const_arrays: Vec<TypedConstArray>,
}

pub(super) fn materialize_reachable_typed_defs(
    mut roots: Vec<(&mut Vec<Stmt>, AnalysisOptions)>,
    defs: Vec<TypedFunction>,
    artifacts: &SemanticConstArtifacts,
    contexts: CallContexts<'_>,
    errors: &mut Vec<Diagnostic>,
    defaults: &super::runtime_defaults::RuntimeDefaults<'_>,
) -> MaterializedRuntime {
    let indices = defs
        .iter()
        .enumerate()
        .map(|(index, def)| (def.name.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut catalog = defs.into_iter().map(Some).collect::<Vec<_>>();
    let mut selected = std::collections::BTreeSet::<FunctionKey>::new();
    let mut pending = Vec::new();
    let mut expanded = HashSet::new();
    for (body, options) in &mut roots {
        for stmt in &mut **body {
            stmt.visit_exprs_mut(|expr| defaults.expand(expr, errors));
        }
        select_calls(
            body,
            CompileContext::new(*options),
            &indices,
            &contexts,
            &mut selected,
            &mut pending,
        );
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
    for (body, options) in &mut roots {
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
                context.options(contexts.host),
                errors,
                contexts
                    .callee(&def.name, None, CompileContext::new(contexts.host))
                    .options(contexts.host),
            );
            rewrite(&mut def.body, context);
            def.compile_context = Some(context);
            def.name = names[&key].clone();
            def
        })
        .collect();
    MaterializedRuntime {
        defs,
        aliases,
        const_arrays: artifacts.const_values.runtime_arrays(&array_names),
    }
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
