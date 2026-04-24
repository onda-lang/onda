use super::*;
use crate::proc_call_support::rewrite_proc_alias_call_sites_in_expr;

fn is_proc_operator_helper_name(name: &str) -> bool {
    name.ends_with(PROC_STEP_FN_SUFFIX)
        || name.contains(PROC_CALL_OUT_FN_PREFIX)
        || (name.contains(".__proc_nested_")
            && (name.ends_with("_step") || name.contains("_call_out")))
}

fn collect_proc_operator_helper_diags_from_expr(expr: &Expr, out: &mut Vec<DiagCtx>) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_proc_operator_helper_diags_from_expr(value, out);
            }
        }
        Expr::Index { index, .. } => collect_proc_operator_helper_diags_from_expr(index, out),
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_proc_operator_helper_diags_from_expr(start, out);
            }
            if let Some(end) = end {
                collect_proc_operator_helper_diags_from_expr(end, out);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_proc_operator_helper_diags_from_expr(&spec.size, out);
            if let Some(values) = init {
                for value in values {
                    collect_proc_operator_helper_diags_from_expr(value, out);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_proc_operator_helper_diags_from_expr(lhs, out);
            collect_proc_operator_helper_diags_from_expr(rhs, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_proc_operator_helper_diags_from_expr(arg, out);
            }
        }
        Expr::UserCall {
            name, args, loc, ..
        } => {
            if is_proc_operator_helper_name(name) {
                out.push(DiagCtx::new(*loc));
            }
            for arg in args {
                collect_proc_operator_helper_diags_from_expr(&arg.expr, out);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_proc_operator_helper_diags_from_expr(expr, out);
        }
    }
}

fn collect_proc_operator_helper_diags_from_stmt(stmt: &Stmt, out: &mut Vec<DiagCtx>) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_proc_operator_helper_diags_from_expr(expr, out);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_proc_operator_helper_diags_from_expr(cond, out);
            for stmt in then_branch {
                collect_proc_operator_helper_diags_from_stmt(stmt, out);
            }
            for stmt in else_branch {
                collect_proc_operator_helper_diags_from_stmt(stmt, out);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_proc_operator_helper_diags_from_expr(start, out);
            collect_proc_operator_helper_diags_from_expr(end, out);
            if let Some(step) = step {
                collect_proc_operator_helper_diags_from_expr(step, out);
            }
            for stmt in body {
                collect_proc_operator_helper_diags_from_stmt(stmt, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_proc_operator_helper_diags_from_expr(cond, out);
            for stmt in body {
                collect_proc_operator_helper_diags_from_stmt(stmt, out);
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
    out: &mut Vec<DiagCtx>,
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
    collect_proc_operator_helper_diags_from_expr(&rewritten, out);
}

fn collect_non_sample_proc_operator_diags_from_expr_stmt(
    expr: &Expr,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    out: &mut Vec<DiagCtx>,
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
    collect_proc_operator_helper_diags_from_stmt(&rewritten_stmt, out);
}

fn collect_non_sample_proc_operator_diags_from_target(
    target: &AssignTarget,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    out: &mut Vec<DiagCtx>,
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
        AssignTarget::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_non_sample_proc_operator_diags_from_expr(
                    start,
                    owner_proc,
                    nested_instances,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    out,
                );
            }
            if let Some(end) = end {
                collect_non_sample_proc_operator_diags_from_expr(
                    end,
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
    out: &mut Vec<DiagCtx>,
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
    errors: &mut Vec<Diagnostic>,
) {
    let mut aliases = HashMap::<String, ProcArrayAliasInfo>::new();
    let mut diags = Vec::<DiagCtx>::new();
    collect_non_sample_proc_operator_diags_from_stmts(
        stmts,
        owner_proc,
        nested_instances,
        proc_array_slots,
        proc_api,
        &mut aliases,
        &mut diags,
    );
    for diag in diags {
        push_semantic(
            diag,
            errors,
            format!(
                "proc operator '()' is only allowed in sample; found non-sample use in {context}"
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
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                seed_called_proc_local_defs_from_expr(start, def_names, pending, seen_pending);
            }
            if let Some(end) = end {
                seed_called_proc_local_defs_from_expr(end, def_names, pending, seen_pending);
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
        AssignTarget::Slice { start, end, .. } => {
            if let Some(start) = start {
                seed_called_proc_local_defs_from_expr(start, def_names, pending, seen_pending);
            }
            if let Some(end) = end {
                seed_called_proc_local_defs_from_expr(end, def_names, pending, seen_pending);
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
        errors,
    );
    push_non_sample_proc_call_errors_in_proc_stmts(
        &proc.block_pre,
        &proc.name,
        nested_instances,
        proc_array_slots,
        proc_api,
        &format!("processor '{}'.block pre", proc.name),
        errors,
    );
    push_non_sample_proc_call_errors_in_proc_stmts(
        &proc.block_post,
        &proc.name,
        nested_instances,
        proc_array_slots,
        proc_api,
        &format!("processor '{}'.block post", proc.name),
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
    let mut defs_with_proc_calls = HashMap::<String, Vec<DiagCtx>>::new();
    for (hidden_name, def) in &local_defs {
        let mut aliases = HashMap::<String, ProcArrayAliasInfo>::new();
        let mut diags = Vec::<DiagCtx>::new();
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

    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();
    seed_called_proc_local_defs_from_stmts(
        &proc.init.body,
        &def_names,
        &mut pending,
        &mut seen_pending,
    );
    seed_called_proc_local_defs_from_stmts(
        &proc.block_pre,
        &def_names,
        &mut pending,
        &mut seen_pending,
    );
    seed_called_proc_local_defs_from_stmts(
        &proc.block_post,
        &def_names,
        &mut pending,
        &mut seen_pending,
    );
    for event in &proc.events {
        seed_called_proc_local_defs_from_stmts(
            &event.body,
            &def_names,
            &mut pending,
            &mut seen_pending,
        );
    }

    let mut non_sample_reachable_defs = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !non_sample_reachable_defs.insert(name.clone()) {
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

    for (hidden_name, diags) in defs_with_proc_calls {
        if !non_sample_reachable_defs.contains(&hidden_name) {
            continue;
        }
        let def_name = local_defs
            .get(&hidden_name)
            .map(|def| def.name.as_str())
            .unwrap_or(hidden_name.as_str());
        for diag in diags {
            push_semantic(
                diag,
                errors,
                format!(
                    "proc operator '()' is only allowed in sample; call in processor '{}'.def '{}' is not provably sample-only",
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
    let out_ports = normalize_numbered_port_decls(&proc.outs, "out", inferred_io.max_out);
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

fn is_proc_output_alias_name(name: &str, out_count: usize) -> bool {
    let Some(rest) = name.strip_prefix("out") else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let Ok(idx) = rest.parse::<usize>() else {
        return false;
    };
    idx >= 1 && idx <= out_count
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
    proc_defs_by_name: &HashMap<String, onda_frontend::ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
) -> ProcBaseShape {
    let struct_symbols = struct_defs.keys().cloned().collect::<HashSet<_>>();
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let ins_ports = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
    let out_ports = normalize_numbered_port_decls(&proc.outs, "out", inferred_io.max_out);
    let (ins, in_types, in_ports, mut in_array_slots) =
        expand_proc_port_specs(&proc.name, &ins_ports, "input", options, errors);
    let (outs, out_types, _out_ports, out_array_slots) =
        expand_proc_port_specs(&proc.name, &out_ports, "output", options, errors);
    let (param_specs, mut field_array_slots) =
        expand_proc_param_specs(&proc.name, &proc.params, options, errors);
    let buffer_specs = coerce_buffers(&proc.buffers, options, errors)
        .into_iter()
        .map(|b| ProcBufferSpec {
            name: b.name,
            elem_ty: b.elem_ty,
            channels: b.channels,
        })
        .collect::<Vec<_>>();
    for (name, slots) in out_array_slots {
        field_array_slots.insert(name, slots);
    }

    // Add synthetic "ins"/"outs"/"params" array-slot entries for uniform scalar ports
    // so that dynamic indexing (e.g. outs[i], ins[i], params[i]) can be rewritten to
    // helper function calls during proc lowering.
    if ins.len() > 1
        && !in_array_slots.contains_key("ins")
        && uniform_port_index_info_from_names(true, &ins, &in_types).is_some()
    {
        in_array_slots.insert("ins".to_owned(), ins.clone());
    }
    if outs.len() > 1
        && !field_array_slots.contains_key("outs")
        && uniform_port_index_info_from_names(true, &outs, &out_types).is_some()
    {
        field_array_slots.insert("outs".to_owned(), outs.clone());
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
    let mut seen_event_names = HashSet::<String>::new();
    for event in &proc.events {
        if !seen_event_names.insert(event.name.clone()) {
            continue;
        }
        if ins_names.contains(&event.name)
            || out_names.contains(&event.name)
            || param_names.contains(&event.name)
            || is_proc_output_alias_name(&event.name, outs.len())
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
        options,
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
            param_names: &typed_param_names,
            struct_defs: &typed_struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types,
            options,
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
        },
        init_default_ty,
        proc_resolution,
        top_level_proc_symbols: None,
    };
    let mut init_st = InitAnalysisState {
        known_scalars: HashSet::new(),
        local_aliases: HashMap::new(),
        local_array_aliases: HashMap::new(),
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
                            options,
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
                            options,
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
            let target_io = infer_numbered_io_from_sample(&target_proc.sample);
            let target_ins =
                normalize_numbered_port_decls(&target_proc.ins, "in", target_io.max_in);
            let params: Vec<String> = target_ins.iter().map(|p| p.name.clone()).collect();
            proc_fn_signatures.insert(
                instance_name.clone(),
                FnSignature {
                    params,
                    defaults: Vec::new(),
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
            let nested_out_ports =
                normalize_numbered_port_decls(&target_proc.outs, "out", inferred_io.max_out);
            let (_, nested_out_types, _, nested_out_arrays) = expand_proc_port_specs(
                &target_proc.name,
                &nested_out_ports,
                "output",
                options,
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

            let (nested_param_specs, _) =
                expand_proc_param_specs(&target_proc.name, &target_proc.params, options, errors);
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
    let port_index_params = uniform_port_index_info_from_types(
        true,
        param_specs.iter().map(|spec| spec.slots.len()).sum(),
        param_specs
            .iter()
            .flat_map(|spec| spec.slots.iter().map(|slot| slot.ty)),
    );

    let analysis_plan_seeds = build_proc_owner_analysis_plan_seeds(
        &reserved,
        &out_names,
        &proc_struct_instances_typed,
        &state.nested_procs,
        &proc_state_arrays,
    );
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
                    sample_output_names: &out_names,
                    param_names: &typed_param_names,
                    struct_defs: &typed_struct_defs,
                    fn_signatures: &proc_fn_signatures,
                    fn_return_types,
                    options,
                    port_index_ins,
                    port_index_outs,
                    port_index_params,
                    registration_input_names: &ins_names,
                    registration_output_names: &out_names,
                    registration_param_names: &typed_param_names,
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
                    param_names: &typed_param_names,
                    validation_input_names: &ins_names,
                    validation_output_names: &out_names,
                    struct_defs: &typed_struct_defs,
                    fn_signatures: &proc_fn_signatures,
                    fn_return_types,
                    options,
                    port_index_ins,
                    port_index_outs,
                    port_index_params,
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
        let Some(len) = eval_data_size_expr(&array_state.size_expr, options, &size_context, errors)
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
                options,
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
                    options,
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

    let Some(specialized) =
        specialize_generic_struct_template(struct_template, &state_struct.type_args, errors)
    else {
        return None;
    };
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
