//! References retained by an uninstantiated namespace. Walk source metadata and
//! bodies without evaluating them; lexical resolution belongs to const_validation.
use super::*;

type Visitor<'a> = dyn FnMut(&str, SourceLoc) + 'a;

fn in_type_scope(types: &[String], visit: &mut Visitor<'_>, body: impl FnOnce(&mut Visitor<'_>)) {
    body(&mut |reference, loc| {
        if !types.iter().any(|bound| bound == reference) {
            visit(reference, loc);
        }
    });
}

fn name(name: &str, loc: SourceLoc, visit: &mut Visitor<'_>) {
    visit(name, loc);
    named_type(name, visit);
}

// A type name is not a value dependency. Only its namespace arguments can
// reference constant declarations.
fn named_type(name: &str, visit: &mut Visitor<'_>) {
    if looks_like_namespace_ref(name) {
        if let Ok(segments) = onda_frontend::parse_namespace_ref_text_ast(name) {
            segments_args(&segments, visit);
        }
    }
}

fn segments_args(segments: &[NamespaceRefSegment], visit: &mut Visitor<'_>) {
    for arg in segments
        .iter()
        .filter_map(|segment| segment.args.as_ref())
        .flatten()
    {
        visit_expression(&arg.expr, visit);
    }
}

fn node_references(node: &Expr, visit: &mut Visitor<'_>) {
    if let Some(reference) = referenced_const_name(node).or(match node {
        Expr::UserCall { name, .. } => Some(name.as_str()),
        _ => None,
    }) {
        name(reference, node.loc(), visit);
    }
    if let Expr::ArrayCtor { spec, .. } = node {
        element(&spec.elem, visit);
    }
    if let Expr::UserCall { type_args, .. } = node {
        for arg in type_args {
            if let CallTypeArg::Generic(reference) = arg {
                named_type(reference, visit);
            }
        }
    }
}

pub(super) fn visit_expression(expr: &Expr, visit: &mut Visitor<'_>) {
    for node in expr.walk() {
        node_references(node, visit);
    }
}

fn optional(expr: Option<&Expr>, visit: &mut Visitor<'_>) {
    if let Some(expr) = expr {
        visit_expression(expr, visit);
    }
}

fn element(elem: &ArrayElemType, visit: &mut Visitor<'_>) {
    if let ArrayElemType::Struct(reference) = elem {
        named_type(reference, visit);
    }
}

fn scalar(ty: &ScalarTypeRef, visit: &mut Visitor<'_>) {
    if let ScalarTypeRef::Named(reference) = ty {
        named_type(reference, visit);
    }
}

fn decl_type_names(ty: &DeclType, visit: &mut Visitor<'_>) {
    match ty {
        DeclType::Generic(reference)
        | DeclType::ArrayGeneric {
            elem: reference, ..
        } => named_type(reference, visit),
        DeclType::Slice(elem) => element(elem, visit),
        DeclType::Array { .. } | DeclType::Scalar(_) | DeclType::Tuple(_) => {}
    }
}

fn decl_type(ty: &DeclType, visit: &mut Visitor<'_>) {
    decl_type_names(ty, visit);
    if let DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. } = ty {
        visit_expression(size, visit);
    }
}

fn buffer_type(ty: &BufferType, visit: &mut Visitor<'_>) {
    if let BufferElemType::Generic(reference) = &ty.elem {
        named_type(reference, visit);
    }
    if let BufferChannels::Static(size) = &ty.channels {
        visit_expression(size, visit);
    }
}

fn fn_param_type(ty: &FnParamType, visit: &mut Visitor<'_>) {
    match ty {
        FnParamType::Struct(reference) | FnParamType::ArrayGeneric(reference) => {
            named_type(reference, visit)
        }
        FnParamType::SizedArray { generic_name, .. } => {
            if let Some(reference) = generic_name {
                named_type(reference, visit);
            }
        }
        FnParamType::Buffer(ty) | FnParamType::BufferArray { buffer: ty, .. } => {
            buffer_type(ty, visit)
        }
        FnParamType::Primitive(_)
        | FnParamType::Array(_)
        | FnParamType::BareBuffer
        | FnParamType::Tuple(_) => {}
    }
}

