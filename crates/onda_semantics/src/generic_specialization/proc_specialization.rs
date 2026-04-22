use super::*;
use onda_frontend::ast::{FnReturnScalarType, FnReturnType};

fn parse_explicit_proc_array_elem_type_args(
    name: &str,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, Vec<PrimitiveType>)> {
    let (base, suffix) = name.split_once('<')?;
    let args_raw = suffix.strip_suffix('>')?;
    let mut resolved = Vec::<PrimitiveType>::new();
    for raw in args_raw.split(',') {
        let ty = match raw.trim() {
            "f32" => PrimitiveType::F32,
            "f64" => PrimitiveType::F64,
            "i32" => PrimitiveType::I32,
            "i64" => PrimitiveType::I64,
            "bool" => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "{context}: 'bool' is not allowed as a generic type argument; only numeric types (f32, f64, i32, i64) are supported"
                    ),
                );
                return None;
            }
            other => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "{context}: generic type argument '{}' is not allowed here; expected concrete primitive type",
                        other
                    ),
                );
                return None;
            }
        };
        resolved.push(ty);
    }
    Some((base.to_owned(), resolved))
}

pub(crate) fn specialize_generic_proc_event_param_type(
    ty: &EventParamType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    event_name: &str,
    param_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> EventParamType {
    match ty {
        EventParamType::Scalar(prim) => EventParamType::Scalar(*prim),
        EventParamType::Array { elem, size } => EventParamType::Array {
            elem: *elem,
            size: size.clone(),
        },
        EventParamType::Slice { elem } => EventParamType::Slice { elem: *elem },
        EventParamType::GenericSlice { elem } => match type_bindings.get(elem).copied() {
            Some(bound) => EventParamType::Slice { elem: bound },
            None => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor '{}.{}' event parameter '{}' references unknown generic slice element type '{}'",
                        proc_name, event_name, param_name, elem
                    ),
                );
                EventParamType::Slice {
                    elem: PrimitiveType::F32,
                }
            }
        },
    }
}

pub(crate) fn specialize_generic_proc_decl_type(
    ty: &DeclType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    symbol_kind: &str,
    symbol_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> DeclType {
    match ty {
        DeclType::Scalar(prim) => DeclType::Scalar(*prim),
        DeclType::Generic(param) => match type_bindings.get(param).copied() {
            Some(bound) => DeclType::Scalar(bound),
            None => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor '{}' {} '{}' references unknown generic type parameter '{}'",
                        proc_name, symbol_kind, symbol_name, param
                    ),
                );
                DeclType::Scalar(PrimitiveType::F32)
            }
        },
        DeclType::ArrayGeneric { elem, size } => match type_bindings.get(elem).copied() {
            Some(bound) => DeclType::Array {
                elem: bound,
                size: size.clone(),
            },
            None => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor '{}' {} '{}' references unknown generic array element type '{}'",
                        proc_name, symbol_kind, symbol_name, elem
                    ),
                );
                DeclType::Array {
                    elem: PrimitiveType::F32,
                    size: size.clone(),
                }
            }
        },
        DeclType::Array { elem, size } => DeclType::Array {
            elem: *elem,
            size: size.clone(),
        },
        DeclType::Tuple(elems) => DeclType::Tuple(elems.clone()),
    }
}

pub(crate) fn specialize_generic_proc_buffer_type(
    ty: &BufferType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    buffer_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> BufferType {
    let elem = match &ty.elem {
        BufferElemType::Primitive(prim) => BufferElemType::Primitive(*prim),
        BufferElemType::Generic(param) => match type_bindings.get(param).copied() {
            Some(bound) => BufferElemType::Primitive(bound),
            None => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor '{}' buffer '{}' references unknown generic element type '{}'",
                        proc_name, buffer_name, param
                    ),
                );
                BufferElemType::Primitive(PrimitiveType::F32)
            }
        },
    };
    BufferType {
        elem,
        channels: ty.channels.clone(),
    }
}

