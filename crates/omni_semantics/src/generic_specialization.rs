use std::collections::{HashMap, HashSet};

use crate::*;

mod proc_specialization;
pub(crate) use proc_specialization::*;

pub(crate) fn primitive_sig_code_for_specialization(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

pub(crate) fn specialized_struct_name(base: &str, type_args: &[PrimitiveType]) -> String {
    if type_args.is_empty() {
        return base.to_owned();
    }
    let sig = type_args
        .iter()
        .map(|t| primitive_sig_code_for_specialization(*t))
        .collect::<Vec<_>>()
        .join("_");
    format!("{base}.__gen__{sig}")
}

pub(crate) fn resolve_explicit_call_type_args(
    type_args: &[CallTypeArg],
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut resolved = Vec::<PrimitiveType>::with_capacity(type_args.len());
    for arg in type_args {
        match arg {
            CallTypeArg::Primitive(ty) => {
                if *ty == PrimitiveType::Bool {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{context}: 'bool' is not allowed as a generic type argument; only numeric types (f32, f64, i32, i64) are supported"
                        ),
                        0,
                        0,
                    ));
                    return None;
                }
                resolved.push(*ty);
            }
            CallTypeArg::Generic(name) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: generic type argument '{}' is not allowed here; expected concrete primitive type",
                        name
                    ),
                    0,
                    0,
                ));
                return None;
            }
        }
    }
    Some(resolved)
}

