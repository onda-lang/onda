use super::*;

pub(super) fn fixed_array_type_label(elem_ty: PrimitiveType, len: usize) -> String {
    format!("{}[{len}]", primitive_type_label(elem_ty))
}

pub(super) fn reject_forward_const_ref_name(
    name: &str,
    loc: SourceLoc,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if future_consts.contains(name) {
        errors.push(Diagnostic::semantic_span(
            format!("constant '{name}' is not visible before its declaration"),
            loc,
        ));
    }
}

pub(super) fn reject_forward_const_refs_expr(
    expr: &Expr,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for expr in expr.walk() {
        match expr {
            Expr::Var { name, .. }
            | Expr::Index { base: name, .. }
            | Expr::Slice { base: name, .. } => {
                reject_forward_const_ref_name(name, expr.loc(), future_consts, errors);
            }
            Expr::UserCall { name, args, .. } => {
                reject_forward_const_ref_name(name, expr.loc(), future_consts, errors);
                if args.is_empty() {
                    if let Some(base) = parse_array_len_instance_base(name) {
                        reject_forward_const_ref_name(base, expr.loc(), future_consts, errors);
                    }
                }
                if let Some((base, _)) = name.rsplit_once('.') {
                    reject_forward_const_ref_name(base, expr.loc(), future_consts, errors);
                }
            }
            _ => {}
        }
    }
}

pub(super) fn reject_forward_const_refs_decl_type(
    ty: &Option<DeclType>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            reject_forward_const_refs_expr(size, future_consts, errors);
        }
        Some(
            DeclType::Slice(_) | DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_),
        )
        | None => {}
    }
}

