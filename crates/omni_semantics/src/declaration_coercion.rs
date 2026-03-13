use super::*;
use omni_frontend::ArrayTypeSpec;

fn is_primitive_type_name(name: &str) -> bool {
    matches!(name, "f32" | "f64" | "i32" | "i64" | "bool")
}

fn reinterpret_scalar_specialized_struct_field(
    spec: &ArrayTypeSpec,
    type_param_set: &HashSet<String>,
) -> Option<String> {
    let ArrayElemType::Struct(base) = &spec.elem else {
        return None;
    };
    let Expr::Var { name: type_arg, .. } = spec.size.as_ref() else {
        return None;
    };
    if !type_param_set.contains(type_arg) && !is_primitive_type_name(type_arg.as_str()) {
        return None;
    }
    Some(format!("{base}<{type_arg}>"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn coerce_struct_fields(
    struct_name: &str,
    type_params: &[String],
    fields: &[StructField],
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedStructField> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let type_param_set = type_params.iter().cloned().collect::<HashSet<_>>();
    for field in fields {
        let field_loc = field.loc.as_ref();
        if !seen.insert(field.name.clone()) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "duplicate field '{}' in struct '{}'",
                    field.name, struct_name
                ),
                field_loc,
            ));
            continue;
        }
        let (ty, default, struct_name_ref, array_elem_ty, array_elem_struct) = match &field.ty {
            FieldType::Scalar(prim) => {
                if let Some(expr) = &field.default {
                    with_loc_diag_context(field_loc, |_diag| {
                        validate_default_expr(
                            expr,
                            errors,
                            &format!("struct field '{}.{}'", struct_name, field.name),
                        );
                    });
                }
                (
                    TypedFieldType::Scalar(*prim),
                    field.default.clone(),
                    None,
                    None,
                    None,
                )
            }
            FieldType::Generic(param) if type_param_set.contains(param) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "field '{}.{}' uses unresolved generic type '{}'",
                        struct_name, field.name, param
                    ),
                    field_loc,
                ));
                (
                    TypedFieldType::Scalar(PrimitiveType::F32),
                    field.default.clone(),
                    None,
                    None,
                    None,
                )
            }
            FieldType::Generic(nested_struct_name) => {
                let nested_fields = struct_defs.get(nested_struct_name).cloned();
                if nested_fields.is_none() && !nested_struct_name.contains('<') {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "field '{}.{}' references unknown struct '{}'",
                            struct_name, field.name, nested_struct_name
                        ),
                        field_loc,
                    ));
                    out.push(TypedStructField {
                        name: field.name.clone(),
                        ty: TypedFieldType::Scalar(PrimitiveType::F32),
                        default: field.default.clone(),
                        struct_name: None,
                        array_elem_ty: None,
                        array_elem_struct: None,
                    });
                    continue;
                }
                if field.default.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "struct field '{}.{}' cannot have a default expression",
                            struct_name, field.name
                        ),
                        field_loc,
                    ));
                }
                out.push(TypedStructField {
                    name: field.name.clone(),
                    ty: TypedFieldType::Struct,
                    default: None,
                    struct_name: Some(nested_struct_name.clone()),
                    array_elem_ty: None,
                    array_elem_struct: None,
                });
                if let Some(nested_fields) = nested_fields {
                    for nested in nested_fields {
                        out.push(TypedStructField {
                            name: format!("{}.{}", field.name, nested.name),
                            ty: nested.ty,
                            default: nested.default,
                            struct_name: nested.struct_name,
                            array_elem_ty: nested.array_elem_ty,
                            array_elem_struct: nested.array_elem_struct,
                        });
                    }
                }
                continue;
            }
            FieldType::Array(spec) => {
                if let Some(nested_struct_name) =
                    reinterpret_scalar_specialized_struct_field(spec, &type_param_set)
                {
                    let nested_fields = struct_defs.get(&nested_struct_name).cloned();
                    if nested_fields.is_none() && !nested_struct_name.contains('<') {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "field '{}.{}' references unknown struct '{}'",
                                struct_name, field.name, nested_struct_name
                            ),
                            field_loc,
                        ));
                        out.push(TypedStructField {
                            name: field.name.clone(),
                            ty: TypedFieldType::Scalar(PrimitiveType::F32),
                            default: field.default.clone(),
                            struct_name: None,
                            array_elem_ty: None,
                            array_elem_struct: None,
                        });
                        continue;
                    }
                    if field.default.is_some() {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "struct field '{}.{}' cannot have a default expression",
                                struct_name, field.name
                            ),
                            field_loc,
                        ));
                    }
                    out.push(TypedStructField {
                        name: field.name.clone(),
                        ty: TypedFieldType::Struct,
                        default: None,
                        struct_name: Some(nested_struct_name.clone()),
                        array_elem_ty: None,
                        array_elem_struct: None,
                    });
                    if let Some(nested_fields) = nested_fields {
                        for nested in nested_fields {
                            out.push(TypedStructField {
                                name: format!("{}.{}", field.name, nested.name),
                                ty: nested.ty,
                                default: nested.default,
                                struct_name: nested.struct_name,
                                array_elem_ty: nested.array_elem_ty,
                                array_elem_struct: nested.array_elem_struct,
                            });
                        }
                    }
                    continue;
                }
                let size_context = format!("field '{}.{}' array size", struct_name, field.name);
                let size = with_loc_diag_context(field_loc, |_diag| {
                    eval_data_size_expr(&spec.size, options, &size_context, errors)
                })
                .unwrap_or(1);
                if field.default.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "array field '{}.{}' cannot have a default expression",
                            struct_name, field.name
                        ),
                        field_loc,
                    ));
                }
                let (elem_ty, elem_struct) = match &spec.elem {
                    ArrayElemType::Primitive(prim) => (Some(*prim), None),
                    ArrayElemType::Struct(name) => (None, Some(name.clone())),
                };
                (
                    TypedFieldType::Array(size),
                    None,
                    None,
                    elem_ty,
                    elem_struct,
                )
            }
        };
        out.push(TypedStructField {
            name: field.name.clone(),
            ty,
            default,
            struct_name: struct_name_ref,
            array_elem_ty,
            array_elem_struct,
        });
    }
    out
}

