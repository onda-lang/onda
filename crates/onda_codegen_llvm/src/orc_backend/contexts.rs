use super::*;

#[derive(Clone, Copy)]
pub(super) struct OrcValue {
    pub(super) value: LLVMValueRef,
    pub(super) ty: PrimitiveType,
}

#[derive(Clone, Copy)]
pub(super) struct ArrayElementPtr {
    pub(super) ptr: LLVMValueRef,
    pub(super) elem_ty: PrimitiveType,
}

#[derive(Clone)]
pub(super) enum LocalArrayAlias {
    Primitive {
        base_ptr: LLVMValueRef,
        len: usize,
        elem_ty: PrimitiveType,
    },
    Struct {
        root_base: String,
        elem_struct: String,
        len: usize,
        start_index: LLVMValueRef,
    },
}

#[derive(Clone, Copy)]
pub(super) struct DefLocalSlot {
    pub(super) ptr: LLVMValueRef,
    pub(super) ty: PrimitiveType,
}

#[derive(Clone)]
pub(super) struct DefTupleSlot {
    pub(super) ptr: LLVMValueRef,
    pub(super) elem_tys: Vec<PrimitiveType>,
}

#[derive(Clone, Copy)]
pub(super) struct StateSlot {
    pub(super) ptr: LLVMValueRef,
    pub(super) ty: PrimitiveType,
}

#[derive(Clone, Copy)]
pub(super) struct OutSlot {
    pub(super) ptr: LLVMValueRef,
    pub(super) ty: PrimitiveType,
}

#[derive(Clone, Copy)]
pub(super) struct AliasSlot {
    pub(super) ptr: LLVMValueRef,
    pub(super) ty: PrimitiveType,
}

#[derive(Clone, Copy)]
pub(super) struct ProcSlotBufferRefArrays {
    pub(super) ptrs_base: LLVMValueRef,
    pub(super) frames_base: LLVMValueRef,
    pub(super) channels_base: LLVMValueRef,
    pub(super) samplerates_base: LLVMValueRef,
    pub(super) len: usize,
}

#[derive(Clone, Copy)]
pub(super) struct LoopControl {
    pub(super) break_bb: LLVMBasicBlockRef,
    pub(super) continue_bb: LLVMBasicBlockRef,
}

pub(super) struct LoweringCtx<'a> {
    pub(super) builder: LLVMBuilderRef,
    pub(super) context: LLVMContextRef,
    pub(super) module: LLVMModuleRef,
    pub(super) fn_ref: LLVMValueRef,
    pub(super) float_ty: LLVMTypeRef,
    pub(super) float_ptr_ty: LLVMTypeRef,
    pub(super) i32_ty: LLVMTypeRef,
    pub(super) fast_math_flags: LLVMFastMathFlags,
    pub(super) sample_rate: f32,
    pub(super) block_size: f32,
    pub(super) in_ptrs: LLVMValueRef,
    pub(super) params_ptr: LLVMValueRef,
    pub(super) buffer_ptrs: LLVMValueRef,
    pub(super) buffer_frames_ptr: LLVMValueRef,
    pub(super) buffer_channels_ptr: LLVMValueRef,
    pub(super) buffer_samplerates_ptr: LLVMValueRef,
    pub(super) frame_idx: LLVMValueRef,
    pub(super) state_slots: &'a HashMap<String, StateSlot>,
    pub(super) array_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    pub(super) out_slots: &'a HashMap<String, OutSlot>,
    pub(super) out_array_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    pub(super) input_index: &'a HashMap<String, u32>,
    pub(super) input_types: &'a HashMap<String, PrimitiveType>,
    pub(super) input_arrays: &'a HashMap<String, TypedArrayInfo>,
    pub(super) const_arrays: &'a HashMap<String, TypedArrayInfo>,
    pub(super) const_array_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    pub(super) buffer_index: &'a HashMap<String, u32>,
    pub(super) buffer_elem_types: &'a HashMap<String, PrimitiveType>,
    pub(super) buffer_channels: &'a HashMap<String, TypedBufferChannels>,
    pub(super) buffer_mono: &'a HashSet<String>,
    pub(super) proc_slot_buffer_refs: &'a HashMap<Vec<u32>, ProcSlotBufferRefArrays>,
    pub(super) param_byte_offset: &'a HashMap<String, usize>,
    pub(super) param_types: &'a HashMap<String, PrimitiveType>,
    pub(super) param_arrays: &'a HashMap<String, TypedArrayInfo>,
    pub(super) output_arrays: &'a HashMap<String, TypedArrayInfo>,
    pub(super) array_len: &'a HashMap<String, usize>,
    pub(super) array_len_values: HashMap<String, LLVMValueRef>,
    pub(super) array_elem_ty: &'a HashMap<String, PrimitiveType>,
    pub(super) array_struct_roots: &'a HashMap<String, String>,
    pub(super) array_struct_len: &'a HashMap<String, usize>,
    pub(super) struct_fields: &'a HashMap<String, Vec<TypedStructField>>,
    pub(super) allow_struct_ctor: bool,
    pub(super) user_fn_param_names: &'a HashMap<String, Vec<String>>,
    pub(super) user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    pub(super) user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    pub(super) user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    pub(super) user_registry: *const UserFnRegistry,
    pub(super) oversample_input_cache: Option<LLVMValueRef>,
    pub(super) loop_stack: Vec<LoopControl>,
    pub(super) port_index_ins: Option<PortIndexMeta>,
    pub(super) port_index_outs: Option<PortIndexMeta>,
    pub(super) port_index_params: Option<PortIndexMeta>,
    #[allow(dead_code)]
    pub(super) out_ptrs: LLVMValueRef,
    pub(super) out_slot_ptr_array: Option<LLVMValueRef>,
}

