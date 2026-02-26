use super::*;

pub(crate) fn specialize_generic_proc_decl_type(
    ty: &DeclType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    symbol_kind: &str,
    symbol_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> DeclType {
    match ty {
        DeclType::Scalar(prim) => DeclType::Scalar(*prim),
        DeclType::Generic(param) => match type_bindings.get(param).copied() {
            Some(bound) => DeclType::Scalar(bound),
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' {} '{}' references unknown generic type parameter '{}'",
                        proc_name, symbol_kind, symbol_name, param
                    ),
                    0,
                    0,
                ));
                DeclType::Scalar(PrimitiveType::F32)
            }
        },
        DeclType::ArrayGeneric { elem, size } => match type_bindings.get(elem).copied() {
            Some(bound) => DeclType::Array {
                elem: bound,
                size: size.clone(),
            },
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' {} '{}' references unknown generic array element type '{}'",
                        proc_name, symbol_kind, symbol_name, elem
                    ),
                    0,
                    0,
                ));
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
    }
}

pub(crate) fn specialize_generic_proc_buffer_type(
    ty: &BufferType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    buffer_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> BufferType {
    let elem = match &ty.elem {
        BufferElemType::Primitive(prim) => BufferElemType::Primitive(*prim),
        BufferElemType::Generic(param) => match type_bindings.get(param).copied() {
            Some(bound) => BufferElemType::Primitive(bound),
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' buffer '{}' references unknown generic element type '{}'",
                        proc_name, buffer_name, param
                    ),
                    0,
                    0,
                ));
                BufferElemType::Primitive(PrimitiveType::F32)
            }
        },
    };
    BufferType {
        elem,
        channels: ty.channels.clone(),
    }
}

