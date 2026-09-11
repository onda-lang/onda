use super::*;

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
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> EventParamType {
    match ty {
        EventParamType::GenericScalar { name } => {
            match specialize_generic_type_name(name, type_bindings, context, diag, errors) {
                Some(SpecializedTypeName::Primitive(ty)) => EventParamType::Scalar(ty),
                Some(SpecializedTypeName::Named(name)) => EventParamType::GenericScalar { name },
                None => ty.clone(),
            }
        }
        EventParamType::GenericArray { elem, size } => {
            match specialize_generic_type_name(elem, type_bindings, context, diag, errors) {
                Some(SpecializedTypeName::Primitive(elem)) => EventParamType::Array {
                    elem,
                    size: size.clone(),
                },
                Some(SpecializedTypeName::Named(elem)) => EventParamType::GenericArray {
                    elem,
                    size: size.clone(),
                },
                None => ty.clone(),
            }
        }
        EventParamType::GenericSlice { elem } => {
            match specialize_generic_type_name(elem, type_bindings, context, diag, errors) {
                Some(SpecializedTypeName::Primitive(elem)) => EventParamType::Slice { elem },
                Some(SpecializedTypeName::Named(elem)) => EventParamType::GenericSlice { elem },
                None => ty.clone(),
            }
        }
        _ => ty.clone(),
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
    specialize_decl_type(
        ty,
        type_bindings,
        &format!("processor '{proc_name}' {symbol_kind} '{symbol_name}'"),
        diag,
        errors,
    )
}

pub(crate) fn specialize_generic_proc_buffer_type(
    ty: &BufferType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    buffer_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> BufferType {
    let mut specialized = specialize_buffer_type(ty, type_bindings);
    if let BufferElemType::Generic(param) = &specialized.elem {
        push_semantic(
            diag,
            errors,
            format!(
                "processor '{}' buffer '{}' references unknown generic element type '{}'",
                proc_name, buffer_name, param
            ),
        );
        specialized.elem = BufferElemType::Primitive(PrimitiveType::F32);
    }
    specialized
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
                        loc: *loc,
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
            loc: decl.loc,
            name: decl.name.clone(),
            output_timing: decl.output_timing,
            output_timing_loc: decl.output_timing_loc,
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
            ty_loc: decl.ty_loc,
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut outs = template
        .outs
        .iter()
        .map(|decl| PortDecl {
            loc: decl.loc,
            name: decl.name.clone(),
            output_timing: decl.output_timing,
            output_timing_loc: decl.output_timing_loc,
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
            ty_loc: decl.ty_loc,
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut params = template
        .params
        .iter()
        .map(|decl| ParamDecl {
            loc: decl.loc,
            name: decl.name.clone(),
            private: decl.private,
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
            ty_loc: decl.ty_loc,
            default: decl.default.clone(),
            range: decl.range.clone(),
            control: decl.control.clone(),
            bind: decl.bind.clone(),
        })
        .collect::<Vec<_>>();
    for input in &mut ins {
        if let Some(default) = &mut input.default {
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' input default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut input.range {
            if let Some(min) = &mut range.min {
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' input range minimum", template.name),
                    errors,
                );
            }
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
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' output range minimum", template.name),
                    errors,
                );
            }
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
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' parameter default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut param.range {
            if let Some(min) = &mut range.min {
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' parameter range minimum", template.name),
                    errors,
                );
            }
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' parameter range maximum", template.name),
                errors,
            );
        }
        for (field, expr) in [
            ("curve", &mut param.control.curve),
            ("step", &mut param.control.step),
        ]
        .into_iter()
        .filter_map(|(field, expr)| expr.as_mut().map(|expr| (field, expr)))
        {
            substitute_call_type_args_with_bindings_expr(
                expr,
                &type_bindings,
                &format!("processor '{}' parameter {field}", template.name),
                errors,
            );
        }
    }
    let buffers = template
        .buffers
        .iter()
        .map(|decl| BufferDecl {
            loc: decl.loc,
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
            ty_loc: decl.ty_loc,
            array_size: decl.array_size.clone(),
        })
        .collect::<Vec<_>>();
    let mut init = template.init.clone();
    let mut block_pre = template.block_pre.clone();
    let mut sample = template.sample.clone();
    let mut block_post = template.block_post.clone();
    let mut local_defs = template.local_defs.clone();
    let mut events = template.events.clone();
    let mut delegates = template.delegates.clone();
    let mut whens = template.whens.clone();
    let mut tasks = template.tasks.clone();
    for event in &mut events {
        for param in &mut event.params {
            let param_context = format!(
                "processor '{}.{}' event parameter '{}'",
                template.name, event.name, param.name
            );
            param.ty = specialize_generic_proc_event_param_type(
                &param.ty,
                &type_bindings,
                &param_context,
                DiagCtx::new(param.ty_loc.or(param.loc)),
                errors,
            );
            if let Some(default) = &mut param.default {
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
    for delegate in &mut delegates {
        for param in &mut delegate.params {
            let param_context = format!(
                "processor '{}.{}' delegate parameter '{}'",
                template.name, delegate.name, param.name
            );
            param.ty = specialize_generic_proc_event_param_type(
                &param.ty,
                &type_bindings,
                &param_context,
                DiagCtx::new(param.ty_loc.or(param.loc)),
                errors,
            );
            if let Some(default) = &mut param.default {
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!(
                        "processor '{}.{}' delegate parameter default",
                        template.name, delegate.name
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
            DeclType::Slice(_)
            | DeclType::Array { .. }
            | DeclType::ArrayGeneric { .. }
            | DeclType::Tuple(_) => {
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
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' init", template.name),
            errors,
        );
    }
    for stmt in &mut block_pre {
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-pre", template.name),
            errors,
        );
    }
    for stmt in &mut sample {
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' sample", template.name),
            errors,
        );
    }
    for stmt in &mut block_post {
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-post", template.name),
            errors,
        );
    }
    for when in &mut whens {
        if let Some(index) = &mut when.target.index {
            substitute_call_type_args_with_bindings_expr(
                index,
                &type_bindings,
                &format!("processor '{}' when target", template.name),
                errors,
            );
        }
        for stmt in &mut when.body {
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("processor '{}' when handler", template.name),
                errors,
            );
        }
        expand_inline_array_ctor_initializers(&mut when.body);
    }
    for event in &mut events {
        for stmt in &mut event.body {
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
    for task in &mut tasks {
        for stmt in &mut task.body {
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("processor '{}' task '{}'", template.name, task.name),
                errors,
            );
        }
        expand_inline_array_ctor_initializers(&mut task.body);
    }
    for def in &mut local_defs {
        let def_context = format!("processor '{}' local def '{}'", template.name, def.name);
        specialize_function_type_annotations(def, &type_bindings, &def_context, errors);
        for param in &mut def.params {
            if let Some(default) = &mut param.default {
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!("{def_context} parameter default"),
                    errors,
                );
            }
        }
        for stmt in &mut def.body {
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &def_context,
                errors,
            );
        }
        expand_inline_array_ctor_initializers(&mut def.body);
    }

    Some(ProcessorDef {
        loc: template.loc,
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        consts: template.consts.clone(),
        ins,
        ins_deferred_count: None,
        ins_deferred_default_ty: None,
        outs,
        outs_deferred_count: None,
        outs_deferred_default_ty: None,
        outs_timing: template.outs_timing,
        outs_timing_loc: template.outs_timing_loc,
        params,
        params_deferred_count: None,
        params_deferred_default_ty: None,
        events,
        delegates,
        whens,
        tasks,
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
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_generic_proc_ctor_expr(
                    coordinate, templates, generated, errors, locals, current_ns,
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
            update_generic_inference_locals_from_assign(
                target,
                decl_ty.as_ref().and_then(DeclType::scalar),
                expr,
                locals,
            );
            locals.default_ctor_missing_type_params_to_f32 = prior_default_mode;
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_proc_ctor_expr(expr, templates, generated, errors, locals, current_ns);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                rewrite_generic_proc_ctor_expr(
                    value, templates, generated, errors, locals, current_ns,
                );
            }
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
