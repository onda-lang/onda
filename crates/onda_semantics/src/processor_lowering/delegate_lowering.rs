use super::*;
use crate::internal_names::METHOD_RECEIVER_ARG;
use onda_frontend::{Span, TaskDef};
use std::collections::BTreeMap;

pub(super) const DELEGATE_ROUTE_FIELD: &str = "__onda_delegate_route";
pub(super) const DELEGATE_INDEX_FIELD: &str = "__onda_delegate_index";
const PUBLISH_DELEGATE_PREFIX: &str = "__onda_publish_delegate_";
const TOP_WHEN_PREFIX: &str = "__onda_when_";
const SELF_WHEN_PREFIX: &str = "__onda_delegate_self_when_";
const PROC_CHILD_WHEN_PREFIX: &str = "__onda_delegate_child_when_";
const WHEN_INDEX_PARAM: &str = "__onda_when_index";
const WHEN_PAYLOAD_PARAM_PREFIX: &str = "__onda_when_payload_";

#[derive(Debug, Clone)]
pub(super) struct PreparedDelegates {
    pub(super) top_level: Vec<DelegateDef>,
    pub(super) runtime_defs: HashSet<String>,
}

pub(crate) fn publish_delegate_index(name: &str) -> Option<usize> {
    name.strip_prefix(PUBLISH_DELEGATE_PREFIX)?.parse().ok()
}

fn call_stmt(name: String, params: &[EventParamDecl]) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name,
            type_args: Vec::new(),
            args: params
                .iter()
                .map(|param| CallArg {
                    name: None,
                    expr: Expr::var(param.name.clone()),
                })
                .collect(),
        },
    }
}

fn delegate_function(name: String, delegate: &DelegateDef, body: Vec<Stmt>) -> FunctionDef {
    FunctionDef {
        loc: delegate.loc,
        name,
        is_const: false,
        type_params: Vec::new(),
        params: delegate
            .params
            .iter()
            .map(crate::event_param_as_fn_param)
            .collect(),
        return_ty: None,
        return_ty_loc: Default::default(),
        body,
    }
}

fn when_handler_function(
    name: String,
    when: &WhenDef,
    delegate: &DelegateDef,
    takes_index: bool,
    owner: &str,
    errors: &mut Vec<Diagnostic>,
) -> FunctionDef {
    let mut params = Vec::new();
    let leading = if takes_index {
        params.push(onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: WHEN_INDEX_PARAM.to_owned(),
            ty: Some(FnParamType::Primitive(PrimitiveType::I32)),
            ty_loc: Default::default(),
            default: None,
        });
        Some(Expr::var(WHEN_INDEX_PARAM))
    } else {
        None
    };
    let payload_params = delegate
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let mut param = crate::event_param_as_fn_param(param);
            param.name = format!("{WHEN_PAYLOAD_PARAM_PREFIX}{index}");
            param
        })
        .collect::<Vec<_>>();
    params.extend(payload_params.iter().cloned());
    FunctionDef {
        loc: when.loc,
        name,
        is_const: false,
        type_params: Vec::new(),
        params,
        return_ty: None,
        return_ty_loc: Default::default(),
        body: validate_and_bind_when(when, delegate, leading, &payload_params, owner, errors),
    }
}

fn when_handler_call(name: String, delegate: &DelegateDef, leading_index: Option<Expr>) -> Stmt {
    let mut args = Vec::with_capacity(delegate.params.len() + usize::from(leading_index.is_some()));
    if let Some(index) = leading_index {
        args.push(CallArg {
            name: None,
            expr: index,
        });
    }
    args.extend(delegate.params.iter().map(|param| CallArg {
        name: None,
        expr: Expr::var(param.name.clone()),
    }));
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name,
            type_args: Vec::new(),
            args,
        },
    }
}

fn self_when_handler_name(ordinal: usize) -> String {
    format!("{SELF_WHEN_PREFIX}{ordinal}")
}

fn proc_child_when_handler_name(ordinal: usize) -> String {
    format!("{PROC_CHILD_WHEN_PREFIX}{ordinal}")
}

fn validate_and_bind_when(
    when: &WhenDef,
    delegate: &DelegateDef,
    leading_index: Option<Expr>,
    payload_params: &[onda_frontend::FnParamDecl],
    owner: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    let expected = delegate.params.len() + usize::from(leading_index.is_some());
    if when.bindings.len() != expected {
        push_semantic(
            DiagCtx::new(when.loc),
            errors,
            format!(
                "when handler for '{owner}.{}' expects {expected} bindings, got {}",
                delegate.name,
                when.bindings.len()
            ),
        );
        return Vec::new();
    }

    let mut seen = HashSet::new();
    for binding in &when.bindings {
        if binding.name != "_" && !seen.insert(binding.name.clone()) {
            push_semantic(
                DiagCtx::new(binding.loc),
                errors,
                format!("duplicate when payload binding '{}'", binding.name),
            );
        }
    }

    let mut replacements = HashMap::<String, Expr>::new();
    let mut offset = 0;
    if let Some(index) = leading_index {
        let binding = &when.bindings[0];
        if binding.name != "_" {
            replacements.insert(binding.name.clone(), index);
        }
        offset = 1;
    }
    for (binding, param) in when.bindings[offset..].iter().zip(payload_params) {
        if binding.name != "_" {
            replacements.insert(binding.name.clone(), Expr::var(param.name.clone()));
        }
    }

    let mut body = when.body.clone();
    for stmt in &mut body {
        replace_when_bindings_stmt(stmt, &replacements, errors);
    }
    body
}

fn replace_name(name: &mut String, replacements: &HashMap<String, Expr>) {
    let Some(Expr::Var {
        name: replacement, ..
    }) = replacements.get(name)
    else {
        return;
    };
    *name = replacement.clone();
}

