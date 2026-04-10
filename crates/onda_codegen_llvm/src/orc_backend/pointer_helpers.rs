use super::*;

pub(super) unsafe fn build_f32_ptr_offset(
    builder: LLVMBuilderRef,
    float_ty: LLVMTypeRef,
    base_ptr: LLVMValueRef,
    index: LLVMValueRef,
    name: &[u8],
) -> LLVMValueRef {
    build_ptr_offset(builder, float_ty, base_ptr, index, name)
}

pub(super) unsafe fn build_ptr_offset(
    builder: LLVMBuilderRef,
    elem_ty: LLVMTypeRef,
    base_ptr: LLVMValueRef,
    index: LLVMValueRef,
    name: &[u8],
) -> LLVMValueRef {
    let mut indices = [index];
    LLVMBuildGEP2(
        builder,
        elem_ty,
        base_ptr,
        indices.as_mut_ptr(),
        1,
        name.as_ptr().cast(),
    )
}

pub(super) unsafe fn build_i8_ptr_offset(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    base_ptr: LLVMValueRef,
    offset_bytes: usize,
    name: &[u8],
) -> LLVMValueRef {
    let i8_ty = LLVMInt8TypeInContext(context);
    let mut indices = [LLVMConstInt(
        LLVMInt64TypeInContext(context),
        offset_bytes as u64,
        0,
    )];
    LLVMBuildGEP2(
        builder,
        i8_ty,
        base_ptr,
        indices.as_mut_ptr(),
        1,
        name.as_ptr().cast(),
    )
}

pub(super) unsafe fn build_typed_ptr_from_byte_offset(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    base_ptr: LLVMValueRef,
    offset_bytes_i32: LLVMValueRef,
    ty: PrimitiveType,
    i8_name: &[u8],
    typed_name: &[u8],
) -> LLVMValueRef {
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let base_i8_ptr = LLVMBuildBitCast(builder, base_ptr, i8_ptr_ty, i8_name.as_ptr().cast());
    let byte_ptr = build_ptr_offset(builder, i8_ty, base_i8_ptr, offset_bytes_i32, i8_name);
    LLVMBuildBitCast(
        builder,
        byte_ptr,
        LLVMPointerType(llvm_ty_for_primitive(context, ty), 0),
        typed_name.as_ptr().cast(),
    )
}

pub(super) unsafe fn build_typed_state_ptr(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    state_base_ptr: LLVMValueRef,
    offset_bytes: usize,
    ty: PrimitiveType,
    gep_name: &[u8],
    cast_name: &[u8],
) -> LLVMValueRef {
    let byte_ptr = build_i8_ptr_offset(builder, context, state_base_ptr, offset_bytes, gep_name);
    let ptr_ty = LLVMPointerType(llvm_ty_for_primitive(context, ty), 0);
    LLVMBuildBitCast(builder, byte_ptr, ptr_ty, cast_name.as_ptr().cast())
}

pub(super) unsafe fn build_state_ptr_with_elem_ty(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    state_base_ptr: LLVMValueRef,
    offset_bytes: usize,
    elem_ty: LLVMTypeRef,
    gep_name: &[u8],
    cast_name: &[u8],
) -> LLVMValueRef {
    let byte_ptr = build_i8_ptr_offset(builder, context, state_base_ptr, offset_bytes, gep_name);
    let ptr_ty = LLVMPointerType(elem_ty, 0);
    LLVMBuildBitCast(builder, byte_ptr, ptr_ty, cast_name.as_ptr().cast())
}
