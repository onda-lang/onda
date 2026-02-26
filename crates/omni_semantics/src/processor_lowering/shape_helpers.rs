use super::*;

pub(super) fn proc_os_prev_input_field_name(input_name: &str) -> String {
    format!("__omni_os_prev_in__{input_name}")
}

pub(super) fn proc_os_up1_input_field_name(input_name: &str) -> String {
    format!("__omni_os_up1_in__{input_name}")
}

pub(super) fn proc_os_up2_input_field_name(input_name: &str) -> String {
    format!("__omni_os_up2_in__{input_name}")
}

pub(super) fn proc_os_down1_output_field_name(output_name: &str) -> String {
    format!("__omni_os_down1_out__{output_name}")
}

pub(super) fn proc_os_down2_output_field_name(output_name: &str) -> String {
    format!("__omni_os_down2_out__{output_name}")
}

pub(super) fn compute_effective_proc_block_flags(
    proc_order: &[String],
    proc_defs_by_name: &HashMap<String, omni_frontend::ProcessorDef>,
    base_shapes: &HashMap<String, ProcBaseShape>,
) -> HashMap<String, bool> {
    fn visit(
        proc_name: &str,
        proc_defs_by_name: &HashMap<String, omni_frontend::ProcessorDef>,
        base_shapes: &HashMap<String, ProcBaseShape>,
        cache: &mut HashMap<String, bool>,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if let Some(value) = cache.get(proc_name) {
            return *value;
        }
        if !visiting.insert(proc_name.to_owned()) {
            return false;
        }
        let mut has_block = proc_defs_by_name
            .get(proc_name)
            .map(|p| p.has_block_block)
            .unwrap_or(false);
        if !has_block {
            if let (Some(proc_def), Some(shape)) =
                (proc_defs_by_name.get(proc_name), base_shapes.get(proc_name))
            {
                let nested_instances = shape
                    .state
                    .nested_procs
                    .iter()
                    .map(|(name, state)| {
                        (
                            name.clone(),
                            ProcCallInstance {
                                proc_name: state.proc_name.clone(),
                                buffer_args: Vec::new(),
                            },
                        )
                    })
                    .collect::<HashMap<_, _>>();
                let called_nested = collect_called_proc_instances_in_stmts(
                    &proc_def.sample,
                    &nested_instances,
                    &shape.nested_proc_array_slots,
                );
                for nested_var in called_nested {
                    let Some(instance) = nested_instances.get(&nested_var) else {
                        continue;
                    };
                    if visit(
                        &instance.proc_name,
                        proc_defs_by_name,
                        base_shapes,
                        cache,
                        visiting,
                    ) {
                        has_block = true;
                        break;
                    }
                }
            }
        }
        visiting.remove(proc_name);
        cache.insert(proc_name.to_owned(), has_block);
        has_block
    }

    let mut cache = HashMap::<String, bool>::new();
    let mut visiting = HashSet::<String>::new();
    for proc_name in proc_order {
        let _ = visit(
            proc_name,
            proc_defs_by_name,
            base_shapes,
            &mut cache,
            &mut visiting,
        );
    }
    cache
}

pub(super) fn infer_primary_output_type_from_processor(proc: &ProcessorDef) -> PrimitiveType {
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let out_ports = normalize_numbered_port_decls(&proc.outs, "out", inferred_io.max_out);
    let Some(first_out) = out_ports.first() else {
        return PrimitiveType::F32;
    };
    match first_out.ty.as_ref() {
        Some(DeclType::Scalar(ty)) => *ty,
        Some(DeclType::Array { elem, .. }) => *elem,
        Some(DeclType::Generic(_)) | Some(DeclType::ArrayGeneric { .. }) | None => {
            PrimitiveType::F32
        }
    }
}

