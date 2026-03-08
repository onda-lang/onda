use std::collections::{HashMap, HashSet};

use omni_frontend::ast::*;
use omni_frontend::Diagnostic;

const MAX_INLINE_DEPTH: usize = 16;

/// Inline all proc-local def calls within a processor definition.
/// This rewrites init, events, sample, block_pre, and block_post bodies
/// by splicing the local def bodies at call sites.
///
/// Return-valued local defs are rewritten in-place by converting `return`
/// to assignments into a generated temp/target.
///
/// After this pass, `proc.local_defs` is cleared.
pub(crate) fn inline_proc_local_defs(proc: &mut ProcessorDef, errors: &mut Vec<Diagnostic>) {
    if proc.local_defs.is_empty() {
        return;
    }

    // Build name→def map and validate no duplicates.
    let mut def_map = HashMap::<String, FunctionDef>::new();
    for def in &proc.local_defs {
        if def_map.contains_key(&def.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}': duplicate proc-local def '{}'",
                    proc.name, def.name
                ),
                0,
                0,
            ));
        } else {
            def_map.insert(def.name.clone(), def.clone());
        }
    }

    // Detect cycles among proc-local defs.
    if let Some(cycle) = detect_cycles(&def_map) {
        errors.push(Diagnostic::semantic(
            format!(
                "processor '{}': recursive proc-local def cycle: {}",
                proc.name,
                cycle.join(" -> ")
            ),
            0,
            0,
        ));
        proc.local_defs.clear();
        return;
    }

    // Inline all proc-local defs into proc scopes.
    let mut call_id = 0u32;
    inline_stmts(
        &mut proc.init.body,
        &def_map,
        &proc.name,
        &mut call_id,
        0,
        errors,
    );
    inline_stmts(
        &mut proc.block_pre,
        &def_map,
        &proc.name,
        &mut call_id,
        0,
        errors,
    );
    inline_stmts(
        &mut proc.sample,
        &def_map,
        &proc.name,
        &mut call_id,
        0,
        errors,
    );
    inline_stmts(
        &mut proc.block_post,
        &def_map,
        &proc.name,
        &mut call_id,
        0,
        errors,
    );
    for event in &mut proc.events {
        inline_stmts(
            &mut event.body,
            &def_map,
            &proc.name,
            &mut call_id,
            0,
            errors,
        );
    }

    proc.local_defs.clear();
}