pub(crate) fn rewrite_generic_array_ctor_expr_types(
    expr: &mut Expr,
    type_bindings: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) {
    let diag = DiagCtx::new(expr.loc());
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_array_ctor_expr_types(index, type_bindings, errors);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                rewrite_generic_array_ctor_expr_types(start, type_bindings, errors);
            }
            if let Some(end) = end {
                rewrite_generic_array_ctor_expr_types(end, type_bindings, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            if let ArrayElemType::Struct(elem_name) = &spec.elem {
                if let Some(bound) = type_bindings.get(elem_name).copied() {
                    spec.elem = ArrayElemType::Primitive(bound);
                } else if let Some(specialized) = specialize_named_type_ref(
                    elem_name,
                    type_bindings,
                    &format!("array element type '{elem_name}'"),
                    diag,
                    errors,
                ) {
                    match specialized {
                        FieldType::Scalar(bound) => spec.elem = ArrayElemType::Primitive(bound),
                        FieldType::Generic(name) => spec.elem = ArrayElemType::Struct(name),
                        FieldType::Array(_) | FieldType::Tuple(_) => {}
                    }
                }
            }
            rewrite_generic_array_ctor_expr_types(&mut spec.size, type_bindings, errors);
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_array_ctor_expr_types(value, type_bindings, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_array_ctor_expr_types(lhs, type_bindings, errors);
            rewrite_generic_array_ctor_expr_types(rhs, type_bindings, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_array_ctor_expr_types(arg, type_bindings, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_generic_array_ctor_expr_types(&mut arg.expr, type_bindings, errors);
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_generic_array_ctor_expr_types(inner, type_bindings, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_generic_array_ctor_expr_types(value, type_bindings, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(crate) fn rewrite_generic_array_ctor_stmt_types(
    stmt: &mut Stmt,
    type_bindings: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |_diag, stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_array_ctor_expr_types(index, type_bindings, errors);
            }
            rewrite_generic_array_ctor_expr_types(expr, type_bindings, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_array_ctor_expr_types(expr, type_bindings, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_array_ctor_expr_types(cond, type_bindings, errors);
            for nested in then_branch {
                rewrite_generic_array_ctor_stmt_types(nested, type_bindings, errors);
            }
            for nested in else_branch {
                rewrite_generic_array_ctor_stmt_types(nested, type_bindings, errors);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_generic_array_ctor_expr_types(start, type_bindings, errors);
            rewrite_generic_array_ctor_expr_types(end, type_bindings, errors);
            if let Some(step_expr) = step {
                rewrite_generic_array_ctor_expr_types(step_expr, type_bindings, errors);
            }
            for nested in body {
                rewrite_generic_array_ctor_stmt_types(nested, type_bindings, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_generic_array_ctor_expr_types(cond, type_bindings, errors);
            for nested in body {
                rewrite_generic_array_ctor_stmt_types(nested, type_bindings, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn specialize_generic_typed_decls(
    stmt: &mut Stmt,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |diag, stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            ..
        } => {
            let Some(param) = generic_decl_ty.clone() else {
                return;
            };
            let AssignTarget::Var(name) = target else {
                push_semantic(
                    diag,
                    errors,
                    "typed declaration is only supported for plain scalar variables",
                );
                *generic_decl_ty = None;
                return;
            };
            if decl_ty.is_some() {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor '{}' init declaration '{}: {}' cannot combine primitive and generic type annotations",
                        proc_name, name, param
                    ),
                );
                *generic_decl_ty = None;
                return;
            }
            match type_bindings.get(&param).copied() {
                Some(bound) => {
                    *decl_ty = Some(bound);
                    *generic_decl_ty = None;
                    *is_typed_decl = true;
                }
                None => {
                    push_semantic(
                        diag,
                        errors,
                        format!(
                            "processor '{}' init declaration '{}: {}' references unknown generic type parameter '{}'",
                            proc_name, name, param, param
                        ),
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
                specialize_generic_typed_decls(nested, type_bindings, proc_name, errors);
            }
            for nested in else_branch {
                specialize_generic_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                specialize_generic_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                specialize_generic_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn expand_inline_array_ctor_initializers(stmts: &mut Vec<Stmt>) {
    let mut expanded = Vec::<Stmt>::new();
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                expand_inline_array_ctor_initializers(then_branch);
                expand_inline_array_ctor_initializers(else_branch);
            }
            Stmt::For { body, .. } => {
                expand_inline_array_ctor_initializers(body);
            }
            Stmt::While { body, .. } => {
                expand_inline_array_ctor_initializers(body);
            }
            _ => {}
        }

        let mut index_writes = Vec::<Stmt>::new();
        if let Stmt::Assign {
            loc,
            target: AssignTarget::Var(base),
            expr: Expr::ArrayCtor { init, .. },
            ..
        } = &mut stmt
        {
            if let Some(values) = init.take() {
                for (idx, value) in values.into_iter().enumerate() {
                    index_writes.push(Stmt::Assign {
                        loc: loc.clone(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: base.clone(),
                            index: Expr::int(idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: value,
                    });
                }
            }
        }

        expanded.push(stmt);
        expanded.extend(index_writes);
    }
    *stmts = expanded;
}

pub(crate) fn specialize_generic_proc_template(
    template: &ProcessorDef,
    type_args: &[PrimitiveType],
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcessorDef> {
    let diag = DiagCtx::new(template.loc);
    if type_args.len() != template.type_params.len() {
        push_semantic(
            diag,
            errors,
            format!(
                "processor '{}' expects {} type arguments, got {}",
                template.name,
                template.type_params.len(),
                type_args.len()
            ),
        );
        return None;
    }

    let mut type_bindings = HashMap::<String, PrimitiveType>::new();
    for (param, ty) in template.type_params.iter().zip(type_args.iter()) {
        type_bindings.insert(param.clone(), *ty);
    }

    let mut ins = template
        .ins
        .iter()
        .map(|decl| PortDecl {
            loc: decl.loc.clone(),
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "input",
                    &decl.name,
                    DiagCtx::new(decl.ty_loc.or(decl.loc)),
                    errors,
                )
            }),
            ty_loc: decl.ty_loc.clone(),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut outs = template
        .outs
        .iter()
        .map(|decl| PortDecl {
            loc: decl.loc.clone(),
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "output",
                    &decl.name,
                    DiagCtx::new(decl.ty_loc.or(decl.loc)),
                    errors,
                )
            }),
            ty_loc: decl.ty_loc.clone(),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut params = template
        .params
        .iter()
        .map(|decl| ParamDecl {
            loc: decl.loc.clone(),
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "param",
                    &decl.name,
                    DiagCtx::new(decl.ty_loc.or(decl.loc)),
                    errors,
                )
            }),
            ty_loc: decl.ty_loc.clone(),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    for input in &mut ins {
        if let Some(default) = &mut input.default {
            rewrite_generic_array_ctor_expr_types(default, &type_bindings, errors);
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' input default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut input.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_array_ctor_expr_types(min, &type_bindings, errors);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' input range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_array_ctor_expr_types(&mut range.max, &type_bindings, errors);
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' input range maximum", template.name),
                errors,
            );
        }
    }
    for output in &mut outs {
        if let Some(range) = &mut output.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_array_ctor_expr_types(min, &type_bindings, errors);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' output range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_array_ctor_expr_types(&mut range.max, &type_bindings, errors);
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' output range maximum", template.name),
                errors,
            );
        }
    }
    for param in &mut params {
        if let Some(default) = &mut param.default {
            rewrite_generic_array_ctor_expr_types(default, &type_bindings, errors);
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' parameter default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut param.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_array_ctor_expr_types(min, &type_bindings, errors);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' parameter range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_array_ctor_expr_types(&mut range.max, &type_bindings, errors);
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' parameter range maximum", template.name),
                errors,
            );
        }
    }
    let buffers = template
        .buffers
        .iter()
        .map(|decl| BufferDecl {
            loc: decl.loc.clone(),
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_buffer_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    &decl.name,
                    DiagCtx::new(decl.ty_loc.or(decl.loc)),
                    errors,
                )
            }),
            ty_loc: decl.ty_loc.clone(),
        })
        .collect::<Vec<_>>();
    let mut init = template.init.clone();
    let mut block_pre = template.block_pre.clone();
    let mut sample = template.sample.clone();
    let mut block_post = template.block_post.clone();
    let mut local_defs = template.local_defs.clone();
    let mut events = template.events.clone();
    for event in &mut events {
        for param in &mut event.params {
            param.ty = specialize_generic_proc_event_param_type(
                &param.ty,
                &type_bindings,
                &template.name,
                &event.name,
                &param.name,
                DiagCtx::new(param.ty_loc.or(param.loc)),
                errors,
            );
            if let Some(default) = &mut param.default {
                rewrite_generic_array_ctor_expr_types(default, &type_bindings, errors);
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!(
                        "processor '{}.{}' event parameter default",
                        template.name, event.name
                    ),
                    errors,
                );
            }
        }
    }
    if let Some(init_default_ty) = init.default_ty.clone() {
        let specialized = specialize_generic_proc_decl_type(
            &init_default_ty,
            &type_bindings,
            &template.name,
            "init section default type",
            "init",
            DiagCtx::new(init.default_ty_loc.or(init.loc)),
            errors,
        );
        match specialized {
            DeclType::Scalar(_) | DeclType::Generic(_) => {
                init.default_ty = Some(specialized);
            }
            DeclType::Array { .. } | DeclType::ArrayGeneric { .. } | DeclType::Tuple(_) => {
                push_semantic(
                    DiagCtx::new(init.default_ty_loc.or(init.loc)),
                    errors,
                    format!(
                        "processor '{}' init section default type must be a scalar primitive or generic type",
                        template.name
                    ),
                );
                init.default_ty = None;
            }
        }
    }
    for stmt in &mut init.body {
        specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
    }
    for stmt in &mut init.body {
        rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings, errors);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' init", template.name),
            errors,
        );
    }
    for stmt in &mut block_pre {
        specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
    }
    for stmt in &mut block_pre {
        rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings, errors);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-pre", template.name),
            errors,
        );
    }
    for stmt in &mut sample {
        specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
    }
    for stmt in &mut sample {
        rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings, errors);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' sample", template.name),
            errors,
        );
    }
    for stmt in &mut block_post {
        specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
    }
    for stmt in &mut block_post {
        rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings, errors);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-post", template.name),
            errors,
        );
    }
    for event in &mut events {
        for stmt in &mut event.body {
            specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
        }
    }
    for event in &mut events {
        for stmt in &mut event.body {
            rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings, errors);
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("processor '{}' event '{}'", template.name, event.name),
                errors,
            );
        }
    }
    expand_inline_array_ctor_initializers(&mut init.body);
    expand_inline_array_ctor_initializers(&mut block_pre);
    expand_inline_array_ctor_initializers(&mut sample);
    expand_inline_array_ctor_initializers(&mut block_post);
    for event in &mut events {
        expand_inline_array_ctor_initializers(&mut event.body);
    }
    for def in &mut local_defs {
        let specialize_return_scalar = |ty: &FnReturnScalarType| -> FnReturnScalarType {
            match ty {
                FnReturnScalarType::Primitive(prim) => FnReturnScalarType::Primitive(*prim),
                FnReturnScalarType::Named(name) => match type_bindings.get(name).copied() {
                    Some(bound) => FnReturnScalarType::Primitive(bound),
                    None => FnReturnScalarType::Named(name.clone()),
                },
            }
        };
        for param in &mut def.params {
            if let Some(ty) = &mut param.ty {
                *ty = match ty {
                    FnParamType::Primitive(prim) => FnParamType::Primitive(*prim),
                    FnParamType::Struct(name) => match type_bindings.get(name).copied() {
                        Some(bound) => FnParamType::Primitive(bound),
                        None => FnParamType::Struct(name.clone()),
                    },
                    FnParamType::Buffer(buffer_ty) => {
                        let elem = match &buffer_ty.elem {
                            BufferElemType::Primitive(prim) => BufferElemType::Primitive(*prim),
                            BufferElemType::Generic(param) => {
                                match type_bindings.get(param).copied() {
                                    Some(bound) => BufferElemType::Primitive(bound),
                                    None => BufferElemType::Generic(param.clone()),
                                }
                            }
                        };
                        FnParamType::Buffer(BufferType {
                            elem,
                            channels: buffer_ty.channels.clone(),
                        })
                    }
                    FnParamType::Array(elem) => FnParamType::Array(*elem),
                    FnParamType::ArrayGeneric(name) => match type_bindings.get(name).copied() {
                        Some(bound) => FnParamType::Array(Some(bound)),
                        None => FnParamType::ArrayGeneric(name.clone()),
                    },
                    FnParamType::SizedArray {
                        elem,
                        generic_name,
                        size,
                    } => {
                        let resolved_elem = generic_name
                            .as_ref()
                            .and_then(|n| type_bindings.get(n).copied())
                            .or(*elem);
                        FnParamType::SizedArray {
                            elem: resolved_elem,
                            generic_name: if resolved_elem.is_some() {
                                None
                            } else {
                                generic_name.clone()
                            },
                            size: size.clone(),
                        }
                    }
                    FnParamType::BareBuffer => FnParamType::BareBuffer,
                    FnParamType::Tuple(elems) => FnParamType::Tuple(elems.clone()),
                };
            }
            if let Some(default) = &mut param.default {
                rewrite_generic_array_ctor_expr_types(default, &type_bindings, errors);
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!(
                        "processor '{}' local def '{}' parameter default",
                        template.name, def.name
                    ),
                    errors,
                );
            }
        }
        if let Some(return_ty) = &mut def.return_ty {
            *return_ty = match return_ty {
                FnReturnType::Scalar(scalar) => {
                    FnReturnType::Scalar(specialize_return_scalar(scalar))
                }
                FnReturnType::Tuple(elems) => FnReturnType::Tuple(
                    elems
                        .iter()
                        .map(|elem| specialize_return_scalar(elem))
                        .collect(),
                ),
            };
        }
        for stmt in &mut def.body {
            specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
        }
        for stmt in &mut def.body {
            rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings, errors);
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("processor '{}' local def '{}'", template.name, def.name),
                errors,
            );
        }
        expand_inline_array_ctor_initializers(&mut def.body);
    }

    Some(ProcessorDef {
        loc: template.loc.clone(),
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        consts: template.consts.clone(),
        ins,
        ins_deferred_count: None,
        ins_deferred_default_ty: None,
        outs,
        outs_deferred_count: None,
        outs_deferred_default_ty: None,
        params,
        params_deferred_count: None,
        params_deferred_default_ty: None,
        events,
        buffers,
        buffers_deferred_count: None,
        buffers_deferred_default_ty: None,
        has_init_block: template.has_init_block,
        has_block_block: template.has_block_block,
        has_sample_block: template.has_sample_block,
        has_graph_block: template.has_graph_block,
        sample_oversample_factor: template.sample_oversample_factor.clone(),
        init,
        block_pre,
        sample,
        block_post,
        graph: template.graph.clone(),
        local_defs,
    })
}

