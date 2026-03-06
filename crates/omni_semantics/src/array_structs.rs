use std::collections::{HashMap, HashSet};

use omni_frontend::{Diagnostic, PrimitiveType};

use crate::decl_symbols::{declared_type_key, DECLARED_DATA_ELEM_TYPE_PREFIX};
use crate::{
    ArrayStructRootInfo, LocalAliasTypes, LocalArrayAliasInfo, TypedFieldType, TypedStructField,
};

#[derive(Debug, Clone)]
enum StructArrayLayoutKind {
    Scalar(PrimitiveType),
    Array {
        len: usize,
        elem_ty: Option<PrimitiveType>,
        elem_struct: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct StructArrayLayoutField {
    name: String,
    kind: StructArrayLayoutKind,
}

pub(crate) fn validate_data_struct_layout(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    collect_data_struct_layout(struct_name, struct_defs, context, errors).is_some()
}

fn collect_data_struct_layout(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<StructArrayLayoutField>> {
    let mut stack = Vec::<String>::new();
    collect_data_struct_layout_inner(struct_name, struct_defs, context, errors, &mut stack)
}

fn collect_data_struct_layout_inner(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> Option<Vec<StructArrayLayoutField>> {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        errors.push(Diagnostic::semantic(
            format!("{context} contains recursive array[Struct, N] cycle: {cycle}"),
            0,
            0,
        ));
        return None;
    }
    let fields = struct_defs.get(struct_name).cloned();
    let Some(fields) = fields else {
        errors.push(Diagnostic::semantic(
            format!("{context} references unknown struct '{struct_name}'"),
            0,
            0,
        ));
        return None;
    };

    stack.push(struct_name.to_owned());
    let mut layout = Vec::new();
    for field in fields {
        match field.ty {
            TypedFieldType::Scalar(prim) => layout.push(StructArrayLayoutField {
                name: field.name,
                kind: StructArrayLayoutKind::Scalar(prim),
            }),
            TypedFieldType::Struct => {}
            TypedFieldType::Array(len) => {
                if let Some(elem_struct) = &field.array_elem_struct {
                    let nested_context = format!(
                        "{context} nested array field '{}.{}'",
                        struct_name, field.name
                    );
                    if collect_data_struct_layout_inner(
                        elem_struct,
                        struct_defs,
                        &nested_context,
                        errors,
                        stack,
                    )
                    .is_none()
                    {
                        stack.pop();
                        return None;
                    }
                }
                layout.push(StructArrayLayoutField {
                    name: field.name,
                    kind: StructArrayLayoutKind::Array {
                        len,
                        elem_ty: field.array_elem_ty,
                        elem_struct: field.array_elem_struct.clone(),
                    },
                });
            }
        }
    }
    stack.pop();
    Some(layout)
}

pub(crate) fn register_data_struct_root(
    base: &str,
    struct_name: &str,
    len: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if !validate_data_struct_layout(struct_name, struct_defs, context, errors) {
        return false;
    }
    let mut stack = Vec::<String>::new();
    register_data_struct_root_inner(
        base,
        struct_name,
        len,
        struct_defs,
        context,
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        errors,
        &mut stack,
    )
}

#[allow(clippy::too_many_arguments)]
fn register_data_struct_root_inner(
    base: &str,
    struct_name: &str,
    len: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> bool {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        errors.push(Diagnostic::semantic(
            format!("{context} contains recursive array[Struct, N] cycle: {cycle}"),
            0,
            0,
        ));
        return false;
    }
    let Some(fields) = struct_defs.get(struct_name).cloned() else {
        errors.push(Diagnostic::semantic(
            format!("{context} references unknown struct '{struct_name}'"),
            0,
            0,
        ));
        return false;
    };
    state_array_struct_roots
        .entry(base.to_owned())
        .or_insert(ArrayStructRootInfo {
            struct_name: struct_name.to_owned(),
            len,
        });
    // Mark the root symbol so index validation can recognize `base[idx]` even
    // when the struct has no scalar/array fields to contribute `base.*` keys.
    state_scalars
        .entry(declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, base))
        .or_insert(PrimitiveType::F32);

    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                state_scalars.insert(
                    declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                    prim,
                );
                state_arrays.entry(flat).or_insert(len);
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Array(field_len) => {
                let nested_len = len.saturating_mul(field_len);
                if let Some(elem_struct) = &field.array_elem_struct {
                    let nested_context = format!(
                        "{context} nested array field '{}.{}'",
                        struct_name, field.name
                    );
                    if !register_data_struct_root_inner(
                        &flat,
                        elem_struct,
                        nested_len,
                        struct_defs,
                        &nested_context,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                        stack,
                    ) {
                        stack.pop();
                        return false;
                    }
                } else {
                    let elem_ty = field.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        elem_ty,
                    );
                    state_arrays.entry(flat).or_insert(nested_len);
                }
            }
        }
    }
    stack.pop();
    true
}

pub(crate) fn add_struct_element_alias_bindings(
    alias_name: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(layout) = collect_data_struct_layout(struct_name, struct_defs, context, errors) else {
        return false;
    };
    for field in layout {
        match field.kind {
            StructArrayLayoutKind::Scalar(prim) => {
                let alias = format!("{alias_name}.{}", field.name);
                local_aliases.insert(alias.clone(), prim);
                known_scalars.insert(alias);
            }
            StructArrayLayoutKind::Array {
                len,
                elem_ty,
                elem_struct,
            } => {
                local_array_aliases.insert(
                    format!("{alias_name}.{}", field.name),
                    LocalArrayAliasInfo {
                        len,
                        elem_ty: elem_ty.unwrap_or(PrimitiveType::F32),
                        elem_struct,
                        writable: true,
                    },
                );
            }
        }
    }
    true
}
