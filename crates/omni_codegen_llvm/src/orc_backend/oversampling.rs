use super::*;

const SINC_A1_COEFF: f64 = 0.039151597734460045;
const SINC_A2_COEFF: f64 = 0.3026468483284934;
const SINC_A3_COEFF: f64 = 0.6746159185469639;
const SINC_B1_COEFF: f64 = 0.1473771136010466;
const SINC_B2_COEFF: f64 = 0.48246854276970014;
const SINC_B3_COEFF: f64 = 0.8830050257693731;

#[derive(Debug, Clone)]
pub(super) struct SincStageLayout {
    pub(super) ty: PrimitiveType,
    pub(super) offset: usize,
}

#[derive(Debug, Clone)]
pub(super) struct TopLevelSincSlotLayout {
    pub(super) ty: PrimitiveType,
    pub(super) stages: Vec<SincStageLayout>,
}

#[derive(Debug, Clone)]
pub(super) struct TopLevelOversamplingLayout {
    pub(super) state_size_bytes: usize,
    pub(super) input_slots: HashMap<String, TopLevelSincSlotLayout>,
    pub(super) output_slots: HashMap<String, TopLevelSincSlotLayout>,
}

impl TopLevelOversamplingLayout {
    pub(super) fn empty(state_size_bytes: usize) -> Self {
        Self {
            state_size_bytes,
            input_slots: HashMap::new(),
            output_slots: HashMap::new(),
        }
    }
}

pub(super) fn oversampling_stage_count(factor: usize) -> usize {
    factor.max(1).trailing_zeros() as usize
}

pub(super) fn compute_top_level_oversampling_layout(
    typed: &TypedProgram,
    base_state_size_bytes: usize,
) -> Result<TopLevelOversamplingLayout, Diagnostic> {
    let factor = typed.sample_oversample_factor.max(1);
    if factor <= 1 {
        return Ok(TopLevelOversamplingLayout::empty(base_state_size_bytes));
    }

    let stage_count = oversampling_stage_count(factor);
    let mut offset = base_state_size_bytes;
    let mut input_slots = HashMap::new();
    let mut output_slots = HashMap::new();

    let mut append_slot_layout = |name: &str,
                                  ty: PrimitiveType,
                                  target: &mut HashMap<String, TopLevelSincSlotLayout>|
     -> Result<(), Diagnostic> {
        if !matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
            return Ok(());
        }
        let (elem_size, elem_align) = primitive_size_align(ty);
        let stage_bytes = elem_size.checked_mul(8).ok_or_else(|| {
            Diagnostic::internal(format!(
                "oversampling state size overflow for '{name}' in ORC layout"
            ))
        })?;
        let mut stages = Vec::with_capacity(stage_count);
        for _ in 0..stage_count {
            offset = align_up(offset, elem_align);
            stages.push(SincStageLayout { ty, offset });
            offset = offset.checked_add(stage_bytes).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "oversampling state byte offset overflow for '{name}' in ORC layout"
                ))
            })?;
        }
        target.insert(name.to_owned(), TopLevelSincSlotLayout { ty, stages });
        Ok(())
    };

    for name in &typed.ins {
        append_slot_layout(
            name,
            *typed.in_types.get(name).unwrap_or(&PrimitiveType::F32),
            &mut input_slots,
        )?;
    }
    for name in &typed.outs {
        append_slot_layout(
            name,
            *typed.out_types.get(name).unwrap_or(&PrimitiveType::F32),
            &mut output_slots,
        )?;
    }

    Ok(TopLevelOversamplingLayout {
        state_size_bytes: offset,
        input_slots,
        output_slots,
    })
}

pub(super) unsafe fn build_contiguous_sinc_tap_ptrs(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    ty: PrimitiveType,
    stage_base_ptr: LLVMValueRef,
) -> [LLVMValueRef; 8] {
    let llvm_ty = llvm_ty_for_primitive(context, ty);
    std::array::from_fn(|index| {
        let index_v = LLVMConstInt(LLVMInt32TypeInContext(context), index as u64, 0);
        build_ptr_offset(
            builder,
            llvm_ty,
            stage_base_ptr,
            index_v,
            b"os_sinc_tap_ptr\0",
        )
    })
}