pub(super) fn struct_defs_for_scalar_expr_inference(
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
) -> HashMap<String, Vec<TypedStructField>> {
    struct_defs
        .iter()
        .map(|(name, def)| {
            let fields = def
                .fields
                .iter()
                .map(|field| {
                    let (ty, data_elem_ty, data_elem_struct) = match &field.ty {
                        FieldType::Scalar(prim) => (TypedFieldType::Scalar(*prim), None, None),
                        FieldType::Generic(_) => {
                            // Generic struct fields are unresolved in this prepass; default like other unresolved scalar contexts.
                            (TypedFieldType::Scalar(PrimitiveType::F32), None, None)
                        }
                        FieldType::Data(spec) => {
                            let (elem_ty, elem_struct) = match &spec.elem {
                                DataElemType::Primitive(prim) => (Some(*prim), None),
                                DataElemType::Struct(struct_name) => {
                                    (None, Some(struct_name.clone()))
                                }
                            };
                            (TypedFieldType::Data(0), elem_ty, elem_struct)
                        }
                    };
                    TypedStructField {
                        name: field.name.clone(),
                        ty,
                        default: field.default.clone(),
                        data_elem_ty,
                        data_elem_struct,
                    }
                })
                .collect::<Vec<_>>();
            (name.clone(), fields)
        })
        .collect::<HashMap<_, _>>()
}

fn is_proc_output_alias_name(name: &str, out_count: usize) -> bool {
    let Some(rest) = name.strip_prefix("out") else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let Ok(idx) = rest.parse::<usize>() else {
        return false;
    };
    idx >= 1 && idx <= out_count
}

