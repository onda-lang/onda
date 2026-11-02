//! Lexical processor helpers retain resources needed by borrowed descendants.

use super::*;

pub(super) fn captures(proc: &ProcessorDef) -> HashSet<String> {
    let roots = proc
        .init
        .iter()
        .filter_map(|statement| {
            let Stmt::Assign {
                target: AssignTarget::Var(name),
                expr,
                ..
            } = statement
            else {
                return None;
            };
            matches!(
                expr,
                Expr::UserCall { .. } | Expr::ArrayCtor { .. } | Expr::ArrayLiteral { .. }
            )
            .then_some(name.as_str())
        })
        .collect::<HashSet<_>>();
    let mut captures = HashSet::new();
    let mut dependencies = HashMap::<String, HashSet<String>>::new();
    for def in &proc.local_defs {
        let shadows = def
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<HashSet<_>>();
        let mut borrows = is_generated_proc_delegate_local(proc, &def.name);
        let calls = dependencies.entry(def.name.clone()).or_default();
        for statement in &def.body {
            statement.visit_exprs(|expr| {
                expr.visit(|expr| {
                    let reference = match expr {
                        Expr::Var { name, .. } | Expr::UserCall { name, .. } => Some(name.as_str()),
                        Expr::Index { base, .. } | Expr::Slice { base, .. } => Some(base.as_str()),
                        _ => None,
                    };
                    if let Some(reference) = reference {
                        let root = reference
                            .strip_prefix("self.")
                            .unwrap_or(reference)
                            .split('.')
                            .next()
                            .unwrap_or(reference);
                        borrows |= reference == "self"
                            || (!shadows.contains(root) && roots.contains(root));
                    }
                    if let Expr::UserCall { name, .. } = expr {
                        for callee in &proc.local_defs {
                            if *name == callee.name
                                || *name == proc_local_hidden_def_name(&proc.name, &callee.name)
                            {
                                calls.insert(callee.name.clone());
                            }
                        }
                    }
                    true
                })
            });
        }
        if borrows {
            captures.insert(def.name.clone());
        }
    }
    loop {
        let before = captures.len();
        for (caller, callees) in &dependencies {
            if callees.iter().any(|callee| captures.contains(callee)) {
                captures.insert(caller.clone());
            }
        }
        if captures.len() == before {
            return captures;
        }
    }
}

pub(super) fn parameter_name(def: &FunctionDef, name: &str, index: usize) -> String {
    if def.params.iter().any(|param| param.name == name) {
        format!("__onda_proc_owner_resource_{index}")
    } else {
        name.to_owned()
    }
}

pub(super) fn parameters(def: &FunctionDef, specs: &[ProcBufferSpec]) -> Vec<ProcBufferSpec> {
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let mut spec = spec.clone();
            spec.name = parameter_name(def, &spec.name, index);
            spec
        })
        .collect()
}
