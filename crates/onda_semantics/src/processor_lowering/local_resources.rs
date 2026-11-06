//! Lexical processor helpers retain resources needed by borrowed descendants.

use super::*;

pub(super) fn captures(proc: &ProcessorDef, proc_symbols: &HashSet<String>) -> HashSet<String> {
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
            let processor = match expr {
                Expr::UserCall { name, .. } => proc_symbols.contains(name),
                Expr::ArrayCtor { spec, .. } => matches!(&spec.elem,
                    ArrayElemType::Struct(name) if proc_symbols.contains(name)),
                Expr::ArrayLiteral { values, .. } => values.iter().any(|value|
                    matches!(value, Expr::UserCall { name, .. } if proc_symbols.contains(name))),
                _ => false,
            };
            processor.then_some(name.as_str())
        })
        .collect::<HashSet<_>>();
    let mut captures = HashSet::new();
    let local_names = proc
        .local_defs
        .iter()
        .flat_map(|def| {
            [
                (def.name.clone(), def.name.as_str()),
                (
                    proc_local_hidden_def_name(&proc.name, &def.name),
                    def.name.as_str(),
                ),
            ]
        })
        .collect::<HashMap<_, _>>();
    let mut callers = HashMap::<&str, HashSet<&str>>::new();
    let mut pending = Vec::new();
    for def in &proc.local_defs {
        let shadows = def
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<HashSet<_>>();
        let mut borrows = is_generated_proc_delegate_local(proc, &def.name);
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
                        if let Some(callee) = local_names.get(name) {
                            callers.entry(callee).or_default().insert(&def.name);
                        }
                    }
                    true
                })
            });
        }
        if borrows {
            captures.insert(def.name.clone());
            pending.push(def.name.as_str());
        }
    }
    while let Some(callee) = pending.pop() {
        for caller in callers.get(callee).into_iter().flatten() {
            if captures.insert((*caller).to_owned()) {
                pending.push(caller);
            }
        }
    }
    captures
}

pub(super) fn parameter_name(def: &FunctionDef, name: &str, index: usize) -> String {
    let mut names = def
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<HashSet<_>>();
    for statement in &def.body {
        statement.visit_statements(|statement| match statement {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            }
            | Stmt::For { var: name, .. } => {
                names.insert(name.clone());
            }
            Stmt::Assign {
                target: AssignTarget::Tuple(targets),
                ..
            } => {
                names.extend(
                    targets
                        .iter()
                        .filter_map(|target| target.binding())
                        .map(str::to_owned),
                );
            }
            _ => {}
        });
    }
    let mut result = name.to_owned();
    if names.contains(name) {
        result = format!("__onda_proc_owner_resource_{index}");
        while names.contains(result.as_str()) {
            result.push('_');
        }
    }
    result
}

pub(super) fn parameters(def: &FunctionDef, specs: &[TypedBufferDecl]) -> Vec<TypedBufferDecl> {
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
