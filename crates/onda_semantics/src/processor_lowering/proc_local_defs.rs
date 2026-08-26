use std::collections::{HashMap, HashSet};

use crate::push_semantic;
use onda_frontend::ast::*;
use onda_frontend::{DiagCtx, Diagnostic};

pub(super) const PROC_LOCAL_DEF_FN_PREFIX: &str = ".__onda_proc_local__";

pub(crate) fn proc_local_hidden_def_name(owner_proc: &str, local_name: &str) -> String {
    format!("{owner_proc}{PROC_LOCAL_DEF_FN_PREFIX}{local_name}")
}

pub(super) fn nested_wrapper_local_def_name(
    owner_proc: &str,
    nested_path: &str,
    local_name: &str,
) -> String {
    let nested_component = nested_path
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{owner_proc}{PROC_LOCAL_DEF_FN_PREFIX}nested__{nested_component}__{local_name}")
}

pub(super) fn unique_proc_local_defs(proc: &ProcessorDef) -> Vec<FunctionDef> {
    let mut seen = HashSet::<String>::new();
    let mut out = Vec::<FunctionDef>::new();
    for def in &proc.local_defs {
        if seen.insert(def.name.clone()) {
            out.push(def.clone());
        }
    }
    out
}

pub(super) fn pre_desugar_proc_local_hidden_def(
    owner_proc: &str,
    local_def: &FunctionDef,
) -> FunctionDef {
    FunctionDef {
        loc: Default::default(),
        is_const: false,
        type_params: local_def.type_params.clone(),
        name: proc_local_hidden_def_name(owner_proc, &local_def.name),
        params: local_def.params.clone(),
        return_ty: local_def.return_ty.clone(),
        return_ty_loc: local_def.return_ty_loc,
        body: local_def.body.clone(),
    }
}

pub(super) fn owner_proc_local_hidden_def(
    owner_proc: &str,
    local_def: &FunctionDef,
    body: Vec<Stmt>,
) -> FunctionDef {
    FunctionDef {
        loc: Default::default(),
        is_const: false,
        type_params: local_def.type_params.clone(),
        name: proc_local_hidden_def_name(owner_proc, &local_def.name),
        params: proc_local_hidden_def_params(owner_proc, local_def),
        return_ty: local_def.return_ty.clone(),
        return_ty_loc: local_def.return_ty_loc,
        body,
    }
}

pub(super) fn nested_wrapper_proc_local_hidden_def(
    owner_proc: &str,
    nested_path: &str,
    local_def: &FunctionDef,
    body: Vec<Stmt>,
) -> FunctionDef {
    FunctionDef {
        loc: Default::default(),
        is_const: false,
        type_params: local_def.type_params.clone(),
        name: nested_wrapper_local_def_name(owner_proc, nested_path, &local_def.name),
        params: proc_local_hidden_def_params(owner_proc, local_def),
        return_ty: local_def.return_ty.clone(),
        return_ty_loc: local_def.return_ty_loc,
        body,
    }
}

fn proc_local_hidden_def_params(owner_proc: &str, local_def: &FunctionDef) -> Vec<FnParamDecl> {
    let mut params = Vec::<FnParamDecl>::with_capacity(local_def.params.len() + 1);
    params.push(FnParamDecl {
        loc: Default::default(),
        name: "self".to_owned(),
        ty: Some(FnParamType::Struct(owner_proc.to_owned())),
        ty_loc: Default::default(),
        default: None,
    });
    params.extend(local_def.params.clone());
    params
}

pub(super) fn inject_owner_self_into_hidden_local_calls(stmt: &mut Stmt, owner_proc: &str) {
    inject_hidden_local_call_receiver(stmt, owner_proc, &Expr::var("self"));
}

pub(super) fn inject_hidden_local_call_receiver(
    stmt: &mut Stmt,
    owner_proc: &str,
    receiver: &Expr,
) {
    inject_owner_self_into_hidden_local_calls_in_stmt(stmt, owner_proc, receiver);
}

