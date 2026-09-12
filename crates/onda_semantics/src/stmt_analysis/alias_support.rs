use super::*;

pub(crate) fn merged_data_vars(
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
) -> HashMap<String, usize> {
    let mut merged = state_arrays.clone();
    for (name, alias) in local_array_aliases {
        if alias.elem_struct.is_none() {
            merged.insert(name.clone(), alias.len);
        }
    }
    merged
}

pub(crate) fn seed_top_level_array_aliases(
    aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    arrays: &HashMap<String, TypedArrayInfo>,
    writable: bool,
) {
    for (name, info) in arrays {
        aliases.insert(
            name.clone(),
            LocalArrayAliasInfo {
                proven_len: None,
                len: info.len,
                static_len: Some(info.len),
                elem_ty: info.elem_ty,
                elem_struct: None,
                writable,
            },
        );
    }
}

fn resolve_executable_array_source(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    owner_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
    preserve_static_len: bool,
    diagnose_non_array_field: bool,
) -> Option<LocalArrayAliasInfo> {
    if let Some(alias) = local_array_aliases.get(base) {
        return Some(LocalArrayAliasInfo {
            proven_len: prove_static_slice_len(alias.static_len.or(alias.proven_len), start, end),
            len: infer_static_slice_len_hint(Some(alias.len), start, end),
            static_len: if preserve_static_len {
                alias.static_len
            } else {
                None
            },
            elem_ty: alias.elem_ty,
            elem_struct: alias.elem_struct.clone(),
            writable: alias.writable,
        });
    }
    if let Some(len) = state_arrays.get(base).copied() {
        return Some(LocalArrayAliasInfo {
            proven_len: prove_static_slice_len(Some(len), start, end),
            len: infer_static_slice_len_hint(Some(len), start, end),
            static_len: preserve_static_len.then_some(len),
            elem_ty: declared_symbol_scalar_type(declared_symbols, base)
                .unwrap_or(PrimitiveType::F32),
            elem_struct: None,
            writable: true,
        });
    }
    if let Some((elem_ty, _)) = declared_buffer_info(declared_symbols, base) {
        return Some(LocalArrayAliasInfo {
            proven_len: None,
            len: 1,
            static_len: None,
            elem_ty,
            elem_struct: None,
            writable: true,
        });
    }

    let (root, field) = split_root_field_path(base)?;
    let struct_name = owner_structs.get(root)?;
    let field_decl = resolve_struct_field_decl(struct_name, field, struct_defs)?;
    if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
        if diagnose_non_array_field {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("field '{root}.{field}' is not array and cannot be sliced"),
            );
        }
        return None;
    }
    Some(LocalArrayAliasInfo {
        proven_len: prove_static_slice_len(
            match field_decl.ty {
                TypedFieldType::Array(len) => Some(len),
                _ => None,
            },
            start,
            end,
        ),
        len: infer_static_slice_len_hint(
            match field_decl.ty {
                TypedFieldType::Array(len) => Some(len),
                _ => None,
            },
            start,
            end,
        ),
        static_len: if preserve_static_len {
            match field_decl.ty {
                TypedFieldType::Array(len) => Some(len),
                _ => None,
            }
        } else {
            None
        },
        elem_ty: field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32),
        elem_struct: field_decl.array_elem_struct.clone(),
        writable: true,
    })
}

pub(crate) fn resolve_executable_slice_alias_info(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    owner_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    resolve_executable_array_source(
        base,
        start,
        end,
        declared_symbols,
        state_arrays,
        local_array_aliases,
        owner_structs,
        struct_defs,
        errors,
        false,
        true,
    )
}