pub(super) unsafe fn build_sinc_interpolate(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    ty: PrimitiveType,
    tap_ptrs: &[LLVMValueRef; 8],
    input: LLVMValueRef,
    fast_math_flags: LLVMFastMathFlags,
) -> (LLVMValueRef, LLVMValueRef) {
    let llvm_ty = llvm_ty_for_primitive(context, ty);
    let a0 = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[0],
        b"os_sinc_a0\0".as_ptr().cast(),
    );
    let a1_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[1],
        b"os_sinc_a1\0".as_ptr().cast(),
    );
    let a2_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[2],
        b"os_sinc_a2\0".as_ptr().cast(),
    );
    let a3_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[3],
        b"os_sinc_a3\0".as_ptr().cast(),
    );
    let b0 = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[4],
        b"os_sinc_b0\0".as_ptr().cast(),
    );
    let b1_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[5],
        b"os_sinc_b1\0".as_ptr().cast(),
    );
    let b2_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[6],
        b"os_sinc_b2\0".as_ptr().cast(),
    );
    let b3_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[7],
        b"os_sinc_b3\0".as_ptr().cast(),
    );

    let a1 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        a0,
        input,
        a1_old,
        SINC_A1_COEFF,
        b"os_sinc_a1_new\0",
        fast_math_flags,
    );
    let a2 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        a1_old,
        a1,
        a2_old,
        SINC_A2_COEFF,
        b"os_sinc_a2_new\0",
        fast_math_flags,
    );
    let a3 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        a2_old,
        a2,
        a3_old,
        SINC_A3_COEFF,
        b"os_sinc_a3_new\0",
        fast_math_flags,
    );

    let b1 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        b0,
        input,
        b1_old,
        SINC_B1_COEFF,
        b"os_sinc_b1_new\0",
        fast_math_flags,
    );
    let b2 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        b1_old,
        b1,
        b2_old,
        SINC_B2_COEFF,
        b"os_sinc_b2_new\0",
        fast_math_flags,
    );
    let b3 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        b2_old,
        b2,
        b3_old,
        SINC_B3_COEFF,
        b"os_sinc_b3_new\0",
        fast_math_flags,
    );

    LLVMBuildStore(builder, input, tap_ptrs[0]);
    LLVMBuildStore(builder, a1, tap_ptrs[1]);
    LLVMBuildStore(builder, a2, tap_ptrs[2]);
    LLVMBuildStore(builder, a3, tap_ptrs[3]);
    LLVMBuildStore(builder, input, tap_ptrs[4]);
    LLVMBuildStore(builder, b1, tap_ptrs[5]);
    LLVMBuildStore(builder, b2, tap_ptrs[6]);
    LLVMBuildStore(builder, b3, tap_ptrs[7]);

    (a3, b3)
}