fn replace_when_bindings_expr(expr: &mut Expr, replacements: &HashMap<String, Expr>) {
    match expr {
        Expr::Var { name, .. } => {
            if let Some(replacement) = replacements.get(name) {
                *expr = replacement.clone().with_loc(expr.loc());
            }
        }
        Expr::Index { base, index, .. } => {
            replace_name(base, replacements);
            replace_when_bindings_expr(index, replacements);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            replace_name(base, replacements);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                replace_when_bindings_expr(coordinate, replacements);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            replace_when_bindings_expr(&mut spec.size, replacements);
            if let Some(values) = init {
                for value in values {
                    replace_when_bindings_expr(value, replacements);
                }
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                replace_when_bindings_expr(arg, replacements);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                replace_when_bindings_expr(&mut arg.expr, replacements);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            replace_when_bindings_expr(lhs, replacements);
            replace_when_bindings_expr(rhs, replacements);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            replace_when_bindings_expr(expr, replacements)
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                replace_when_bindings_expr(value, replacements);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn replace_when_bindings_stmt(
    stmt: &mut Stmt,
    replacements: &HashMap<String, Expr>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target,
            target_loc,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if replacements.contains_key(name) {
                        push_semantic(
                            DiagCtx::new(*target_loc),
                            errors,
                            format!("cannot assign to read-only when payload binding '{name}'"),
                        );
                    }
                    replace_name(name, replacements);
                }
                AssignTarget::Index { base, index } => {
                    if replacements.contains_key(base) {
                        push_semantic(
                            DiagCtx::new(*target_loc),
                            errors,
                            format!("cannot write through read-only when payload binding '{base}'"),
                        );
                    }
                    replace_name(base, replacements);
                    replace_when_bindings_expr(index, replacements);
                }
                AssignTarget::Slice {
                    base,
                    selector,
                    channel,
                    start,
                    end,
                } => {
                    if replacements.contains_key(base) {
                        push_semantic(
                            DiagCtx::new(*target_loc),
                            errors,
                            format!("cannot write through read-only when payload binding '{base}'"),
                        );
                    }
                    replace_name(base, replacements);
                    for coordinate in [selector, channel, start, end].into_iter().flatten() {
                        replace_when_bindings_expr(coordinate, replacements);
                    }
                }
                AssignTarget::Tuple(names) => {
                    for name in names {
                        if replacements.contains_key(name) {
                            push_semantic(
                                DiagCtx::new(*target_loc),
                                errors,
                                format!("cannot assign to read-only when payload binding '{name}'"),
                            );
                        }
                        replace_name(name, replacements);
                    }
                }
            }
            replace_when_bindings_expr(expr, replacements);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            replace_when_bindings_expr(expr, replacements)
        }
        Stmt::Print { values, .. } => {
            for value in values {
                replace_when_bindings_expr(value, replacements);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            replace_when_bindings_expr(cond, replacements);
            for nested in then_branch.iter_mut().chain(else_branch) {
                replace_when_bindings_stmt(nested, replacements, errors);
            }
        }
        Stmt::For {
            loc,
            var,
            start,
            end,
            step,
            body,
            ..
        } => {
            if replacements.contains_key(var) {
                push_semantic(
                    DiagCtx::new(*loc),
                    errors,
                    format!("loop variable '{var}' conflicts with a when payload binding"),
                );
            }
            replace_when_bindings_expr(start, replacements);
            replace_when_bindings_expr(end, replacements);
            if let Some(step) = step {
                replace_when_bindings_expr(step, replacements);
            }
            for nested in body {
                replace_when_bindings_stmt(nested, replacements, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            replace_when_bindings_expr(cond, replacements);
            for nested in body {
                replace_when_bindings_stmt(nested, replacements, errors);
            }
        }
        Stmt::Const { loc, decl } => {
            if replacements.contains_key(&decl.name) {
                push_semantic(
                    DiagCtx::new(*loc),
                    errors,
                    format!(
                        "const '{}' conflicts with a when payload binding",
                        decl.name
                    ),
                );
            }
            replace_when_bindings_expr(&mut decl.expr, replacements);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceCallableKind {
    Def,
    Event,
    Delegate,
    When,
}

struct SourceCallable<'a> {
    label: String,
    name: Option<&'a str>,
    kind: SourceCallableKind,
    loc: Span,
    body: &'a [Stmt],
}

struct DelegateOwner<'a> {
    label: &'a str,
    delegates: Vec<&'a DelegateDef>,
    events: Vec<&'a EventDef>,
    defs: Vec<&'a FunctionDef>,
    whens: Vec<&'a WhenDef>,
    tasks: Vec<&'a TaskDef>,
    init: &'a [Stmt],
    executable_bodies: Vec<&'a [Stmt]>,
}

#[derive(Clone)]
struct SourceCall {
    name: String,
    loc: Span,
    statement_root: bool,
    indexed_receiver: Option<(String, Expr)>,
    args: Vec<CallArg>,
}

impl SourceCall {
    fn local_name(&self) -> Option<&str> {
        (self.indexed_receiver.is_none() && !self.name.contains('.')).then_some(self.name.as_str())
    }

    fn qualified_target(&self) -> Option<(&str, &str, Option<&Expr>)> {
        if let Some((base, index)) = &self.indexed_receiver {
            return Some((base, &self.name, Some(index)));
        }
        let (receiver, member) = self.name.rsplit_once('.')?;
        Some((receiver, member, None))
    }
}

fn collect_source_calls_expr(expr: &Expr, statement_root: bool, calls: &mut Vec<SourceCall>) {
    match expr {
        Expr::UserCall {
            loc, name, args, ..
        } => {
            let indexed_receiver = args.iter().find_map(|arg| {
                if arg.name.as_deref() != Some(METHOD_RECEIVER_ARG) {
                    return None;
                }
                let Expr::Index { base, index, .. } = &arg.expr else {
                    return None;
                };
                Some((base.clone(), index.as_ref().clone()))
            });
            calls.push(SourceCall {
                name: name.clone(),
                loc: *loc,
                statement_root,
                indexed_receiver,
                args: args.clone(),
            });
            for arg in args {
                collect_source_calls_expr(&arg.expr, false, calls);
            }
        }
        Expr::Index { index, .. } => collect_source_calls_expr(index, false, calls),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_source_calls_expr(coordinate, false, calls);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_source_calls_expr(&spec.size, false, calls);
            if let Some(values) = init {
                for value in values {
                    collect_source_calls_expr(value, false, calls);
                }
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_source_calls_expr(arg, false, calls);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_source_calls_expr(lhs, false, calls);
            collect_source_calls_expr(rhs, false, calls);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_source_calls_expr(expr, false, calls);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_source_calls_expr(value, false, calls);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn collect_source_calls(stmts: &[Stmt]) -> Vec<SourceCall> {
    fn visit(stmts: &[Stmt], calls: &mut Vec<SourceCall>) {
        for stmt in stmts {
            match stmt {
                Stmt::Const { decl, .. } => {
                    collect_source_calls_expr(&decl.expr, false, calls);
                }
                Stmt::Assign { target, expr, .. } => {
                    match target {
                        AssignTarget::Index { index, .. } => {
                            collect_source_calls_expr(index, false, calls);
                        }
                        AssignTarget::Slice {
                            selector,
                            channel,
                            start,
                            end,
                            ..
                        } => {
                            for coordinate in [selector, channel, start, end].into_iter().flatten()
                            {
                                collect_source_calls_expr(coordinate, false, calls);
                            }
                        }
                        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
                    }
                    collect_source_calls_expr(expr, false, calls);
                }
                Stmt::Expr { expr, .. } => collect_source_calls_expr(expr, true, calls),
                Stmt::Return { expr, .. } => collect_source_calls_expr(expr, false, calls),
                Stmt::Print { values, .. } => {
                    for value in values {
                        collect_source_calls_expr(value, false, calls);
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    collect_source_calls_expr(cond, false, calls);
                    visit(then_branch, calls);
                    visit(else_branch, calls);
                }
                Stmt::For {
                    step,
                    start,
                    end,
                    body,
                    ..
                } => {
                    collect_source_calls_expr(start, false, calls);
                    collect_source_calls_expr(end, false, calls);
                    if let Some(step) = step {
                        collect_source_calls_expr(step, false, calls);
                    }
                    visit(body, calls);
                }
                Stmt::While { cond, body, .. } => {
                    collect_source_calls_expr(cond, false, calls);
                    visit(body, calls);
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
        }
    }

    let mut calls = Vec::new();
    visit(stmts, &mut calls);
    calls
}

fn collect_delegate_value_uses_expr(
    expr: &Expr,
    delegate_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var { loc, name } if delegate_names.contains(name) => {
            errors.push(Diagnostic::semantic_span(
                format!("delegate '{name}' is callable only and cannot be used as a value"),
                *loc,
            ));
        }
        Expr::Index { index, .. } => {
            collect_delegate_value_uses_expr(index, delegate_names, errors)
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_delegate_value_uses_expr(coordinate, delegate_names, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_delegate_value_uses_expr(&spec.size, delegate_names, errors);
            if let Some(values) = init {
                for value in values {
                    collect_delegate_value_uses_expr(value, delegate_names, errors);
                }
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_delegate_value_uses_expr(arg, delegate_names, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                collect_delegate_value_uses_expr(&arg.expr, delegate_names, errors);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_delegate_value_uses_expr(lhs, delegate_names, errors);
            collect_delegate_value_uses_expr(rhs, delegate_names, errors);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_delegate_value_uses_expr(expr, delegate_names, errors)
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_delegate_value_uses_expr(value, delegate_names, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn validate_delegate_uses(
    stmts: &[Stmt],
    delegate_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    fn visit_expr(expr: &Expr, names: &HashSet<String>, errors: &mut Vec<Diagnostic>) {
        collect_delegate_value_uses_expr(expr, names, errors);
    }
    fn visit(stmts: &[Stmt], names: &HashSet<String>, errors: &mut Vec<Diagnostic>) {
        for stmt in stmts {
            match stmt {
                Stmt::Const { decl, .. } => visit_expr(&decl.expr, names, errors),
                Stmt::Assign { target, expr, .. } => {
                    match target {
                        AssignTarget::Index { index, .. } => visit_expr(index, names, errors),
                        AssignTarget::Slice {
                            selector,
                            channel,
                            start,
                            end,
                            ..
                        } => {
                            for coordinate in [selector, channel, start, end].into_iter().flatten()
                            {
                                visit_expr(coordinate, names, errors);
                            }
                        }
                        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
                    }
                    visit_expr(expr, names, errors);
                }
                Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                    visit_expr(expr, names, errors)
                }
                Stmt::Print { values, .. } => {
                    for value in values {
                        visit_expr(value, names, errors);
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    visit_expr(cond, names, errors);
                    visit(then_branch, names, errors);
                    visit(else_branch, names, errors);
                }
                Stmt::For {
                    step,
                    start,
                    end,
                    body,
                    ..
                } => {
                    visit_expr(start, names, errors);
                    visit_expr(end, names, errors);
                    if let Some(step) = step {
                        visit_expr(step, names, errors);
                    }
                    visit(body, names, errors);
                }
                Stmt::While { cond, body, .. } => {
                    visit_expr(cond, names, errors);
                    visit(body, names, errors);
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
        }
    }

    for call in collect_source_calls(stmts) {
        let Some(name) = call.local_name() else {
            continue;
        };
        if delegate_names.contains(name) && !call.statement_root {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "delegate call '{}' has no result and must be used as a statement",
                    name
                ),
                call.loc,
            ));
        }
    }
    visit(stmts, delegate_names, errors);
}

fn task_reset_names(stmts: &[Stmt], task_names: &HashSet<String>) -> HashSet<String> {
    collect_source_calls(stmts)
        .into_iter()
        .filter_map(|call| {
            let (receiver, method, index) = call.qualified_target()?;
            (index.is_none() && method == "reset" && task_names.contains(receiver))
                .then(|| receiver.to_owned())
        })
        .collect()
}

fn source_callables<'a>(owner: &'a DelegateOwner<'a>) -> Vec<SourceCallable<'a>> {
    let mut callables = Vec::new();
    callables.extend(owner.defs.iter().map(|def| SourceCallable {
        label: format!("def {}", def.name),
        name: Some(def.name.as_str()),
        kind: SourceCallableKind::Def,
        loc: def.loc,
        body: &def.body,
    }));
    callables.extend(owner.events.iter().map(|event| SourceCallable {
        label: format!("event {}", event.name),
        name: Some(event.name.as_str()),
        kind: SourceCallableKind::Event,
        loc: event.loc,
        body: &event.body,
    }));
    callables.extend(owner.delegates.iter().map(|delegate| SourceCallable {
        label: format!("delegate {}", delegate.name),
        name: Some(delegate.name.as_str()),
        kind: SourceCallableKind::Delegate,
        loc: delegate.loc,
        body: &[],
    }));
    callables.extend(
        owner
            .whens
            .iter()
            .enumerate()
            .map(|(ordinal, when)| SourceCallable {
                label: format!("{} #{}", when_target_label(when), ordinal + 1),
                name: None,
                kind: SourceCallableKind::When,
                loc: when.loc,
                body: &when.body,
            }),
    );
    callables
}

fn when_target_label(when: &WhenDef) -> String {
    let mut target = when.target.receiver.join(".");
    if when.target.index.is_some() {
        target.push_str("[...]");
    }
    if !target.is_empty() {
        target.push('.');
    }
    target.push_str(&when.target.delegate);
    format!("when {target}")
}

#[derive(Clone, Default)]
struct ProcDelegateEffects {
    event_delegates: HashMap<String, HashSet<String>>,
}

#[derive(Clone)]
struct ChildReceiverRef {
    receiver: String,
    index: Option<Expr>,
}

struct OwnerDispatchGraph<'a> {
    callables: Vec<SourceCallable<'a>>,
    by_name: HashMap<String, usize>,
    edges: Vec<Vec<usize>>,
    init_roots: Vec<usize>,
    task_roots: HashMap<String, Vec<usize>>,
}

fn quiet_const_index(expr: &Expr, options: AnalysisOptions) -> Option<i64> {
    eval_const_expr_i64_exact(expr, options, "delegate dispatch index", &mut Vec::new())
}

fn when_matches_child_dispatch(
    when: &WhenDef,
    receiver: &str,
    call_index: Option<&Expr>,
    child: &ChildProcInstance,
    delegate: &str,
    options: AnalysisOptions,
) -> bool {
    if when.target.receiver.as_slice() != [receiver] || when.target.delegate != delegate {
        return false;
    }
    if !child.is_array {
        return call_index.is_none() && when.target.index.is_none();
    }
    let Some(target_index) = &when.target.index else {
        return call_index.is_some();
    };
    let Some(call_index) = call_index else {
        return false;
    };
    match (
        quiet_const_index(target_index, options),
        quiet_const_index(call_index, options),
    ) {
        (Some(target), Some(actual)) => target == actual,
        // An unresolved call selector is runtime-selected and can reach any
        // statically selected element. Invalid subscription selectors are
        // diagnosed by the ordinary `when` target validator.
        _ => true,
    }
}

fn child_receiver_ref(
    expr: &Expr,
    aliases: &HashMap<String, ChildReceiverRef>,
    children: &HashMap<String, ChildProcInstance>,
) -> Option<ChildReceiverRef> {
    match expr {
        Expr::Var { name, .. } => aliases.get(name).cloned().or_else(|| {
            children.get(name).map(|_| ChildReceiverRef {
                receiver: name.clone(),
                index: None,
            })
        }),
        Expr::Index { base, index, .. } => {
            let mut resolved = aliases
                .get(base)
                .cloned()
                .unwrap_or_else(|| ChildReceiverRef {
                    receiver: base.clone(),
                    index: None,
                });
            let child = children.get(&resolved.receiver)?;
            if !child.is_array || resolved.index.is_some() {
                return None;
            }
            resolved.index = Some(index.as_ref().clone());
            Some(resolved)
        }
        _ => None,
    }
}

fn bind_child_receiver_aliases(
    def: &FunctionDef,
    call: &SourceCall,
    aliases: &HashMap<String, ChildReceiverRef>,
    children: &HashMap<String, ChildProcInstance>,
) -> HashMap<String, ChildReceiverRef> {
    let mut positional = call.args.iter().filter(|arg| arg.name.is_none());
    let mut bound = HashMap::new();
    for param in &def.params {
        let argument = call
            .args
            .iter()
            .find(|arg| arg.name.as_deref() == Some(param.name.as_str()))
            .or_else(|| positional.next());
        let Some(argument) = argument else {
            continue;
        };
        if let Some(receiver) = child_receiver_ref(&argument.expr, aliases, children) {
            bound.insert(param.name.clone(), receiver);
        }
    }
    bound
}

#[allow(clippy::too_many_arguments)]
fn source_call_targets<'a>(
    call: &SourceCall,
    owner: &'a DelegateOwner<'a>,
    children: &HashMap<String, ChildProcInstance>,
    proc_effects: &HashMap<String, ProcDelegateEffects>,
    options: AnalysisOptions,
    by_name: &HashMap<String, usize>,
    callables: &mut Vec<SourceCallable<'a>>,
    edges: &mut Vec<Vec<usize>>,
    aliases: &HashMap<String, ChildReceiverRef>,
    expanding_defs: &mut HashSet<String>,
) -> Vec<usize> {
    if let Some(name) = call.local_name() {
        let mut targets = by_name.get(name).copied().into_iter().collect::<Vec<_>>();
        let Some(def) = owner.defs.iter().find(|def| def.name == name) else {
            return targets;
        };
        let bound = bind_child_receiver_aliases(def, call, aliases, children);
        if bound.is_empty() || !expanding_defs.insert(def.name.clone()) {
            return targets;
        }
        for nested in collect_source_calls(&def.body) {
            targets.extend(source_call_targets(
                &nested,
                owner,
                children,
                proc_effects,
                options,
                by_name,
                callables,
                edges,
                &bound,
                expanding_defs,
            ));
        }
        expanding_defs.remove(&def.name);
        return targets;
    }
    let Some((receiver, event, call_index)) = call.qualified_target() else {
        return Vec::new();
    };
    let (receiver, call_index) = if let Some(alias) = aliases.get(receiver) {
        if alias.index.is_some() && call_index.is_some() {
            return Vec::new();
        }
        (alias.receiver.as_str(), alias.index.as_ref().or(call_index))
    } else {
        (receiver, call_index)
    };
    let Some(child) = children.get(receiver) else {
        return Vec::new();
    };
    let Some(delegates) = proc_effects
        .get(&child.proc_name)
        .and_then(|effects| effects.event_delegates.get(event))
    else {
        return Vec::new();
    };

    let mut delegates = delegates.iter().collect::<Vec<_>>();
    delegates.sort();
    let when_offset = owner.defs.len() + owner.events.len() + owner.delegates.len();
    let receiver_label = if child.is_array {
        match call_index.and_then(|index| quiet_const_index(index, options)) {
            Some(index) => format!("{receiver}[{index}]"),
            None => format!("{receiver}[...]"),
        }
    } else {
        receiver.to_owned()
    };
    let mut targets = Vec::with_capacity(delegates.len());
    for delegate in delegates {
        let node = callables.len();
        callables.push(SourceCallable {
            label: format!(
                "delegate {}.{delegate} through {receiver_label}",
                child.proc_name
            ),
            name: None,
            kind: SourceCallableKind::Delegate,
            loc: call.loc,
            body: &[],
        });
        edges.push(
            owner
                .whens
                .iter()
                .enumerate()
                .filter_map(|(ordinal, when)| {
                    when_matches_child_dispatch(
                        when, receiver, call_index, child, delegate, options,
                    )
                    .then_some(when_offset + ordinal)
                })
                .collect(),
        );
        targets.push(node);
    }
    targets
}

fn owner_dispatch_graph<'a>(
    owner: &'a DelegateOwner<'a>,
    children: &HashMap<String, ChildProcInstance>,
    proc_effects: &HashMap<String, ProcDelegateEffects>,
    options: AnalysisOptions,
) -> OwnerDispatchGraph<'a> {
    let mut callables = source_callables(owner);
    let by_name = callables
        .iter()
        .enumerate()
        .filter_map(|(index, callable)| callable.name.map(|name| (name.to_owned(), index)))
        .collect::<HashMap<_, _>>();
    let base_calls = callables
        .iter()
        .map(|callable| collect_source_calls(callable.body))
        .collect::<Vec<_>>();
    let base_len = callables.len();
    let mut edges = vec![Vec::new(); base_len];
    let aliases = HashMap::new();
    let mut expanding_defs = HashSet::new();
    let when_offset = owner.defs.len() + owner.events.len() + owner.delegates.len();
    for index in 0..base_len {
        if callables[index].kind == SourceCallableKind::Delegate {
            let Some(delegate_name) = callables[index].name else {
                continue;
            };
            for (ordinal, when) in owner.whens.iter().enumerate() {
                if when.target.receiver.is_empty() && when.target.delegate == delegate_name {
                    edges[index].push(when_offset + ordinal);
                }
            }
            continue;
        }
        for call in &base_calls[index] {
            let targets = source_call_targets(
                call,
                owner,
                children,
                proc_effects,
                options,
                &by_name,
                &mut callables,
                &mut edges,
                &aliases,
                &mut expanding_defs,
            );
            edges[index].extend(targets);
        }
    }

    let mut init_roots = Vec::new();
    for call in collect_source_calls(owner.init) {
        init_roots.extend(source_call_targets(
            &call,
            owner,
            children,
            proc_effects,
            options,
            &by_name,
            &mut callables,
            &mut edges,
            &aliases,
            &mut expanding_defs,
        ));
    }
    let mut task_roots = HashMap::new();
    for task in &owner.tasks {
        let mut roots = Vec::new();
        for call in collect_source_calls(&task.body) {
            roots.extend(source_call_targets(
                &call,
                owner,
                children,
                proc_effects,
                options,
                &by_name,
                &mut callables,
                &mut edges,
                &aliases,
                &mut expanding_defs,
            ));
        }
        task_roots.insert(task.name.clone(), roots);
    }

    OwnerDispatchGraph {
        callables,
        by_name,
        edges,
        init_roots,
        task_roots,
    }
}

fn find_dispatch_cycle(
    callables: &[SourceCallable<'_>],
    edges: &[Vec<usize>],
) -> Option<Vec<usize>> {
    fn visit(
        node: usize,
        callables: &[SourceCallable<'_>],
        edges: &[Vec<usize>],
        marks: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        marks[node] = 1;
        stack.push(node);
        for &next in &edges[node] {
            if marks[next] == 0 {
                if let Some(cycle) = visit(next, callables, edges, marks, stack) {
                    return Some(cycle);
                }
            } else if marks[next] == 1 {
                let start = stack.iter().position(|&entry| entry == next).unwrap_or(0);
                let mut cycle = stack[start..].to_vec();
                cycle.push(next);
                if cycle.iter().any(|&entry| {
                    matches!(
                        callables[entry].kind,
                        SourceCallableKind::Delegate
                            | SourceCallableKind::When
                            | SourceCallableKind::Event
                    )
                }) {
                    return Some(cycle);
                }
            }
        }
        stack.pop();
        marks[node] = 2;
        None
    }

    let mut marks = vec![0; callables.len()];
    let mut stack = Vec::new();
    for node in 0..callables.len() {
        if marks[node] == 0 {
            if let Some(cycle) = visit(node, callables, edges, &mut marks, &mut stack) {
                return Some(cycle);
            }
        }
    }
    None
}

fn path_to_delegate(
    roots: impl IntoIterator<Item = usize>,
    callables: &[SourceCallable<'_>],
    edges: &[Vec<usize>],
    owner_only: bool,
) -> Option<Vec<usize>> {
    fn visit(
        node: usize,
        callables: &[SourceCallable<'_>],
        edges: &[Vec<usize>],
        owner_only: bool,
        seen: &mut HashSet<usize>,
        path: &mut Vec<usize>,
    ) -> bool {
        if !seen.insert(node) {
            return false;
        }
        path.push(node);
        if callables[node].kind == SourceCallableKind::Delegate
            && (!owner_only || callables[node].name.is_some())
        {
            return true;
        }
        for &next in &edges[node] {
            if visit(next, callables, edges, owner_only, seen, path) {
                return true;
            }
        }
        path.pop();
        false
    }

    for root in roots {
        let mut seen = HashSet::new();
        let mut path = Vec::new();
        if visit(root, callables, edges, owner_only, &mut seen, &mut path) {
            return Some(path);
        }
    }
    None
}

fn task_dispatch_resets_itself(
    task: &TaskDef,
    roots: &[usize],
    callables: &[SourceCallable<'_>],
    edges: &[Vec<usize>],
    resets: &[HashSet<String>],
) -> bool {
    fn visit(
        node: usize,
        task: &str,
        saw_delegate: bool,
        callables: &[SourceCallable<'_>],
        edges: &[Vec<usize>],
        resets: &[HashSet<String>],
        seen: &mut HashSet<(usize, bool)>,
    ) -> bool {
        let saw_delegate = saw_delegate || callables[node].kind == SourceCallableKind::Delegate;
        if !seen.insert((node, saw_delegate)) {
            return false;
        }
        if saw_delegate && resets[node].contains(task) {
            return true;
        }
        edges[node]
            .iter()
            .copied()
            .any(|next| visit(next, task, saw_delegate, callables, edges, resets, seen))
    }

    let mut seen = HashSet::new();
    roots
        .iter()
        .copied()
        .any(|root| visit(root, &task.name, false, callables, edges, resets, &mut seen))
}

fn validate_owner_dispatch(
    owner: &DelegateOwner<'_>,
    children: &HashMap<String, ChildProcInstance>,
    proc_effects: &HashMap<String, ProcDelegateEffects>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashSet<String> {
    let delegate_names = owner
        .delegates
        .iter()
        .map(|delegate| delegate.name.clone())
        .collect::<HashSet<_>>();
    for body in owner
        .executable_bodies
        .iter()
        .copied()
        .chain(std::iter::once(owner.init))
        .chain(owner.defs.iter().map(|def| def.body.as_slice()))
        .chain(owner.events.iter().map(|event| event.body.as_slice()))
        .chain(owner.whens.iter().map(|when| when.body.as_slice()))
        .chain(owner.tasks.iter().map(|task| task.body.as_slice()))
    {
        validate_delegate_uses(body, &delegate_names, errors);
    }

    let graph = owner_dispatch_graph(owner, children, proc_effects, options);
    let callables = &graph.callables;
    let edges = &graph.edges;
    if let Some(cycle) = find_dispatch_cycle(callables, edges) {
        let path = cycle
            .iter()
            .map(|&node| callables[node].label.as_str())
            .collect::<Vec<_>>()
            .join(" -> ");
        errors.push(Diagnostic::semantic_span(
            format!(
                "recursive event/delegate dispatch in {} is not realtime-safe: {path}",
                owner.label
            ),
            callables[cycle[0]].loc,
        ));
    }

    if let Some(path) = path_to_delegate(graph.init_roots.iter().copied(), callables, edges, false)
    {
        let loc = callables[path[0]].loc;
        let path = path
            .iter()
            .map(|&node| callables[node].label.as_str())
            .collect::<Vec<_>>()
            .join(" -> ");
        errors.push(Diagnostic::semantic_span(
            format!(
                "init code in {} cannot call or reach a delegate: init -> {path}",
                owner.label
            ),
            loc,
        ));
    }

    let task_names = owner
        .tasks
        .iter()
        .map(|task| task.name.clone())
        .collect::<HashSet<_>>();
    let resets = callables
        .iter()
        .map(|callable| task_reset_names(callable.body, &task_names))
        .collect::<Vec<_>>();
    for task in &owner.tasks {
        let roots = graph
            .task_roots
            .get(&task.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if task_dispatch_resets_itself(task, roots, callables, edges, &resets) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "task '{}' in {} cannot dispatch a delegate whose synchronous handler may reset that active task",
                    task.name, owner.label
                ),
                task.loc,
            ));
        }
    }

    owner
        .defs
        .iter()
        .filter_map(|def| {
            let root = graph.by_name.get(&def.name).copied()?;
            path_to_delegate([root], callables, edges, true).map(|_| def.name.clone())
        })
        .collect()
}

fn insert_member(
    members: &mut HashMap<String, &'static str>,
    name: &str,
    kind: &'static str,
    owner: &str,
    loc: Span,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(previous) = members.get(name) {
        errors.push(Diagnostic::semantic_span(
            format!("delegate '{name}' conflicts with {previous} '{name}' in {owner}"),
            loc,
        ));
    } else {
        members.insert(name.to_owned(), kind);
    }
}

#[derive(Clone)]
struct ChildProcInstance {
    proc_name: String,
    is_array: bool,
}

fn child_proc_instances(
    init: &[Stmt],
    proc_names: &HashSet<String>,
) -> HashMap<String, ChildProcInstance> {
    let mut instances = HashMap::new();
    for stmt in init {
        let Stmt::Assign {
            target: AssignTarget::Var(instance),
            expr,
            ..
        } = stmt
        else {
            continue;
        };
        let resolved = match expr {
            Expr::UserCall {
                name: proc_name, ..
            } if proc_names.contains(proc_name) => Some((proc_name.clone(), false)),
            Expr::ArrayCtor { spec, .. } => match &spec.elem {
                ArrayElemType::Struct(proc_name) if proc_names.contains(proc_name) => {
                    Some((proc_name.clone(), true))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((proc_name, is_array)) = resolved {
            instances.insert(
                instance.clone(),
                ChildProcInstance {
                    proc_name,
                    is_array,
                },
            );
        }
    }
    instances
}

fn validate_qualified_delegate_calls(
    owner: &str,
    body: &[Stmt],
    children: &HashMap<String, ChildProcInstance>,
    proc_delegates: &HashMap<String, HashSet<String>>,
    errors: &mut Vec<Diagnostic>,
) {
    for call in collect_source_calls(body) {
        let Some((receiver, member, _)) = call.qualified_target() else {
            continue;
        };
        let root_end = receiver.find(['[', '.']).unwrap_or(receiver.len());
        let root = &receiver[..root_end];
        let Some(child) = children.get(root) else {
            continue;
        };
        if proc_delegates
            .get(&child.proc_name)
            .is_some_and(|delegates| delegates.contains(member))
        {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{owner} cannot call delegate '{member}' through child receiver '{receiver}'; only processor '{}' may call its delegate",
                    child.proc_name
                ),
                call.loc,
            ));
        }
    }
}

fn validate_delegate_member_names(program: &Program, errors: &mut Vec<Diagnostic>) {
    let mut top_members = HashMap::<String, &'static str>::new();
    for block in &program.blocks {
        match block {
            Block::Def(def) => {
                top_members.insert(def.name.clone(), "top-level def");
            }
            Block::Proc(proc) => {
                top_members.insert(proc.name.clone(), "processor");
            }
            Block::Struct(def) => {
                top_members.insert(def.name.clone(), "struct");
            }
            Block::Const(decl) => {
                top_members.insert(decl.name.clone(), "constant");
            }
            Block::Events(events) => {
                for event in &events.events {
                    top_members.insert(event.name.clone(), "event");
                }
            }
            Block::Tasks(tasks) => {
                for task in &tasks.tasks {
                    top_members.insert(task.name.clone(), "task");
                }
            }
            Block::Ins(ports) => {
                for port in &ports.decls {
                    top_members.insert(port.name.clone(), "input");
                }
            }
            Block::Outs(ports) | Block::KOuts(ports) => {
                for port in &ports.decls {
                    top_members.insert(port.name.clone(), "output");
                }
            }
            Block::Params(params) => {
                for param in &params.decls {
                    top_members.insert(param.name.clone(), "parameter");
                }
            }
            Block::Buffers(buffers) => {
                for buffer in &buffers.decls {
                    top_members.insert(buffer.name.clone(), "buffer");
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    if let Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } = stmt
                    {
                        top_members.entry(name.clone()).or_insert("state root");
                    }
                }
            }
            _ => {}
        }
    }
    for block in &program.blocks {
        if let Block::Delegates(delegates) = block {
            for delegate in &delegates.delegates {
                insert_member(
                    &mut top_members,
                    &delegate.name,
                    "delegate",
                    "the top-level owner",
                    delegate.loc,
                    errors,
                );
            }
        }
    }

    for proc in program.blocks.iter().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        let mut members = HashMap::<String, &'static str>::new();
        for def in &proc.local_defs {
            members.insert(def.name.clone(), "proc-local def");
        }
        for event in &proc.events {
            members.insert(event.name.clone(), "event");
        }
        for task in &proc.tasks {
            members.insert(task.name.clone(), "task");
        }
        for (name, kind) in proc
            .ins
            .iter()
            .map(|decl| (&decl.name, "input"))
            .chain(proc.outs.iter().map(|decl| (&decl.name, "output")))
            .chain(proc.params.iter().map(|decl| (&decl.name, "parameter")))
            .chain(proc.buffers.iter().map(|decl| (&decl.name, "buffer")))
            .chain(proc.consts.iter().map(|decl| (&decl.name, "constant")))
        {
            members.insert(name.clone(), kind);
        }
        for stmt in &proc.init.body {
            if let Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } = stmt
            {
                members.entry(name.clone()).or_insert("state root");
            }
        }
        for delegate in &proc.delegates {
            insert_member(
                &mut members,
                &delegate.name,
                "delegate",
                &format!("processor '{}'", proc.name),
                delegate.loc,
                errors,
            );
        }
    }
}

fn reachable_owner_delegates(
    root: usize,
    callables: &[SourceCallable<'_>],
    edges: &[Vec<usize>],
) -> HashSet<String> {
    let mut delegates = HashSet::new();
    let mut pending = vec![root];
    let mut seen = HashSet::new();
    while let Some(node) = pending.pop() {
        if !seen.insert(node) {
            continue;
        }
        let callable = &callables[node];
        if callable.kind == SourceCallableKind::Delegate {
            if let Some(name) = callable.name {
                delegates.insert(name.to_owned());
            }
        }
        pending.extend(edges[node].iter().copied());
    }
    delegates
}

fn build_proc_delegate_effects(
    program: &Program,
    options: AnalysisOptions,
) -> HashMap<String, ProcDelegateEffects> {
    fn build(
        proc_name: &str,
        proc_defs: &HashMap<String, &ProcessorDef>,
        proc_names: &HashSet<String>,
        options: AnalysisOptions,
        visiting: &mut HashSet<String>,
        effects: &mut HashMap<String, ProcDelegateEffects>,
    ) {
        if effects.contains_key(proc_name) || !visiting.insert(proc_name.to_owned()) {
            return;
        }
        let Some(proc) = proc_defs.get(proc_name).copied() else {
            visiting.remove(proc_name);
            return;
        };
        let children = child_proc_instances(&proc.init.body, proc_names);
        let mut child_types = children
            .values()
            .map(|child| child.proc_name.clone())
            .collect::<Vec<_>>();
        child_types.sort();
        child_types.dedup();
        for child in child_types {
            build(&child, proc_defs, proc_names, options, visiting, effects);
        }

        let label = format!("processor '{}'", proc.name);
        let owner = DelegateOwner {
            label: &label,
            delegates: proc.delegates.iter().collect(),
            events: proc.events.iter().collect(),
            defs: proc.local_defs.iter().collect(),
            whens: proc.whens.iter().collect(),
            tasks: proc.tasks.iter().collect(),
            init: &proc.init.body,
            executable_bodies: vec![&proc.block_pre, &proc.sample, &proc.block_post],
        };
        let graph = owner_dispatch_graph(&owner, &children, effects, options);
        let event_delegates = proc
            .events
            .iter()
            .filter_map(|event| {
                let root = graph.by_name.get(&event.name).copied()?;
                Some((
                    event.name.clone(),
                    reachable_owner_delegates(root, &graph.callables, &graph.edges),
                ))
            })
            .collect();
        effects.insert(
            proc_name.to_owned(),
            ProcDelegateEffects { event_delegates },
        );
        visiting.remove(proc_name);
    }

    let proc_defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc) => Some((proc.name.clone(), proc)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let proc_names = proc_defs.keys().cloned().collect::<HashSet<_>>();
    let mut names = proc_names.iter().cloned().collect::<Vec<_>>();
    names.sort();
    let mut visiting = HashSet::new();
    let mut effects = HashMap::new();
    for name in names {
        build(
            &name,
            &proc_defs,
            &proc_names,
            options,
            &mut visiting,
            &mut effects,
        );
    }
    effects
}

fn rewrite_delegate_validation_stmts(
    stmts: &mut [Stmt],
    env: &mut crate::def_semantics::CallTypeEnv,
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    return_types: &HashMap<String, ReturnType>,
) {
    let mut ignored_errors = Vec::new();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        stmts,
        env,
        crate::def_semantics::CallTypeContext {
            return_types,
            struct_defs: &HashMap::new(),
        },
        crate::def_semantics::OverloadOwnerContext {
            defer_dependent_calls: true,
        },
        overloads,
        &mut ignored_errors,
    );
}

fn rewrite_delegate_validation_function(
    def: &mut FunctionDef,
    env: &crate::def_semantics::CallTypeEnv,
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    return_types: &HashMap<String, ReturnType>,
) {
    let mut ignored_errors = Vec::new();
    crate::def_semantics::rewrite_overloaded_calls_in_function(
        def,
        env,
        crate::def_semantics::CallTypeContext {
            return_types,
            struct_defs: &HashMap::new(),
        },
        crate::def_semantics::OverloadOwnerContext {
            defer_dependent_calls: true,
        },
        overloads,
        &mut ignored_errors,
    );
}

fn delegate_validation_return_types(
    defs: &[FunctionDef],
    env: &crate::def_semantics::CallTypeEnv,
) -> HashMap<String, ReturnType> {
    let signatures = defs
        .iter()
        .map(|def| (def.name.clone(), FnSignature::from_def(def)))
        .collect::<HashMap<_, _>>();
    crate::def_semantics::infer_known_def_return_types_with_seed(
        defs,
        &signatures,
        env,
        &HashMap::new(),
        &HashMap::new(),
    )
}

fn rewrite_delegate_validation_event(
    event: &mut EventDef,
    state_env: &crate::def_semantics::CallTypeEnv,
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    return_types: &HashMap<String, ReturnType>,
) {
    let mut env = state_env.clone();
    for param in &event.params {
        env.bind_function_param(&crate::event_param_as_fn_param(param), &[]);
    }
    rewrite_delegate_validation_stmts(&mut event.body, &mut env, overloads, return_types);
}

fn rewrite_delegate_validation_task(
    task: &mut TaskDef,
    state_env: &crate::def_semantics::CallTypeEnv,
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    return_types: &HashMap<String, ReturnType>,
) {
    let mut env = state_env.clone();
    rewrite_delegate_validation_stmts(&mut task.body, &mut env, overloads, return_types);
}

fn rewrite_delegate_validation_when(
    when: &mut WhenDef,
    delegate: Option<&DelegateDef>,
    takes_index: bool,
    state_env: &crate::def_semantics::CallTypeEnv,
    overloads: &HashMap<String, Vec<crate::def_semantics::OverloadCandidate>>,
    return_types: &HashMap<String, ReturnType>,
) {
    let mut env = state_env.clone();
    let mut bindings = when.bindings.iter();
    if takes_index {
        if let Some(binding) = bindings.next() {
            env.bind_function_param_type(
                &binding.name,
                Some(&FnParamType::Primitive(PrimitiveType::I32)),
                &[],
            );
        }
    }
    if let Some(delegate) = delegate {
        for (binding, param) in bindings.zip(&delegate.params) {
            let param = crate::event_param_as_fn_param(param);
            env.bind_function_param_type(&binding.name, param.ty.as_ref(), &[]);
        }
    }
    rewrite_delegate_validation_stmts(&mut when.body, &mut env, overloads, return_types);
}

/// Delegate validation runs before the main overload pass because delegates
/// are desugared into ordinary functions. Resolve the calls on a validation
/// clone so the dispatch graph observes concrete overload identities without
/// changing the source program or duplicating overload-selection rules.
fn resolve_delegate_validation_overloads(program: &mut Program) {
    let proc_delegate_defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .flat_map(|proc| {
            proc.delegates
                .iter()
                .map(move |delegate| ((proc.name.clone(), delegate.name.clone()), delegate.clone()))
        })
        .collect::<HashMap<_, _>>();
    let proc_names = proc_delegate_defs
        .keys()
        .map(|(proc_name, _)| proc_name.clone())
        .collect::<HashSet<_>>();
    let top_delegate_defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Delegates(delegates) => Some(delegates.delegates.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|delegate| (delegate.name.clone(), delegate.clone()))
        .collect::<HashMap<_, _>>();
    let top_def_indices = program
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| matches!(block, Block::Def(_)).then_some(index))
        .collect::<Vec<_>>();
    let mut top_defs = top_def_indices
        .iter()
        .filter_map(|&index| match &program.blocks[index] {
            Block::Def(def) => Some(def.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (top_overloads, _) = crate::def_semantics::prepare_function_overloads(&mut top_defs);

    let mut top_state_env = crate::def_semantics::CallTypeEnv::default();
    let provisional_return_types = delegate_validation_return_types(&top_defs, &top_state_env);
    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|block| matches!(block, Block::Init(_)))
    {
        rewrite_delegate_validation_stmts(
            &mut init.body,
            &mut top_state_env,
            &top_overloads,
            &provisional_return_types,
        );
    }
    let top_return_types = delegate_validation_return_types(&top_defs, &top_state_env);
    for def in &mut top_defs {
        rewrite_delegate_validation_function(
            def,
            &top_state_env,
            &top_overloads,
            &top_return_types,
        );
    }
    for (index, def) in top_def_indices.into_iter().zip(top_defs) {
        program.blocks[index] = Block::Def(def);
    }

    let top_children = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(child_proc_instances(&init.body, &proc_names)),
            _ => None,
        })
        .unwrap_or_default();
    for block in &mut program.blocks {
        match block {
            Block::Events(events) => {
                for event in &mut events.events {
                    rewrite_delegate_validation_event(
                        event,
                        &top_state_env,
                        &top_overloads,
                        &top_return_types,
                    );
                }
            }
            Block::Tasks(tasks) => {
                for task in &mut tasks.tasks {
                    rewrite_delegate_validation_task(
                        task,
                        &top_state_env,
                        &top_overloads,
                        &top_return_types,
                    );
                }
            }
            Block::When(when) => {
                let child = when
                    .target
                    .receiver
                    .first()
                    .and_then(|receiver| top_children.get(receiver));
                let delegate = child
                    .and_then(|child| {
                        proc_delegate_defs
                            .get(&(child.proc_name.clone(), when.target.delegate.clone()))
                    })
                    .or_else(|| top_delegate_defs.get(&when.target.delegate));
                rewrite_delegate_validation_when(
                    when,
                    delegate,
                    child.is_some_and(|child| child.is_array && when.target.index.is_none()),
                    &top_state_env,
                    &top_overloads,
                    &top_return_types,
                );
            }
            Block::Sample(sample) => {
                let mut env = top_state_env.clone();
                rewrite_delegate_validation_stmts(
                    &mut sample.body,
                    &mut env,
                    &top_overloads,
                    &top_return_types,
                );
            }
            Block::Block(exec) => {
                for body in [
                    exec.pre.as_mut_slice(),
                    exec.sample
                        .as_mut()
                        .map(|sample| sample.body.as_mut_slice())
                        .unwrap_or(&mut []),
                    exec.post.as_mut_slice(),
                ] {
                    let mut env = top_state_env.clone();
                    rewrite_delegate_validation_stmts(
                        body,
                        &mut env,
                        &top_overloads,
                        &top_return_types,
                    );
                }
            }
            _ => {}
        }
    }

    for proc in program.blocks.iter_mut().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        let local_public_names = proc
            .local_defs
            .iter()
            .map(|def| def.name.clone())
            .collect::<HashSet<_>>();
        let (local_overloads, _) =
            crate::def_semantics::prepare_function_overloads(&mut proc.local_defs);
        let mut overloads = top_overloads
            .iter()
            .filter(|(name, _)| !local_public_names.contains(*name))
            .map(|(name, candidates)| (name.clone(), candidates.clone()))
            .collect::<HashMap<_, _>>();
        overloads.extend(local_overloads);

        let mut state_env = crate::def_semantics::CallTypeEnv::default();
        let provisional_return_types =
            delegate_validation_return_types(&proc.local_defs, &state_env);
        rewrite_delegate_validation_stmts(
            &mut proc.init.body,
            &mut state_env,
            &overloads,
            &provisional_return_types,
        );
        let mut return_types = top_return_types.clone();
        return_types.extend(delegate_validation_return_types(
            &proc.local_defs,
            &state_env,
        ));
        for def in &mut proc.local_defs {
            rewrite_delegate_validation_function(def, &state_env, &overloads, &return_types);
        }
        for event in &mut proc.events {
            rewrite_delegate_validation_event(event, &state_env, &overloads, &return_types);
        }
        for task in &mut proc.tasks {
            rewrite_delegate_validation_task(task, &state_env, &overloads, &return_types);
        }
        let children = child_proc_instances(&proc.init.body, &proc_names);
        for when in &mut proc.whens {
            let child = when
                .target
                .receiver
                .first()
                .and_then(|receiver| children.get(receiver));
            let delegate = child
                .and_then(|child| {
                    proc_delegate_defs.get(&(child.proc_name.clone(), when.target.delegate.clone()))
                })
                .or_else(|| {
                    proc_delegate_defs.get(&(proc.name.clone(), when.target.delegate.clone()))
                });
            rewrite_delegate_validation_when(
                when,
                delegate,
                child.is_some_and(|child| child.is_array && when.target.index.is_none()),
                &state_env,
                &overloads,
                &return_types,
            );
        }
        for body in [
            proc.block_pre.as_mut_slice(),
            proc.sample.as_mut_slice(),
            proc.block_post.as_mut_slice(),
        ] {
            let mut env = state_env.clone();
            rewrite_delegate_validation_stmts(body, &mut env, &overloads, &return_types);
        }
    }
}

fn delegate_validation_has_overloads(program: &Program) -> bool {
    fn has_duplicate_names<'a>(names: impl IntoIterator<Item = &'a str>) -> bool {
        let mut seen = HashSet::new();
        names.into_iter().any(|name| !seen.insert(name))
    }

    has_duplicate_names(program.blocks.iter().filter_map(|block| match block {
        Block::Def(def) => Some(def.name.as_str()),
        _ => None,
    })) || program.blocks.iter().any(|block| match block {
        Block::Proc(proc) => {
            has_duplicate_names(proc.local_defs.iter().map(|def| def.name.as_str()))
        }
        _ => false,
    })
}

pub(super) fn validate_delegate_source_model(
    program: &Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    validate_delegate_member_names(program, errors);
    let resolved_program = delegate_validation_has_overloads(program).then(|| {
        let mut resolved = program.clone();
        resolve_delegate_validation_overloads(&mut resolved);
        resolved
    });
    let program = resolved_program.as_ref().unwrap_or(program);
    let proc_effects = build_proc_delegate_effects(program, options);

    let proc_delegates = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc) => Some((
                proc.name.clone(),
                proc.delegates
                    .iter()
                    .map(|delegate| delegate.name.clone())
                    .collect::<HashSet<_>>(),
            )),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let proc_names = proc_delegates.keys().cloned().collect::<HashSet<_>>();

    let top_delegates = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Delegates(delegates) => Some(delegates.delegates.as_slice()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    let top_events = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Events(events) => Some(events.events.as_slice()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    let top_defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Def(def) => Some(def),
            _ => None,
        })
        .collect::<Vec<_>>();
    let top_whens = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::When(when) => Some(when),
            _ => None,
        })
        .collect::<Vec<_>>();
    let top_tasks = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Tasks(tasks) => Some(tasks.tasks.as_slice()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    let top_init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(init.body.as_slice()),
            _ => None,
        })
        .unwrap_or(&[]);
    let mut top_executable = Vec::new();
    for block in &program.blocks {
        match block {
            Block::Sample(sample) => top_executable.push(sample.body.as_slice()),
            Block::Block(exec) => {
                top_executable.push(exec.pre.as_slice());
                if let Some(sample) = &exec.sample {
                    top_executable.push(sample.body.as_slice());
                }
                top_executable.push(exec.post.as_slice());
            }
            _ => {}
        }
    }
    let top_children = child_proc_instances(top_init, &proc_names);
    let top_effectful_defs = validate_owner_dispatch(
        &DelegateOwner {
            label: "the top-level owner",
            delegates: top_delegates.clone(),
            events: top_events.clone(),
            defs: top_defs.clone(),
            whens: top_whens.clone(),
            tasks: top_tasks.clone(),
            init: top_init,
            executable_bodies: top_executable.clone(),
        },
        &top_children,
        &proc_effects,
        options,
        errors,
    );
    for body in std::iter::once(top_init)
        .chain(top_executable.iter().copied())
        .chain(top_defs.iter().map(|def| def.body.as_slice()))
        .chain(top_events.iter().map(|event| event.body.as_slice()))
        .chain(top_whens.iter().map(|when| when.body.as_slice()))
        .chain(top_tasks.iter().map(|task| task.body.as_slice()))
    {
        validate_qualified_delegate_calls(
            "the top-level owner",
            body,
            &top_children,
            &proc_delegates,
            errors,
        );
    }

    for proc in program.blocks.iter().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        let children = child_proc_instances(&proc.init.body, &proc_names);
        let local_names = proc
            .local_defs
            .iter()
            .map(|def| def.name.clone())
            .chain(proc.events.iter().map(|event| event.name.clone()))
            .chain(proc.delegates.iter().map(|delegate| delegate.name.clone()))
            .collect::<HashSet<_>>();
        for body in std::iter::once(proc.init.body.as_slice())
            .chain(std::iter::once(proc.block_pre.as_slice()))
            .chain(std::iter::once(proc.sample.as_slice()))
            .chain(std::iter::once(proc.block_post.as_slice()))
            .chain(proc.local_defs.iter().map(|def| def.body.as_slice()))
            .chain(proc.events.iter().map(|event| event.body.as_slice()))
            .chain(proc.whens.iter().map(|when| when.body.as_slice()))
            .chain(proc.tasks.iter().map(|task| task.body.as_slice()))
        {
            validate_qualified_delegate_calls(
                &format!("processor '{}'", proc.name),
                body,
                &children,
                &proc_delegates,
                errors,
            );
            for call in collect_source_calls(body) {
                let Some(name) = call.local_name() else {
                    continue;
                };
                if !local_names.contains(name) && top_effectful_defs.contains(name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{}' cannot call top-level def '{}' because it may publish a delegate owned by another owner",
                            proc.name, name
                        ),
                        call.loc,
                    ));
                }
            }
        }

        validate_owner_dispatch(
            &DelegateOwner {
                label: &format!("processor '{}'", proc.name),
                delegates: proc.delegates.iter().collect(),
                events: proc.events.iter().collect(),
                defs: proc.local_defs.iter().collect(),
                whens: proc.whens.iter().collect(),
                tasks: proc.tasks.iter().collect(),
                init: &proc.init.body,
                executable_bodies: vec![&proc.block_pre, &proc.sample, &proc.block_post],
            },
            &children,
            &proc_effects,
            options,
            errors,
        );
    }
}

fn take_top_level_delegates(program: &mut Program) -> Vec<DelegateDef> {
    let mut delegates = Vec::new();
    program.blocks.retain_mut(|block| {
        if let Block::Delegates(block) = block {
            delegates.append(&mut block.delegates);
            false
        } else {
            true
        }
    });
    delegates
}

pub(super) fn prepare_delegate_source(
    program: &mut Program,
    errors: &mut Vec<Diagnostic>,
) -> PreparedDelegates {
    let top_level = take_top_level_delegates(program);
    let mut runtime_defs = HashSet::new();

    let mut top_whens = Vec::new();
    program.blocks.retain_mut(|block| {
        let Block::When(when) = block else {
            return true;
        };
        if when.target.receiver.is_empty() && when.target.index.is_none() {
            top_whens.push(when.clone());
            false
        } else {
            true
        }
    });

    for (delegate_index, delegate) in top_level.iter().enumerate() {
        let publish_name = format!("{PUBLISH_DELEGATE_PREFIX}{delegate_index}");
        let publish = delegate_function(publish_name.clone(), delegate, Vec::new());
        runtime_defs.insert(publish_name);
        program.blocks.push(Block::Def(publish));

        let mut body = vec![call_stmt(
            format!("{PUBLISH_DELEGATE_PREFIX}{delegate_index}"),
            &delegate.params,
        )];
        for (ordinal, when) in top_whens
            .iter()
            .enumerate()
            .filter(|(_, when)| when.target.delegate == delegate.name)
        {
            let handler_name = self_when_handler_name(ordinal);
            runtime_defs.insert(handler_name.clone());
            program.blocks.push(Block::Def(when_handler_function(
                handler_name.clone(),
                when,
                delegate,
                false,
                "top-level",
                errors,
            )));
            body.push(when_handler_call(handler_name, delegate, None));
        }
        runtime_defs.insert(delegate.name.clone());
        program.blocks.push(Block::Def(delegate_function(
            delegate.name.clone(),
            delegate,
            body,
        )));
    }

    for when in &top_whens {
        if !top_level
            .iter()
            .any(|delegate| delegate.name == when.target.delegate)
        {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!(
                    "when target '{}' is not a top-level delegate",
                    when.target.delegate
                ),
            );
        }
    }

    let proc_delegate_defs = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc) => Some((proc.name.clone(), proc.delegates.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let proc_names = proc_delegate_defs.keys().cloned().collect::<HashSet<_>>();

    for block in &mut program.blocks {
        let Block::Proc(proc) = block else {
            continue;
        };
        if proc.delegates.is_empty() && proc.whens.is_empty() {
            continue;
        }
        let mut own_whens = Vec::new();
        proc.whens.retain(|when| {
            if when.target.receiver.is_empty() && when.target.index.is_none() {
                own_whens.push(when.clone());
                false
            } else {
                true
            }
        });
        let delegates = proc.delegates.clone();
        for delegate in &delegates {
            if proc.local_defs.iter().any(|def| def.name == delegate.name) {
                push_semantic(
                    DiagCtx::new(delegate.loc),
                    errors,
                    format!(
                        "delegate '{}' conflicts with proc-local def '{}' in processor '{}'",
                        delegate.name, delegate.name, proc.name
                    ),
                );
                continue;
            }
            let mut body = Vec::new();
            for (ordinal, when) in own_whens
                .iter()
                .enumerate()
                .filter(|(_, when)| when.target.delegate == delegate.name)
            {
                let handler_name = self_when_handler_name(ordinal);
                proc.local_defs.push(when_handler_function(
                    handler_name.clone(),
                    when,
                    delegate,
                    false,
                    &proc.name,
                    errors,
                ));
                body.push(when_handler_call(handler_name, delegate, None));
            }
            proc.local_defs
                .push(delegate_function(delegate.name.clone(), delegate, body));
        }
        for when in &own_whens {
            if !proc
                .delegates
                .iter()
                .any(|delegate| delegate.name == when.target.delegate)
            {
                push_semantic(
                    DiagCtx::new(when.target.loc),
                    errors,
                    format!(
                        "when target '{}' is not a delegate of processor '{}'",
                        when.target.delegate, proc.name
                    ),
                );
            }
        }

        let children = child_proc_instances(&proc.init.body, &proc_names);
        for (ordinal, when) in proc.whens.iter().enumerate() {
            if when.target.receiver.len() != 1 {
                continue;
            }
            let receiver = &when.target.receiver[0];
            let Some(child) = children.get(receiver) else {
                continue;
            };
            let Some(delegate) = proc_delegate_defs
                .get(&child.proc_name)
                .and_then(|delegates| {
                    delegates
                        .iter()
                        .find(|delegate| delegate.name == when.target.delegate)
                })
            else {
                continue;
            };
            let takes_index = child.is_array && when.target.index.is_none();
            proc.local_defs.push(when_handler_function(
                proc_child_when_handler_name(ordinal),
                when,
                delegate,
                takes_index,
                &proc.name,
                errors,
            ));
        }
    }

    PreparedDelegates {
        top_level,
        runtime_defs,
    }
}

pub(super) fn top_when_def_name(route: usize, ordinal: usize) -> String {
    format!("{TOP_WHEN_PREFIX}{route}_{ordinal}")
}

#[derive(Clone)]
struct RoutedHandler {
    delegate: String,
    function: String,
    takes_index: bool,
}

#[derive(Clone)]
struct PendingTopWhen {
    when: WhenDef,
    delegate: DelegateDef,
    function: String,
    takes_index: bool,
    slots: Vec<(String, i32)>,
}

fn top_when_function(pending: &PendingTopWhen, errors: &mut Vec<Diagnostic>) -> FunctionDef {
    when_handler_function(
        pending.function.clone(),
        &pending.when,
        &pending.delegate,
        pending.takes_index,
        "top-level child",
        errors,
    )
}

fn resolve_top_when(
    when: &WhenDef,
    ordinal: usize,
    meta: &TopLevelProcRewriteMeta,
    proc_defs: &HashMap<String, ProcessorDef>,
    proc_api: &HashMap<String, ProcApi>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<PendingTopWhen> {
    if when.target.receiver.len() != 1 {
        push_semantic(
            DiagCtx::new(when.target.loc),
            errors,
            "when subscriptions may cross only one direct child ownership boundary",
        );
        return None;
    }
    let receiver = &when.target.receiver[0];
    let (slots, takes_index) = if let Some(index) = &when.target.index {
        let Some(array_slots) = meta.global_proc_array_slots.get(receiver) else {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!("when target '{receiver}[...]' is not a direct processor array"),
            );
            return None;
        };
        let Some(index) = eval_const_expr_i64_exact(
            index,
            options,
            &format!("when target '{receiver}' index"),
            errors,
        ) else {
            push_semantic(
                DiagCtx::new(index.loc()),
                errors,
                "indexed when targets require a compile-time constant selector",
            );
            return None;
        };
        let Ok(index_usize) = usize::try_from(index) else {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!("when target index {index} is outside processor array '{receiver}'"),
            );
            return None;
        };
        let Some(slot) = array_slots.get(index_usize) else {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!(
                    "when target index {index} is outside processor array '{receiver}' of length {}",
                    array_slots.len()
                ),
            );
            return None;
        };
        (vec![(slot.clone(), index as i32)], false)
    } else if let Some(array_slots) = meta.global_proc_array_slots.get(receiver) {
        (
            array_slots
                .iter()
                .enumerate()
                .map(|(index, slot)| (slot.clone(), index as i32))
                .collect(),
            true,
        )
    } else if meta.global_proc_instances.contains_key(receiver) {
        (vec![(receiver.clone(), -1)], false)
    } else {
        push_semantic(
            DiagCtx::new(when.target.loc),
            errors,
            format!("when target '{receiver}' is not a direct child processor"),
        );
        return None;
    };

    let (first_slot, _) = slots.first()?;
    let Some(instance) = meta.global_proc_instances.get(first_slot) else {
        push_semantic(
            DiagCtx::new(when.target.loc),
            errors,
            format!("when target '{receiver}' has no resolved processor instance"),
        );
        return None;
    };
    let proc = proc_defs.get(&instance.proc_name)?;
    let api = proc_api.get(&instance.proc_name)?;
    if !api.delegates.contains_key(&when.target.delegate) {
        push_semantic(
            DiagCtx::new(when.target.loc),
            errors,
            format!(
                "processor '{}' has no delegate named '{}'",
                proc.name, when.target.delegate
            ),
        );
        return None;
    }
    let Some(delegate) = proc
        .delegates
        .iter()
        .find(|delegate| delegate.name == when.target.delegate)
        .cloned()
    else {
        push_semantic(
            DiagCtx::new(when.target.loc),
            errors,
            format!(
                "processor '{}' has no delegate named '{}'",
                proc.name, when.target.delegate
            ),
        );
        return None;
    };
    Some(PendingTopWhen {
        when: when.clone(),
        delegate,
        function: top_when_def_name(ordinal, 0),
        takes_index,
        slots,
    })
}

