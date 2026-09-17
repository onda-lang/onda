//! Raw event preflight and aligned input preparation. The event entry resets its
//! call-scoped output before entering this path. Shape work is emitted once per
//! tensor; runtime work scales only with the admitted payload contents.
use super::*;
use onda_processor_abi::payload::{IntegerDomain, ScalarEncoding};

struct Transfer {
    wire: LLVMValueRef,
    workspace: LLVMValueRef,
    elements: LLVMValueRef,
    encoding: ScalarEncoding,
    domain: Option<IntegerDomain>,
}

pub(super) unsafe fn aligned_offset(
    builder: LLVMBuilderRef,
    offset: LLVMValueRef,
    alignment: usize,
) -> LLVMValueRef {
    let ty = LLVMTypeOf(offset);
    LLVMBuildAnd(
        builder,
        LLVMBuildAdd(
            builder,
            offset,
            LLVMConstInt(ty, alignment as u64 - 1, 0),
            c"align_add".as_ptr(),
        ),
        LLVMConstInt(ty, !(alignment as u64 - 1), 0),
        c"aligned_offset".as_ptr(),
    )
}

pub(super) unsafe fn prepare(
    module: &ModuleEmitter<'_>,
    function: LLVMValueRef,
    builder: LLVMBuilderRef,
    event: onda_mir::EventId,
) -> Result<LLVMValueRef, MirCodegenError> {
    let emitter = InputEmitter {
        module,
        function,
        builder,
    };
    let i32_ty = LLVMInt32TypeInContext(module.context);
    let i64_ty = LLVMInt64TypeInContext(module.context);
    let descriptor_ty = LLVMStructTypeInContext(
        module.context,
        [module.ptr_ty, i32_ty, module.ptr_ty, i32_ty].as_mut_ptr(),
        4,
        0,
    );
    let descriptor = LLVMGetParam(function, 0);
    emitter.require(LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntNE,
        descriptor,
        LLVMConstPointerNull(module.ptr_ty),
        c"input_present".as_ptr(),
    ))?;
    let field = |index, ty| {
        LLVMBuildLoad2(
            builder,
            ty,
            LLVMBuildStructGEP2(
                builder,
                descriptor_ty,
                descriptor,
                index,
                c"input_field".as_ptr(),
            ),
            c"input_value".as_ptr(),
        )
    };
    let input = field(0, module.ptr_ty);
    let input_bytes = LLVMBuildZExt(builder, field(1, i32_ty), i64_ty, c"input_bytes".as_ptr());
    let workspace = field(2, module.ptr_ty);
    let capacity = LLVMBuildZExt(
        builder,
        field(3, i32_ty),
        i64_ty,
        c"workspace_capacity".as_ptr(),
    );
    emitter.require(emitter.compare(
        LLVMIntPredicate::LLVMIntULE,
        input_bytes,
        emitter.int(i32::MAX as u64),
    ))?;
    emitter.require(LLVMBuildOr(
        builder,
        emitter.compare(LLVMIntPredicate::LLVMIntEQ, input_bytes, emitter.int(0)),
        LLVMBuildICmp(
            builder,
            LLVMIntPredicate::LLVMIntNE,
            input,
            LLVMConstPointerNull(module.ptr_ty),
            c"input_data_present".as_ptr(),
        ),
        c"input_data_valid".as_ptr(),
    ))?;
    let plan = &module.layouts.event_payloads[event.index()].plan;
    let mut transfers = Vec::new();
    let mut wire = emitter.int(0);
    let mut prepared = emitter.int(0);
    for parameter in plan.parameters() {
        let len = if parameter.dynamic {
            let end = emitter.add(wire, emitter.int(4));
            emitter.require(emitter.compare(LLVMIntPredicate::LLVMIntULE, end, input_bytes))?;
            let pointer = emitter.pointer(input, wire);
            let len = emitter.load_wire(pointer, ScalarEncoding::I32);
            emitter.require(LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntSGE,
                len,
                LLVMConstInt(i32_ty, 0, 0),
                c"length_nonnegative".as_ptr(),
            ))?;
            prepared = aligned_offset(builder, prepared, 4);
            transfers.push(Transfer {
                wire,
                workspace: prepared,
                elements: emitter.int(1),
                encoding: ScalarEncoding::I32,
                domain: None,
            });
            wire = end;
            prepared = emitter.add(prepared, emitter.int(4));
            LLVMBuildZExt(builder, len, i64_ty, c"logical_length".as_ptr())
        } else {
            emitter.int(1)
        };
        for index in parameter.tensors.clone() {
            let tensor = &plan.tensors()[index];
            let elements = LLVMBuildMul(
                builder,
                len,
                emitter.int(tensor.elements as u64),
                c"tensor_elements".as_ptr(),
            );
            emitter.require(emitter.compare(
                LLVMIntPredicate::LLVMIntULE,
                elements,
                emitter.int((i32::MAX as usize / tensor.encoding.byte_size()) as u64),
            ))?;
            let bytes = LLVMBuildMul(
                builder,
                elements,
                emitter.int(tensor.encoding.byte_size() as u64),
                c"tensor_bytes".as_ptr(),
            );
            prepared = aligned_offset(builder, prepared, tensor.encoding.byte_size());
            transfers.push(Transfer {
                wire,
                workspace: prepared,
                elements,
                encoding: tensor.encoding,
                domain: tensor.domain,
            });
            wire = emitter.add(wire, bytes);
            prepared = emitter.add(prepared, bytes);
            emitter.require(emitter.compare(LLVMIntPredicate::LLVMIntULE, wire, input_bytes))?;
            emitter.require(emitter.compare(
                LLVMIntPredicate::LLVMIntULE,
                wire,
                emitter.int(i32::MAX as u64),
            ))?;
            emitter.require(emitter.compare(
                LLVMIntPredicate::LLVMIntULE,
                prepared,
                emitter.int(i32::MAX as u64),
            ))?;
        }
    }
    emitter.require(emitter.compare(LLVMIntPredicate::LLVMIntEQ, wire, input_bytes))?;
    emitter.require(emitter.compare(LLVMIntPredicate::LLVMIntULE, prepared, capacity))?;
    let nonempty = emitter.compare(LLVMIntPredicate::LLVMIntNE, prepared, emitter.int(0));
    let present = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntNE,
        workspace,
        LLVMConstPointerNull(module.ptr_ty),
        c"workspace_present".as_ptr(),
    );
    let address = LLVMBuildPtrToInt(builder, workspace, i64_ty, c"workspace_address".as_ptr());
    let aligned = emitter.compare(
        LLVMIntPredicate::LLVMIntEQ,
        LLVMBuildAnd(
            builder,
            address,
            emitter.int(7),
            c"workspace_alignment".as_ptr(),
        ),
        emitter.int(0),
    );
    emitter.require(LLVMBuildOr(
        builder,
        LLVMBuildNot(builder, nonempty, c"empty_input".as_ptr()),
        LLVMBuildAnd(builder, present, aligned, c"workspace_valid".as_ptr()),
        c"workspace_admitted".as_ptr(),
    ))?;
    // No workspace or processor-state writes precede this point.
    for transfer in transfers {
        emitter.copy(input, workspace, transfer)?;
    }
    Ok(workspace)
}

