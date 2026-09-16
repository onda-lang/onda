use super::*;
use std::borrow::Cow;

pub(crate) struct IndexedAssignmentTarget<'a> {
    pub(crate) base: Cow<'a, str>,
    pub(crate) index: &'a Expr,
}

/// Preserves indexed-member syntax in the frontend and exposes the canonical
/// structure-of-arrays leaf only to semantic and lowering code.
pub(crate) fn indexed_assignment_target(
    target: &AssignTarget,
) -> Option<IndexedAssignmentTarget<'_>> {
    match target {
        AssignTarget::Index { base, index } => Some(IndexedAssignmentTarget {
            base: Cow::Borrowed(base),
            index,
        }),
        AssignTarget::IndexedMember {
            base,
            index,
            field,
            field_index: None,
        } => Some(IndexedAssignmentTarget {
            base: Cow::Owned(format!("{base}.{field}")),
            index,
        }),
        AssignTarget::IndexedMember { .. }
        | AssignTarget::Var(_)
        | AssignTarget::Slice { .. }
        | AssignTarget::Tuple(_) => None,
    }
}

pub(crate) fn flatten_indexed_member_target(target: &AssignTarget) -> Cow<'_, AssignTarget> {
    match target {
        AssignTarget::IndexedMember {
            base,
            index,
            field,
            field_index: None,
        } => Cow::Owned(AssignTarget::Index {
            base: format!("{base}.{field}"),
            index: index.clone(),
        }),
        _ => Cow::Borrowed(target),
    }
}

pub(crate) fn is_proc_array_member_base(
    base: &str,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
) -> bool {
    split_root_field_path(base).is_some_and(|(root, _)| proc_array_roots.contains_key(root))
}

pub(crate) fn indexed_assignment_element_type(
    target: &AssignTarget,
    struct_instances: &HashMap<String, String>,
    struct_array_roots: &HashMap<String, ArrayStructRootInfo>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    let target = indexed_assignment_target(target)?;
    let base = target.base.as_ref();
    let index = target.index;
    let (root, field_path) = split_root_field_path(base)?;
    let (struct_name, indexes_struct_element) = struct_instances
        .get(root)
        .map(|struct_name| (struct_name.as_str(), false))
        .or_else(|| {
            struct_array_roots
                .get(root)
                .map(|info| (info.struct_name.as_str(), true))
        })
        .or_else(|| {
            local_array_aliases
                .get(root)
                .and_then(|info| info.elem_struct.as_deref())
                .map(|struct_name| (struct_name, true))
        })
        .or_else(|| {
            proc_array_roots
                .get(root)
                .map(|info| (info.proc_name.as_str(), true))
        })?;
    if !indexes_struct_element {
        return resolve_indexed_struct_field_scalar_type(
            struct_name,
            field_path,
            index,
            struct_defs,
        );
    }
    let Some(field) = resolve_struct_field_decl(struct_name, field_path, struct_defs) else {
        return resolve_flattened_struct_array_leaf_type(struct_name, field_path, struct_defs);
    };
    match field.ty {
        TypedFieldType::Scalar(ty) => Some(ty),
        TypedFieldType::Struct | TypedFieldType::Array(_) | TypedFieldType::Tuple(_) => None,
    }
}