pub(super) fn lower_top_level_child_whens(
    program: &mut Program,
    meta: &TopLevelProcRewriteMeta,
    proc_defs: &mut HashMap<String, ProcessorDef>,
    proc_api: &HashMap<String, ProcApi>,
    options: AnalysisOptions,
    runtime_defs: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut source_whens = Vec::new();
    program.blocks.retain_mut(|block| {
        if let Block::When(when) = block {
            source_whens.push(when.clone());
            false
        } else {
            true
        }
    });

    let pending = source_whens
        .iter()
        .enumerate()
        .filter_map(|(ordinal, when)| {
            resolve_top_when(when, ordinal, meta, proc_defs, proc_api, options, errors)
        })
        .collect::<Vec<_>>();

    let mut slot_handlers = BTreeMap::<String, Vec<RoutedHandler>>::new();
    let mut slot_indices = BTreeMap::<String, i32>::new();
    for handler in &pending {
        runtime_defs.insert(handler.function.clone());
        program
            .blocks
            .push(Block::Def(top_when_function(handler, errors)));
        for (slot, index) in &handler.slots {
            slot_indices.insert(slot.clone(), *index);
            slot_handlers
                .entry(slot.clone())
                .or_default()
                .push(RoutedHandler {
                    delegate: handler.delegate.name.clone(),
                    function: handler.function.clone(),
                    takes_index: handler.takes_index,
                });
        }
    }

    let mut route_by_signature = BTreeMap::<Vec<(String, String, bool)>, i32>::new();
    let mut next_route = 1_i32;
    let mut slot_routes = BTreeMap::<String, i32>::new();
    for (slot, handlers) in &slot_handlers {
        let signature = handlers
            .iter()
            .map(|handler| {
                (
                    handler.delegate.clone(),
                    handler.function.clone(),
                    handler.takes_index,
                )
            })
            .collect::<Vec<_>>();
        let route = *route_by_signature.entry(signature).or_insert_with(|| {
            let route = next_route;
            next_route += 1;
            route
        });
        slot_routes.insert(slot.clone(), route);
    }

    for (signature, route) in &route_by_signature {
        let Some(slot) = slot_routes
            .iter()
            .find_map(|(slot, candidate)| (*candidate == *route).then_some(slot))
        else {
            continue;
        };
        let Some(instance) = meta.global_proc_instances.get(slot) else {
            continue;
        };
        let Some(proc) = proc_defs.get_mut(&instance.proc_name) else {
            continue;
        };
        for delegate in &proc.delegates {
            let matching = signature
                .iter()
                .filter(|(delegate_name, _, _)| delegate_name == &delegate.name)
                .collect::<Vec<_>>();
            if matching.is_empty() {
                continue;
            }
            let Some(dispatcher) = proc
                .local_defs
                .iter_mut()
                .find(|def| def.name == delegate.name)
            else {
                continue;
            };
            let mut route_body = Vec::new();
            for (_, function, takes_index) in matching {
                let mut args = Vec::new();
                if *takes_index {
                    args.push(CallArg {
                        name: None,
                        expr: Expr::var(DELEGATE_INDEX_FIELD),
                    });
                }
                args.extend(delegate.params.iter().map(|param| CallArg {
                    name: None,
                    expr: Expr::var(param.name.clone()),
                }));
                route_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
                        name: function.clone(),
                        type_args: Vec::new(),
                        args,
                    },
                });
            }
            dispatcher.body.push(Stmt::If {
                loc: Default::default(),
                cond: Expr::Compare {
                    loc: Default::default(),
                    op: CmpOp::Eq,
                    lhs: Box::new(Expr::var(DELEGATE_ROUTE_FIELD)),
                    rhs: Box::new(Expr::int(i64::from(*route))),
                },
                then_branch: route_body,
                else_branch: Vec::new(),
            });
        }
    }

    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|block| matches!(block, Block::Init(_)))
    {
        for (slot, route) in slot_routes {
            init.body.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: routed_field_target(
                    &slot,
                    DELEGATE_ROUTE_FIELD,
                    &meta.global_proc_array_slots,
                ),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int(i64::from(route)),
            });
            init.body.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: routed_field_target(
                    &slot,
                    DELEGATE_INDEX_FIELD,
                    &meta.global_proc_array_slots,
                ),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int(i64::from(*slot_indices.get(&slot).unwrap_or(&-1))),
            });
        }
    }
}

