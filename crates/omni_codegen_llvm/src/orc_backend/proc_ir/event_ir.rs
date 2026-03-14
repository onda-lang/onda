use super::super::*;
use super::*;

pub(in crate::orc_backend) unsafe fn build_event_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i32_ptr_ty = LLVMPointerType(i32_ty, 0);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = LLVMPointerType(float_ptr_ty, 0);
    let i8_ptr_ptr_ty = LLVMPointerType(i8_ptr_ty, 0);
    let fast_math_flags = fast_math_flags(fast_math);

    if typed.events.is_empty() {
        return Ok(());
    }

    let shared = build_proc_entry_lowering_metadata(typed)?;
    let buffer_meta = build_buffer_binding_metadata(typed);

    for (event_idx, event) in typed.events.iter().enumerate() {
        let float_ptr_ty_param = LLVMPointerType(float_ty, 0);
        let mut arg_types = [
            i8_ptr_ty,
            i8_ptr_ty,
            i8_ptr_ty,
            i8_ptr_ptr_ty,
            i32_ptr_ty,
            i32_ptr_ty,
            float_ptr_ty_param,
        ];
        let fn_name = CString::new(format!("omni_event_{event_idx}"))
            .map_err(|_| Diagnostic::internal("invalid event function name"))?;
        let fn_ty = LLVMFunctionType(void_ty, arg_types.as_mut_ptr(), arg_types.len() as u32, 0);
        let fn_ref = LLVMAddFunction(module, fn_name.as_ptr(), fn_ty);

        let entry_name =
            CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
        let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());

        let builder = LLVMCreateBuilderInContext(context);
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLVM builder"));
        }
        let result = (|| -> Result<(), Diagnostic> {
            LLVMPositionBuilderAtEnd(builder, entry);

            let payload_ptr = LLVMGetParam(fn_ref, 0);
            let params_ptr = LLVMGetParam(fn_ref, 1);
            let state_ptr = LLVMGetParam(fn_ref, 2);
            let buffer_ptrs = LLVMGetParam(fn_ref, 3);
            let buffer_frames_ptr = LLVMGetParam(fn_ref, 4);
            let buffer_channels_ptr = LLVMGetParam(fn_ref, 5);
            let buffer_samplerates_ptr = LLVMGetParam(fn_ref, 6);

            let storage = build_state_array_storage(
                builder,
                context,
                state_ptr,
                typed,
                &shared.state_layout,
                &shared.array_layout,
                "ORC event lowering",
            )?;

            let out_slots = HashMap::<String, OutSlot>::new();
            let out_array_base_ptrs = HashMap::<String, LLVMValueRef>::new();
            let input_index = HashMap::new();
            let input_types = HashMap::new();
            let input_arrays = HashMap::<String, TypedArrayInfo>::new();
            let output_arrays = HashMap::<String, TypedArrayInfo>::new();
            let proc_slot_buffer_refs = HashMap::<Vec<u32>, ProcSlotBufferRefArrays>::new();

            let mut lctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                fast_math_flags,
                sample_rate,
                block_size: block_size as f32,
                in_ptrs: LLVMConstPointerNull(float_ptr_ptr_ty),
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                buffer_samplerates_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &storage.state_slots,
                array_base_ptrs: &storage.array_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &input_arrays,
                buffer_index: &buffer_meta.buffer_index,
                buffer_elem_types: &buffer_meta.buffer_elem_types,
                buffer_channels: &buffer_meta.buffer_channels,
                buffer_mono: &buffer_meta.buffer_mono,
                proc_slot_buffer_refs: &proc_slot_buffer_refs,
                param_byte_offset: &shared.param_byte_offset,
                param_types: &shared.param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &output_arrays,
                array_len: &storage.array_len,
                array_len_values: HashMap::new(),
                array_elem_ty: &storage.array_elem_ty,
                array_struct_roots: &shared.array_struct_roots,
                array_struct_len: &shared.array_struct_len,
                struct_fields: &shared.struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
                oversample_input_cache: None,
                loop_stack: Vec::new(),
            };

            let mut locals = HashMap::<String, OrcValue>::new();
            let mut local_aliases = HashMap::new();
            let mut local_array_aliases = HashMap::new();
            let mut payload_offset = const_i32(i32_ty, 0);
            for param in &event.params {
                match param.ty {
                    TypedEventParamType::Scalar(ty) => {
                        let param_ptr = build_typed_ptr_from_byte_offset(
                            builder,
                            context,
                            payload_ptr,
                            payload_offset,
                            ty,
                            b"evt_param_ptr_i8\0",
                            b"evt_param_ptr_typed\0",
                        );
                        let loaded = LLVMBuildLoad2(
                            builder,
                            llvm_ty_for_primitive(context, ty),
                            param_ptr,
                            b"evt_param_load\0".as_ptr().cast(),
                        );
                        locals.insert(param.name.clone(), OrcValue { value: loaded, ty });
                        payload_offset = LLVMBuildAdd(
                            builder,
                            payload_offset,
                            const_i32(i32_ty, primitive_type_bytes(ty) as i32),
                            b"evt_payload_off_next\0".as_ptr().cast(),
                        );
                    }
                    TypedEventParamType::Array { elem, len } => {
                        let base_ptr = build_typed_ptr_from_byte_offset(
                            builder,
                            context,
                            payload_ptr,
                            payload_offset,
                            elem,
                            b"evt_arr_ptr_i8\0",
                            b"evt_arr_ptr_typed\0",
                        );
                        local_array_aliases.insert(
                            param.name.clone(),
                            LocalArrayAlias::Primitive {
                                base_ptr,
                                len,
                                elem_ty: elem,
                            },
                        );
                        payload_offset = LLVMBuildAdd(
                            builder,
                            payload_offset,
                            const_i32(
                                i32_ty,
                                primitive_type_bytes(elem).saturating_mul(len) as i32,
                            ),
                            b"evt_payload_off_next\0".as_ptr().cast(),
                        );
                    }
                    TypedEventParamType::Slice { elem } => {
                        let len_ptr = build_typed_ptr_from_byte_offset(
                            builder,
                            context,
                            payload_ptr,
                            payload_offset,
                            PrimitiveType::I32,
                            b"evt_slice_len_i8\0",
                            b"evt_slice_len_ptr\0",
                        );
                        let len_val = LLVMBuildLoad2(
                            builder,
                            llvm_ty_for_primitive(context, PrimitiveType::I32),
                            len_ptr,
                            b"evt_slice_len\0".as_ptr().cast(),
                        );
                        let data_offset = LLVMBuildAdd(
                            builder,
                            payload_offset,
                            const_i32(i32_ty, std::mem::size_of::<i32>() as i32),
                            b"evt_slice_data_off\0".as_ptr().cast(),
                        );
                        let base_ptr = build_typed_ptr_from_byte_offset(
                            builder,
                            context,
                            payload_ptr,
                            data_offset,
                            elem,
                            b"evt_slice_ptr_i8\0",
                            b"evt_slice_ptr_typed\0",
                        );
                        local_array_aliases.insert(
                            param.name.clone(),
                            LocalArrayAlias::Primitive {
                                base_ptr,
                                len: 1,
                                elem_ty: elem,
                            },
                        );
                        lctx.array_len_values.insert(param.name.clone(), len_val);
                        let slice_bytes = LLVMBuildMul(
                            builder,
                            len_val,
                            const_i32(i32_ty, primitive_type_bytes(elem) as i32),
                            b"evt_slice_bytes\0".as_ptr().cast(),
                        );
                        payload_offset = LLVMBuildAdd(
                            builder,
                            data_offset,
                            slice_bytes,
                            b"evt_payload_off_next\0".as_ptr().cast(),
                        );
                    }
                }
            }

            for stmt in &event.body {
                lower_stmt(
                    stmt,
                    &mut lctx,
                    &mut locals,
                    &mut local_aliases,
                    &mut local_array_aliases,
                )?;
            }

            for name in &typed.state_vars {
                let slot = storage.state_slots.get(name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing state slot for '{name}' in ORC event lowering"
                    ))
                })?;
                let value = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, slot.ty),
                    slot.ptr,
                    b"evt_state_out\0".as_ptr().cast(),
                );
                let (_state_ty, state_offset) =
                    *shared.state_layout.get(name).ok_or_else(|| {
                        Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
                    })?;
                let state_ptr_elt = build_typed_state_ptr(
                    builder,
                    context,
                    state_ptr,
                    state_offset,
                    slot.ty,
                    b"evt_state_out_ptr\0",
                    b"evt_state_out_ptr_cast\0",
                );
                LLVMBuildStore(builder, value, state_ptr_elt);
            }

            LLVMBuildRetVoid(builder);
            Ok(())
        })();
        LLVMDisposeBuilder(builder);
        result?;
    }

    Ok(())
}
