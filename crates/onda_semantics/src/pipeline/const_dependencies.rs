use super::*;

pub(super) fn referenced_const_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var { name, .. }
        | Expr::Index { base: name, .. }
        | Expr::Slice { base: name, .. } => Some(name),
        Expr::UserCall { name, args, .. } if args.is_empty() => parse_array_len_instance_base(name),
        _ => None,
    }
}

pub(super) fn visit_free_exprs(
    expr: &Expr,
    bound: &HashSet<String>,
    visitor: &mut impl FnMut(&Expr),
) {
    for expr in expr.walk() {
        if !referenced_const_name(expr).is_some_and(|name| bound.contains(name)) {
            visitor(expr);
        }
    }
}

/// Visit global references in evaluation order. Initializers see the preceding
/// bindings; branch-local bindings hide globals only within that branch.
pub(super) fn visit_free_stmts(
    stmts: &[Stmt],
    bound: &mut HashSet<String>,
    visitor: &mut impl FnMut(&Expr),
) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                expr,
                ..
            } => {
                if let Some(DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. }) =
                    decl_ty
                {
                    visit_free_exprs(size, bound, visitor);
                }
                target.visit_selectors(|expr| visit_free_exprs(expr, bound, visitor));
                visit_free_exprs(expr, bound, visitor);
                if let AssignTarget::Var(name) = target {
                    bound.insert(name.clone());
                }
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                visit_free_exprs(cond, bound, visitor);
                let mut then_bound = bound.clone();
                let mut else_bound = bound.clone();
                visit_free_stmts(then_branch, &mut then_bound, visitor);
                visit_free_stmts(else_branch, &mut else_bound, visitor);
                // Mutable bindings common to both paths can escape.
                bound.extend(then_bound.intersection(&else_bound).cloned());
            }
            Stmt::For {
                var,
                start,
                end,
                step,
                body,
                ..
            } => {
                visit_free_exprs(start, bound, visitor);
                visit_free_exprs(end, bound, visitor);
                if let Some(step) = step {
                    visit_free_exprs(step, bound, visitor);
                }
                let mut loop_bound = bound.clone();
                loop_bound.insert(var.clone());
                visit_free_stmts(body, &mut loop_bound, visitor);
            }
            Stmt::While { cond, body, .. } => {
                visit_free_exprs(cond, bound, visitor);
                visit_free_stmts(body, &mut bound.clone(), visitor);
            }
            _ => stmt.visit_exprs(|expr| visit_free_exprs(expr, bound, visitor)),
        }
    }
}

/// Capture referenced declarations without executing the body, honoring the
/// const evaluator's parameter and local bindings.
pub(super) fn visit_const_def_references(def: &FunctionDef, visitor: &mut impl FnMut(&Expr)) {
    let globals = HashSet::new();
    let mut bound = HashSet::new();
    for param in &def.params {
        if let Some(FnParamType::SizedArray { size, .. }) = &param.ty {
            visit_free_exprs(size, &globals, visitor);
        }
        if let Some(default) = &param.default {
            // Defaults use the declaration's lexical scope, before any
            // argument bindings are introduced.
            visit_free_exprs(default, &globals, visitor);
        }
        bound.insert(param.name.clone());
    }
    if let Some(FnReturnType::Array { size, .. }) = &def.return_ty {
        visit_free_exprs(size, &globals, visitor);
    }
    visit_free_stmts(&def.body, &mut bound, visitor);
}

/// Only symbols visible when the initializer is declared are captured. Keeping
/// its actual references preserves scope without copying the entire environment.
#[derive(Debug, Clone, Default)]
pub(super) struct ConstDependencies {
    pub values: Vec<String>,
    pub defs: Vec<String>,
}

struct DependencyCollector<'a> {
    artifacts: &'a SemanticConstArtifacts,
    names: HashSet<String>,
    defs: HashSet<String>,
}

impl<'a> DependencyCollector<'a> {
    fn new(artifacts: &'a SemanticConstArtifacts) -> Self {
        Self {
            artifacts,
            names: HashSet::new(),
            defs: HashSet::new(),
        }
    }

    fn finish(self) -> ConstDependencies {
        ConstDependencies {
            values: self.names.into_iter().collect(),
            defs: self.defs.into_iter().collect(),
        }
    }

    fn visit(&mut self, expr: &Expr) {
        if let Some(name) = referenced_const_name(expr).filter(|name| {
            self.artifacts.const_values.contains_key(name)
                || self.artifacts.const_array_infos.contains_key(*name)
        }) {
            self.names.insert(name.to_owned());
        }
        if let Expr::UserCall { name, .. } = expr {
            if self.artifacts.const_defs.contains_key(name) {
                self.defs.insert(name.clone());
            }
        }
    }
}

pub(super) fn const_dependencies_from_exprs<'a>(
    exprs: impl IntoIterator<Item = &'a Expr>,
    artifacts: &SemanticConstArtifacts,
) -> ConstDependencies {
    let mut dependencies = DependencyCollector::new(artifacts);
    for root in exprs {
        for expr in root.walk() {
            dependencies.visit(expr);
        }
    }
    dependencies.finish()
}

/// Each declaration captures only its direct references. A callee owns its own
/// lexical environment, so callers never duplicate its transitive dependencies.
pub(super) fn const_def_dependencies(
    def: &FunctionDef,
    artifacts: &SemanticConstArtifacts,
) -> ConstDependencies {
    let mut dependencies = DependencyCollector::new(artifacts);
    visit_const_def_references(def, &mut |expr| dependencies.visit(expr));
    dependencies.finish()
}