/// Detect cycles in the call graph among proc-local defs.
/// Returns Some(cycle_path) if a cycle is found, None otherwise.
fn detect_cycles(def_map: &HashMap<String, FunctionDef>) -> Option<Vec<String>> {
    let mut visited = HashSet::<String>::new();
    let mut in_stack = HashSet::<String>::new();
    let mut path = Vec::<String>::new();

    for name in def_map.keys() {
        if !visited.contains(name) {
            if let Some(cycle) = dfs_cycle(name, def_map, &mut visited, &mut in_stack, &mut path) {
                return Some(cycle);
            }
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

/// Collect names of proc-local defs called from a list of statements.
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
        Stmt::Expr { expr, .. } => collect_local_def_calls_in_expr(expr, def_map, calls),
        Stmt::Assign { expr, target, .. } => {
            collect_local_def_calls_in_expr(expr, def_map, calls);
            collect_local_def_calls_in_target(target, def_map, calls);
        }
        Stmt::Return { expr, .. } => collect_local_def_calls_in_expr(expr, def_map, calls),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_local_def_calls_in_expr(cond, def_map, calls);
            for s in then_branch {
                collect_local_def_calls_in_stmt(s, def_map, calls);
            }
            for s in else_branch {
                collect_local_def_calls_in_stmt(s, def_map, calls);
            }
        }
        Stmt::For {
            body,
            start,
            end,
            step,
            ..
        } => {
            collect_local_def_calls_in_expr(start, def_map, calls);
            collect_local_def_calls_in_expr(end, def_map, calls);
            if let Some(step) = step {
                collect_local_def_calls_in_expr(step, def_map, calls);
            }
            for s in body {
                collect_local_def_calls_in_stmt(s, def_map, calls);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_local_def_calls_in_expr(cond, def_map, calls);
            for s in body {
                collect_local_def_calls_in_stmt(s, def_map, calls);
            }
        }
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn collect_local_def_calls_in_target(
    target: &AssignTarget,
    def_map: &HashMap<String, FunctionDef>,
    calls: &mut Vec<String>,
) {
    match target {
        AssignTarget::Var(_) => {}
        AssignTarget::Index { index, .. } => {
            collect_local_def_calls_in_expr(index, def_map, calls);
        }
        AssignTarget::Slice { start, end, .. } => {
            if let Some(s) = start {
                collect_local_def_calls_in_expr(s, def_map, calls);
            }
            if let Some(e) = end {
                collect_local_def_calls_in_expr(e, def_map, calls);
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
            if def_map.contains_key(name.as_str()) {
                calls.push(name.clone());
            }
            for arg in args {
                collect_local_def_calls_in_expr(&arg.expr, def_map, calls);
            }
        }
        Expr::Call { args, .. } => {
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
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } | Expr::UnaryBitNot { expr } => {
            collect_local_def_calls_in_expr(expr, def_map, calls);
        }
        Expr::Index { index, .. } => {
            collect_local_def_calls_in_expr(index, def_map, calls);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(s) = start {
                collect_local_def_calls_in_expr(s, def_map, calls);
            }
            if let Some(e) = end {
                collect_local_def_calls_in_expr(e, def_map, calls);
            }
        }
        Expr::ArrayCtor { spec, init } => {
            collect_local_def_calls_in_expr(&spec.size, def_map, calls);
            if let Some(values) = init {
                for v in values {
                    collect_local_def_calls_in_expr(v, def_map, calls);
                }
            }
        }
        Expr::ArrayLiteral(elems) => {
            for e in elems {
                collect_local_def_calls_in_expr(e, def_map, calls);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

/// Inline proc-local def calls within a statement list.
fn inline_stmts(
    stmts: &mut Vec<Stmt>,
    def_map: &HashMap<String, FunctionDef>,
    proc_name: &str,
    call_id: &mut u32,
    depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    let mut i = 0;
    while i < stmts.len() {
        // Check if this statement is a bare call to a proc-local def (void usage).
        if let Stmt::Expr {
            expr: Expr::UserCall { name, args, .. },
            loc,
        } = &stmts[i]
        {
            if let Some(target_def) = def_map.get(name.as_str()) {
                if depth >= MAX_INLINE_DEPTH {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}': proc-local def '{}' inline depth exceeds limit ({})",
                            proc_name, name, MAX_INLINE_DEPTH
                        ),
                        0,
                        0,
                    ));
                    i += 1;
                    continue;
                }
                let my_id = *call_id;
                *call_id += 1;
                let mut inlined = build_inlined_body(
                    target_def,
                    args,
                    proc_name,
                    &name.clone(),
                    my_id,
                    loc.clone(),
                    None, // no return target for void calls
                    errors,
                );
                // Recursively inline within the spliced body.
                inline_stmts(&mut inlined, def_map, proc_name, call_id, depth + 1, errors);
                // Replace the call statement with the inlined body.
                stmts.splice(i..i + 1, inlined.iter().cloned());
                i += inlined.len();
                continue;
            }
        }

        // Check if this is an assignment where the RHS is a proc-local def call (return value).
        if let Stmt::Assign {
            expr: Expr::UserCall { name, args, .. },
            target,
            loc,
            ..
        } = &stmts[i]
        {
            if let Some(target_def) = def_map.get(name.as_str()) {
                if depth >= MAX_INLINE_DEPTH {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}': proc-local def '{}' inline depth exceeds limit ({})",
                            proc_name, name, MAX_INLINE_DEPTH
                        ),
                        0,
                        0,
                    ));
                    i += 1;
                    continue;
                }
                let return_target = match target {
                    AssignTarget::Var(v) => v.clone(),
                    _ => {
                        // For indexed/slice targets, use a temp var then assign.
                        format!("__pld_ret_{}_{}", name, *call_id)
                    }
                };
                let my_id = *call_id;
                *call_id += 1;
                let mut inlined = build_inlined_body(
                    target_def,
                    args,
                    proc_name,
                    &name.clone(),
                    my_id,
                    loc.clone(),
                    Some(&return_target),
                    errors,
                );
                // If the target was not a simple var, add a final assignment from temp to target.
                if !matches!(target, AssignTarget::Var(_)) {
                    inlined.push(Stmt::Assign {
                        loc: loc.clone(),
                        target: target.clone(),
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        expr: Expr::Var(return_target),
                    });
                }
                inline_stmts(&mut inlined, def_map, proc_name, call_id, depth + 1, errors);
                stmts.splice(i..i + 1, inlined.iter().cloned());
                i += inlined.len();
                continue;
            }
        }

        rewrite_while_condition_local_calls(&mut stmts[i], def_map, call_id);

        // Hoist any expression-position proc-local def calls into temp
        // assignments before this statement, then re-iterate so the
        // hoisted assignments get processed by the top-level cases above.
        let mut hoisted = Vec::new();
        hoist_calls_in_stmt(&mut stmts[i], def_map, call_id, &mut hoisted);
        if !hoisted.is_empty() {
            stmts.splice(i..i, hoisted);
            // Don't advance i — re-process from the first hoisted stmt.
            continue;
        }

        // Recurse into sub-bodies (if/for/while) for nested statement lists.
        match &mut stmts[i] {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                inline_stmts(then_branch, def_map, proc_name, call_id, depth, errors);
                inline_stmts(else_branch, def_map, proc_name, call_id, depth, errors);
            }
            Stmt::For { body, .. } => {
                inline_stmts(body, def_map, proc_name, call_id, depth, errors);
            }
            Stmt::While { body, .. } => {
                inline_stmts(body, def_map, proc_name, call_id, depth, errors);
            }
            _ => {}
        }
        i += 1;
    }
}