fn statement_types(body: &[Stmt], visit: &mut Visitor<'_>) {
    for stmt in body {
        stmt.visit_statements(|stmt| {
            if let Stmt::Assign {
                decl_ty,
                generic_decl_ty,
                ..
            } = stmt
            {
                if let Some(ty) = decl_ty {
                    decl_type_names(ty, visit);
                }
                if let Some(reference) = generic_decl_ty {
                    named_type(reference, visit);
                }
            }
        });
    }
}

fn statements(body: &[Stmt], bound: &mut HashSet<String>, visit: &mut Visitor<'_>) {
    statement_types(body, visit);
    visit_free_stmts(body, bound, &mut |node| node_references(node, visit));
}

fn function(def: &FunctionDef, visit: &mut Visitor<'_>) {
    in_type_scope(&def.type_params, visit, |visit| {
        for param in &def.params {
            if let Some(ty) = &param.ty {
                fn_param_type(ty, visit);
            }
        }
        if let Some(ty) = &def.return_ty {
            match ty {
                FnReturnType::Scalar(ty) => scalar(ty, visit),
                FnReturnType::Array { elem, .. } => scalar(elem, visit),
                FnReturnType::Tuple(types) => {
                    for ty in types {
                        scalar(ty, visit);
                    }
                }
            }
        }
        visit_const_def_references(def, &mut |node| node_references(node, visit));
        statement_types(&def.body, visit);
    });
}

fn event_params(params: &[EventParamDecl], visit: &mut Visitor<'_>) {
    for param in params {
        match &param.ty {
            EventParamType::GenericScalar { name: reference }
            | EventParamType::GenericSlice { elem: reference } => named_type(reference, visit),
            EventParamType::Array { size, .. } => visit_expression(size, visit),
            EventParamType::GenericArray { elem, size } => {
                named_type(elem, visit);
                visit_expression(size, visit);
            }
            EventParamType::Tuple(_) | EventParamType::Scalar(_) | EventParamType::Slice { .. } => {
            }
        }
        optional(param.default.as_ref(), visit);
    }
}

fn range(range: Option<&DeclRange>, visit: &mut Visitor<'_>) {
    if let Some(range) = range {
        optional(range.min.as_ref(), visit);
        visit_expression(&range.max, visit);
    }
}

