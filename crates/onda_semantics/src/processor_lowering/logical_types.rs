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
            let inferred_io = infer_numbered_io_from_sample(&proc.sample);
            let inferred_names = infer_numbered_names_from_proc(proc);
            let ins = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
            let outs = normalize_numbered_port_decls(
                &proc.outs,
                proc_output_numbered_prefix(proc),
                match proc.outs_timing {
                    OutputTiming::Sample => inferred_io.max_out,
                    OutputTiming::Block => inferred_names.max_kout,
                },
            );
            let params =
                normalize_numbered_param_decls(&proc.params, "param", inferred_names.max_param);
            let (_, ins_types, _, ins_arrays) = expand_proc_port_specs(
                &proc.name,
                &ins,
                "input",
                options,
                const_arrays,
                &mut errors,
            );
            let (_, outs_types, _, outs_arrays) = expand_proc_port_specs(
                &proc.name,
                &outs,
                "output",
                options,
                const_arrays,
                &mut errors,
            );
            let (params, param_arrays) =
                expand_proc_param_specs(&proc.name, &params, options, const_arrays, &mut errors);
            let mut scalar_types = ins_types;
            scalar_types.extend(outs_types);
            for param in params {
                scalar_types.extend(param.slots.into_iter().map(|slot| (slot.name, slot.ty)));
            }
            let arrays = ins_arrays
                .into_iter()
                .chain(outs_arrays)
                .chain(param_arrays)
                .collect::<HashMap<_, _>>();
            let mut symbols = DeclaredSymbolMap::new();
            crate::decl_symbols::bind_const_symbols(&mut symbols, const_scalars, const_arrays);
            symbols.const_scope = const_scope.cloned();
            symbols.options = options;
            for buffer in coerce_buffers(&proc.buffers, options, &mut errors) {
                symbols.insert(
                    buffer.name,
                    DeclaredSymbolInfo::Buffer {
                        elem_ty: buffer.elem_ty,
                        channels: match buffer.channels {
                            TypedBufferChannels::Mono => BufferChannelInfo::Mono,
                            TypedBufferChannels::Static(n) => BufferChannelInfo::Static(n),
                            TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
                        },
                        array_len: buffer.array_len,
                        is_array: buffer.is_array,
                    },
                );
            }
            let label = format!("processor '{}'", proc.name);
            let namespace = namespace_of_symbol(&proc.name);
            let reserved = scalar_types
                .keys()
                .chain(symbols.keys())
                .cloned()
                .collect::<HashSet<_>>();
            let ctx = InitAnalysisCtx {
                context_label: &label,
                common: ScopeAnalysisCtx {
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
                init_default_ty: resolve_init_default_ty(
                    proc.init.default_ty.as_ref(),
                    &label,
                    &mut errors,
                ),
                proc_resolution: Some(ProcResolutionCtx {
                    owner_proc_name: &proc.name,
                    reserved: &reserved,
                    current_ns: &namespace,
                    proc_symbols: &proc_symbols,
                    struct_symbols: &struct_symbols,
                    frontend_struct_defs: structs,
                    ctor_symbols,
                    in_init_scope: true,
                }),
                top_level_proc_symbols: None,
            };
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
            analyze_owner_init_stmts(&proc.init, &ctx, &empty, &mut state, &mut errors);
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