/// Extract proc-local def calls from expression positions within a statement.
/// Replaces each call with a temp var and pushes hoisted assignments to `out`.
/// Does NOT recurse into sub-bodies (if/for/while bodies) — those are handled
/// by recursive `inline_stmts` calls.
/// Does NOT hoist from `while` conditions since they are re-evaluated per iteration.
fn hoist_calls_in_stmt(
    stmt: &mut Stmt,
    def_map: &HashMap<String, FunctionDef>,
    call_id: &mut u32,
    out: &mut Vec<Stmt>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            hoist_calls_in_expr(expr, def_map, call_id, out);
            hoist_calls_in_target(target, def_map, call_id, out);
        }
        Stmt::Expr { expr, .. } => {
            hoist_calls_in_expr(expr, def_map, call_id, out);
        }
        Stmt::Return { expr, .. } => {
            hoist_calls_in_expr(expr, def_map, call_id, out);
        }
        Stmt::If { cond, .. } => {
            hoist_calls_in_expr(cond, def_map, call_id, out);
        }
        Stmt::For {
            start, end, step, ..
        } => {
            hoist_calls_in_expr(start, def_map, call_id, out);
            hoist_calls_in_expr(end, def_map, call_id, out);
            if let Some(s) = step {
                hoist_calls_in_expr(s, def_map, call_id, out);
            }
        }
        // While conditions are re-evaluated per iteration — do not hoist.
        Stmt::While { .. } => {}
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn hoist_calls_in_target(
    target: &mut AssignTarget,
    def_map: &HashMap<String, FunctionDef>,
    call_id: &mut u32,
    out: &mut Vec<Stmt>,
) {
    match target {
        AssignTarget::Var(_) => {}
        AssignTarget::Index { index, .. } => {
            hoist_calls_in_expr(index, def_map, call_id, out);
        }
        AssignTarget::Slice { start, end, .. } => {
            if let Some(s) = start {
                hoist_calls_in_expr(s, def_map, call_id, out);
            }
            if let Some(e) = end {
                hoist_calls_in_expr(e, def_map, call_id, out);
            }
        }
    }
}