pub(super) fn rewrite_nested_wrapper_local_calls(
    stmt: &mut Stmt,
    callee_proc: &str,
    owner_proc: &str,
    nested_path: &str,
) {
    rewrite_nested_wrapper_local_calls_in_stmt(stmt, callee_proc, owner_proc, nested_path);
}

pub(crate) fn rewrite_proc_local_defs(proc: &mut ProcessorDef, errors: &mut Vec<Diagnostic>) {
    if proc.local_defs.is_empty() {
        return;
    }

    let mut def_map = HashMap::<String, FunctionDef>::new();
    for def in &proc.local_defs {
        if def_map.contains_key(&def.name) {
            push_semantic(
                DiagCtx::new(def.loc),
                errors,
                format!(
                    "processor '{}': duplicate proc-local def '{}'",
                    proc.name, def.name
                ),
            );
            continue;
        }
        def_map.insert(def.name.clone(), def.clone());
    }

    if let Some(cycle) = detect_cycles(&def_map) {
        push_semantic(
            DiagCtx::new(proc.loc),
            errors,
            format!(
                "processor '{}': recursive proc-local def cycle: {}",
                proc.name,
                cycle.join(" -> ")
            ),
        );
    }

    let local_names = def_map.keys().cloned().collect::<HashSet<_>>();
    rewrite_stmt_list_local_calls(&mut proc.init.body, &local_names, &proc.name);
    rewrite_stmt_list_local_calls(&mut proc.block_pre, &local_names, &proc.name);
    rewrite_stmt_list_local_calls(&mut proc.sample, &local_names, &proc.name);
    rewrite_stmt_list_local_calls(&mut proc.block_post, &local_names, &proc.name);
    for event in &mut proc.events {
        rewrite_stmt_list_local_calls(&mut event.body, &local_names, &proc.name);
    }
    for when in &mut proc.whens {
        rewrite_stmt_list_local_calls(&mut when.body, &local_names, &proc.name);
    }
    for task in &mut proc.tasks {
        rewrite_stmt_list_local_calls(&mut task.body, &local_names, &proc.name);
    }
    for def in &mut proc.local_defs {
        rewrite_stmt_list_local_calls(&mut def.body, &local_names, &proc.name);
    }
}

fn detect_cycles(def_map: &HashMap<String, FunctionDef>) -> Option<Vec<String>> {
    let mut visited = HashSet::<String>::new();
    let mut in_stack = HashSet::<String>::new();
    let mut path = Vec::<String>::new();

    for name in def_map.keys() {
        if visited.contains(name) {
            continue;
        }
        if let Some(cycle) = dfs_cycle(name, def_map, &mut visited, &mut in_stack, &mut path) {
            return Some(cycle);
        }
    }
    None
}

fn dfs_cycle(
    name: &str,
    def_map: &HashMap<String, FunctionDef>,
    visited: &mut HashSet<String>,
    in_stack: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    visited.insert(name.to_owned());
    in_stack.insert(name.to_owned());
    path.push(name.to_owned());

    if let Some(def) = def_map.get(name) {
        let callees = collect_local_def_calls(&def.body, def_map);
        for callee in callees {
            if !visited.contains(&callee) {
                if let Some(cycle) = dfs_cycle(&callee, def_map, visited, in_stack, path) {
                    return Some(cycle);
                }
            } else if in_stack.contains(&callee) {
                let mut cycle = path.clone();
                cycle.push(callee);
                return Some(cycle);
            }
        }
    }

    in_stack.remove(name);
    path.pop();
    None
}

fn collect_local_def_calls(stmts: &[Stmt], def_map: &HashMap<String, FunctionDef>) -> Vec<String> {
    let mut calls = Vec::new();
    for stmt in stmts {
        collect_local_def_calls_in_stmt(stmt, def_map, &mut calls);
    }
    calls
}

