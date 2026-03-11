use super::*;

pub(crate) fn merged_data_vars_for_runtime(
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
                len: info.len,
                elem_ty: info.elem_ty,
                elem_struct: None,
                writable,
            },
        );
    }
}

pub(crate) fn infer_scope_slice_alias_info(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: Option<&HashMap<String, usize>>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    owner_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    if let Some(alias) = local_array_aliases.get(base) {
        if alias.elem_struct.is_some() {
            errors.push(Diagnostic::semantic(
                format!("slice expression '{base}[...]' requires primitive elements"),
                0,
                0,
            ));
            return None;
        }
        return Some(LocalArrayAliasInfo {
            len: infer_static_slice_len_hint(Some(alias.len), start, end),
            elem_ty: alias.elem_ty,
            elem_struct: None,
            writable: alias.writable,
        });
    }
    if let Some(state_arrays) = state_arrays {
        if let Some(len) = state_arrays.get(base).copied() {
            return Some(LocalArrayAliasInfo {
                len: infer_static_slice_len_hint(Some(len), start, end),
                elem_ty: declared_symbol_scalar_type(declared_symbols, base)
                    .unwrap_or(PrimitiveType::F32),
                elem_struct: None,
                writable: true,
            });
        }
    }
    if let Some((elem_ty, _)) = declared_buffer_info(declared_symbols, base) {
        return Some(LocalArrayAliasInfo {
            len: 1,
            elem_ty,
            elem_struct: None,
            writable: true,
        });
    }

    let Some((root, field)) = split_field_path(base, errors) else {
        return None;
    };
    let Some(struct_name) = owner_structs.get(root) else {
        return None;
    };
    let Some(field_decl) = resolve_struct_field_decl(struct_name, field, struct_defs) else {
        return None;
    };
    if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
        errors.push(Diagnostic::semantic(
            format!("field '{root}.{field}' is not array and cannot be sliced"),
            0,
            0,
        ));
        return None;
    }
    if field_decl.array_elem_struct.is_some() {
        errors.push(Diagnostic::semantic(
            format!("slice expression '{base}[...]' requires primitive elements"),
            0,
            0,
        ));
        return None;
    }

    Some(LocalArrayAliasInfo {
        len: infer_static_slice_len_hint(
            match field_decl.ty {
                TypedFieldType::Array(len) => Some(len),
                _ => None,
            },
            start,
            end,
        ),
        elem_ty: field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32),
        elem_struct: None,
        writable: true,
    })
}

pub(crate) fn infer_scope_data_like_info(
    expr: &Expr,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: Option<&HashMap<String, usize>>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    owner_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    match expr {
        Expr::Var(base) => infer_scope_slice_alias_info(
            base,
            None,
            None,
            declared_symbols,
            state_arrays,
            local_array_aliases,
            owner_structs,
            struct_defs,
            errors,
        ),
        Expr::Slice { base, start, end } => infer_scope_slice_alias_info(
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