pub(crate) fn coerce_struct_defs_for_inference(
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    options: AnalysisOptions,
) -> HashMap<String, Vec<TypedStructField>> {
    let mut typed = HashMap::<String, Vec<TypedStructField>>::new();
    let mut names = struct_defs.keys().cloned().collect::<Vec<_>>();
    names.sort();
    let mut sink = Vec::<Diagnostic>::new();

    for _ in 0..names.len().max(1) {
        for name in &names {
            let Some(def) = struct_defs.get(name) else {
                continue;
            };
            let fields = coerce_struct_fields(
                name,
                &def.type_params,
                &def.fields,
                &typed,
                options,
                &mut sink,
            );
            typed.insert(name.clone(), fields);
        }
    }

    typed
}

pub(crate) fn split_field_path<'a>(
    name: &'a str,
    _errors: &mut Vec<Diagnostic>,
) -> Option<(&'a str, &'a str)> {
    split_root_field_path(name)
}

pub(crate) fn split_root_field_path(name: &str) -> Option<(&str, &str)> {
    let (base, field) = name.split_once('.')?;
    if base.is_empty() || field.is_empty() {
        return None;
    }
    Some((base, field))
}

pub(crate) fn resolve_struct_field_decl<'a>(
    struct_name: &str,
    field_path: &str,
    struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
) -> Option<&'a TypedStructField> {
    let fields = struct_defs.get(struct_name)?;
    if let Some(field_decl) = fields.iter().find(|f| f.name == field_path) {
        return Some(field_decl);
    }

    let mut current_struct = struct_name;
    let mut parts = field_path.split('.').peekable();

    while let Some(part) = parts.next() {
        let fields = struct_defs.get(current_struct)?;
        let field_decl = fields.iter().find(|f| f.name == part)?;

        if parts.peek().is_none() {
            return Some(field_decl);
        }

        if field_decl.ty != TypedFieldType::Struct {
            return None;
        }
        current_struct = field_decl.struct_name.as_deref()?;
    }

    None
}

