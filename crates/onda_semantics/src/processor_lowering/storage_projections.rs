//! Resolved storage symbols for declared processor projections.
//!
//! The producer registers each generated symbol with its typed selection plan.
//! Materialization uses exact symbol lookup; storage names encode no metadata.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct Selection {
    pub field: String,
    pub proc_name: String,
    pub identity: String,
    pub coordinate: usize,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedStorageProjection {
    pub selections: Vec<Selection>,
    pub field: Option<String>,
}

pub(super) type StorageProjectionSymbols =
    HashMap<String, HashMap<String, ResolvedStorageProjection>>;

pub(super) fn register_storage_projections(
    shapes: &HashMap<String, ProcLoweringShape>,
    structs: &HashMap<String, StructDef>,
) -> StorageProjectionSymbols {
    fn field_paths(
        field: &StructField,
        structs: &HashMap<String, StructDef>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) {
        let path = format!("{prefix}{}", field.name);
        paths.push(path.clone());
        match &field.ty {
            FieldType::Generic(name) => {
                if let Some(strukt) = structs.get(name) {
                    for nested in &strukt.fields {
                        field_paths(nested, structs, &format!("{path}."), paths);
                    }
                }
            }
            FieldType::Tuple(elements) => {
                paths.extend((0..elements.len()).map(|index| format!("{path}.__{index}")))
            }
            _ => {}
        }
    }
    fn visit(
        proc_name: &str,
        source_prefix: &str,
        field_prefix: &str,
        logical_prefix: &str,
        scalar_aliases: &[String],
        selections: &[Selection],
        shapes: &HashMap<String, ProcLoweringShape>,
        structs: &HashMap<String, StructDef>,
        symbols: &mut HashMap<String, ResolvedStorageProjection>,
    ) {
        let Some(shape) = shapes.get(proc_name) else {
            return;
        };
        for (declaration, child) in &shape.state.nested_procs {
            let source = format!("{source_prefix}{declaration}");
            let physical = format!("{field_prefix}{declaration}");
            let logical = format!("{logical_prefix}{declaration}");
            let mut aliases = scalar_aliases
                .iter()
                .map(|alias| format!("{alias}.{declaration}"))
                .collect::<Vec<_>>();
            aliases.push(source.clone());
            for alias in &aliases {
                symbols.insert(
                    alias.clone(),
                    ResolvedStorageProjection {
                        selections: selections.to_vec(),
                        field: Some(logical.clone()),
                    },
                );
            }
            visit(
                &child.proc_name,
                &format!("{source}__"),
                &format!("{physical}__"),
                &format!("{logical}."),
                &aliases,
                selections,
                shapes,
                structs,
                symbols,
            );
        }
        for declaration in shape.state.nested_proc_arrays.keys() {
            let Some(array) = shape.nested_proc_array_slots.get(declaration) else {
                continue;
            };
            let Some(child) = shapes.get(&array.proc_name) else {
                continue;
            };
            let array_projection = ResolvedStorageProjection {
                selections: selections.to_vec(),
                field: Some(format!("{logical_prefix}{declaration}")),
            };
            symbols.insert(
                format!("{source_prefix}{declaration}"),
                array_projection.clone(),
            );
            for alias in scalar_aliases {
                symbols.insert(format!("{alias}.{declaration}"), array_projection.clone());
            }
            for (coordinate, binding) in array.binding_names().enumerate() {
                let source = format!("{source_prefix}{binding}");
                let mut projected = selections.to_vec();
                projected.push(Selection {
                    field: format!("{field_prefix}{declaration}"),
                    proc_name: array.proc_name.clone(),
                    identity: source.clone(),
                    coordinate,
                });
                symbols.insert(
                    source.clone(),
                    ResolvedStorageProjection {
                        selections: projected.clone(),
                        field: None,
                    },
                );
                let mut paths = Vec::new();
                for field in &child.fields {
                    field_paths(field, structs, "", &mut paths);
                }
                for path in paths {
                    let projection = ResolvedStorageProjection {
                        selections: projected.clone(),
                        field: Some(path.clone()),
                    };
                    symbols.insert(format!("{source}__{path}"), projection.clone());
                    symbols.insert(format!("{source}.{path}"), projection);
                }
                visit(
                    &array.proc_name,
                    &format!("{source}__"),
                    "",
                    "",
                    &[],
                    &projected,
                    shapes,
                    structs,
                    symbols,
                );
            }
        }
    }
    shapes
        .keys()
        .map(|owner| {
            let mut symbols = HashMap::new();
            visit(owner, "", "", "", &[], &[], shapes, structs, &mut symbols);
            (owner.clone(), symbols)
        })
        .collect()
}