pub(crate) fn rewrite_generic_data_ctor_expr_types(
    expr: &mut Expr,
    type_bindings: &HashMap<String, PrimitiveType>,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_data_ctor_expr_types(index, type_bindings);
        }
        Expr::DataCtor { spec, init } => {
            if let DataElemType::Struct(param) = &spec.elem {
                if let Some(bound) = type_bindings.get(param).copied() {
                    spec.elem = DataElemType::Primitive(bound);
                }
            }
            rewrite_generic_data_ctor_expr_types(&mut spec.size, type_bindings);
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_data_ctor_expr_types(value, type_bindings);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_data_ctor_expr_types(lhs, type_bindings);
            rewrite_generic_data_ctor_expr_types(rhs, type_bindings);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_data_ctor_expr_types(arg, type_bindings);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_generic_data_ctor_expr_types(&mut arg.expr, type_bindings);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_generic_data_ctor_expr_types(inner, type_bindings);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_generic_data_ctor_expr_types(value, type_bindings);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(crate) fn rewrite_generic_data_ctor_stmt_types(
    stmt: &mut Stmt,
    type_bindings: &HashMap<String, PrimitiveType>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_data_ctor_expr_types(index, type_bindings);
            }
            rewrite_generic_data_ctor_expr_types(expr, type_bindings);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_data_ctor_expr_types(expr, type_bindings);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_data_ctor_expr_types(cond, type_bindings);
            for nested in then_branch {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
            for nested in else_branch {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_generic_data_ctor_expr_types(start, type_bindings);
            rewrite_generic_data_ctor_expr_types(end, type_bindings);
            if let Some(step_expr) = step {
                rewrite_generic_data_ctor_expr_types(step_expr, type_bindings);
            }
            for nested in body {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_generic_data_ctor_expr_types(cond, type_bindings);
            for nested in body {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn specialize_generic_init_typed_decls(
    stmt: &mut Stmt,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
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
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
                *generic_decl_ty = None;
                return;
            };
            if decl_ty.is_some() {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' init declaration '{}: {}' cannot combine primitive and generic type annotations",
                        proc_name, name, param
                    ),
                    0,
                    0,
                ));
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
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}' init declaration '{}: {}' references unknown generic type parameter '{}'",
                            proc_name, name, param, param
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
            for nested in else_branch {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn expand_inline_data_ctor_initializers(stmts: &mut Vec<Stmt>) {
    let mut expanded = Vec::<Stmt>::new();
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                expand_inline_data_ctor_initializers(then_branch);
                expand_inline_data_ctor_initializers(else_branch);
            }
            Stmt::For { body, .. } => {
                expand_inline_data_ctor_initializers(body);
            }
            Stmt::While { body, .. } => {
                expand_inline_data_ctor_initializers(body);
            }
            _ => {}
        }

        let mut index_writes = Vec::<Stmt>::new();
        if let Stmt::Assign {
            loc,
            target: AssignTarget::Var(base),
            expr: Expr::DataCtor { init, .. },
            ..
        } = &mut stmt
        {
            if let Some(values) = init.take() {
                for (idx, value) in values.into_iter().enumerate() {
                    index_writes.push(Stmt::Assign {
                        loc: loc.clone(),
                        target: AssignTarget::Index {
                            base: base.clone(),
                            index: Expr::Int(idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
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
    if type_args.len() != template.type_params.len() {
        errors.push(Diagnostic::semantic(
            format!(
                "processor '{}' expects {} type arguments, got {}",
                template.name,
                template.type_params.len(),
                type_args.len()
            ),
            0,
            0,
        ));
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
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "input",
                    &decl.name,
                    errors,
                )
            }),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut outs = template
        .outs
        .iter()
        .map(|decl| PortDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "output",
                    &decl.name,
                    errors,
                )
            }),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut params = template
        .params
        .iter()
        .map(|decl| ParamDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "param",
                    &decl.name,
                    errors,
                )
            }),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    for input in &mut ins {
        if let Some(default) = &mut input.default {
            rewrite_generic_data_ctor_expr_types(default, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' input default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut input.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_data_ctor_expr_types(min, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' input range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_data_ctor_expr_types(&mut range.max, &type_bindings);
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
                rewrite_generic_data_ctor_expr_types(min, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' output range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_data_ctor_expr_types(&mut range.max, &type_bindings);
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
            rewrite_generic_data_ctor_expr_types(default, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' parameter default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut param.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_data_ctor_expr_types(min, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' parameter range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_data_ctor_expr_types(&mut range.max, &type_bindings);
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
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_buffer_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    &decl.name,
                    errors,
                )
            }),
        })
        .collect::<Vec<_>>();
    let mut init = template.init.clone();
    let mut block_pre = template.block_pre.clone();
    let mut sample = template.sample.clone();
    let mut block_post = template.block_post.clone();
    let mut events = template.events.clone();
    for stmt in &mut init {
        specialize_generic_init_typed_decls(stmt, &type_bindings, &template.name, errors);
    }
    for stmt in &mut init {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' init", template.name),
            errors,
        );
    }
    for stmt in &mut block_pre {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-pre", template.name),
            errors,
        );
    }
    for stmt in &mut sample {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' sample", template.name),
            errors,
        );
    }
    for stmt in &mut block_post {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-post", template.name),
            errors,
        );
    }
    for event in &mut events {
        for stmt in &mut event.body {
            rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("processor '{}' event '{}'", template.name, event.name),
                errors,
            );
        }
    }
    expand_inline_data_ctor_initializers(&mut init);
    expand_inline_data_ctor_initializers(&mut block_pre);
    expand_inline_data_ctor_initializers(&mut sample);
    expand_inline_data_ctor_initializers(&mut block_post);
    for event in &mut events {
        expand_inline_data_ctor_initializers(&mut event.body);
    }

    Some(ProcessorDef {
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        ins,
        outs,
        params,
        events,
        buffers,
        has_init_block: template.has_init_block,
        has_block_block: template.has_block_block,
        has_sample_block: template.has_sample_block,
        sample_oversample_factor: template.sample_oversample_factor.clone(),
        init,
        block_pre,
        sample,
        block_post,
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
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_proc_ctor_expr(index, templates, generated, errors, locals, current_ns);
        }
        Expr::DataCtor { spec, init } => {
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
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_generic_proc_ctor_expr(inner, templates, generated, errors, locals, current_ns);
        }
        Expr::ArrayLiteral(values) => {
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
                        errors,
                    )
                } else {
                    resolve_explicit_call_type_args(
                        type_args,
                        &format!("processor constructor '{}'", name),
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
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
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
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            expr,
            ..
        } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_proc_ctor_expr(
                    index, templates, generated, errors, locals, current_ns,
                );
            }
            rewrite_generic_proc_ctor_expr(expr, templates, generated, errors, locals, current_ns);
            update_generic_inference_locals_from_assign(target, *decl_ty, expr, locals);
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
            generated.insert(name.clone(), spec);
            processed.insert(name);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}
