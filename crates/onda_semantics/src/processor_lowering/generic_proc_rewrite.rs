use super::*;
use onda_frontend::{DeclRange, Span};

pub(super) fn rewrite_and_materialize_generic_processors(
    program: &mut Program,
    errors: &mut Vec<Diagnostic>,
) {
    let initial_proc_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(p) => Some(p.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if initial_proc_defs.is_empty() {
        return;
    }

    let mut generic_proc_templates = HashMap::<String, ProcessorDef>::new();
    for p in &initial_proc_defs {
        if p.type_params.is_empty() {
            continue;
        }
        if generic_proc_templates.contains_key(&p.name) {
            push_semantic(
                DiagCtx::new(p.loc),
                errors,
                format!("duplicate generic processor '{}'", p.name),
            );
            continue;
        }
        let mut seen_tp = HashSet::<String>::new();
        for tp in &p.type_params {
            if !seen_tp.insert(tp.clone()) {
                push_semantic(
                    DiagCtx::new(p.loc),
                    errors,
                    format!(
                        "duplicate generic type parameter '{}' in processor '{}'",
                        tp, p.name
                    ),
                );
            }
        }
        generic_proc_templates.insert(p.name.clone(), p.clone());
    }

    let error_count_before_template_validation = errors.len();
    for template in generic_proc_templates.values() {
        validate_generic_proc_template_forwarded_type_args(template, errors);
    }
    if errors.len() > error_count_before_template_validation {
        return;
    }

    if generic_proc_templates.is_empty() {
        return;
    }

    let mut generated_specializations = HashMap::<String, ProcessorDef>::new();
    let top_level_seed = generic_inference_seed_for_top_level(&program.blocks);
    let empty_seed = GenericInferenceLocals::default();
    for block in &mut program.blocks {
        match block {
            Block::Struct(s) => {
                let struct_ns = namespace_of_symbol(&s.name);
                for field in &mut s.fields {
                    if let Some(default) = &mut field.default {
                        let mut locals = GenericInferenceLocals::default();
                        rewrite_generic_proc_ctor_expr(
                            default,
                            &generic_proc_templates,
                            &mut generated_specializations,
                            errors,
                            &mut locals,
                            &struct_ns,
                        );
                    }
                }
                for method in &mut s.methods {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut method.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &empty_seed,
                        &struct_ns,
                    );
                }
            }
            Block::Def(d) => {
                let def_ns = namespace_of_symbol(&d.name);
                rewrite_generic_proc_ctor_stmt_list(
                    &mut d.body,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &empty_seed,
                    &def_ns,
                );
            }
            Block::Proc(p) => {
                if !p.type_params.is_empty() {
                    continue;
                }
                let proc_ns = namespace_of_symbol(&p.name);
                let proc_seed = generic_inference_seed_for_processor(p);
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.init,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.block_pre,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.sample,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.block_post,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                for event in &mut p.events {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut event.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                }
                for task in &mut p.tasks {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut task.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                }
                for def in &mut p.local_defs {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut def.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                }
            }
            Block::Init(stmts) => {
                rewrite_generic_proc_ctor_stmt_list(
                    &mut stmts.body,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
            }
            Block::Block(exec) => {
                rewrite_generic_proc_ctor_stmt_list(
                    &mut exec.pre,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
                if let Some(sample) = &mut exec.sample {
                    rewrite_generic_proc_ctor_stmt_list(
                        sample,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
                rewrite_generic_proc_ctor_stmt_list(
                    &mut exec.post,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
            }
            Block::Sample(stmts) => {
                rewrite_generic_proc_ctor_stmt_list(
                    stmts,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
            }
            Block::Events(events) => {
                for event in events {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut event.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
            }
            _ => {}
        }
    }
    finalize_generated_generic_proc_specializations(
        &generic_proc_templates,
        &mut generated_specializations,
        errors,
    );

    program
        .blocks
        .retain(|b| !matches!(b, Block::Proc(p) if !p.type_params.is_empty()));
    let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
    generated.sort_by(|a, b| a.name.cmp(&b.name));
    for proc in generated {
        program.blocks.push(Block::Proc(proc));
    }
}

pub(crate) fn validate_generic_proc_template_forwarded_type_args(
    proc: &ProcessorDef,
    errors: &mut Vec<Diagnostic>,
) {
    let allowed = proc.type_params.iter().cloned().collect::<HashSet<_>>();

    for decl in &proc.ins {
        validate_optional_expr_type_args(
            decl.default.as_ref(),
            &allowed,
            proc,
            "input default",
            errors,
        );
        validate_range_type_args(decl.range.as_ref(), &allowed, proc, "input range", errors);
    }
    for decl in &proc.outs {
        validate_range_type_args(decl.range.as_ref(), &allowed, proc, "output range", errors);
    }
    for decl in &proc.params {
        validate_optional_expr_type_args(
            decl.default.as_ref(),
            &allowed,
            proc,
            "parameter default",
            errors,
        );
        validate_range_type_args(
            decl.range.as_ref(),
            &allowed,
            proc,
            "parameter range",
            errors,
        );
    }
    for decl in &proc.consts {
        validate_expr_type_args(&decl.expr, &allowed, proc, "const", errors);
    }
    if let Some(factor) = &proc.sample_oversample_factor {
        validate_expr_type_args(factor, &allowed, proc, "oversample factor", errors);
    }
    for stmt in &proc.init.body {
        validate_stmt_type_args(stmt, &allowed, proc, "init", errors);
    }
    for stmt in &proc.block_pre {
        validate_stmt_type_args(stmt, &allowed, proc, "block", errors);
    }
    for stmt in &proc.sample {
        validate_stmt_type_args(stmt, &allowed, proc, "sample", errors);
    }
    for stmt in &proc.block_post {
        validate_stmt_type_args(stmt, &allowed, proc, "block", errors);
    }
    for event in &proc.events {
        for param in &event.params {
            validate_optional_expr_type_args(
                param.default.as_ref(),
                &allowed,
                proc,
                &format!("event '{}' parameter default", event.name),
                errors,
            );
        }
        for stmt in &event.body {
            validate_stmt_type_args(
                stmt,
                &allowed,
                proc,
                &format!("event '{}'", event.name),
                errors,
            );
        }
    }
    for def in &proc.local_defs {
        let mut local_allowed = allowed.clone();
        local_allowed.extend(def.type_params.iter().cloned());
        for param in &def.params {
            validate_optional_expr_type_args(
                param.default.as_ref(),
                &local_allowed,
                proc,
                &format!("local def '{}' parameter default", def.name),
                errors,
            );
        }
        for stmt in &def.body {
            validate_stmt_type_args(
                stmt,
                &local_allowed,
                proc,
                &format!("local def '{}'", def.name),
                errors,
            );
        }
    }
}

fn validate_range_type_args(
    range: Option<&DeclRange>,
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(range) = range else {
        return;
    };
    validate_optional_expr_type_args(range.min.as_ref(), allowed, proc, context, errors);
    validate_expr_type_args(&range.max, allowed, proc, context, errors);
}

fn validate_optional_expr_type_args(
    expr: Option<&Expr>,
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(expr) = expr {
        validate_expr_type_args(expr, allowed, proc, context, errors);
    }
}

fn validate_stmt_type_args(
    stmt: &Stmt,
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            validate_expr_type_args(&decl.expr, allowed, proc, context, errors);
        }
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                validate_expr_type_args(index, allowed, proc, context, errors);
            }
            validate_expr_type_args(expr, allowed, proc, context, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            validate_expr_type_args(expr, allowed, proc, context, errors);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                validate_expr_type_args(value, allowed, proc, context, errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            validate_expr_type_args(cond, allowed, proc, context, errors);
            for stmt in then_branch {
                validate_stmt_type_args(stmt, allowed, proc, context, errors);
            }
            for stmt in else_branch {
                validate_stmt_type_args(stmt, allowed, proc, context, errors);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            validate_expr_type_args(start, allowed, proc, context, errors);
            validate_expr_type_args(end, allowed, proc, context, errors);
            validate_optional_expr_type_args(step.as_ref(), allowed, proc, context, errors);
            for stmt in body {
                validate_stmt_type_args(stmt, allowed, proc, context, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            validate_expr_type_args(cond, allowed, proc, context, errors);
            for stmt in body {
                validate_stmt_type_args(stmt, allowed, proc, context, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn validate_expr_type_args(
    expr: &Expr,
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            validate_call_type_args(
                type_args,
                allowed,
                proc,
                context,
                name,
                expr.loc().into(),
                errors,
            );
            for arg in args {
                validate_expr_type_args(&arg.expr, allowed, proc, context, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            validate_array_elem_type_args(
                &spec.elem,
                allowed,
                proc,
                context,
                expr.loc().into(),
                errors,
            );
            validate_expr_type_args(&spec.size, allowed, proc, context, errors);
            if let Some(values) = init {
                for value in values {
                    validate_expr_type_args(value, allowed, proc, context, errors);
                }
            }
        }
        Expr::Index { index, .. } => {
            validate_expr_type_args(index, allowed, proc, context, errors);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end] {
                validate_optional_expr_type_args(
                    coordinate.as_deref(),
                    allowed,
                    proc,
                    context,
                    errors,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_expr_type_args(lhs, allowed, proc, context, errors);
            validate_expr_type_args(rhs, allowed, proc, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_expr_type_args(arg, allowed, proc, context, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_expr_type_args(expr, allowed, proc, context, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                validate_expr_type_args(value, allowed, proc, context, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn validate_array_elem_type_args(
    elem: &ArrayElemType,
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    let ArrayElemType::Struct(name) = elem else {
        return;
    };
    let Some((_, suffix)) = name.split_once('<') else {
        return;
    };
    let Some(args_raw) = suffix.strip_suffix('>') else {
        return;
    };
    for raw in args_raw.split(',') {
        validate_raw_type_arg(raw.trim(), allowed, proc, context, name, span, errors);
    }
}

fn validate_call_type_args(
    type_args: &[CallTypeArg],
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    callee: &str,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    for type_arg in type_args {
        match type_arg {
            CallTypeArg::Primitive(PrimitiveType::Bool) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{}' {context}: 'bool' is not allowed as a generic type argument in call to '{}'; only numeric types (f32, f64, i32, i64) are supported",
                        proc.name, callee
                    ),
                    span,
                ));
            }
            CallTypeArg::Primitive(_) => {}
            CallTypeArg::Generic(name) => {
                validate_raw_type_arg(name, allowed, proc, context, callee, span, errors);
            }
        }
    }
}

fn validate_raw_type_arg(
    name: &str,
    allowed: &HashSet<String>,
    proc: &ProcessorDef,
    context: &str,
    callee: &str,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    if allowed.contains(name) {
        return;
    }
    if let Some(primitive) = PrimitiveType::from_name(name) {
        if primitive.is_numeric() {
            return;
        }
        errors.push(Diagnostic::semantic_span(
            format!(
                "processor '{}' {context}: 'bool' is not allowed as a generic type argument in call to '{}'; only numeric types (f32, f64, i32, i64) are supported",
                proc.name, callee
            ),
            span,
        ));
        return;
    }
    errors.push(Diagnostic::semantic_span(
        format!(
            "processor '{}' {context}: unknown generic type argument '{}' in call to '{}'; not declared in current generic owner",
            proc.name, name, callee
        ),
        span,
    ));
}