pub(crate) fn resolve_generic_proc_template_name(
    name: &str,
    current_ns: &str,
    templates: &HashMap<String, ProcessorDef>,
) -> Option<String> {
    if templates.contains_key(name) {
        return Some(name.to_owned());
    }
    if name.contains("::") {
        return None;
    }
    let symbols = templates.keys().cloned().collect::<HashSet<_>>();
    resolve_unqualified_symbol_name(name, current_ns, &symbols)
}

pub(crate) fn rewrite_generic_proc_ctor_expr(
    expr: &mut Expr,
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
    current_ns: &str,
) {
    let diag = DiagCtx::new(expr.loc());
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_proc_ctor_expr(index, templates, generated, errors, locals, current_ns);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                rewrite_generic_proc_ctor_expr(
                    start, templates, generated, errors, locals, current_ns,
                );
            }
            if let Some(end) = end {
                rewrite_generic_proc_ctor_expr(
                    end, templates, generated, errors, locals, current_ns,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_generic_proc_ctor_expr(
                &mut spec.size,
                templates,
                generated,
                errors,
                locals,
                current_ns,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_proc_ctor_expr(
                        value, templates, generated, errors, locals, current_ns,
                    );
                }
            }
            if let ArrayElemType::Struct(elem_name) = &mut spec.elem {
                if let Some((resolved_base, explicit_type_args)) =
                    parse_explicit_proc_array_elem_type_args(
                        elem_name,
                        "processor array element type",
                        diag,
                        errors,
                    )
                {
                    if let Some(template) = templates.get(&resolved_base) {
                        if let Some(specialized) =
                            specialize_generic_proc_template(template, &explicit_type_args, errors)
                        {
                            let specialized_name = specialized.name.clone();
                            generated
                                .entry(specialized_name.clone())
                                .or_insert(specialized);
                            *elem_name = specialized_name;
                            return;
                        }
                    }
                }

                if let Some(inferred_ctor) = init.as_ref().and_then(|values| {
                    let mut ctor_names = values.iter().filter_map(|value| match value {
                        Expr::UserCall { name, .. } => Some(name.as_str()),
                        _ => None,
                    });
                    let first = ctor_names.next()?;
                    ctor_names
                        .all(|name| name == first)
                        .then(|| first.to_owned())
                }) {
                    *elem_name = inferred_ctor;
                }

                let resolved_name = if templates.contains_key(elem_name) {
                    Some(elem_name.clone())
                } else if elem_name.contains("::") {
                    None
                } else {
                    resolve_generic_proc_template_name(elem_name, current_ns, templates)
                };
                if let Some(resolved_name) = resolved_name {
                    if *elem_name != resolved_name {
                        *elem_name = resolved_name.clone();
                    }
                    if let Some(template) = templates.get(elem_name) {
                        let type_args_to_use = infer_generic_proc_ctor_type_args(
                            template,
                            &[],
                            &locals.scalar_types,
                            &locals.array_elem_types,
                            locals.default_ctor_missing_type_params_to_f32,
                            diag,
                            errors,
                        );
                        if let Some(type_args_to_use) = type_args_to_use {
                            if let Some(specialized) = specialize_generic_proc_template(
                                template,
                                &type_args_to_use,
                                errors,
                            ) {
                                let specialized_name = specialized.name.clone();
                                generated
                                    .entry(specialized_name.clone())
                                    .or_insert(specialized);
                                *elem_name = specialized_name;
                            }
                        }
                    }
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_proc_ctor_expr(lhs, templates, generated, errors, locals, current_ns);
            rewrite_generic_proc_ctor_expr(rhs, templates, generated, errors, locals, current_ns);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_proc_ctor_expr(
                    arg, templates, generated, errors, locals, current_ns,
                );
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_generic_proc_ctor_expr(inner, templates, generated, errors, locals, current_ns);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_generic_proc_ctor_expr(
                    value, templates, generated, errors, locals, current_ns,
                );
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            for arg in args.iter_mut() {
                rewrite_generic_proc_ctor_expr(
                    &mut arg.expr,
                    templates,
                    generated,
                    errors,
                    locals,
                    current_ns,
                );
            }
            let resolved_name = if templates.contains_key(name) {
                Some(name.clone())
            } else if name.contains("::") {
                None
            } else {
                resolve_generic_proc_template_name(name, current_ns, templates)
            };
            if let Some(resolved_name) = resolved_name {
                if *name != resolved_name {
                    *name = resolved_name.clone();
                }
            }
            if let Some(template) = templates.get(name) {
                let type_args_to_use = if type_args.is_empty() {
                    infer_generic_proc_ctor_type_args(
                        template,
                        args,
                        &locals.scalar_types,
                        &locals.array_elem_types,
                        locals.default_ctor_missing_type_params_to_f32,
                        diag,
                        errors,
                    )
                } else {
                    resolve_explicit_call_type_args(
                        type_args,
                        &format!("processor constructor '{}'", name),
                        diag,
                        errors,
                    )
                };
                let Some(type_args_to_use) = type_args_to_use else {
                    return;
                };
                let Some(specialized) =
                    specialize_generic_proc_template(template, &type_args_to_use, errors)
                else {
                    return;
                };
                let specialized_name = specialized.name.clone();
                generated
                    .entry(specialized_name.clone())
                    .or_insert(specialized);
                *name = specialized_name;
                type_args.clear();
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(crate) fn rewrite_generic_proc_ctor_stmt(
    stmt: &mut Stmt,
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
    current_ns: &str,
) {
    with_stmt_diag_context_mut(stmt, |_diag, stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            let prior_default_mode = locals.default_ctor_missing_type_params_to_f32;
            let typed_named_ctor_decl_without_type_args =
                *is_typed_decl && decl_ty.is_none() && generic_decl_ty.is_none();
            if typed_named_ctor_decl_without_type_args {
                locals.default_ctor_missing_type_params_to_f32 = false;
            }
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_proc_ctor_expr(
                    index, templates, generated, errors, locals, current_ns,
                );
            }
            rewrite_generic_proc_ctor_expr(expr, templates, generated, errors, locals, current_ns);
            update_generic_inference_locals_from_assign(target, *decl_ty, expr, locals);
            locals.default_ctor_missing_type_params_to_f32 = prior_default_mode;
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_proc_ctor_expr(expr, templates, generated, errors, locals, current_ns);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_proc_ctor_expr(cond, templates, generated, errors, locals, current_ns);
            let mut then_locals = locals.clone();
            for nested in then_branch {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut then_locals,
                    current_ns,
                );
            }
            let mut else_locals = locals.clone();
            for nested in else_branch {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut else_locals,
                    current_ns,
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
            rewrite_generic_proc_ctor_expr(start, templates, generated, errors, locals, current_ns);
            rewrite_generic_proc_ctor_expr(end, templates, generated, errors, locals, current_ns);
            if let Some(step_expr) = step {
                rewrite_generic_proc_ctor_expr(
                    step_expr, templates, generated, errors, locals, current_ns,
                );
            }
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut body_locals,
                    current_ns,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_generic_proc_ctor_expr(cond, templates, generated, errors, locals, current_ns);
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut body_locals,
                    current_ns,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn rewrite_generic_proc_ctor_stmt_list(
    stmts: &mut [Stmt],
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
    seed_locals: &GenericInferenceLocals,
    current_ns: &str,
) {
    let mut locals = seed_locals.clone();
    for stmt in stmts {
        rewrite_generic_proc_ctor_stmt(stmt, templates, generated, errors, &mut locals, current_ns);
    }
}

pub(crate) fn finalize_generated_generic_proc_specializations(
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut processed = HashSet::<String>::new();
    loop {
        let names = generated.keys().cloned().collect::<Vec<_>>();
        let mut progressed = false;
        for name in names {
            if processed.contains(&name) {
                continue;
            }
            let Some(mut spec) = generated.remove(&name) else {
                continue;
            };
            let spec_ns = namespace_of_symbol(&spec.name);
            let spec_seed = generic_inference_seed_for_processor(&spec);
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.init,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.block_pre,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.sample,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.block_post,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            for event in &mut spec.events {
                rewrite_generic_proc_ctor_stmt_list(
                    &mut event.body,
                    templates,
                    generated,
                    errors,
                    &spec_seed,
                    &spec_ns,
                );
            }
            for def in &mut spec.local_defs {
                rewrite_generic_proc_ctor_stmt_list(
                    &mut def.body,
                    templates,
                    generated,
                    errors,
                    &spec_seed,
                    &spec_ns,
                );
            }
            generated.insert(name.clone(), spec);
            processed.insert(name);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}
