use super::namespaces::{
    looks_like_namespace_ref, namespace_of_symbol, qualify_local_namespace_member_name,
    resolve_namespace_symbol_name, rewrite_named_type_ref_name,
};
use super::*;

fn rebase_expr_locs(expr: &mut Expr, loc: SourceLoc) {
    expr.set_loc(loc);
    match expr {
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                rebase_expr_locs(value, loc);
            }
        }
        Expr::Index { index, .. }
        | Expr::Cast { expr: index, .. }
        | Expr::UnaryNot { expr: index, .. }
        | Expr::UnaryBitNot { expr: index, .. } => {
            rebase_expr_locs(index, loc);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                rebase_expr_locs(start, loc);
            }
            if let Some(end) = end {
                rebase_expr_locs(end, loc);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rebase_expr_locs(&mut spec.size, loc);
            if let Some(init) = init {
                for value in init {
                    rebase_expr_locs(value, loc);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rebase_expr_locs(lhs, loc);
            rebase_expr_locs(rhs, loc);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rebase_expr_locs(arg, loc);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rebase_expr_locs(&mut arg.expr, loc);
            }
        }
        Expr::Tuple { values, .. } => {
            for value in values {
                rebase_expr_locs(value, loc);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn is_builtin_compile_time_const(name: &str) -> bool {
    matches!(
        name,
        "PI" | "pi"
            | "TWO_PI"
            | "TWOPI"
            | "two_pi"
            | "twopi"
            | "SAMPLE_RATE"
            | "SAMPLERATE"
            | "SR"
            | "sample_rate"
            | "samplerate"
            | "BLOCK_SIZE"
            | "BLOCKSIZE"
            | "BS"
            | "block_size"
            | "blocksize"
    )
}

fn resolve_namespace_symbol_name_at(
    name: &str,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<String, Vec<Diagnostic>> {
    let loc = loc.into();
    let span = loc.span();
    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated, span)
}

fn rewrite_named_type_ref_name_at(
    name: &mut String,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    rewrite_named_type_ref_name(name, current_ns, const_env, state, generated, Some(&loc))
}

pub(super) fn validate_compile_time_expr(
    expr: &Expr,
    known_consts: &HashMap<String, Expr>,
    context: &str,
) -> Result<(), Vec<Diagnostic>> {
    let expr_span = expr.loc().span();
    match expr {
        Expr::Number { .. } | Expr::Bool { .. } => Ok(()),
        Expr::Int { .. } => Ok(()),
        Expr::Var { name, .. } => {
            if known_consts.contains_key(name) || is_builtin_compile_time_const(name) {
                Ok(())
            } else {
                Err(vec![Diagnostic::semantic_span(
                    format!(
                        "{context}: expression references non-compile-time symbol '{}'",
                        name
                    ),
                    expr_span,
                )])
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_compile_time_expr(expr, known_consts, context)
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_compile_time_expr(lhs, known_consts, context)?;
            validate_compile_time_expr(rhs, known_consts, context)
        }
        Expr::Call { .. }
        | Expr::UserCall { .. }
        | Expr::Index { .. }
        | Expr::Slice { .. }
        | Expr::ArrayCtor { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Tuple { .. } => Err(vec![Diagnostic::semantic_span(
            format!("{context}: expression is not compile-time evaluable"),
            expr_span,
        )]),
    }
}

pub(super) fn finalize_const_decl_expr(
    decl: &mut ConstDecl,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<Expr, Vec<Diagnostic>> {
    if is_builtin_compile_time_const(&decl.name) {
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "constant name '{}' is reserved as a builtin compile-time constant",
                decl.name
            ),
            decl.loc.as_ref(),
        )]);
    }

    rewrite_expr(&mut decl.expr, current_ns, const_env, state, generated)?;
    validate_compile_time_expr(&decl.expr, const_env, &format!("const '{}'", decl.name))?;

    let expr = decl.expr.clone();
    Ok(if let Some(ty) = decl.ty {
        Expr::Cast {
            loc: expr.loc().into(),
            to: ty,
            expr: Box::new(expr),
        }
    } else {
        expr
    })
}

pub(super) fn substitute_expr_with_env(expr: &Expr, const_env: &HashMap<String, Expr>) -> Expr {
    match expr {
        Expr::Var { name, .. } => const_env.get(name).cloned().unwrap_or_else(|| expr.clone()),
        Expr::ArrayLiteral { loc, values } => Expr::ArrayLiteral {
            loc: loc.clone(),
            values: values
                .iter()
                .map(|v| substitute_expr_with_env(v, const_env))
                .collect(),
        },
        Expr::Index { loc, base, index } => Expr::Index {
            loc: loc.clone(),
            base: base.clone(),
            index: Box::new(substitute_expr_with_env(index, const_env)),
        },
        Expr::Slice {
            loc,
            base,
            start,
            end,
        } => Expr::Slice {
            loc: loc.clone(),
            base: base.clone(),
            start: start
                .as_ref()
                .map(|expr| Box::new(substitute_expr_with_env(expr, const_env))),
            end: end
                .as_ref()
                .map(|expr| Box::new(substitute_expr_with_env(expr, const_env))),
        },
        Expr::ArrayCtor { loc, spec, init } => Expr::ArrayCtor {
            loc: loc.clone(),
            spec: ArrayTypeSpec {
                elem: spec.elem.clone(),
                size: Box::new(substitute_expr_with_env(&spec.size, const_env)),
            },
            init: init.as_ref().map(|values| {
                values
                    .iter()
                    .map(|v| substitute_expr_with_env(v, const_env))
                    .collect()
            }),
        },
        Expr::Compare { loc, op, lhs, rhs } => Expr::Compare {
            loc: loc.clone(),
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Call { loc, func, args } => Expr::Call {
            loc: loc.clone(),
            func: *func,
            args: args
                .iter()
                .map(|a| substitute_expr_with_env(a, const_env))
                .collect(),
        },
        Expr::UserCall {
            loc,
            name,
            type_args,
            args,
        } => Expr::UserCall {
            loc: loc.clone(),
            name: name.clone(),
            type_args: type_args.clone(),
            args: args
                .iter()
                .map(|a| CallArg {
                    name: a.name.clone(),
                    expr: substitute_expr_with_env(&a.expr, const_env),
                })
                .collect(),
        },
        Expr::Cast { loc, to, expr } => Expr::Cast {
            loc: loc.clone(),
            to: *to,
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::UnaryNot { loc, expr } => Expr::UnaryNot {
            loc: loc.clone(),
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::UnaryBitNot { loc, expr } => Expr::UnaryBitNot {
            loc: loc.clone(),
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::Logical { loc, op, lhs, rhs } => Expr::Logical {
            loc: loc.clone(),
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Binary { loc, op, lhs, rhs } => Expr::Binary {
            loc: loc.clone(),
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Tuple { loc, values } => Expr::Tuple {
            loc: loc.clone(),
            values: values
                .iter()
                .map(|v| substitute_expr_with_env(v, const_env))
                .collect(),
        },
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => expr.clone(),
    }
}

pub(super) fn rewrite_blocks_namespace_refs(
    blocks: &mut Vec<Block>,
    _current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let mut scope_consts = const_env.clone();
    let mut local_const_names = HashSet::<String>::new();
    let mut rewritten = Vec::<Block>::with_capacity(blocks.len());
    for mut block in std::mem::take(blocks) {
        if let Block::Const(decl) = &mut block {
            if !local_const_names.insert(decl.name.clone()) {
                return Err(vec![Diagnostic::semantic_span(
                    format!("duplicate top-level constant '{}'", decl.name),
                    decl.loc.as_ref(),
                )]);
            }
            let value = finalize_const_decl_expr(decl, "", &scope_consts, state, generated)?;
            scope_consts.insert(decl.name.clone(), value);
            continue;
        }
        let current_ns = match block {
            Block::Struct(ref s) => namespace_of_symbol(&s.name),
            Block::Def(ref d) => namespace_of_symbol(&d.name),
            Block::Proc(ref p) => namespace_of_symbol(&p.name),
            _ => String::new(),
        };
        rewrite_block_namespace_refs(&mut block, &current_ns, &scope_consts, state, generated)?;
        rewritten.push(block);
    }
    *blocks = rewritten;
    Ok(())
}

fn try_eval_const_count_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int { value, .. } => Some(*value),
        Expr::Number { value, .. } => {
            let v = *value;
            if v == (v as i64 as f64) {
                Some(v as i64)
            } else {
                None
            }
        }
        Expr::Cast { expr, .. } => try_eval_const_count_i64(expr),
        Expr::Binary { op, lhs, rhs, .. } => {
            let l = try_eval_const_count_i64(lhs)?;
            let r = try_eval_const_count_i64(rhs)?;
            match op {
                BinaryOp::Add => Some(l + r),
                BinaryOp::Sub => Some(l - r),
                BinaryOp::Mul => Some(l * r),
                BinaryOp::Div if r != 0 => Some(l / r),
                BinaryOp::Mod if r != 0 => Some(l % r),
                _ => None,
            }
        }
        _ => None,
    }
}

fn try_eval_const_count_expr(expr: &Expr) -> Option<usize> {
    let v = try_eval_const_count_i64(expr)?;
    if v > 0 {
        Some(v as usize)
    } else {
        None
    }
}

fn expand_deferred_port_count(
    port_block: &mut PortBlock,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let Some(mut count_expr) = port_block.deferred_count.take() else {
        return Ok(());
    };
    rewrite_expr(&mut count_expr, current_ns, const_env, state, generated)?;
    let Some(count) = try_eval_const_count_expr(&count_expr) else {
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "{} count expression must evaluate to a positive integer constant",
                port_block.deferred_prefix
            ),
            count_expr.loc().span(),
        )]);
    };
    let prefix = &port_block.deferred_prefix;
    let default_ty = port_block.deferred_default_ty.take();
    if port_block.decls.is_empty() {
        for idx in 1..=count {
            port_block.decls.push(PortDecl {
                loc: port_block.loc.clone(),
                name: format!("{prefix}{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if port_block.decls.len() != count {
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "{} block count ({count}) does not match explicit declaration count ({})",
                prefix,
                port_block.decls.len()
            ),
            port_block.loc.as_ref(),
        )]);
    }
    Ok(())
}

fn expand_deferred_param_count(
    param_block: &mut ParamBlock,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let Some(mut count_expr) = param_block.deferred_count.take() else {
        return Ok(());
    };
    rewrite_expr(&mut count_expr, current_ns, const_env, state, generated)?;
    let Some(count) = try_eval_const_count_expr(&count_expr) else {
        return Err(vec![Diagnostic::semantic_span(
            "params count expression must evaluate to a positive integer constant",
            count_expr.loc().span(),
        )]);
    };
    let default_ty = param_block.deferred_default_ty.take();
    if param_block.decls.is_empty() {
        for idx in 1..=count {
            param_block.decls.push(ParamDecl {
                loc: param_block.loc.clone(),
                name: format!("param{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if param_block.decls.len() != count {
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "params block count ({count}) does not match explicit declaration count ({})",
                param_block.decls.len()
            ),
            param_block.loc.as_ref(),
        )]);
    }
    Ok(())
}

fn expand_deferred_proc_port_count(
    decls: &mut Vec<PortDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    loc: &Span,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let Some(mut count_expr) = deferred_count.take() else {
        return Ok(());
    };
    rewrite_expr(&mut count_expr, current_ns, const_env, state, generated)?;
    let Some(count) = try_eval_const_count_expr(&count_expr) else {
        return Err(vec![Diagnostic::semantic_span(
            format!("{prefix} count expression must evaluate to a positive integer constant"),
            count_expr.loc().span(),
        )]);
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(PortDecl {
                loc: loc.clone(),
                name: format!("{prefix}{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if decls.len() != count {
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "{prefix} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        )]);
    }
    Ok(())
}

fn expand_deferred_proc_param_count(
    decls: &mut Vec<ParamDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    loc: &Span,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let Some(mut count_expr) = deferred_count.take() else {
        return Ok(());
    };
    rewrite_expr(&mut count_expr, current_ns, const_env, state, generated)?;
    let Some(count) = try_eval_const_count_expr(&count_expr) else {
        return Err(vec![Diagnostic::semantic_span(
            "params count expression must evaluate to a positive integer constant",
            count_expr.loc().span(),
        )]);
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(ParamDecl {
                loc: loc.clone(),
                name: format!("param{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if decls.len() != count {
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "params block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        )]);
    }
    Ok(())
}

pub(super) fn rewrite_block_namespace_refs(
    block: &mut Block,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match block {
        Block::Ins(port_block) | Block::Outs(port_block) => {
            expand_deferred_port_count(port_block, current_ns, const_env, state, generated)?;
            rewrite_port_decls(port_block, current_ns, const_env, state, generated)?;
        }
        Block::Params(param_block) => {
            expand_deferred_param_count(param_block, current_ns, const_env, state, generated)?;
            rewrite_param_decls(param_block, current_ns, const_env, state, generated)?;
        }
        Block::Const(_) => {}
        Block::Buffers(decls) => {
            for decl in decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(
                        ty,
                        current_ns,
                        const_env,
                        state,
                        generated,
                        decl.ty_loc.as_ref().or(decl.loc.as_ref()),
                    )?;
                }
            }
        }
        Block::Assert(assert_decl) => {
            rewrite_expr(
                &mut assert_decl.expr,
                current_ns,
                const_env,
                state,
                generated,
            )?;
        }
        Block::Events(events) => {
            for event in events {
                rewrite_event_def(event, current_ns, const_env, state, generated)?;
            }
        }
        Block::Struct(s) => {
            for field in &mut s.fields {
                rewrite_field_type(
                    &mut field.ty,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    field.ty_loc.as_ref().or(field.loc.as_ref()),
                )?;
                if let Some(default) = &mut field.default {
                    rewrite_expr(default, current_ns, const_env, state, generated)?;
                }
            }
            for method in &mut s.methods {
                rewrite_function_def(method, current_ns, const_env, state, generated)?;
            }
        }
        Block::Def(d) => {
            rewrite_function_def(d, current_ns, const_env, state, generated)?;
        }
        Block::Proc(p) => {
            let mut proc_const_env = const_env.clone();
            let mut proc_const_names = HashSet::<String>::new();
            for decl in &mut p.consts {
                if !proc_const_names.insert(decl.name.clone()) {
                    return Err(vec![Diagnostic::semantic_span(
                        format!("duplicate proc constant '{}'", decl.name),
                        decl.loc.as_ref(),
                    )]);
                }
                let value =
                    finalize_const_decl_expr(decl, current_ns, &proc_const_env, state, generated)?;
                proc_const_env.insert(decl.name.clone(), value);
            }
            p.consts.clear();
            expand_deferred_proc_port_count(
                &mut p.ins,
                &mut p.ins_deferred_count,
                &mut p.ins_deferred_default_ty,
                "in",
                &p.loc,
                current_ns,
                &proc_const_env,
                state,
                generated,
            )?;
            expand_deferred_proc_port_count(
                &mut p.outs,
                &mut p.outs_deferred_count,
                &mut p.outs_deferred_default_ty,
                "out",
                &p.loc,
                current_ns,
                &proc_const_env,
                state,
                generated,
            )?;
            expand_deferred_proc_param_count(
                &mut p.params,
                &mut p.params_deferred_count,
                &mut p.params_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_const_env,
                state,
                generated,
            )?;
            rewrite_port_decls(&mut p.ins, current_ns, &proc_const_env, state, generated)?;
            rewrite_port_decls(&mut p.outs, current_ns, &proc_const_env, state, generated)?;
            rewrite_param_decls(&mut p.params, current_ns, &proc_const_env, state, generated)?;
            for event in &mut p.events {
                rewrite_event_def(event, current_ns, &proc_const_env, state, generated)?;
            }
            for decl in &mut p.buffers {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(
                        ty,
                        current_ns,
                        &proc_const_env,
                        state,
                        generated,
                        decl.ty_loc.as_ref().or(decl.loc.as_ref()),
                    )?;
                }
            }
            if let Some(default_ty) = &mut p.init.default_ty {
                rewrite_decl_type(
                    default_ty,
                    current_ns,
                    &proc_const_env,
                    state,
                    generated,
                    p.init.default_ty_loc.as_ref().or(p.init.loc.as_ref()),
                )?;
            }
            rewrite_stmts(
                &mut p.init.body,
                current_ns,
                &proc_const_env,
                state,
                generated,
            )?;
            rewrite_stmts(
                &mut p.block_pre,
                current_ns,
                &proc_const_env,
                state,
                generated,
            )?;
            if let Some(os) = &mut p.sample_oversample_factor {
                rewrite_expr(os, current_ns, &proc_const_env, state, generated)?;
            }
            rewrite_stmts(&mut p.sample, current_ns, &proc_const_env, state, generated)?;
            rewrite_stmts(
                &mut p.block_post,
                current_ns,
                &proc_const_env,
                state,
                generated,
            )?;
            if let Some(graph) = &mut p.graph {
                rewrite_graph_block(graph, current_ns, &proc_const_env, state, generated)?;
            }
            for def in &mut p.local_defs {
                rewrite_function_def(def, current_ns, &proc_const_env, state, generated)?;
            }
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                rewrite_decl_type(
                    default_ty,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    init.default_ty_loc.as_ref().or(init.loc.as_ref()),
                )?;
            }
            rewrite_stmts(&mut init.body, current_ns, const_env, state, generated)?;
        }
        Block::Block(exec) => {
            rewrite_stmts(&mut exec.pre, current_ns, const_env, state, generated)?;
            if let Some(sample) = &mut exec.sample {
                if let Some(os) = &mut sample.oversample_factor {
                    rewrite_expr(os, current_ns, const_env, state, generated)?;
                }
                rewrite_stmts(&mut sample.body, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut exec.post, current_ns, const_env, state, generated)?;
        }
        Block::Sample(sample) => {
            if let Some(os) = &mut sample.oversample_factor {
                rewrite_expr(os, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut sample.body, current_ns, const_env, state, generated)?;
        }
        Block::Graph(graph) => {
            rewrite_graph_block(graph, current_ns, const_env, state, generated)?;
        }
    }
    Ok(())
}

fn rewrite_port_decls(
    decls: &mut [PortDecl],
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for decl in decls {
        rewrite_decl_type_default_range(
            &mut decl.ty,
            &mut decl.default,
            &mut decl.range,
            current_ns,
            const_env,
            state,
            generated,
            decl.ty_loc.as_ref().or(decl.loc.as_ref()),
        )?;
    }
    Ok(())
}

fn rewrite_param_decls(
    decls: &mut [ParamDecl],
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for decl in decls {
        rewrite_decl_type_default_range(
            &mut decl.ty,
            &mut decl.default,
            &mut decl.range,
            current_ns,
            const_env,
            state,
            generated,
            decl.ty_loc.as_ref().or(decl.loc.as_ref()),
        )?;
    }
    Ok(())
}

fn rewrite_decl_type_default_range(
    ty: &mut Option<DeclType>,
    default: &mut Option<Expr>,
    range: &mut Option<DeclRange>,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    if let Some(ty) = ty {
        rewrite_decl_type(ty, current_ns, const_env, state, generated, loc)?;
    }
    if let Some(default) = default {
        rewrite_expr(default, current_ns, const_env, state, generated)?;
    }
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            rewrite_expr(min, current_ns, const_env, state, generated)?;
        }
        rewrite_expr(&mut range.max, current_ns, const_env, state, generated)?;
    }
    Ok(())
}

fn rewrite_graph_block(
    graph: &mut GraphBlock,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for edge in &mut graph.edges {
        rewrite_expr(&mut edge.source, current_ns, const_env, state, generated)?;
        if let Some(delay) = &mut edge.delay {
            rewrite_expr(delay, current_ns, const_env, state, generated)?;
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                rewrite_expr(index, current_ns, const_env, state, generated)?;
            }
        }
    }
    Ok(())
}

fn rewrite_function_def(
    def: &mut FunctionDef,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            rewrite_fn_param_type(
                ty,
                current_ns,
                const_env,
                state,
                generated,
                param.ty_loc.as_ref().or(param.loc.as_ref()),
            )?;
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(default, current_ns, const_env, state, generated)?;
        }
    }
    rewrite_stmts(&mut def.body, current_ns, const_env, state, generated)
}

fn rewrite_event_def(
    event: &mut EventDef,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for param in &mut event.params {
        match &mut param.ty {
            EventParamType::Array { size, .. } => {
                rewrite_expr(size, current_ns, const_env, state, generated)?;
            }
            EventParamType::GenericSlice { elem } => {
                rewrite_named_type_ref_name_at(
                    elem,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    param.ty_loc.as_ref().or(param.loc.as_ref()),
                )?;
            }
            EventParamType::Scalar(_) | EventParamType::Slice { .. } => {}
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(default, current_ns, const_env, state, generated)?;
        }
    }
    rewrite_stmts(&mut event.body, current_ns, const_env, state, generated)
}

fn rewrite_decl_type(
    ty: &mut DeclType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    match ty {
        DeclType::Generic(name) => {
            rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
        }
        DeclType::ArrayGeneric { elem, size } => {
            rewrite_named_type_ref_name_at(elem, current_ns, const_env, state, generated, loc)?;
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
        DeclType::Array { size, .. } => {
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
        DeclType::Scalar(_) | DeclType::Tuple(_) => {}
    }
    Ok(())
}

fn rewrite_field_type(
    ty: &mut FieldType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    match ty {
        FieldType::Generic(name) => {
            rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
        }
        FieldType::Array(spec) => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
            }
            rewrite_expr(&mut spec.size, current_ns, const_env, state, generated)?;
        }
        FieldType::Scalar(_) | FieldType::Tuple(_) => {}
    }
    Ok(())
}

fn rewrite_fn_param_type(
    ty: &mut FnParamType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    match ty {
        FnParamType::Struct(name) => {
            rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
        }
        FnParamType::ArrayGeneric(name) => {
            rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
        }
        FnParamType::SizedArray {
            generic_name: Some(name),
            ..
        } => {
            rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
        }
        FnParamType::Buffer(buffer_ty) => {
            rewrite_buffer_type(buffer_ty, current_ns, const_env, state, generated, loc)?;
        }
        FnParamType::Primitive(_)
        | FnParamType::Array(_)
        | FnParamType::SizedArray { .. }
        | FnParamType::BareBuffer
        | FnParamType::Tuple(_) => {}
    }
    Ok(())
}

fn rewrite_buffer_type(
    ty: &mut BufferType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    eprintln!("[rewrite-buf-debug] before: elem={:?} channels={:?} const_env_keys={:?}", ty.elem, ty.channels, const_env.keys().collect::<Vec<_>>());
    if let BufferElemType::Generic(name) = &mut ty.elem {
        rewrite_named_type_ref_name_at(name, current_ns, const_env, state, generated, loc)?;
    }
    if let BufferChannels::Static(expr) = &mut ty.channels {
        rewrite_expr(expr, current_ns, const_env, state, generated)?;
    }
    eprintln!("[rewrite-buf-debug] after: elem={:?} channels={:?}", ty.elem, ty.channels);
    Ok(())
}

fn rewrite_stmts(
    stmts: &mut Vec<Stmt>,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let mut scope_consts = const_env.clone();
    let mut local_const_names = HashSet::<String>::new();
    let mut rewritten = Vec::<Stmt>::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        if let Stmt::Const { decl, .. } = &mut stmt {
            if !local_const_names.insert(decl.name.clone()) {
                return Err(vec![Diagnostic::semantic_span(
                    format!("duplicate constant '{}' in scope", decl.name),
                    decl.loc.as_ref(),
                )]);
            }
            let value =
                finalize_const_decl_expr(decl, current_ns, &scope_consts, state, generated)?;
            scope_consts.insert(decl.name.clone(), value);
            continue;
        }
        rewrite_stmt(&mut stmt, current_ns, &scope_consts, state, generated)?;
        rewritten.push(stmt);
    }
    *stmts = rewritten;
    Ok(())
}

