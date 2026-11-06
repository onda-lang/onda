//! Propagate boundary-hook requirements through forwarding and lexical aliases.
use super::*;
use crate::def_semantics::call_types::{join_reachable_scopes, statement_flow, StatementFlow};

type Aliases = HashMap<String, HashSet<String>>;
type Requirements = HashMap<String, HashSet<usize>>;

struct HookDependencies<'a> {
    defs: HashMap<String, &'a TypedFunction>,
    proc_api: &'a HashMap<String, ProcApi>,
    requirements: Requirements,
}

fn origins(expression: &Expr, aliases: &Aliases) -> HashSet<String> {
    match expression {
        Expr::Var { name, .. } => aliases.get(name).cloned().unwrap_or_default(),
        Expr::Index { base, index, .. } => {
            if let Expr::Int { value, .. } = index.as_ref() {
                if let Some(origins) = aliases.get(&format!("{base}[{value}]")) {
                    return origins.clone();
                }
            }
            // An indexed child can require its enclosing scalar owner's hooks.
            aliases
                .get(base)
                .or_else(|| aliases.get(base.split('.').next().unwrap_or(base)))
                .cloned()
                .unwrap_or_default()
        }
        _ => HashSet::new(),
    }
}

fn merge_aliases(destination: &mut Aliases, source: Aliases) {
    for (name, origins) in source {
        destination.entry(name).or_default().extend(origins);
    }
}