#[derive(Clone, Copy)]
pub(super) struct PortIndexMeta {
    pub(super) count: usize,
    pub(super) elem_ty: PrimitiveType,
}

pub(super) struct UserFnRegistry {
    pub(super) defs: HashMap<String, TypedFunction>,
    pub(super) struct_fields: HashMap<String, Vec<TypedStructField>>,
    pub(super) sample_oversample_factors: HashMap<String, usize>,
    pub(super) proc_step_oversample_meta: HashMap<String, ProcStepOversampleMeta>,
    pub(super) refs: HashMap<String, LLVMValueRef>,
    pub(super) base_return_tys: HashMap<String, ReturnType>,
    pub(super) mono_refs: HashMap<String, LLVMValueRef>,
    pub(super) mono_tys: HashMap<String, LLVMTypeRef>,
    pub(super) mono_return_tys: HashMap<String, ReturnType>,
    pub(super) param_names: HashMap<String, Vec<String>>,
    pub(super) param_defaults: HashMap<String, Vec<Option<Expr>>>,
    pub(super) param_kinds: HashMap<String, Vec<TypedFnParam>>,
    pub(super) param_by_ref: HashMap<String, Vec<bool>>,
    pub(super) const_arrays: HashMap<String, TypedArrayInfo>,
    pub(super) const_array_base_ptrs: HashMap<String, LLVMValueRef>,
    pub(super) in_progress: HashSet<String>,
    pub(super) return_in_progress: HashSet<String>,
}

pub(super) struct DefLoweringCtx<'a> {
    pub(super) builder: LLVMBuilderRef,
    pub(super) context: LLVMContextRef,
    pub(super) module: LLVMModuleRef,
    pub(super) fn_ref: LLVMValueRef,
    pub(super) float_ty: LLVMTypeRef,
    pub(super) i32_ty: LLVMTypeRef,
    pub(super) fast_math_flags: LLVMFastMathFlags,
    pub(super) sample_rate: f32,
    pub(super) block_size: f32,
    pub(super) return_ty: ReturnType,
    pub(super) return_slot: LLVMValueRef,
    pub(super) return_block: LLVMBasicBlockRef,
    pub(super) local_slots: HashMap<String, DefLocalSlot>,
    pub(super) tuple_slots: HashMap<String, DefTupleSlot>,
    pub(super) local_array_aliases: HashMap<String, LocalArrayAlias>,
    pub(super) buffer_params: HashMap<String, DefBufferParamInfo>,
    pub(super) array_ptrs: HashMap<String, LLVMValueRef>,
    pub(super) array_len: HashMap<String, usize>,
    pub(super) array_len_values: HashMap<String, LLVMValueRef>,
    pub(super) array_elem_ty: HashMap<String, PrimitiveType>,
    pub(super) array_struct_roots: HashMap<String, String>,
    pub(super) const_array_names: HashSet<String>,
    pub(super) struct_fields: &'a HashMap<String, Vec<TypedStructField>>,
    pub(super) user_fn_param_names: &'a HashMap<String, Vec<String>>,
    pub(super) user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    pub(super) user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    pub(super) user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    pub(super) user_registry: *const UserFnRegistry,
    pub(super) loop_stack: Vec<LoopControl>,
}

#[derive(Clone)]
pub(super) struct DefBufferParamInfo {
    pub(super) ptr: LLVMValueRef,
    pub(super) frames: LLVMValueRef,
    pub(super) channels: LLVMValueRef,
    pub(super) sample_rate: LLVMValueRef,
    pub(super) elem_ty: PrimitiveType,
    pub(super) declared_channels: TypedBufferChannels,
}