fn rewrite_stmt(
    stmt: &mut Stmt,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target_loc,
            target,
            generic_decl_ty,
            typed_decl_ty_loc,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if const_env.contains_key(name) {
                        return Err(vec![Diagnostic::semantic_span(
                            format!("cannot assign to constant '{}'", name),
                            target_loc.as_ref(),
                        )]);
                    }
                    if looks_like_namespace_ref(name) {
                        let resolved = resolve_namespace_symbol_name_at(
                            name,
                            current_ns,
                            const_env,
                            state,
                            generated,
                            target_loc.as_ref(),
                        )?;
                        if state.namespace_const_values.contains_key(&resolved) {
                            return Err(vec![Diagnostic::semantic_span(
                                format!("cannot assign to constant '{}'", name),
                                target_loc.as_ref(),
                            )]);
                        }
                        *name = resolved;
                    }
                }
                AssignTarget::Index { base, index } => {
                    if looks_like_namespace_ref(base) {
                        let resolved = resolve_namespace_symbol_name_at(
                            base,
                            current_ns,
                            const_env,
                            state,
                            generated,
                            target_loc.as_ref(),
                        )?;
                        if state.namespace_const_values.contains_key(&resolved) {
                            return Err(vec![Diagnostic::semantic_span(
                                format!("cannot assign to constant '{}'", base),
                                target_loc.as_ref(),
                            )]);
                        }
                        *base = resolved;
                    }
                    rewrite_expr(index, current_ns, const_env, state, generated)?;
                }
                AssignTarget::Slice { base, start, end } => {
                    if looks_like_namespace_ref(base) {
                        let resolved = resolve_namespace_symbol_name_at(
                            base,
                            current_ns,
                            const_env,
                            state,
                            generated,
                            target_loc.as_ref(),
                        )?;
                        if state.namespace_const_values.contains_key(&resolved) {
                            return Err(vec![Diagnostic::semantic_span(
                                format!("cannot assign to constant '{}'", base),
                                target_loc.as_ref(),
                            )]);
                        }
                        *base = resolved;
                    }
                    if let Some(start) = start {
                        rewrite_expr(start, current_ns, const_env, state, generated)?;
                    }
                    if let Some(end) = end {
                        rewrite_expr(end, current_ns, const_env, state, generated)?;
                    }
                }
                AssignTarget::Tuple(_) => {}
            }
            if let Some(name) = generic_decl_ty {
                if looks_like_namespace_ref(name) {
                    *name = resolve_namespace_symbol_name_at(
                        name,
                        current_ns,
                        const_env,
                        state,
                        generated,
                        typed_decl_ty_loc.as_ref().or(target_loc.as_ref()),
                    )?;
                }
            }
            rewrite_expr(expr, current_ns, const_env, state, generated)?;
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_expr(expr, current_ns, const_env, state, generated)?;
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr(cond, current_ns, const_env, state, generated)?;
            rewrite_stmts(then_branch, current_ns, const_env, state, generated)?;
            rewrite_stmts(else_branch, current_ns, const_env, state, generated)?;
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                rewrite_expr(step, current_ns, const_env, state, generated)?;
            }
            rewrite_expr(start, current_ns, const_env, state, generated)?;
            rewrite_expr(end, current_ns, const_env, state, generated)?;
            rewrite_stmts(body, current_ns, const_env, state, generated)?;
        }
        Stmt::While { cond, body, .. } => {
            rewrite_expr(cond, current_ns, const_env, state, generated)?;
            rewrite_stmts(body, current_ns, const_env, state, generated)?;
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    Ok(())
}