pub(crate) fn resolve_executable_data_like_info(
    expr: &Expr,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    owner_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    match expr {
        Expr::Var { name: base, .. } => resolve_executable_array_source(
            base,
            None,
            None,
            declared_symbols,
            state_arrays,
            local_array_aliases,
            owner_structs,
            struct_defs,
            errors,
            true,
            false,
        ),
        Expr::Slice {
            base, start, end, ..
        } => resolve_executable_slice_alias_info(
            base,
            start.as_deref(),
            end.as_deref(),
            declared_symbols,
            state_arrays,
            local_array_aliases,
            owner_structs,
            struct_defs,
            errors,
        ),
        _ => None,
    }
}

/// Slice annotations constrain the view; they never create independent storage.
pub(crate) fn typed_slice_alias_info(
    expr: &Expr,
    element: &ArrayElemType,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    // Retain the invalid binding's expected slice shape so later uses do not
    // cascade into unrelated unknown-symbol diagnostics.
    let _ = reject_empty_slice_backing_literal(expr, errors);
    let literal = match (expr, element) {
        (Expr::ArrayLiteral { values, .. }, ArrayElemType::Primitive(ty)) => Some((values, *ty)),
        _ => None,
    };
    let fixed = literal
        .map(|(values, ty)| DataType::Array {
            element: ArrayElemType::Primitive(ty),
            len: values.len(),
        })
        .or_else(|| infer_fixed_data_type(expr, env));
    let source = resolve_executable_data_like_info(
        expr,
        env.declared_symbols,
        env.array_vars,
        env.local_array_aliases,
        env.struct_instances,
        env.struct_defs,
        errors,
    );
    let actual = match &fixed {
        Some(DataType::Array { element, .. }) => Some(element.clone()),
        _ => source.as_ref().map(|source| {
            source
                .elem_struct
                .as_ref()
                .map(|name| ArrayElemType::Struct(name.clone()))
                .unwrap_or(ArrayElemType::Primitive(source.elem_ty))
        }),
    };
    if actual.as_ref() != Some(element) {
        let element_name = |element: &ArrayElemType| match element {
            ArrayElemType::Primitive(ty) => ty.name().to_owned(),
            ArrayElemType::Struct(name) => name.clone(),
        };
        let actual = actual.as_ref().map_or_else(
            || "a non-array value".to_owned(),
            |actual| format!("element type '{}'", element_name(actual)),
        );
        errors.push(Diagnostic::semantic_span(
            format!(
                "slice declaration expects element type '{}', got {actual}",
                element_name(element)
            ),
            expr.loc(),
        ));
        return None;
    }
    if let Some((values, ty)) = literal {
        crate::expr_validation::validate_primitive_array_values(
            values,
            ty,
            values.len(),
            expr,
            env,
            errors,
        );
    } else {
        validate_fixed_data_expr(expr, env, errors);
    }
    let (elem_ty, elem_struct) = match element {
        ArrayElemType::Primitive(ty) => (*ty, None),
        ArrayElemType::Struct(name) => (PrimitiveType::F32, Some(name.clone())),
    };
    Some(LocalArrayAliasInfo {
        proven_len: source
            .as_ref()
            .and_then(|source| source.static_len.or(source.proven_len))
            .or(match &fixed {
                Some(DataType::Array { len, .. }) => Some(*len),
                _ => None,
            }),
        len: source
            .as_ref()
            .map(|source| source.len)
            .or(match fixed {
                Some(DataType::Array { len, .. }) => Some(len),
                _ => None,
            })
            .unwrap_or(1),
        static_len: None,
        elem_ty,
        elem_struct,
        writable: source.is_none_or(|source| source.writable),
    })
}

pub(crate) fn seed_struct_array_aliases(
    aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    roots: &HashMap<String, ArrayStructRootInfo>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    for (name, info) in roots {
        if struct_defs.contains_key(&info.struct_name) {
            aliases
                .entry(name.clone())
                .or_insert_with(|| LocalArrayAliasInfo {
                    len: info.len,
                    static_len: info.static_len,
                    proven_len: None,
                    elem_ty: PrimitiveType::F32,
                    elem_struct: Some(info.struct_name.clone()),
                    writable: true,
                });
        }
    }
}