struct InputEmitter<'a, 'p> {
    module: &'a ModuleEmitter<'p>,
    function: LLVMValueRef,
    builder: LLVMBuilderRef,
}
impl InputEmitter<'_, '_> {
    unsafe fn int(&self, value: u64) -> LLVMValueRef {
        LLVMConstInt(LLVMInt64TypeInContext(self.module.context), value, 0)
    }
    unsafe fn add(&self, a: LLVMValueRef, b: LLVMValueRef) -> LLVMValueRef {
        LLVMBuildAdd(self.builder, a, b, c"input_offset".as_ptr())
    }
    unsafe fn compare(
        &self,
        predicate: LLVMIntPredicate,
        a: LLVMValueRef,
        b: LLVMValueRef,
    ) -> LLVMValueRef {
        LLVMBuildICmp(self.builder, predicate, a, b, c"input_valid".as_ptr())
    }
    unsafe fn require(&self, condition: LLVMValueRef) -> Result<(), MirCodegenError> {
        let valid = append_block(self.module.context, self.function, "input_valid")?;
        let rejected = append_block(self.module.context, self.function, "input_rejected")?;
        LLVMBuildCondBr(self.builder, condition, valid, rejected);
        LLVMPositionBuilderAtEnd(self.builder, rejected);
        LLVMBuildRet(
            self.builder,
            LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                onda_processor_abi::PROCESSOR_EXECUTION_INPUT_REJECTED as u64,
                0,
            ),
        );
        LLVMPositionBuilderAtEnd(self.builder, valid);
        Ok(())
    }
    unsafe fn pointer(&self, base: LLVMValueRef, offset: LLVMValueRef) -> LLVMValueRef {
        LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            base,
            [offset].as_mut_ptr(),
            1,
            c"input_pointer".as_ptr(),
        )
    }
    unsafe fn little_endian(&self) -> bool {
        *LLVMGetDataLayoutStr(self.module.module) != b'E' as i8
    }
    unsafe fn load_wire(&self, pointer: LLVMValueRef, encoding: ScalarEncoding) -> LLVMValueRef {
        let bytes = encoding.byte_size();
        let ty = LLVMIntTypeInContext(self.module.context, bytes as u32 * 8);
        let loaded = LLVMBuildLoad2(self.builder, ty, pointer, c"wire_bits".as_ptr());
        LLVMSetAlignment(loaded, 1);
        wire_bits(self.module, self.builder, loaded, bytes)
    }
    unsafe fn normalize(
        &self,
        value: LLVMValueRef,
        encoding: ScalarEncoding,
        domain: Option<IntegerDomain>,
    ) -> LLVMValueRef {
        let ty = LLVMTypeOf(value);
        if encoding == ScalarEncoding::Bool {
            return LLVMBuildZExt(
                self.builder,
                self.compare(LLVMIntPredicate::LLVMIntNE, value, LLVMConstInt(ty, 0, 0)),
                ty,
                c"normalized_bool".as_ptr(),
            );
        }
        let Some(domain) = domain else { return value };
        let wide = if encoding == ScalarEncoding::I32 {
            LLVMBuildSExt(
                self.builder,
                value,
                LLVMInt64TypeInContext(self.module.context),
                c"integer_wide".as_ptr(),
            )
        } else {
            value
        };
        let min = self.int(domain.min as u64);
        let max = self.int(domain.max as u64);
        let result = if !domain.wrap {
            let low = LLVMBuildSelect(
                self.builder,
                self.compare(LLVMIntPredicate::LLVMIntSLT, wide, min),
                min,
                wide,
                c"clamped_low".as_ptr(),
            );
            LLVMBuildSelect(
                self.builder,
                self.compare(LLVMIntPredicate::LLVMIntSGT, low, max),
                max,
                low,
                c"clamped_integer".as_ptr(),
            )
        } else {
            let width = (domain.max as u64)
                .wrapping_sub(domain.min as u64)
                .wrapping_add(1);
            if width == 0 {
                return value;
            }
            let biased = LLVMBuildXor(
                self.builder,
                wide,
                self.int(1 << 63),
                c"biased_integer".as_ptr(),
            );
            let residue = LLVMBuildURem(
                self.builder,
                biased,
                self.int(width),
                c"integer_residue".as_ptr(),
            );
            let min_residue = self.int(((domain.min as u64) ^ (1 << 63)) % width);
            let delta = LLVMBuildSelect(
                self.builder,
                self.compare(LLVMIntPredicate::LLVMIntUGE, residue, min_residue),
                LLVMBuildSub(
                    self.builder,
                    residue,
                    min_residue,
                    c"positive_delta".as_ptr(),
                ),
                LLVMBuildSub(
                    self.builder,
                    self.int(width),
                    LLVMBuildSub(
                        self.builder,
                        min_residue,
                        residue,
                        c"negative_delta".as_ptr(),
                    ),
                    c"wrapped_delta".as_ptr(),
                ),
                c"integer_delta".as_ptr(),
            );
            self.add(min, delta)
        };
        if encoding == ScalarEncoding::I32 {
            LLVMBuildTrunc(self.builder, result, ty, c"normalized_integer".as_ptr())
        } else {
            result
        }
    }
    unsafe fn copy(
        &self,
        input: LLVMValueRef,
        workspace: LLVMValueRef,
        transfer: Transfer,
    ) -> Result<(), MirCodegenError> {
        let source = self.pointer(input, transfer.wire);
        let destination = self.pointer(workspace, transfer.workspace);
        let size = transfer.encoding.byte_size();
        let before = LLVMGetInsertBlock(self.builder);
        let body = append_block(self.module.context, self.function, "prepare_tensor")?;
        let done = append_block(self.module.context, self.function, "prepared_tensor")?;
        LLVMBuildCondBr(
            self.builder,
            self.compare(LLVMIntPredicate::LLVMIntNE, transfer.elements, self.int(0)),
            body,
            done,
        );
        LLVMPositionBuilderAtEnd(self.builder, body);
        if self.little_endian()
            && transfer.domain.is_none()
            && transfer.encoding != ScalarEncoding::Bool
        {
            let bytes = LLVMBuildMul(
                self.builder,
                transfer.elements,
                self.int(size as u64),
                c"copy_bytes".as_ptr(),
            );
            LLVMBuildMemCpy(self.builder, destination, size as u32, source, 1, bytes);
            LLVMBuildBr(self.builder, done);
        } else {
            let index = LLVMBuildPhi(
                self.builder,
                LLVMInt64TypeInContext(self.module.context),
                c"prepare_index".as_ptr(),
            );
            LLVMAddIncoming(index, [self.int(0)].as_mut_ptr(), [before].as_mut_ptr(), 1);
            let offset = LLVMBuildMul(
                self.builder,
                index,
                self.int(size as u64),
                c"prepare_offset".as_ptr(),
            );
            let value = self.load_wire(self.pointer(source, offset), transfer.encoding);
            let value = self.normalize(value, transfer.encoding, transfer.domain);
            let store = LLVMBuildStore(self.builder, value, self.pointer(destination, offset));
            LLVMSetAlignment(store, size as u32);
            let next = self.add(index, self.int(1));
            LLVMBuildCondBr(
                self.builder,
                self.compare(LLVMIntPredicate::LLVMIntULT, next, transfer.elements),
                body,
                done,
            );
            LLVMAddIncoming(index, [next].as_mut_ptr(), [body].as_mut_ptr(), 1);
        }
        LLVMPositionBuilderAtEnd(self.builder, done);
        Ok(())
    }
}

// Byte swapping is symmetric: use the same conversion for input and output.
pub(super) unsafe fn wire_bits(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
    value: LLVMValueRef,
    bytes: usize,
) -> LLVMValueRef {
    if *LLVMGetDataLayoutStr(module.module) != b'E' as i8 || bytes == 1 {
        return value;
    }
    let ty = LLVMIntTypeInContext(module.context, bytes as u32 * 8);
    let bits = LLVMBuildBitCast(builder, value, ty, c"wire_bits".as_ptr());
    let mut result = LLVMConstInt(ty, 0, 0);
    for byte in 0..bytes {
        let part = LLVMBuildAnd(
            builder,
            LLVMBuildLShr(
                builder,
                bits,
                LLVMConstInt(ty, (byte * 8) as u64, 0),
                c"wire_shift".as_ptr(),
            ),
            LLVMConstInt(ty, 255, 0),
            c"wire_byte".as_ptr(),
        );
        result = LLVMBuildOr(
            builder,
            result,
            LLVMBuildShl(
                builder,
                part,
                LLVMConstInt(ty, ((bytes - 1 - byte) * 8) as u64, 0),
                c"native_shift".as_ptr(),
            ),
            c"native_bits".as_ptr(),
        );
    }
    result
}