fn collect_local_def_calls_in_stmt(
    stmt: &Stmt,
    def_map: &HashMap<String, FunctionDef>,
    calls: &mut Vec<String>,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { expr, target, .. } => {
            collect_local_def_calls_in_expr(expr, def_map, calls);
            collect_local_def_calls_in_target(target, def_map, calls);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_local_def_calls_in_expr(expr, def_map, calls);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_local_def_calls_in_expr(cond, def_map, calls);
            for stmt in then_branch {
                collect_local_def_calls_in_stmt(stmt, def_map, calls);
            }
            for stmt in else_branch {
                collect_local_def_calls_in_stmt(stmt, def_map, calls);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_local_def_calls_in_expr(start, def_map, calls);
            collect_local_def_calls_in_expr(end, def_map, calls);
            if let Some(step) = step {
                collect_local_def_calls_in_expr(step, def_map, calls);
            }
            for stmt in body {
                collect_local_def_calls_in_stmt(stmt, def_map, calls);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_local_def_calls_in_expr(cond, def_map, calls);
            for stmt in body {
                collect_local_def_calls_in_stmt(stmt, def_map, calls);
            }
        }
    }
}

fn collect_local_def_calls_in_target(
    target: &AssignTarget,
    def_map: &HashMap<String, FunctionDef>,
    calls: &mut Vec<String>,
) {
    match target {
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        AssignTarget::Index { index, .. } => collect_local_def_calls_in_expr(index, def_map, calls),
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_local_def_calls_in_expr(coordinate, def_map, calls);
            }
        }
    }
}

fn collect_local_def_calls_in_expr(
    expr: &Expr,
    def_map: &HashMap<String, FunctionDef>,
    calls: &mut Vec<String>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if def_map.contains_key(name) {
                calls.push(name.clone());
            }
            for arg in args {
                collect_local_def_calls_in_expr(&arg.expr, def_map, calls);
            }
        }
        Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => {
            for arg in args {
                collect_local_def_calls_in_expr(arg, def_map, calls);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_local_def_calls_in_expr(lhs, def_map, calls);
            collect_local_def_calls_in_expr(rhs, def_map, calls);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_local_def_calls_in_expr(expr, def_map, calls);
        }
        Expr::Index { index, .. } => collect_local_def_calls_in_expr(index, def_map, calls),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_local_def_calls_in_expr(coordinate, def_map, calls);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_local_def_calls_in_expr(&spec.size, def_map, calls);
            if let Some(init) = init {
                for expr in init {
                    collect_local_def_calls_in_expr(expr, def_map, calls);
                }
            }
        }
        Expr::Tuple { values, .. } => {
            for v in values {
                collect_local_def_calls_in_expr(v, def_map, calls);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn rewrite_stmt_list_local_calls(
    stmts: &mut Vec<Stmt>,
    local_names: &HashSet<String>,
    owner_proc: &str,
) {
    for stmt in stmts {
        rewrite_stmt_local_calls(stmt, local_names, owner_proc);
    }
}

fn rewrite_stmt_local_calls(stmt: &mut Stmt, local_names: &HashSet<String>, owner_proc: &str) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            rewrite_target_local_calls(target, local_names, owner_proc);
            rewrite_expr_local_calls(expr, local_names, owner_proc);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_expr_local_calls(expr, local_names, owner_proc);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr_local_calls(cond, local_names, owner_proc);
            rewrite_stmt_list_local_calls(then_branch, local_names, owner_proc);
            rewrite_stmt_list_local_calls(else_branch, local_names, owner_proc);
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_expr_local_calls(start, local_names, owner_proc);
            rewrite_expr_local_calls(end, local_names, owner_proc);
            if let Some(step) = step {
                rewrite_expr_local_calls(step, local_names, owner_proc);
            }
            rewrite_stmt_list_local_calls(body, local_names, owner_proc);
        }
        Stmt::While { cond, body, .. } => {
            rewrite_expr_local_calls(cond, local_names, owner_proc);
            rewrite_stmt_list_local_calls(body, local_names, owner_proc);
        }
    }
}

fn rewrite_target_local_calls(
    target: &mut AssignTarget,
    local_names: &HashSet<String>,
    owner_proc: &str,
) {
    match target {
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        AssignTarget::Index { index, .. } => {
            rewrite_expr_local_calls(index, local_names, owner_proc)
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_expr_local_calls(coordinate, local_names, owner_proc);
            }
        }
    }
}

