use onda_frontend::{
    ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec, BufferBlock,
    BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp,
    ConstType, DeclType, EventDef, EventParamType, Expr, FieldType, FnParamType,
    FnReturnScalarType, FnReturnType, FunctionDef, GraphEndpoint, GraphRate, InitBlock, LogicalOp,
    ParamBlock, ParamDecl, PortBlock, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock,
    Stmt, StructDef,
};

pub(crate) fn primitive_type_name(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

pub(crate) fn format_program(program: &Program) -> String {
    let mut out = String::new();
    for block in program
        .blocks
        .iter()
        .filter(|block| !matches!(block, Block::Def(_)))
    {
        format_block(block, 0, &mut out);
        out.push('\n');
    }
    out
}

pub(crate) fn format_expr(expr: &Expr) -> String {
    format_expr_prec(expr, 0)
}

fn format_block(block: &Block, indent: usize, out: &mut String) {
    match block {
        Block::Ins(ports) => format_port_block("ins", ports, indent, out),
        Block::Outs(ports) => format_port_block("outs", ports, indent, out),
        Block::Params(params) => format_param_block("params", params, indent, out),
        Block::Const(decl) => {
            let mut text = format!("const {}", decl.name);
            if let Some(ty) = &decl.ty {
                text.push_str(": ");
                text.push_str(&format_const_type(ty));
            }
            text.push_str(" = ");
            text.push_str(&format_expr(&decl.expr));
            push_line(out, indent, &text);
        }
        Block::Events(events) => {
            push_line(out, indent, "events:");
            for event in events {
                format_event(event, indent + 1, out);
            }
        }
        Block::Buffers(buffers) => format_buffer_block("buffers", buffers, indent, out),
        Block::Assert(assert_decl) => {
            push_line(
                out,
                indent,
                &format!("assert({})", format_expr(&assert_decl.expr)),
            );
        }
        Block::Namespace(namespace) => format_namespace(namespace, indent, out),
        Block::NamespaceAlias(alias) => format_namespace_alias(alias, indent, out),
        Block::Proc(proc) => format_proc(proc, indent, out),
        Block::Struct(def) => format_struct(def, indent, out),
        Block::Def(def) => format_def(def, indent, out),
        Block::Init(init) => format_init_block("init", init, indent, out),
        Block::Block(exec) => format_block_exec(exec, indent, out),
        Block::Sample(sample) => format_sample_block("sample", sample, indent, out),
        Block::Graph(graph) => {
            push_line(out, indent, "graph:");
            for edge in &graph.edges {
                let mut text = String::new();
                if let Some(rate) = edge.rate {
                    text.push_str(match rate {
                        GraphRate::Block => "@block ",
                        GraphRate::Sample => "@sample ",
                    });
                }
                text.push_str(&format_expr(&edge.source));
                text.push_str(" >>");
                if let Some(delay) = &edge.delay {
                    text.push('[');
                    text.push_str(&format_expr(delay));
                    text.push(']');
                }
                text.push(' ');
                text.push_str(&format_graph_destinations(&edge.dests));
                push_line(out, indent + 1, &text);
            }
        }
    }
}

fn format_namespace(namespace: &onda_frontend::NamespaceDecl, indent: usize, out: &mut String) {
    let header = if namespace.params.is_empty() {
        format!("namespace {}:", namespace.name)
    } else {
        let params = namespace
            .params
            .iter()
            .map(|param| format!("{} = {}", param.name, format_expr(&param.default)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("namespace {}<{}>:", namespace.name, params)
    };
    push_line(out, indent, &header);
    if namespace.items.is_empty() {
        push_line(out, indent + 1, "pass");
        return;
    }
    for item in &namespace.items {
        format_namespace_item(item, indent + 1, out);
    }
}

fn format_namespace_item(item: &onda_frontend::NamespaceItem, indent: usize, out: &mut String) {
    match item {
        onda_frontend::NamespaceItem::Assert(assert_decl) => {
            push_line(
                out,
                indent,
                &format!("assert({})", format_expr(&assert_decl.expr)),
            );
        }
        onda_frontend::NamespaceItem::Const(decl) => {
            let mut text = format!("const {}", decl.name);
            if let Some(ty) = &decl.ty {
                text.push_str(": ");
                text.push_str(&format_const_type(ty));
            }
            text.push_str(" = ");
            text.push_str(&format_expr(&decl.expr));
            push_line(out, indent, &text);
        }
        onda_frontend::NamespaceItem::Struct(def) => format_struct(def, indent, out),
        onda_frontend::NamespaceItem::Def(def) => format_def(def, indent, out),
        onda_frontend::NamespaceItem::Proc(proc) => format_proc(proc, indent, out),
        onda_frontend::NamespaceItem::Namespace(namespace) => {
            format_namespace(namespace, indent, out)
        }
        onda_frontend::NamespaceItem::Alias(alias) => format_namespace_alias(alias, indent, out),
    }
}

fn format_namespace_alias(
    alias: &onda_frontend::NamespaceAliasDecl,
    indent: usize,
    out: &mut String,
) {
    push_line(
        out,
        indent,
        &format!(
            "namespace {} = {}",
            alias.name,
            format_namespace_ref(&alias.target)
        ),
    );
}

fn format_namespace_ref(segments: &[onda_frontend::NamespaceRefSegment]) -> String {
    segments
        .iter()
        .map(|segment| {
            let mut text = segment.name.clone();
            if let Some(args) = &segment.args {
                text.push('<');
                text.push_str(
                    &args
                        .iter()
                        .map(format_namespace_call_arg)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                text.push('>');
            }
            text
        })
        .collect::<Vec<_>>()
        .join("::")
}

fn format_namespace_call_arg(arg: &onda_frontend::NamespaceCallArg) -> String {
    if let Some(name) = &arg.name {
        format!("{name} = {}", format_expr(&arg.expr))
    } else {
        format_expr(&arg.expr)
    }
}

fn format_count_section_header(
    label: &str,
    count: Option<&Expr>,
    default_ty: Option<String>,
) -> String {
    let mut header = label.to_owned();
    if let Some(default_ty) = default_ty {
        header.push_str(&default_ty);
    }
    if let Some(count) = count {
        header.push(' ');
        header.push_str(&format_expr(count));
    }
    header
}

fn format_port_block(label: &str, ports: &PortBlock, indent: usize, out: &mut String) {
    format_port_section(
        label,
        &ports.decls,
        ports.deferred_count.as_ref(),
        ports
            .deferred_default_ty
            .as_ref()
            .map(|ty| format!("<{}>", format_decl_type(ty))),
        indent,
        out,
    );
}

fn format_port_section(
    label: &str,
    ports: &[PortDecl],
    deferred_count: Option<&Expr>,
    deferred_default_ty: Option<String>,
    indent: usize,
    out: &mut String,
) {
    let mut header = format_count_section_header(label, deferred_count, deferred_default_ty);
    if !ports.is_empty() || deferred_count.is_none() {
        header.push(':');
    }
    push_line(out, indent, &header);
    for port in ports {
        push_line(out, indent + 1, &format_port_decl(port));
    }
}

fn format_param_block(label: &str, params: &ParamBlock, indent: usize, out: &mut String) {
    format_param_section(
        label,
        &params.decls,
        params.deferred_count.as_ref(),
        params
            .deferred_default_ty
            .as_ref()
            .map(|ty| format!("<{}>", format_decl_type(ty))),
        indent,
        out,
    );
}

fn format_param_section(
    label: &str,
    params: &[ParamDecl],
    deferred_count: Option<&Expr>,
    deferred_default_ty: Option<String>,
    indent: usize,
    out: &mut String,
) {
    let mut header = format_count_section_header(label, deferred_count, deferred_default_ty);
    if !params.is_empty() || deferred_count.is_none() {
        header.push(':');
    }
    push_line(out, indent, &header);
    for param in params {
        push_line(out, indent + 1, &format_param_decl(param));
    }
}

fn format_buffer_section_default_type(ty: &BufferType) -> String {
    let elem = match &ty.elem {
        BufferElemType::Primitive(prim) => primitive_type_name(*prim).to_owned(),
        BufferElemType::Generic(name) => name.clone(),
    };
    format!("[{elem}]")
}

fn format_buffer_block(label: &str, buffers: &BufferBlock, indent: usize, out: &mut String) {
    format_buffer_section(
        label,
        &buffers.decls,
        buffers.deferred_count.as_ref(),
        buffers
            .deferred_default_ty
            .as_ref()
            .map(format_buffer_section_default_type),
        indent,
        out,
    );
}

fn format_buffer_section(
    label: &str,
    buffers: &[BufferDecl],
    deferred_count: Option<&Expr>,
    deferred_default_ty: Option<String>,
    indent: usize,
    out: &mut String,
) {
    let mut header = format_count_section_header(label, deferred_count, deferred_default_ty);
    if !buffers.is_empty() || deferred_count.is_none() {
        header.push(':');
    }
    push_line(out, indent, &header);
    for buffer in buffers {
        let mut text = buffer.name.clone();
        if let Some(ty) = &buffer.ty {
            text.push_str(": ");
            text.push_str(&format_buffer_type(ty));
        }
        push_line(out, indent + 1, &text);
    }
}

fn format_init_block(label: &str, init: &InitBlock, indent: usize, out: &mut String) {
    if let Some(default_ty) = &init.default_ty {
        push_line(
            out,
            indent,
            &format!("{label}<{}>:", format_decl_type(default_ty)),
        );
    } else {
        push_line(out, indent, &format!("{label}:"));
    }
    format_stmt_list(&init.body, indent + 1, out);
}

fn format_sample_block(label: &str, sample: &SampleBlock, indent: usize, out: &mut String) {
    let header = if let Some(factor) = &sample.oversample_factor {
        format!("{label} {}:", format_expr(factor))
    } else {
        format!("{label}:")
    };
    push_line(out, indent, &header);
    format_stmt_list(&sample.body, indent + 1, out);
}

fn format_block_exec(exec: &BlockExec, indent: usize, out: &mut String) {
    push_line(out, indent, "block:");
    if !exec.pre.is_empty() {
        push_line(out, indent + 1, "pre:");
        format_stmt_list(&exec.pre, indent + 2, out);
    }
    if let Some(sample) = &exec.sample {
        format_sample_block("sample", sample, indent + 1, out);
    }
    if !exec.post.is_empty() {
        push_line(out, indent + 1, "post:");
        format_stmt_list(&exec.post, indent + 2, out);
    }
}

fn format_proc(proc: &ProcessorDef, indent: usize, out: &mut String) {
    let header = if proc.type_params.is_empty() {
        format!("proc {}:", proc.name)
    } else {
        format!("proc {}<{}>:", proc.name, proc.type_params.join(", "))
    };
    push_line(out, indent, &header);
    if !proc.ins.is_empty() || proc.ins_deferred_count.is_some() {
        format_port_section(
            "ins",
            &proc.ins,
            proc.ins_deferred_count.as_ref(),
            proc.ins_deferred_default_ty
                .as_ref()
                .map(|ty| format!("<{}>", format_decl_type(ty))),
            indent + 1,
            out,
        );
    }
    if !proc.outs.is_empty() || proc.outs_deferred_count.is_some() {
        format_port_section(
            "outs",
            &proc.outs,
            proc.outs_deferred_count.as_ref(),
            proc.outs_deferred_default_ty
                .as_ref()
                .map(|ty| format!("<{}>", format_decl_type(ty))),
            indent + 1,
            out,
        );
    }
    if !proc.params.is_empty() || proc.params_deferred_count.is_some() {
        format_param_section(
            "params",
            &proc.params,
            proc.params_deferred_count.as_ref(),
            proc.params_deferred_default_ty
                .as_ref()
                .map(|ty| format!("<{}>", format_decl_type(ty))),
            indent + 1,
            out,
        );
    }
    if !proc.events.is_empty() {
        push_line(out, indent + 1, "events:");
        for event in &proc.events {
            format_event(event, indent + 2, out);
        }
    }
    if !proc.buffers.is_empty() || proc.buffers_deferred_count.is_some() {
        format_buffer_section(
            "buffers",
            &proc.buffers,
            proc.buffers_deferred_count.as_ref(),
            proc.buffers_deferred_default_ty
                .as_ref()
                .map(format_buffer_section_default_type),
            indent + 1,
            out,
        );
    }
    if proc.has_init_block || !proc.init.body.is_empty() {
        format_init_block("init", &proc.init, indent + 1, out);
    }
    if proc.has_block_block || !proc.block_pre.is_empty() || !proc.block_post.is_empty() {
        push_line(out, indent + 1, "block:");
        if !proc.block_pre.is_empty() {
            push_line(out, indent + 2, "pre:");
            format_stmt_list(&proc.block_pre, indent + 3, out);
        }
        if !proc.block_post.is_empty() {
            push_line(out, indent + 2, "post:");
            format_stmt_list(&proc.block_post, indent + 3, out);
        }
    }
    if proc.has_sample_block || !proc.sample.is_empty() {
        let header = if let Some(factor) = &proc.sample_oversample_factor {
            format!("sample {}:", format_expr(factor))
        } else {
            "sample:".to_owned()
        };
        push_line(out, indent + 1, &header);
        format_stmt_list(&proc.sample, indent + 2, out);
    }
    for def in &proc.local_defs {
        format_def(def, indent + 1, out);
    }
}

fn format_struct(def: &StructDef, indent: usize, out: &mut String) {
    let header = if def.type_params.is_empty() {
        format!("struct {}:", def.name)
    } else {
        format!("struct {}<{}>:", def.name, def.type_params.join(", "))
    };
    push_line(out, indent, &header);
    for field in &def.fields {
        let mut text = format!("{}: {}", field.name, format_field_type(&field.ty));
        if let Some(default) = &field.default {
            text.push_str(" = ");
            text.push_str(&format_expr(default));
        }
        push_line(out, indent + 1, &text);
    }
    for method in &def.methods {
        format_def(method, indent + 1, out);
    }
}

fn format_def(def: &FunctionDef, indent: usize, out: &mut String) {
    let prefix = if def.is_const { "const def" } else { "def" };
    let mut header = format!("{prefix} {}", def.name);
    if !def.type_params.is_empty() {
        header.push('<');
        header.push_str(&def.type_params.join(", "));
        header.push('>');
    }
    header.push('(');
    header.push_str(
        &def.params
            .iter()
            .map(|param| {
                let mut text = param.name.clone();
                if let Some(ty) = &param.ty {
                    text.push_str(": ");
                    text.push_str(&format_fn_param_type(ty));
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(&format_expr(default));
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", "),
    );
    header.push(')');
    if let Some(return_ty) = &def.return_ty {
        header.push_str(" -> ");
        header.push_str(&format_fn_return_type(return_ty));
    }
    header.push(':');
    push_line(out, indent, &header);
    format_stmt_list(&def.body, indent + 1, out);
}

fn format_event(event: &EventDef, indent: usize, out: &mut String) {
    let mut header = format!("{}(", event.name);
    header.push_str(
        &event
            .params
            .iter()
            .map(|param| {
                let mut text = format!("{}: {}", param.name, format_event_param_type(&param.ty));
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(&format_expr(default));
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", "),
    );
    header.push_str("):");
    push_line(out, indent, &header);
    format_stmt_list(&event.body, indent + 1, out);
}

fn format_stmt_list(stmts: &[Stmt], indent: usize, out: &mut String) {
    if stmts.is_empty() {
        push_line(out, indent, "pass");
        return;
    }
    for stmt in stmts {
        format_stmt(stmt, indent, out);
    }
}

fn format_stmt(stmt: &Stmt, indent: usize, out: &mut String) {
    match stmt {
        Stmt::Const { decl, .. } => {
            let mut text = format!("const {}", decl.name);
            if let Some(ty) = &decl.ty {
                text.push_str(": ");
                text.push_str(&format_const_type(ty));
            }
            text.push_str(" = ");
            text.push_str(&format_expr(&decl.expr));
            push_line(out, indent, &text);
        }
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            let lhs = format_assign_target(target);
            let mut text = lhs;
            if *is_typed_decl {
                if let Some(ty) = decl_ty {
                    text.push_str(": ");
                    text.push_str(primitive_type_name(*ty));
                } else if let Some(ty) = generic_decl_ty {
                    text.push_str(": ");
                    text.push_str(ty);
                }
            }
            text.push_str(" = ");
            text.push_str(&format_expr(expr));
            push_line(out, indent, &text);
        }
        Stmt::Expr { expr, .. } => push_line(out, indent, &format_expr(expr)),
        Stmt::Return { expr, .. } => {
            push_line(out, indent, &format!("return {}", format_expr(expr)))
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            push_line(out, indent, &format!("if {}:", format_expr(cond)));
            format_stmt_list(then_branch, indent + 1, out);
            if !else_branch.is_empty() {
                push_line(out, indent, "else:");
                format_stmt_list(else_branch, indent + 1, out);
            }
        }
        Stmt::For {
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
            ..
        } => {
            let mut text = format!("for {} in {}..", var, format_expr(start));
            if *end_inclusive {
                text.push('=');
            }
            text.push_str(&format_expr(end));
            if let Some(step) = step {
                text.push_str(" step ");
                text.push_str(&format_expr(step));
            }
            text.push(':');
            push_line(out, indent, &text);
            format_stmt_list(body, indent + 1, out);
        }
        Stmt::While { cond, body, .. } => {
            push_line(out, indent, &format!("while {}:", format_expr(cond)));
            format_stmt_list(body, indent + 1, out);
        }
        Stmt::Break { .. } => push_line(out, indent, "break"),
        Stmt::Continue { .. } => push_line(out, indent, "continue"),
    }
}

fn format_assign_target(target: &AssignTarget) -> String {
    match target {
        AssignTarget::Var(name) => name.clone(),
        AssignTarget::Index { base, index } => format!("{base}[{}]", format_expr(index)),
        AssignTarget::Slice { base, start, end } => format!(
            "{base}[{}:{}]",
            start
                .as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default(),
            end.as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default()
        ),
        AssignTarget::Tuple(names) => format!("({})", names.join(", ")),
    }
}

fn format_graph_endpoint(endpoint: &GraphEndpoint) -> String {
    match endpoint {
        GraphEndpoint::Symbol { name, .. } => name.clone(),
        GraphEndpoint::ProcField { proc, field, .. } => format!("{proc}.{field}"),
        GraphEndpoint::ProcIndexedField {
            proc, index, field, ..
        } => {
            format!("{proc}[{}].{field}", format_expr(index))
        }
    }
}

fn format_graph_destinations(dests: &[GraphEndpoint]) -> String {
    match dests {
        [] => String::new(),
        [dest] => format_graph_endpoint(dest),
        _ => format!(
            "{{ {} }}",
            dests
                .iter()
                .map(format_graph_endpoint)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_expr_prec(expr: &Expr, parent_prec: u8) -> String {
    let my_prec = expr_precedence(expr);
    match expr {
        Expr::Number { value, .. } => format_number(*value),
        Expr::Int { value, .. } => value.to_string(),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::ArrayLiteral { values, .. } => format!(
            "[{}]",
            values
                .iter()
                .map(format_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Var { name, .. } => name.clone(),
        Expr::Index { base, index, .. } => format!("{base}[{}]", format_expr(index)),
        Expr::Slice {
            base, start, end, ..
        } => format!(
            "{base}[{}:{}]",
            start
                .as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default(),
            end.as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default()
        ),
        Expr::ArrayCtor { spec, init, .. } => {
            let mut text = format!("{}(", format_array_type_spec(spec));
            if let Some(init) = init {
                text.push_str(&init.iter().map(format_expr).collect::<Vec<_>>().join(", "));
            }
            text.push(')');
            text
        }
        Expr::Compare { op, lhs, rhs, .. } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_cmp_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Call { func, args, .. } => format!(
            "{}({})",
            format_builtin_fn(*func),
            args.iter().map(format_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            let mut text = name.clone();
            if !type_args.is_empty() {
                text.push('<');
                text.push_str(
                    &type_args
                        .iter()
                        .map(format_call_type_arg)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                text.push('>');
            }
            text.push('(');
            text.push_str(
                &args
                    .iter()
                    .map(format_call_arg)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push(')');
            text
        }
        Expr::Cast { to, expr, .. } => {
            format!("{}({})", primitive_type_name(*to), format_expr(expr))
        }
        Expr::UnaryNot { expr, .. } => wrap_if_needed(
            format!("!{}", format_expr_prec(expr, my_prec)),
            my_prec,
            parent_prec,
        ),
        Expr::UnaryBitNot { expr, .. } => wrap_if_needed(
            format!("~{}", format_expr_prec(expr, my_prec)),
            my_prec,
            parent_prec,
        ),
        Expr::Logical { op, lhs, rhs, .. } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_logical_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Binary { op, lhs, rhs, .. } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_binary_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Tuple { values, .. } => {
            format!(
                "({})",
                values
                    .iter()
                    .map(format_expr)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn expr_precedence(expr: &Expr) -> u8 {
    match expr {
        Expr::Logical {
            op: LogicalOp::Or, ..
        } => 1,
        Expr::Logical {
            op: LogicalOp::And, ..
        } => 2,
        Expr::Binary {
            op: BinaryOp::BitOr,
            ..
        } => 3,
        Expr::Binary {
            op: BinaryOp::BitXor,
            ..
        } => 4,
        Expr::Binary {
            op: BinaryOp::BitAnd,
            ..
        } => 5,
        Expr::Compare { .. } => 6,
        Expr::Binary {
            op: BinaryOp::ShiftLeft | BinaryOp::ShiftRight,
            ..
        } => 7,
        Expr::Binary {
            op: BinaryOp::Add | BinaryOp::Sub,
            ..
        } => 8,
        Expr::Binary {
            op: BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod,
            ..
        } => 9,
        Expr::UnaryNot { .. } | Expr::UnaryBitNot { .. } => 10,
        _ => 11,
    }
}

fn wrap_if_needed(text: String, my_prec: u8, parent_prec: u8) -> String {
    if my_prec < parent_prec {
        format!("({text})")
    } else {
        text
    }
}

fn format_call_arg(arg: &CallArg) -> String {
    match &arg.name {
        Some(name) => format!("{name} = {}", format_expr(&arg.expr)),
        None => format_expr(&arg.expr),
    }
}

fn format_call_type_arg(arg: &CallTypeArg) -> String {
    match arg {
        CallTypeArg::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        CallTypeArg::Generic(name) => name.clone(),
    }
}

fn format_decl_type(ty: &DeclType) -> String {
    match ty {
        DeclType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        DeclType::Generic(name) => name.clone(),
        DeclType::ArrayGeneric { elem, size } => format!("{elem}[{}]", format_expr(size)),
        DeclType::Array { elem, size } => {
            format!("{}[{}]", primitive_type_name(*elem), format_expr(size))
        }
        DeclType::Tuple(elems) => format!(
            "({})",
            elems
                .iter()
                .map(|t| primitive_type_name(*t).to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_const_type(ty: &ConstType) -> String {
    match ty {
        ConstType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        ConstType::Array { elem, size } => {
            format!("{}[{}]", primitive_type_name(*elem), format_expr(size))
        }
        ConstType::Slice { elem } => format!("{}[]", primitive_type_name(*elem)),
    }
}

fn format_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        FieldType::Generic(name) => name.clone(),
        FieldType::Array(spec) => format_array_type_spec(spec),
        FieldType::Tuple(elem_tys) => {
            let elems: Vec<String> = elem_tys
                .iter()
                .map(|ty| primitive_type_name(*ty).to_owned())
                .collect();
            format!("({})", elems.join(", "))
        }
    }
}

fn format_array_type_spec(spec: &ArrayTypeSpec) -> String {
    let elem = match &spec.elem {
        ArrayElemType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        ArrayElemType::Struct(name) => name.clone(),
    };
    format!("{elem}[{}]", format_expr(spec.size.as_ref()))
}

fn format_fn_param_type(ty: &FnParamType) -> String {
    match ty {
        FnParamType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        FnParamType::Struct(name) => name.clone(),
        FnParamType::Buffer(ty) => format_buffer_type(ty),
        FnParamType::Array(Some(ty)) => format!("{}[]", primitive_type_name(*ty)),
        FnParamType::Array(None) => "[]".to_owned(),
        FnParamType::ArrayGeneric(name) => format!("{name}[]"),
        FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        } => {
            let type_str = if let Some(prim) = elem {
                primitive_type_name(*prim).to_owned()
            } else if let Some(g) = generic_name {
                g.clone()
            } else {
                "?".to_owned()
            };
            format!("{type_str}[{size:?}]")
        }
        FnParamType::BareBuffer => "buffer".to_owned(),
        FnParamType::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(|p| primitive_type_name(*p))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

fn format_fn_return_scalar_type(ty: &FnReturnScalarType) -> String {
    match ty {
        FnReturnScalarType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        FnReturnScalarType::Named(name) => name.clone(),
    }
}

fn format_fn_return_type(ty: &FnReturnType) -> String {
    match ty {
        FnReturnType::Scalar(ty) => format_fn_return_scalar_type(ty),
        FnReturnType::Array { elem, size } => {
            format!("{}[{}]", primitive_type_name(*elem), format_expr(size))
        }
        FnReturnType::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(format_fn_return_scalar_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

fn format_buffer_type(ty: &BufferType) -> String {
    let elem = match &ty.elem {
        BufferElemType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        BufferElemType::Generic(name) => name.clone(),
    };
    let channels = match &ty.channels {
        BufferChannels::Mono => String::new(),
        BufferChannels::Static(expr) => format!("[{}]", format_expr(expr)),
        BufferChannels::Dynamic => "[]".to_owned(),
    };
    format!("buffer[{elem}{channels}]")
}

fn format_event_param_type(ty: &EventParamType) -> String {
    match ty {
        EventParamType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        EventParamType::Array { elem, size } => {
            format!("{}[{}]", primitive_type_name(*elem), format_expr(size))
        }
        EventParamType::Slice { elem } => format!("{}[]", primitive_type_name(*elem)),
        EventParamType::GenericSlice { elem } => format!("{elem}[]"),
    }
}

fn format_port_decl(port: &PortDecl) -> String {
    let mut text = port.name.clone();
    if let Some(ty) = &port.ty {
        text.push_str(": ");
        text.push_str(&format_decl_type(ty));
    }
    if let Some(default) = &port.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    if let Some(range) = &port.range {
        text.push(' ');
        text.push('{');
        if let Some(min) = &range.min {
            text.push_str(&format_expr(min));
            text.push_str(", ");
        }
        text.push_str(&format_expr(&range.max));
        text.push('}');
    }
    text
}

fn format_param_decl(param: &ParamDecl) -> String {
    let mut text = param.name.clone();
    if let Some(ty) = &param.ty {
        text.push_str(": ");
        text.push_str(&format_decl_type(ty));
    }
    if let Some(default) = &param.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    if let Some(range) = &param.range {
        text.push(' ');
        text.push('{');
        if let Some(min) = &range.min {
            text.push_str(&format_expr(min));
            text.push_str(", ");
        }
        text.push_str(&format_expr(&range.max));
        text.push('}');
    }
    text
}

fn format_builtin_fn(func: BuiltinFn) -> &'static str {
    match func {
        BuiltinFn::Sin => "sin",
        BuiltinFn::Cos => "cos",
        BuiltinFn::Tan => "tan",
        BuiltinFn::Tanh => "tanh",
        BuiltinFn::Atan => "atan",
        BuiltinFn::Atan2 => "atan2",
        BuiltinFn::Exp => "exp",
        BuiltinFn::Log => "log",
        BuiltinFn::Sqrt => "sqrt",
        BuiltinFn::Pow => "pow",
        BuiltinFn::Abs => "abs",
        BuiltinFn::Floor => "floor",
        BuiltinFn::Ceil => "ceil",
        BuiltinFn::Round => "round",
        BuiltinFn::Trunc => "trunc",
        BuiltinFn::Min => "min",
        BuiltinFn::Max => "max",
        BuiltinFn::Fma => "fma",
    }
}

fn format_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight => ">>",
    }
}

fn format_logical_op(op: LogicalOp) -> &'static str {
    match op {
        LogicalOp::And => "&&",
        LogicalOp::Or => "||",
    }
}

fn format_cmp_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn push_line(out: &mut String, indent: usize, line: &str) {
    out.push_str(&"  ".repeat(indent));
    out.push_str(line);
    out.push('\n');
}
