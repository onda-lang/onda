use std::collections::HashMap;

use onda_frontend::{
    AssignTarget, Block, Diagnostic, EventDef, EventParamDecl, FunctionDef, Program, Span, Stmt,
    WhenDef,
};

const TOP_LEVEL_OWNER: &str = "the top-level owner";

struct Callable {
    kind: &'static str,
    loc: Span,
}

type Callables<'a> = HashMap<&'a str, Vec<Callable>>;

fn symbol_parts(name: &str) -> (&str, &str) {
    name.rsplit_once("::").unwrap_or(("", name))
}

fn insert_callable<'a>(
    callables: &mut Callables<'a>,
    kind: &'static str,
    name: &'a str,
    loc: Span,
) {
    let candidates = callables.entry(name).or_default();
    if candidates
        .iter()
        .all(|callable| !callable.loc.shares_source_file(loc))
    {
        candidates.push(Callable { kind, loc });
    }
}

fn reject_binding(
    binding_kind: &str,
    name: &str,
    loc: Span,
    owner: &str,
    callables: &Callables<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(callable) = callables.get(name).and_then(|candidates| {
        candidates
            .iter()
            .find(|callable| callable.loc.shares_source_file(loc))
    }) else {
        return;
    };
    errors.push(Diagnostic::semantic_span(
        format!(
            "{binding_kind} '{name}' conflicts with owner-local {} '{}' in {owner}",
            callable.kind, name
        ),
        loc.as_ref(),
    ));
}

fn validate_stmts(
    stmts: &[Stmt],
    owner: &str,
    callables: &Callables<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => {
                reject_binding(
                    "local constant",
                    &decl.name,
                    decl.loc,
                    owner,
                    callables,
                    errors,
                );
            }
            Stmt::Assign {
                target, target_loc, ..
            } => match target {
                AssignTarget::Var(name) => {
                    reject_binding("binding", name, *target_loc, owner, callables, errors);
                }
                AssignTarget::Tuple(names) => {
                    for name in names.iter().filter_map(|target| target.binding()) {
                        reject_binding(
                            "tuple binding",
                            name,
                            *target_loc,
                            owner,
                            callables,
                            errors,
                        );
                    }
                }
                AssignTarget::Index { .. } | AssignTarget::Slice { .. } => {}
            },
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                validate_stmts(then_branch, owner, callables, errors);
                validate_stmts(else_branch, owner, callables, errors);
            }
            Stmt::For { loc, var, body, .. } => {
                reject_binding("loop variable", var, *loc, owner, callables, errors);
                validate_stmts(body, owner, callables, errors);
            }
            Stmt::While { body, .. } => validate_stmts(body, owner, callables, errors),
            Stmt::Expr { .. }
            | Stmt::Print { .. }
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
}

fn validate_function(
    def: &FunctionDef,
    owner: &str,
    callables: &Callables<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_binding(
            "function parameter",
            &param.name,
            param.loc,
            owner,
            callables,
            errors,
        );
    }
    validate_stmts(&def.body, owner, callables, errors);
}

fn validate_event_params(
    params: &[EventParamDecl],
    binding_kind: &str,
    owner: &str,
    callables: &Callables<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in params {
        reject_binding(
            binding_kind,
            &param.name,
            param.loc,
            owner,
            callables,
            errors,
        );
    }
}

fn validate_event(
    event: &EventDef,
    owner: &str,
    callables: &Callables<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    validate_event_params(&event.params, "event parameter", owner, callables, errors);
    validate_stmts(&event.body, owner, callables, errors);
}

fn validate_when(
    when: &WhenDef,
    owner: &str,
    callables: &Callables<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for binding in &when.bindings {
        if binding.name != "_" {
            reject_binding(
                "when binding",
                &binding.name,
                binding.loc,
                owner,
                callables,
                errors,
            );
        }
    }
    validate_stmts(&when.body, owner, callables, errors);
}

fn namespace_owner(namespace: &str) -> String {
    if namespace.is_empty() {
        TOP_LEVEL_OWNER.to_owned()
    } else {
        format!("namespace '{namespace}'")
    }
}