/// Depth-first walk of an expression tree. When a `UserCall` to a proc-local
/// def is found, replace it with a temp var and push a hoisted assignment.
fn hoist_calls_in_expr(
    expr: &mut Expr,
    def_map: &HashMap<String, FunctionDef>,
    call_id: &mut u32,
    out: &mut Vec<Stmt>,
) {
    // First recurse into sub-expressions (depth-first) so inner calls
    // are hoisted before outer ones, preserving evaluation order.
    match expr {
        Expr::UserCall { args, .. } => {
            for arg in args {
                hoist_calls_in_expr(&mut arg.expr, def_map, call_id, out);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                hoist_calls_in_expr(arg, def_map, call_id, out);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            hoist_calls_in_expr(lhs, def_map, call_id, out);
            hoist_calls_in_expr(rhs, def_map, call_id, out);
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner }
        | Expr::UnaryBitNot { expr: inner } => {
            hoist_calls_in_expr(inner, def_map, call_id, out);
        }
        Expr::Index { index, .. } => {
            hoist_calls_in_expr(index, def_map, call_id, out);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(s) = start {
                hoist_calls_in_expr(s, def_map, call_id, out);
            }
            if let Some(e) = end {
                hoist_calls_in_expr(e, def_map, call_id, out);
            }
        }
        Expr::ArrayCtor { spec, init } => {
            hoist_calls_in_expr(&mut spec.size, def_map, call_id, out);
            if let Some(values) = init {
                for v in values {
                    hoist_calls_in_expr(v, def_map, call_id, out);
                }
            }
        }
        Expr::ArrayLiteral(elems) => {
            for e in elems {
                hoist_calls_in_expr(e, def_map, call_id, out);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }

    // After recursing, check if THIS expression is a proc-local def call.
    if let Expr::UserCall { name, .. } = expr {
        if def_map.contains_key(name.as_str()) {
            let temp = format!("__pld_ret_{}_{}", name, *call_id);
            *call_id += 1;
            // Replace the call expression with a temp var reference,
            // and push the original call as a hoisted assignment.
            let original = std::mem::replace(expr, Expr::Var(temp.clone()));
            out.push(Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(temp),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: original,
            });
        }
    }
}

/// Build the inlined body for a proc-local def call.
/// Creates parameter binding assignments and clones the body with alpha-renamed locals.
fn build_inlined_body(
    target_def: &FunctionDef,
    call_args: &[CallArg],
    proc_name: &str,
    def_name: &str,
    id: u32,
    loc: Option<SourceLoc>,
    _return_target: Option<&str>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    let prefix = format!("__pld_{def_name}_{id}_");
    let mut stmts = Vec::new();

    // Bind parameters: create assignments for scalar params, and alias
    // array/buffer params directly only when the argument is already a
    // simple symbol. Slice arguments must stay as slice expressions so
    // the narrowed view is preserved.
    let resolved_args =
        resolve_positional_args(&target_def.params, call_args, proc_name, def_name, errors);
    let mut alias_params = HashMap::<String, String>::new();
    for (param, arg_expr) in target_def.params.iter().zip(resolved_args.iter()) {
        let is_ref_param = matches!(
            &param.ty,
            Some(FnParamType::Array(_))
                | Some(FnParamType::ArrayGeneric(_))
                | Some(FnParamType::Buffer(_))
                | Some(FnParamType::BareBuffer)
        );
        if is_ref_param {
            // For array/buffer params, alias the param name to the argument's
            // variable name so the inlined body uses it directly.
            if let Expr::Var(arg_name) = arg_expr {
                alias_params.insert(param.name.clone(), arg_name.clone());
            }
            // If the arg is not a simple Var, fall through to assignment
            // (will likely fail at semantic analysis, but that's correct behavior).
            else {
                let renamed = format!("{prefix}{}", param.name);
                stmts.push(Stmt::Assign {
                    loc: loc.clone(),
                    target: AssignTarget::Var(renamed),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    expr: arg_expr.clone(),
                });
            }
        } else {
            let renamed = format!("{prefix}{}", param.name);
            stmts.push(Stmt::Assign {
                loc: loc.clone(),
                target: AssignTarget::Var(renamed),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: arg_expr.clone(),
            });
        }
    }

    // Clone the body and rename only params and for-loop iteration vars.
    // State variables and other names pass through unchanged — they refer
    // to the proc's state, which is exactly what we want for implicit access.
    // Array/buffer params are aliased directly to the argument's symbol name.
    let loop_vars = collect_for_loop_vars(target_def);
    let mut rename_map = HashMap::<String, String>::new();
    for param in &target_def.params {
        if let Some(alias) = alias_params.get(&param.name) {
            rename_map.insert(param.name.clone(), alias.clone());
        } else {
            rename_map.insert(param.name.clone(), format!("{prefix}{}", param.name));
        }
    }
    for var in &loop_vars {
        rename_map.insert(var.clone(), format!("{prefix}{var}"));
    }

    let mut body = target_def.body.clone();
    for stmt in &mut body {
        rename_locals_in_stmt(stmt, &rename_map);
    }

    if let Some(target) = _return_target {
        // If the return target is a generated temp var (from hoisting), pre-declare
        // it so it exists at the outer scope. The restructured body may only assign
        // inside if/else branches, which wouldn't declare the var at the outer level.
        if target.starts_with("__pld_ret_") {
            let default_val = find_any_return_expr(&body).unwrap_or(Expr::Number(0.0));
            stmts.push(Stmt::Assign {
                loc: loc.clone(),
                target: AssignTarget::Var(target.to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: default_val,
            });
        }
        // Restructure the body so that `return expr` becomes `target = expr`
        // with proper control flow (early returns become if/else nesting).
        let restructured = restructure_returns(&body, target);
        stmts.extend(restructured);
    } else {
        // Void call — just strip any return statements and add all other stmts.
        for stmt in body {
            match stmt {
                Stmt::Return { .. } => {}
                other => stmts.push(other),
            }
        }
    }

    stmts
}

/// Restructure a statement list so that `return expr` becomes `target = expr`
/// with proper control flow. Early returns inside if-blocks are handled by
/// wrapping remaining statements into the else branch.
///
/// For example:
/// ```text
/// stmt1
/// if (cond) { return x }
/// stmt2
/// return y
/// ```
/// becomes:
/// ```text
/// stmt1
/// if (cond) { target = x }
/// else {
///   stmt2
///   target = y
/// }
/// ```
fn restructure_returns(stmts: &[Stmt], target: &str) -> Vec<Stmt> {
    let mut result = Vec::new();
    restructure_returns_inner(stmts, target, &mut result);
    result
}

fn restructure_returns_inner(stmts: &[Stmt], target: &str, out: &mut Vec<Stmt>) {
    for (i, stmt) in stmts.iter().enumerate() {
        match stmt {
            Stmt::Return { expr, loc } => {
                // Convert return to assignment; remaining stmts are dead code.
                out.push(Stmt::Assign {
                    loc: loc.clone(),
                    target: AssignTarget::Var(target.to_owned()),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    expr: expr.clone(),
                });
                return; // Skip remaining statements (dead code after return).
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                loc,
            } => {
                let then_has_return = branch_always_returns(then_branch);
                let else_has_return = branch_always_returns(else_branch);

                if then_has_return && else_has_return {
                    // Both branches return — restructure both and skip rest.
                    let new_then = restructure_returns(then_branch, target);
                    let new_else = restructure_returns(else_branch, target);
                    out.push(Stmt::If {
                        loc: loc.clone(),
                        cond: cond.clone(),
                        then_branch: new_then,
                        else_branch: new_else,
                    });
                    return; // Rest is dead code.
                } else if then_has_return {
                    // Then branch returns — remaining stmts + else become the else branch.
                    let new_then = restructure_returns(then_branch, target);
                    let mut new_else = else_branch.clone();
                    // Append remaining stmts to else branch.
                    new_else.extend_from_slice(&stmts[i + 1..]);
                    let restructured_else = restructure_returns(&new_else, target);
                    out.push(Stmt::If {
                        loc: loc.clone(),
                        cond: cond.clone(),
                        then_branch: new_then,
                        else_branch: restructured_else,
                    });
                    return; // Remaining stmts already folded into else.
                } else if else_has_return {
                    // Else branch returns — remaining stmts go into then continuation.
                    let new_else = restructure_returns(else_branch, target);
                    let mut new_then = then_branch.clone();
                    new_then.extend_from_slice(&stmts[i + 1..]);
                    let restructured_then = restructure_returns(&new_then, target);
                    out.push(Stmt::If {
                        loc: loc.clone(),
                        cond: cond.clone(),
                        then_branch: restructured_then,
                        else_branch: new_else,
                    });
                    return; // Remaining stmts already folded into then.
                } else {
                    // Neither branch returns — just clone and continue.
                    out.push(stmt.clone());
                }
            }
            _ => out.push(stmt.clone()),
        }
    }
}

/// Find the expression of the last top-level `return` statement in a body.
/// Used to pre-declare the return target variable with the correct type.
fn find_any_return_expr(stmts: &[Stmt]) -> Option<Expr> {
    for stmt in stmts {
        match stmt {
            Stmt::Return { expr, .. } => return Some(expr.clone()),
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                if let Some(expr) = find_any_return_expr(then_branch) {
                    return Some(expr);
                }
                if let Some(expr) = find_any_return_expr(else_branch) {
                    return Some(expr);
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                if let Some(expr) = find_any_return_expr(body) {
                    return Some(expr);
                }
            }
            Stmt::Assign { .. }
            | Stmt::Expr { .. }
            | Stmt::Const { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
    None
}

/// Check if a statement list always ends with a return (every path returns).
fn branch_always_returns(stmts: &[Stmt]) -> bool {
    if let Some(last) = stmts.last() {
        match last {
            Stmt::Return { .. } => true,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => branch_always_returns(then_branch) && branch_always_returns(else_branch),
            _ => false,
        }
    } else {
        false
    }
}

/// Resolve call args to positional order matching the def's params.
fn resolve_positional_args(
    params: &[FnParamDecl],
    call_args: &[CallArg],
    proc_name: &str,
    def_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Expr> {
    let mut resolved = vec![None; params.len()];
    let mut param_index = HashMap::<&str, usize>::new();
    for (idx, param) in params.iter().enumerate() {
        param_index.insert(param.name.as_str(), idx);
    }

    let mut next_pos = 0usize;
    let mut seen_named = false;
    let mut seen_named_params = HashSet::<String>::new();

    for arg in call_args {
        if let Some(name) = &arg.name {
            seen_named = true;
            let Some(&idx) = param_index.get(name.as_str()) else {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}': proc-local def '{}' has no parameter named '{}'",
                        proc_name, def_name, name
                    ),
                    0,
                    0,
                ));
                continue;
            };
            if !seen_named_params.insert(name.clone()) || resolved[idx].is_some() {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}': duplicate named argument '{}' in call to proc-local def '{}'",
                        proc_name, name, def_name
                    ),
                    0,
                    0,
                ));
                continue;
            }
            resolved[idx] = Some(arg.expr.clone());
            continue;
        }

        if seen_named {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}': positional argument cannot follow named arguments in call to proc-local def '{}'",
                    proc_name, def_name
                ),
                0,
                0,
            ));
        }

        while next_pos < resolved.len() && resolved[next_pos].is_some() {
            next_pos += 1;
        }
        if next_pos >= resolved.len() {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}': too many arguments in call to proc-local def '{}'",
                    proc_name, def_name
                ),
                0,
                0,
            ));
            continue;
        }
        resolved[next_pos] = Some(arg.expr.clone());
        next_pos += 1;
    }

    let mut result = Vec::with_capacity(params.len());
    for (idx, param) in params.iter().enumerate() {
        if let Some(expr) = resolved[idx].take() {
            result.push(expr);
        } else if let Some(default) = &param.default {
            result.push(default.clone());
        } else {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}': missing required argument '{}' in call to proc-local def '{}'",
                    proc_name, param.name, def_name
                ),
                0,
                0,
            ));
            result.push(Expr::Int(0));
        }
    }

    result
}