fn direct_proc_when_target(
    when: &WhenDef,
    shape: &ProcLoweringShape,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, Option<i32>)> {
    if when.target.receiver.len() != 1 {
        push_semantic(
            DiagCtx::new(when.target.loc),
            errors,
            "when subscriptions may cross only one direct child ownership boundary",
        );
        return None;
    }
    let receiver = &when.target.receiver[0];
    if let Some(index) = &when.target.index {
        let Some(slots) = shape.nested_proc_array_slots.get(receiver) else {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!("when target '{receiver}[...]' is not a direct processor array"),
            );
            return None;
        };
        let Some(index) = eval_const_expr_i64_exact(
            index,
            options,
            &format!("when target '{receiver}' index"),
            errors,
        ) else {
            push_semantic(
                DiagCtx::new(index.loc()),
                errors,
                "indexed when targets require a compile-time constant selector",
            );
            return None;
        };
        let Ok(index_usize) = usize::try_from(index) else {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!("when target index {index} is outside processor array '{receiver}'"),
            );
            return None;
        };
        let Some(slot) = slots.get(index_usize) else {
            push_semantic(
                DiagCtx::new(when.target.loc),
                errors,
                format!(
                    "when target index {index} is outside processor array '{receiver}' of length {}",
                    slots.len()
                ),
            );
            return None;
        };
        return Some((slot.clone(), None));
    }
    if shape.nested_proc_array_slots.contains_key(receiver) {
        return Some((receiver.clone(), Some(0)));
    }
    if shape.state.nested_procs.contains_key(receiver) {
        return Some((receiver.clone(), None));
    }
    push_semantic(
        DiagCtx::new(when.target.loc),
        errors,
        format!("when target '{receiver}' is not a direct child processor"),
    );
    None
}

