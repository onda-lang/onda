use super::namespaces::{
    looks_like_namespace_ref, namespace_of_symbol, qualify_local_namespace_member_name,
    resolve_namespace_symbol_name, rewrite_named_type_ref_name,
};
use super::*;

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

pub(super) fn validate_compile_time_expr(
    expr: &Expr,
    known_consts: &HashMap<String, Expr>,
    context: &str,
) -> Result<(), Vec<Diagnostic>> {
    match expr {
        Expr::Number(_) | Expr::Bool(_) => Ok(()),
        Expr::Int(_) => Ok(()),
        Expr::Var(name) => {
            if known_consts.contains_key(name) || is_builtin_compile_time_const(name) {
                Ok(())
            } else {
                Err(vec![Diagnostic::semantic(
                    format!(
                        "{context}: expression references non-compile-time symbol '{}'",
                        name
                    ),
                    0,
                    0,
                )])
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } | Expr::UnaryBitNot { expr } => {
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
        | Expr::ArrayLiteral(_) => Err(vec![Diagnostic::semantic(
            format!("{context}: expression is not compile-time evaluable"),
            0,
            0,
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
        return Err(vec![Diagnostic::semantic(
            format!(
                "constant name '{}' is reserved as a builtin compile-time constant",
                decl.name
            ),
            0,
            0,
        )]);
    }

    rewrite_expr(&mut decl.expr, current_ns, const_env, state, generated)?;
    validate_compile_time_expr(&decl.expr, const_env, &format!("const '{}'", decl.name))?;

    let expr = decl.expr.clone();
    Ok(if let Some(ty) = decl.ty {
        Expr::Cast {
            to: ty,
            expr: Box::new(expr),
        }
    } else {
        expr
    })
}

pub(super) fn substitute_expr_with_env(expr: &Expr, const_env: &HashMap<String, Expr>) -> Expr {
    match expr {
        Expr::Var(name) => const_env
            .get(name)
            .cloned()
            .unwrap_or_else(|| Expr::Var(name.clone())),
        Expr::ArrayLiteral(values) => Expr::ArrayLiteral(
            values
                .iter()
                .map(|v| substitute_expr_with_env(v, const_env))
                .collect(),
        ),
        Expr::Index { base, index } => Expr::Index {
            base: base.clone(),
            index: Box::new(substitute_expr_with_env(index, const_env)),
        },
        Expr::Slice { base, start, end } => Expr::Slice {
            base: base.clone(),
            start: start
                .as_ref()
                .map(|expr| Box::new(substitute_expr_with_env(expr, const_env))),
            end: end
                .as_ref()
                .map(|expr| Box::new(substitute_expr_with_env(expr, const_env))),
        },
        Expr::ArrayCtor { spec, init } => Expr::ArrayCtor {
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
        Expr::Compare { op, lhs, rhs } => Expr::Compare {
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Call { func, args } => Expr::Call {
            func: *func,
            args: args
                .iter()
                .map(|a| substitute_expr_with_env(a, const_env))
                .collect(),
        },
        Expr::UserCall {
            name,
            type_args,
            args,
        } => Expr::UserCall {
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
        Expr::Cast { to, expr } => Expr::Cast {
            to: *to,
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::UnaryNot { expr } => Expr::UnaryNot {
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::UnaryBitNot { expr } => Expr::UnaryBitNot {
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::Logical { op, lhs, rhs } => Expr::Logical {
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => expr.clone(),
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
                return Err(vec![Diagnostic::semantic(
                    format!("duplicate top-level constant '{}'", decl.name),
                    0,
                    0,
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

pub(super) fn rewrite_block_namespace_refs(
    block: &mut Block,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match block {
        Block::Ins(decls) | Block::Outs(decls) => {
            rewrite_port_decls(decls, current_ns, const_env, state, generated)?;
        }
        Block::Params(decls) => {
            rewrite_param_decls(decls, current_ns, const_env, state, generated)?;
        }
        Block::Const(_) => {}
        Block::Buffers(decls) => {
            for decl in decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(ty, current_ns, const_env, state, generated)?;
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
                rewrite_field_type(&mut field.ty, current_ns, const_env, state, generated)?;
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
            rewrite_port_decls(&mut p.ins, current_ns, const_env, state, generated)?;
            rewrite_port_decls(&mut p.outs, current_ns, const_env, state, generated)?;
            rewrite_param_decls(&mut p.params, current_ns, const_env, state, generated)?;
            for event in &mut p.events {
                rewrite_event_def(event, current_ns, const_env, state, generated)?;
            }
            for decl in &mut p.buffers {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(ty, current_ns, const_env, state, generated)?;
                }
            }
            if let Some(default_ty) = &mut p.init.default_ty {
                rewrite_decl_type(default_ty, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut p.init.body, current_ns, const_env, state, generated)?;
            rewrite_stmts(&mut p.block_pre, current_ns, const_env, state, generated)?;
            if let Some(os) = &mut p.sample_oversample_factor {
                rewrite_expr(os, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut p.sample, current_ns, const_env, state, generated)?;
            rewrite_stmts(&mut p.block_post, current_ns, const_env, state, generated)?;
            if let Some(graph) = &mut p.graph {
                rewrite_graph_block(graph, current_ns, const_env, state, generated)?;
            }
            for def in &mut p.local_defs {
                rewrite_function_def(def, current_ns, const_env, state, generated)?;
            }
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                rewrite_decl_type(default_ty, current_ns, const_env, state, generated)?;
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
) -> Result<(), Vec<Diagnostic>> {
    if let Some(ty) = ty {
        rewrite_decl_type(ty, current_ns, const_env, state, generated)?;
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
            rewrite_fn_param_type(ty, current_ns, const_env, state, generated)?;
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
            EventParamType::Scalar(_)
            | EventParamType::Slice { .. }
            | EventParamType::GenericSlice { .. } => {}
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
) -> Result<(), Vec<Diagnostic>> {
    match ty {
        DeclType::Generic(name) => {
            rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
        }
        DeclType::ArrayGeneric { elem, size } => {
            rewrite_named_type_ref_name(elem, current_ns, const_env, state, generated)?;
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
        DeclType::Array { size, .. } => {
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
        DeclType::Scalar(_) => {}
    }
    Ok(())
}

fn rewrite_field_type(
    ty: &mut FieldType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match ty {
        FieldType::Generic(name) => {
            rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
        }
        FieldType::Array(spec) => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
            }
            rewrite_expr(&mut spec.size, current_ns, const_env, state, generated)?;
        }
        FieldType::Scalar(_) => {}
    }
    Ok(())
}

fn rewrite_fn_param_type(
    ty: &mut FnParamType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match ty {
        FnParamType::Struct(name) => {
            rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
        }
        FnParamType::ArrayGeneric(name) => {
            rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
        }
        FnParamType::Buffer(buffer_ty) => {
            rewrite_buffer_type(buffer_ty, current_ns, const_env, state, generated)?;
        }
        FnParamType::Primitive(_) | FnParamType::Array(_) | FnParamType::BareBuffer => {}
    }
    Ok(())
}

fn rewrite_buffer_type(
    ty: &mut BufferType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    if let BufferElemType::Generic(name) = &mut ty.elem {
        rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
    }
    if let BufferChannels::Static(expr) = &mut ty.channels {
        rewrite_expr(expr, current_ns, const_env, state, generated)?;
    }
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
                return Err(vec![Diagnostic::semantic(
                    format!("duplicate constant '{}' in scope", decl.name),
                    0,
                    0,
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
            target,
            generic_decl_ty,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if const_env.contains_key(name) {
                        return Err(vec![Diagnostic::semantic(
                            format!("cannot assign to constant '{}'", name),
                            0,
                            0,
                        )]);
                    }
                    if looks_like_namespace_ref(name) {
                        let resolved = resolve_namespace_symbol_name(
                            name, current_ns, const_env, state, generated,
                        )?;
                        if state.namespace_const_values.contains_key(&resolved) {
                            return Err(vec![Diagnostic::semantic(
                                format!("cannot assign to constant '{}'", name),
                                0,
                                0,
                            )]);
                        }
                        *name = resolved;
                    }
                }
                AssignTarget::Index { base, index } => {
                    if looks_like_namespace_ref(base) {
                        let resolved = resolve_namespace_symbol_name(
                            base, current_ns, const_env, state, generated,
                        )?;
                        if state.namespace_const_values.contains_key(&resolved) {
                            return Err(vec![Diagnostic::semantic(
                                format!("cannot assign to constant '{}'", base),
                                0,
                                0,
                            )]);
                        }
                        *base = resolved;
                    }
                    rewrite_expr(index, current_ns, const_env, state, generated)?;
                }
                AssignTarget::Slice { base, start, end } => {
                    if looks_like_namespace_ref(base) {
                        let resolved = resolve_namespace_symbol_name(
                            base, current_ns, const_env, state, generated,
                        )?;
                        if state.namespace_const_values.contains_key(&resolved) {
                            return Err(vec![Diagnostic::semantic(
                                format!("cannot assign to constant '{}'", base),
                                0,
                                0,
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
            }
            if let Some(name) = generic_decl_ty {
                if looks_like_namespace_ref(name) {
                    *name = resolve_namespace_symbol_name(
                        name, current_ns, const_env, state, generated,
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
    if let Expr::Var(name) = expr {
        if let Some(value) = const_env.get(name).cloned() {
            *expr = value;
            return rewrite_expr(expr, current_ns, const_env, state, generated);
        }
    }

    match expr {
        Expr::Var(name) => {
            if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
                if let Some(value) = state.namespace_const_values.get(&qualified).cloned() {
                    *expr = value;
                    return rewrite_expr(expr, current_ns, const_env, state, generated);
                }
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                let resolved =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
                if let Some(value) = state.namespace_const_values.get(&resolved).cloned() {
                    *expr = value;
                    return rewrite_expr(expr, current_ns, const_env, state, generated);
                }
                *name = resolved;
            }
        }
        Expr::Index { base, index } => {
            if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                *base =
                    resolve_namespace_symbol_name(base, current_ns, const_env, state, generated)?;
            }
            rewrite_expr(index, current_ns, const_env, state, generated)?;
        }
        Expr::Slice { base, start, end } => {
            if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                *base =
                    resolve_namespace_symbol_name(base, current_ns, const_env, state, generated)?;
            }
            if let Some(start) = start {
                rewrite_expr(start, current_ns, const_env, state, generated)?;
            }
            if let Some(end) = end {
                rewrite_expr(end, current_ns, const_env, state, generated)?;
            }
        }
        Expr::ArrayCtor { spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(name, current_ns, const_env, state, generated)?;
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
                *name =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
            }
            for arg in args {
                rewrite_expr(&mut arg.expr, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Cast { expr: arg, .. }
        | Expr::UnaryNot { expr: arg }
        | Expr::UnaryBitNot { expr: arg } => {
            rewrite_expr(arg, current_ns, const_env, state, generated)?;
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_expr(value, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
    Ok(())
}