pub(crate) fn validate_owner_callable_bindings(program: &Program, errors: &mut Vec<Diagnostic>) {
    let mut namespaces = HashMap::<&str, Callables<'_>>::new();
    for block in &program.blocks {
        let (kind, full_name) = match block {
            Block::Def(def) => ("function", def.name.as_str()),
            Block::Proc(proc) => ("processor", proc.name.as_str()),
            Block::Struct(def) => ("struct", def.name.as_str()),
            _ => continue,
        };
        let (namespace, name) = symbol_parts(full_name);
        insert_callable(
            namespaces.entry(namespace).or_default(),
            kind,
            name,
            block.loc().span(),
        );
    }
    {
        let root = namespaces.entry("").or_default();
        for block in &program.blocks {
            match block {
                Block::Events(events) => {
                    for event in &events.events {
                        insert_callable(root, "event", &event.name, event.loc);
                    }
                }
                Block::Delegates(delegates) => {
                    for delegate in &delegates.delegates {
                        insert_callable(root, "delegate", &delegate.name, delegate.loc);
                    }
                }
                Block::Tasks(tasks) => {
                    for task in &tasks.tasks {
                        insert_callable(root, "task", &task.name, task.loc);
                    }
                }
                _ => {}
            }
        }
    }
    let root = &namespaces[""];

    for block in &program.blocks {
        match block {
            Block::Def(def) => {
                let (namespace, _) = symbol_parts(&def.name);
                let owner = namespace_owner(namespace);
                validate_function(def, &owner, &namespaces[namespace], errors);
            }
            Block::Events(events) => {
                for event in &events.events {
                    validate_event(event, TOP_LEVEL_OWNER, root, errors);
                }
            }
            Block::Delegates(delegates) => {
                for delegate in &delegates.delegates {
                    validate_event_params(
                        &delegate.params,
                        "delegate parameter",
                        TOP_LEVEL_OWNER,
                        root,
                        errors,
                    );
                }
            }
            Block::When(when) => {
                validate_when(when, TOP_LEVEL_OWNER, root, errors);
            }
            Block::Tasks(tasks) => {
                for task in &tasks.tasks {
                    validate_stmts(&task.body, TOP_LEVEL_OWNER, root, errors);
                }
            }
            Block::Init(init) => {
                validate_stmts(&init.body, TOP_LEVEL_OWNER, root, errors);
            }
            Block::Block(block) => {
                validate_stmts(&block.pre, TOP_LEVEL_OWNER, root, errors);
                if let Some(sample) = &block.sample {
                    validate_stmts(&sample.body, TOP_LEVEL_OWNER, root, errors);
                }
                validate_stmts(&block.post, TOP_LEVEL_OWNER, root, errors);
            }
            Block::Sample(sample) => {
                validate_stmts(&sample.body, TOP_LEVEL_OWNER, root, errors);
            }
            Block::Proc(proc) => {
                let owner = format!("processor '{}'", proc.name);
                let mut callables = Callables::new();
                for def in &proc.local_defs {
                    insert_callable(&mut callables, "function", &def.name, def.loc);
                }
                for event in &proc.events {
                    insert_callable(&mut callables, "event", &event.name, event.loc);
                }
                for delegate in &proc.delegates {
                    insert_callable(&mut callables, "delegate", &delegate.name, delegate.loc);
                }
                for task in &proc.tasks {
                    insert_callable(&mut callables, "task", &task.name, task.loc);
                }
                for def in &proc.local_defs {
                    validate_function(def, &owner, &callables, errors);
                }
                for event in &proc.events {
                    validate_event(event, &owner, &callables, errors);
                }
                for delegate in &proc.delegates {
                    validate_event_params(
                        &delegate.params,
                        "delegate parameter",
                        &owner,
                        &callables,
                        errors,
                    );
                }
                for when in &proc.whens {
                    validate_when(when, &owner, &callables, errors);
                }
                for task in &proc.tasks {
                    validate_stmts(&task.body, &owner, &callables, errors);
                }
                validate_stmts(&proc.init.body, &owner, &callables, errors);
                validate_stmts(&proc.block_pre, &owner, &callables, errors);
                validate_stmts(&proc.sample, &owner, &callables, errors);
                validate_stmts(&proc.block_post, &owner, &callables, errors);
            }
            // Methods are receiver-qualified (`self.method()`), so their names
            // do not occupy the bare callable namespace of the method body.
            Block::Struct(_) => {}
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Const(_)
            | Block::Buffers(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_)
            | Block::Graph(_) => {}
        }
    }
}