pub(super) fn validate_proc_child_whens(
    proc_defs: &HashMap<String, ProcessorDef>,
    shapes: &HashMap<String, ProcLoweringShape>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for (proc_name, proc) in proc_defs {
        let Some(shape) = shapes.get(proc_name) else {
            continue;
        };
        for when in &proc.whens {
            let Some((resolved, whole_array)) =
                direct_proc_when_target(when, shape, options, errors)
            else {
                continue;
            };
            let child_slot = if whole_array.is_some() {
                shape
                    .nested_proc_array_slots
                    .get(&resolved)
                    .and_then(|slots| slots.first())
            } else {
                Some(&resolved)
            };
            let Some(child_slot) = child_slot else {
                continue;
            };
            let Some(child) = shape.state.nested_procs.get(child_slot) else {
                continue;
            };
            let Some(child_proc) = proc_defs.get(&child.proc_name) else {
                continue;
            };
            let Some(_delegate) = child_proc
                .delegates
                .iter()
                .find(|delegate| delegate.name == when.target.delegate)
            else {
                push_semantic(
                    DiagCtx::new(when.target.loc),
                    errors,
                    format!(
                        "processor '{}' has no delegate named '{}'",
                        child.proc_name, when.target.delegate
                    ),
                );
                continue;
            };
        }
    }
}

