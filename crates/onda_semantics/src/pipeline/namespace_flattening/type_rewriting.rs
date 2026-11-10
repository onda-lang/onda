use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_decl_type(
    ty: &mut DeclType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        DeclType::Generic(name) | DeclType::Slice(ArrayElemType::Struct(name)) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        DeclType::ArrayGeneric { elem, size } => {
            rewrite_named_type_ref_name(
                elem,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        DeclType::Array { size, .. } => {
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        DeclType::Tuple(types) => {
            rewrite_tuple_types(
                types,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &loc,
            );
        }
        DeclType::Slice(ArrayElemType::Primitive(_)) | DeclType::Scalar(_) => {}
    }
    enums::decl_type(ty, state.enum_scope());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_field_type(
    ty: &mut FieldType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FieldType::Generic(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FieldType::Array(spec) => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                &mut spec.size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FieldType::Tuple(types) => {
            rewrite_tuple_types(
                types,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &loc,
            );
        }
        FieldType::Scalar(_) => {}
    }
    enums::field_type(ty, state.enum_scope());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_fn_param_type(
    ty: &mut FnParamType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FnParamType::Struct(name) | FnParamType::ArrayGeneric(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnParamType::SizedArray {
            generic_name, size, ..
        } => {
            if let Some(name) = generic_name {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FnParamType::Buffer(buffer_ty)
        | FnParamType::BufferArray {
            buffer: buffer_ty, ..
        } => {
            rewrite_buffer_type(
                buffer_ty,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnParamType::Tuple(types) => {
            rewrite_tuple_types(
                types,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &loc,
            );
        }
        FnParamType::Primitive(_) | FnParamType::Array(_) | FnParamType::BareBuffer => {}
    }
    enums::param_type(ty, state.enum_scope());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_fn_return_type(
    ty: &mut FnReturnType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FnReturnType::Scalar(FnReturnScalarType::Named(name)) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnReturnType::Tuple(types) => {
            rewrite_tuple_types(
                types,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &loc,
            );
        }
        FnReturnType::Array { elem, size } => {
            if let FnReturnScalarType::Named(name) = elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FnReturnType::Scalar(FnReturnScalarType::Primitive(_)) => {}
    }
    enums::return_type(ty, state.enum_scope());
}

/// Tuple elements share name lookup and nominal scalar checking in every source position.
#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_tuple_types(
    types: &mut [ScalarTypeRef],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    for ty in types {
        if let ScalarTypeRef::Named(name) = ty {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &loc,
            );
        }
        enums::scalar_ref(ty, state.enum_scope());
        if let ScalarTypeRef::Named(name) = ty {
            if !state.type_params.contains(name) {
                errors.push(Diagnostic::semantic_span(
                    format!("tuple element type '{name}' must be a scalar type"),
                    &loc,
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_named_type_ref_name(
    name: &mut String,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_loc: impl Into<SourceLoc>,
) {
    if state.type_params.iter().any(|param| param == name) {
        return;
    }
    let use_site_loc = use_site_loc.into();
    let use_site_span = use_site_loc.span();
    if let Some((base, mut args)) =
        crate::generic_specialization::parse_array_struct_elem_with_type_args(name)
    {
        for arg in &mut args {
            if let CallTypeArg::Generic(name) = arg {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    &use_site_loc,
                );
                if let Some(ty) = (state.enum_scope())(name) {
                    *arg = CallTypeArg::Primitive(ty);
                }
            }
        }
        let args = args
            .iter()
            .map(|arg| match arg {
                CallTypeArg::Primitive(ty) => {
                    crate::generic_specialization::concrete_type_argument_name(*ty)
                }
                CallTypeArg::Generic(name) => name.as_str().into(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        *name = format!("{base}<{args}>");
    }
    if let Some(qualified) =
        resolve_visible_unqualified_member_name(name, current_ns, state, &use_site_loc, errors)
    {
        *name = qualified;
        return;
    }

    let (base, suffix) = split_named_type_base_and_suffix(name);
    if !suffix.is_empty() {
        if let Some(qualified) =
            resolve_visible_unqualified_member_name(base, current_ns, state, use_site_loc, errors)
        {
            *name = format!("{qualified}{suffix}");
            return;
        }
        if looks_like_namespace_ref(base) {
            if let Some(resolved) = resolve_namespace_symbol_name(
                base,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
            ) {
                *name = format!("{resolved}{suffix}");
            }
            return;
        }
    }

    if looks_like_namespace_ref(name) {
        if let Some(resolved) = resolve_namespace_symbol_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            use_site_span,
        ) {
            *name = resolved;
        }
    }
}