pub(super) unsafe fn build_sinc_decimate(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    ty: PrimitiveType,
    tap_ptrs: &[LLVMValueRef; 8],
    in1: LLVMValueRef,
    in2: LLVMValueRef,
    fast_math_flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let llvm_ty = llvm_ty_for_primitive(context, ty);
    let a0 = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[0],
        b"os_sinc_a0\0".as_ptr().cast(),
    );
    let a1_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[1],
        b"os_sinc_a1\0".as_ptr().cast(),
    );
    let a2_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[2],
        b"os_sinc_a2\0".as_ptr().cast(),
    );
    let a3_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[3],
        b"os_sinc_a3\0".as_ptr().cast(),
    );
    let b0 = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[4],
        b"os_sinc_b0\0".as_ptr().cast(),
    );
    let b1_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[5],
        b"os_sinc_b1\0".as_ptr().cast(),
    );
    let b2_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[6],
        b"os_sinc_b2\0".as_ptr().cast(),
    );
    let b3_old = LLVMBuildLoad2(
        builder,
        llvm_ty,
        tap_ptrs[7],
        b"os_sinc_b3\0".as_ptr().cast(),
    );

    let a1 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        a0,
        in2,
        a1_old,
        SINC_A1_COEFF,
        b"os_sinc_a1_new\0",
        fast_math_flags,
    );
    let a2 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        a1_old,
        a1,
        a2_old,
        SINC_A2_COEFF,
        b"os_sinc_a2_new\0",
        fast_math_flags,
    );
    let a3 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        a2_old,
        a2,
        a3_old,
        SINC_A3_COEFF,
        b"os_sinc_a3_new\0",
        fast_math_flags,
    );

    let b1 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        b0,
        in1,
        b1_old,
        SINC_B1_COEFF,
        b"os_sinc_b1_new\0",
        fast_math_flags,
    );
    let b2 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        b1_old,
        b1,
        b2_old,
        SINC_B2_COEFF,
        b"os_sinc_b2_new\0",
        fast_math_flags,
    );
    let b3 = build_sinc_multiply_add(
        builder,
        context,
        ty,
        b2_old,
        b2,
        b3_old,
        SINC_B3_COEFF,
        b"os_sinc_b3_new\0",
        fast_math_flags,
    );

    LLVMBuildStore(builder, in2, tap_ptrs[0]);
    LLVMBuildStore(builder, a1, tap_ptrs[1]);
    LLVMBuildStore(builder, a2, tap_ptrs[2]);
    LLVMBuildStore(builder, a3, tap_ptrs[3]);
    LLVMBuildStore(builder, in1, tap_ptrs[4]);
    LLVMBuildStore(builder, b1, tap_ptrs[5]);
    LLVMBuildStore(builder, b2, tap_ptrs[6]);
    LLVMBuildStore(builder, b3, tap_ptrs[7]);

    let sum = build_fadd_fast(builder, a3, b3, b"os_sinc_decim_sum\0", fast_math_flags);
    let half = LLVMConstReal(llvm_ty, 0.5);
    build_fmul_fast(builder, sum, half, b"os_sinc_decim_half\0", fast_math_flags)
}

pub(super) unsafe fn store_cached_oversampled_input(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    cache_words_ptr: LLVMValueRef,
    channel_index: LLVMValueRef,
    ty: PrimitiveType,
    value: LLVMValueRef,
) {
    let word_ty = LLVMInt64TypeInContext(context);
    let word_ptr = build_ptr_offset(
        builder,
        word_ty,
        cache_words_ptr,
        channel_index,
        b"os_input_cache_word_ptr\0",
    );
    let typed_ptr = LLVMBuildBitCast(
        builder,
        word_ptr,
        LLVMPointerType(llvm_ty_for_primitive(context, ty), 0),
        b"os_input_cache_typed_ptr\0".as_ptr().cast(),
    );
    LLVMBuildStore(builder, value, typed_ptr);
}

pub(super) unsafe fn load_cached_oversampled_input(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    cache_words_ptr: LLVMValueRef,
    channel_index: LLVMValueRef,
    ty: PrimitiveType,
) -> LLVMValueRef {
    let word_ty = LLVMInt64TypeInContext(context);
    let word_ptr = build_ptr_offset(
        builder,
        word_ty,
        cache_words_ptr,
        channel_index,
        b"os_input_cache_word_ptr\0",
    );
    let typed_ptr = LLVMBuildBitCast(
        builder,
        word_ptr,
        LLVMPointerType(llvm_ty_for_primitive(context, ty), 0),
        b"os_input_cache_typed_ptr\0".as_ptr().cast(),
    );
    LLVMBuildLoad2(
        builder,
        llvm_ty_for_primitive(context, ty),
        typed_ptr,
        b"os_input_cache_load\0".as_ptr().cast(),
    )
}

unsafe fn build_sinc_multiply_add(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    ty: PrimitiveType,
    accum: LLVMValueRef,
    input: LLVMValueRef,
    history: LLVMValueRef,
    coeff: f64,
    name: &[u8],
    fast_math_flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let coeff_value = LLVMConstReal(llvm_ty_for_primitive(context, ty), coeff);
    let delta = build_fsub_fast(builder, input, history, b"os_sinc_delta\0", fast_math_flags);
    let scaled = build_fmul_fast(
        builder,
        delta,
        coeff_value,
        b"os_sinc_scaled\0",
        fast_math_flags,
    );
    build_fadd_fast(builder, accum, scaled, name, fast_math_flags)
}