pub(super) fn compute_proc_shape(
    proc: &omni_frontend::ProcessorDef,
    sample_oversample_factor: usize,
    options: AnalysisOptions,
    proc_symbols: &HashSet<String>,
    proc_primary_output_types: &HashMap<String, PrimitiveType>,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    ctor_symbols: &HashSet<String>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> ProcBaseShape {
    let struct_symbols = struct_defs.keys().cloned().collect::<HashSet<_>>();
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let ins_ports = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
    let out_ports = normalize_numbered_port_decls(&proc.outs, "out", inferred_io.max_out);
    let (ins, in_types, in_ports, in_array_slots) =
        expand_proc_port_specs(&proc.name, &ins_ports, "input", options, errors);
    let (outs, out_types, _out_ports, out_array_slots) =
        expand_proc_port_specs(&proc.name, &out_ports, "output", options, errors);
    let (param_specs, mut field_array_slots) =
        expand_proc_param_specs(&proc.name, &proc.params, options, errors);
    let buffer_specs = coerce_buffers(&proc.buffers, options, errors)
        .into_iter()
        .map(|b| ProcBufferSpec {
            name: b.name,
            elem_ty: b.elem_ty,
            channels: b.channels,
        })
        .collect::<Vec<_>>();
    for (name, slots) in out_array_slots {
        field_array_slots.insert(name, slots);
    }

    let typed_struct_defs = struct_defs_for_scalar_expr_inference(struct_defs);
    let mut typed_param_names = HashSet::<String>::new();
    for spec in &param_specs {
        typed_param_names.insert(spec.name.clone());
        for slot in &spec.slots {
            typed_param_names.insert(slot.name.clone());
        }
    }
    let mut param_names = typed_param_names.clone();
    for buffer in &buffer_specs {
        if param_names.contains(&buffer.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' buffer '{}' conflicts with param name",
                    proc.name, buffer.name
                ),
                0,
                0,
            ));
        }
        param_names.insert(buffer.name.clone());
    }
    let mut ins_names = ins.iter().cloned().collect::<HashSet<_>>();
    for spec in &in_ports {
        ins_names.insert(spec.name.clone());
    }
    let mut out_names = outs.iter().cloned().collect::<HashSet<_>>();
    for port in &out_ports {
        out_names.insert(port.name.clone());
    }
    let mut seen_event_names = HashSet::<String>::new();
    for event in &proc.events {
        if !seen_event_names.insert(event.name.clone()) {
            continue;
        }
        if ins_names.contains(&event.name)
            || out_names.contains(&event.name)
            || param_names.contains(&event.name)
            || is_proc_output_alias_name(&event.name, outs.len())
        {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}.{}' event name conflicts with an existing callable/endpoint name",
                    proc.name, event.name
                ),
                0,
                0,
            ));
        }
    }
    let mut reserved = HashSet::<String>::new();
    reserved.extend(param_names.iter().cloned());
    reserved.extend(ins_names.iter().cloned());
    reserved.extend(out_names.iter().cloned());

    let mut state_type_hints = HashMap::<String, PrimitiveType>::new();
    set_declared_symbol_types(
        &mut state_type_hints,
        &ins_names,
        &in_types,
        DECLARED_INPUT_TYPE_PREFIX,
    );
    set_declared_symbol_types(
        &mut state_type_hints,
        &out_names,
        &out_types,
        DECLARED_OUTPUT_TYPE_PREFIX,
    );
    let param_slot_types = param_specs
        .iter()
        .flat_map(|spec| spec.slots.iter())
        .map(|slot| (slot.name.clone(), slot.ty))
        .collect::<HashMap<_, _>>();
    set_declared_symbol_types(
        &mut state_type_hints,
        &typed_param_names,
        &param_slot_types,
        DECLARED_PARAM_TYPE_PREFIX,
    );
    for (fn_name, fn_ty) in fn_return_types {
        state_type_hints.insert(
            declared_type_key(DECLARED_FUNCTION_RETURN_TYPE_PREFIX, fn_name),
            *fn_ty,
        );
    }

    let mut state = ProcStateFields::default();
    let proc_ns = namespace_of_symbol(&proc.name);
    let proc_locals = HashSet::<String>::new();
    for stmt in &proc.init {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_locals,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            true,
            &mut state,
            errors,
        );
    }
    for stmt in &proc.block_pre {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_locals,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            false,
            &mut state,
            errors,
        );
    }
    for stmt in &proc.sample {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_locals,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            false,
            &mut state,
            errors,
        );
    }
    for stmt in &proc.block_post {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_locals,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            false,
            &mut state,
            errors,
        );
    }

    let mut nested_proc_array_slots = HashMap::<String, Vec<String>>::new();
    let mut nested_proc_array_names = state.nested_proc_arrays.keys().cloned().collect::<Vec<_>>();
    nested_proc_array_names.sort();
    for array_name in nested_proc_array_names {
        let Some(array_state) = state.nested_proc_arrays.get(&array_name).cloned() else {
            continue;
        };
        let size_context = format!(
            "processor '{}.{}' processor-array size",
            proc.name, array_name
        );
        let Some(len) = eval_data_size_expr(&array_state.size_expr, options, &size_context, errors)
        else {
            continue;
        };
        let mut slots = Vec::<String>::with_capacity(len);
        for idx in 0..len {
            let slot = format!("{array_name}[{idx}]");
            if reserved.contains(&slot) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' processor-array slot '{}' conflicts with reserved symbol",
                        proc.name, slot
                    ),
                    0,
                    0,
                ));
                continue;
            }
            if let Some(existing) = state.nested_procs.get(&slot) {
                if existing.proc_name != array_state.proc_name {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}' processor-array slot '{}' has conflicting processor types '{}' and '{}'",
                            proc.name, slot, existing.proc_name, array_state.proc_name
                        ),
                        0,
                        0,
                    ));
                }
            } else if state.scalars.contains_key(&slot)
                || state.data.contains_key(&slot)
                || state.struct_instances.contains_key(&slot)
            {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' processor-array slot '{}' conflicts with existing state symbol",
                        proc.name, slot
                    ),
                    0,
                    0,
                ));
                continue;
            } else {
                state.nested_procs.insert(
                    slot.clone(),
                    ProcNestedState {
                        proc_name: array_state.proc_name.clone(),
                    },
                );
            }
            slots.push(slot);
        }
        nested_proc_array_slots.insert(array_name, slots);
    }

    let mut state_scalar_names = state.scalars.keys().cloned().collect::<Vec<_>>();
    state_scalar_names.sort();
    let mut state_data_names = state.data.keys().cloned().collect::<Vec<_>>();
    state_data_names.sort();
    let mut struct_instance_names = state.struct_instances.keys().cloned().collect::<Vec<_>>();
    struct_instance_names.sort();

    let mut fields = Vec::<StructField>::new();
    let mut instance_fields = HashMap::<String, HashSet<String>>::new();
    for spec in &param_specs {
        for slot in &spec.slots {
            fields.push(StructField {
                name: slot.name.clone(),
                ty: FieldType::Scalar(slot.ty),
                default: slot.default.clone(),
            });
        }
    }
    for out_name in &outs {
        let out_ty = *out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
        fields.push(StructField {
            name: out_name.clone(),
            ty: FieldType::Scalar(out_ty),
            default: None,
        });
    }
    if sample_oversample_factor > 1 {
        for in_name in &ins {
            for state_name in [
                proc_os_prev_input_field_name(in_name),
                proc_os_up1_input_field_name(in_name),
                proc_os_up2_input_field_name(in_name),
            ] {
                fields.push(StructField {
                    name: state_name,
                    ty: FieldType::Scalar(PrimitiveType::F32),
                    default: Some(Expr::Number(0.0)),
                });
            }
        }
        for out_name in &outs {
            let out_ty = *out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
            if matches!(out_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                fields.push(StructField {
                    name: proc_os_down1_output_field_name(out_name),
                    ty: FieldType::Scalar(out_ty),
                    default: Some(Expr::Number(0.0)),
                });
                fields.push(StructField {
                    name: proc_os_down2_output_field_name(out_name),
                    ty: FieldType::Scalar(out_ty),
                    default: Some(Expr::Number(0.0)),
                });
            }
        }
    }
    for name in &state_scalar_names {
        if reserved.contains(name) {
            continue;
        }
        fields.push(StructField {
            name: name.clone(),
            ty: FieldType::Scalar(*state.scalars.get(name).unwrap_or(&PrimitiveType::F32)),
            default: None,
        });
    }
    for name in &state_data_names {
        if reserved.contains(name) {
            continue;
        }
        if let Some(spec) = state.data.get(name) {
            let _ = eval_data_size_expr(
                &spec.size,
                options,
                &format!("processor '{}.{}' Data size", proc.name, name),
                errors,
            );
            fields.push(StructField {
                name: name.clone(),
                ty: FieldType::Data(spec.clone()),
                default: None,
            });
        }
    }
    for instance in &struct_instance_names {
        if reserved.contains(instance) {
            continue;
        }
        let Some(state_struct) = state.struct_instances.get(instance) else {
            continue;
        };
        let Some(struct_def) =
            resolve_proc_state_struct_def(&proc.name, instance, state_struct, struct_defs, errors)
        else {
            continue;
        };
        let mut member_names = HashSet::<String>::new();
        for field in &struct_def.fields {
            let flat_name = nested_field_name(instance, &field.name);
            member_names.insert(field.name.clone());
            match &field.ty {
                FieldType::Scalar(prim) => {
                    fields.push(StructField {
                        name: flat_name,
                        ty: FieldType::Scalar(*prim),
                        default: None,
                    });
                }
                FieldType::Data(spec) => {
                    let _ = eval_data_size_expr(
                        &spec.size,
                        options,
                        &format!(
                            "processor '{}.{}' struct field '{}' Data size",
                            proc.name, instance, field.name
                        ),
                        errors,
                    );
                    fields.push(StructField {
                        name: flat_name,
                        ty: FieldType::Data(spec.clone()),
                        default: None,
                    });
                }
                FieldType::Generic(param) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}' state symbol '{}' field '{}' uses unresolved generic type parameter '{}'",
                            proc.name, instance, field.name, param
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
        instance_fields.insert(instance.clone(), member_names);
    }

    let field_names = fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<HashSet<_>>();
    let data_field_names = fields
        .iter()
        .filter_map(|f| match f.ty {
            FieldType::Data(_) => Some(f.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    ProcBaseShape {
        ins,
        outs,
        in_ports,
        param_specs,
        buffer_specs,
        in_types,
        out_types,
        in_array_slots,
        field_array_slots,
        nested_proc_array_slots,
        state,
        instance_fields,
        fields,
        field_names,
        data_field_names,
    }
}

pub(super) fn resolve_proc_state_struct_def(
    proc_name: &str,
    instance: &str,
    state_struct: &ProcStructState,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    errors: &mut Vec<Diagnostic>,
) -> Option<omni_frontend::StructDef> {
    let Some(struct_template) = struct_defs.get(&state_struct.struct_name) else {
        errors.push(Diagnostic::semantic(
            format!(
                "processor '{}' state symbol '{}' references unknown struct '{}'",
                proc_name, instance, state_struct.struct_name
            ),
            0,
            0,
        ));
        return None;
    };

    if state_struct.type_args.is_empty() {
        if !struct_template.type_params.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' state symbol '{}' uses generic struct '{}' without type arguments",
                    proc_name, instance, state_struct.struct_name
                ),
                0,
                0,
            ));
            return None;
        }
        return Some(struct_template.clone());
    }

    let Some(specialized) =
        specialize_generic_struct_template(struct_template, &state_struct.type_args, errors)
    else {
        return None;
    };
    Some(specialized)
}