impl HookDependencies<'_> {
    fn expression(&self, expression: &Expr, aliases: &Aliases, used: &mut HashSet<String>) {
        for expression in expression.walk() {
            let Expr::UserCall { name, args, .. } = expression else {
                continue;
            };
            if crate::processor_lowering::proc_name_for_lowered_proc_call(name, self.proc_api)
                .is_some_and(|name| self.proc_api[name].has_block)
            {
                if let Some(receiver) = args.first() {
                    used.extend(origins(&receiver.expr, aliases));
                }
            }
            let Some(callee) = self.defs.get(name) else {
                continue;
            };
            let Some(required) = self.requirements.get(name).filter(|set| !set.is_empty()) else {
                continue;
            };
            let resolved = resolve_call_args(
                args,
                &callee.params,
                &callee.param_defaults,
                false,
                false,
                &format!("call '{}(...)'", callee.name),
                &mut Vec::new(),
            );
            for index in required {
                if let Some(Some(argument)) = resolved.get(*index) {
                    used.extend(origins(argument, aliases));
                }
            }
        }
    }

    fn statements(
        &self,
        body: &[Stmt],
        aliases: &mut Aliases,
        used: &mut HashSet<String>,
    ) -> StatementFlow {
        for statement in body {
            match statement {
                Stmt::Assign {
                    target,
                    expr,
                    decl_ty,
                    is_typed_decl,
                    generic_decl_ty,
                    ..
                } => {
                    target.visit_selectors(|expr| self.expression(expr, aliases, used));
                    self.expression(expr, aliases, used);
                    if let AssignTarget::Var(name) = target {
                        let sources = if crate::stmt_analysis::data_declaration_owns_storage(
                            decl_ty.as_ref(),
                            *is_typed_decl || generic_decl_ty.is_some(),
                        ) {
                            HashSet::new()
                        } else {
                            origins(expr, aliases)
                        };
                        // Reassignment preserves existing storage. A fresh owned
                        // copy has no lifecycle dependency on its source.
                        aliases.entry(name.clone()).or_insert(sources);
                    }
                }
                Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                    self.expression(expr, aliases, used);
                }
                Stmt::Print { values, .. } => {
                    for expression in values {
                        self.expression(expression, aliases, used);
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.expression(cond, aliases, used);
                    let mut then_aliases = aliases.clone();
                    let mut else_aliases = aliases.clone();
                    let then_flow = self.statements(then_branch, &mut then_aliases, used);
                    let else_flow = self.statements(else_branch, &mut else_aliases, used);
                    let (joined, flow) = join_reachable_scopes(
                        then_aliases,
                        then_flow,
                        else_aliases,
                        else_flow,
                        merge_aliases,
                    );
                    *aliases = joined;
                    if flow == StatementFlow::Terminates {
                        return flow;
                    }
                    continue;
                }
                Stmt::For {
                    start,
                    end,
                    step,
                    body,
                    ..
                } => {
                    for expression in [Some(start), Some(end), step.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        self.expression(expression, aliases, used);
                    }
                    let mut body_aliases = aliases.clone();
                    self.statements(body, &mut body_aliases, used);
                    merge_aliases(aliases, body_aliases);
                }
                Stmt::While { cond, body, .. } => {
                    self.expression(cond, aliases, used);
                    let mut body_aliases = aliases.clone();
                    self.statements(body, &mut body_aliases, used);
                    merge_aliases(aliases, body_aliases);
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
            if statement_flow(statement) == StatementFlow::Terminates {
                return StatementFlow::Terminates;
            }
        }
        StatementFlow::Continues
    }
}

/// Constructor instances and retained owned copies have independent scalar
/// lifecycle identities. Untyped aliases continue to name their original root.
pub(super) fn scalar_hook_roots(
    init: &[Stmt],
    boundary: crate::processor_views::ProcessorBoundary,
    instances: &HashMap<String, ProcCallInstance>,
    views: &crate::processor_views::ProcessorViewCatalog,
) -> HashMap<String, String> {
    let mut roots = instances
        .iter()
        .filter(|(name, _)| {
            views
                .roots
                .get(*name)
                .is_some_and(|source| source.shape.is_scalar_root())
        })
        .map(|(name, instance)| (name.clone(), instance.proc_name.clone()))
        .collect::<HashMap<_, _>>();
    for statement in init {
        statement.visit_statements(|statement| {
            if let Stmt::Assign {
                target: AssignTarget::Var(name),
                is_typed_decl,
                generic_decl_ty,
                ..
            } = statement
            {
                if *is_typed_decl || generic_decl_ty.is_some() {
                    if let Some(shape) = views
                        .boundary_views
                        .get(&boundary)
                        .and_then(|views| views.get(name))
                        .filter(|shape| shape.is_scalar_root())
                    {
                        roots.insert(name.clone(), shape.proc_name.clone());
                    }
                }
            }
        });
    }
    roots
}

pub(super) fn sample_hook_instances(
    init: &[Stmt],
    block_pre: &[Stmt],
    sample: &[Stmt],
    defs_in_postorder: &[&TypedFunction],
    proc_api: &HashMap<String, ProcApi>,
    roots: &HashMap<String, String>,
    views: &crate::processor_views::ProcessorViewCatalog,
) -> std::collections::BTreeSet<String> {
    let mut dependencies = HookDependencies {
        defs: defs_in_postorder
            .iter()
            .map(|&def| (def.name.clone(), def))
            .collect(),
        proc_api,
        requirements: HashMap::new(),
    };
    // Recursion validation already ordered callees before callers. Each body
    // therefore sees complete dependency summaries on its first and only visit.
    for def in defs_in_postorder {
        let mut aliases = Aliases::new();
        for (name, kind) in def.params.iter().zip(&def.param_kinds) {
            if let TypedFnParam::Struct { struct_name } = kind {
                if proc_api.get(struct_name).is_some_and(|api| api.has_block)
                    && views
                        .function_param(&def.name, name)
                        .is_none_or(|shape| shape.is_scalar_root())
                {
                    aliases.insert(name.clone(), HashSet::from([name.clone()]));
                }
            }
        }
        let mut used = HashSet::new();
        dependencies.statements(&def.body, &mut aliases, &mut used);
        let required = def
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, name)| used.contains(name).then_some(index))
            .collect();
        dependencies.requirements.insert(def.name.clone(), required);
    }

    let mut aliases = roots
        .keys()
        .map(|name| (name.clone(), HashSet::from([name.clone()])))
        .collect();
    // Resolve retained aliases to the owning storage before discovering sample
    // dependencies, so direct and forwarded calls share one scheduled hook.
    dependencies.statements(init, &mut aliases, &mut HashSet::new());
    dependencies.statements(block_pre, &mut aliases, &mut HashSet::new());
    let mut used = HashSet::new();
    dependencies.statements(sample, &mut aliases, &mut used);
    used.into_iter().collect()
}
