use super::super::*;
use super::*;

pub(in crate::orc_backend) unsafe fn build_init_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    const_arrays: &HashMap<String, TypedArrayInfo>,
    const_array_base_ptrs: &HashMap<String, LLVMValueRef>,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = LLVMPointerType(float_ptr_ty, 0);
    let fast_math_flags = fast_math_flags(fast_math);

    let mut arg_types = [i8_ptr_ty, i8_ptr_ty];

    let fn_name = CString::new("onda_init")
        .map_err(|_| Diagnostic::internal("invalid init function name"))?;
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

        let params_ptr = LLVMGetParam(fn_ref, 0);
        let state_ptr = LLVMGetParam(fn_ref, 1);

        let shared = build_proc_entry_lowering_metadata(typed)?;
        let storage = build_state_array_storage(
            builder,
            context,
            state_ptr,
            typed,
            &shared.state_layout,
            &shared.array_layout,
            "ORC init lowering",
        )?;

        let out_slots = HashMap::<String, OutSlot>::new();
        let out_array_base_ptrs = HashMap::<String, LLVMValueRef>::new();
        let input_index = HashMap::new();
        let input_types = HashMap::new();
        let input_arrays = HashMap::<String, TypedArrayInfo>::new();
        let buffer_index = HashMap::new();
        let buffer_elem_types = HashMap::new();
        let buffer_channels = HashMap::new();
        let buffer_mono = HashSet::<String>::new();
        let proc_slot_buffer_refs = HashMap::<Vec<u32>, ProcSlotBufferRefArrays>::new();
        let output_arrays = HashMap::<String, TypedArrayInfo>::new();

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
            buffer_ptrs: LLVMConstPointerNull(LLVMPointerType(i8_ptr_ty, 0)),
            buffer_frames_ptr: LLVMConstPointerNull(LLVMPointerType(i32_ty, 0)),
            buffer_channels_ptr: LLVMConstPointerNull(LLVMPointerType(i32_ty, 0)),
            buffer_samplerates_ptr: LLVMConstPointerNull(LLVMPointerType(float_ty, 0)),
            frame_idx: LLVMConstInt(i32_ty, 0, 0),
            state_slots: &storage.state_slots,
            array_base_ptrs: &storage.array_base_ptrs,
            out_slots: &out_slots,
            out_array_base_ptrs: &out_array_base_ptrs,
            input_index: &input_index,
            input_types: &input_types,
            input_arrays: &input_arrays,
            const_arrays,
            const_array_base_ptrs,
            buffer_index: &buffer_index,
            buffer_elem_types: &buffer_elem_types,
            buffer_channels: &buffer_channels,
            buffer_mono: &buffer_mono,
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
            allow_struct_ctor: true,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
            oversample_input_cache: None,
            loop_stack: Vec::new(),
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
            out_slot_ptr_array: None,
        };

        let mut locals = HashMap::new();
        let mut local_aliases = HashMap::new();
        let mut local_array_aliases = HashMap::new();
        let mut local_tuples = HashMap::new();
        for stmt in &typed.init {
            lower_stmt(
                stmt,
                &mut lctx,
                &mut locals,
                &mut local_aliases,
                &mut local_array_aliases,
                &mut local_tuples,
            )?;
        }

        for (_idx, name) in typed.state_vars.iter().enumerate() {
            let slot = storage.state_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing state slot for '{name}' in ORC init lowering"
                ))
            })?;
            let value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"init_state_out\0".as_ptr().cast(),
            );
            let (_state_ty, state_offset) = *shared.state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                slot.ty,
                b"init_state_out_ptr\0",
                b"init_state_out_ptr_cast\0",
            );
            LLVMBuildStore(builder, value, state_ptr_elt);
        }
        LLVMBuildRetVoid(builder);

        Ok(())
    })();
    LLVMDisposeBuilder(builder);
    result
}
