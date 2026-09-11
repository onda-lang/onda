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
        AssignTarget::IndexedMember { base, index, field } => Some(IndexedAssignmentTarget {
            base: Cow::Owned(format!("{base}.{field}")),
            index,
        }),
        AssignTarget::Var(_) | AssignTarget::Slice { .. } | AssignTarget::Tuple(_) => None,
    }
}

pub(crate) fn flatten_indexed_member_target(target: &AssignTarget) -> Cow<'_, AssignTarget> {
    match target {
        AssignTarget::IndexedMember { base, index, field } => Cow::Owned(AssignTarget::Index {
            base: format!("{base}.{field}"),
            index: index.clone(),
        }),
        _ => Cow::Borrowed(target),
    }
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
    let field = if let Some(field) = resolve_struct_field_decl(struct_name, field_path, struct_defs)
    {
        field
    } else {
        return resolve_flattened_struct_array_leaf_type(struct_name, field_path, struct_defs);
    };
    if indexes_struct_element {
        return match field.ty {
            TypedFieldType::Scalar(ty) => Some(ty),
            TypedFieldType::Struct | TypedFieldType::Array(_) | TypedFieldType::Tuple(_) => None,
        };
    }
    match &field.ty {
        TypedFieldType::Array(_) => field.array_elem_ty,
        TypedFieldType::Tuple(types) => match index {
            Expr::Int { value, .. } => usize::try_from(*value)
                .ok()
                .and_then(|index| types.get(index).copied()),
            _ => None,
        },
        TypedFieldType::Scalar(_) | TypedFieldType::Struct => None,
    }
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
