use super::*;

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

    let prefix = format!("{field_name}[");
    if fields.iter().any(|f| f.name.starts_with(&prefix)) {
        return Some(IndexedBindingKind::ProcArrayAlias);
    }

    None
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
