//! Nominal source fields exist before processors receive their physical layout.
//!
//! Build these declarations independently of processor order. Runtime scope
//! checking can then follow a borrowed child through any processor type without
//! mistaking the eventual flattened storage fields for its source interface.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn processor_source_types(
    procs: &HashMap<String, onda_frontend::ProcessorDef>,
    structs: &HashMap<String, onda_frontend::StructDef>,
    ctor_symbols: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    fn_return_types: &HashMap<String, ReturnType>,
    options: AnalysisOptions,
    const_arrays: &HashMap<String, TypedArrayInfo>,
    const_scalars: &HashMap<String, PrimitiveType>,
    const_scope: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
) -> HashMap<String, Vec<TypedStructField>> {
    let typed_structs = struct_defs_for_scalar_expr_inference(structs);
    let proc_symbols = procs.keys().cloned().collect::<HashSet<_>>();
    let struct_symbols = structs.keys().cloned().collect::<HashSet<_>>();
    let empty = HashSet::new();
    procs
        .values()
        .map(|proc| {
            // The real shape pass reports diagnostics. This declaration pass
            // reuses the same rules for constructor resolution and state types.
            let mut errors = Vec::new();
            let options = proc_options_for_shape(proc, options);
            let declarations = prepare_proc_declarations(proc, options, const_arrays, &mut errors);
            let mut scalar_types = declarations.inputs.types;
            scalar_types.extend(declarations.outputs.types);
            for param in declarations.params {
                scalar_types.extend(param.slots.into_iter().map(|slot| (slot.name, slot.ty)));
            }
            let arrays = declarations
                .inputs
                .arrays
                .into_iter()
                .chain(declarations.outputs.arrays)
                .chain(declarations.param_arrays)
                .collect::<HashMap<_, _>>();
            let mut symbols = DeclaredSymbolMap::new();
            crate::decl_symbols::bind_const_symbols(&mut symbols, const_scalars, const_arrays);
            symbols.const_scope = const_scope.cloned();
            symbols.options = options;
            bind_proc_buffer_symbols(&declarations.buffers, &mut symbols);
            let reserved = scalar_types
                .keys()
                .chain(symbols.keys())
                .cloned()
                .collect::<HashSet<_>>();
            let mut aliases = HashMap::new();
            seed_top_level_array_aliases(&mut aliases, const_arrays, false);
            for (name, slots) in &arrays {
                let elem_ty = slots
                    .first()
                    .and_then(|slot| scalar_types.get(slot))
                    .copied()
                    .unwrap_or(PrimitiveType::F32);
                aliases.insert(
                    name.clone(),
                    LocalArrayAliasInfo {
                        proven_len: None,
                        len: slots.len(),
                        static_len: Some(slots.len()),
                        elem_ty,
                        elem_struct: None,
                        writable: true,
                    },
                );
            }
            let mut state = InitAnalysisState::new(
                reserved.clone(),
                HashMap::new(),
                aliases,
                symbols,
                scalar_types,
            );
            analyze_processor_init(
                proc,
                ScopeAnalysisCtx {
                    policy: ScopePolicy::Init,
                    input_names: &empty,
                    output_names: &empty,
                    output_array_names: &empty,
                    io_surface_names: &empty,
                    io_surface_array_names: &empty,
                    dynamic_param_array_names: &empty,
                    param_names: &empty,
                    struct_defs: &typed_structs,
                    fn_signatures,
                    fn_return_types,
                    options,
                    port_index_ins: None,
                    port_index_outs: None,
                    port_index_params: None,
                    port_index_kins: None,
                    proc_event_names: &empty,
                },
                ProcConstructors {
                    reserved: &reserved,
                    procs: &proc_symbols,
                    struct_symbols: &struct_symbols,
                    structs,
                    symbols: ctor_symbols,
                },
                &mut state,
                &mut errors,
            );
            let mut fields = HashMap::new();
            let mut add = |name: &str, ty, struct_name, array_elem_ty, array_elem_struct| {
                if !name.contains(['.', '['])
                    && !proc
                        .init
                        .compiler_scratch_roots
                        .iter()
                        .any(|root| root == name)
                {
                    fields.insert(
                        name.to_owned(),
                        TypedStructField {
                            name: name.to_owned(),
                            ty,
                            default: None,
                            integer_range: None,
                            struct_name,
                            array_elem_ty,
                            array_elem_struct,
                        },
                    );
                }
            };
            for (name, ty) in &state.state_scalars {
                add(name, TypedFieldType::Scalar(*ty), None, None, None);
            }
            for (name, slots) in &arrays {
                let ty = slots
                    .first()
                    .and_then(|slot| state.state_scalars.get(slot))
                    .copied()
                    .unwrap_or(PrimitiveType::F32);
                add(
                    name,
                    TypedFieldType::Array(slots.len()),
                    None,
                    Some(ty),
                    None,
                );
            }
            for (name, len) in &state.state_arrays {
                let ty = state
                    .state_scalars
                    .get(name)
                    .copied()
                    .unwrap_or(PrimitiveType::F32);
                add(name, TypedFieldType::Array(*len), None, Some(ty), None);
            }
            for (name, root) in &state.state_array_struct_roots {
                add(
                    name,
                    TypedFieldType::Array(root.len),
                    None,
                    None,
                    Some(root.struct_name.clone()),
                );
            }
            for (name, ty) in &state.struct_instances {
                add(name, TypedFieldType::Struct, Some(ty.clone()), None, None);
            }
            for (name, child) in &state.nested_procs {
                add(
                    name,
                    TypedFieldType::Struct,
                    Some(child.proc_name.clone()),
                    None,
                    None,
                );
            }
            for (name, tys) in &state.state_tuples {
                add(name, TypedFieldType::Tuple(tys.clone()), None, None, None);
            }
            let mut fields = fields.into_values().collect::<Vec<_>>();
            fields.sort_by(|a, b| a.name.cmp(&b.name));
            (proc.name.clone(), fields)
        })
        .collect()
}