fn rewrite_while_condition_local_calls(
    stmt: &mut Stmt,
    def_map: &HashMap<String, FunctionDef>,
    call_id: &mut u32,
) {
    let Stmt::While { loc, cond, body } = stmt else {
        return;
    };

    let mut lowered_cond = cond.clone();
    let mut cond_prelude = Vec::new();
    hoist_calls_in_expr(&mut lowered_cond, def_map, call_id, &mut cond_prelude);
    if cond_prelude.is_empty() {
        return;
    }

    let mut guarded_body = cond_prelude;
    guarded_body.push(Stmt::If {
        loc: loc.clone(),
        cond: Expr::UnaryNot {
            expr: Box::new(lowered_cond),
        },
        then_branch: vec![Stmt::Break { loc: loc.clone() }],
        else_branch: Vec::new(),
    });
    guarded_body.extend(body.clone());

    *cond = Expr::Bool(true);
    *body = guarded_body;
}

/// Collect for-loop iteration variable names within a def body.
/// These need renaming to avoid collisions between multiple inline sites.
fn collect_for_loop_vars(def: &FunctionDef) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in &def.body {
        collect_for_vars_in_stmt(stmt, &mut names);
    }
    // Remove param names — they're handled separately.
    for param in &def.params {
        names.remove(&param.name);
    }
    names
}

