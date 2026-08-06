use super::*;
use crate::proc_call_support::rewrite_proc_alias_call_sites_in_expr;

fn is_proc_operator_helper_name(name: &str) -> bool {
    name.ends_with(PROC_STEP_FN_SUFFIX)
        || name.contains(PROC_CALL_OUT_FN_PREFIX)
        || (name.contains(".__proc_nested_")
            && (name.ends_with("_step") || name.contains("_call_out")))
}

fn proc_name_for_lowered_proc_call(name: &str) -> Option<&str> {
    if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
        return Some(step_proc);
    }
    let (call_proc, out_idx_raw) = name.rsplit_once(PROC_CALL_OUT_FN_PREFIX)?;
    out_idx_raw.parse::<usize>().ok()?;
    Some(call_proc)
}

fn nested_proc_operator_timing(
    name: &str,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<OutputTiming> {
    for (nested_var, instance) in nested_instances {
        if name == nested_step_fn_name(owner_proc, nested_var) {
            return proc_api
                .get(&instance.proc_name)
                .map(|api| api.outputs.timing);
        }
        let prefix = format!("{owner_proc}.__proc_nested_{nested_var}_call_out");
        if let Some(raw_idx) = name.strip_prefix(&prefix) {
            raw_idx.parse::<usize>().ok()?;
            return proc_api
                .get(&instance.proc_name)
                .map(|api| api.outputs.timing);
        }
    }
    None
}

fn proc_operator_helper_timing(
    name: &str,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<OutputTiming> {
    if let Some(timing) = nested_proc_operator_timing(name, owner_proc, nested_instances, proc_api)
    {
        return Some(timing);
    }
    let proc_name = proc_name_for_lowered_proc_call(name)?;
    proc_api.get(proc_name).map(|api| api.outputs.timing)
}

fn collect_proc_operator_helper_diags_from_expr(
    expr: &Expr,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_proc_operator_helper_diags_from_expr(
                    value,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
        }
        Expr::Index { index, .. } => collect_proc_operator_helper_diags_from_expr(
            index,
            owner_proc,
            nested_instances,
            proc_api,
            out,
        ),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_proc_operator_helper_diags_from_expr(
                    coordinate,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_proc_operator_helper_diags_from_expr(
                &spec.size,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_proc_operator_helper_diags_from_expr(
                        value,
                        owner_proc,
                        nested_instances,
                        proc_api,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_proc_operator_helper_diags_from_expr(
                lhs,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
            collect_proc_operator_helper_diags_from_expr(
                rhs,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_proc_operator_helper_diags_from_expr(
                    arg,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
        }
        Expr::UserCall {
            name, args, loc, ..
        } => {
            if is_proc_operator_helper_name(name) {
                if let Some(timing) =
                    proc_operator_helper_timing(name, owner_proc, nested_instances, proc_api)
                {
                    out.push((DiagCtx::new(*loc), timing));
                }
            }
            for arg in args {
                collect_proc_operator_helper_diags_from_expr(
                    &arg.expr,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_proc_operator_helper_diags_from_expr(
                expr,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
        }
    }
}

fn collect_proc_operator_helper_diags_from_stmt(
    stmt: &Stmt,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_proc_operator_helper_diags_from_expr(
                expr,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_proc_operator_helper_diags_from_expr(
                cond,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
            for stmt in then_branch {
                collect_proc_operator_helper_diags_from_stmt(
                    stmt,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
            for stmt in else_branch {
                collect_proc_operator_helper_diags_from_stmt(
                    stmt,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
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
            collect_proc_operator_helper_diags_from_expr(
                start,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
            collect_proc_operator_helper_diags_from_expr(
                end,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
            if let Some(step) = step {
                collect_proc_operator_helper_diags_from_expr(
                    step,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
            for stmt in body {
                collect_proc_operator_helper_diags_from_stmt(
                    stmt,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_proc_operator_helper_diags_from_expr(
                cond,
                owner_proc,
                nested_instances,
                proc_api,
                out,
            );
            for stmt in body {
                collect_proc_operator_helper_diags_from_stmt(
                    stmt,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    out,
                );
            }
        }
    }
}

fn collect_non_sample_proc_operator_diags_from_expr(
    expr: &Expr,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    let mut rewritten = expr.clone();
    rewrite_proc_alias_call_sites_in_expr(&mut rewritten, aliases);
    let mut scratch = Vec::<Diagnostic>::new();
    rewrite_nested_proc_calls_in_expr(
        &mut rewritten,
        owner_proc,
        nested_instances,
        proc_array_slots,
        proc_api,
        &mut scratch,
    );
    collect_proc_operator_helper_diags_from_expr(
        &rewritten,
        owner_proc,
        nested_instances,
        proc_api,
        out,
    );
}

fn collect_non_sample_proc_operator_diags_from_expr_stmt(
    expr: &Expr,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    if !matches!(expr, Expr::UserCall { .. }) {
        collect_non_sample_proc_operator_diags_from_expr(
            expr,
            owner_proc,
            nested_instances,
            proc_array_slots,
            proc_api,
            aliases,
            out,
        );
        return;
    }
    let mut rewritten_stmt = Stmt::Expr {
        loc: expr.loc().into(),
        expr: expr.clone(),
    };
    if let Stmt::Expr { expr, .. } = &mut rewritten_stmt {
        rewrite_proc_alias_call_sites_in_expr(expr, aliases);
    }
    let mut scratch = Vec::<Diagnostic>::new();
    rewrite_nested_proc_calls_in_stmt(
        &mut rewritten_stmt,
        owner_proc,
        nested_instances,
        proc_array_slots,
        proc_api,
        &mut scratch,
    );
    collect_proc_operator_helper_diags_from_stmt(
        &rewritten_stmt,
        owner_proc,
        nested_instances,
        proc_api,
        out,
    );
}

fn collect_non_sample_proc_operator_diags_from_target(
    target: &AssignTarget,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    match target {
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        AssignTarget::Index { index, .. } => collect_non_sample_proc_operator_diags_from_expr(
            index,
            owner_proc,
            nested_instances,
            proc_array_slots,
            proc_api,
            aliases,
            out,
        ),
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_non_sample_proc_operator_diags_from_expr(
                    coordinate,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
            }
        }
    }
}

fn proc_alias_from_expr(
    expr: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> Option<ProcArrayAliasInfo> {
    match expr {
        Expr::Index { base, index, .. } if proc_array_slots.contains_key(base) => {
            Some(ProcArrayAliasInfo {
                array_base: base.clone(),
                index_expr: index.as_ref().clone(),
            })
        }
        Expr::Var { name, .. } => aliases.get(name).cloned(),
        _ => None,
    }
}

fn merge_proc_aliases_union(
    aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    extra: HashMap<String, ProcArrayAliasInfo>,
) {
    for (name, info) in extra {
        aliases.insert(name, info);
    }
}

fn collect_non_sample_proc_operator_diags_from_stmts(
    stmts: &[Stmt],
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { target, expr, .. } => {
                collect_non_sample_proc_operator_diags_from_target(
                    target,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
                collect_non_sample_proc_operator_diags_from_expr(
                    expr,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
                if let AssignTarget::Var(name) = target {
                    if let Some(alias) = proc_alias_from_expr(expr, proc_array_slots, aliases) {
                        aliases.insert(name.clone(), alias);
                    } else {
                        aliases.remove(name);
                    }
                }
            }
            Stmt::Expr { expr, .. } => collect_non_sample_proc_operator_diags_from_expr_stmt(
                expr,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                aliases,
                out,
            ),
            Stmt::Return { expr, .. } => collect_non_sample_proc_operator_diags_from_expr(
                expr,
                owner_proc,
                nested_instances,
                proc_array_slots,
                proc_api,
                aliases,
                out,
            ),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_non_sample_proc_operator_diags_from_expr(
                    cond,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
                let mut then_aliases = aliases.clone();
                collect_non_sample_proc_operator_diags_from_stmts(
                    then_branch,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    &mut then_aliases,
                    out,
                );
                let mut else_aliases = aliases.clone();
                collect_non_sample_proc_operator_diags_from_stmts(
                    else_branch,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    &mut else_aliases,
                    out,
                );
                merge_proc_aliases_union(aliases, then_aliases);
                merge_proc_aliases_union(aliases, else_aliases);
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_non_sample_proc_operator_diags_from_expr(
                    start,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
                collect_non_sample_proc_operator_diags_from_expr(
                    end,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
                if let Some(step) = step {
                    collect_non_sample_proc_operator_diags_from_expr(
                        step,
                        owner_proc,
                        nested_instances,
                        proc_array_slots,
                        proc_api,
                        aliases,
                        out,
                    );
                }
                let mut body_aliases = aliases.clone();
                collect_non_sample_proc_operator_diags_from_stmts(
                    body,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    &mut body_aliases,
                    out,
                );
                merge_proc_aliases_union(aliases, body_aliases);
            }
            Stmt::While { cond, body, .. } => {
                collect_non_sample_proc_operator_diags_from_expr(
                    cond,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
                let mut body_aliases = aliases.clone();
                collect_non_sample_proc_operator_diags_from_stmts(
                    body,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    &mut body_aliases,
                    out,
                );
                merge_proc_aliases_union(aliases, body_aliases);
            }
        }
    }
}

fn push_non_sample_proc_call_errors_in_proc_stmts(
    stmts: &[Stmt],
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    context: &str,
    allowed: Option<OutputTiming>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut aliases = HashMap::<String, ProcArrayAliasInfo>::new();
    let mut diags = Vec::<(DiagCtx, OutputTiming)>::new();
    collect_non_sample_proc_operator_diags_from_stmts(
        stmts,
        owner_proc,
        nested_instances,
        proc_array_slots,
        proc_api,
        &mut aliases,
        &mut diags,
    );
    for (diag, timing) in diags {
        if Some(timing) == allowed {
            continue;
        }
        let required = match timing {
            OutputTiming::Sample => "sample",
            OutputTiming::Block => "block",
        };
        push_semantic(
            diag,
            errors,
            format!(
                "proc operator '()' for {required}-rate proc is only allowed in {required}; found use in {context}"
            ),
        );
    }
}

fn seed_called_proc_local_defs_from_expr(
    expr: &Expr,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                seed_called_proc_local_defs_from_expr(value, def_names, pending, seen_pending);
            }
        }
        Expr::Index { index, .. } => {
            seed_called_proc_local_defs_from_expr(index, def_names, pending, seen_pending);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                seed_called_proc_local_defs_from_expr(coordinate, def_names, pending, seen_pending);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            seed_called_proc_local_defs_from_expr(&spec.size, def_names, pending, seen_pending);
            if let Some(values) = init {
                for value in values {
                    seed_called_proc_local_defs_from_expr(value, def_names, pending, seen_pending);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            seed_called_proc_local_defs_from_expr(lhs, def_names, pending, seen_pending);
            seed_called_proc_local_defs_from_expr(rhs, def_names, pending, seen_pending);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                seed_called_proc_local_defs_from_expr(arg, def_names, pending, seen_pending);
            }
        }
        Expr::UserCall { name, args, .. } => {
            if def_names.contains(name) && seen_pending.insert(name.clone()) {
                pending.push(name.clone());
            }
            for arg in args {
                seed_called_proc_local_defs_from_expr(&arg.expr, def_names, pending, seen_pending);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            seed_called_proc_local_defs_from_expr(expr, def_names, pending, seen_pending);
        }
    }
}

fn seed_called_proc_local_defs_from_target(
    target: &AssignTarget,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match target {
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
        AssignTarget::Index { index, .. } => {
            seed_called_proc_local_defs_from_expr(index, def_names, pending, seen_pending);
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                seed_called_proc_local_defs_from_expr(coordinate, def_names, pending, seen_pending);
            }
        }
    }
}

fn seed_called_proc_local_defs_from_stmts(
    stmts: &[Stmt],
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { target, expr, .. } => {
                seed_called_proc_local_defs_from_target(target, def_names, pending, seen_pending);
                seed_called_proc_local_defs_from_expr(expr, def_names, pending, seen_pending);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                seed_called_proc_local_defs_from_expr(expr, def_names, pending, seen_pending);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                seed_called_proc_local_defs_from_expr(cond, def_names, pending, seen_pending);
                seed_called_proc_local_defs_from_stmts(
                    then_branch,
                    def_names,
                    pending,
                    seen_pending,
                );
                seed_called_proc_local_defs_from_stmts(
                    else_branch,
                    def_names,
                    pending,
                    seen_pending,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                seed_called_proc_local_defs_from_expr(start, def_names, pending, seen_pending);
                seed_called_proc_local_defs_from_expr(end, def_names, pending, seen_pending);
                if let Some(step) = step {
                    seed_called_proc_local_defs_from_expr(step, def_names, pending, seen_pending);
                }
                seed_called_proc_local_defs_from_stmts(body, def_names, pending, seen_pending);
            }
            Stmt::While { cond, body, .. } => {
                seed_called_proc_local_defs_from_expr(cond, def_names, pending, seen_pending);
                seed_called_proc_local_defs_from_stmts(body, def_names, pending, seen_pending);
            }
        }
    }
}

pub(super) fn reject_non_sample_proc_operator_calls_in_proc(
    proc: &onda_frontend::ProcessorDef,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    push_non_sample_proc_call_errors_in_proc_stmts(
        &proc.init.body,
        &proc.name,
        nested_instances,
        proc_array_slots,
        proc_api,
        &format!("processor '{}'.init", proc.name),
        None,
        errors,
    );
    push_non_sample_proc_call_errors_in_proc_stmts(
        &proc.block_pre,
        &proc.name,
        nested_instances,
        proc_array_slots,
        proc_api,
        &format!("processor '{}'.block pre", proc.name),
        Some(OutputTiming::Block),
        errors,
    );
    push_non_sample_proc_call_errors_in_proc_stmts(
        &proc.sample,
        &proc.name,
        nested_instances,
        proc_array_slots,
        proc_api,
        &format!("processor '{}'.sample", proc.name),
        Some(OutputTiming::Sample),
        errors,
    );
    push_non_sample_proc_call_errors_in_proc_stmts(
        &proc.block_post,
        &proc.name,
        nested_instances,
        proc_array_slots,
        proc_api,
        &format!("processor '{}'.block post", proc.name),
        Some(OutputTiming::Block),
        errors,
    );
    for event in &proc.events {
        push_non_sample_proc_call_errors_in_proc_stmts(
            &event.body,
            &proc.name,
            nested_instances,
            proc_array_slots,
            proc_api,
            &format!("processor '{}'.event '{}'", proc.name, event.name),
            None,
            errors,
        );
    }

    let local_defs = proc
        .local_defs
        .iter()
        .map(|def| (proc_local_hidden_def_name(&proc.name, &def.name), def))
        .collect::<HashMap<_, _>>();
    if local_defs.is_empty() {
        return;
    }

    let def_names = local_defs.keys().cloned().collect::<HashSet<_>>();
    let mut defs_with_proc_calls = HashMap::<String, Vec<(DiagCtx, OutputTiming)>>::new();
    for (hidden_name, def) in &local_defs {
        let mut aliases = HashMap::<String, ProcArrayAliasInfo>::new();
        let mut diags = Vec::<(DiagCtx, OutputTiming)>::new();
        collect_non_sample_proc_operator_diags_from_stmts(
            &def.body,
            &proc.name,
            nested_instances,
            proc_array_slots,
            proc_api,
            &mut aliases,
            &mut diags,
        );
        if !diags.is_empty() {
            defs_with_proc_calls.insert(hidden_name.clone(), diags);
        }
    }

    let collect_reachable_defs = |roots: Vec<&[Stmt]>| {
        let mut pending = Vec::<String>::new();
        let mut seen_pending = HashSet::<String>::new();
        for root in roots {
            seed_called_proc_local_defs_from_stmts(
                root,
                &def_names,
                &mut pending,
                &mut seen_pending,
            );
        }

        let mut reachable = HashSet::<String>::new();
        while let Some(name) = pending.pop() {
            if !reachable.insert(name.clone()) {
                continue;
            }
            let Some(def) = local_defs.get(&name) else {
                continue;
            };
            seed_called_proc_local_defs_from_stmts(
                &def.body,
                &def_names,
                &mut pending,
                &mut seen_pending,
            );
        }
        reachable
    };

    let block_reachable_defs =
        collect_reachable_defs(vec![proc.block_pre.as_slice(), proc.block_post.as_slice()]);
    let sample_reachable_defs = collect_reachable_defs(vec![proc.sample.as_slice()]);
    let mut neither_roots = Vec::<&[Stmt]>::new();
    neither_roots.push(proc.init.body.as_slice());
    for event in &proc.events {
        neither_roots.push(event.body.as_slice());
    }
    let neither_reachable_defs = collect_reachable_defs(neither_roots);

    for (hidden_name, diags) in defs_with_proc_calls {
        let def_name = local_defs
            .get(&hidden_name)
            .map(|def| def.name.as_str())
            .unwrap_or(hidden_name.as_str());
        for (diag, timing) in diags {
            let allowed = match timing {
                OutputTiming::Sample => {
                    sample_reachable_defs.contains(&hidden_name)
                        && !block_reachable_defs.contains(&hidden_name)
                        && !neither_reachable_defs.contains(&hidden_name)
                }
                OutputTiming::Block => {
                    block_reachable_defs.contains(&hidden_name)
                        && !sample_reachable_defs.contains(&hidden_name)
                        && !neither_reachable_defs.contains(&hidden_name)
                }
            };
            if allowed {
                continue;
            }
            let required = match timing {
                OutputTiming::Sample => "sample",
                OutputTiming::Block => "block",
            };
            push_semantic(
                diag,
                errors,
                format!(
                    "proc operator '()' for {required}-rate proc is only allowed in {required}; call in processor '{}'.def '{}' is not provably {required}-only",
                    proc.name, def_name
                ),
            );
        }
    }
}

pub(super) fn proc_os_sinc_stage_count(factor: usize) -> usize {
    factor.max(1).trailing_zeros() as usize
}

pub(super) fn proc_os_up_stage_tap_field_name(input_name: &str, stage: usize, tap: &str) -> String {
    format!("__onda_os_up_in__{input_name}__stage{stage}__{tap}")
}

pub(super) fn proc_os_down_stage_tap_field_name(
    output_name: &str,
    stage: usize,
    tap: &str,
) -> String {
    format!("__onda_os_down_out__{output_name}__stage{stage}__{tap}")
}

pub(super) fn compute_effective_proc_block_flags(
    proc_order: &[String],
    proc_defs_by_name: &HashMap<String, onda_frontend::ProcessorDef>,
    base_shapes: &HashMap<String, ProcBaseShape>,
) -> HashMap<String, bool> {
    fn visit(
        proc_name: &str,
        proc_defs_by_name: &HashMap<String, onda_frontend::ProcessorDef>,
        base_shapes: &HashMap<String, ProcBaseShape>,
        cache: &mut HashMap<String, bool>,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if let Some(value) = cache.get(proc_name) {
            return *value;
        }
        if !visiting.insert(proc_name.to_owned()) {
            return false;
        }
        let mut has_block = proc_defs_by_name
            .get(proc_name)
            .map(|p| p.has_block_block)
            .unwrap_or(false);
        if !has_block {
            if let Some(shape) = base_shapes.get(proc_name) {
                for nested_state in shape.state.nested_procs.values() {
                    if visit(
                        &nested_state.proc_name,
                        proc_defs_by_name,
                        base_shapes,
                        cache,
                        visiting,
                    ) {
                        has_block = true;
                        break;
                    }
                }
                if !has_block {
                    for nested_array in shape.state.nested_proc_arrays.values() {
                        if visit(
                            &nested_array.proc_name,
                            proc_defs_by_name,
                            base_shapes,
                            cache,
                            visiting,
                        ) {
                            has_block = true;
                            break;
                        }
                    }
                }
            }
        }
        visiting.remove(proc_name);
        cache.insert(proc_name.to_owned(), has_block);
        has_block
    }

    let mut cache = HashMap::<String, bool>::new();
    let mut visiting = HashSet::<String>::new();
    for proc_name in proc_order {
        let _ = visit(
            proc_name,
            proc_defs_by_name,
            base_shapes,
            &mut cache,
            &mut visiting,
        );
    }
    cache
}

pub(super) fn infer_primary_output_type_from_processor(proc: &ProcessorDef) -> PrimitiveType {
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let inferred_names = infer_numbered_names_from_proc(proc);
    let inferred_out_max = match proc.outs_timing {
        OutputTiming::Sample => inferred_io.max_out,
        OutputTiming::Block => inferred_names.max_kout,
    };
    let out_ports = normalize_numbered_port_decls(
        &proc.outs,
        proc_output_numbered_prefix(proc),
        inferred_out_max,
    );
    let Some(first_out) = out_ports.first() else {
        return PrimitiveType::F32;
    };
    match first_out.ty.as_ref() {
        Some(DeclType::Scalar(ty)) => *ty,
        Some(DeclType::Array { elem, .. }) => *elem,
        Some(DeclType::Tuple(_))
        | Some(DeclType::Generic(_))
        | Some(DeclType::ArrayGeneric { .. })
        | None => PrimitiveType::F32,
    }
}

pub(super) fn struct_defs_for_scalar_expr_inference(
    struct_defs: &HashMap<String, onda_frontend::StructDef>,
) -> HashMap<String, Vec<TypedStructField>> {
    coerce_struct_defs_for_inference(struct_defs, AnalysisOptions::default())
}

fn is_proc_output_alias_name(name: &str, timing: OutputTiming, out_count: usize) -> bool {
    let prefix = match timing {
        OutputTiming::Sample => "out",
        OutputTiming::Block => "kout",
    };
    parse_numbered_port_index(name, prefix)
        .map(|idx| idx <= out_count)
        .unwrap_or(false)
}

fn array_infos_from_slot_map(
    slots_by_name: &HashMap<String, Vec<String>>,
    slot_types: &HashMap<String, PrimitiveType>,
) -> HashMap<String, TypedArrayInfo> {
    slots_by_name
        .iter()
        .map(|(name, slots)| {
            let elem_ty = slots
                .iter()
                .find_map(|slot| slot_types.get(slot).copied())
                .unwrap_or(PrimitiveType::F32);
            (
                name.clone(),
                TypedArrayInfo {
                    elem_ty,
                    len: slots.len(),
                    offset: 0,
                },
            )
        })
        .collect()
}

fn array_infos_from_param_specs(param_specs: &[ProcParamSpec]) -> HashMap<String, TypedArrayInfo> {
    param_specs
        .iter()
        .filter(|spec| spec.slots.len() > 1)
        .map(|spec| {
            let elem_ty = spec
                .slots
                .first()
                .map(|slot| slot.ty)
                .unwrap_or(PrimitiveType::F32);
            (
                spec.name.clone(),
                TypedArrayInfo {
                    elem_ty,
                    len: spec.slots.len(),
                    offset: 0,
                },
            )
        })
        .collect()
}

fn simple_dot_path(name: &str) -> Option<(&str, &str)> {
    let (base, field) = name.rsplit_once('.')?;
    if base.is_empty() || field.is_empty() {
        return None;
    }
    Some((base, field))
}

fn proc_bound_param_hook_calls(param_specs: &[ProcParamSpec]) -> Vec<String> {
    param_specs
        .iter()
        .filter_map(|spec| {
            if spec.slots.len() == 1 && spec.slots[0].name == spec.name {
                spec.slots[0].bind.clone()
            } else {
                None
            }
        })
        .collect()
}

fn bound_proc_param_names(param_specs: &[ProcParamSpec]) -> HashSet<String> {
    param_specs
        .iter()
        .flat_map(|spec| {
            std::iter::once(spec.name.clone())
                .chain(spec.slots.iter().map(|slot| slot.name.clone()))
        })
        .collect()
}

fn nested_proc_receiver_root<'a>(
    root: &'a str,
    state: &ProcStateFields,
    nested_proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<&'a str> {
    if state.nested_procs.contains_key(root) || nested_proc_array_slots.contains_key(root) {
        return Some(root);
    }
    if let Some((base, rest)) = root.split_once('[') {
        if rest.ends_with(']') && nested_proc_array_slots.contains_key(base) {
            return Some(base);
        }
    }
    if let Some((base, _field)) = simple_dot_path(root) {
        if state.nested_procs.contains_key(base) || nested_proc_array_slots.contains_key(base) {
            return Some(base);
        }
        if let Some((array_base, rest)) = base.split_once('[') {
            if rest.ends_with(']') && nested_proc_array_slots.contains_key(array_base) {
                return Some(array_base);
            }
        }
    }
    None
}

fn stmt_list_contains_return(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_return)
}

fn stmt_contains_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => stmt_list_contains_return(then_branch) || stmt_list_contains_return(else_branch),
        Stmt::For { body, .. } | Stmt::While { body, .. } => stmt_list_contains_return(body),
        Stmt::Const { .. }
        | Stmt::Assign { .. }
        | Stmt::Expr { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => false,
    }
}

struct HookSafeCtx<'a> {
    owner_proc: &'a str,
    local_defs_by_name: &'a HashMap<String, &'a FunctionDef>,
    fn_defs_by_name: &'a HashMap<String, &'a FunctionDef>,
    owner_param_names: &'a HashSet<String>,
    owner_param_array_names: &'a HashSet<String>,
    input_names: &'a HashSet<String>,
    input_array_names: &'a HashSet<String>,
    output_names: &'a HashSet<String>,
    output_array_names: &'a HashSet<String>,
    state: &'a ProcStateFields,
    nested_proc_array_slots: &'a HashMap<String, Vec<String>>,
    child_proc_surfaces: &'a HashMap<String, ChildProcSurface>,
}

#[derive(Debug, Default)]
struct ChildProcSurface {
    params: HashSet<String>,
    param_arrays: HashSet<String>,
    inputs: HashSet<String>,
    input_arrays: HashSet<String>,
    outputs: HashSet<String>,
    output_arrays: HashSet<String>,
}

#[derive(Debug, Eq, PartialEq)]
enum ChildProcWriteKind {
    Param,
    DynamicParams,
    Input,
    Output,
    InternalState,
}

fn sample_oversample_factor_for_shape(
    proc: &onda_frontend::ProcessorDef,
    options: AnalysisOptions,
) -> usize {
    let mut scratch_errors = Vec::<Diagnostic>::new();
    validated_sample_oversample_factor(
        proc.sample_oversample_factor.as_ref(),
        options,
        &format!("processor '{}' sample block", proc.name),
        &mut scratch_errors,
    )
}

fn proc_options_for_shape(
    proc: &onda_frontend::ProcessorDef,
    host_options: AnalysisOptions,
) -> AnalysisOptions {
    proc_runtime_analysis_options(
        host_options,
        sample_oversample_factor_for_shape(proc, host_options),
    )
}

fn infer_numbered_names_from_proc(proc: &onda_frontend::ProcessorDef) -> IoInference {
    let mut inferred = IoInference::default();
    for stmt in &proc.init.body {
        infer_io_from_stmt(stmt, &mut inferred);
    }
    for stmt in &proc.block_pre {
        infer_io_from_stmt(stmt, &mut inferred);
    }
    for stmt in &proc.sample {
        infer_io_from_stmt(stmt, &mut inferred);
    }
    for stmt in &proc.block_post {
        infer_io_from_stmt(stmt, &mut inferred);
    }
    for event in &proc.events {
        for stmt in &event.body {
            infer_io_from_stmt(stmt, &mut inferred);
        }
    }
    for def in &proc.local_defs {
        for stmt in &def.body {
            infer_io_from_stmt(stmt, &mut inferred);
        }
    }
    inferred
}

fn proc_output_numbered_prefix(proc: &onda_frontend::ProcessorDef) -> &'static str {
    match proc.outs_timing {
        OutputTiming::Sample => "out",
        OutputTiming::Block => "kout",
    }
}

fn build_child_proc_surfaces(
    proc_defs_by_name: &HashMap<String, onda_frontend::ProcessorDef>,
    options: AnalysisOptions,
) -> HashMap<String, ChildProcSurface> {
    let mut out = HashMap::<String, ChildProcSurface>::new();
    for proc in proc_defs_by_name.values() {
        let mut scratch_errors = Vec::<Diagnostic>::new();
        let proc_options = proc_options_for_shape(proc, options);
        let inferred_io = infer_numbered_io_from_sample(&proc.sample);
        let inferred_names = infer_numbered_names_from_proc(proc);
        let ins_ports = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
        let out_inferred_max = match proc.outs_timing {
            OutputTiming::Sample => inferred_io.max_out,
            OutputTiming::Block => inferred_names.max_kout,
        };
        let outs_ports = normalize_numbered_port_decls(
            &proc.outs,
            proc_output_numbered_prefix(proc),
            out_inferred_max,
        );
        let params =
            normalize_numbered_param_decls(&proc.params, "param", inferred_names.max_param);
        let (ins, _, _, in_array_slots) = expand_proc_port_specs(
            &proc.name,
            &ins_ports,
            "input",
            proc_options,
            &mut scratch_errors,
        );
        let (outs, _, _, out_array_slots) = expand_proc_port_specs(
            &proc.name,
            &outs_ports,
            "output",
            proc_options,
            &mut scratch_errors,
        );
        let (param_specs, _) =
            expand_proc_param_specs(&proc.name, &params, proc_options, &mut scratch_errors);

        let mut surface = ChildProcSurface::default();
        surface.inputs.extend(ins);
        surface.inputs.extend(in_array_slots.keys().cloned());
        surface.input_arrays.extend(in_array_slots.keys().cloned());
        if surface.inputs.len() > 1 {
            surface.inputs.insert("ins".to_owned());
            surface.input_arrays.insert("ins".to_owned());
        }
        surface.outputs.extend(outs);
        surface.outputs.extend(out_array_slots.keys().cloned());
        surface
            .output_arrays
            .extend(out_array_slots.keys().cloned());
        if surface.outputs.len() > 1 {
            match proc.outs_timing {
                OutputTiming::Sample => {
                    surface.outputs.insert("outs".to_owned());
                    surface.output_arrays.insert("outs".to_owned());
                }
                OutputTiming::Block => {
                    surface.outputs.insert("kouts".to_owned());
                    surface.output_arrays.insert("kouts".to_owned());
                }
            }
        }
        for spec in &param_specs {
            if spec.is_pinned() {
                continue;
            }
            surface.params.insert(spec.name.clone());
            surface
                .params
                .extend(spec.slots.iter().map(|slot| slot.name.clone()));
            if spec.slots.len() > 1 {
                surface.param_arrays.insert(spec.name.clone());
            }
        }
        if param_specs
            .iter()
            .filter(|spec| !spec.is_pinned())
            .map(|spec| spec.slots.len())
            .sum::<usize>()
            > 1
        {
            surface.params.insert("params".to_owned());
            surface.param_arrays.insert("params".to_owned());
        }
        out.insert(proc.name.clone(), surface);
    }
    out
}

fn nested_proc_name_for_receiver<'a>(
    receiver: &str,
    state: &'a ProcStateFields,
    nested_proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<&'a str> {
    let receiver = receiver.strip_prefix("self.").unwrap_or(receiver);
    if let Some(nested) = state.nested_procs.get(receiver) {
        return Some(nested.proc_name.as_str());
    }
    if let Some(nested_array) = state.nested_proc_arrays.get(receiver) {
        return Some(nested_array.proc_name.as_str());
    }
    if let Some((base, rest)) = receiver.split_once('[') {
        if rest.ends_with(']') {
            if let Some(nested_array) = state.nested_proc_arrays.get(base) {
                return Some(nested_array.proc_name.as_str());
            }
        }
    }
    if let Some(array_base) = nested_proc_array_slots.iter().find_map(|(base, slots)| {
        slots
            .iter()
            .any(|slot| slot == receiver)
            .then_some(base.as_str())
    }) {
        if let Some(nested_array) = state.nested_proc_arrays.get(array_base) {
            return Some(nested_array.proc_name.as_str());
        }
    }
    None
}

fn child_proc_write_target<'a>(
    path: &'a str,
    ctx: &HookSafeCtx<'_>,
) -> Option<(&'a str, String, String)> {
    let (receiver, field) = simple_dot_path(path)?;
    if nested_proc_name_for_receiver(receiver, ctx.state, ctx.nested_proc_array_slots).is_some() {
        return Some((receiver, field.to_owned(), format!("{receiver}.{field}")));
    }
    let (receiver_root, receiver_tail) = simple_dot_path(receiver)?;
    if nested_proc_name_for_receiver(receiver_root, ctx.state, ctx.nested_proc_array_slots)
        .is_some()
    {
        let field_path = format!("{receiver_tail}.{field}");
        return Some((
            receiver_root,
            field_path.clone(),
            format!("{receiver_root}.{field_path}"),
        ));
    }
    None
}

fn immediate_child_field(field_path: &str) -> &str {
    field_path.split(['.', '[']).next().unwrap_or(field_path)
}

fn child_proc_write_kind(
    receiver: &str,
    field_path: &str,
    ctx: &HookSafeCtx<'_>,
) -> ChildProcWriteKind {
    let Some(proc_name) =
        nested_proc_name_for_receiver(receiver, ctx.state, ctx.nested_proc_array_slots)
    else {
        return ChildProcWriteKind::InternalState;
    };
    let field = immediate_child_field(field_path);
    let Some(surface) = ctx.child_proc_surfaces.get(proc_name) else {
        return ChildProcWriteKind::InternalState;
    };
    if field == "params" {
        return ChildProcWriteKind::DynamicParams;
    }
    if surface.params.contains(field) {
        ChildProcWriteKind::Param
    } else if surface.inputs.contains(field) {
        ChildProcWriteKind::Input
    } else if surface.outputs.contains(field) {
        ChildProcWriteKind::Output
    } else {
        ChildProcWriteKind::InternalState
    }
}

fn raw_local_hook_name_from_call(ctx: &HookSafeCtx<'_>, name: &str) -> Option<String> {
    if ctx.local_defs_by_name.contains_key(name) {
        return Some(name.to_owned());
    }
    let prefix = format!("{}{}", ctx.owner_proc, PROC_LOCAL_DEF_FN_PREFIX);
    let raw = name.strip_prefix(prefix.as_str())?;
    ctx.local_defs_by_name
        .contains_key(raw)
        .then_some(raw.to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookBodyMode {
    ProcLocal,
    OrdinaryHelper,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum HookPlaceKind {
    OwnerParam,
    OwnerInput,
    OwnerOutput,
    OwnerState,
    ChildParam,
    ChildDynamicParams,
    ChildInput,
    ChildOutput,
    ChildState,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HookPlace {
    display: String,
    kind: HookPlaceKind,
}

#[derive(Clone, Debug)]
struct HookValidationFrame {
    mode: HookBodyMode,
    aliases: HashMap<String, HookPlace>,
    locals: HashSet<String>,
}

impl HookValidationFrame {
    fn for_def(def: &FunctionDef, mode: HookBodyMode) -> Self {
        Self {
            mode,
            aliases: HashMap::new(),
            locals: def.params.iter().map(|param| param.name.clone()).collect(),
        }
    }

    fn add_local(&mut self, name: impl Into<String>) {
        self.locals.insert(name.into());
    }
}

fn hook_root_symbol(name: &str) -> &str {
    name.split(['.', '[']).next().unwrap_or(name)
}

fn hook_child_place_for_path(path: &str, ctx: &HookSafeCtx<'_>) -> Option<HookPlace> {
    let (receiver, field_path, display_path) = child_proc_write_target(path, ctx)?;
    let kind = match child_proc_write_kind(receiver, &field_path, ctx) {
        ChildProcWriteKind::Param => HookPlaceKind::ChildParam,
        ChildProcWriteKind::DynamicParams => HookPlaceKind::ChildDynamicParams,
        ChildProcWriteKind::Input => HookPlaceKind::ChildInput,
        ChildProcWriteKind::Output => HookPlaceKind::ChildOutput,
        ChildProcWriteKind::InternalState => HookPlaceKind::ChildState,
    };
    Some(HookPlace {
        display: display_path,
        kind,
    })
}

fn hook_place_from_alias(name: &str, frame: &HookValidationFrame) -> Option<HookPlace> {
    if let Some(place) = frame.aliases.get(name) {
        return Some(place.clone());
    }
    for (alias, place) in &frame.aliases {
        let Some(suffix) = name.strip_prefix(alias) else {
            continue;
        };
        if let Some(field_suffix) = suffix.strip_prefix('.') {
            return Some(HookPlace {
                display: format!("{}.{}", place.display, field_suffix),
                kind: place.kind.clone(),
            });
        }
    }
    None
}

fn hook_place_for_name(
    name: &str,
    ctx: &HookSafeCtx<'_>,
    frame: &HookValidationFrame,
) -> Option<HookPlace> {
    if let Some(place) = hook_place_from_alias(name, frame) {
        return Some(place);
    }
    if frame.locals.contains(hook_root_symbol(name)) {
        return None;
    }
    if frame.mode == HookBodyMode::OrdinaryHelper {
        return None;
    }
    if ctx.owner_param_names.contains(name) || name == "params" {
        return Some(HookPlace {
            display: name.to_owned(),
            kind: HookPlaceKind::OwnerParam,
        });
    }
    if ctx.input_names.contains(name) || matches!(name, "ins" | "kins") {
        return Some(HookPlace {
            display: name.to_owned(),
            kind: HookPlaceKind::OwnerInput,
        });
    }
    if ctx.output_names.contains(name) || matches!(name, "outs" | "kouts") {
        return Some(HookPlace {
            display: name.to_owned(),
            kind: HookPlaceKind::OwnerOutput,
        });
    }
    if let Some(place) = hook_child_place_for_path(name, ctx) {
        return Some(place);
    }
    if ctx.state.has_any(name) || ctx.state.has_any(hook_root_symbol(name)) {
        return Some(HookPlace {
            display: name.to_owned(),
            kind: HookPlaceKind::OwnerState,
        });
    }
    Some(HookPlace {
        display: name.to_owned(),
        kind: HookPlaceKind::Unknown,
    })
}

fn hook_place_for_alias_arg(
    expr: &Expr,
    ctx: &HookSafeCtx<'_>,
    frame: &HookValidationFrame,
) -> Option<HookPlace> {
    match expr {
        Expr::Var { name, .. } | Expr::Slice { base: name, .. } => {
            hook_place_for_name(name, ctx, frame)
        }
        _ => None,
    }
}

fn hook_child_place_is_array(path: &str, ctx: &HookSafeCtx<'_>) -> bool {
    let Some((receiver, field_path, _display_path)) = child_proc_write_target(path, ctx) else {
        return false;
    };
    let Some(proc_name) =
        nested_proc_name_for_receiver(receiver, ctx.state, ctx.nested_proc_array_slots)
    else {
        return false;
    };
    let field = immediate_child_field(&field_path);
    let Some(surface) = ctx.child_proc_surfaces.get(proc_name) else {
        return false;
    };
    surface.param_arrays.contains(field)
        || surface.input_arrays.contains(field)
        || surface.output_arrays.contains(field)
}

fn hook_untyped_arg_may_alias(actual: &Expr, place: &HookPlace, ctx: &HookSafeCtx<'_>) -> bool {
    match actual {
        Expr::Slice { .. } => true,
        Expr::Var { name, .. } => match place.kind {
            HookPlaceKind::OwnerParam => ctx.owner_param_array_names.contains(name),
            HookPlaceKind::OwnerInput => ctx.input_array_names.contains(name),
            HookPlaceKind::OwnerOutput => ctx.output_array_names.contains(name),
            HookPlaceKind::OwnerState => {
                ctx.state.has_non_scalar(name) || ctx.state.has_non_scalar(hook_root_symbol(name))
            }
            HookPlaceKind::ChildDynamicParams => true,
            HookPlaceKind::ChildParam | HookPlaceKind::ChildInput | HookPlaceKind::ChildOutput => {
                hook_child_place_is_array(name, ctx)
            }
            HookPlaceKind::ChildState => true,
            HookPlaceKind::Unknown => false,
        },
        _ => false,
    }
}

fn hook_place_is_dynamic_param_surface(place: &HookPlace) -> bool {
    matches!(place.kind, HookPlaceKind::ChildDynamicParams)
        || (matches!(place.kind, HookPlaceKind::OwnerParam) && place.display == "params")
}

fn hook_dynamic_param_surface_place(
    name: &str,
    ctx: &HookSafeCtx<'_>,
    frame: &HookValidationFrame,
) -> Option<HookPlace> {
    let place = hook_place_for_name(name, ctx, frame)?;
    hook_place_is_dynamic_param_surface(&place).then_some(place)
}

fn reject_hook_dynamic_param_surface_use(
    place: &HookPlace,
    loc: SourceLoc,
    ctx: &HookSafeCtx<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    push_semantic(
        DiagCtx::new(loc),
        errors,
        format!(
            "bind hook in processor '{}' cannot use dynamic param array '{}'; dynamic params are only available as direct indexed accesses in block or sample",
            ctx.owner_proc, place.display
        ),
    );
}

fn hook_param_may_alias(
    param: &onda_frontend::FnParamDecl,
    actual: &Expr,
    place: &HookPlace,
    ctx: &HookSafeCtx<'_>,
) -> bool {
    match param.ty.as_ref() {
        Some(FnParamType::Primitive(_) | FnParamType::Tuple(_)) => false,
        Some(
            FnParamType::Struct(_)
            | FnParamType::Buffer(_)
            | FnParamType::BufferArray { .. }
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::SizedArray { .. }
            | FnParamType::BareBuffer,
        ) => true,
        None => hook_untyped_arg_may_alias(actual, place, ctx),
    }
}

fn reject_hook_place_write(
    place: &HookPlace,
    ctx: &HookSafeCtx<'_>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    match place.kind {
        HookPlaceKind::OwnerParam => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' cannot assign owner param '{}'",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::OwnerInput => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' cannot write input '{}'",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::OwnerOutput => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' cannot write output '{}'",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::ChildDynamicParams => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' cannot assign child proc dynamic params '{}'",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::ChildInput => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' cannot write child proc input '{}'",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::ChildOutput => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' cannot write child proc output '{}'",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::ChildState => {
            push_semantic(
                diag,
                errors,
                format!(
                    "bind hook in processor '{}' can only assign child proc params; '{}' is child proc state",
                    ctx.owner_proc, place.display
                ),
            );
        }
        HookPlaceKind::ChildParam | HookPlaceKind::OwnerState | HookPlaceKind::Unknown => {}
    }
}

fn reject_hook_target_write(
    target: &AssignTarget,
    ctx: &HookSafeCtx<'_>,
    frame: &HookValidationFrame,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    let mut check_symbol = |name: &str| {
        if let Some(place) = hook_place_for_name(name, ctx, frame) {
            reject_hook_place_write(&place, ctx, diag, errors);
        }
    };

    match target {
        AssignTarget::Var(name) => check_symbol(name),
        AssignTarget::Index { base, .. } | AssignTarget::Slice { base, .. } => check_symbol(base),
        AssignTarget::Tuple(names) => {
            for name in names {
                check_symbol(name);
            }
        }
    }
}

fn hook_validation_key(name: &str, def: &FunctionDef, frame: &HookValidationFrame) -> String {
    let mut key = format!("{:?}:{name}", frame.mode);
    for param in &def.params {
        let Some(place) = frame.aliases.get(&param.name) else {
            continue;
        };
        key.push('|');
        key.push_str(&param.name);
        key.push('=');
        key.push_str(&format!("{:?}:{}", place.kind, place.display));
    }
    key
}

fn hook_call_actuals_by_param<'a>(def: &FunctionDef, args: &'a [CallArg]) -> Vec<Option<&'a Expr>> {
    let mut out = Vec::<Option<&Expr>>::with_capacity(def.params.len());
    let mut positional = args.iter().filter(|arg| arg.name.is_none());
    for param in &def.params {
        if let Some(named) = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some(param.name.as_str()))
        {
            out.push(Some(&named.expr));
        } else {
            out.push(positional.next().map(|arg| &arg.expr));
        }
    }
    out
}

fn resolve_hook_call<'a>(
    name: &str,
    ctx: &HookSafeCtx<'a>,
) -> Option<(String, &'a FunctionDef, HookBodyMode)> {
    if let Some(local_name) = raw_local_hook_name_from_call(ctx, name) {
        let def = ctx.local_defs_by_name.get(&local_name).copied()?;
        return Some((local_name, def, HookBodyMode::ProcLocal));
    }
    let def = ctx.fn_defs_by_name.get(name).copied()?;
    Some((name.to_owned(), def, HookBodyMode::OrdinaryHelper))
}

fn validate_hook_safe_function(
    call_name: &str,
    def: &FunctionDef,
    frame: HookValidationFrame,
    ctx: &HookSafeCtx<'_>,
    visiting: &mut HashSet<String>,
    validated: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let key = hook_validation_key(call_name, def, &frame);
    if validated.contains(&key) || !visiting.insert(key.clone()) {
        return;
    }
    validate_hook_safe_stmts(
        &def.body,
        ctx,
        &mut frame.clone(),
        visiting,
        validated,
        errors,
    );
    visiting.remove(&key);
    validated.insert(key);
}

fn validate_hook_safe_def(
    hook_name: &str,
    ctx: &HookSafeCtx<'_>,
    visiting: &mut HashSet<String>,
    validated: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(def) = ctx.local_defs_by_name.get(hook_name).copied() else {
        return;
    };
    validate_hook_safe_function(
        hook_name,
        def,
        HookValidationFrame::for_def(def, HookBodyMode::ProcLocal),
        ctx,
        visiting,
        validated,
        errors,
    );
}

fn validate_hook_safe_expr(
    expr: &Expr,
    ctx: &HookSafeCtx<'_>,
    frame: &HookValidationFrame,
    visiting: &mut HashSet<String>,
    validated: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::Var { name, loc } => {
            if let Some(place) = hook_dynamic_param_surface_place(name, ctx, frame) {
                reject_hook_dynamic_param_surface_use(&place, (*loc).into(), ctx, errors);
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                validate_hook_safe_expr(value, ctx, frame, visiting, validated, errors);
            }
        }
        Expr::Index { base, index, loc } => {
            if let Some(place) = hook_dynamic_param_surface_place(base, ctx, frame) {
                reject_hook_dynamic_param_surface_use(&place, (*loc).into(), ctx, errors);
            }
            validate_hook_safe_expr(index, ctx, frame, visiting, validated, errors)
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            loc,
        } => {
            if let Some(place) = hook_dynamic_param_surface_place(base, ctx, frame) {
                reject_hook_dynamic_param_surface_use(&place, (*loc).into(), ctx, errors);
            }
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                validate_hook_safe_expr(coordinate, ctx, frame, visiting, validated, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            validate_hook_safe_expr(&spec.size, ctx, frame, visiting, validated, errors);
            if let Some(values) = init {
                for value in values {
                    validate_hook_safe_expr(value, ctx, frame, visiting, validated, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_hook_safe_expr(lhs, ctx, frame, visiting, validated, errors);
            validate_hook_safe_expr(rhs, ctx, frame, visiting, validated, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_hook_safe_expr(arg, ctx, frame, visiting, validated, errors);
            }
        }
        Expr::UserCall {
            name, args, loc, ..
        } => {
            for arg in args {
                validate_hook_safe_expr(&arg.expr, ctx, frame, visiting, validated, errors);
            }
            if !name.contains('.')
                && nested_proc_receiver_root(name, ctx.state, ctx.nested_proc_array_slots).is_some()
            {
                push_semantic(
                    DiagCtx::new(*loc),
                    errors,
                    format!(
                        "bind hook in processor '{}' cannot call child proc receiver '{}(...)'",
                        ctx.owner_proc, name
                    ),
                );
            }
            if let Some((base, method)) = simple_dot_path(name) {
                if nested_proc_receiver_root(base, ctx.state, ctx.nested_proc_array_slots).is_some()
                {
                    push_semantic(
                        DiagCtx::new(*loc),
                        errors,
                        format!(
                            "bind hook in processor '{}' cannot call child proc receiver '{}.{}(...)'",
                            ctx.owner_proc, base, method
                        ),
                    );
                }
            }
            if name.ends_with(PROC_INIT_FN_SUFFIX) || name.contains(PROC_EVENT_FN_PREFIX) {
                push_semantic(
                    DiagCtx::new(*loc),
                    errors,
                    format!(
                        "bind hook in processor '{}' cannot call processor events",
                        ctx.owner_proc
                    ),
                );
            }
            if let Some((resolved_name, def, mode)) = resolve_hook_call(name, ctx) {
                let mut callee_frame = HookValidationFrame::for_def(def, mode);
                for (param, actual) in def.params.iter().zip(hook_call_actuals_by_param(def, args))
                {
                    let Some(actual) = actual else {
                        continue;
                    };
                    if let Some(place) = hook_place_for_alias_arg(actual, ctx, frame) {
                        if !hook_param_may_alias(param, actual, &place, ctx) {
                            continue;
                        }
                        callee_frame.aliases.insert(param.name.clone(), place);
                    }
                }
                validate_hook_safe_function(
                    &resolved_name,
                    def,
                    callee_frame,
                    ctx,
                    visiting,
                    validated,
                    errors,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_hook_safe_expr(expr, ctx, frame, visiting, validated, errors);
        }
    }
}

fn validate_hook_safe_stmts(
    stmts: &[Stmt],
    ctx: &HookSafeCtx<'_>,
    frame: &mut HookValidationFrame,
    visiting: &mut HashSet<String>,
    validated: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        let diag = DiagCtx::new(stmt.loc().cloned().unwrap_or_default());
        match stmt {
            Stmt::Const { decl, .. } => {
                validate_hook_safe_expr(&decl.expr, ctx, frame, visiting, validated, errors);
                frame.add_local(decl.name.clone());
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign {
                target,
                expr,
                is_typed_decl,
                ..
            } => {
                if !is_typed_decl {
                    reject_hook_target_write(target, ctx, frame, diag, errors);
                }
                validate_hook_safe_expr(expr, ctx, frame, visiting, validated, errors);
                match target {
                    AssignTarget::Index { index, .. } => {
                        validate_hook_safe_expr(index, ctx, frame, visiting, validated, errors);
                    }
                    AssignTarget::Slice {
                        selector,
                        channel,
                        start,
                        end,
                        ..
                    } => {
                        for coordinate in [selector, channel, start, end].into_iter().flatten() {
                            validate_hook_safe_expr(
                                coordinate, ctx, frame, visiting, validated, errors,
                            );
                        }
                    }
                    AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
                }
                if *is_typed_decl {
                    match target {
                        AssignTarget::Var(name) => frame.add_local(name.clone()),
                        AssignTarget::Tuple(names) => {
                            for name in names {
                                frame.add_local(name.clone());
                            }
                        }
                        AssignTarget::Index { .. } | AssignTarget::Slice { .. } => {}
                    }
                }
            }
            Stmt::Expr { expr, .. } => {
                validate_hook_safe_expr(expr, ctx, frame, visiting, validated, errors);
            }
            Stmt::Return { expr, .. } => {
                validate_hook_safe_expr(expr, ctx, frame, visiting, validated, errors);
                if frame.mode == HookBodyMode::ProcLocal {
                    push_semantic(
                        diag,
                        errors,
                        format!(
                            "bind hook in processor '{}' cannot contain return",
                            ctx.owner_proc
                        ),
                    );
                }
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_hook_safe_expr(cond, ctx, frame, visiting, validated, errors);
                validate_hook_safe_stmts(
                    then_branch,
                    ctx,
                    &mut frame.clone(),
                    visiting,
                    validated,
                    errors,
                );
                validate_hook_safe_stmts(
                    else_branch,
                    ctx,
                    &mut frame.clone(),
                    visiting,
                    validated,
                    errors,
                );
            }
            Stmt::For {
                var,
                start,
                end,
                step,
                body,
                ..
            } => {
                validate_hook_safe_expr(start, ctx, frame, visiting, validated, errors);
                validate_hook_safe_expr(end, ctx, frame, visiting, validated, errors);
                if let Some(step) = step {
                    validate_hook_safe_expr(step, ctx, frame, visiting, validated, errors);
                }
                let mut body_frame = frame.clone();
                body_frame.add_local(var.clone());
                validate_hook_safe_stmts(body, ctx, &mut body_frame, visiting, validated, errors);
            }
            Stmt::While { cond, body, .. } => {
                validate_hook_safe_expr(cond, ctx, frame, visiting, validated, errors);
                validate_hook_safe_stmts(
                    body,
                    ctx,
                    &mut frame.clone(),
                    visiting,
                    validated,
                    errors,
                );
            }
        }
    }
}

fn validate_proc_param_binds(
    proc: &ProcessorDef,
    param_specs: &[ProcParamSpec],
    fn_defs_full: &[FunctionDef],
    owner_param_array_names: &HashSet<String>,
    input_names: &HashSet<String>,
    input_array_names: &HashSet<String>,
    output_names: &HashSet<String>,
    output_array_names: &HashSet<String>,
    state: &ProcStateFields,
    nested_proc_array_slots: &HashMap<String, Vec<String>>,
    child_proc_surfaces: &HashMap<String, ChildProcSurface>,
    errors: &mut Vec<Diagnostic>,
) {
    let hooks = proc_bound_param_hook_calls(param_specs);
    if hooks.is_empty() {
        return;
    }
    reject_dynamic_bound_param_assignments(proc, errors);

    let local_defs_by_name = proc
        .local_defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let fn_defs_by_name = fn_defs_full
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let owner_param_names = bound_proc_param_names(param_specs);
    let ctx = HookSafeCtx {
        owner_proc: &proc.name,
        local_defs_by_name: &local_defs_by_name,
        fn_defs_by_name: &fn_defs_by_name,
        owner_param_names: &owner_param_names,
        owner_param_array_names,
        input_names,
        input_array_names,
        output_names,
        output_array_names,
        state,
        nested_proc_array_slots,
        child_proc_surfaces,
    };

    let mut checked_targets = HashSet::<String>::new();
    let mut validated_safe = HashSet::<String>::new();
    for hook in hooks {
        let Some(def) = local_defs_by_name.get(&hook).copied() else {
            push_semantic(
                DiagCtx::new(proc.loc),
                errors,
                format!(
                    "processor '{}' param bind target '{}' is missing; bind targets must be proc-local defs in the same processor",
                    proc.name, hook
                ),
            );
            continue;
        };
        if checked_targets.insert(hook.clone()) {
            if !def.params.is_empty() {
                push_semantic(
                    DiagCtx::new(def.loc),
                    errors,
                    format!(
                        "processor '{}' bind target '{}' must take zero parameters",
                        proc.name, hook
                    ),
                );
            }
            if def.return_ty.is_some() {
                push_semantic(
                    DiagCtx::new(def.return_ty_loc.or(def.loc)),
                    errors,
                    format!(
                        "processor '{}' bind target '{}' must not declare a return type",
                        proc.name, hook
                    ),
                );
            }
            if stmt_list_contains_return(&def.body) {
                push_semantic(
                    DiagCtx::new(def.loc),
                    errors,
                    format!(
                        "processor '{}' bind target '{}' must not contain return",
                        proc.name, hook
                    ),
                );
            }
        }
        validate_hook_safe_def(
            &hook,
            &ctx,
            &mut HashSet::new(),
            &mut validated_safe,
            errors,
        );
    }
}

fn reject_dynamic_bound_param_assignments(proc: &ProcessorDef, errors: &mut Vec<Diagnostic>) {
    fn check_stmts(proc_name: &str, stmts: &[Stmt], errors: &mut Vec<Diagnostic>) {
        for stmt in stmts {
            let diag = DiagCtx::new(stmt.loc().cloned().unwrap_or_default());
            match stmt {
                Stmt::Assign {
                    target: AssignTarget::Index { base, .. } | AssignTarget::Slice { base, .. },
                    ..
                } if base == "params" => {
                    push_semantic(
                        diag,
                        errors,
                        format!(
                            "processor '{proc_name}' has bound params, so assignment through dynamic params[...] is not supported; assign the named param instead"
                        ),
                    );
                }
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    check_stmts(proc_name, then_branch, errors);
                    check_stmts(proc_name, else_branch, errors);
                }
                Stmt::For { body, .. } | Stmt::While { body, .. } => {
                    check_stmts(proc_name, body, errors);
                }
                Stmt::Const { .. }
                | Stmt::Assign { .. }
                | Stmt::Expr { .. }
                | Stmt::Return { .. }
                | Stmt::Break { .. }
                | Stmt::Continue { .. } => {}
            }
        }
    }

    check_stmts(&proc.name, &proc.init.body, errors);
    check_stmts(&proc.name, &proc.block_pre, errors);
    check_stmts(&proc.name, &proc.sample, errors);
    check_stmts(&proc.name, &proc.block_post, errors);
    for event in &proc.events {
        check_stmts(&proc.name, &event.body, errors);
    }
    for def in &proc.local_defs {
        check_stmts(&proc.name, &def.body, errors);
    }
}

fn insert_declared_array_bases(
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    arrays: &HashMap<String, TypedArrayInfo>,
) {
    for (name, info) in arrays {
        insert_declared_symbol(
            state_scalars,
            declared_symbols,
            name.clone(),
            DeclaredSymbolInfo::DataArray {
                elem_ty: info.elem_ty,
            },
        );
    }
}

#[derive(Default)]
struct SurfaceValidationScratch {
    known_scalars: HashSet<String>,
    state_scalars: HashMap<String, PrimitiveType>,
    outputs: HashSet<String>,
    array_vars: HashMap<String, usize>,
    declared_symbols: DeclaredSymbolMap,
    param_structs: HashMap<String, String>,
    struct_instances: HashMap<String, String>,
    struct_defs: HashMap<String, Vec<TypedStructField>>,
    fn_signatures: HashMap<String, FnSignature>,
}

#[allow(clippy::too_many_arguments)]
fn proc_local_surface_expr_env<'a>(
    scratch: &'a SurfaceValidationScratch,
    locals: &'a HashSet<String>,
    io_surface_names: &'a HashSet<String>,
    io_surface_array_names: &'a HashSet<String>,
    dynamic_param_array_names: &'a HashSet<String>,
) -> ExprEnv<'a> {
    let mut env = build_expr_env(
        &scratch.known_scalars,
        &scratch.state_scalars,
        locals,
        &scratch.outputs,
        &scratch.array_vars,
        &scratch.declared_symbols,
        &scratch.param_structs,
        &scratch.struct_instances,
        &scratch.struct_defs,
        &scratch.fn_signatures,
        ScopeKind::Def,
    );
    env.io_surface_names = io_surface_names;
    env.io_surface_array_names = io_surface_array_names;
    env.io_surface_access_allowed = false;
    env.dynamic_param_arrays = dynamic_param_array_names;
    env.dynamic_param_indexing_allowed = false;
    env
}

fn proc_local_target_local_names(target: &AssignTarget) -> Vec<&String> {
    match target {
        AssignTarget::Var(name) => vec![name],
        AssignTarget::Tuple(names) => names.iter().collect(),
        AssignTarget::Index { .. } | AssignTarget::Slice { .. } => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_proc_local_def_surface_stmt(
    stmt: &Stmt,
    scratch: &SurfaceValidationScratch,
    locals: &mut HashSet<String>,
    io_surface_names: &HashSet<String>,
    io_surface_array_names: &HashSet<String>,
    dynamic_param_array_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign {
            target,
            target_loc,
            expr,
            ..
        } => {
            let introduced_locals = {
                let env = proc_local_surface_expr_env(
                    scratch,
                    locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                );
                validate_block_bound_surface_assign_target(
                    target,
                    target_loc.as_ref().into(),
                    env,
                    errors,
                );
                validate_block_bound_surface_expr(expr, env, errors);
                proc_local_target_local_names(target)
                    .into_iter()
                    .filter(|name| {
                        io_surface_name(name, env).is_none()
                            && dynamic_param_surface_name(name, env).is_none()
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };
            for name in introduced_locals {
                locals.insert(name);
            }
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            let env = proc_local_surface_expr_env(
                scratch,
                locals,
                io_surface_names,
                io_surface_array_names,
                dynamic_param_array_names,
            );
            validate_block_bound_surface_expr(expr, env, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            {
                let env = proc_local_surface_expr_env(
                    scratch,
                    locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                );
                validate_block_bound_surface_expr(cond, env, errors);
            }
            let mut then_locals = locals.clone();
            for nested in then_branch {
                validate_proc_local_def_surface_stmt(
                    nested,
                    scratch,
                    &mut then_locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                    errors,
                );
            }
            let mut else_locals = locals.clone();
            for nested in else_branch {
                validate_proc_local_def_surface_stmt(
                    nested,
                    scratch,
                    &mut else_locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                    errors,
                );
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
            {
                let env = proc_local_surface_expr_env(
                    scratch,
                    locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                );
                validate_block_bound_surface_expr(start, env, errors);
                validate_block_bound_surface_expr(end, env, errors);
                if let Some(step) = step {
                    validate_block_bound_surface_expr(step, env, errors);
                }
            }
            let mut body_locals = locals.clone();
            body_locals.insert(var.clone());
            for nested in body {
                validate_proc_local_def_surface_stmt(
                    nested,
                    scratch,
                    &mut body_locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                    errors,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            {
                let env = proc_local_surface_expr_env(
                    scratch,
                    locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                );
                validate_block_bound_surface_expr(cond, env, errors);
            }
            let mut body_locals = locals.clone();
            for nested in body {
                validate_proc_local_def_surface_stmt(
                    nested,
                    scratch,
                    &mut body_locals,
                    io_surface_names,
                    io_surface_array_names,
                    dynamic_param_array_names,
                    errors,
                );
            }
        }
    }
}

fn validate_proc_local_def_surfaces(
    proc: &ProcessorDef,
    io_surface_names: &HashSet<String>,
    io_surface_array_names: &HashSet<String>,
    dynamic_param_array_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if proc.local_defs.is_empty() {
        return;
    }
    let scratch = SurfaceValidationScratch::default();
    for def in &proc.local_defs {
        let mut locals = def
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect::<HashSet<_>>();
        for stmt in &def.body {
            validate_proc_local_def_surface_stmt(
                stmt,
                &scratch,
                &mut locals,
                io_surface_names,
                io_surface_array_names,
                dynamic_param_array_names,
                errors,
            );
        }
    }
}

pub(super) fn compute_proc_shape(
    proc: &mut onda_frontend::ProcessorDef,
    sample_oversample_factor: usize,
    options: AnalysisOptions,
    proc_symbols: &HashSet<String>,
    struct_defs: &HashMap<String, onda_frontend::StructDef>,
    ctor_symbols: &HashSet<String>,
    fn_return_types: &HashMap<String, ReturnType>,
    fn_signatures_full: &HashMap<String, FnSignature>,
    fn_defs_full: &[FunctionDef],
    proc_defs_by_name: &HashMap<String, onda_frontend::ProcessorDef>,
    const_arrays: &HashMap<String, TypedArrayInfo>,
    errors: &mut Vec<Diagnostic>,
) -> ProcBaseShape {
    let struct_symbols = struct_defs.keys().cloned().collect::<HashSet<_>>();
    let proc_options = proc_runtime_analysis_options(options, sample_oversample_factor);
    let child_proc_surfaces = build_child_proc_surfaces(proc_defs_by_name, options);
    check_local_param_duplicates(&proc.params, errors);
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let inferred_names = infer_numbered_names_from_proc(proc);
    let ins_ports = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
    let out_inferred_max = match proc.outs_timing {
        OutputTiming::Sample => inferred_io.max_out,
        OutputTiming::Block => inferred_names.max_kout,
    };
    let out_ports = normalize_numbered_port_decls(
        &proc.outs,
        proc_output_numbered_prefix(proc),
        out_inferred_max,
    );
    let params = normalize_numbered_param_decls(&proc.params, "param", inferred_names.max_param);
    let (ins, in_types, in_ports, mut in_array_slots) =
        expand_proc_port_specs(&proc.name, &ins_ports, "input", proc_options, errors);
    let (outs, out_types, _out_ports, out_array_slots) =
        expand_proc_port_specs(&proc.name, &out_ports, "output", proc_options, errors);
    let (param_specs, mut field_array_slots) =
        expand_proc_param_specs(&proc.name, &params, proc_options, errors);
    let port_index_params = uniform_port_index_info_from_types(
        true,
        param_specs.iter().map(|spec| spec.slots.len()).sum(),
        param_specs
            .iter()
            .flat_map(|spec| spec.slots.iter().map(|slot| slot.ty)),
    );
    let mut owner_dynamic_param_array_names = HashSet::<String>::new();
    if port_index_params.is_some() {
        owner_dynamic_param_array_names.insert("params".to_owned());
    }
    let proc_event_names = proc
        .events
        .iter()
        .map(|event| event.name.clone())
        .collect::<HashSet<_>>();
    let proc_input_arrays = array_infos_from_slot_map(&in_array_slots, &in_types);
    let proc_output_arrays = array_infos_from_slot_map(&out_array_slots, &out_types);
    let proc_param_arrays = array_infos_from_param_specs(&param_specs);
    let buffer_specs = coerce_buffers(&proc.buffers, proc_options, errors)
        .into_iter()
        .map(|b| ProcBufferSpec {
            name: b.name,
            elem_ty: b.elem_ty,
            channels: b.channels,
            array_len: b.array_len,
            is_array: b.is_array,
        })
        .collect::<Vec<_>>();
    for (name, slots) in out_array_slots {
        field_array_slots.insert(name, slots);
    }

    // Add synthetic "ins"/"outs"/"kouts"/"params" array-slot entries for uniform scalar ports
    // so that dynamic indexing (e.g. outs[i], kouts[i], ins[i], params[i]) can be rewritten to
    // helper function calls during proc lowering.
    if ins.len() > 1
        && !in_array_slots.contains_key("ins")
        && uniform_port_index_info_from_names(true, &ins, &in_types).is_some()
    {
        in_array_slots.insert("ins".to_owned(), ins.clone());
    }
    if proc.outs_timing == OutputTiming::Sample
        && outs.len() > 1
        && !field_array_slots.contains_key("outs")
        && uniform_port_index_info_from_names(true, &outs, &out_types).is_some()
    {
        field_array_slots.insert("outs".to_owned(), outs.clone());
    }
    if proc.outs_timing == OutputTiming::Block
        && outs.len() > 1
        && !field_array_slots.contains_key("kouts")
        && uniform_port_index_info_from_names(true, &outs, &out_types).is_some()
    {
        field_array_slots.insert("kouts".to_owned(), outs.clone());
    }
    if param_specs.len() > 1 && !field_array_slots.contains_key("params") {
        let all_scalar_slots: Vec<String> = param_specs
            .iter()
            .flat_map(|s| s.slots.iter().map(|slot| slot.name.clone()))
            .collect();
        if all_scalar_slots.len() > 1 {
            let first_ty = param_specs[0]
                .slots
                .first()
                .map(|s| s.ty)
                .unwrap_or(PrimitiveType::F32);
            if param_specs
                .iter()
                .flat_map(|s| &s.slots)
                .all(|s| s.ty == first_ty)
            {
                field_array_slots.insert("params".to_owned(), all_scalar_slots);
            }
        }
    }

    let mut typed_struct_defs = struct_defs_for_scalar_expr_inference(struct_defs);
    let mut typed_param_names = HashSet::<String>::new();
    for spec in &param_specs {
        typed_param_names.insert(spec.name.clone());
        for slot in &spec.slots {
            typed_param_names.insert(slot.name.clone());
        }
    }
    let mut param_names = typed_param_names.clone();
    for buffer in &buffer_specs {
        if param_names.contains(&buffer.name) {
            push_semantic(
                DiagCtx::new(proc.loc),
                errors,
                format!(
                    "processor '{}' buffer '{}' conflicts with param name",
                    proc.name, buffer.name
                ),
            );
        }
        param_names.insert(buffer.name.clone());
    }
    let mut ins_names = ins.iter().cloned().collect::<HashSet<_>>();
    for spec in &in_ports {
        ins_names.insert(spec.name.clone());
    }
    let mut out_names = outs.iter().cloned().collect::<HashSet<_>>();
    for port in &out_ports {
        out_names.insert(port.name.clone());
    }
    let proc_output_array_names = proc_output_arrays.keys().cloned().collect::<HashSet<_>>();
    let mut proc_io_output_array_names = proc_output_array_names.clone();
    for synthetic in ["outs", "kouts"] {
        if field_array_slots.contains_key(synthetic) {
            proc_io_output_array_names.insert(synthetic.to_owned());
        }
    }
    let mut proc_io_surface_array_names = in_array_slots.keys().cloned().collect::<HashSet<_>>();
    proc_io_surface_array_names.extend(proc_io_output_array_names.iter().cloned());
    let mut proc_io_surface_names = ins_names.union(&out_names).cloned().collect::<HashSet<_>>();
    proc_io_surface_names.extend(proc_io_surface_array_names.iter().cloned());
    let mut seen_event_names = HashSet::<String>::new();
    for event in &proc.events {
        if !seen_event_names.insert(event.name.clone()) {
            continue;
        }
        if ins_names.contains(&event.name)
            || out_names.contains(&event.name)
            || param_names.contains(&event.name)
            || is_proc_output_alias_name(&event.name, proc.outs_timing, outs.len())
        {
            push_semantic(
                DiagCtx::new(event.loc),
                errors,
                format!(
                    "processor '{}.{}' event name conflicts with an existing callable/endpoint name",
                    proc.name, event.name
                ),
            );
        }
    }
    let typed_events = coerce_typed_events(
        &proc.events,
        true,
        &format!("processor '{}'", proc.name),
        proc_options,
        errors,
    );
    let mut reserved = HashSet::<String>::new();
    reserved.extend(param_names.iter().cloned());
    reserved.extend(ins_names.iter().cloned());
    reserved.extend(out_names.iter().cloned());

    let mut state_type_hints = HashMap::<String, PrimitiveType>::new();
    let mut declared_symbols = DeclaredSymbolMap::new();
    set_declared_symbol_types(
        &mut state_type_hints,
        &mut declared_symbols,
        &ins_names,
        &in_types,
        DeclaredScalarSymbolKind::Input,
    );
    set_declared_symbol_types(
        &mut state_type_hints,
        &mut declared_symbols,
        &out_names,
        &out_types,
        DeclaredScalarSymbolKind::Output,
    );
    let param_slot_types = param_specs
        .iter()
        .flat_map(|spec| spec.slots.iter())
        .map(|slot| (slot.name.clone(), slot.ty))
        .collect::<HashMap<_, _>>();
    set_declared_symbol_types(
        &mut state_type_hints,
        &mut declared_symbols,
        &typed_param_names,
        &param_slot_types,
        DeclaredScalarSymbolKind::Param,
    );
    insert_declared_array_bases(
        &mut state_type_hints,
        &mut declared_symbols,
        &proc_input_arrays,
    );
    insert_declared_array_bases(
        &mut state_type_hints,
        &mut declared_symbols,
        &proc_output_arrays,
    );
    insert_declared_array_bases(
        &mut state_type_hints,
        &mut declared_symbols,
        &proc_param_arrays,
    );
    for (fn_name, ret_ty) in fn_return_types {
        if let ReturnType::Scalar(scalar_ty) = ret_ty {
            insert_declared_symbol(
                &mut state_type_hints,
                &mut declared_symbols,
                fn_name.clone(),
                DeclaredSymbolInfo::FunctionReturn { ty: *scalar_ty },
            );
        }
    }

    let proc_ns = namespace_of_symbol(&proc.name);
    let proc_locals = HashSet::<String>::new();
    let proc_label = format!("processor '{}'", proc.name);
    let init_default_ty =
        resolve_init_default_ty(proc.init.default_ty.as_ref(), &proc_label, errors);

    let fn_signatures = fn_signatures_full.clone();

    // Unified init scope: use analyze_init_stmt
    let proc_resolution = Some(ProcResolutionCtx {
        owner_proc_name: &proc.name,
        reserved: &reserved,
        current_ns: &proc_ns,
        proc_symbols,
        struct_symbols: &struct_symbols,
        frontend_struct_defs: struct_defs,
        ctor_symbols,
        in_init_scope: true,
    });
    debug_assert!(
        proc_resolution.is_some(),
        "proc init analysis requires ProcResolutionCtx"
    );
    let init_ctx = InitAnalysisCtx {
        context_label: &proc_label,
        common: ScopeAnalysisCtx {
            policy: ScopePolicy::Init,
            input_names: &ins_names,
            output_names: &out_names,
            output_array_names: &proc_output_array_names,
            io_surface_names: &proc_io_surface_names,
            io_surface_array_names: &proc_io_surface_array_names,
            dynamic_param_array_names: &owner_dynamic_param_array_names,
            param_names: &typed_param_names,
            struct_defs: &typed_struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types,
            options: proc_options,
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
            port_index_kins: None,
            proc_event_names: &proc_event_names,
        },
        init_default_ty,
        proc_resolution,
        top_level_proc_symbols: None,
    };
    let mut init_local_array_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_array_aliases, &proc_param_arrays, false);
    seed_top_level_array_aliases(&mut init_local_array_aliases, const_arrays, false);
    let mut init_st = InitAnalysisState {
        known_scalars: HashSet::new(),
        local_aliases: HashMap::new(),
        local_array_aliases: init_local_array_aliases,
        declared_symbols,
        state_scalars: state_type_hints.clone(),
        state_arrays: HashMap::new(),
        state_array_struct_roots: HashMap::new(),
        struct_instances: HashMap::new(),
        state_array_specs: HashMap::new(),
        struct_instance_type_args: HashMap::new(),
        nested_procs: HashMap::new(),
        nested_proc_arrays: HashMap::new(),
        state_tuples: HashMap::new(),
    };
    // Seed known_scalars with reserved names so they're visible for decl-order checks
    init_st.known_scalars.extend(reserved.iter().cloned());
    analyze_owner_init_stmts(&proc.init, &init_ctx, &proc_locals, &mut init_st, errors);
    let mut state = convert_init_state_to_proc_fields(&init_st);

    // Non-init scopes: unified runtime analysis via register_scope_state + runtime stmt analysis.
    let mut proc_state_scalars = init_st.state_scalars;
    let mut proc_declared_symbols = init_st.declared_symbols;
    let mut proc_state_arrays = init_st.state_arrays;
    let proc_state_array_struct_roots = init_st.state_array_struct_roots;
    let proc_struct_instances = init_st.struct_instances;
    let init_st_type_args = init_st.struct_instance_type_args;
    let mut proc_state_tuples = init_st.state_tuples;
    let mut proc_struct_instances_typed = proc_struct_instances.clone();

    rewrite_owner_struct_array_inline_fields(
        ExecutableOwnerBodies {
            init: &mut proc.init,
            block_pre: &mut proc.block_pre,
            sample: &mut proc.sample,
            block_post: &mut proc.block_post,
            events: &mut proc.events,
        },
        &proc_state_array_struct_roots,
        &typed_struct_defs,
        errors,
    );
    for def in &mut proc.local_defs {
        rewrite_struct_array_inline_field_stmts(
            &mut def.body,
            &proc_state_array_struct_roots,
            &typed_struct_defs,
            errors,
        );
    }

    // Add buffer prefix entries so has_declared_buffer_symbol / validate_buffer_param_call_arg work
    for buffer in &buffer_specs {
        let channels = match buffer.channels {
            TypedBufferChannels::Mono => BufferChannelInfo::Mono,
            TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(ch),
            TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
        };
        insert_declared_symbol(
            &mut proc_state_scalars,
            &mut proc_declared_symbols,
            buffer.name.clone(),
            DeclaredSymbolInfo::Buffer {
                elem_ty: buffer.elem_ty,
                channels,
                array_len: buffer.array_len,
                is_array: buffer.is_array,
            },
        );
    }

    // Add flat struct field names to proc_state_scalars so block/sample validation resolves them
    for (inst_name, struct_type_name) in &proc_struct_instances {
        let type_args = init_st_type_args
            .get(inst_name)
            .cloned()
            .unwrap_or_default();
        let resolved = if type_args.is_empty() {
            struct_defs.get(struct_type_name).cloned()
        } else {
            struct_defs
                .get(struct_type_name)
                .and_then(|tmpl| specialize_generic_struct_template(tmpl, &type_args, errors))
        };
        if let Some(resolved_def) = resolved {
            if !type_args.is_empty() {
                let resolved_struct_name = resolved_def.name.clone();
                proc_struct_instances_typed.insert(inst_name.clone(), resolved_struct_name.clone());
                if !typed_struct_defs.contains_key(&resolved_struct_name) {
                    typed_struct_defs.insert(
                        resolved_struct_name.clone(),
                        coerce_struct_fields(
                            &resolved_struct_name,
                            &resolved_def.type_params,
                            &resolved_def.fields,
                            &typed_struct_defs,
                            proc_options,
                            errors,
                        ),
                    );
                }
            }
            for field in &resolved_def.fields {
                let flat = format!("{inst_name}.{}", field.name);
                match &field.ty {
                    FieldType::Scalar(prim) => {
                        proc_state_scalars.entry(flat).or_insert(*prim);
                    }
                    FieldType::Array(spec) => {
                        if let ArrayElemType::Primitive(elem_ty) = spec.elem {
                            insert_declared_symbol(
                                &mut proc_state_scalars,
                                &mut proc_declared_symbols,
                                flat.clone(),
                                DeclaredSymbolInfo::DataArray { elem_ty },
                            );
                        }
                        if let Some(size_val) = eval_data_size_expr(
                            &spec.size,
                            proc_options,
                            &format!("struct field '{flat}' array size"),
                            errors,
                        ) {
                            proc_state_arrays.entry(flat).or_insert(size_val);
                        }
                    }
                    FieldType::Tuple(elem_tys) => {
                        for (idx, prim) in elem_tys.iter().enumerate() {
                            let elem_flat = format!("{flat}.__{idx}");
                            proc_state_scalars.entry(elem_flat).or_insert(*prim);
                        }
                        proc_state_tuples.insert(flat, elem_tys.clone());
                    }
                    FieldType::Generic(_) => {}
                }
            }
        }
    }

    // Add input/output/param array port sizes so indexed access is recognized
    for (array_name, slots) in &in_array_slots {
        proc_state_arrays
            .entry(array_name.clone())
            .or_insert(slots.len());
    }
    for (array_name, slots) in &field_array_slots {
        proc_state_arrays
            .entry(array_name.clone())
            .or_insert(slots.len());
    }

    // Build enriched fn_signatures with nested proc instance stubs
    let mut proc_fn_signatures = fn_signatures;
    proc_fn_signatures
        .entry(PROC_INDEX_CALL_SENTINEL.to_owned())
        .or_insert_with(|| internal_proc_index_call_signature(false));
    proc_fn_signatures
        .entry(format!(
            "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
        ))
        .or_insert_with(|| internal_proc_index_call_signature(true));
    for (instance_name, nested) in &state.nested_procs {
        if let Some(target_proc) = proc_defs_by_name.get(&nested.proc_name) {
            let target_proc_options = proc_options_for_shape(target_proc, options);
            let target_io = infer_numbered_io_from_sample(&target_proc.sample);
            let target_ins =
                normalize_numbered_port_decls(&target_proc.ins, "in", target_io.max_in);
            let (nested_param_specs, _) = expand_proc_param_specs(
                &target_proc.name,
                &target_proc.params,
                target_proc_options,
                errors,
            );
            let mut params: Vec<String> = target_ins.iter().map(|p| p.name.clone()).collect();
            let mut defaults: Vec<Option<Expr>> =
                target_ins.iter().map(|p| p.default.clone()).collect();
            for spec in &nested_param_specs {
                params.push(spec.name.clone());
                defaults.push(Some(Expr::number(0.0)));
            }
            proc_fn_signatures.insert(
                instance_name.clone(),
                FnSignature {
                    params,
                    defaults,
                    param_types: Vec::new(),
                    type_params: Vec::new(),
                    readonly_array_params: HashSet::new(),
                },
            );

            let primary_ty = infer_primary_output_type_from_processor(target_proc);
            insert_declared_symbol(
                &mut proc_state_scalars,
                &mut proc_declared_symbols,
                instance_name.clone(),
                DeclaredSymbolInfo::FunctionReturn { ty: primary_ty },
            );

            let inferred_io = infer_numbered_io_from_sample(&target_proc.sample);
            let inferred_names = infer_numbered_names_from_proc(target_proc);
            let inferred_out_max = match target_proc.outs_timing {
                OutputTiming::Sample => inferred_io.max_out,
                OutputTiming::Block => inferred_names.max_kout,
            };
            let nested_out_ports = normalize_numbered_port_decls(
                &target_proc.outs,
                proc_output_numbered_prefix(target_proc),
                inferred_out_max,
            );
            let (_, nested_out_types, _, nested_out_arrays) = expand_proc_port_specs(
                &target_proc.name,
                &nested_out_ports,
                "output",
                target_proc_options,
                errors,
            );
            for port in &nested_out_ports {
                let flat_base = format!("{instance_name}.{}", port.name);
                if let Some(slots) = nested_out_arrays.get(&port.name) {
                    if let Some(elem_ty) = nested_out_types
                        .get(slots.first().map(|s| s.as_str()).unwrap_or_default())
                        .copied()
                    {
                        insert_declared_symbol(
                            &mut proc_state_scalars,
                            &mut proc_declared_symbols,
                            flat_base.clone(),
                            DeclaredSymbolInfo::DataArray { elem_ty },
                        );
                    }
                    proc_state_arrays.entry(flat_base).or_insert(slots.len());
                } else if let Some(ty) = nested_out_types.get(&port.name).copied() {
                    proc_state_scalars.entry(flat_base).or_insert(ty);
                }
            }

            for spec in nested_param_specs {
                let flat_base = format!("{instance_name}.{}", spec.name);
                if spec.slots.len() <= 1 {
                    if let Some(slot) = spec.slots.first() {
                        proc_state_scalars.entry(flat_base).or_insert(slot.ty);
                    }
                } else {
                    if let Some(first_slot) = spec.slots.first() {
                        insert_declared_symbol(
                            &mut proc_state_scalars,
                            &mut proc_declared_symbols,
                            flat_base.clone(),
                            DeclaredSymbolInfo::DataArray {
                                elem_ty: first_slot.ty,
                            },
                        );
                    }
                    proc_state_arrays
                        .entry(flat_base)
                        .or_insert(spec.slots.len());
                }
            }
        }
    }

    // Snapshot init-scope scalar keys to detect new additions later
    let init_scalar_keys: HashSet<String> = proc_state_scalars.keys().cloned().collect();
    let init_writable_roots = collect_runtime_state_roots(&proc_state_scalars, &proc_state_arrays);

    // Compute port index info for proc scopes (procs always have explicit port blocks)
    let port_index_ins = uniform_port_index_info_from_names(true, &ins, &in_types);
    let port_index_outs = uniform_port_index_info_from_names(true, &outs, &out_types);
    let block_port_index_outs = if proc.outs_timing == OutputTiming::Block {
        port_index_outs
    } else {
        None
    };
    let sample_port_index_outs = if proc.outs_timing == OutputTiming::Sample {
        port_index_outs
    } else {
        None
    };
    let mut dynamic_param_array_names = owner_dynamic_param_array_names.clone();
    for (name, nested) in &state.nested_procs {
        if child_proc_surfaces
            .get(&nested.proc_name)
            .is_some_and(|surface| surface.param_arrays.contains("params"))
        {
            dynamic_param_array_names.insert(format!("{name}.params"));
        }
    }
    for (name, nested) in &state.nested_proc_arrays {
        if child_proc_surfaces
            .get(&nested.proc_name)
            .is_some_and(|surface| surface.param_arrays.contains("params"))
        {
            dynamic_param_array_names.insert(format!("{name}.params"));
        }
    }
    validate_proc_local_def_surfaces(
        proc,
        &proc_io_surface_names,
        &proc_io_surface_array_names,
        &dynamic_param_array_names,
        errors,
    );

    let analysis_plan_seeds = build_proc_owner_analysis_plan_seeds(
        &reserved,
        &out_names,
        &proc_output_arrays,
        proc.outs_timing,
        &proc_struct_instances_typed,
        &state.nested_procs,
        &proc_state_arrays,
        const_arrays,
    );
    let empty_output_names = HashSet::<String>::new();
    {
        let mut runtime_state = ExecutableOwnerRuntimeState {
            state_scalars: &mut proc_state_scalars,
            declared_symbols: &proc_declared_symbols,
            state_arrays: &proc_state_arrays,
            state_array_struct_roots: &proc_state_array_struct_roots,
            nested_proc_instances: &state.nested_procs,
            proc_array_roots: &state.nested_proc_arrays,
            struct_instances: &proc_struct_instances_typed,
            state_tuples: &proc_state_tuples,
        };
        analyze_owner_runtime_scopes(
            &mut runtime_state,
            analysis_plan_seeds.runtime_scope_plans(
                RuntimeScopeBodies {
                    block_pre: &proc.block_pre,
                    sample: &proc.sample,
                    block_post: &proc.block_post,
                },
                RuntimeScopePlanInputs {
                    sample_input_names: &ins_names,
                    block_output_names: if proc.outs_timing == OutputTiming::Block {
                        &out_names
                    } else {
                        &empty_output_names
                    },
                    sample_output_names: if proc.outs_timing == OutputTiming::Sample {
                        &out_names
                    } else {
                        &empty_output_names
                    },
                    output_array_names: &proc_output_array_names,
                    io_surface_names: &proc_io_surface_names,
                    io_surface_array_names: &proc_io_surface_array_names,
                    dynamic_param_array_names: &dynamic_param_array_names,
                    param_names: &typed_param_names,
                    struct_defs: &typed_struct_defs,
                    fn_signatures: &proc_fn_signatures,
                    fn_return_types,
                    options: proc_options,
                    port_index_ins,
                    block_port_index_outs,
                    sample_port_index_outs,
                    port_index_params,
                    port_index_kins: None,
                    registration_input_names: &ins_names,
                    registration_output_names: &out_names,
                    registration_param_names: &typed_param_names,
                    proc_event_names: &proc_event_names,
                },
            ),
            errors,
        );

        analyze_owner_events(
            &runtime_state,
            analysis_plan_seeds.event_plan(
                runtime_state.state_scalars,
                EventPlanInputs {
                    typed_events: &typed_events,
                    init_writable_roots: &init_writable_roots,
                    input_names: &ins_names,
                    output_names: &out_names,
                    output_array_names: &proc_output_array_names,
                    io_surface_names: &proc_io_surface_names,
                    io_surface_array_names: &proc_io_surface_array_names,
                    dynamic_param_array_names: &dynamic_param_array_names,
                    param_names: &typed_param_names,
                    validation_input_names: &ins_names,
                    validation_output_names: &out_names,
                    struct_defs: &typed_struct_defs,
                    fn_signatures: &proc_fn_signatures,
                    fn_return_types,
                    options: proc_options,
                    port_index_ins,
                    port_index_outs,
                    port_index_params,
                    port_index_kins: None,
                    proc_event_names: &proc_event_names,
                },
            ),
            errors,
        );
    }

    // Merge new scalars from block/sample into state
    for (name, ty) in &proc_state_scalars {
        if !init_scalar_keys.contains(name) && !state.scalars.contains_key(name) {
            state.scalars.insert(name.clone(), *ty);
        }
    }

    let mut nested_proc_array_slots = HashMap::<String, Vec<String>>::new();
    let mut nested_proc_array_active_fields = HashMap::<String, String>::new();
    let mut nested_proc_array_names = state.nested_proc_arrays.keys().cloned().collect::<Vec<_>>();
    nested_proc_array_names.sort();
    for array_name in nested_proc_array_names {
        let Some(array_state) = state.nested_proc_arrays.get(&array_name).cloned() else {
            continue;
        };
        let size_context = format!(
            "processor '{}.{}' processor-array size",
            proc.name, array_name
        );
        let Some(len) =
            eval_data_size_expr(&array_state.size_expr, proc_options, &size_context, errors)
        else {
            continue;
        };
        let mut slots = Vec::<String>::with_capacity(len);
        for idx in 0..len {
            let slot = format!("{array_name}[{idx}]");
            if reserved.contains(&slot) {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "processor '{}' processor-array slot '{}' conflicts with reserved symbol",
                        proc.name, slot
                    ),
                );
                continue;
            }
            if let Some(existing) = state.nested_procs.get(&slot) {
                if existing.proc_name != array_state.proc_name {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "processor '{}' processor-array slot '{}' has conflicting processor types '{}' and '{}'",
                            proc.name, slot, existing.proc_name, array_state.proc_name
                        ),
                    );
                }
            } else if state.scalars.contains_key(&slot)
                || state.data.contains_key(&slot)
                || state.struct_instances.contains_key(&slot)
            {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "processor '{}' processor-array slot '{}' conflicts with existing state symbol",
                        proc.name, slot
                    ),
                );
                continue;
            } else {
                state.nested_procs.insert(
                    slot.clone(),
                    ProcNestedState {
                        proc_name: array_state.proc_name.clone(),
                    },
                );
            }
            slots.push(slot);
        }
        nested_proc_array_active_fields.insert(
            array_name.clone(),
            runtime_proc_array_active_symbol(&array_name),
        );
        nested_proc_array_slots.insert(array_name, slots);
    }
    let mut owner_param_array_names = proc_param_arrays.keys().cloned().collect::<HashSet<_>>();
    if field_array_slots.contains_key("params") {
        owner_param_array_names.insert("params".to_owned());
    }
    let input_array_names = in_array_slots.keys().cloned().collect::<HashSet<_>>();
    let mut output_array_names = proc_output_arrays.keys().cloned().collect::<HashSet<_>>();
    for synthetic in ["outs", "kouts"] {
        if field_array_slots.contains_key(synthetic) {
            output_array_names.insert(synthetic.to_owned());
        }
    }
    validate_proc_param_binds(
        proc,
        &param_specs,
        fn_defs_full,
        &owner_param_array_names,
        &ins_names,
        &input_array_names,
        &out_names,
        &output_array_names,
        &state,
        &nested_proc_array_slots,
        &child_proc_surfaces,
        errors,
    );

    let mut state_scalar_names = state.scalars.keys().cloned().collect::<Vec<_>>();
    state_scalar_names.sort();
    let mut state_data_names = state.data.keys().cloned().collect::<Vec<_>>();
    state_data_names.sort();
    let mut struct_instance_names = state.struct_instances.keys().cloned().collect::<Vec<_>>();
    struct_instance_names.sort();

    let mut fields = Vec::<StructField>::new();
    for spec in &param_specs {
        for slot in &spec.slots {
            fields.push(StructField {
                loc: Default::default(),
                name: slot.name.clone(),
                ty: FieldType::Scalar(slot.ty),
                ty_loc: Default::default(),
                default: slot.default.clone(),
            });
        }
    }
    for out_name in &outs {
        let out_ty = *out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
        fields.push(StructField {
            loc: Default::default(),
            name: out_name.clone(),
            ty: FieldType::Scalar(out_ty),
            ty_loc: Default::default(),
            default: None,
        });
    }
    if sample_oversample_factor > 1 {
        let stage_count = proc_os_sinc_stage_count(sample_oversample_factor);
        for in_name in &ins {
            let in_ty = *in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
            if matches!(in_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                for stage in 0..stage_count {
                    for tap in ["a0", "a1", "a2", "a3", "b0", "b1", "b2", "b3"] {
                        fields.push(StructField {
                            loc: Default::default(),
                            name: proc_os_up_stage_tap_field_name(in_name, stage, tap),
                            ty: FieldType::Scalar(in_ty),
                            ty_loc: Default::default(),
                            default: Some(Expr::number(0.0)),
                        });
                    }
                }
            }
        }
        for out_name in &outs {
            let out_ty = *out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
            if matches!(out_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                for stage in 0..stage_count {
                    for tap in ["a0", "a1", "a2", "a3", "b0", "b1", "b2", "b3"] {
                        fields.push(StructField {
                            loc: Default::default(),
                            name: proc_os_down_stage_tap_field_name(out_name, stage, tap),
                            ty: FieldType::Scalar(out_ty),
                            ty_loc: Default::default(),
                            default: Some(Expr::number(0.0)),
                        });
                    }
                }
            }
        }
    }
    for name in &state_scalar_names {
        if reserved.contains(name) {
            continue;
        }
        fields.push(StructField {
            loc: Default::default(),
            name: name.clone(),
            ty: FieldType::Scalar(*state.scalars.get(name).unwrap_or(&PrimitiveType::F32)),
            ty_loc: Default::default(),
            default: None,
        });
    }
    for name in &state_data_names {
        if reserved.contains(name) {
            continue;
        }
        if let Some(spec) = state.data.get(name) {
            let _ = eval_data_size_expr(
                &spec.size,
                proc_options,
                &format!("processor '{}.{}' array size", proc.name, name),
                errors,
            );
            fields.push(StructField {
                loc: Default::default(),
                name: name.clone(),
                ty: FieldType::Array(spec.clone()),
                ty_loc: Default::default(),
                default: None,
            });
        }
    }
    for instance in &struct_instance_names {
        if reserved.contains(instance) {
            continue;
        }
        let Some(state_struct) = state.struct_instances.get(instance) else {
            continue;
        };
        let Some(struct_def) =
            resolve_proc_state_struct_def(&proc.name, instance, state_struct, struct_defs, errors)
        else {
            continue;
        };
        fields.push(StructField {
            loc: Default::default(),
            name: instance.clone(),
            ty: FieldType::Generic(struct_def.name.clone()),
            ty_loc: Default::default(),
            default: None,
        });
        for field in &struct_def.fields {
            if let FieldType::Array(spec) = &field.ty {
                let _ = eval_data_size_expr(
                    &spec.size,
                    proc_options,
                    &format!(
                        "processor '{}.{}' struct field '{}' array size",
                        proc.name, instance, field.name
                    ),
                    errors,
                );
            }
        }
    }

    let field_names = fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<HashSet<_>>();
    let array_field_names = fields
        .iter()
        .filter_map(|f| match f.ty {
            FieldType::Array(_) => Some(f.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    ProcBaseShape {
        ins,
        outs,
        in_ports,
        param_specs,
        buffer_specs,
        in_types,
        out_types,
        in_array_slots,
        field_array_slots,
        nested_proc_array_slots,
        nested_proc_array_active_fields,
        state,
        fields,
        field_names,
        array_field_names,
    }
}

pub(super) fn resolve_proc_state_struct_def(
    proc_name: &str,
    instance: &str,
    state_struct: &ProcStructState,
    struct_defs: &HashMap<String, onda_frontend::StructDef>,
    errors: &mut Vec<Diagnostic>,
) -> Option<onda_frontend::StructDef> {
    let Some(struct_template) = struct_defs.get(&state_struct.struct_name) else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "processor '{}' state symbol '{}' references unknown struct '{}'",
                proc_name, instance, state_struct.struct_name
            ),
        );
        return None;
    };

    if state_struct.type_args.is_empty() {
        if !struct_template.type_params.is_empty() {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "processor '{}' state symbol '{}' uses generic struct '{}' without type arguments",
                    proc_name, instance, state_struct.struct_name
                ),
            );
            return None;
        }
        return Some(struct_template.clone());
    }

    let specialized =
        specialize_generic_struct_template(struct_template, &state_struct.type_args, errors)?;
    Some(specialized)
}

pub(super) fn build_proc_lowering_shape(
    proc_name: &str,
    base_shapes: &HashMap<String, ProcBaseShape>,
    cache: &mut HashMap<String, ProcLoweringShape>,
    visiting: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcLoweringShape> {
    if let Some(cached) = cache.get(proc_name) {
        return Some(cached.clone());
    }

    if let Some(idx) = visiting.iter().position(|n| n == proc_name) {
        let mut cycle = visiting[idx..].to_vec();
        cycle.push(proc_name.to_owned());
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "processor nested-state cycle detected: {}",
                cycle.join(" -> ")
            ),
        );
        return None;
    }

    let Some(base) = base_shapes.get(proc_name).cloned() else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("unknown processor '{proc_name}'"),
        );
        return None;
    };

    visiting.push(proc_name.to_owned());

    let mut fields = base.fields.clone();
    let mut field_names = base.field_names.clone();
    let mut array_field_names = base.array_field_names.clone();
    let mut field_array_slots = base.field_array_slots.clone();
    let mut nested_proc_array_slots = base.nested_proc_array_slots.clone();
    let mut nested_proc_array_active_fields = base.nested_proc_array_active_fields.clone();
    let mut nested_fields = HashMap::<String, HashSet<String>>::new();

    let mut nested_vars = base.state.nested_procs.keys().cloned().collect::<Vec<_>>();
    nested_vars.sort();

    for nested_var in nested_vars {
        let Some(nested_state) = base.state.nested_procs.get(&nested_var) else {
            continue;
        };

        let Some(callee_shape) = build_proc_lowering_shape(
            &nested_state.proc_name,
            base_shapes,
            cache,
            visiting,
            errors,
        ) else {
            continue;
        };

        nested_fields.insert(nested_var.clone(), callee_shape.field_names.clone());
        for (array_base, slots) in &callee_shape.field_array_slots {
            let prefixed_base = nested_field_name(&nested_var, array_base);
            let prefixed_slots = slots
                .iter()
                .map(|slot| nested_field_name(&nested_var, slot))
                .collect::<Vec<_>>();
            field_array_slots.insert(prefixed_base, prefixed_slots);
        }
        for (array_base, slots) in &callee_shape.nested_proc_array_slots {
            let prefixed_base = nested_field_name(&nested_var, array_base);
            let prefixed_slots = slots
                .iter()
                .map(|slot| nested_field_name(&nested_var, slot))
                .collect::<Vec<_>>();
            nested_proc_array_slots.insert(prefixed_base, prefixed_slots);
        }
        for (array_base, active_field) in &callee_shape.nested_proc_array_active_fields {
            nested_proc_array_active_fields.insert(
                nested_field_name(&nested_var, array_base),
                nested_field_name(&nested_var, active_field),
            );
        }

        let mut nested_callee_fields = callee_shape.fields.clone();
        nested_callee_fields.sort_by(|a, b| a.name.cmp(&b.name));
        for mut nested_field in nested_callee_fields {
            let flat_name = nested_field_name(&nested_var, &nested_field.name);
            if field_names.contains(&flat_name) {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "processor '{}' nested field '{}' conflicts with existing field '{}'",
                        proc_name, nested_field.name, flat_name
                    ),
                );
                continue;
            }
            nested_field.name = flat_name.clone();
            if matches!(nested_field.ty, FieldType::Array(_)) {
                array_field_names.insert(flat_name.clone());
            }
            field_names.insert(flat_name);
            fields.push(nested_field);
        }
    }

    let mut nested_proc_array_names = base
        .nested_proc_array_slots
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    nested_proc_array_names.sort();
    for array_name in nested_proc_array_names {
        let Some(array_state) = base.state.nested_proc_arrays.get(&array_name) else {
            continue;
        };
        let Some(array_slots) = base.nested_proc_array_slots.get(&array_name) else {
            continue;
        };
        let Some(callee_shape) =
            build_proc_lowering_shape(&array_state.proc_name, base_shapes, cache, visiting, errors)
        else {
            continue;
        };
        let mut scalar_fields = callee_shape
            .fields
            .iter()
            .filter_map(|field| {
                if matches!(field.ty, FieldType::Scalar(_)) {
                    Some(field.name.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        scalar_fields.sort();
        for field_name in scalar_fields {
            let field_array_base = format!("{array_name}.{field_name}");
            let field_slots = array_slots
                .iter()
                .map(|slot| nested_field_name(slot, &field_name))
                .collect::<Vec<_>>();
            field_array_slots
                .entry(field_array_base)
                .or_insert(field_slots);
        }
    }

    let _ = visiting.pop();

    let resolved = ProcLoweringShape {
        ins: base.ins,
        outs: base.outs,
        in_ports: base.in_ports,
        param_specs: base.param_specs,
        buffer_specs: base.buffer_specs,
        in_types: base.in_types,
        out_types: base.out_types,
        in_array_slots: base.in_array_slots,
        field_array_slots,
        nested_proc_array_slots,
        nested_proc_array_active_fields,
        state: base.state,
        fields,
        field_names,
        array_field_names,
        nested_fields,
    };
    cache.insert(proc_name.to_owned(), resolved.clone());
    Some(resolved)
}