fn processor(proc: &ProcessorDef, visit: &mut Visitor<'_>) {
    in_type_scope(&proc.type_params, visit, |visit| {
        for expr in [
            &proc.ins_deferred_count,
            &proc.outs_deferred_count,
            &proc.params_deferred_count,
            &proc.buffers_deferred_count,
            &proc.sample_oversample_factor,
        ] {
            optional(expr.as_ref(), visit);
        }
        for ty in [
            &proc.ins_deferred_default_ty,
            &proc.outs_deferred_default_ty,
            &proc.params_deferred_default_ty,
            &proc.init.default_ty,
        ]
        .into_iter()
        .flatten()
        {
            decl_type(ty, visit);
        }
        if let Some(ty) = &proc.buffers_deferred_default_ty {
            buffer_type(ty, visit);
        }
        for port in proc.ins.iter().chain(&proc.outs) {
            if let Some(ty) = &port.ty {
                decl_type(ty, visit);
            }
            optional(port.default.as_ref(), visit);
            range(port.range.as_ref(), visit);
        }
        for param in &proc.params {
            if let Some(ty) = &param.ty {
                decl_type(ty, visit);
            }
            optional(param.default.as_ref(), visit);
            range(param.range.as_ref(), visit);
            for expr in [
                &param.control.curve,
                &param.control.step,
                &param.control.smooth,
            ] {
                optional(expr.as_ref(), visit);
            }
        }
        for buffer in &proc.buffers {
            if let Some(ty) = &buffer.ty {
                buffer_type(ty, visit);
            }
            optional(buffer.array_size.as_ref(), visit);
        }
        let mut owner_bound = proc
            .ins
            .iter()
            .chain(&proc.outs)
            .map(|port| port.name.clone())
            .chain(proc.params.iter().map(|param| param.name.clone()))
            .chain(proc.buffers.iter().map(|buffer| buffer.name.clone()))
            .collect::<HashSet<_>>();
        statements(&proc.init.body, &mut owner_bound, visit);
        let mut block_bound = owner_bound.clone();
        statements(&proc.block_pre, &mut block_bound, visit);
        statements(&proc.sample, &mut block_bound.clone(), visit);
        statements(&proc.block_post, &mut block_bound, visit);
        for event in &proc.events {
            event_params(&event.params, visit);
            let mut bound = owner_bound.clone();
            bound.extend(event.params.iter().map(|param| param.name.clone()));
            statements(&event.body, &mut bound, visit);
        }
        for delegate in &proc.delegates {
            event_params(&delegate.params, visit);
        }
        for when in &proc.whens {
            optional(when.target.index.as_ref(), visit);
            let mut bound = owner_bound.clone();
            bound.extend(when.bindings.iter().map(|binding| binding.name.clone()));
            statements(&when.body, &mut bound, visit);
        }
        for task in &proc.tasks {
            statements(&task.body, &mut owner_bound.clone(), visit);
        }
        for def in &proc.local_defs {
            function(def, &mut |reference, loc| {
                if !owner_bound.contains(reference) {
                    visit(reference, loc);
                }
            });
        }
        if let Some(graph) = &proc.graph {
            for edge in &graph.edges {
                optional(edge.delay.as_ref(), visit);
                visit_free_exprs(&edge.source, &owner_bound, &mut |node| {
                    node_references(node, visit)
                });
                for endpoint in &edge.dests {
                    if let GraphEndpoint::ProcIndexedField { index, .. } = endpoint {
                        visit_expression(index, visit);
                    }
                }
            }
        }
    });
}

/// Nested namespaces own a fresh lexical scope and are traversed by the caller.
pub(super) fn visit_item(item: &NamespaceItem, visit: &mut Visitor<'_>) {
    match item {
        NamespaceItem::Assert(assertion) => visit_expression(&assertion.expr, visit),
        NamespaceItem::Const(decl) => {
            if let Some(ConstType::Array { size, .. }) = &decl.ty {
                visit_expression(size, visit);
            }
            visit_expression(&decl.expr, visit);
        }
        NamespaceItem::Def(def) => function(def, visit),
        NamespaceItem::Struct(def) => in_type_scope(&def.type_params, visit, |visit| {
            for field in &def.fields {
                match &field.ty {
                    FieldType::Generic(reference) => named_type(reference, visit),
                    FieldType::Array(spec) => {
                        element(&spec.elem, visit);
                        visit_expression(&spec.size, visit);
                    }
                    FieldType::Tuple(types) => {
                        for ty in types {
                            scalar(ty, visit);
                        }
                    }
                    FieldType::Scalar(_) => {}
                }
                optional(field.default.as_ref(), visit);
            }
            for method in &def.methods {
                function(method, visit);
            }
        }),
        NamespaceItem::Proc(proc) => processor(proc, visit),
        NamespaceItem::Alias(alias) => segments_args(&alias.target, visit),
        NamespaceItem::Use(use_decl) => {
            name(
                &namespace_segments_key(&use_decl.target),
                use_decl.loc.into(),
                visit,
            );
            segments_args(&use_decl.target, visit);
        }
        NamespaceItem::Namespace(_) => {}
    }
}