pub(super) fn reject_forward_const_refs_decl_range(
    range: &Option<DeclRange>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &range.min {
            reject_forward_const_refs_expr(min, future_consts, errors);
        }
        reject_forward_const_refs_expr(&range.max, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_port_decl(
    decl: &PortDecl,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    reject_forward_const_refs_decl_type(&decl.ty, future_consts, errors);
    if let Some(default) = &decl.default {
        reject_forward_const_refs_expr(default, future_consts, errors);
    }
    reject_forward_const_refs_decl_range(&decl.range, future_consts, errors);
}

pub(super) fn reject_forward_const_refs_param_decl(
    decl: &ParamDecl,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    reject_forward_const_refs_decl_type(&decl.ty, future_consts, errors);
    if let Some(default) = &decl.default {
        reject_forward_const_refs_expr(default, future_consts, errors);
    }
    reject_forward_const_refs_decl_range(&decl.range, future_consts, errors);
    for expr in [
        &decl.control.curve,
        &decl.control.step,
        &decl.control.smooth,
    ]
    .into_iter()
    .flatten()
    {
        reject_forward_const_refs_expr(expr, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_buffer_type(
    ty: &Option<BufferType>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        reject_forward_const_refs_expr(expr, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_fn_param_type(
    ty: &Option<FnParamType>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            reject_forward_const_refs_expr(size, future_consts, errors);
        }
        Some(FnParamType::Buffer(buffer_ty))
        | Some(FnParamType::BufferArray {
            buffer: buffer_ty, ..
        }) => {
            if let BufferChannels::Static(expr) = &buffer_ty.channels {
                reject_forward_const_refs_expr(expr, future_consts, errors);
            }
        }
        Some(
            FnParamType::Primitive(_)
            | FnParamType::Struct(_)
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::BareBuffer
            | FnParamType::Tuple(_),
        )
        | None => {}
    }
}

pub(super) fn reject_forward_const_refs_event_param_type(
    ty: &EventParamType,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            reject_forward_const_refs_expr(size, future_consts, errors);
        }
        _ => {}
    }
}

pub(super) fn reject_forward_const_refs_field_type(
    ty: &FieldType,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        reject_forward_const_refs_expr(&spec.size, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_return_type(
    ty: &Option<FnReturnType>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            reject_forward_const_refs_expr(size, future_consts, errors);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

pub(super) fn reject_forward_const_refs_assign_target(
    target: &AssignTarget,
    target_loc: SourceLoc,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match target {
        AssignTarget::Index { base, index } => {
            reject_forward_const_ref_name(base, target_loc, future_consts, errors);
            reject_forward_const_refs_expr(index, future_consts, errors);
        }
        AssignTarget::IndexedMember { base, .. } => {
            reject_forward_const_ref_name(base, target_loc, future_consts, errors);
            target.visit_selectors(|selector| {
                reject_forward_const_refs_expr(selector, future_consts, errors)
            });
        }
        AssignTarget::Slice {
            base,
            selector,
            channel,
            start,
            end,
        } => {
            reject_forward_const_ref_name(base, target_loc, future_consts, errors);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                reject_forward_const_refs_expr(coordinate, future_consts, errors);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

pub(super) fn reject_forward_const_refs_stmt(
    stmt: &Stmt,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target_loc,
            target,
            expr,
            ..
        } => {
            reject_forward_const_refs_assign_target(
                target,
                target_loc.as_ref().into(),
                future_consts,
                errors,
            );
            reject_forward_const_refs_expr(expr, future_consts, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            reject_forward_const_refs_expr(expr, future_consts, errors);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                reject_forward_const_refs_expr(value, future_consts, errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            reject_forward_const_refs_expr(cond, future_consts, errors);
            for nested in then_branch {
                reject_forward_const_refs_stmt(nested, future_consts, errors);
            }
            for nested in else_branch {
                reject_forward_const_refs_stmt(nested, future_consts, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                reject_forward_const_refs_expr(step, future_consts, errors);
            }
            reject_forward_const_refs_expr(start, future_consts, errors);
            reject_forward_const_refs_expr(end, future_consts, errors);
            for nested in body {
                reject_forward_const_refs_stmt(nested, future_consts, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            reject_forward_const_refs_expr(cond, future_consts, errors);
            for nested in body {
                reject_forward_const_refs_stmt(nested, future_consts, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn reject_forward_const_refs_function(
    def: &FunctionDef,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_forward_const_refs_fn_param_type(&param.ty, future_consts, errors);
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, future_consts, errors);
        }
    }
    reject_forward_const_refs_return_type(&def.return_ty, future_consts, errors);
    for stmt in &def.body {
        reject_forward_const_refs_stmt(stmt, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_event(
    event: &EventDef,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &event.params {
        reject_forward_const_refs_event_param_type(&param.ty, future_consts, errors);
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, future_consts, errors);
        }
    }
    for stmt in &event.body {
        reject_forward_const_refs_stmt(stmt, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_delegate(
    delegate: &DelegateDef,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &delegate.params {
        reject_forward_const_refs_event_param_type(&param.ty, future_consts, errors);
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, future_consts, errors);
        }
    }
}

pub(super) fn reject_forward_const_refs_when(
    when: &WhenDef,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(index) = &when.target.index {
        reject_forward_const_refs_expr(index, future_consts, errors);
    }
    for stmt in &when.body {
        reject_forward_const_refs_stmt(stmt, future_consts, errors);
    }
}

pub(super) fn reject_forward_const_refs_graph(
    graph: &GraphBlock,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &graph.edges {
        reject_forward_const_refs_expr(&edge.source, future_consts, errors);
        if let Some(delay) = &edge.delay {
            reject_forward_const_refs_expr(delay, future_consts, errors);
        }
        for dest in &edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                reject_forward_const_refs_expr(index, future_consts, errors);
            }
        }
    }
}

pub(super) fn reject_forward_const_refs_in_block(
    block: &Block,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &ports.deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            for decl in &ports.decls {
                reject_forward_const_refs_port_decl(decl, future_consts, errors);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &params.deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            for decl in &params.decls {
                reject_forward_const_refs_param_decl(decl, future_consts, errors);
            }
        }
        Block::Events(events) => {
            for event in &events.events {
                reject_forward_const_refs_event(event, future_consts, errors);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &delegates.delegates {
                reject_forward_const_refs_delegate(delegate, future_consts, errors);
            }
        }
        Block::When(when) => reject_forward_const_refs_when(when, future_consts, errors),
        Block::Tasks(tasks) => {
            for task in &tasks.tasks {
                for stmt in &task.body {
                    reject_forward_const_refs_stmt(stmt, future_consts, errors);
                }
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &buffers.deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            for decl in &buffers.decls {
                reject_forward_const_refs_buffer_type(&decl.ty, future_consts, errors);
            }
        }
        Block::Init(init) => {
            for stmt in &init.body {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &block_exec.pre {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
            if let Some(sample) = &block_exec.sample {
                if let Some(factor) = &sample.oversample_factor {
                    reject_forward_const_refs_expr(factor, future_consts, errors);
                }
                for stmt in &sample.body {
                    reject_forward_const_refs_stmt(stmt, future_consts, errors);
                }
            }
            for stmt in &block_exec.post {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &sample.oversample_factor {
                reject_forward_const_refs_expr(factor, future_consts, errors);
            }
            for stmt in &sample.body {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
        }
        Block::Graph(graph) => {
            reject_forward_const_refs_graph(graph, future_consts, errors);
        }
        Block::Assert(assert_decl) => {
            reject_forward_const_refs_expr(&assert_decl.expr, future_consts, errors);
        }
        Block::Def(def) if !def.is_const => {
            reject_forward_const_refs_function(def, future_consts, errors);
        }
        Block::Struct(struct_def) => {
            for field in &struct_def.fields {
                reject_forward_const_refs_field_type(&field.ty, future_consts, errors);
                if let Some(default) = &field.default {
                    reject_forward_const_refs_expr(default, future_consts, errors);
                }
            }
            for method in &struct_def.methods {
                reject_forward_const_refs_function(method, future_consts, errors);
            }
        }
        Block::Proc(proc) => {
            if let Some(count) = &proc.ins_deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            if let Some(default_ty) = &proc.ins_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, future_consts, errors);
            }
            if let Some(count) = &proc.outs_deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            if let Some(default_ty) = &proc.outs_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, future_consts, errors);
            }
            if let Some(count) = &proc.params_deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            if let Some(default_ty) = &proc.params_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, future_consts, errors);
            }
            if let Some(count) = &proc.buffers_deferred_count {
                reject_forward_const_refs_expr(count, future_consts, errors);
            }
            reject_forward_const_refs_buffer_type(
                &proc.buffers_deferred_default_ty,
                future_consts,
                errors,
            );
            for decl in &proc.ins {
                reject_forward_const_refs_port_decl(decl, future_consts, errors);
            }
            for decl in &proc.outs {
                reject_forward_const_refs_port_decl(decl, future_consts, errors);
            }
            for decl in &proc.params {
                reject_forward_const_refs_param_decl(decl, future_consts, errors);
            }
            for decl in &proc.buffers {
                reject_forward_const_refs_buffer_type(&decl.ty, future_consts, errors);
            }
            if let Some(default_ty) = &proc.init.default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, future_consts, errors);
            }
            if let Some(factor) = &proc.sample_oversample_factor {
                reject_forward_const_refs_expr(factor, future_consts, errors);
            }
            for event in &proc.events {
                reject_forward_const_refs_event(event, future_consts, errors);
            }
            for delegate in &proc.delegates {
                reject_forward_const_refs_delegate(delegate, future_consts, errors);
            }
            for when in &proc.whens {
                reject_forward_const_refs_when(when, future_consts, errors);
            }
            for stmt in &proc.init.body {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
            for stmt in &proc.block_pre {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
            for stmt in &proc.sample {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
            for stmt in &proc.block_post {
                reject_forward_const_refs_stmt(stmt, future_consts, errors);
            }
            if let Some(graph) = &proc.graph {
                reject_forward_const_refs_graph(graph, future_consts, errors);
            }
            for task in &proc.tasks {
                for stmt in &task.body {
                    reject_forward_const_refs_stmt(stmt, future_consts, errors);
                }
            }
            for def in &proc.local_defs {
                reject_forward_const_refs_function(def, future_consts, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

pub(super) fn reject_const_assignment_target(
    target: &AssignTarget,
    target_loc: &Span,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let AssignTarget::Var(name) = target {
        if const_symbols.contains(name) {
            errors.push(Diagnostic::semantic_span(
                format!("cannot assign to constant '{name}'"),
                target_loc.as_ref(),
            ));
        }
    }
}

pub(super) fn reject_const_shadowing_name(
    symbol_kind: &str,
    name: &str,
    loc: Span,
    scope_ns: &str,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(const_name) = visible_const_symbol_for_local_name(name, scope_ns, const_symbols) {
        errors.push(Diagnostic::semantic_span(
            format!("{symbol_kind} '{name}' conflicts with constant '{const_name}'"),
            loc.as_ref(),
        ));
    }
}

pub(super) fn reject_const_shadowing_stmt(
    stmt: &Stmt,
    scope_ns: &str,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target, target_loc, ..
        } => {
            if let AssignTarget::Tuple(names) = target {
                for name in names.iter().filter_map(|target| target.binding()) {
                    reject_const_shadowing_name(
                        "tuple assignment target",
                        name,
                        *target_loc,
                        scope_ns,
                        const_symbols,
                        errors,
                    );
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                reject_const_shadowing_stmt(nested, scope_ns, const_symbols, errors);
            }
            for nested in else_branch {
                reject_const_shadowing_stmt(nested, scope_ns, const_symbols, errors);
            }
        }
        Stmt::For { var, loc, body, .. } => {
            reject_const_shadowing_name(
                "loop variable",
                var,
                *loc,
                scope_ns,
                const_symbols,
                errors,
            );
            for nested in body {
                reject_const_shadowing_stmt(nested, scope_ns, const_symbols, errors);
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                reject_const_shadowing_stmt(nested, scope_ns, const_symbols, errors);
            }
        }
        Stmt::Expr { .. }
        | Stmt::Print { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => {}
    }
}

pub(super) fn reject_const_shadowing_function(
    def: &FunctionDef,
    scope_ns: &str,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_const_shadowing_name(
            "function parameter",
            &param.name,
            param.loc,
            scope_ns,
            const_symbols,
            errors,
        );
    }
    for stmt in &def.body {
        reject_const_shadowing_stmt(stmt, scope_ns, const_symbols, errors);
    }
}

pub(super) fn reject_const_shadowing_event(
    event: &EventDef,
    scope_ns: &str,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &event.params {
        reject_const_shadowing_name(
            "event parameter",
            &param.name,
            param.loc,
            scope_ns,
            const_symbols,
            errors,
        );
    }
    for stmt in &event.body {
        reject_const_shadowing_stmt(stmt, scope_ns, const_symbols, errors);
    }
}

pub(super) fn reject_const_shadowing_proc_decl(
    proc_name: &str,
    symbol_kind: &str,
    name: &str,
    loc: Span,
    scope_ns: &str,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(const_name) = visible_const_symbol_for_local_name(name, scope_ns, const_symbols) {
        errors.push(Diagnostic::semantic_span(
            format!("{symbol_kind} '{name}' in processor '{proc_name}' conflicts with constant '{const_name}'"),
            loc.as_ref(),
        ));
    }
}

pub(super) fn reject_const_shadowing_in_program(
    program: &Program,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        match block {
            Block::Events(events) => {
                for event in &events.events {
                    let scope_ns = symbol_namespace(&event.name);
                    reject_const_shadowing_event(event, &scope_ns, const_symbols, errors);
                }
            }
            Block::Delegates(delegates) => {
                for delegate in &delegates.delegates {
                    let scope_ns = symbol_namespace(&delegate.name);
                    for param in &delegate.params {
                        if let Some(const_name) = visible_const_symbol_for_local_name(
                            &param.name,
                            &scope_ns,
                            const_symbols,
                        ) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "delegate parameter '{}' in '{}' conflicts with constant '{}'",
                                    param.name, delegate.name, const_name
                                ),
                                param.loc.as_ref(),
                            ));
                        }
                    }
                }
            }
            Block::When(when) => {
                for stmt in &when.body {
                    reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                }
            }
            Block::Tasks(tasks) => {
                for task in &tasks.tasks {
                    for stmt in &task.body {
                        reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                    }
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                }
            }
            Block::Block(block_exec) => {
                for stmt in &block_exec.pre {
                    reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                }
                if let Some(sample) = &block_exec.sample {
                    for stmt in &sample.body {
                        reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                    }
                }
                for stmt in &block_exec.post {
                    reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                }
            }
            Block::Sample(sample) => {
                for stmt in &sample.body {
                    reject_const_shadowing_stmt(stmt, "", const_symbols, errors);
                }
            }
            Block::Def(def) if !def.is_const => {
                let scope_ns = symbol_namespace(&def.name);
                reject_const_shadowing_function(def, &scope_ns, const_symbols, errors);
            }
            Block::Struct(struct_def) => {
                let scope_ns = symbol_namespace(&struct_def.name);
                for method in &struct_def.methods {
                    reject_const_shadowing_function(method, &scope_ns, const_symbols, errors);
                }
            }
            Block::Proc(proc) => {
                let scope_ns = symbol_namespace(&proc.name);
                for decl in &proc.ins {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor input",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_symbols,
                        errors,
                    );
                }
                for decl in &proc.outs {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor output",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_symbols,
                        errors,
                    );
                }
                for decl in &proc.params {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor parameter",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_symbols,
                        errors,
                    );
                }
                for decl in &proc.buffers {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor buffer",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_symbols,
                        errors,
                    );
                }
                for event in &proc.events {
                    reject_const_shadowing_event(event, &scope_ns, const_symbols, errors);
                }
                for delegate in &proc.delegates {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "delegate",
                        &delegate.name,
                        delegate.loc,
                        &scope_ns,
                        const_symbols,
                        errors,
                    );
                }
                for when in &proc.whens {
                    for stmt in &when.body {
                        reject_const_shadowing_stmt(stmt, &scope_ns, const_symbols, errors);
                    }
                }
                for stmt in &proc.init.body {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_symbols, errors);
                }
                for stmt in &proc.block_pre {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_symbols, errors);
                }
                for stmt in &proc.sample {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_symbols, errors);
                }
                for stmt in &proc.block_post {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_symbols, errors);
                }
                for task in &proc.tasks {
                    for stmt in &task.body {
                        reject_const_shadowing_stmt(stmt, &scope_ns, const_symbols, errors);
                    }
                }
                for def in &proc.local_defs {
                    reject_const_shadowing_function(def, &scope_ns, const_symbols, errors);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Graph(_)
            | Block::Const(_)
            | Block::Def(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
}

pub(super) fn reject_const_assignments_stmt(
    stmt: &Stmt,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target_loc, target, ..
        } => reject_const_assignment_target(target, target_loc, const_symbols, errors),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                reject_const_assignments_stmt(nested, const_symbols, errors);
            }
            for nested in else_branch {
                reject_const_assignments_stmt(nested, const_symbols, errors);
            }
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            for nested in body {
                reject_const_assignments_stmt(nested, const_symbols, errors);
            }
        }
        Stmt::Expr { .. }
        | Stmt::Print { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => {}
    }
}

pub(super) fn reject_const_assignments_function(
    def: &FunctionDef,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    // Const bodies validate their own lexical bindings, including locals that
    // shadow captured globals. Runtime bodies retain the global assignment guard.
    if def.is_const {
        return;
    }
    for stmt in &def.body {
        reject_const_assignments_stmt(stmt, const_symbols, errors);
    }
}

pub(super) fn reject_const_assignments_event(
    event: &EventDef,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in &event.body {
        reject_const_assignments_stmt(stmt, const_symbols, errors);
    }
}

pub(super) fn reject_const_assignments_in_program(
    program: &Program,
    const_symbols: &ConstSymbolSet<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        match block {
            Block::Events(events) => {
                for event in &events.events {
                    reject_const_assignments_event(event, const_symbols, errors);
                }
            }
            Block::Delegates(_) => {}
            Block::When(when) => {
                for stmt in &when.body {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
            }
            Block::Tasks(tasks) => {
                for task in &tasks.tasks {
                    for stmt in &task.body {
                        reject_const_assignments_stmt(stmt, const_symbols, errors);
                    }
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
            }
            Block::Block(block_exec) => {
                for stmt in &block_exec.pre {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
                if let Some(sample) = &block_exec.sample {
                    for stmt in &sample.body {
                        reject_const_assignments_stmt(stmt, const_symbols, errors);
                    }
                }
                for stmt in &block_exec.post {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
            }
            Block::Sample(sample) => {
                for stmt in &sample.body {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
            }
            Block::Def(def) => reject_const_assignments_function(def, const_symbols, errors),
            Block::Struct(struct_def) => {
                for method in &struct_def.methods {
                    reject_const_assignments_function(method, const_symbols, errors);
                }
            }
            Block::Proc(proc) => {
                for event in &proc.events {
                    reject_const_assignments_event(event, const_symbols, errors);
                }
                for when in &proc.whens {
                    for stmt in &when.body {
                        reject_const_assignments_stmt(stmt, const_symbols, errors);
                    }
                }
                for stmt in &proc.init.body {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
                for stmt in &proc.block_pre {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
                for stmt in &proc.sample {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
                for stmt in &proc.block_post {
                    reject_const_assignments_stmt(stmt, const_symbols, errors);
                }
                for task in &proc.tasks {
                    for stmt in &task.body {
                        reject_const_assignments_stmt(stmt, const_symbols, errors);
                    }
                }
                for def in &proc.local_defs {
                    reject_const_assignments_function(def, const_symbols, errors);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Graph(_)
            | Block::Const(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
}

pub(super) fn const_def_registry(artifacts: &SemanticConstArtifacts) -> ConstDefRegistry<'_> {
    &artifacts.const_defs
}

pub(super) fn substitute_scalar_const_expr(
    expr: &mut Expr,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    expr.visit_mut(|expr| {
        if let Expr::Var { loc, name } = expr {
            if let Some(value) = local_consts.get(name) {
                *expr = contextual_const_expr(*value).with_loc(*loc);
            }
        }
        true
    });
}

pub(super) fn substitute_scalar_const_decl_type(
    ty: &mut Option<DeclType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            substitute_scalar_const_expr(size, local_consts);
        }
        Some(
            DeclType::Slice(_) | DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_),
        )
        | None => {}
    }
}

pub(super) fn substitute_scalar_const_buffer_type(
    ty: &mut Option<BufferType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        substitute_scalar_const_expr(expr, local_consts);
    }
}

pub(super) fn substitute_scalar_const_fn_param_type(
    ty: &mut Option<FnParamType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            substitute_scalar_const_expr(size, local_consts);
        }
        Some(FnParamType::Buffer(buffer_ty))
        | Some(FnParamType::BufferArray {
            buffer: buffer_ty, ..
        }) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                substitute_scalar_const_expr(expr, local_consts);
            }
        }
        Some(
            FnParamType::Primitive(_)
            | FnParamType::Struct(_)
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::BareBuffer
            | FnParamType::Tuple(_),
        )
        | None => {}
    }
}

pub(super) fn substitute_scalar_const_event_param_type(
    ty: &mut EventParamType,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            substitute_scalar_const_expr(size, local_consts);
        }
        _ => {}
    }
}

pub(super) fn substitute_scalar_const_field_type(
    ty: &mut FieldType,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let FieldType::Array(spec) = ty {
        substitute_scalar_const_expr(&mut spec.size, local_consts);
    }
}

pub(super) fn substitute_scalar_const_return_type(
    ty: &mut Option<FnReturnType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            substitute_scalar_const_expr(size, local_consts);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

pub(super) fn substitute_scalar_const_decl_range(
    range: &mut Option<DeclRange>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            substitute_scalar_const_expr(min, local_consts);
        }
        substitute_scalar_const_expr(&mut range.max, local_consts);
    }
}

pub(super) fn substitute_scalar_const_port_decl(
    decl: &mut PortDecl,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    substitute_scalar_const_decl_type(&mut decl.ty, local_consts);
    if let Some(default) = &mut decl.default {
        substitute_scalar_const_expr(default, local_consts);
    }
    substitute_scalar_const_decl_range(&mut decl.range, local_consts);
}

pub(super) fn substitute_scalar_const_param_decl(
    decl: &mut ParamDecl,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    substitute_scalar_const_decl_type(&mut decl.ty, local_consts);
    if let Some(default) = &mut decl.default {
        substitute_scalar_const_expr(default, local_consts);
    }
    substitute_scalar_const_decl_range(&mut decl.range, local_consts);
    for expr in [
        &mut decl.control.curve,
        &mut decl.control.step,
        &mut decl.control.smooth,
    ]
    .into_iter()
    .flatten()
    {
        substitute_scalar_const_expr(expr, local_consts);
    }
}

pub(super) fn eval_count_shorthand(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let folded = fold_const_eval_expr(
        expr,
        &locals,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        context,
        &[],
        errors,
    )?;
    eval_data_size_expr(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_port_count_shorthand(
    decls: &mut Vec<PortDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(PortDecl {
                loc,
                name: format!("{prefix}{idx}"),
                output_timing: None,
                output_timing_loc: Span::ZERO,
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_param_count_shorthand(
    decls: &mut Vec<ParamDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(ParamDecl {
                loc,
                name: format!("{prefix}{idx}"),
                private: false,
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
                control: Default::default(),
                bind: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_buffer_count_shorthand(
    decls: &mut Vec<BufferDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<BufferType>,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(BufferDecl {
                loc,
                name: format!("buf{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                array_size: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

pub(super) fn expand_proc_count_shorthand(
    proc: &mut ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let loc = proc.loc;
    let proc_name = proc.name.clone();
    expand_port_count_shorthand(
        &mut proc.ins,
        &mut proc.ins_deferred_count,
        &mut proc.ins_deferred_default_ty,
        "in",
        &format!("processor '{proc_name}' ins"),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_port_count_shorthand(
        &mut proc.outs,
        &mut proc.outs_deferred_count,
        &mut proc.outs_deferred_default_ty,
        match proc.outs_timing {
            OutputTiming::Sample => "out",
            OutputTiming::Block => "kout",
        },
        &format!(
            "processor '{proc_name}' {}",
            match proc.outs_timing {
                OutputTiming::Sample => "outs",
                OutputTiming::Block => "kouts",
            }
        ),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_param_count_shorthand(
        &mut proc.params,
        &mut proc.params_deferred_count,
        &mut proc.params_deferred_default_ty,
        "param",
        &format!("processor '{proc_name}' params"),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_buffer_count_shorthand(
        &mut proc.buffers,
        &mut proc.buffers_deferred_count,
        &mut proc.buffers_deferred_default_ty,
        &format!("processor '{proc_name}' buffers"),
        loc,
        artifacts,
        options,
        errors,
    );
}

pub(super) fn proc_options_for_count_expansion(
    proc: &ProcessorDef,
    options: AnalysisOptions,
) -> AnalysisOptions {
    let sample_oversample_factor = validated_sample_oversample_factor(
        proc.sample_oversample_factor.as_ref(),
        options,
        &format!("processor '{}' sample oversample factor", proc.name),
        &mut Vec::new(),
    );
    proc_runtime_analysis_options(options, sample_oversample_factor)
}

pub(super) fn coerce_consts_and_expand_counts(
    program: &mut Program,
    mut artifacts: SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> SemanticConstArtifacts {
    let mut seen = HashSet::<String>::new();
    let ordinary_symbols = ordinary_top_level_symbol_names(program);
    let mut future_const_symbols = top_level_const_symbol_names(program);
    future_const_symbols.extend(program.blocks.iter().filter_map(|block| match block {
        Block::Def(def) if def.is_const => Some(def.name.clone()),
        _ => None,
    }));
    for name in &ordinary_symbols {
        future_const_symbols.remove(name);
    }
    for block in &mut program.blocks {
        match block {
            Block::Const(decl) => {
                future_const_symbols.remove(&decl.name);
            }
            Block::Def(def) if def.is_const => {
                future_const_symbols.remove(&def.name);
            }
            _ => {}
        }
        let before = errors.len();
        reject_forward_const_refs_in_block(block, &future_const_symbols, errors);
        if errors.len() != before {
            continue;
        }
        let has_declaration_metadata = match block {
            Block::Const(_) => false,
            Block::Def(def) if def.is_const => false,
            _ => true,
        };
        if has_declaration_metadata {
            normalize_declaration_metadata(block, &mut artifacts, options, errors);
        }
        match block {
            Block::Def(def) if def.is_const => {
                if is_builtin_constant_name(&def.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def name '{}' is reserved as a builtin constant",
                            def.name
                        ),
                        def.loc,
                    ));
                    continue;
                }
                if ordinary_symbols.contains(&def.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def name '{}' conflicts with existing symbol",
                            def.name
                        ),
                        def.loc,
                    ));
                    continue;
                }
                if !seen.insert(def.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate const symbol '{}'", def.name),
                        def.loc,
                    ));
                    continue;
                }
                if !artifacts.const_defs.contains_key(&def.name) {
                    record_const_def_artifact(&mut artifacts, def.clone(), options, errors);
                }
            }
            Block::Const(decl) => {
                if is_builtin_constant_name(&decl.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "constant name '{}' is reserved as a builtin constant",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                if ordinary_symbols.contains(&decl.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "constant name '{}' conflicts with existing symbol",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                if !seen.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate const symbol '{}'", decl.name),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                let inferred_const_array = decl.ty.is_none()
                    && is_known_const_array_initializer(
                        &decl.expr,
                        &artifacts.const_values,
                        &artifacts.const_array_infos,
                        &artifacts.const_defs,
                    );
                if artifacts.const_values.contains_key(&decl.name) {
                    // Registered by namespace flattening; keep its shared cache.
                } else if is_const_array_decl(decl) || inferred_const_array {
                    record_const_array_artifact(
                        decl,
                        &mut artifacts,
                        ConstContext::Declaration(options),
                        errors,
                    );
                } else {
                    record_const_scalar_artifact(decl, &mut artifacts, options, errors);
                }
            }
            Block::Ins(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "ins",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
            }
            Block::Outs(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "outs",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
            }
            Block::KOuts(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "kouts",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
            }
            Block::Params(params) => {
                let prefix = params.deferred_prefix.clone();
                let block_label = if prefix == "kin" { "kins" } else { "params" };
                expand_param_count_shorthand(
                    &mut params.decls,
                    &mut params.deferred_count,
                    &mut params.deferred_default_ty,
                    &prefix,
                    block_label,
                    params.loc,
                    &artifacts,
                    options,
                    errors,
                );
            }
            Block::Buffers(buffers) => {
                expand_buffer_count_shorthand(
                    &mut buffers.decls,
                    &mut buffers.deferred_count,
                    &mut buffers.deferred_default_ty,
                    "buffers",
                    buffers.loc,
                    &artifacts,
                    options,
                    errors,
                );
            }
            Block::Proc(proc) => {
                let proc_options = proc_options_for_count_expansion(proc, options);
                expand_proc_count_shorthand(proc, &artifacts, proc_options, errors);
            }
            _ => {}
        }
    }
    artifacts
}

pub(super) fn compile_const_descriptors(
    program: &Program,
    artifacts: &SemanticConstArtifacts,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CompileConstDescriptor> {
    program
        .blocks
        .iter()
        .filter_map(|block| {
            let Block::Const(decl) = block else {
                return None;
            };
            if !decl.configurable {
                return None;
            }
            let value = artifacts
                .const_values
                .resolve(&decl.name, errors)?
                .to_value();
            let kind = match (&decl.ty, &value) {
                (Some(ConstType::Scalar(_)), ConstValue::Scalar(_)) => CompileConstKind::Scalar,
                (Some(ConstType::Array { .. }), ConstValue::Array { .. }) => {
                    CompileConstKind::FixedArray
                }
                (Some(ConstType::Slice { .. }), ConstValue::Array { .. }) => {
                    CompileConstKind::Array
                }
                _ => return None,
            };
            Some(CompileConstDescriptor {
                name: decl.name.clone(),
                kind,
                value,
            })
        })
        .collect()
}
