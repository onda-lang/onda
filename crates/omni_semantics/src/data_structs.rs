use std::collections::{HashMap, HashSet};

use omni_frontend::{Diagnostic, PrimitiveType};

use crate::decl_symbols::{declared_type_key, DECLARED_DATA_ELEM_TYPE_PREFIX};
use crate::{
    DataStructRootInfo, LocalAliasTypes, LocalDataAliasInfo, TypedFieldType, TypedStructField,
};

#[derive(Debug, Clone)]
enum StructDataLayoutKind {
    Scalar(PrimitiveType),
    Data {
        len: usize,
        elem_ty: Option<PrimitiveType>,
        elem_struct: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct StructDataLayoutField {
    name: String,
    kind: StructDataLayoutKind,
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
) -> Option<Vec<StructDataLayoutField>> {
    let mut stack = Vec::<String>::new();
    collect_data_struct_layout_inner(struct_name, struct_defs, context, errors, &mut stack)
}

fn collect_data_struct_layout_inner(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> Option<Vec<StructDataLayoutField>> {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        errors.push(Diagnostic::semantic(
            format!("{context} contains recursive Data[Struct, N] cycle: {cycle}"),
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
            TypedFieldType::Scalar(prim) => layout.push(StructDataLayoutField {
                name: field.name,
                kind: StructDataLayoutKind::Scalar(prim),
            }),
            TypedFieldType::Data(len) => {
                if let Some(elem_struct) = &field.data_elem_struct {
                    let nested_context = format!(
                        "{context} nested Data field '{}.{}'",
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
                layout.push(StructDataLayoutField {
                    name: field.name,
                    kind: StructDataLayoutKind::Data {
                        len,
                        elem_ty: field.data_elem_ty,
                        elem_struct: field.data_elem_struct.clone(),
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
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
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
        state_data,
        state_data_struct_roots,
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
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
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
            format!("{context} contains recursive Data[Struct, N] cycle: {cycle}"),
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
    state_data_struct_roots
        .entry(base.to_owned())
        .or_insert(DataStructRootInfo {
            struct_name: struct_name.to_owned(),
            len,
        });

    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                state_scalars.insert(
                    declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                    prim,
                );
                state_data.entry(flat).or_insert(len);
            }
            TypedFieldType::Data(field_len) => {
                let nested_len = len.saturating_mul(field_len);
                if let Some(elem_struct) = &field.data_elem_struct {
                    let nested_context = format!(
                        "{context} nested Data field '{}.{}'",
                        struct_name, field.name
                    );
                    if !register_data_struct_root_inner(
                        &flat,
                        elem_struct,
                        nested_len,
                        struct_defs,
                        &nested_context,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        errors,
                        stack,
                    ) {
                        stack.pop();
                        return false;
                    }
                } else {
                    let elem_ty = field.data_elem_ty.unwrap_or(PrimitiveType::F32);
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        elem_ty,
                    );
                    state_data.entry(flat).or_insert(nested_len);
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
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(layout) = collect_data_struct_layout(struct_name, struct_defs, context, errors) else {
        return false;
    };
    for field in layout {
        match field.kind {
            StructDataLayoutKind::Scalar(prim) => {
                let alias = format!("{alias_name}.{}", field.name);
                local_aliases.insert(alias.clone(), prim);
                known_scalars.insert(alias);
            }
            StructDataLayoutKind::Data {
                len,
                elem_ty,
                elem_struct,
            } => {
                local_data_aliases.insert(
                    format!("{alias_name}.{}", field.name),
                    LocalDataAliasInfo {
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