fn rewrite_expr(
    expr: &mut Expr,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let use_site_loc = expr.loc();
    if let Expr::Var { name, .. } = expr {
        if let Some(value) = const_env.get(name).cloned() {
            let mut value = value;
            rebase_expr_locs(&mut value, use_site_loc);
            *expr = value;
            return rewrite_expr(expr, current_ns, const_env, state, generated);
        }
    }

    match expr {
        Expr::Var { name, .. } => {
            if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
                if let Some(value) = state.namespace_const_values.get(&qualified).cloned() {
                    let mut value = value;
                    rebase_expr_locs(&mut value, use_site_loc);
                    *expr = value;
                    return rewrite_expr(expr, current_ns, const_env, state, generated);
                }
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                let resolved = resolve_namespace_symbol_name_at(
                    name,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    use_site_loc,
                )?;
                if let Some(value) = state.namespace_const_values.get(&resolved).cloned() {
                    let mut value = value;
                    rebase_expr_locs(&mut value, use_site_loc);
                    *expr = value;
                    return rewrite_expr(expr, current_ns, const_env, state, generated);
                }
                *name = resolved;
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                *base = resolve_namespace_symbol_name_at(
                    base,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    use_site_loc,
                )?;
            }
            rewrite_expr(index, current_ns, const_env, state, generated)?;
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                *base = resolve_namespace_symbol_name_at(
                    base,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    use_site_loc,
                )?;
            }
            if let Some(start) = start {
                rewrite_expr(start, current_ns, const_env, state, generated)?;
            }
            if let Some(end) = end {
                rewrite_expr(end, current_ns, const_env, state, generated)?;
            }
        }
        Expr::ArrayCtor { loc, spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    loc.as_ref(),
                )?;
            }
            rewrite_expr(&mut spec.size, current_ns, const_env, state, generated)?;
            if let Some(values) = init {
                for value in values {
                    rewrite_expr(value, current_ns, const_env, state, generated)?;
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_expr(lhs, current_ns, const_env, state, generated)?;
            rewrite_expr(rhs, current_ns, const_env, state, generated)?;
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_expr(arg, current_ns, const_env, state, generated)?;
            }
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                *name = resolve_namespace_symbol_name_at(
                    name,
                    current_ns,
                    const_env,
                    state,
                    generated,
                    use_site_loc,
                )?;
            }
            for arg in args {
                rewrite_expr(&mut arg.expr, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Cast { expr: arg, .. }
        | Expr::UnaryNot { expr: arg, .. }
        | Expr::UnaryBitNot { expr: arg, .. } => {
            rewrite_expr(arg, current_ns, const_env, state, generated)?;
        }
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                rewrite_expr(value, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_expr(value, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
    Ok(())
}
