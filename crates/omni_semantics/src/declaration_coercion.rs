use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn coerce_struct_fields(
    struct_name: &str,
    fields: &[StructField],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedStructField> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for field in fields {
        if !seen.insert(field.name.clone()) {
            errors.push(Diagnostic::semantic(
                format!(
                    "duplicate field '{}' in struct '{}'",
                    field.name, struct_name
                ),
                0,
                0,
            ));
            continue;
        }
        let (ty, default, array_elem_ty, array_elem_struct) = match &field.ty {
            FieldType::Scalar(prim) => {
                if let Some(expr) = &field.default {
                    validate_default_expr(
                        expr,
                        errors,
                        &format!("struct field '{}.{}'", struct_name, field.name),
                    );
                }
                (
                    TypedFieldType::Scalar(*prim),
                    field.default.clone(),
                    None,
                    None,
                )
            }
            FieldType::Generic(param) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "field '{}.{}' uses unresolved generic type '{}'",
                        struct_name, field.name, param
                    ),
                    0,
                    0,
                ));
                (
                    TypedFieldType::Scalar(PrimitiveType::F32),
                    field.default.clone(),
                    None,
                    None,
                )
            }
            FieldType::Array(spec) => {
                let size_context = format!("field '{}.{}' array size", struct_name, field.name);
                let size =
                    eval_data_size_expr(&spec.size, options, &size_context, errors).unwrap_or(1);
                if field.default.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "array field '{}.{}' cannot have a default expression",
                            struct_name, field.name
                        ),
                        0,
                        0,
                    ));
                }
                let (elem_ty, elem_struct) = match &spec.elem {
                    ArrayElemType::Primitive(prim) => (Some(*prim), None),
                    ArrayElemType::Struct(name) => (None, Some(name.clone())),
                };
                (TypedFieldType::Array(size), None, elem_ty, elem_struct)
            }
        };
        out.push(TypedStructField {
            name: field.name.clone(),
            ty,
            default,
            array_elem_ty,
            array_elem_struct,
        });
    }
    out
}
pub(crate) fn split_field_path<'a>(
    name: &'a str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(&'a str, &'a str)> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let second = parts.next();
    let third = parts.next();
    match (second, third) {
        (None, None) => None,
        (Some(f), None) => Some((first, f)),
        _ => {
            errors.push(Diagnostic::semantic(
                format!("unsupported nested path '{name}'"),
                0,
                0,
            ));
            None
        }
    }
}

