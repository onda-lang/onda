use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) unsafe fn allocate_event_parameters(
        &mut self,
        event: onda_mir::EventId,
    ) -> Result<(), MirCodegenError> {
        let payload = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            12,
            "event_payload",
        )?;
        let i8_ty = LLVMInt8TypeInContext(self.module.context);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let mut offset = LLVMConstInt(i32_ty, 0, 0);
        let event = &self.module.program.interface.events[event.index()];
        let plan = &self.module.layouts.event_payloads[match self.function.kind {
            FunctionKind::Event(id) => id.index(),
            _ => unreachable!(),
        }]
        .plan;
        self.event_parameters.reserve(event.params.len());
        for group in plan.parameters() {
            let length = if group.dynamic {
                offset = super::super::event_input::aligned_offset(self.builder, offset, 4);
                let pointer = LLVMBuildGEP2(
                    self.builder,
                    i8_ty,
                    payload,
                    [offset].as_mut_ptr(),
                    1,
                    c"event_length_ptr".as_ptr(),
                );
                let length =
                    LLVMBuildLoad2(self.builder, i32_ty, pointer, c"event_length".as_ptr());
                LLVMSetAlignment(length, 4);
                if let Some(index) = group.length_parameter {
                    self.event_parameters.push(PlaceRef {
                        ptr: pointer,
                        ty: event.params[index].ty,
                        alignment: 4,
                    });
                }
                offset = LLVMBuildAdd(
                    self.builder,
                    offset,
                    LLVMConstInt(i32_ty, 4, 0),
                    c"event_data_offset".as_ptr(),
                );
                length
            } else {
                LLVMConstInt(i32_ty, 1, 0)
            };
            for index in group.tensors.clone() {
                let tensor = &plan.tensors()[index];
                let parameter = &event.params[tensor.parameter];
                let size = tensor.encoding.byte_size();
                offset = super::super::event_input::aligned_offset(self.builder, offset, size);
                let pointer = LLVMBuildGEP2(
                    self.builder,
                    i8_ty,
                    payload,
                    [offset].as_mut_ptr(),
                    1,
                    c"event_tensor".as_ptr(),
                );
                let elements = LLVMBuildMul(
                    self.builder,
                    length,
                    LLVMConstInt(i32_ty, tensor.elements as u64, 0),
                    c"event_elements".as_ptr(),
                );
                if group.dynamic {
                    let stride = LLVMConstInt(i32_ty, size as u64, 0);
                    let descriptor = self.build_slice_descriptor(
                        parameter.ty,
                        pointer,
                        pointer,
                        elements,
                        stride,
                    )?;
                    let ptr = LLVMBuildAlloca(
                        self.builder,
                        self.module.types.get(parameter.ty),
                        c"event_slice".as_ptr(),
                    );
                    LLVMBuildStore(self.builder, descriptor, ptr);
                    self.event_parameters.push(PlaceRef {
                        ptr,
                        ty: parameter.ty,
                        alignment: self.module.layouts.type_alignments[parameter.ty.index()],
                    });
                } else {
                    self.event_parameters.push(PlaceRef {
                        ptr: pointer,
                        ty: parameter.ty,
                        alignment: size,
                    });
                }
                offset = LLVMBuildAdd(
                    self.builder,
                    offset,
                    LLVMBuildMul(
                        self.builder,
                        elements,
                        LLVMConstInt(i32_ty, size as u64, 0),
                        c"event_tensor_bytes".as_ptr(),
                    ),
                    c"event_next_tensor".as_ptr(),
                );
            }
        }
        Ok(())
    }

    pub(super) unsafe fn lower_publish_delegate(
        &mut self,
        delegate: onda_mir::DelegateId,
        args: &[CallArgument],
    ) -> Result<(), MirCodegenError> {
        let descriptor = &self.module.program.interface.delegates[delegate.index()];
        let plan = onda_processor_abi::payload::PayloadPlan::new(&descriptor.schema)
            .map_err(|error| MirCodegenError::invalid(error.to_string()))?;
        let mut prefix_bytes = vec![0_u64; descriptor.params.len()];
        for tensor in plan.tensors() {
            prefix_bytes[tensor.parameter] = if tensor.length_prefix { 4 } else { 0 };
        }
        let i8_ty = LLVMInt8TypeInContext(self.module.context);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let i64_ty = LLVMInt64TypeInContext(self.module.context);
        let mut fixed_array_length_invalid = None;
        for (param, argument) in descriptor.params.iter().zip(args) {
            let CallArgument::Value(value) = argument else {
                return Err(MirCodegenError::invalid(
                    "delegate publication payload is not an evaluated value",
                ));
            };
            let Type::Array { len, .. } = self.module.program.types[param.ty.index()] else {
                continue;
            };
            let parts = self.slice_parts(*value)?;
            let wrong_length = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntNE,
                parts.len,
                LLVMConstInt(i32_ty, u64::from(len), 0),
                c_name("delegate_fixed_array_wrong_length")?.as_ptr(),
            );
            fixed_array_length_invalid = Some(match fixed_array_length_invalid {
                Some(previous) => LLVMBuildOr(
                    self.builder,
                    previous,
                    wrong_length,
                    c_name("delegate_fixed_array_length_invalid")?.as_ptr(),
                ),
                None => wrong_length,
            });
        }
        for group in plan.parameters() {
            let Some(length_parameter) = group.length_parameter else {
                continue;
            };
            let CallArgument::Value(length) = args[length_parameter] else {
                unreachable!()
            };
            let length = self.lower_value(length)?;
            self.emit_failure_if(
                LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    length,
                    LLVMConstInt(i32_ty, 0, 0),
                    c"negative_message_length".as_ptr(),
                ),
                "message_length_ok",
            )?;
            for tensor in &plan.tensors()[group.tensors.clone()] {
                let CallArgument::Value(value) = args[tensor.parameter] else {
                    unreachable!()
                };
                let actual = self.slice_parts(value)?.len;
                let expected = LLVMBuildMul(
                    self.builder,
                    LLVMBuildZExt(self.builder, length, i64_ty, c"logical_length".as_ptr()),
                    LLVMConstInt(i64_ty, tensor.elements as u64, 0),
                    c"tensor_length".as_ptr(),
                );
                self.emit_failure_if(
                    LLVMBuildICmp(
                        self.builder,
                        LLVMIntPredicate::LLVMIntNE,
                        LLVMBuildZExt(self.builder, actual, i64_ty, c"actual_length".as_ptr()),
                        expected,
                        c"invalid_tensor_length".as_ptr(),
                    ),
                    "tensor_length_ok",
                )?;
            }
        }
        if let Some(fixed_array_length_invalid) = fixed_array_length_invalid {
            self.emit_failure_if(fixed_array_length_invalid, "delegate_fixed_array_length_ok")?;
        }

        let batch = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            DELEGATE_BATCH_CONTEXT_INDEX,
            "delegate_batch",
        )?;
        let batch_present = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntNE,
            batch,
            LLVMConstPointerNull(self.module.ptr_ty),
            c_name("delegate_batch_present")?.as_ptr(),
        );
        let inspect = append_block(
            self.module.context,
            self.declaration.value,
            "publish_delegate_inspect_batch",
        )?;
        let done = append_block(
            self.module.context,
            self.declaration.value,
            "publish_delegate_done",
        )?;
        LLVMBuildCondBr(self.builder, batch_present, inspect, done);
        LLVMPositionBuilderAtEnd(self.builder, inspect);
        let sequence = self.next_output_sequence()?;

        let mut payload_bytes = LLVMConstInt(i64_ty, 0, 0);
        for (index, (param, argument)) in descriptor.params.iter().zip(args).enumerate() {
            let CallArgument::Value(value) = argument else {
                unreachable!("validated above")
            };
            let bytes = match self.module.program.types[param.ty.index()] {
                Type::Slice { element, .. } => {
                    let parts = self.slice_parts(*value)?;
                    let len = LLVMBuildZExt(
                        self.builder,
                        parts.len,
                        i64_ty,
                        c_name("delegate_slice_len_i64")?.as_ptr(),
                    );
                    LLVMBuildAdd(
                        self.builder,
                        LLVMConstInt(i64_ty, prefix_bytes[index], 0),
                        LLVMBuildMul(
                            self.builder,
                            len,
                            LLVMConstInt(i64_ty, scalar_store_size(element), 0),
                            c_name("delegate_slice_bytes")?.as_ptr(),
                        ),
                        c_name("delegate_dynamic_param_bytes")?.as_ptr(),
                    )
                }
                _ => LLVMConstInt(
                    i64_ty,
                    fixed_payload_type_size(self.module.program, param.ty)?.ok_or_else(|| {
                        MirCodegenError::invalid("delegate payload contains a nested dynamic type")
                    })? as u64,
                    0,
                ),
            };
            payload_bytes = LLVMBuildAdd(
                self.builder,
                payload_bytes,
                bytes,
                c_name("delegate_payload_bytes")?.as_ptr(),
            );
        }

        let storage_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.delegate_batch_ty,
            batch,
            0,
            c_name("delegate_storage_ptr")?.as_ptr(),
        );
        let storage = LLVMBuildLoad2(
            self.builder,
            self.module.ptr_ty,
            storage_ptr,
            c_name("delegate_storage")?.as_ptr(),
        );
        let storage_present = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntNE,
            storage,
            LLVMConstPointerNull(self.module.ptr_ty),
            c_name("delegate_storage_present")?.as_ptr(),
        );
        let capacity_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.delegate_batch_ty,
            batch,
            1,
            c_name("delegate_capacity_ptr")?.as_ptr(),
        );
        let used_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.delegate_batch_ty,
            batch,
            2,
            c_name("delegate_used_ptr")?.as_ptr(),
        );
        let capacity = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            capacity_ptr,
            c_name("delegate_capacity")?.as_ptr(),
        );
        let used = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            used_ptr,
            c_name("delegate_used")?.as_ptr(),
        );
        let capacity_i64 = LLVMBuildZExt(
            self.builder,
            capacity,
            i64_ty,
            c_name("delegate_capacity_i64")?.as_ptr(),
        );
        let used_i64 = LLVMBuildZExt(
            self.builder,
            used,
            i64_ty,
            c_name("delegate_used_i64")?.as_ptr(),
        );
        let required = LLVMBuildAdd(
            self.builder,
            payload_bytes,
            LLVMConstInt(
                i64_ty,
                onda_processor_abi::DELEGATE_RECORD_HEADER_SIZE as u64,
                0,
            ),
            c_name("delegate_required")?.as_ptr(),
        );
        let used_valid = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            used_i64,
            capacity_i64,
            c_name("delegate_used_valid")?.as_ptr(),
        );
        let available = LLVMBuildSub(
            self.builder,
            capacity_i64,
            used_i64,
            c_name("delegate_available")?.as_ptr(),
        );
        let required_fits = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            required,
            available,
            c_name("delegate_required_fits")?.as_ptr(),
        );
        let fits = LLVMBuildAnd(
            self.builder,
            storage_present,
            LLVMBuildAnd(
                self.builder,
                used_valid,
                required_fits,
                c_name("delegate_capacity_valid")?.as_ptr(),
            ),
            c_name("delegate_record_fits")?.as_ptr(),
        );
        let write = append_block(
            self.module.context,
            self.declaration.value,
            "publish_delegate_write",
        )?;
        let dropped = append_block(
            self.module.context,
            self.declaration.value,
            "publish_delegate_dropped",
        )?;
        let no_storage = append_block(
            self.module.context,
            self.declaration.value,
            "publish_delegate_no_storage",
        )?;
        let capacity_ok = append_block(
            self.module.context,
            self.declaration.value,
            "publish_delegate_capacity_ok",
        )?;
        LLVMBuildCondBr(self.builder, storage_present, write, no_storage);
        LLVMPositionBuilderAtEnd(self.builder, no_storage);
        LLVMBuildBr(self.builder, done);
        LLVMPositionBuilderAtEnd(self.builder, write);
        LLVMBuildCondBr(self.builder, fits, capacity_ok, dropped);

        LLVMPositionBuilderAtEnd(self.builder, dropped);
        let overflow_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.delegate_batch_ty,
            batch,
            4,
            c_name("delegate_overflow_ptr")?.as_ptr(),
        );
        let overflow = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            overflow_ptr,
            c_name("delegate_overflow")?.as_ptr(),
        );
        let saturated = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            overflow,
            LLVMConstInt(i32_ty, u64::from(u32::MAX), 0),
            c_name("delegate_overflow_saturated")?.as_ptr(),
        );
        let incremented = LLVMBuildAdd(
            self.builder,
            overflow,
            LLVMConstInt(i32_ty, 1, 0),
            c_name("delegate_overflow_incremented")?.as_ptr(),
        );
        LLVMBuildStore(
            self.builder,
            LLVMBuildSelect(
                self.builder,
                saturated,
                overflow,
                incremented,
                c_name("delegate_overflow_next")?.as_ptr(),
            ),
            overflow_ptr,
        );
        LLVMBuildBr(self.builder, done);

        LLVMPositionBuilderAtEnd(self.builder, capacity_ok);
        let record = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            storage,
            [used_i64].as_mut_ptr(),
            1,
            c_name("delegate_record")?.as_ptr(),
        );
        let delegate_store = LLVMBuildStore(
            self.builder,
            LLVMConstInt(i32_ty, u64::from(delegate.raw()), 0),
            record,
        );
        LLVMSetAlignment(delegate_store, 1);
        let payload_size_ptr = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            record,
            [LLVMConstInt(i64_ty, 4, 0)].as_mut_ptr(),
            1,
            c_name("delegate_payload_size_ptr")?.as_ptr(),
        );
        let payload_size_store = LLVMBuildStore(
            self.builder,
            LLVMBuildTrunc(
                self.builder,
                payload_bytes,
                i32_ty,
                c_name("delegate_payload_size")?.as_ptr(),
            ),
            payload_size_ptr,
        );
        LLVMSetAlignment(payload_size_store, 1);
        let sequence_ptr = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            record,
            [LLVMConstInt(i64_ty, 8, 0)].as_mut_ptr(),
            1,
            c_name("delegate_sequence_ptr")?.as_ptr(),
        );
        let sequence_store = LLVMBuildStore(self.builder, sequence, sequence_ptr);
        LLVMSetAlignment(sequence_store, 1);
        let mut cursor = LLVMConstInt(
            i64_ty,
            onda_processor_abi::DELEGATE_RECORD_HEADER_SIZE as u64,
            0,
        );
        for (index, (param, argument)) in descriptor.params.iter().zip(args).enumerate() {
            let CallArgument::Value(value) = argument else {
                unreachable!("validated above")
            };
            let destination = LLVMBuildGEP2(
                self.builder,
                i8_ty,
                record,
                [cursor].as_mut_ptr(),
                1,
                c_name("delegate_payload_param")?.as_ptr(),
            );
            match self.module.program.types[param.ty.index()] {
                Type::Scalar(scalar) => {
                    let store = LLVMBuildStore(
                        self.builder,
                        super::super::event_input::wire_bits(
                            self.module,
                            self.builder,
                            self.lower_value(*value)?,
                            scalar_store_size(scalar) as usize,
                        ),
                        destination,
                    );
                    LLVMSetAlignment(store, 1);
                    cursor = LLVMBuildAdd(
                        self.builder,
                        cursor,
                        LLVMConstInt(i64_ty, scalar_store_size(scalar), 0),
                        c_name("delegate_payload_cursor")?.as_ptr(),
                    );
                }
                Type::Array { element, len } => {
                    let Type::Scalar(element) = self.module.program.types[element.index()] else {
                        return Err(MirCodegenError::invalid(
                            "delegate fixed array element is not scalar",
                        ));
                    };
                    let parts = self.slice_parts(*value)?;
                    self.copy_slice_to_packed_payload(
                        parts,
                        LLVMConstInt(i32_ty, u64::from(len), 0),
                        destination,
                    )?;
                    cursor = LLVMBuildAdd(
                        self.builder,
                        cursor,
                        LLVMConstInt(i64_ty, u64::from(len) * scalar_store_size(element), 0),
                        c_name("delegate_payload_cursor")?.as_ptr(),
                    );
                }
                Type::Slice { element, .. } => {
                    let parts = self.slice_parts(*value)?;
                    if prefix_bytes[index] != 0 {
                        let len_store = LLVMBuildStore(
                            self.builder,
                            super::super::event_input::wire_bits(
                                self.module,
                                self.builder,
                                parts.len,
                                4,
                            ),
                            destination,
                        );
                        LLVMSetAlignment(len_store, 1);
                    }
                    let data = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        destination,
                        [LLVMConstInt(i64_ty, prefix_bytes[index], 0)].as_mut_ptr(),
                        1,
                        c_name("delegate_slice_data")?.as_ptr(),
                    );
                    self.copy_slice_to_packed_payload(parts, parts.len, data)?;
                    let data_bytes = LLVMBuildMul(
                        self.builder,
                        LLVMBuildZExt(
                            self.builder,
                            parts.len,
                            i64_ty,
                            c_name("delegate_slice_len_i64")?.as_ptr(),
                        ),
                        LLVMConstInt(i64_ty, scalar_store_size(element), 0),
                        c_name("delegate_slice_data_bytes")?.as_ptr(),
                    );
                    cursor = LLVMBuildAdd(
                        self.builder,
                        cursor,
                        LLVMBuildAdd(
                            self.builder,
                            LLVMConstInt(i64_ty, prefix_bytes[index], 0),
                            data_bytes,
                            c_name("delegate_slice_param_bytes")?.as_ptr(),
                        ),
                        c_name("delegate_payload_cursor")?.as_ptr(),
                    );
                }
                _ => {
                    return Err(MirCodegenError::invalid(
                        "unsupported delegate payload type",
                    ));
                }
            }
        }
        let next_used = LLVMBuildTrunc(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                used_i64,
                required,
                c_name("delegate_next_used_i64")?.as_ptr(),
            ),
            i32_ty,
            c_name("delegate_next_used")?.as_ptr(),
        );
        LLVMBuildStore(self.builder, next_used, used_ptr);
        let count_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.delegate_batch_ty,
            batch,
            3,
            c_name("delegate_count_ptr")?.as_ptr(),
        );
        let count = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            count_ptr,
            c_name("delegate_count")?.as_ptr(),
        );
        LLVMBuildStore(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                count,
                LLVMConstInt(i32_ty, 1, 0),
                c_name("delegate_count_next")?.as_ptr(),
            ),
            count_ptr,
        );
        LLVMBuildBr(self.builder, done);
        LLVMPositionBuilderAtEnd(self.builder, done);
        Ok(())
    }

    pub(super) unsafe fn copy_slice_to_packed_payload(
        &mut self,
        source: SliceParts,
        len: LLVMValueRef,
        destination: LLVMValueRef,
    ) -> Result<(), MirCodegenError> {
        let i8_ty = LLVMInt8TypeInContext(self.module.context);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let i64_ty = LLVMInt64TypeInContext(self.module.context);
        let preheader = LLVMGetInsertBlock(self.builder);
        let body = append_block(
            self.module.context,
            self.declaration.value,
            "delegate_payload_copy",
        )?;
        let done = append_block(
            self.module.context,
            self.declaration.value,
            "delegate_payload_copy_done",
        )?;
        let nonempty = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntNE,
            len,
            LLVMConstInt(i32_ty, 0, 0),
            c_name("delegate_payload_nonempty")?.as_ptr(),
        );
        LLVMBuildCondBr(self.builder, nonempty, body, done);
        LLVMPositionBuilderAtEnd(self.builder, body);
        let index = LLVMBuildPhi(
            self.builder,
            i32_ty,
            c_name("delegate_payload_index")?.as_ptr(),
        );
        let zero = LLVMConstInt(i32_ty, 0, 0);
        LLVMAddIncoming(index, [zero].as_mut_ptr(), [preheader].as_mut_ptr(), 1);
        let index_i64 = LLVMBuildZExt(
            self.builder,
            index,
            i64_ty,
            c_name("delegate_payload_index_i64")?.as_ptr(),
        );
        let source_offset = LLVMBuildMul(
            self.builder,
            index_i64,
            LLVMBuildZExt(
                self.builder,
                source.stride_bytes,
                i64_ty,
                c_name("delegate_payload_stride_i64")?.as_ptr(),
            ),
            c_name("delegate_payload_source_offset")?.as_ptr(),
        );
        let source_ptr = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            source.read_ptr,
            [source_offset].as_mut_ptr(),
            1,
            c_name("delegate_payload_source")?.as_ptr(),
        );
        let destination_offset = LLVMBuildMul(
            self.builder,
            index_i64,
            LLVMConstInt(i64_ty, scalar_store_size(source.element), 0),
            c_name("delegate_payload_destination_offset")?.as_ptr(),
        );
        let destination_ptr = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            destination,
            [destination_offset].as_mut_ptr(),
            1,
            c_name("delegate_payload_destination")?.as_ptr(),
        );
        let value = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, source.element),
            source_ptr,
            c_name("delegate_payload_value")?.as_ptr(),
        );
        LLVMSetAlignment(value, 1);
        let store = LLVMBuildStore(
            self.builder,
            super::super::event_input::wire_bits(
                self.module,
                self.builder,
                value,
                scalar_store_size(source.element) as usize,
            ),
            destination_ptr,
        );
        LLVMSetAlignment(store, 1);
        let next = LLVMBuildAdd(
            self.builder,
            index,
            LLVMConstInt(i32_ty, 1, 0),
            c_name("delegate_payload_next_index")?.as_ptr(),
        );
        let again = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULT,
            next,
            len,
            c_name("delegate_payload_copy_more")?.as_ptr(),
        );
        LLVMBuildCondBr(self.builder, again, body, done);
        LLVMAddIncoming(index, [next].as_mut_ptr(), [body].as_mut_ptr(), 1);
        LLVMPositionBuilderAtEnd(self.builder, done);
        Ok(())
    }
}