/// Validates the indexed-member forms whose destination is not a single
/// scalar field. Lowering later materializes the same element view and routes
/// the assignment through the ordinary aggregate or indexed store machinery.
pub(crate) fn validate_struct_array_member_assignment(
    target: &AssignTarget,
    value: &Expr,
    env: ExprEnv<'_>,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if validate_indexed_struct_field_assignment(target, value, env, target_loc, errors) {
        return true;
    }
    let AssignTarget::IndexedMember {
        base,
        index,
        field,
        field_index,
    } = target
    else {
        return false;
    };
    let Some(struct_name) = array_data_struct_element_type(base, env) else {
        return false;
    };
    let Some(field_decl) = resolve_struct_field_decl(&struct_name, field, env.struct_defs) else {
        errors.push(Diagnostic::semantic_span(
            format!("struct '{struct_name}' has no field '{field}'"),
            target_loc,
        ));
        return true;
    };
    if field_index.is_none() && matches!(field_decl.ty, TypedFieldType::Scalar(_)) {
        return false;
    }

    validate_numeric_selector(index, "struct-array index", env, errors);
    if env
        .local_array_aliases
        .get(base)
        .is_some_and(|alias| !alias.writable)
    {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign through immutable array alias '{base}'"),
            target_loc,
        ));
    }

    match (field_index.as_deref(), &field_decl.ty) {
        (Some(field_index), TypedFieldType::Array(_)) => {
            validate_numeric_selector(field_index, "struct field index", env, errors);
            if let Some(element) = &field_decl.array_elem_struct {
                validate_expected_data(
                    value,
                    &DataType::Struct(element.clone()),
                    "indexed field replacement",
                    env,
                    target_loc,
                    errors,
                );
            } else {
                validate_expr(value, env, errors);
                let expected = field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32);
                require_expr_assignable_type(
                    value,
                    infer_call_argument_scalar_type(value, env),
                    expected,
                    &format!("assignment to '{base}[...].{field}[...]'"),
                    errors,
                );
            }
        }
        (Some(field_index), TypedFieldType::Tuple(types)) => {
            validate_tuple_field_element_assignment(
                field_index,
                value,
                types,
                &format!("{base}[...].{field}"),
                env,
                errors,
            );
        }
        (Some(_), TypedFieldType::Scalar(_) | TypedFieldType::Struct) => {
            errors.push(Diagnostic::semantic_span(
                format!("field '{field}' of struct '{struct_name}' is not indexable"),
                target_loc,
            ));
        }
        (None, TypedFieldType::Array(len)) => {
            let element = field_decl
                .array_elem_struct
                .as_ref()
                .map(|name| ArrayElemType::Struct(name.clone()))
                .unwrap_or(ArrayElemType::Primitive(
                    field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32),
                ));
            validate_expected_data(
                value,
                &DataType::Array { element, len: *len },
                "indexed field replacement",
                env,
                target_loc,
                errors,
            );
        }
        (None, TypedFieldType::Struct) => {
            if let Some(name) = &field_decl.struct_name {
                validate_expected_data(
                    value,
                    &DataType::Struct(name.clone()),
                    "indexed field replacement",
                    env,
                    target_loc,
                    errors,
                );
            }
        }
        (None, TypedFieldType::Tuple(types)) => {
            validate_expr(value, env, errors);
            let Some(actual) = infer_call_argument_tuple_types(value, env) else {
                errors.push(Diagnostic::semantic_span(
                    format!("assignment to tuple field '{field}' requires a tuple value"),
                    target_loc,
                ));
                return true;
            };
            resolve_tuple_assignment_types(
                &format!("{base}[...].{field}"),
                value,
                &actual,
                None,
                Some(types),
                false,
                errors,
            );
        }
        (None, TypedFieldType::Scalar(_)) => unreachable!(),
    }
    true
}

fn validate_indexed_struct_field_assignment(
    target: &AssignTarget,
    value: &Expr,
    env: ExprEnv<'_>,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let AssignTarget::Index { base, index } = target else {
        return false;
    };
    let Some((root, field)) = split_root_field_path(base) else {
        return false;
    };
    let Some(struct_name) = env
        .struct_instances
        .get(root)
        .or_else(|| env.param_structs.get(root))
    else {
        return false;
    };
    let Some(field_decl) = resolve_struct_field_decl(struct_name, field, env.struct_defs) else {
        errors.push(Diagnostic::semantic_span(
            format!("struct '{struct_name}' has no field '{field}'"),
            target_loc,
        ));
        return true;
    };
    match &field_decl.ty {
        TypedFieldType::Tuple(types) => {
            validate_tuple_field_element_assignment(index, value, types, field, env, errors);
            true
        }
        TypedFieldType::Scalar(_) | TypedFieldType::Struct => {
            errors.push(Diagnostic::semantic_span(
                format!("field '{field}' of struct '{struct_name}' is not indexable"),
                target_loc,
            ));
            true
        }
        TypedFieldType::Array(_) => false,
    }
}