fn proc_array_slot_index(path: &str, base: &str) -> Option<i32> {
    let suffix = path
        .strip_prefix(base)?
        .strip_prefix('[')?
        .strip_suffix(']')?;
    suffix.parse().ok()
}

fn routed_field_target(
    slot: &str,
    field: &str,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> AssignTarget {
    if let Some((array, index)) = find_proc_array_slot(slot, proc_array_slots) {
        AssignTarget::Index {
            base: format!("{array}.{field}"),
            index: Expr::int(index as i64),
        }
    } else {
        AssignTarget::Var(format!("{slot}.{field}"))
    }
}

pub(super) fn proc_child_when_body(
    owner: &ProcessorDef,
    callee: &ProcessorDef,
    nested_path: &str,
    delegate_name: &str,
    options: AnalysisOptions,
) -> Vec<Stmt> {
    let Some(delegate) = callee
        .delegates
        .iter()
        .find(|delegate| delegate.name == delegate_name)
    else {
        return Vec::new();
    };
    let mut body = Vec::new();
    for (ordinal, when) in owner.whens.iter().enumerate() {
        if when.target.delegate != delegate_name || when.target.receiver.len() != 1 {
            continue;
        }
        let receiver = &when.target.receiver[0];
        let leading = if let Some(index_expr) = &when.target.index {
            let Some(index) = eval_const_expr_i64_exact(
                index_expr,
                options,
                &format!("when target '{receiver}' index"),
                &mut Vec::new(),
            ) else {
                continue;
            };
            if nested_path != format!("{receiver}[{index}]") {
                continue;
            }
            None
        } else if nested_path == receiver {
            None
        } else if let Some(index) = proc_array_slot_index(nested_path, receiver) {
            Some(Expr::int(i64::from(index)))
        } else {
            continue;
        };
        let handler = proc_child_when_handler_name(ordinal);
        if !owner.local_defs.iter().any(|def| def.name == handler) {
            continue;
        }
        body.push(when_handler_call(
            proc_local_hidden_def_name(&owner.name, &handler),
            delegate,
            leading,
        ));
    }
    body
}