fn rewrite_expr_local_calls(expr: &mut Expr, local_names: &HashSet<String>, owner_proc: &str) {
    match expr {
        Expr::UserCall {
            name,
            type_args: _,
            args,
            ..
        } => {
            for arg in args.iter_mut() {
                rewrite_expr_local_calls(&mut arg.expr, local_names, owner_proc);
            }
            if local_names.contains(name) {
                *name = proc_local_hidden_def_name(owner_proc, name);
            }
        }
        Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => {
            for arg in args {
                rewrite_expr_local_calls(arg, local_names, owner_proc);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_expr_local_calls(lhs, local_names, owner_proc);
            rewrite_expr_local_calls(rhs, local_names, owner_proc);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_expr_local_calls(expr, local_names, owner_proc);
        }
        Expr::Index { index, .. } => rewrite_expr_local_calls(index, local_names, owner_proc),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_expr_local_calls(coordinate, local_names, owner_proc);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_expr_local_calls(&mut spec.size, local_names, owner_proc);
            if let Some(init) = init {
                for expr in init {
                    rewrite_expr_local_calls(expr, local_names, owner_proc);
                }
            }
        }
        Expr::Tuple { values, .. } => {
            for v in values {
                rewrite_expr_local_calls(v, local_names, owner_proc);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn inject_owner_self_into_hidden_local_calls_in_stmt(
    stmt: &mut Stmt,
    owner_proc: &str,
    receiver: &Expr,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            inject_owner_self_into_hidden_local_calls_in_target(target, owner_proc, receiver);
            inject_owner_self_into_hidden_local_calls_in_expr(expr, owner_proc, receiver);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(expr, owner_proc, receiver);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            inject_owner_self_into_hidden_local_calls_in_expr(cond, owner_proc, receiver);
            for stmt in then_branch {
                inject_owner_self_into_hidden_local_calls_in_stmt(stmt, owner_proc, receiver);
            }
            for stmt in else_branch {
                inject_owner_self_into_hidden_local_calls_in_stmt(stmt, owner_proc, receiver);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            inject_owner_self_into_hidden_local_calls_in_expr(start, owner_proc, receiver);
            inject_owner_self_into_hidden_local_calls_in_expr(end, owner_proc, receiver);
            if let Some(step) = step {
                inject_owner_self_into_hidden_local_calls_in_expr(step, owner_proc, receiver);
            }
            for stmt in body {
                inject_owner_self_into_hidden_local_calls_in_stmt(stmt, owner_proc, receiver);
            }
        }
        Stmt::While { cond, body, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(cond, owner_proc, receiver);
            for stmt in body {
                inject_owner_self_into_hidden_local_calls_in_stmt(stmt, owner_proc, receiver);
            }
        }
    }
}

fn inject_owner_self_into_hidden_local_calls_in_target(
    target: &mut AssignTarget,
    owner_proc: &str,
    receiver: &Expr,
) {
    match target {
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        AssignTarget::Index { index, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(index, owner_proc, receiver);
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                inject_owner_self_into_hidden_local_calls_in_expr(coordinate, owner_proc, receiver);
            }
        }
    }
}

fn inject_owner_self_into_hidden_local_calls_in_expr(
    expr: &mut Expr,
    owner_proc: &str,
    receiver: &Expr,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                inject_owner_self_into_hidden_local_calls_in_expr(
                    &mut arg.expr,
                    owner_proc,
                    receiver,
                );
            }
            let local_prefix = format!("{owner_proc}{PROC_LOCAL_DEF_FN_PREFIX}");
            if name.starts_with(&local_prefix) {
                args.insert(
                    0,
                    CallArg {
                        name: None,
                        expr: receiver.clone(),
                    },
                );
            }
        }
        Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => {
            for arg in args {
                inject_owner_self_into_hidden_local_calls_in_expr(arg, owner_proc, receiver);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(lhs, owner_proc, receiver);
            inject_owner_self_into_hidden_local_calls_in_expr(rhs, owner_proc, receiver);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(expr, owner_proc, receiver);
        }
        Expr::Index { index, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(index, owner_proc, receiver);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                inject_owner_self_into_hidden_local_calls_in_expr(coordinate, owner_proc, receiver);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            inject_owner_self_into_hidden_local_calls_in_expr(&mut spec.size, owner_proc, receiver);
            if let Some(init) = init {
                for expr in init {
                    inject_owner_self_into_hidden_local_calls_in_expr(expr, owner_proc, receiver);
                }
            }
        }
        Expr::Tuple { values, .. } => {
            for v in values {
                inject_owner_self_into_hidden_local_calls_in_expr(v, owner_proc, receiver);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn rewrite_nested_wrapper_local_calls_in_stmt(
    stmt: &mut Stmt,
    callee_proc: &str,
    owner_proc: &str,
    nested_path: &str,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            rewrite_nested_wrapper_local_calls_in_target(
                target,
                callee_proc,
                owner_proc,
                nested_path,
            );
            rewrite_nested_wrapper_local_calls_in_expr(expr, callee_proc, owner_proc, nested_path);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(expr, callee_proc, owner_proc, nested_path);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_nested_wrapper_local_calls_in_expr(cond, callee_proc, owner_proc, nested_path);
            for stmt in then_branch {
                rewrite_nested_wrapper_local_calls_in_stmt(
                    stmt,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
            for stmt in else_branch {
                rewrite_nested_wrapper_local_calls_in_stmt(
                    stmt,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_nested_wrapper_local_calls_in_expr(start, callee_proc, owner_proc, nested_path);
            rewrite_nested_wrapper_local_calls_in_expr(end, callee_proc, owner_proc, nested_path);
            if let Some(step) = step {
                rewrite_nested_wrapper_local_calls_in_expr(
                    step,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
            for stmt in body {
                rewrite_nested_wrapper_local_calls_in_stmt(
                    stmt,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(cond, callee_proc, owner_proc, nested_path);
            for stmt in body {
                rewrite_nested_wrapper_local_calls_in_stmt(
                    stmt,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
        }
    }
}

fn rewrite_nested_wrapper_local_calls_in_target(
    target: &mut AssignTarget,
    callee_proc: &str,
    owner_proc: &str,
    nested_path: &str,
) {
    match target {
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        AssignTarget::Index { index, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(index, callee_proc, owner_proc, nested_path);
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_nested_wrapper_local_calls_in_expr(
                    coordinate,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
        }
    }
}

fn rewrite_nested_wrapper_local_calls_in_expr(
    expr: &mut Expr,
    callee_proc: &str,
    owner_proc: &str,
    nested_path: &str,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                rewrite_nested_wrapper_local_calls_in_expr(
                    &mut arg.expr,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
            let local_prefix = format!("{callee_proc}{PROC_LOCAL_DEF_FN_PREFIX}");
            if let Some(local_name) = name.strip_prefix(&local_prefix) {
                *name = nested_wrapper_local_def_name(owner_proc, nested_path, local_name);
                args.insert(
                    0,
                    CallArg {
                        name: None,
                        expr: Expr::var("self"),
                    },
                );
            }
        }
        Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => {
            for arg in args {
                rewrite_nested_wrapper_local_calls_in_expr(
                    arg,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(lhs, callee_proc, owner_proc, nested_path);
            rewrite_nested_wrapper_local_calls_in_expr(rhs, callee_proc, owner_proc, nested_path);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(expr, callee_proc, owner_proc, nested_path);
        }
        Expr::Index { index, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(index, callee_proc, owner_proc, nested_path);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_nested_wrapper_local_calls_in_expr(
                    coordinate,
                    callee_proc,
                    owner_proc,
                    nested_path,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_nested_wrapper_local_calls_in_expr(
                &mut spec.size,
                callee_proc,
                owner_proc,
                nested_path,
            );
            if let Some(init) = init {
                for expr in init {
                    rewrite_nested_wrapper_local_calls_in_expr(
                        expr,
                        callee_proc,
                        owner_proc,
                        nested_path,
                    );
                }
            }
        }
        Expr::Tuple { values, .. } => {
            for v in values {
                rewrite_nested_wrapper_local_calls_in_expr(v, callee_proc, owner_proc, nested_path);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}