pub(crate) fn split_receiver_method_path(name: &str) -> Option<(&str, &str)> {
    let (receiver, method) = name.rsplit_once('.')?;
    if receiver.is_empty() || method.is_empty() {
        return None;
    }
    Some((receiver, method))
}

pub(crate) fn register_struct_instance_roots(
    base: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    struct_instances: &mut HashMap<String, String>,
) {
    struct_instances.insert(base.to_owned(), struct_name.to_owned());
    let Some(fields) = struct_defs.get(struct_name) else {
        return;
    };
    for field in fields {
        if field.ty != TypedFieldType::Struct {
            continue;
        }
        let Some(nested_struct_name) = &field.struct_name else {
            continue;
        };
        register_struct_instance_roots(
            &format!("{base}.{}", field.name),
            nested_struct_name,
            struct_defs,
            struct_instances,
        );
    }
}

pub(crate) fn register_struct_array_roots(
    base: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    struct_array_roots: &mut HashMap<String, String>,
) {
    struct_array_roots.insert(base.to_owned(), struct_name.to_owned());
    let Some(fields) = struct_defs.get(struct_name) else {
        return;
    };
    for field in fields {
        match field.ty {
            TypedFieldType::Struct => {
                if let Some(nested_struct_name) = &field.struct_name {
                    register_struct_array_roots(
                        &format!("{base}.{}", field.name),
                        nested_struct_name,
                        struct_defs,
                        struct_array_roots,
                    );
                }
            }
            TypedFieldType::Array(_) => {
                if let Some(elem_struct_name) = &field.array_elem_struct {
                    register_struct_array_roots(
                        &format!("{base}.{}", field.name),
                        elem_struct_name,
                        struct_defs,
                        struct_array_roots,
                    );
                }
            }
            TypedFieldType::Scalar(_) => {}
        }
    }
}

