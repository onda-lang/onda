use super::super::*;

pub(in crate::orc_backend) struct ProcEntryLoweringMetadata {
    pub(in crate::orc_backend) param_byte_offset: HashMap<String, usize>,
    pub(in crate::orc_backend) param_types: HashMap<String, PrimitiveType>,
    pub(in crate::orc_backend) state_layout: HashMap<String, (PrimitiveType, usize)>,
    pub(in crate::orc_backend) array_layout: HashMap<String, (PrimitiveType, usize)>,
    pub(in crate::orc_backend) array_struct_roots: HashMap<String, String>,
    pub(in crate::orc_backend) array_struct_len: HashMap<String, usize>,
    pub(in crate::orc_backend) struct_fields: HashMap<String, Vec<TypedStructField>>,
}

pub(in crate::orc_backend) struct ProcStateArrayStorage {
    pub(in crate::orc_backend) array_base_ptrs: HashMap<String, LLVMValueRef>,
    pub(in crate::orc_backend) array_len: HashMap<String, usize>,
    pub(in crate::orc_backend) array_elem_ty: HashMap<String, PrimitiveType>,
    pub(in crate::orc_backend) state_slots: HashMap<String, StateSlot>,
}

pub(in crate::orc_backend) struct BufferBindingMetadata {
    pub(in crate::orc_backend) buffer_index: HashMap<String, u32>,
    pub(in crate::orc_backend) buffer_elem_types: HashMap<String, PrimitiveType>,
    pub(in crate::orc_backend) buffer_channels: HashMap<String, TypedBufferChannels>,
    pub(in crate::orc_backend) buffer_mono: HashSet<String>,
}

pub(in crate::orc_backend) fn build_proc_entry_lowering_metadata(
    typed: &TypedProgram,
) -> Result<ProcEntryLoweringMetadata, Diagnostic> {
    let arrays_by_offset = typed
        .param_arrays
        .iter()
        .map(|(name, info)| (info.offset, name.as_str()))
        .collect::<HashMap<_, _>>();
    let mut param_byte_offset = HashMap::new();
    let mut running_param_bytes = 0usize;
    for (slot_idx, param) in typed.params.iter().enumerate() {
        if let Some(base_name) = arrays_by_offset.get(&slot_idx) {
            param_byte_offset.insert((*base_name).to_owned(), running_param_bytes);
        }
        param_byte_offset.insert(param.name.clone(), running_param_bytes);
        running_param_bytes = running_param_bytes.saturating_add(primitive_type_bytes(param.ty));
    }

    let param_types = typed
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty))
        .collect::<HashMap<_, _>>();

    let state_layout_entries = compute_state_layout(typed)?;
    let state_layout = state_layout_map(&state_layout_entries);
    let array_layout_entries = compute_arrays_layout(typed, &state_layout_entries)?;
    let array_layout = arrays_layout_map(&array_layout_entries);

    let array_struct_roots = typed
        .array_struct_roots
        .iter()
        .map(|root| (root.name.clone(), root.struct_name.clone()))
        .collect::<HashMap<_, _>>();
    let array_struct_len = typed
        .array_struct_roots
        .iter()
        .map(|root| (root.name.clone(), root.len))
        .collect::<HashMap<_, _>>();
    let struct_fields = typed
        .structs
        .iter()
        .map(|s| (s.name.clone(), s.fields.clone()))
        .collect::<HashMap<_, _>>();

    Ok(ProcEntryLoweringMetadata {
        param_byte_offset,
        param_types,
        state_layout,
        array_layout,
        array_struct_roots,
        array_struct_len,
        struct_fields,
    })
}

pub(in crate::orc_backend) unsafe fn build_state_array_storage(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    state_ptr: LLVMValueRef,
    typed: &TypedProgram,
    state_layout: &HashMap<String, (PrimitiveType, usize)>,
    array_layout: &HashMap<String, (PrimitiveType, usize)>,
    diag_context: &str,
) -> Result<ProcStateArrayStorage, Diagnostic> {
    let mut array_base_ptrs = HashMap::new();
    let mut array_len = HashMap::new();
    let mut array_elem_ty = HashMap::new();
    for array_var in &typed.array_vars {
        let (_, offset) = *array_layout.get(&array_var.name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing array layout metadata for '{}' in {diag_context}",
                array_var.name
            ))
        })?;
        let ptr = build_typed_state_ptr(
            builder,
            context,
            state_ptr,
            offset,
            array_var.elem_ty,
            b"entry_arr_state_ptr\0",
            b"entry_arr_state_ptr_cast\0",
        );
        array_base_ptrs.insert(array_var.name.clone(), ptr);
        array_len.insert(array_var.name.clone(), array_var.len);
        array_elem_ty.insert(array_var.name.clone(), array_var.elem_ty);
    }

    let mut state_slots = HashMap::new();
    for (idx, name) in typed.state_vars.iter().enumerate() {
        let (state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing state layout metadata for '{name}' in {diag_context}"
            ))
        })?;
        let slot_name = CString::new(format!("entry_state_{idx}"))
            .map_err(|_| Diagnostic::internal("invalid state slot name"))?;
        let slot = LLVMBuildAlloca(
            builder,
            llvm_ty_for_primitive(context, state_ty),
            slot_name.as_ptr(),
        );
        let state_ptr_elt = build_typed_state_ptr(
            builder,
            context,
            state_ptr,
            state_offset,
            state_ty,
            b"entry_state_ptr\0",
            b"entry_state_ptr_cast\0",
        );
        let state_load = LLVMBuildLoad2(
            builder,
            llvm_ty_for_primitive(context, state_ty),
            state_ptr_elt,
            b"entry_state_load\0".as_ptr().cast(),
        );
        LLVMBuildStore(builder, state_load, slot);
        state_slots.insert(
            name.clone(),
            StateSlot {
                ptr: slot,
                ty: state_ty,
            },
        );
    }

    Ok(ProcStateArrayStorage {
        array_base_ptrs,
        array_len,
        array_elem_ty,
        state_slots,
    })
}

pub(in crate::orc_backend) fn build_buffer_binding_metadata(
    typed: &TypedProgram,
) -> BufferBindingMetadata {
    let mut buffer_index = HashMap::new();
    let mut buffer_elem_types = HashMap::new();
    let mut buffer_channels = HashMap::new();
    let mut buffer_mono = HashSet::new();
    for (idx, decl) in typed.buffers.iter().enumerate() {
        buffer_index.insert(decl.name.clone(), idx as u32);
        buffer_elem_types.insert(decl.name.clone(), decl.elem_ty);
        buffer_channels.insert(decl.name.clone(), decl.channels.clone());
        let mono = match decl.channels {
            TypedBufferChannels::Mono => true,
            TypedBufferChannels::Static(ch) => ch <= 1,
            TypedBufferChannels::Dynamic => false,
        };
        if mono {
            buffer_mono.insert(decl.name.clone());
        }
    }
    BufferBindingMetadata {
        buffer_index,
        buffer_elem_types,
        buffer_channels,
        buffer_mono,
    }
}