fn validate_tuple_field_element_assignment(
    index: &Expr,
    value: &Expr,
    types: &[PrimitiveType],
    field: &str,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    validate_expr(index, env, errors);
    let Expr::Int { value: raw, .. } = index else {
        errors.push(Diagnostic::semantic_span(
            "tuple field index must be a compile-time integer constant",
            index.loc(),
        ));
        return;
    };
    let Some(expected) = usize::try_from(*raw)
        .ok()
        .and_then(|index| types.get(index))
        .copied()
    else {
        errors.push(Diagnostic::semantic_span(
            format!(
                "tuple field index {raw} is out of bounds for '{field}' with {} elements",
                types.len()
            ),
            index.loc(),
        ));
        return;
    };
    validate_expr(value, env, errors);
    require_expr_assignable_type(
        value,
        infer_call_argument_scalar_type(value, env),
        expected,
        &format!("assignment to '{field}[{raw}]'"),
        errors,
    );
}

fn validate_numeric_selector(
    selector: &Expr,
    context: &str,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    validate_expr(selector, env, errors);
    require_expr_numeric_type(
        selector,
        infer_call_argument_scalar_type(selector, env),
        context,
        errors,
    );
}

pub(crate) fn validate_expected_data(
    value: &Expr,
    expected: &DataType,
    context: &str,
    env: ExprEnv<'_>,
    target_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let actual = infer_data_value_type(value, env);
    if actual.as_ref() != Some(expected) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context} {}",
                data_type_mismatch(expected, actual.as_ref())
            ),
            target_loc,
        ));
    }
    validate_fixed_data_expr(value, env, errors);
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum IndexedBindingKind {
    PrimitiveScalar,
    StructElementAlias(String),
    ProcArrayAlias,
}

fn classify_array_field_decl(
    field_decl: &TypedStructField,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<IndexedBindingKind> {
    if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
        return None;
    }

    match &field_decl.array_elem_struct {
        Some(elem_struct) if struct_defs.contains_key(elem_struct) => {
            Some(IndexedBindingKind::StructElementAlias(elem_struct.clone()))
        }
        Some(_) => Some(IndexedBindingKind::ProcArrayAlias),
        None => Some(IndexedBindingKind::PrimitiveScalar),
    }
}

fn classify_named_or_flattened_field(
    struct_name: &str,
    field_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<IndexedBindingKind> {
    let fields = struct_defs.get(struct_name)?;
    if let Some(field_decl) = fields.iter().find(|f| f.name == field_name) {
        return classify_array_field_decl(field_decl, struct_defs);
    }
    if field_name.contains('.') {
        if let Some(field_decl) = resolve_struct_field_decl(struct_name, field_name, struct_defs) {
            return classify_array_field_decl(field_decl, struct_defs);
        }
    }

    if is_flattened_proc_array_field(struct_name, field_name, struct_defs) {
        return Some(IndexedBindingKind::ProcArrayAlias);
    }

    None
}

pub(crate) fn is_flattened_proc_array_field(
    struct_name: &str,
    field_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> bool {
    let prefix = format!("{field_name}[");
    struct_defs
        .get(struct_name)
        .is_some_and(|fields| fields.iter().any(|field| field.name.starts_with(&prefix)))
}

pub(crate) fn classify_runtime_like_indexed_binding(
    base: &str,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    errors: &mut Vec<Diagnostic>,
) -> Option<IndexedBindingKind> {
    if proc_array_roots.contains_key(base) {
        return Some(IndexedBindingKind::ProcArrayAlias);
    }
    if state_arrays.contains_key(base) {
        return Some(IndexedBindingKind::PrimitiveScalar);
    }
    if let Some(root) = state_array_struct_roots.get(base) {
        return Some(IndexedBindingKind::StructElementAlias(
            root.struct_name.clone(),
        ));
    }
    if let Some(alias) = local_array_aliases.get(base) {
        return Some(match &alias.elem_struct {
            Some(elem_struct) => IndexedBindingKind::StructElementAlias(elem_struct.clone()),
            None => IndexedBindingKind::PrimitiveScalar,
        });
    }
    let flattened_proc_slot_prefix = format!("{base}[");
    if state_scalars
        .keys()
        .any(|field| field.starts_with(&flattened_proc_slot_prefix))
    {
        return Some(IndexedBindingKind::ProcArrayAlias);
    }
    let (root, field) = split_field_path(base, errors)?;
    let struct_name = struct_instances.get(root)?;
    classify_named_or_flattened_field(struct_name, field, struct_defs)
}