fn collect_for_vars_in_stmt(stmt: &Stmt, names: &mut HashSet<String>) {
    match stmt {
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for s in then_branch {
                collect_for_vars_in_stmt(s, names);
            }
            for s in else_branch {
                collect_for_vars_in_stmt(s, names);
            }
        }
        Stmt::For { var, body, .. } => {
            names.insert(var.clone());
            for s in body {
                collect_for_vars_in_stmt(s, names);
            }
        }
        Stmt::While { body, .. } => {
            for s in body {
                collect_for_vars_in_stmt(s, names);
            }
        }
        _ => {}
    }
}

/// Rename local variables in a statement according to the rename map.
/// Only renames variables that are in the map — state vars pass through unchanged.
fn rename_locals_in_stmt(stmt: &mut Stmt, map: &HashMap<String, String>) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            rename_locals_in_target(target, map);
            rename_locals_in_expr(expr, map);
        }
        Stmt::Expr { expr, .. } => {
            rename_locals_in_expr(expr, map);
        }
        Stmt::Return { expr, .. } => {
            rename_locals_in_expr(expr, map);
        }
        Stmt::Const { .. } => {}
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rename_locals_in_expr(cond, map);
            for s in then_branch {
                rename_locals_in_stmt(s, map);
            }
            for s in else_branch {
                rename_locals_in_stmt(s, map);
            }
        }
        Stmt::For {
            var,
            start,
            end,
            step,
            body,
            ..
        } => {
            if let Some(new) = map.get(var.as_str()) {
                *var = new.clone();
            }
            rename_locals_in_expr(start, map);
            rename_locals_in_expr(end, map);
            if let Some(step) = step {
                rename_locals_in_expr(step, map);
            }
            for s in body {
                rename_locals_in_stmt(s, map);
            }
        }
        Stmt::While { cond, body, .. } => {
            rename_locals_in_expr(cond, map);
            for s in body {
                rename_locals_in_stmt(s, map);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn rename_locals_in_target(target: &mut AssignTarget, map: &HashMap<String, String>) {
    match target {
        AssignTarget::Var(name) => {
            if let Some(new) = map.get(name.as_str()) {
                *name = new.clone();
            }
        }
        AssignTarget::Index { base, index, .. } => {
            if let Some(new) = map.get(base.as_str()) {
                *base = new.clone();
            }
            rename_locals_in_expr(index, map);
        }
        AssignTarget::Slice {
            base, start, end, ..
        } => {
            if let Some(new) = map.get(base.as_str()) {
                *base = new.clone();
            }
            if let Some(s) = start {
                rename_locals_in_expr(s, map);
            }
            if let Some(e) = end {
                rename_locals_in_expr(e, map);
            }
        }
    }
}

fn rename_locals_in_expr(expr: &mut Expr, map: &HashMap<String, String>) {
    match expr {
        Expr::Var(name) => {
            if let Some(new) = map.get(name.as_str()) {
                *name = new.clone();
            }
        }
        Expr::UserCall { name, args, .. } => {
            // Rename dotted receiver prefix (e.g., "arr.len" → "data.len").
            if let Some(dot) = name.find('.') {
                let receiver = &name[..dot];
                if let Some(new_receiver) = map.get(receiver) {
                    *name = format!("{}{}", new_receiver, &name[dot..]);
                }
            }
            for arg in args {
                rename_locals_in_expr(&mut arg.expr, map);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rename_locals_in_expr(arg, map);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rename_locals_in_expr(lhs, map);
            rename_locals_in_expr(rhs, map);
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner }
        | Expr::UnaryBitNot { expr: inner } => {
            rename_locals_in_expr(inner, map);
        }
        Expr::Index { base, index, .. } => {
            if let Some(new) = map.get(base.as_str()) {
                *base = new.clone();
            }
            rename_locals_in_expr(index, map);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            if let Some(new) = map.get(base.as_str()) {
                *base = new.clone();
            }
            if let Some(s) = start {
                rename_locals_in_expr(s, map);
            }
            if let Some(e) = end {
                rename_locals_in_expr(e, map);
            }
        }
        Expr::ArrayCtor { spec, init } => {
            rename_locals_in_expr(&mut spec.size, map);
            if let Some(values) = init {
                for v in values {
                    rename_locals_in_expr(v, map);
                }
            }
        }
        Expr::ArrayLiteral(elems) => {
            for e in elems {
                rename_locals_in_expr(e, map);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}