pub(crate) fn coerce_params(
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<TypedParam>, HashMap<String, TypedArrayInfo>) {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut arrays = HashMap::new();
    for param in params {
        if is_builtin_constant_name(&param.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "param name '{}' is reserved as a builtin constant",
                    param.name
                ),
                0,
                0,
            ));
            continue;
        }
        if !seen.insert(param.name.as_str()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate param '{}'", param.name),
                0,
                0,
            ));
            continue;
        }
        match param.ty.as_ref() {
            None | Some(DeclType::Scalar(_)) => {
                let ty = match param.ty.as_ref() {
                    Some(DeclType::Scalar(ty)) => *ty,
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("param '{}.{}' default", "<top-level>", param.name),
                        is_float_type(ty),
                        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                        errors,
                    )
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("param '{}.{}'", "<top-level>", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                out.push(TypedParam {
                    name: param.name.clone(),
                    ty,
                    default,
                    range,
                });
            }
            Some(DeclType::Generic(param_ty)) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "param '{}.{}' uses unresolved generic type '{}'",
                        "<top-level>", param.name, param_ty
                    ),
                    0,
                    0,
                ));
                let ty = PrimitiveType::F32;
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("param '{}.{}' default", "<top-level>", param.name),
                        true,
                        false,
                        errors,
                    )
                    .unwrap_or(TypedConstValue::F32(0.0)),
                    None => TypedConstValue::F32(0.0),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("param '{}.{}'", "<top-level>", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                out.push(TypedParam {
                    name: param.name.clone(),
                    ty,
                    default,
                    range,
                });
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "param '{}.{}' range is not supported for array declarations",
                            "<top-level>", param.name
                        ),
                        0,
                        0,
                    ));
                }
                errors.push(Diagnostic::semantic(
                    format!(
                        "param '{}.{}' uses unresolved generic array element type '{}'",
                        "<top-level>", param.name, elem
                    ),
                    0,
                    0,
                ));
                let size_context = format!("param '{}.{}' array size", "<top-level>", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                arrays.insert(
                    param.name.clone(),
                    TypedArrayInfo {
                        elem_ty: PrimitiveType::F32,
                        len,
                        offset: out.len(),
                    },
                );

                let defaults = match &param.default {
                    None => vec![coerce_const_default_to_typed(0.0, PrimitiveType::F32); len],
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "param '{}.{}' default expects {len} elements, got {}",
                                    "<top-level>",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        let mut defaults = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values
                                .get(idx)
                                .and_then(|expr| {
                                    eval_typed_const_expr(
                                        expr,
                                        PrimitiveType::F32,
                                        options,
                                        &format!(
                                            "param '{}.{}' default element [{idx}]",
                                            "<top-level>", param.name
                                        ),
                                        true,
                                        false,
                                        errors,
                                    )
                                })
                                .unwrap_or(TypedConstValue::F32(0.0));
                            defaults.push(value);
                        }
                        defaults
                    }
                    Some(expr) => {
                        let value = eval_typed_const_expr(
                            expr,
                            PrimitiveType::F32,
                            options,
                            &format!("param '{}.{}' default", "<top-level>", param.name),
                            true,
                            false,
                            errors,
                        )
                        .unwrap_or(TypedConstValue::F32(0.0));
                        vec![value; len]
                    }
                };

                for (idx, default) in defaults.into_iter().enumerate() {
                    out.push(TypedParam {
                        name: format!("{}[{idx}]", param.name),
                        ty: PrimitiveType::F32,
                        default,
                        range: None,
                    });
                }
            }
            Some(DeclType::Array { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "param '{}.{}' range is not supported for array declarations",
                            "<top-level>", param.name
                        ),
                        0,
                        0,
                    ));
                }
                let size_context = format!("param '{}.{}' array size", "<top-level>", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                arrays.insert(
                    param.name.clone(),
                    TypedArrayInfo {
                        elem_ty: *elem,
                        len,
                        offset: out.len(),
                    },
                );

                let defaults = match &param.default {
                    None => vec![coerce_const_default_to_typed(0.0, *elem); len],
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "param '{}.{}' default expects {len} elements, got {}",
                                    "<top-level>",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        let mut defaults = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values.get(idx).and_then(|expr| {
                                eval_typed_const_expr(
                                    expr,
                                    *elem,
                                    options,
                                    &format!(
                                        "param '{}.{}' default element {idx}",
                                        "<top-level>", param.name
                                    ),
                                    is_float_type(*elem),
                                    matches!(*elem, PrimitiveType::I32 | PrimitiveType::I64),
                                    errors,
                                )
                            });
                            defaults.push(
                                value.unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem)),
                            );
                        }
                        defaults
                    }
                    Some(expr) => {
                        let value = eval_typed_const_expr(
                            expr,
                            *elem,
                            options,
                            &format!("param '{}.{}' default", "<top-level>", param.name),
                            is_float_type(*elem),
                            matches!(*elem, PrimitiveType::I32 | PrimitiveType::I64),
                            errors,
                        )
                        .unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem));
                        vec![value; len]
                    }
                };

                for (idx, default) in defaults.into_iter().enumerate() {
                    out.push(TypedParam {
                        name: format!("{}[{idx}]", param.name),
                        ty: *elem,
                        default,
                        range: None,
                    });
                }
            }
        }
    }
    (out, arrays)
}

pub(crate) fn coerce_buffers(
    buffers: &[BufferDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedBufferDecl> {
    let mut seen = HashSet::new();
    let mut out = Vec::<TypedBufferDecl>::new();

    for buffer in buffers {
        if !seen.insert(buffer.name.as_str()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate buffer '{}'", buffer.name),
                0,
                0,
            ));
            continue;
        }
        if is_builtin_constant_name(&buffer.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "buffer name '{}' is reserved as a builtin constant",
                    buffer.name
                ),
                0,
                0,
            ));
            continue;
        }

        let (elem_ty, channels) = match &buffer.ty {
            None => (PrimitiveType::F32, TypedBufferChannels::Mono),
            Some(spec) => {
                let elem_ty = match spec.elem {
                    BufferElemType::Primitive(ty) => ty,
                    BufferElemType::Generic(ref param_ty) => {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "buffer '{}' uses unresolved generic element type '{}'",
                                buffer.name, param_ty
                            ),
                            0,
                            0,
                        ));
                        PrimitiveType::F32
                    }
                };
                let channels = match &spec.channels {
                    BufferChannels::Mono => TypedBufferChannels::Mono,
                    BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                    BufferChannels::Static(expr) => {
                        let ctx = format!("buffer '{}' static channel count", buffer.name);
                        let Some(ch) = eval_data_size_expr(expr, options, &ctx, errors) else {
                            continue;
                        };
                        TypedBufferChannels::Static(ch)
                    }
                };
                (elem_ty, channels)
            }
        };
        out.push(TypedBufferDecl {
            name: buffer.name.clone(),
            elem_ty,
            channels,
        });
    }

    out
}