pub(super) fn build_proc_lowering_shape(
    proc_name: &str,
    base_shapes: &HashMap<String, ProcBaseShape>,
    cache: &mut HashMap<String, ProcLoweringShape>,
    visiting: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcLoweringShape> {
    if let Some(cached) = cache.get(proc_name) {
        return Some(cached.clone());
    }

    if let Some(idx) = visiting.iter().position(|n| n == proc_name) {
        let mut cycle = visiting[idx..].to_vec();
        cycle.push(proc_name.to_owned());
        errors.push(Diagnostic::semantic(
            format!(
                "processor nested-state cycle detected: {}",
                cycle.join(" -> ")
            ),
            0,
            0,
        ));
        return None;
    }

    let Some(base) = base_shapes.get(proc_name).cloned() else {
        errors.push(Diagnostic::semantic(
            format!("unknown processor '{proc_name}'"),
            0,
            0,
        ));
        return None;
    };

    visiting.push(proc_name.to_owned());

    let mut fields = base.fields.clone();
    let mut field_names = base.field_names.clone();
    let mut data_field_names = base.data_field_names.clone();
    let mut field_array_slots = base.field_array_slots.clone();
    let mut nested_proc_array_slots = base.nested_proc_array_slots.clone();
    let mut nested_fields = base.instance_fields.clone();

    let mut nested_vars = base.state.nested_procs.keys().cloned().collect::<Vec<_>>();
    nested_vars.sort();

    for nested_var in nested_vars {
        let Some(nested_state) = base.state.nested_procs.get(&nested_var) else {
            continue;
        };

        let Some(callee_shape) = build_proc_lowering_shape(
            &nested_state.proc_name,
            base_shapes,
            cache,
            visiting,
            errors,
        ) else {
            continue;
        };

        nested_fields.insert(nested_var.clone(), callee_shape.field_names.clone());
        for (array_base, slots) in &callee_shape.field_array_slots {
            let prefixed_base = nested_field_name(&nested_var, array_base);
            let prefixed_slots = slots
                .iter()
                .map(|slot| nested_field_name(&nested_var, slot))
                .collect::<Vec<_>>();
            field_array_slots.insert(prefixed_base, prefixed_slots);
        }
        for (array_base, slots) in &callee_shape.nested_proc_array_slots {
            let prefixed_base = nested_field_name(&nested_var, array_base);
            let prefixed_slots = slots
                .iter()
                .map(|slot| nested_field_name(&nested_var, slot))
                .collect::<Vec<_>>();
            nested_proc_array_slots.insert(prefixed_base, prefixed_slots);
        }

        let mut nested_callee_fields = callee_shape.fields.clone();
        nested_callee_fields.sort_by(|a, b| a.name.cmp(&b.name));
        for mut nested_field in nested_callee_fields {
            let flat_name = nested_field_name(&nested_var, &nested_field.name);
            if field_names.contains(&flat_name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' nested field '{}' conflicts with existing field '{}'",
                        proc_name, nested_field.name, flat_name
                    ),
                    0,
                    0,
                ));
                continue;
            }
            nested_field.name = flat_name.clone();
            if matches!(nested_field.ty, FieldType::Data(_)) {
                data_field_names.insert(flat_name.clone());
            }
            field_names.insert(flat_name);
            fields.push(nested_field);
        }
    }

    let _ = visiting.pop();

    let resolved = ProcLoweringShape {
        ins: base.ins,
        outs: base.outs,
        in_ports: base.in_ports,
        param_specs: base.param_specs,
        buffer_specs: base.buffer_specs,
        in_types: base.in_types,
        out_types: base.out_types,
        in_array_slots: base.in_array_slots,
        field_array_slots,
        nested_proc_array_slots,
        state: base.state,
        fields,
        field_names,
        data_field_names,
        nested_fields,
    };
    cache.insert(proc_name.to_owned(), resolved.clone());
    Some(resolved)
}