pub(crate) fn register_struct_instance_and_array_roots(
    base: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    struct_instances: &mut HashMap<String, String>,
    struct_array_roots: &mut HashMap<String, String>,
) {
    struct_instances.insert(base.to_owned(), struct_name.to_owned());
    let Some(fields) = struct_defs.get(struct_name) else {
        return;
    };
    for field in fields {
        match field.ty {
            TypedFieldType::Struct => {
                let Some(nested_struct_name) = &field.struct_name else {
                    continue;
                };
                register_struct_instance_and_array_roots(
                    &format!("{base}.{}", field.name),
                    nested_struct_name,
                    struct_defs,
                    struct_instances,
                    struct_array_roots,
                );
            }
            TypedFieldType::Array(_) => {
                let Some(elem_struct_name) = &field.array_elem_struct else {
                    continue;
                };
                register_struct_array_roots(
                    &format!("{base}.{}", field.name),
                    elem_struct_name,
                    struct_defs,
                    struct_array_roots,
                );
            }
            TypedFieldType::Scalar(_) => {}
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
        let param_loc = param.loc.as_ref();
        if is_builtin_constant_name(&param.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "param name '{}' is reserved as a builtin constant",
                    param.name
                ),
                param_loc,
            ));
            continue;
        }
        if !seen.insert(param.name.as_str()) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate param '{}'", param.name),
                param_loc,
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
                    Some(expr) => with_loc_diag_context(param_loc, |_diag| {
                        eval_typed_const_expr(
                            expr,
                            ty,
                            options,
                            &format!("param '{}.{}' default", "<top-level>", param.name),
                            is_float_type(ty),
                            matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                            errors,
                        )
                    })
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let range = with_loc_diag_context(param_loc, |_diag| {
                    param.range.as_ref().and_then(|r| {
                        eval_decl_range_for_type(
                            r,
                            ty,
                            options,
                            &format!("param '{}.{}'", "<top-level>", param.name),
                            errors,
                        )
                    })
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
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "param '{}.{}' uses unresolved generic type '{}'",
                        "<top-level>", param.name, param_ty
                    ),
                    param_loc,
                ));
                let ty = PrimitiveType::F32;
                let raw_default = match &param.default {
                    Some(expr) => with_loc_diag_context(param_loc, |_diag| {
                        eval_typed_const_expr(
                            expr,
                            ty,
                            options,
                            &format!("param '{}.{}' default", "<top-level>", param.name),
                            true,
                            false,
                            errors,
                        )
                    })
                    .unwrap_or(TypedConstValue::F32(0.0)),
                    None => TypedConstValue::F32(0.0),
                };
                let range = with_loc_diag_context(param_loc, |_diag| {
                    param.range.as_ref().and_then(|r| {
                        eval_decl_range_for_type(
                            r,
                            ty,
                            options,
                            &format!("param '{}.{}'", "<top-level>", param.name),
                            errors,
                        )
                    })
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
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "param '{}.{}' range is not supported for array declarations",
                            "<top-level>", param.name
                        ),
                        param_loc,
                    ));
                }
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "param '{}.{}' uses unresolved generic array element type '{}'",
                        "<top-level>", param.name, elem
                    ),
                    param_loc,
                ));
                let size_context = format!("param '{}.{}' array size", "<top-level>", param.name);
                let Some(len) = with_loc_diag_context(param_loc, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
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
                    Some(Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "param '{}.{}' default expects {len} elements, got {}",
                                    "<top-level>",
                                    param.name,
                                    values.len()
                                ),
                                param_loc,
                            ));
                        }
                        let mut defaults = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values
                                .get(idx)
                                .and_then(|expr| {
                                    with_loc_diag_context(param_loc, |_diag| {
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
                                })
                                .unwrap_or(TypedConstValue::F32(0.0));
                            defaults.push(value);
                        }
                        defaults
                    }
                    Some(expr) => {
                        let value = with_loc_diag_context(param_loc, |_diag| {
                            eval_typed_const_expr(
                                expr,
                                PrimitiveType::F32,
                                options,
                                &format!("param '{}.{}' default", "<top-level>", param.name),
                                true,
                                false,
                                errors,
                            )
                        })
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
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "param '{}.{}' range is not supported for array declarations",
                            "<top-level>", param.name
                        ),
                        param_loc,
                    ));
                }
                let size_context = format!("param '{}.{}' array size", "<top-level>", param.name);
                let Some(len) = with_loc_diag_context(param_loc, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
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
                    Some(Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "param '{}.{}' default expects {len} elements, got {}",
                                    "<top-level>",
                                    param.name,
                                    values.len()
                                ),
                                param_loc,
                            ));
                        }
                        let mut defaults = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values.get(idx).and_then(|expr| {
                                with_loc_diag_context(param_loc, |_diag| {
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
                                })
                            });
                            defaults.push(
                                value.unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem)),
                            );
                        }
                        defaults
                    }
                    Some(expr) => {
                        let value = with_loc_diag_context(param_loc, |_diag| {
                            eval_typed_const_expr(
                                expr,
                                *elem,
                                options,
                                &format!("param '{}.{}' default", "<top-level>", param.name),
                                is_float_type(*elem),
                                matches!(*elem, PrimitiveType::I32 | PrimitiveType::I64),
                                errors,
                            )
                        })
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
        let buffer_loc = buffer.loc.as_ref();
        if !seen.insert(buffer.name.as_str()) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate buffer '{}'", buffer.name),
                buffer_loc,
            ));
            continue;
        }
        if is_builtin_constant_name(&buffer.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "buffer name '{}' is reserved as a builtin constant",
                    buffer.name
                ),
                buffer_loc,
            ));
            continue;
        }

        let (elem_ty, channels) = match &buffer.ty {
            None => (PrimitiveType::F32, TypedBufferChannels::Mono),
            Some(spec) => {
                let elem_ty = match spec.elem {
                    BufferElemType::Primitive(ty) => ty,
                    BufferElemType::Generic(ref param_ty) => {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "buffer '{}' uses unresolved generic element type '{}'",
                                buffer.name, param_ty
                            ),
                            buffer_loc,
                        ));
                        PrimitiveType::F32
                    }
                };
                let channels = match &spec.channels {
                    BufferChannels::Mono => TypedBufferChannels::Mono,
                    BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                    BufferChannels::Static(expr) => {
                        let ctx = format!("buffer '{}' static channel count", buffer.name);
                        let Some(ch) = with_loc_diag_context(buffer_loc, |_diag| {
                            eval_data_size_expr(expr, options, &ctx, errors)
                        }) else {
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