pub(crate) fn substitute_call_type_args_with_bindings_expr(
    expr: &mut Expr,
    bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            substitute_call_type_args_with_bindings_expr(index, bindings, context, errors);
        }
        Expr::ArrayCtor { spec, init } => {
            substitute_call_type_args_with_bindings_expr(&mut spec.size, bindings, context, errors);
            if let Some(values) = init {
                for value in values {
                    substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            substitute_call_type_args_with_bindings_expr(lhs, bindings, context, errors);
            substitute_call_type_args_with_bindings_expr(rhs, bindings, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                substitute_call_type_args_with_bindings_expr(arg, bindings, context, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            substitute_call_type_args_with_bindings_expr(inner, bindings, context, errors);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
            }
        }
        Expr::UserCall {
            type_args, args, ..
        } => {
            for arg in args {
                substitute_call_type_args_with_bindings_expr(
                    &mut arg.expr,
                    bindings,
                    context,
                    errors,
                );
            }
            for type_arg in type_args.iter_mut() {
                if let CallTypeArg::Generic(param) = type_arg {
                    let Some(bound) = bindings.get(param).copied() else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{context}: unknown generic type argument '{}'; not declared in current generic owner",
                                param
                            ),
                            0,
                            0,
                        ));
                        continue;
                    };
                    *type_arg = CallTypeArg::Primitive(bound);
                }
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(crate) fn substitute_call_type_args_with_bindings_stmt(
    stmt: &mut Stmt,
    bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                substitute_call_type_args_with_bindings_expr(index, bindings, context, errors);
            }
            substitute_call_type_args_with_bindings_expr(expr, bindings, context, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            substitute_call_type_args_with_bindings_expr(expr, bindings, context, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            substitute_call_type_args_with_bindings_expr(cond, bindings, context, errors);
            for nested in then_branch {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
            for nested in else_branch {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            substitute_call_type_args_with_bindings_expr(start, bindings, context, errors);
            substitute_call_type_args_with_bindings_expr(end, bindings, context, errors);
            if let Some(step_expr) = step {
                substitute_call_type_args_with_bindings_expr(step_expr, bindings, context, errors);
            }
            for nested in body {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            substitute_call_type_args_with_bindings_expr(cond, bindings, context, errors);
            for nested in body {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn specialize_generic_struct_template(
    template: &StructDef,
    type_args: &[PrimitiveType],
    errors: &mut Vec<Diagnostic>,
) -> Option<StructDef> {
    if type_args.len() != template.type_params.len() {
        errors.push(Diagnostic::semantic(
            format!(
                "struct '{}' expects {} type arguments, got {}",
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

    let specialize_fn_param_type = |ty: &FnParamType| -> FnParamType {
        match ty {
            FnParamType::Primitive(prim) => FnParamType::Primitive(*prim),
            FnParamType::Struct(name) => match type_bindings.get(name).copied() {
                Some(bound) => FnParamType::Primitive(bound),
                None => FnParamType::Struct(name.clone()),
            },
            FnParamType::Buffer(buffer_ty) => {
                let elem = match &buffer_ty.elem {
                    BufferElemType::Primitive(prim) => BufferElemType::Primitive(*prim),
                    BufferElemType::Generic(param) => match type_bindings.get(param).copied() {
                        Some(bound) => BufferElemType::Primitive(bound),
                        None => BufferElemType::Generic(param.clone()),
                    },
                };
                FnParamType::Buffer(BufferType {
                    elem,
                    channels: buffer_ty.channels.clone(),
                })
            }
            FnParamType::Array(elem) => FnParamType::Array(*elem),
            FnParamType::BareBuffer => FnParamType::BareBuffer,
        }
    };

    let mut fields = Vec::<StructField>::new();
    for field in &template.fields {
        let mut default = field.default.clone();
        if let Some(expr) = &mut default {
            rewrite_generic_array_ctor_expr_types(expr, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                expr,
                &type_bindings,
                &format!("struct '{}'", template.name),
                errors,
            );
        }
        let specialized_ty = match &field.ty {
            FieldType::Scalar(prim) => FieldType::Scalar(*prim),
            FieldType::Generic(param) => {
                let bound = match type_bindings.get(param).copied() {
                    Some(bound) => bound,
                    None => {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct '{}.{}' references unknown generic type parameter '{}'",
                                template.name, field.name, param
                            ),
                            0,
                            0,
                        ));
                        PrimitiveType::F32
                    }
                };
                FieldType::Scalar(bound)
            }
            FieldType::Array(spec) => {
                let elem = match &spec.elem {
                    ArrayElemType::Primitive(prim) => ArrayElemType::Primitive(*prim),
                    ArrayElemType::Struct(elem) => match type_bindings.get(elem).copied() {
                        Some(bound) => ArrayElemType::Primitive(bound),
                        None => ArrayElemType::Struct(elem.clone()),
                    },
                };
                FieldType::Array(omni_frontend::ArrayTypeSpec {
                    elem,
                    size: spec.size.clone(),
                })
            }
        };
        fields.push(StructField {
            name: field.name.clone(),
            ty: specialized_ty,
            default,
        });
    }
    let mut methods = template.methods.clone();
    for method in &mut methods {
        for param in &mut method.params {
            if let Some(ty) = &param.ty {
                param.ty = Some(specialize_fn_param_type(ty));
            }
            if let Some(default) = &mut param.default {
                rewrite_generic_array_ctor_expr_types(default, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!(
                        "struct '{}.{}' parameter default",
                        template.name, method.name
                    ),
                    errors,
                );
            }
        }
        for stmt in &mut method.body {
            specialize_generic_typed_decls(stmt, &type_bindings, &template.name, errors);
        }
        for stmt in &mut method.body {
            rewrite_generic_array_ctor_stmt_types(stmt, &type_bindings);
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("struct '{}.{}' method body", template.name, method.name),
                errors,
            );
        }
    }

    Some(StructDef {
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        fields,
        methods,
    })
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GenericInferenceLocals {
    scalar_types: HashMap<String, PrimitiveType>,
    array_elem_types: HashMap<String, PrimitiveType>,
}

pub(crate) fn add_decl_type_to_generic_inference_locals(
    name: &str,
    ty: Option<&DeclType>,
    locals: &mut GenericInferenceLocals,
) {
    match ty {
        Some(DeclType::Scalar(prim)) => {
            locals.scalar_types.entry(name.to_owned()).or_insert(*prim);
        }
        Some(DeclType::Array { elem, .. }) => {
            locals
                .array_elem_types
                .entry(name.to_owned())
                .or_insert(*elem);
        }
        Some(DeclType::Generic(_)) | Some(DeclType::ArrayGeneric { .. }) => {}
        None => {
            locals
                .scalar_types
                .entry(name.to_owned())
                .or_insert(PrimitiveType::F32);
        }
    }
}

pub(crate) fn generic_inference_seed_for_processor(proc: &ProcessorDef) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::default();
    for input in &proc.ins {
        add_decl_type_to_generic_inference_locals(&input.name, input.ty.as_ref(), &mut locals);
    }
    for output in &proc.outs {
        add_decl_type_to_generic_inference_locals(&output.name, output.ty.as_ref(), &mut locals);
    }
    for param in &proc.params {
        add_decl_type_to_generic_inference_locals(&param.name, param.ty.as_ref(), &mut locals);
    }
    locals
}

pub(crate) fn generic_inference_seed_for_top_level(blocks: &[Block]) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::default();
    for block in blocks {
        match block {
            Block::Ins(ports) | Block::Outs(ports) => {
                for port in ports {
                    add_decl_type_to_generic_inference_locals(
                        &port.name,
                        port.ty.as_ref(),
                        &mut locals,
                    );
                }
            }
            Block::Params(params) => {
                for param in params {
                    add_decl_type_to_generic_inference_locals(
                        &param.name,
                        param.ty.as_ref(),
                        &mut locals,
                    );
                }
            }
            _ => {}
        }
    }
    locals
}

pub(crate) fn update_generic_inference_locals_from_assign(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    locals: &mut GenericInferenceLocals,
) {
    match target {
        AssignTarget::Var(name) => {
            if locals.scalar_types.contains_key(name) || locals.array_elem_types.contains_key(name)
            {
                return;
            }
            if let Some(declared) = decl_ty {
                locals.scalar_types.insert(name.clone(), declared);
                return;
            }
            if let Some(elem_ty) = infer_array_elem_type_for_generic_binding(
                expr,
                &locals.scalar_types,
                &locals.array_elem_types,
            ) {
                locals.array_elem_types.insert(name.clone(), elem_ty);
                return;
            }
            if let Some(scalar_ty) = infer_scalar_type_for_generic_binding(
                expr,
                &locals.scalar_types,
                &locals.array_elem_types,
            ) {
                locals.scalar_types.insert(name.clone(), scalar_ty);
            }
        }
        AssignTarget::Index { base, .. } => {
            if locals.array_elem_types.contains_key(base) {
                return;
            }
            if let Some(elem_ty) = infer_scalar_type_for_generic_binding(
                expr,
                &locals.scalar_types,
                &locals.array_elem_types,
            ) {
                locals.array_elem_types.insert(base.clone(), elem_ty);
            }
        }
    }
}

fn parse_primitive_type_arg_token(token: &str) -> Option<PrimitiveType> {
    match token {
        "f32" => Some(PrimitiveType::F32),
        "f64" => Some(PrimitiveType::F64),
        "i32" => Some(PrimitiveType::I32),
        "i64" => Some(PrimitiveType::I64),
        "bool" => Some(PrimitiveType::Bool),
        _ => None,
    }
}

fn is_simple_ident_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn split_trailing_bracket_group(text: &str) -> Option<(String, String)> {
    if !text.ends_with(']') {
        return None;
    }
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().rev() {
        match ch {
            ']' => depth += 1,
            '[' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    let base = text[..idx].trim().to_owned();
                    let args = text[idx + 1..text.len() - 1].trim().to_owned();
                    if base.is_empty() || args.is_empty() {
                        return None;
                    }
                    return Some((base, args));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_array_struct_elem_with_type_args(text: &str) -> Option<(String, Vec<CallTypeArg>)> {
    let (base, args_text) = split_trailing_bracket_group(text)?;
    let mut args = Vec::<CallTypeArg>::new();
    for raw in args_text.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            return None;
        }
        if let Some(prim) = parse_primitive_type_arg_token(token) {
            args.push(CallTypeArg::Primitive(prim));
        } else if is_simple_ident_token(token) {
            args.push(CallTypeArg::Generic(token.to_owned()));
        } else {
            return None;
        }
    }
    Some((base, args))
}

pub(crate) fn rewrite_generic_struct_ctor_expr(
    expr: &mut Expr,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_struct_ctor_expr(index, templates, generated, errors, locals);
        }
        Expr::ArrayCtor { spec, init } => {
            if let ArrayElemType::Struct(elem_name) = &mut spec.elem {
                let elem_text = elem_name.clone();
                let (template_lookup_name, explicit_type_args) =
                    match parse_array_struct_elem_with_type_args(&elem_text) {
                        Some((base, type_args)) => (base, Some(type_args)),
                        None => (elem_text.clone(), None),
                    };
                if let Some(template) = templates.get(template_lookup_name.as_str()) {
                    if !template.type_params.is_empty() {
                        let type_args_to_use = if let Some(type_args) = explicit_type_args {
                            let Some(resolved) = resolve_explicit_call_type_args(
                                &type_args,
                                &format!("array element type '{}'", elem_text),
                                errors,
                            ) else {
                                return;
                            };
                            if resolved.len() != template.type_params.len() {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "array element type '{}' expects {} type arguments, got {}",
                                        template_lookup_name.as_str(),
                                        template.type_params.len(),
                                        resolved.len()
                                    ),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            resolved
                        } else {
                            vec![PrimitiveType::F32; template.type_params.len()]
                        };
                        if let Some(specialized) =
                            specialize_generic_struct_template(template, &type_args_to_use, errors)
                        {
                            let specialized_name = specialized.name.clone();
                            generated
                                .entry(specialized_name.clone())
                                .or_insert(specialized);
                            *elem_name = specialized_name;
                        }
                    }
                }
            }
            rewrite_generic_struct_ctor_expr(&mut spec.size, templates, generated, errors, locals);
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_struct_ctor_expr(value, templates, generated, errors, locals);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_struct_ctor_expr(lhs, templates, generated, errors, locals);
            rewrite_generic_struct_ctor_expr(rhs, templates, generated, errors, locals);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_struct_ctor_expr(arg, templates, generated, errors, locals);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_generic_struct_ctor_expr(inner, templates, generated, errors, locals);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_generic_struct_ctor_expr(value, templates, generated, errors, locals);
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            for arg in args.iter_mut() {
                rewrite_generic_struct_ctor_expr(
                    &mut arg.expr,
                    templates,
                    generated,
                    errors,
                    locals,
                );
            }
            if let Some(template) = templates.get(name) {
                let type_args_to_use = if type_args.is_empty() {
                    infer_generic_struct_ctor_type_args(
                        template,
                        args,
                        &locals.scalar_types,
                        &locals.array_elem_types,
                        errors,
                    )
                } else {
                    resolve_explicit_call_type_args(
                        type_args,
                        &format!("struct constructor '{}'", name),
                        errors,
                    )
                };
                let Some(type_args_to_use) = type_args_to_use else {
                    return;
                };
                let Some(specialized) =
                    specialize_generic_struct_template(template, &type_args_to_use, errors)
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

pub(crate) fn rewrite_generic_struct_ctor_stmt(
    stmt: &mut Stmt,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            expr,
            ..
        } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_struct_ctor_expr(index, templates, generated, errors, locals);
            }
            rewrite_generic_struct_ctor_expr(expr, templates, generated, errors, locals);
            update_generic_inference_locals_from_assign(target, *decl_ty, expr, locals);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_struct_ctor_expr(expr, templates, generated, errors, locals);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_struct_ctor_expr(cond, templates, generated, errors, locals);
            let mut then_locals = locals.clone();
            for nested in then_branch {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut then_locals,
                );
            }
            let mut else_locals = locals.clone();
            for nested in else_branch {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut else_locals,
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
            rewrite_generic_struct_ctor_expr(start, templates, generated, errors, locals);
            rewrite_generic_struct_ctor_expr(end, templates, generated, errors, locals);
            if let Some(step_expr) = step {
                rewrite_generic_struct_ctor_expr(step_expr, templates, generated, errors, locals);
            }
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut body_locals,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_generic_struct_ctor_expr(cond, templates, generated, errors, locals);
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut body_locals,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn rewrite_generic_struct_ctor_stmt_list(
    stmts: &mut [Stmt],
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut locals = GenericInferenceLocals::default();
    for stmt in stmts {
        rewrite_generic_struct_ctor_stmt(stmt, templates, generated, errors, &mut locals);
    }
}

pub(crate) fn infer_scalar_type_for_generic_binding(
    expr: &Expr,
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    let mut locals = scalar_locals.clone();
    for (name, ty) in array_elem_locals {
        locals.entry(name.clone()).or_insert(*ty);
    }
    infer_expr_type_for_def_return_inference(expr, &locals, &HashMap::new())
}

pub(crate) fn infer_array_elem_type_for_generic_binding(
    expr: &Expr,
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::ArrayLiteral(values) => {
            let mut acc = None::<PrimitiveType>;
            for value in values {
                let ty =
                    infer_scalar_type_for_generic_binding(value, scalar_locals, array_elem_locals)?;
                acc = Some(match acc {
                    Some(existing) => merge_inferred_return_types(existing, ty)?,
                    None => ty,
                });
            }
            acc
        }
        Expr::Var(name) => array_elem_locals.get(name).copied(),
        Expr::ArrayCtor { spec, .. } => match &spec.elem {
            ArrayElemType::Primitive(ty) => Some(*ty),
            ArrayElemType::Struct(_) => None,
        },
        _ => None,
    }
}

pub(crate) fn bind_inferred_generic_type(
    bindings: &mut HashMap<String, PrimitiveType>,
    type_param: &str,
    inferred: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(existing) = bindings.get(type_param).copied() {
        if existing == inferred {
            return;
        }
        if let Some(merged) = merge_inferred_return_types(existing, inferred) {
            bindings.insert(type_param.to_owned(), merged);
        } else {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: conflicting inferred types {:?} and {:?} for generic parameter '{}'",
                    existing, inferred, type_param
                ),
                0,
                0,
            ));
        }
    } else {
        bindings.insert(type_param.to_owned(), inferred);
    }
}

pub(crate) fn finalize_inferred_generic_type_args(
    owner_name: &str,
    owner_kind: &str,
    type_params: &[String],
    bindings: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut out = Vec::<PrimitiveType>::with_capacity(type_params.len());
    for param in type_params {
        let Some(bound) = bindings.get(param).copied() else {
            errors.push(Diagnostic::semantic(
                format!(
                    "cannot infer generic type parameter '{}' for {} '{}' constructor; provide explicit type arguments",
                    param, owner_kind, owner_name
                ),
                0,
                0,
            ));
            return None;
        };
        out.push(bound);
    }
    Some(out)
}

pub(crate) fn infer_generic_struct_ctor_type_args(
    template: &StructDef,
    args: &[CallArg],
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let scalar_fields = template
        .fields
        .iter()
        .filter(|f| !matches!(f.ty, FieldType::Array(_)))
        .collect::<Vec<_>>();
    let param_names = scalar_fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let defaults = scalar_fields
        .iter()
        .map(|f| f.default.clone().or(Some(Expr::Number(0.0))))
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        args,
        &param_names,
        &defaults,
        false,
        false,
        &format!("struct constructor '{}'", template.name),
        errors,
    );

    let mut bindings = HashMap::<String, PrimitiveType>::new();
    for (idx, field) in scalar_fields.iter().enumerate() {
        let Some(expr) = resolved.get(idx).and_then(|arg| *arg).or_else(|| {
            defaults
                .get(idx)
                .and_then(|default_expr| default_expr.as_ref())
        }) else {
            continue;
        };
        if let FieldType::Generic(type_param) = &field.ty {
            if let Some(inferred) =
                infer_scalar_type_for_generic_binding(expr, scalar_locals, array_elem_locals)
            {
                bind_inferred_generic_type(
                    &mut bindings,
                    type_param,
                    inferred,
                    &format!("struct constructor '{}'", template.name),
                    errors,
                );
            }
        }
    }
    finalize_inferred_generic_type_args(
        &template.name,
        "struct",
        &template.type_params,
        &bindings,
        errors,
    )
}

pub(crate) fn infer_generic_proc_ctor_type_args(
    template: &ProcessorDef,
    args: &[CallArg],
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut ctor_param_names = template
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    ctor_param_names.extend(template.buffers.iter().map(|b| b.name.clone()));
    let mut ctor_defaults = template
        .params
        .iter()
        .map(|p| p.default.clone())
        .collect::<Vec<_>>();
    ctor_defaults.extend((0..template.buffers.len()).map(|_| None));

    let resolved = resolve_call_args(
        args,
        &ctor_param_names,
        &ctor_defaults,
        false,
        false,
        &format!("processor constructor '{}'", template.name),
        errors,
    );

    let mut bindings = HashMap::<String, PrimitiveType>::new();
    for (idx, param) in template.params.iter().enumerate() {
        let Some(expr) = resolved.get(idx).and_then(|arg| *arg).or_else(|| {
            ctor_defaults
                .get(idx)
                .and_then(|default_expr| default_expr.as_ref())
        }) else {
            continue;
        };
        if let Some(param_ty) = &param.ty {
            match param_ty {
                DeclType::Generic(type_param) => {
                    if let Some(inferred) = infer_scalar_type_for_generic_binding(
                        expr,
                        scalar_locals,
                        array_elem_locals,
                    ) {
                        bind_inferred_generic_type(
                            &mut bindings,
                            type_param,
                            inferred,
                            &format!("processor constructor '{}'", template.name),
                            errors,
                        );
                    }
                }
                DeclType::ArrayGeneric { elem, .. } => {
                    if let Some(inferred) = infer_array_elem_type_for_generic_binding(
                        expr,
                        scalar_locals,
                        array_elem_locals,
                    ) {
                        bind_inferred_generic_type(
                            &mut bindings,
                            elem,
                            inferred,
                            &format!("processor constructor '{}'", template.name),
                            errors,
                        );
                    }
                }
                DeclType::Scalar(_) | DeclType::Array { .. } => {}
            }
        }
    }
    finalize_inferred_generic_type_args(
        &template.name,
        "processor",
        &template.type_params,
        &bindings,
        errors,
    )
}

pub(crate) fn finalize_generated_generic_struct_specializations(
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
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
            for field in &mut spec.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = GenericInferenceLocals::default();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        templates,
                        generated,
                        errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut spec.methods {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    templates,
                    generated,
                    errors,
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
