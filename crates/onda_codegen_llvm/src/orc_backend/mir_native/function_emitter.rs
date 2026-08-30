use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) unsafe fn allocate_storage(&mut self) -> Result<(), MirCodegenError> {
        for (index, local) in self.function.locals.iter().enumerate() {
            let name = c_name(&format!("local_{index}"))?;
            let ty = self.module.types.get(local.ty);
            let ptr = LLVMBuildAlloca(self.builder, ty, name.as_ptr());
            self.locals.push(PlaceRef {
                ptr,
                ty: local.ty,
                alignment: self.module.layouts.type_alignments[local.ty.index()],
            });
        }
        match self.function.kind {
            FunctionKind::Process => {
                for (parameter, context_field) in
                    self.function.params.iter().zip([2_u32, 3_u32, 4_u32])
                {
                    self.parameters.push(PlaceRef {
                        ptr: context_field_ptr(
                            self.module,
                            self.builder,
                            self.runtime_context,
                            context_field,
                        )?,
                        ty: parameter.ty,
                        alignment: self.module.layouts.type_alignments[parameter.ty.index()],
                    });
                }
            }
            FunctionKind::User => {
                for (index, parameter) in self.function.params.iter().enumerate() {
                    let incoming = LLVMGetParam(self.declaration.value, (index + 1) as u32);
                    match parameter.mode {
                        onda_mir::PassingMode::Value => {
                            let ty = self.module.types.get(parameter.ty);
                            let name = c_name(&format!("param_{index}"))?;
                            let ptr = LLVMBuildAlloca(self.builder, ty, name.as_ptr());
                            LLVMBuildStore(self.builder, incoming, ptr);
                            self.parameters.push(PlaceRef {
                                ptr,
                                ty: parameter.ty,
                                alignment: self.module.layouts.type_alignments
                                    [parameter.ty.index()],
                            });
                        }
                        onda_mir::PassingMode::ReadOnlyReference
                        | onda_mir::PassingMode::ReadWriteReference => {
                            self.parameters.push(PlaceRef {
                                ptr: incoming,
                                ty: parameter.ty,
                                alignment: 1,
                            });
                        }
                    }
                }
            }
            FunctionKind::Event(event) => self.allocate_event_parameters(event)?,
            FunctionKind::Init => {}
        }
        Ok(())
    }

    unsafe fn allocate_event_parameters(
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
        let parameters = &self.module.program.interface.events[event.index()].params;
        self.event_parameters.reserve(parameters.len());
        for (index, parameter) in parameters.iter().enumerate() {
            match self.module.program.types[parameter.ty.index()] {
                Type::Slice { element, .. } => {
                    let len_ptr = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        payload,
                        [offset].as_mut_ptr(),
                        1,
                        c_name("event_slice_len_ptr")?.as_ptr(),
                    );
                    let len = LLVMBuildLoad2(
                        self.builder,
                        i32_ty,
                        len_ptr,
                        c_name("event_slice_len")?.as_ptr(),
                    );
                    LLVMSetAlignment(len, 1);
                    let data_offset = LLVMBuildAdd(
                        self.builder,
                        offset,
                        LLVMConstInt(i32_ty, 4, 0),
                        c_name("event_slice_data_offset")?.as_ptr(),
                    );
                    let data_ptr = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        payload,
                        [data_offset].as_mut_ptr(),
                        1,
                        c_name("event_slice_data")?.as_ptr(),
                    );
                    let stride = LLVMConstInt(i32_ty, scalar_store_size(element), 0);
                    let descriptor =
                        self.build_slice_descriptor(parameter.ty, data_ptr, data_ptr, len, stride)?;
                    let name = c_name(&format!("event_slice_{index}"))?;
                    let ptr = LLVMBuildAlloca(
                        self.builder,
                        self.module.types.get(parameter.ty),
                        name.as_ptr(),
                    );
                    LLVMBuildStore(self.builder, descriptor, ptr);
                    self.event_parameters.push(PlaceRef {
                        ptr,
                        ty: parameter.ty,
                        alignment: self.module.layouts.type_alignments[parameter.ty.index()],
                    });
                    let data_bytes = LLVMBuildMul(
                        self.builder,
                        len,
                        stride,
                        c_name("event_slice_data_bytes")?.as_ptr(),
                    );
                    offset = LLVMBuildAdd(
                        self.builder,
                        data_offset,
                        data_bytes,
                        c_name("event_payload_next")?.as_ptr(),
                    );
                }
                _ => {
                    let ptr = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        payload,
                        [offset].as_mut_ptr(),
                        1,
                        c_name("event_parameter")?.as_ptr(),
                    );
                    self.event_parameters.push(PlaceRef {
                        ptr,
                        ty: parameter.ty,
                        alignment: 1,
                    });
                    let size = fixed_payload_type_size(self.module.program, parameter.ty)?
                        .ok_or_else(|| {
                            MirCodegenError::invalid(
                                "event payload has an unexpected nested dynamic type",
                            )
                        })?;
                    offset = LLVMBuildAdd(
                        self.builder,
                        offset,
                        LLVMConstInt(i32_ty, size as u64, 0),
                        c_name("event_payload_next")?.as_ptr(),
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) unsafe fn lower_block(&mut self, block: &Block) -> Result<bool, MirCodegenError> {
        for statement in &block.statements {
            if current_block_terminated(self.builder) {
                return Ok(true);
            }
            self.lower_statement(&statement.kind)?;
        }
        Ok(current_block_terminated(self.builder))
    }

    unsafe fn lower_statement(&mut self, statement: &StatementKind) -> Result<(), MirCodegenError> {
        match statement {
            StatementKind::Assign { destination, value } => {
                let destination = self.lower_place(destination)?;
                let value = self.lower_rvalue(value, destination.ty)?;
                self.store(destination, value);
            }
            StatementKind::Call {
                results,
                function,
                args,
            } => self.lower_call(results, *function, args)?,
            StatementKind::PublishDelegate { delegate, args } => {
                self.lower_publish_delegate(*delegate, args)?
            }
            StatementKind::PublishLog { site, arguments } => {
                self.lower_publish_log(*site, arguments)?
            }
            StatementKind::OutputStore {
                output,
                element,
                bounds,
                frame,
                value,
            } => self.lower_output_store(*output, *element, *bounds, *frame, *value)?,
            StatementKind::ControlOutputStore {
                output,
                element,
                bounds,
                value,
            } => self.lower_control_output_store(*output, *element, *bounds, *value)?,
            StatementKind::BufferStore {
                buffer,
                channel,
                index,
                value,
                bounds,
            } => self.lower_buffer_store(*buffer, *channel, *index, *value, *bounds)?,
            StatementKind::BufferParamStore {
                parameter,
                channel,
                index,
                value,
                bounds,
            } => self.lower_buffer_param_store(*parameter, *channel, *index, *value, *bounds)?,
            StatementKind::SliceStore {
                slice,
                index,
                value,
                bounds,
            } => self.lower_slice_store(*slice, *index, *value, *bounds)?,
            StatementKind::SliceFill { destination, value } => {
                self.lower_slice_fill(*destination, *value)?;
            }
            StatementKind::SliceCopy {
                destination,
                source,
            } => self.lower_slice_copy(*destination, *source)?,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => self.lower_if(*condition, then_block, else_block)?,
            StatementKind::Loop { body } => self.lower_loop(body)?,
            StatementKind::Break => {
                let Some((break_target, _)) = self.loop_stack.last().copied() else {
                    return Err(MirCodegenError::invalid("break outside a MIR loop"));
                };
                LLVMBuildBr(self.builder, break_target);
            }
            StatementKind::Continue => {
                let Some((_, continue_target)) = self.loop_stack.last().copied() else {
                    return Err(MirCodegenError::invalid("continue outside a MIR loop"));
                };
                LLVMBuildBr(self.builder, continue_target);
            }
            StatementKind::Return { values } => self.lower_return(values)?,
        }
        Ok(())
    }

    unsafe fn next_output_sequence(&mut self) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let sequence_ptr = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            OUTPUT_SEQUENCE_CONTEXT_INDEX,
            "output_sequence_ptr",
        )?;
        let sequence = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            sequence_ptr,
            c_name("output_sequence")?.as_ptr(),
        );
        let saturated = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            sequence,
            LLVMConstInt(i32_ty, u64::from(u32::MAX), 0),
            c_name("output_sequence_saturated")?.as_ptr(),
        );
        let incremented = LLVMBuildAdd(
            self.builder,
            sequence,
            LLVMConstInt(i32_ty, 1, 0),
            c_name("output_sequence_incremented")?.as_ptr(),
        );
        LLVMBuildStore(
            self.builder,
            LLVMBuildSelect(
                self.builder,
                saturated,
                sequence,
                incremented,
                c_name("output_sequence_next")?.as_ptr(),
            ),
            sequence_ptr,
        );
        Ok(sequence)
    }

    unsafe fn lower_publish_delegate(
        &mut self,
        delegate: onda_mir::DelegateId,
        args: &[CallArgument],
    ) -> Result<(), MirCodegenError> {
        let descriptor = &self.module.program.interface.delegates[delegate.index()];
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
        for (param, argument) in descriptor.params.iter().zip(args) {
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
                        LLVMConstInt(i64_ty, 4, 0),
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
        for (param, argument) in descriptor.params.iter().zip(args) {
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
                    let store =
                        LLVMBuildStore(self.builder, self.lower_value(*value)?, destination);
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
                    let len_store = LLVMBuildStore(self.builder, parts.len, destination);
                    LLVMSetAlignment(len_store, 1);
                    let data = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        destination,
                        [LLVMConstInt(i64_ty, 4, 0)].as_mut_ptr(),
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
                            LLVMConstInt(i64_ty, 4, 0),
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

    unsafe fn lower_publish_log(
        &mut self,
        site: onda_mir::LogSiteId,
        arguments: &[Value],
    ) -> Result<(), MirCodegenError> {
        let descriptor = &self.module.program.log_sites[site.index()];
        let i8_ty = LLVMInt8TypeInContext(self.module.context);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let i64_ty = LLVMInt64TypeInContext(self.module.context);
        let batch = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            PRINT_BATCH_CONTEXT_INDEX,
            "print_batch",
        )?;
        let batch_present = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntNE,
            batch,
            LLVMConstPointerNull(self.module.ptr_ty),
            c_name("print_batch_present")?.as_ptr(),
        );
        let inspect = append_block(
            self.module.context,
            self.declaration.value,
            "publish_log_inspect_batch",
        )?;
        let done = append_block(
            self.module.context,
            self.declaration.value,
            "publish_log_done",
        )?;
        LLVMBuildCondBr(self.builder, batch_present, inspect, done);
        LLVMPositionBuilderAtEnd(self.builder, inspect);
        let sequence = self.next_output_sequence()?;

        let storage_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.print_batch_ty,
            batch,
            0,
            c_name("print_storage_ptr")?.as_ptr(),
        );
        let storage = LLVMBuildLoad2(
            self.builder,
            self.module.ptr_ty,
            storage_ptr,
            c_name("print_storage")?.as_ptr(),
        );
        let storage_present = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntNE,
            storage,
            LLVMConstPointerNull(self.module.ptr_ty),
            c_name("print_storage_present")?.as_ptr(),
        );
        let capacity_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.print_batch_ty,
            batch,
            1,
            c_name("print_capacity_ptr")?.as_ptr(),
        );
        let used_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.print_batch_ty,
            batch,
            2,
            c_name("print_used_ptr")?.as_ptr(),
        );
        let capacity = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            capacity_ptr,
            c_name("print_capacity")?.as_ptr(),
        );
        let used = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            used_ptr,
            c_name("print_used")?.as_ptr(),
        );
        let capacity_i64 = LLVMBuildZExt(
            self.builder,
            capacity,
            i64_ty,
            c_name("print_capacity_i64")?.as_ptr(),
        );
        let used_i64 = LLVMBuildZExt(
            self.builder,
            used,
            i64_ty,
            c_name("print_used_i64")?.as_ptr(),
        );
        let required = LLVMConstInt(
            i64_ty,
            (onda_processor_abi::PRINT_RECORD_HEADER_SIZE + descriptor.payload_size as usize)
                as u64,
            0,
        );
        let used_valid = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            used_i64,
            capacity_i64,
            c_name("print_used_valid")?.as_ptr(),
        );
        let available = LLVMBuildSub(
            self.builder,
            capacity_i64,
            used_i64,
            c_name("print_available")?.as_ptr(),
        );
        let required_fits = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            required,
            available,
            c_name("print_required_fits")?.as_ptr(),
        );
        let fits = LLVMBuildAnd(
            self.builder,
            used_valid,
            required_fits,
            c_name("print_record_fits")?.as_ptr(),
        );
        let choose = append_block(
            self.module.context,
            self.declaration.value,
            "publish_log_choose",
        )?;
        let write = append_block(
            self.module.context,
            self.declaration.value,
            "publish_log_write",
        )?;
        let dropped = append_block(
            self.module.context,
            self.declaration.value,
            "publish_log_dropped",
        )?;
        LLVMBuildCondBr(self.builder, storage_present, choose, done);
        LLVMPositionBuilderAtEnd(self.builder, choose);
        LLVMBuildCondBr(self.builder, fits, write, dropped);

        LLVMPositionBuilderAtEnd(self.builder, dropped);
        let overflow_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.print_batch_ty,
            batch,
            4,
            c_name("print_overflow_ptr")?.as_ptr(),
        );
        let overflow = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            overflow_ptr,
            c_name("print_overflow")?.as_ptr(),
        );
        let saturated = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            overflow,
            LLVMConstInt(i32_ty, u64::from(u32::MAX), 0),
            c_name("print_overflow_saturated")?.as_ptr(),
        );
        let incremented = LLVMBuildAdd(
            self.builder,
            overflow,
            LLVMConstInt(i32_ty, 1, 0),
            c_name("print_overflow_incremented")?.as_ptr(),
        );
        LLVMBuildStore(
            self.builder,
            LLVMBuildSelect(
                self.builder,
                saturated,
                overflow,
                incremented,
                c_name("print_overflow_next")?.as_ptr(),
            ),
            overflow_ptr,
        );
        LLVMBuildBr(self.builder, done);

        LLVMPositionBuilderAtEnd(self.builder, write);
        let record = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            storage,
            [used_i64].as_mut_ptr(),
            1,
            c_name("print_record")?.as_ptr(),
        );
        let site_store = LLVMBuildStore(
            self.builder,
            LLVMConstInt(i32_ty, u64::from(site.raw()), 0),
            record,
        );
        LLVMSetAlignment(site_store, 1);
        let size_ptr = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            record,
            [LLVMConstInt(i64_ty, 4, 0)].as_mut_ptr(),
            1,
            c_name("print_payload_size_ptr")?.as_ptr(),
        );
        let size_store = LLVMBuildStore(
            self.builder,
            LLVMConstInt(i32_ty, u64::from(descriptor.payload_size), 0),
            size_ptr,
        );
        LLVMSetAlignment(size_store, 1);
        let sequence_ptr = LLVMBuildGEP2(
            self.builder,
            i8_ty,
            record,
            [LLVMConstInt(i64_ty, 8, 0)].as_mut_ptr(),
            1,
            c_name("print_sequence_ptr")?.as_ptr(),
        );
        let sequence_store = LLVMBuildStore(self.builder, sequence, sequence_ptr);
        LLVMSetAlignment(sequence_store, 1);
        let mut cursor = onda_processor_abi::PRINT_RECORD_HEADER_SIZE as u64;
        for (argument, scalar) in arguments.iter().zip(&descriptor.argument_types) {
            let destination = LLVMBuildGEP2(
                self.builder,
                i8_ty,
                record,
                [LLVMConstInt(i64_ty, cursor, 0)].as_mut_ptr(),
                1,
                c_name("print_payload_value")?.as_ptr(),
            );
            let store = LLVMBuildStore(self.builder, self.lower_value(*argument)?, destination);
            LLVMSetAlignment(store, 1);
            cursor += scalar_store_size(*scalar);
        }
        let next_used = LLVMBuildAdd(
            self.builder,
            used,
            LLVMConstInt(
                i32_ty,
                (onda_processor_abi::PRINT_RECORD_HEADER_SIZE + descriptor.payload_size as usize)
                    as u64,
                0,
            ),
            c_name("print_next_used")?.as_ptr(),
        );
        LLVMBuildStore(self.builder, next_used, used_ptr);
        let count_ptr = LLVMBuildStructGEP2(
            self.builder,
            self.module.print_batch_ty,
            batch,
            3,
            c_name("print_count_ptr")?.as_ptr(),
        );
        let count = LLVMBuildLoad2(
            self.builder,
            i32_ty,
            count_ptr,
            c_name("print_count")?.as_ptr(),
        );
        LLVMBuildStore(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                count,
                LLVMConstInt(i32_ty, 1, 0),
                c_name("print_count_next")?.as_ptr(),
            ),
            count_ptr,
        );
        LLVMBuildBr(self.builder, done);
        LLVMPositionBuilderAtEnd(self.builder, done);
        Ok(())
    }

    unsafe fn copy_slice_to_packed_payload(
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
        let store = LLVMBuildStore(self.builder, value, destination_ptr);
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

    unsafe fn lower_if(
        &mut self,
        condition: onda_mir::Value,
        then_block: &Block,
        else_block: &Block,
    ) -> Result<(), MirCodegenError> {
        let condition = self.lower_value(condition)?;
        let then_bb = append_block(self.module.context, self.declaration.value, "if_then")?;
        let else_bb = append_block(self.module.context, self.declaration.value, "if_else")?;
        let merge_bb = append_block(self.module.context, self.declaration.value, "if_merge")?;
        LLVMBuildCondBr(self.builder, condition, then_bb, else_bb);

        LLVMPositionBuilderAtEnd(self.builder, then_bb);
        let then_terminated = self.lower_block(then_block)?;
        if !then_terminated {
            LLVMBuildBr(self.builder, merge_bb);
        }

        LLVMPositionBuilderAtEnd(self.builder, else_bb);
        let else_terminated = self.lower_block(else_block)?;
        if !else_terminated {
            LLVMBuildBr(self.builder, merge_bb);
        }

        LLVMPositionBuilderAtEnd(self.builder, merge_bb);
        if then_terminated && else_terminated {
            LLVMBuildUnreachable(self.builder);
        }
        Ok(())
    }

    unsafe fn lower_loop(&mut self, body: &Block) -> Result<(), MirCodegenError> {
        let body_bb = append_block(self.module.context, self.declaration.value, "loop_body")?;
        let exit_bb = append_block(self.module.context, self.declaration.value, "loop_exit")?;
        LLVMBuildBr(self.builder, body_bb);
        LLVMPositionBuilderAtEnd(self.builder, body_bb);
        self.loop_stack.push((exit_bb, body_bb));
        let terminated = self.lower_block(body)?;
        self.loop_stack.pop();
        if !terminated {
            LLVMBuildBr(self.builder, body_bb);
        }
        LLVMPositionBuilderAtEnd(self.builder, exit_bb);
        Ok(())
    }

    unsafe fn lower_return(&mut self, values: &[onda_mir::Value]) -> Result<(), MirCodegenError> {
        if !matches!(self.function.kind, FunctionKind::User) {
            if !values.is_empty() {
                return Err(MirCodegenError::invalid(
                    "native MIR entry point unexpectedly returns values",
                ));
            }
            LLVMBuildRet(
                self.builder,
                LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0),
            );
            return Ok(());
        }
        match values {
            [] => {
                LLVMBuildRetVoid(self.builder);
            }
            [value] => {
                let value = self.lower_value(*value)?;
                LLVMBuildRet(self.builder, value);
            }
            values => {
                let result_ty = LLVMGetReturnType(self.declaration.ty);
                let mut aggregate = LLVMGetUndef(result_ty);
                for (index, value) in values.iter().enumerate() {
                    let value = self.lower_value(*value)?;
                    aggregate = LLVMBuildInsertValue(
                        self.builder,
                        aggregate,
                        value,
                        index as u32,
                        c_name("return_value")?.as_ptr(),
                    );
                }
                LLVMBuildRet(self.builder, aggregate);
            }
        }
        Ok(())
    }

    unsafe fn lower_call(
        &mut self,
        results: &[onda_mir::LocalId],
        function: onda_mir::FunctionId,
        args: &[CallArgument],
    ) -> Result<(), MirCodegenError> {
        let callee = &self.module.program.functions[function.index()];
        let declaration = self.module.functions[function.index()];
        let mut llvm_args = Vec::with_capacity(args.len() + 1);
        let mut reference_alignments = Vec::with_capacity(args.len());
        llvm_args.push(self.runtime_context);
        for (index, (argument, parameter)) in args.iter().zip(&callee.params).enumerate() {
            let (lowered, reference_alignment) = match parameter.mode {
                onda_mir::PassingMode::Value => (
                    match argument {
                        CallArgument::Value(value) => self.lower_value(*value)?,
                        CallArgument::Place(place) => {
                            let place = self.lower_place(place)?;
                            self.load(place)
                        }
                        CallArgument::Buffer(buffer) => {
                            self.build_external_buffer_descriptor(*buffer, parameter.ty)?
                        }
                        CallArgument::BufferSpan(span) => {
                            self.build_buffer_span(*span, parameter.ty)?
                        }
                        _ => {
                            return Err(MirCodegenError::unsupported(format!(
                                "MIR call argument {index} cannot be passed by value"
                            )));
                        }
                    },
                    None,
                ),
                onda_mir::PassingMode::ReadOnlyReference
                | onda_mir::PassingMode::ReadWriteReference => {
                    let place = match argument {
                        CallArgument::Place(place) => self.lower_place(place)?,
                        CallArgument::SliceElement {
                            slice,
                            index,
                            bounds,
                        } => {
                            let (ptr, _) = self.slice_element_ptr(
                                *slice,
                                *index,
                                *bounds,
                                parameter.mode == onda_mir::PassingMode::ReadWriteReference,
                            )?;
                            PlaceRef {
                                ptr,
                                ty: parameter.ty,
                                alignment: 1,
                            }
                        }
                        CallArgument::BufferParam(reference) => {
                            self.buffer_param_place(*reference, parameter.ty)?
                        }
                        CallArgument::ArrayWindow {
                            array,
                            start,
                            bounds,
                        } => self.array_window_ptr(array, *start, *bounds, parameter.ty)?,
                        CallArgument::SliceWindow {
                            slice,
                            start,
                            bounds,
                        } => self.slice_window_ptr(
                            *slice,
                            *start,
                            *bounds,
                            parameter.ty,
                            parameter.mode == onda_mir::PassingMode::ReadWriteReference,
                        )?,
                        CallArgument::Buffer(buffer) => {
                            let descriptor =
                                self.build_external_buffer_descriptor(*buffer, parameter.ty)?;
                            let ptr = LLVMBuildAlloca(
                                self.builder,
                                self.module.types.get(parameter.ty),
                                c_name("buffer_argument")?.as_ptr(),
                            );
                            LLVMBuildStore(self.builder, descriptor, ptr);
                            PlaceRef {
                                ptr,
                                ty: parameter.ty,
                                alignment: self.module.layouts.type_alignments
                                    [parameter.ty.index()],
                            }
                        }
                        _ => {
                            return Err(MirCodegenError::unsupported(format!(
                                "MIR reference argument {index} is not a place"
                            )));
                        }
                    };
                    (place.ptr, Some(place.alignment))
                }
            };
            reference_alignments.push(reference_alignment);
            llvm_args.push(lowered);
        }
        let call = LLVMBuildCall2(
            self.builder,
            declaration.ty,
            declaration.value,
            llvm_args.as_mut_ptr(),
            llvm_args.len() as u32,
            c_name(if results.is_empty() { "" } else { "call" })?.as_ptr(),
        );
        for (index, (alignment, parameter)) in reference_alignments
            .into_iter()
            .zip(&callee.params)
            .enumerate()
        {
            let Some(alignment) = alignment else {
                continue;
            };
            let llvm_index = index as u32 + 2;
            add_enum_callsite_attribute(
                self.module.context,
                call,
                llvm_index,
                "align",
                alignment as u64,
            )?;
            add_enum_callsite_attribute(self.module.context, call, llvm_index, "nonnull", 0)?;
            add_enum_callsite_attribute(
                self.module.context,
                call,
                llvm_index,
                "dereferenceable",
                self.module.layouts.type_sizes[parameter.ty.index()] as u64,
            )?;
            let parameter_effects = self.module.effects.function(function).parameters[index];
            if !parameter_effects.writes {
                add_enum_callsite_attribute(self.module.context, call, llvm_index, "readonly", 0)?;
            } else if !parameter_effects.reads {
                add_enum_callsite_attribute(self.module.context, call, llvm_index, "writeonly", 0)?;
            }
        }
        if self.module.effects.function(function).may_fail {
            let failure_status = load_context_field(
                self.module,
                self.builder,
                self.runtime_context,
                RUNTIME_FAILURE_CONTEXT_INDEX,
                "runtime_failure",
            )?;
            let failed = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntNE,
                failure_status,
                LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0),
                c_name("call_failed")?.as_ptr(),
            );
            let failure =
                append_block(self.module.context, self.declaration.value, "call_failure")?;
            let success =
                append_block(self.module.context, self.declaration.value, "call_success")?;
            LLVMBuildCondBr(self.builder, failed, failure, success);
            LLVMPositionBuilderAtEnd(self.builder, failure);
            self.emit_failure_return(failure_status)?;
            LLVMPositionBuilderAtEnd(self.builder, success);
        }
        match results {
            [] => {}
            [result] => self.store(self.locals[result.index()], call),
            results => {
                for (index, result) in results.iter().enumerate() {
                    let value = LLVMBuildExtractValue(
                        self.builder,
                        call,
                        index as u32,
                        c_name("call_result")?.as_ptr(),
                    );
                    self.store(self.locals[result.index()], value);
                }
            }
        }
        Ok(())
    }

    unsafe fn array_window_ptr(
        &mut self,
        array: &Place,
        start: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        parameter_ty: onda_mir::TypeId,
    ) -> Result<PlaceRef, MirCodegenError> {
        let Type::Array {
            element: parameter_element,
            len: required_len,
        } = self.module.program.types[parameter_ty.index()]
        else {
            return Err(MirCodegenError::invalid(
                "array-window call target is not a fixed-array reference",
            ));
        };
        let array = self.lower_place(array)?;
        let Type::Array {
            element: source_element,
            len: source_len,
        } = self.module.program.types[array.ty.index()]
        else {
            return Err(MirCodegenError::invalid(
                "array-window source is not a fixed array",
            ));
        };
        if !self
            .module
            .program
            .types_equivalent(source_element, parameter_element)
            || source_len < required_len
        {
            return Err(MirCodegenError::invalid(
                "array-window source does not contain the required parameter shape",
            ));
        }
        let start = self.lower_value(start)?;
        let max_start = source_len - required_len;
        let start = self.normalize_fixed_window_start(start, max_start, bounds)?;
        let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
        let ptr = LLVMBuildGEP2(
            self.builder,
            self.module.types.get(array.ty),
            array.ptr,
            [zero, start].as_mut_ptr(),
            2,
            c_name("array_window")?.as_ptr(),
        );
        Ok(PlaceRef {
            ptr,
            ty: parameter_ty,
            alignment: array
                .alignment
                .min(self.module.layouts.type_alignments[parameter_element.index()]),
        })
    }

    unsafe fn slice_window_ptr(
        &mut self,
        slice: onda_mir::Value,
        start: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        parameter_ty: onda_mir::TypeId,
        write: bool,
    ) -> Result<PlaceRef, MirCodegenError> {
        let Type::Array {
            element,
            len: required_len,
        } = self.module.program.types[parameter_ty.index()]
        else {
            return Err(MirCodegenError::invalid(
                "slice-window call target is not a fixed-array reference",
            ));
        };
        let Type::Scalar(element) = self.module.program.types[element.index()] else {
            return Err(MirCodegenError::invalid(
                "slice-window fixed-array parameter element is not scalar",
            ));
        };
        let parts = self.slice_parts(slice)?;
        if parts.element != element {
            return Err(MirCodegenError::invalid(
                "slice-window element type does not match fixed-array parameter",
            ));
        }
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let required = LLVMConstInt(i32_ty, u64::from(required_len), 0);
        if !matches!(bounds, onda_mir::BoundsMode::Unchecked) {
            let unit_stride = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntEQ,
                parts.stride_bytes,
                LLVMConstInt(i32_ty, scalar_store_size(element), 0),
                c_name("slice_window_unit_stride")?.as_ptr(),
            );
            let too_short = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSLT,
                parts.len,
                required,
                c_name("slice_window_too_short")?.as_ptr(),
            );
            let invalid = LLVMBuildOr(
                self.builder,
                LLVMBuildNot(
                    self.builder,
                    unit_stride,
                    c_name("slice_window_noncontiguous")?.as_ptr(),
                ),
                too_short,
                c_name("slice_window_invalid_shape")?.as_ptr(),
            );
            self.emit_failure_if(invalid, "slice_window_shape_ok")?;
        }
        let max_start = LLVMBuildSub(
            self.builder,
            parts.len,
            required,
            c_name("slice_window_max_start")?.as_ptr(),
        );
        let start = self.lower_value(start)?;
        let start = self.normalize_dynamic_window_start(start, max_start, bounds)?;
        Ok(PlaceRef {
            ptr: self.slice_ptr_at_index(parts, start, "slice_window", write)?,
            ty: parameter_ty,
            alignment: 1,
        })
    }

    unsafe fn normalize_fixed_window_start(
        &mut self,
        start: LLVMValueRef,
        max_start: u32,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        self.normalize_dynamic_window_start(
            start,
            LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                u64::from(max_start),
                0,
            ),
            bounds,
        )
    }

    unsafe fn normalize_dynamic_window_start(
        &mut self,
        start: LLVMValueRef,
        max_start: LLVMValueRef,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        match bounds {
            onda_mir::BoundsMode::Unchecked => Ok(start),
            onda_mir::BoundsMode::Clamp => {
                let below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("window_start_below")?.as_ptr(),
                );
                let low = LLVMBuildSelect(
                    self.builder,
                    below,
                    zero,
                    start,
                    c_name("window_start_low")?.as_ptr(),
                );
                let above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    low,
                    max_start,
                    c_name("window_start_above")?.as_ptr(),
                );
                Ok(LLVMBuildSelect(
                    self.builder,
                    above,
                    max_start,
                    low,
                    c_name("window_start_clamped")?.as_ptr(),
                ))
            }
            onda_mir::BoundsMode::Checked => {
                let below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("window_start_negative")?.as_ptr(),
                );
                let above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    start,
                    max_start,
                    c_name("window_start_out_of_range")?.as_ptr(),
                );
                let invalid = LLVMBuildOr(
                    self.builder,
                    below,
                    above,
                    c_name("window_start_invalid")?.as_ptr(),
                );
                self.emit_failure_if(invalid, "window_start_ok")?;
                Ok(start)
            }
        }
    }

    unsafe fn build_external_buffer_descriptor(
        &mut self,
        buffer: onda_mir::BufferRef,
        ty: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (parts, sample_rate, bound) = match buffer {
            onda_mir::BufferRef::Direct(_) => (
                self.external_buffer_parts(buffer)?,
                self.lower_external_buffer_metadata(buffer, 4)?,
                self.lower_external_buffer_is_bound(buffer)?,
            ),
            onda_mir::BufferRef::ArrayElement { .. } => {
                // The selector is an arbitrary MIR value and must be evaluated
                // exactly once while constructing a forwarded descriptor.
                let runtime_index = self.lower_buffer_ref_index(buffer)?;
                (
                    self.external_buffer_parts_at(buffer, runtime_index)?,
                    self.lower_external_buffer_metadata_at(runtime_index, 4)?,
                    self.lower_external_buffer_is_bound_at(runtime_index)?,
                )
            }
        };
        let mut descriptor = LLVMGetUndef(self.module.types.get(ty));
        for (index, value) in [
            parts.read_ptr,
            parts.write_ptr,
            parts.frames,
            parts.channels,
            sample_rate,
            bound,
        ]
        .into_iter()
        .enumerate()
        {
            descriptor = LLVMBuildInsertValue(
                self.builder,
                descriptor,
                value,
                index as u32,
                c_name("buffer_descriptor")?.as_ptr(),
            );
        }
        Ok(descriptor)
    }

    unsafe fn lower_rvalue(
        &mut self,
        rvalue: &Rvalue,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        match rvalue {
            Rvalue::Use(value) => self.lower_value(*value),
            Rvalue::Load(place) => {
                let lowered = self.lower_place(place)?;
                let load = self.load(lowered);
                if let Some(range) = self.direct_place_integer_range(place) {
                    self.mark_integer_range(load, range);
                }
                Ok(load)
            }
            Rvalue::InitAll => load_context_field(
                self.module,
                self.builder,
                self.runtime_context,
                INIT_ALL_CONTEXT_INDEX,
                "init_all",
            ),
            Rvalue::Unary { op, operand } => self.lower_unary(*op, *operand),
            Rvalue::Binary { op, lhs, rhs } => self.lower_binary(*op, *lhs, *rhs),
            Rvalue::Compare { op, lhs, rhs } => self.lower_compare(*op, *lhs, *rhs),
            Rvalue::Cast { value, to } => self.lower_cast(*value, *to),
            Rvalue::Intrinsic { intrinsic, args } => self.lower_intrinsic(*intrinsic, args),
            Rvalue::ProcessFrame { offset } => self.lower_process_frame(*offset),
            Rvalue::InputLoad {
                input,
                element,
                bounds,
                frame,
            } => self.lower_input_load(*input, *element, *bounds, *frame),
            Rvalue::OutputLoad {
                output,
                element,
                bounds,
                frame,
            } => self.lower_output_load(*output, *element, *bounds, *frame),
            Rvalue::BufferLoad {
                buffer,
                channel,
                index,
                bounds,
            } => self.lower_buffer_load(*buffer, *channel, *index, *bounds),
            Rvalue::BufferParamLoad {
                parameter,
                channel,
                index,
                bounds,
            } => self.lower_buffer_param_load(*parameter, *channel, *index, *bounds),
            Rvalue::BufferLen(buffer) => self.lower_external_buffer_metadata(*buffer, 2),
            Rvalue::BufferChannels(buffer) => self.lower_external_buffer_channels(*buffer),
            Rvalue::BufferSampleRate(buffer) => self.lower_external_buffer_metadata(*buffer, 4),
            Rvalue::BufferIsBound(buffer) => self.lower_external_buffer_is_bound(*buffer),
            Rvalue::BufferParamLen(parameter) => self.lower_buffer_param_metadata(*parameter, 2),
            Rvalue::BufferParamChannels(parameter) => {
                self.lower_buffer_param_metadata(*parameter, 3)
            }
            Rvalue::BufferParamSampleRate(parameter) => {
                self.lower_buffer_param_metadata(*parameter, 4)
            }
            Rvalue::BufferParamIsBound(parameter) => {
                self.lower_buffer_param_metadata(*parameter, 5)
            }
            Rvalue::ConstDataLoad {
                data,
                index,
                bounds,
            } => self.lower_const_data_load(*data, *index, *bounds),
            Rvalue::MakeSlice {
                source,
                start,
                len,
                bounds,
                access: _,
            } => self.lower_make_slice(source, *start, *len, *bounds, expected),
            Rvalue::SliceLoad {
                slice,
                index,
                bounds,
            } => self.lower_slice_load(*slice, *index, *bounds),
            Rvalue::SliceLen(slice) => self.lower_slice_len(*slice),
        }
    }

    unsafe fn lower_process_frame(
        &mut self,
        offset: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let offset = self.lower_value(offset)?;
        let start_frame = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            2,
            "process_start_frame",
        )?;
        let frames = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            3,
            "process_frames",
        )?;
        // One unsigned comparison covers both sides of the signed range:
        // negative offsets become large unsigned values. This shape also
        // lets LLVM fold the check directly into canonical 0..frames loops.
        let valid = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULT,
            offset,
            frames,
            c_name("process_frame_valid")?.as_ptr(),
        );
        let invalid = LLVMBuildNot(
            self.builder,
            valid,
            c_name("process_frame_invalid")?.as_ptr(),
        );
        self.emit_failure_if(invalid, "process_frame_ok")?;
        Ok(LLVMBuildAdd(
            self.builder,
            start_frame,
            offset,
            c_name("process_frame")?.as_ptr(),
        ))
    }

    unsafe fn lower_value(
        &mut self,
        value: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        Ok(match value {
            onda_mir::Value::Local(local) => self.load(self.locals[local.index()]),
            onda_mir::Value::Constant(value) => llvm_scalar_constant(self.module.context, value),
        })
    }

    unsafe fn lower_unary(
        &mut self,
        op: onda_mir::UnaryOp,
        operand: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(operand)?;
        let operand = self.lower_value(operand)?;
        let name = c_name("unary")?;
        Ok(match op {
            onda_mir::UnaryOp::Negate if is_float(scalar) => {
                let value = LLVMBuildFNeg(self.builder, operand, name.as_ptr());
                self.set_fast_math(value);
                value
            }
            onda_mir::UnaryOp::Negate => LLVMBuildNeg(self.builder, operand, name.as_ptr()),
            onda_mir::UnaryOp::LogicalNot => LLVMBuildNot(self.builder, operand, name.as_ptr()),
            onda_mir::UnaryOp::BitNot => LLVMBuildNot(self.builder, operand, name.as_ptr()),
        })
    }

    unsafe fn lower_binary(
        &mut self,
        op: onda_mir::BinaryOp,
        lhs: onda_mir::Value,
        rhs: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(lhs)?;
        let lhs = self.lower_value(lhs)?;
        let rhs = self.lower_value(rhs)?;
        let name = c_name("binary")?;
        let value = if is_float(scalar) {
            match op {
                onda_mir::BinaryOp::Add => LLVMBuildFAdd(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Subtract => {
                    LLVMBuildFSub(self.builder, lhs, rhs, name.as_ptr())
                }
                onda_mir::BinaryOp::Multiply => {
                    LLVMBuildFMul(self.builder, lhs, rhs, name.as_ptr())
                }
                onda_mir::BinaryOp::Divide => LLVMBuildFDiv(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Remainder => {
                    LLVMBuildFRem(self.builder, lhs, rhs, name.as_ptr())
                }
                _ => {
                    return Err(MirCodegenError::invalid(
                        "bitwise MIR operation has floating-point operands",
                    ));
                }
            }
        } else {
            match op {
                onda_mir::BinaryOp::Add => LLVMBuildAdd(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Subtract => LLVMBuildSub(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Multiply => LLVMBuildMul(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Divide | onda_mir::BinaryOp::Remainder => {
                    self.lower_signed_division_or_remainder(op, scalar, lhs, rhs)?
                }
                onda_mir::BinaryOp::BitAnd => LLVMBuildAnd(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::BitOr => LLVMBuildOr(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::BitXor => LLVMBuildXor(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::ShiftLeft => {
                    let rhs = self.mask_shift_count(scalar, rhs)?;
                    LLVMBuildShl(self.builder, lhs, rhs, name.as_ptr())
                }
                onda_mir::BinaryOp::ShiftRight => {
                    let rhs = self.mask_shift_count(scalar, rhs)?;
                    LLVMBuildAShr(self.builder, lhs, rhs, name.as_ptr())
                }
            }
        };
        if is_float(scalar) {
            self.set_fast_math(value);
        }
        Ok(value)
    }

    unsafe fn mask_shift_count(
        &self,
        scalar: onda_mir::ScalarType,
        rhs: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let bits = match scalar {
            onda_mir::ScalarType::I32 => 32_u64,
            onda_mir::ScalarType::I64 => 64_u64,
            _ => {
                return Err(MirCodegenError::invalid(
                    "shift operation requires an i32 or i64 operand",
                ));
            }
        };
        let ty = llvm_scalar_type(self.module.context, scalar);
        Ok(LLVMBuildAnd(
            self.builder,
            rhs,
            LLVMConstInt(ty, bits - 1, 0),
            c_name("masked_shift_count")?.as_ptr(),
        ))
    }

    unsafe fn lower_signed_division_or_remainder(
        &mut self,
        op: onda_mir::BinaryOp,
        scalar: onda_mir::ScalarType,
        lhs: LLVMValueRef,
        rhs: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let bits = match scalar {
            onda_mir::ScalarType::I32 => 32_u32,
            onda_mir::ScalarType::I64 => 64_u32,
            _ => {
                return Err(MirCodegenError::invalid(
                    "integer division requires an i32 or i64 operand",
                ));
            }
        };
        let ty = llvm_scalar_type(self.module.context, scalar);
        let zero = LLVMConstNull(ty);
        let divisor_is_zero = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            rhs,
            zero,
            c_name("division_by_zero")?.as_ptr(),
        );
        self.emit_failure_if(divisor_is_zero, "division_nonzero")?;

        // LLVM makes signed MIN / -1 and MIN % -1 poison. MIR instead uses
        // two's-complement wrapping division semantics: quotient MIN,
        // remainder zero.
        let min = LLVMConstInt(ty, 1_u64 << (bits - 1), 0);
        let minus_one = LLVMConstInt(ty, u64::MAX, 1);
        let lhs_is_min = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            lhs,
            min,
            c_name("division_lhs_is_min")?.as_ptr(),
        );
        let rhs_is_minus_one = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            rhs,
            minus_one,
            c_name("division_rhs_is_minus_one")?.as_ptr(),
        );
        let overflow = LLVMBuildAnd(
            self.builder,
            lhs_is_min,
            rhs_is_minus_one,
            c_name("division_overflow")?.as_ptr(),
        );
        let one = LLVMConstInt(ty, 1, 0);
        let safe_rhs = LLVMBuildSelect(
            self.builder,
            overflow,
            one,
            rhs,
            c_name("division_safe_rhs")?.as_ptr(),
        );
        let raw = match op {
            onda_mir::BinaryOp::Divide => {
                LLVMBuildSDiv(self.builder, lhs, safe_rhs, c_name("division")?.as_ptr())
            }
            onda_mir::BinaryOp::Remainder => {
                LLVMBuildSRem(self.builder, lhs, safe_rhs, c_name("remainder")?.as_ptr())
            }
            _ => unreachable!("only division and remainder use this lowering"),
        };
        Ok(LLVMBuildSelect(
            self.builder,
            overflow,
            if matches!(op, onda_mir::BinaryOp::Divide) {
                min
            } else {
                zero
            },
            raw,
            c_name("division_result")?.as_ptr(),
        ))
    }

    unsafe fn emit_failure_if(
        &mut self,
        failed: LLVMValueRef,
        ok_name: &str,
    ) -> Result<(), MirCodegenError> {
        let ok = append_block(self.module.context, self.declaration.value, ok_name)?;
        let failure = append_block(self.module.context, self.declaration.value, "failure")?;
        LLVMBuildCondBr(self.builder, failed, failure, ok);
        LLVMPositionBuilderAtEnd(self.builder, failure);
        self.emit_failure_return(LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            u64::from(crate::PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE),
            0,
        ))?;
        LLVMPositionBuilderAtEnd(self.builder, ok);
        Ok(())
    }

    unsafe fn emit_failure_return(
        &mut self,
        failure_status: LLVMValueRef,
    ) -> Result<(), MirCodegenError> {
        let failure_ptr = context_field_ptr(
            self.module,
            self.builder,
            self.runtime_context,
            RUNTIME_FAILURE_CONTEXT_INDEX,
        )?;
        LLVMBuildStore(self.builder, failure_status, failure_ptr);
        if !self.module.program.interface.delegates.is_empty()
            && matches!(
                self.function.kind,
                FunctionKind::Init | FunctionKind::Process | FunctionKind::Event(_)
            )
        {
            // Nested helpers propagate failure through the runtime status slot.
            // Clearing belongs at the generated host-call boundary; doing it
            // here would give otherwise pure helpers a delegate-batch memory
            // effect and inhibit interprocedural optimization.
            reset_delegate_batch(self.module, self.builder, self.runtime_context)?;
        }
        if matches!(self.function.kind, FunctionKind::User) {
            let result_ty = LLVMGetReturnType(self.declaration.ty);
            if LLVMGetTypeKind(result_ty) == llvm_sys::LLVMTypeKind::LLVMVoidTypeKind {
                LLVMBuildRetVoid(self.builder);
            } else {
                LLVMBuildRet(self.builder, LLVMConstNull(result_ty));
            }
        } else {
            LLVMBuildRet(self.builder, failure_status);
        }
        Ok(())
    }

    unsafe fn lower_compare(
        &mut self,
        op: onda_mir::CompareOp,
        lhs: onda_mir::Value,
        rhs: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(lhs)?;
        let lhs = self.lower_value(lhs)?;
        let rhs = self.lower_value(rhs)?;
        if is_float(scalar) {
            let predicate = match op {
                onda_mir::CompareOp::Equal => LLVMRealPredicate::LLVMRealOEQ,
                onda_mir::CompareOp::NotEqual => LLVMRealPredicate::LLVMRealUNE,
                onda_mir::CompareOp::Less => LLVMRealPredicate::LLVMRealOLT,
                onda_mir::CompareOp::LessEqual => LLVMRealPredicate::LLVMRealOLE,
                onda_mir::CompareOp::Greater => LLVMRealPredicate::LLVMRealOGT,
                onda_mir::CompareOp::GreaterEqual => LLVMRealPredicate::LLVMRealOGE,
            };
            let value = LLVMBuildFCmp(self.builder, predicate, lhs, rhs, c_name("fcmp")?.as_ptr());
            self.set_fast_math(value);
            Ok(value)
        } else {
            let predicate = match op {
                onda_mir::CompareOp::Equal => LLVMIntPredicate::LLVMIntEQ,
                onda_mir::CompareOp::NotEqual => LLVMIntPredicate::LLVMIntNE,
                onda_mir::CompareOp::Less => LLVMIntPredicate::LLVMIntSLT,
                onda_mir::CompareOp::LessEqual => LLVMIntPredicate::LLVMIntSLE,
                onda_mir::CompareOp::Greater => LLVMIntPredicate::LLVMIntSGT,
                onda_mir::CompareOp::GreaterEqual => LLVMIntPredicate::LLVMIntSGE,
            };
            Ok(LLVMBuildICmp(
                self.builder,
                predicate,
                lhs,
                rhs,
                c_name("icmp")?.as_ptr(),
            ))
        }
    }

    unsafe fn lower_cast(
        &mut self,
        value: onda_mir::Value,
        to: onda_mir::ScalarType,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let from = self.scalar_type_of_value(value)?;
        let value = self.lower_value(value)?;
        if from == to {
            return Ok(value);
        }
        let to_ty = llvm_scalar_type(self.module.context, to);
        let name = c_name("cast")?;
        Ok(match (from, to) {
            (onda_mir::ScalarType::F32, onda_mir::ScalarType::F64) => {
                LLVMBuildFPExt(self.builder, value, to_ty, name.as_ptr())
            }
            (onda_mir::ScalarType::F64, onda_mir::ScalarType::F32) => {
                LLVMBuildFPTrunc(self.builder, value, to_ty, name.as_ptr())
            }
            (from, to) if is_float(from) && is_integer(to) => {
                let from_suffix = if from == onda_mir::ScalarType::F64 {
                    "f64"
                } else {
                    "f32"
                };
                let to_suffix = if to == onda_mir::ScalarType::I64 {
                    "i64"
                } else {
                    "i32"
                };
                let intrinsic_name = format!("llvm.fptosi.sat.{to_suffix}.{from_suffix}");
                let from_ty = llvm_scalar_type(self.module.context, from);
                let mut parameter_types = [from_ty];
                let fn_ty = LLVMFunctionType(to_ty, parameter_types.as_mut_ptr(), 1, 0);
                let function = ensure_named_function(self.module.module, &intrinsic_name, fn_ty)?;
                LLVMBuildCall2(
                    self.builder,
                    fn_ty,
                    function,
                    [value].as_mut_ptr(),
                    1,
                    name.as_ptr(),
                )
            }
            (from, to) if is_integer(from) && is_float(to) => {
                LLVMBuildSIToFP(self.builder, value, to_ty, name.as_ptr())
            }
            (onda_mir::ScalarType::I32, onda_mir::ScalarType::I64) => {
                LLVMBuildSExt(self.builder, value, to_ty, name.as_ptr())
            }
            (onda_mir::ScalarType::I64, onda_mir::ScalarType::I32) => {
                LLVMBuildTrunc(self.builder, value, to_ty, name.as_ptr())
            }
            _ => {
                return Err(MirCodegenError::invalid(format!(
                    "unsupported validated numeric cast {from:?} to {to:?}"
                )));
            }
        })
    }

    unsafe fn lower_intrinsic(
        &mut self,
        intrinsic: onda_mir::Intrinsic,
        args: &[onda_mir::Value],
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(args[0])?;
        let mut lowered = args
            .iter()
            .map(|value| self.lower_value(*value))
            .collect::<Result<Vec<_>, _>>()?;
        if intrinsic == onda_mir::Intrinsic::RangeClamp {
            if matches!(
                scalar,
                onda_mir::ScalarType::I32 | onda_mir::ScalarType::I64
            ) {
                let lower = self.lower_integer_intrinsic(
                    onda_mir::Intrinsic::Max,
                    scalar,
                    &mut vec![lowered[0], lowered[1]],
                )?;
                return self.lower_integer_intrinsic(
                    onda_mir::Intrinsic::Min,
                    scalar,
                    &mut vec![lower, lowered[2]],
                );
            }
            let suffix = if scalar == onda_mir::ScalarType::F64 {
                "f64"
            } else {
                "f32"
            };
            let scalar_ty = llvm_scalar_type(self.module.context, scalar);
            let lower = self.lower_binary_float_intrinsic(
                &format!("llvm.maxnum.{suffix}"),
                scalar_ty,
                lowered[0],
                lowered[1],
                "range_clamp_lower",
            )?;
            return self.lower_binary_float_intrinsic(
                &format!("llvm.minnum.{suffix}"),
                scalar_ty,
                lower,
                lowered[2],
                "range_clamp_upper",
            );
        }
        if intrinsic == onda_mir::Intrinsic::RangeWrap {
            if !matches!(
                scalar,
                onda_mir::ScalarType::I32 | onda_mir::ScalarType::I64
            ) {
                return Err(MirCodegenError::invalid(
                    "range_wrap requires an i32 or i64 value",
                ));
            }
            let scalar_ty = llvm_scalar_type(self.module.context, scalar);
            let full_domain = match (args[1], args[2]) {
                (
                    onda_mir::Value::Constant(onda_mir::ScalarValue::I32(lower)),
                    onda_mir::Value::Constant(onda_mir::ScalarValue::I32(upper)),
                ) => lower == i32::MIN && upper == i32::MAX,
                (
                    onda_mir::Value::Constant(onda_mir::ScalarValue::I64(lower)),
                    onda_mir::Value::Constant(onda_mir::ScalarValue::I64(upper)),
                ) => lower == i64::MIN && upper == i64::MAX,
                _ => unreachable!("range_wrap bounds were validated as matching constants"),
            };
            if full_domain {
                return Ok(lowered[0]);
            }
            // In two's-complement arithmetic, this unsigned distance check is
            // equivalent to `lower <= value && value <= upper`, including
            // ranges whose inclusive span crosses the signed midpoint. It
            // gives the hot path one subtraction, one comparison, and a
            // predictable branch.
            let distance_from_lower = LLVMBuildSub(
                self.builder,
                lowered[0],
                lowered[1],
                c_name("range_wrap_distance")?.as_ptr(),
            );
            let range_span = LLVMBuildSub(
                self.builder,
                lowered[2],
                lowered[1],
                c_name("range_wrap_span_narrow")?.as_ptr(),
            );
            let in_range = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntULE,
                distance_from_lower,
                range_span,
                c_name("range_wrap_in_range")?.as_ptr(),
            );
            let preheader = LLVMGetInsertBlock(self.builder);
            let wrap_block = append_block(
                self.module.context,
                self.declaration.value,
                "range_wrap_slow",
            )?;
            let merge_block = append_block(
                self.module.context,
                self.declaration.value,
                "range_wrap_merge",
            )?;
            LLVMBuildCondBr(self.builder, in_range, merge_block, wrap_block);

            LLVMPositionBuilderAtEnd(self.builder, wrap_block);
            let below = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSLT,
                lowered[0],
                lowered[1],
                c_name("range_wrap_below")?.as_ptr(),
            );
            // For a non-full-domain inclusive range, its width is representable
            // as a non-zero unsigned value of the source type. Splitting values
            // below and above the range lets both distances remain exact under
            // modular arithmetic, avoiding a widened remainder operation.
            let one = LLVMConstInt(scalar_ty, 1, 0);
            let width = LLVMBuildAdd(
                self.builder,
                range_span,
                one,
                c_name("range_wrap_width")?.as_ptr(),
            );
            let below_block = append_block(
                self.module.context,
                self.declaration.value,
                "range_wrap_below",
            )?;
            let above_block = append_block(
                self.module.context,
                self.declaration.value,
                "range_wrap_above",
            )?;
            LLVMBuildCondBr(self.builder, below, below_block, above_block);

            LLVMPositionBuilderAtEnd(self.builder, below_block);
            let distance_below = LLVMBuildSub(
                self.builder,
                LLVMBuildSub(
                    self.builder,
                    lowered[1],
                    one,
                    c_name("range_wrap_before_lower")?.as_ptr(),
                ),
                lowered[0],
                c_name("range_wrap_distance_below")?.as_ptr(),
            );
            let below_remainder = LLVMBuildURem(
                self.builder,
                distance_below,
                width,
                c_name("range_wrap_remainder_below")?.as_ptr(),
            );
            let wrapped_below = LLVMBuildSub(
                self.builder,
                lowered[2],
                below_remainder,
                c_name("range_wrapped_below")?.as_ptr(),
            );
            LLVMBuildBr(self.builder, merge_block);

            LLVMPositionBuilderAtEnd(self.builder, above_block);
            let distance_above = LLVMBuildSub(
                self.builder,
                lowered[0],
                LLVMBuildAdd(
                    self.builder,
                    lowered[2],
                    one,
                    c_name("range_wrap_after_upper")?.as_ptr(),
                ),
                c_name("range_wrap_distance_above")?.as_ptr(),
            );
            let above_remainder = LLVMBuildURem(
                self.builder,
                distance_above,
                width,
                c_name("range_wrap_remainder_above")?.as_ptr(),
            );
            let wrapped_above = LLVMBuildAdd(
                self.builder,
                lowered[1],
                above_remainder,
                c_name("range_wrapped_above")?.as_ptr(),
            );
            LLVMBuildBr(self.builder, merge_block);

            LLVMPositionBuilderAtEnd(self.builder, merge_block);
            let result = LLVMBuildPhi(
                self.builder,
                scalar_ty,
                c_name("range_wrap_value_or_wrapped")?.as_ptr(),
            );
            LLVMAddIncoming(
                result,
                [lowered[0], wrapped_below, wrapped_above].as_mut_ptr(),
                [preheader, below_block, above_block].as_mut_ptr(),
                3,
            );
            return Ok(result);
        }
        if matches!(
            scalar,
            onda_mir::ScalarType::I32 | onda_mir::ScalarType::I64
        ) {
            return self.lower_integer_intrinsic(intrinsic, scalar, &mut lowered);
        }
        let suffix = if scalar == onda_mir::ScalarType::F64 {
            "f64"
        } else {
            "f32"
        };
        let base = match intrinsic {
            onda_mir::Intrinsic::Sin => "llvm.sin",
            onda_mir::Intrinsic::Cos => "llvm.cos",
            onda_mir::Intrinsic::Tan => {
                if suffix == "f64" {
                    "tan"
                } else {
                    "tanf"
                }
            }
            onda_mir::Intrinsic::Tanh => {
                if suffix == "f64" {
                    "tanh"
                } else {
                    "tanhf"
                }
            }
            onda_mir::Intrinsic::Atan => {
                if suffix == "f64" {
                    "atan"
                } else {
                    "atanf"
                }
            }
            onda_mir::Intrinsic::Atan2 => {
                if suffix == "f64" {
                    "atan2"
                } else {
                    "atan2f"
                }
            }
            onda_mir::Intrinsic::Exp => "llvm.exp",
            onda_mir::Intrinsic::Log => "llvm.log",
            onda_mir::Intrinsic::Sqrt => "llvm.sqrt",
            onda_mir::Intrinsic::Pow => "llvm.pow",
            onda_mir::Intrinsic::Abs => "llvm.fabs",
            onda_mir::Intrinsic::Floor => "llvm.floor",
            onda_mir::Intrinsic::Ceil => "llvm.ceil",
            onda_mir::Intrinsic::Round => "llvm.round",
            onda_mir::Intrinsic::Trunc => "llvm.trunc",
            onda_mir::Intrinsic::Min => "llvm.minimum",
            onda_mir::Intrinsic::Max => "llvm.maximum",
            onda_mir::Intrinsic::Fma => "llvm.fma",
            onda_mir::Intrinsic::RangeClamp => {
                unreachable!("range clamp lowers before ordinary float intrinsics")
            }
            onda_mir::Intrinsic::RangeWrap => {
                unreachable!("range wrap lowers before ordinary intrinsics")
            }
        };
        let name = if base.starts_with("llvm.") {
            format!("{base}.{suffix}")
        } else {
            base.to_owned()
        };
        let scalar_ty = llvm_scalar_type(self.module.context, scalar);
        let mut parameter_types = vec![scalar_ty; lowered.len()];
        let fn_ty = LLVMFunctionType(
            scalar_ty,
            parameter_types.as_mut_ptr(),
            parameter_types.len() as u32,
            0,
        );
        let function = ensure_named_function(self.module.module, &name, fn_ty)?;
        let call = LLVMBuildCall2(
            self.builder,
            fn_ty,
            function,
            lowered.as_mut_ptr(),
            lowered.len() as u32,
            c_name("intrinsic")?.as_ptr(),
        );
        self.set_fast_math(call);
        Ok(call)
    }

    unsafe fn lower_integer_intrinsic(
        &mut self,
        intrinsic: onda_mir::Intrinsic,
        scalar: onda_mir::ScalarType,
        lowered: &mut Vec<LLVMValueRef>,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let bits = if scalar == onda_mir::ScalarType::I64 {
            64
        } else {
            32
        };
        let scalar_ty = llvm_scalar_type(self.module.context, scalar);
        let (name, extra_bool) = match intrinsic {
            onda_mir::Intrinsic::Abs => (format!("llvm.abs.i{bits}"), true),
            onda_mir::Intrinsic::Min => (format!("llvm.smin.i{bits}"), false),
            onda_mir::Intrinsic::Max => (format!("llvm.smax.i{bits}"), false),
            _ => {
                return Err(MirCodegenError::invalid(format!(
                    "integer MIR intrinsic {intrinsic:?} is not supported by validation"
                )));
            }
        };
        let mut parameter_types = vec![scalar_ty; lowered.len()];
        if extra_bool {
            parameter_types.push(LLVMInt1TypeInContext(self.module.context));
            lowered.push(LLVMConstInt(
                LLVMInt1TypeInContext(self.module.context),
                0,
                0,
            ));
        }
        let fn_ty = LLVMFunctionType(
            scalar_ty,
            parameter_types.as_mut_ptr(),
            parameter_types.len() as u32,
            0,
        );
        let function = ensure_named_function(self.module.module, &name, fn_ty)?;
        Ok(LLVMBuildCall2(
            self.builder,
            fn_ty,
            function,
            lowered.as_mut_ptr(),
            lowered.len() as u32,
            c_name("integer_intrinsic")?.as_ptr(),
        ))
    }

    unsafe fn lower_input_load(
        &mut self,
        input: onda_mir::InputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (scalar, port) = self.interface_port(
            self.module.program.interface.inputs[input.index()].ty,
            self.module.layouts.input_bases[input.index()],
            element,
            bounds,
        )?;
        self.load_audio_sample(0, scalar, port, frame)
    }

    unsafe fn lower_output_load(
        &mut self,
        output: onda_mir::OutputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (scalar, port) = self.interface_port(
            self.module.program.interface.outputs[output.index()].ty,
            self.module.layouts.output_bases[output.index()],
            element,
            bounds,
        )?;
        self.load_audio_sample(1, scalar, port, frame)
    }

    unsafe fn lower_output_store(
        &mut self,
        output: onda_mir::OutputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        frame: onda_mir::Value,
        value: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let (scalar, port) = self.interface_port(
            self.module.program.interface.outputs[output.index()].ty,
            self.module.layouts.output_bases[output.index()],
            element,
            bounds,
        )?;
        let value = self.lower_value(value)?;
        let ptr = self.audio_sample_ptr(1, scalar, port, frame)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, scalar_store_size(scalar) as u32);
        self.mark_audio_output_access(store);
        Ok(())
    }

    unsafe fn lower_control_output_store(
        &mut self,
        output: onda_mir::ControlOutputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        value: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let descriptor = &self.module.program.interface.control_outputs[output.index()];
        let state_ptr = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            6,
            "state_ptr",
        )?;
        let ptr = byte_offset_ptr(
            self.module.context,
            self.builder,
            state_ptr,
            self.module.layouts.control_offsets[output.index()],
            "control_output",
        )?;
        let mut place = PlaceRef {
            ptr,
            ty: descriptor.ty,
            alignment: self.module.layouts.type_alignments[descriptor.ty.index()],
        };
        if let Some(index) = element {
            place = self.project_array(place, index, bounds)?;
        }
        let value = self.lower_value(value)?;
        self.store(place, value);
        Ok(())
    }

    unsafe fn load_audio_sample(
        &mut self,
        context_field: u32,
        scalar: onda_mir::ScalarType,
        port: LLVMValueRef,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let ptr = self.audio_sample_ptr(context_field, scalar, port, frame)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, scalar),
            ptr,
            c_name("audio_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, scalar_store_size(scalar) as u32);
        if context_field == 1 {
            self.mark_audio_output_access(load);
        }
        Ok(load)
    }

    unsafe fn audio_sample_ptr(
        &mut self,
        context_field: u32,
        scalar: onda_mir::ScalarType,
        port: LLVMValueRef,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let ports = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            context_field,
            "audio_ports",
        )?;
        let port_ptr = LLVMBuildGEP2(
            self.builder,
            self.module.ptr_ty,
            ports,
            [port].as_mut_ptr(),
            1,
            c_name("audio_port_ptr")?.as_ptr(),
        );
        let channel = LLVMBuildLoad2(
            self.builder,
            self.module.ptr_ty,
            port_ptr,
            c_name("audio_channel")?.as_ptr(),
        );
        // Segmented process MIR already computes the logical I/O frame as
        // `start_frame + local_frame`. Host pointers address the full block,
        // so the ABI start must not be added again here.
        let logical_frame = self.lower_value(frame)?;
        Ok(LLVMBuildGEP2(
            self.builder,
            llvm_scalar_type(self.module.context, scalar),
            channel,
            [logical_frame].as_mut_ptr(),
            1,
            c_name("audio_sample_ptr")?.as_ptr(),
        ))
    }

    unsafe fn interface_port(
        &mut self,
        ty: onda_mir::TypeId,
        base: usize,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(onda_mir::ScalarType, LLVMValueRef), MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        match &self.module.program.types[ty.index()] {
            Type::Scalar(scalar) => Ok((*scalar, LLVMConstInt(i32_ty, base as u64, 0))),
            Type::Array { element: item, len } => {
                let Type::Scalar(scalar) = self.module.program.types[item.index()] else {
                    return Err(MirCodegenError::unsupported(
                        "nested arrays are not audio interface values",
                    ));
                };
                let element = element.ok_or_else(|| {
                    MirCodegenError::invalid("array audio interface access has no element index")
                })?;
                let index =
                    self.lower_fixed_index(element, usize::try_from(*len).unwrap(), bounds)?;
                let port = LLVMBuildAdd(
                    self.builder,
                    LLVMConstInt(i32_ty, base as u64, 0),
                    index,
                    c_name("audio_port")?.as_ptr(),
                );
                Ok((scalar, port))
            }
            _ => Err(MirCodegenError::unsupported(
                "unsupported audio interface aggregate",
            )),
        }
    }

    unsafe fn lower_buffer_ref_index(
        &mut self,
        buffer: onda_mir::BufferRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        match buffer {
            onda_mir::BufferRef::Direct(buffer) => {
                Ok(LLVMConstInt(i32_ty, u64::from(buffer.raw()), 0))
            }
            onda_mir::BufferRef::ArrayElement {
                first,
                len,
                selector,
                bounds,
            } => {
                let selector = self.lower_value(selector)?;
                let len = LLVMConstInt(i32_ty, u64::from(len), 0);
                let selector = self.apply_dynamic_bounds(selector, len, bounds)?;
                Ok(LLVMBuildAdd(
                    self.builder,
                    LLVMConstInt(i32_ty, u64::from(first.raw()), 0),
                    selector,
                    c_name("buffer_array_index")?.as_ptr(),
                ))
            }
        }
    }

    unsafe fn lower_external_buffer_metadata(
        &mut self,
        buffer: onda_mir::BufferRef,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        match buffer {
            onda_mir::BufferRef::Direct(buffer) => {
                self.snapshot_direct_buffer_field(buffer, descriptor_field)
            }
            onda_mir::BufferRef::ArrayElement { .. } => {
                let index = self.lower_buffer_ref_index(buffer)?;
                self.lower_external_buffer_metadata_at(index, descriptor_field)
            }
        }
    }

    unsafe fn lower_external_buffer_channels(
        &mut self,
        buffer: onda_mir::BufferRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let channels = self.module.program.interface.buffers[buffer.index()].channels;
        match channels {
            onda_mir::BufferChannels::Mono => Ok(LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                1,
                0,
            )),
            onda_mir::BufferChannels::Static(channels) => Ok(LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                channels as u64,
                0,
            )),
            onda_mir::BufferChannels::Dynamic => self.lower_external_buffer_metadata(buffer, 3),
        }
    }

    unsafe fn lower_external_buffer_metadata_at(
        &mut self,
        index: LLVMValueRef,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        self.load_external_buffer_metadata_at(self.builder, index, descriptor_field)
    }

    unsafe fn lower_external_buffer_is_bound(
        &mut self,
        buffer: onda_mir::BufferRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let raw = match buffer {
            onda_mir::BufferRef::Direct(buffer) => self.snapshot_direct_buffer_field(buffer, 5)?,
            onda_mir::BufferRef::ArrayElement { .. } => {
                let index = self.lower_buffer_ref_index(buffer)?;
                self.load_external_buffer_metadata_at(self.builder, index, 5)?
            }
        };
        self.buffer_pointer_is_bound(self.builder, raw)
    }

    unsafe fn lower_external_buffer_is_bound_at(
        &self,
        index: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let raw = self.load_external_buffer_metadata_at(self.builder, index, 5)?;
        self.buffer_pointer_is_bound(self.builder, raw)
    }

    unsafe fn buffer_pointer_is_bound(
        &self,
        builder: LLVMBuilderRef,
        pointer: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        Ok(LLVMBuildICmp(
            builder,
            LLVMIntPredicate::LLVMIntNE,
            pointer,
            LLVMConstPointerNull(self.module.ptr_ty),
            c_name("buffer_is_bound")?.as_ptr(),
        ))
    }

    unsafe fn load_external_buffer_metadata_at(
        &self,
        builder: LLVMBuilderRef,
        index: LLVMValueRef,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (element_ty, context_field, name) = self.buffer_table_component(descriptor_field)?;
        let values = load_context_field(
            self.module,
            builder,
            self.runtime_context,
            context_field,
            name,
        )?;
        let ptr = LLVMBuildGEP2(
            builder,
            element_ty,
            values,
            [index].as_mut_ptr(),
            1,
            c_name(name)?.as_ptr(),
        );
        let load = LLVMBuildLoad2(builder, element_ty, ptr, c_name(name)?.as_ptr());
        self.mark_external_buffer_descriptor_access(load);
        if descriptor_field <= 1 {
            self.resolve_buffer_pointer(builder, load, descriptor_field == 1)
        } else {
            Ok(load)
        }
    }

    unsafe fn resolve_buffer_pointer(
        &self,
        builder: LLVMBuilderRef,
        pointer: LLVMValueRef,
        write: bool,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let fallback = if write {
            self.fallback_buffer_write
        } else {
            self.fallback_buffer_read
        };
        if fallback.is_null() {
            return Err(MirCodegenError::invalid(
                "buffer pointer used without fallback storage",
            ));
        }
        let bound = self.buffer_pointer_is_bound(builder, pointer)?;
        Ok(LLVMBuildSelect(
            builder,
            bound,
            pointer,
            fallback,
            c_name(if write {
                "buffer_write_or_discard"
            } else {
                "buffer_read_or_zero"
            })?
            .as_ptr(),
        ))
    }

    unsafe fn snapshot_direct_buffer_field(
        &mut self,
        buffer: onda_mir::BufferId,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let field = usize::try_from(descriptor_field)
            .map_err(|_| MirCodegenError::invalid("buffer descriptor field does not fit usize"))?;
        let cached = self
            .direct_buffer_fields
            .get(buffer.index())
            .and_then(|fields| fields.get(field))
            .ok_or_else(|| MirCodegenError::invalid("direct buffer descriptor is out of range"))?;
        if let Some(value) = *cached {
            return Ok(value);
        }

        let entry = LLVMGetEntryBasicBlock(self.declaration.value);
        let terminator = LLVMGetBasicBlockTerminator(entry);
        if terminator.is_null() {
            LLVMPositionBuilderAtEnd(self.prologue_builder, entry);
        } else {
            LLVMPositionBuilderBefore(self.prologue_builder, terminator);
        }
        let index = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            u64::from(buffer.raw()),
            0,
        );
        let value =
            self.load_external_buffer_metadata_at(self.prologue_builder, index, descriptor_field)?;
        self.direct_buffer_fields[buffer.index()][field] = Some(value);
        Ok(value)
    }

    unsafe fn buffer_table_component(
        &self,
        field: u32,
    ) -> Result<(LLVMTypeRef, u32, &'static str), MirCodegenError> {
        match field {
            0 => Ok((self.module.ptr_ty, 7, "buffer_ptr")),
            1 => Ok((self.module.ptr_ty, 8, "buffer_write_ptr")),
            2 => Ok((
                LLVMInt32TypeInContext(self.module.context),
                9,
                "buffer_frames",
            )),
            3 => Ok((
                LLVMInt32TypeInContext(self.module.context),
                10,
                "buffer_channels",
            )),
            4 => Ok((
                LLVMFloatTypeInContext(self.module.context),
                11,
                "buffer_sample_rate",
            )),
            // Boundness is derived from the raw read-pointer table. Keeping the
            // table in spans preserves it after selecting and forwarding entries.
            5 => Ok((self.module.ptr_ty, 7, "buffer_bound_ptr")),
            _ => Err(MirCodegenError::invalid("invalid buffer descriptor field")),
        }
    }

    unsafe fn offset_buffer_table(
        &self,
        base: LLVMValueRef,
        index: LLVMValueRef,
        field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (element, _, name) = self.buffer_table_component(field)?;
        Ok(LLVMBuildGEP2(
            self.builder,
            element,
            base,
            [index].as_mut_ptr(),
            1,
            c_name(name)?.as_ptr(),
        ))
    }

    unsafe fn build_buffer_span(
        &mut self,
        span: onda_mir::BufferSpanRef,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let Type::BufferSpan {
            len: expected_len, ..
        } = self.module.program.types[expected.index()]
        else {
            return Err(MirCodegenError::invalid(
                "buffer span argument targets a non-span parameter",
            ));
        };
        let (start, len, source) = match span {
            onda_mir::BufferSpanRef::Interface { first, len } => (first.raw(), len, None),
            onda_mir::BufferSpanRef::Parameter { span, start, len } => (start, len, Some(span)),
        };
        if len != expected_len {
            return Err(MirCodegenError::invalid(
                "buffer span argument length does not match parameter type",
            ));
        }
        let index = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            u64::from(start),
            0,
        );
        let source_span = if let Some(source) = source {
            let place = self
                .parameters
                .get(source.index())
                .copied()
                .ok_or_else(|| MirCodegenError::invalid("buffer span parameter is out of range"))?;
            let Type::BufferSpan {
                len: source_len, ..
            } = self.module.program.types[place.ty.index()]
            else {
                return Err(MirCodegenError::invalid(
                    "buffer span source is not span-typed",
                ));
            };
            if start.checked_add(len).is_none_or(|end| end > source_len) {
                return Err(MirCodegenError::invalid(
                    "buffer span source window is out of range",
                ));
            }
            Some(self.load(place))
        } else {
            None
        };
        let mut value = LLVMGetUndef(self.module.types.get(expected));
        for field in 0..6 {
            let base = if let Some(source) = source_span {
                LLVMBuildExtractValue(
                    self.builder,
                    source,
                    field,
                    c_name("buffer_span_table")?.as_ptr(),
                )
            } else {
                let (_, context_field, name) = self.buffer_table_component(field)?;
                load_context_field(
                    self.module,
                    self.builder,
                    self.runtime_context,
                    context_field,
                    name,
                )?
            };
            let base = self.offset_buffer_table(base, index, field)?;
            value = LLVMBuildInsertValue(
                self.builder,
                value,
                base,
                field,
                c_name("buffer_span")?.as_ptr(),
            );
        }
        Ok(value)
    }

    unsafe fn buffer_param_descriptor(
        &mut self,
        reference: onda_mir::BufferParamRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        match reference {
            onda_mir::BufferParamRef::Direct(parameter) => {
                let place = self
                    .parameters
                    .get(parameter.index())
                    .copied()
                    .ok_or_else(|| MirCodegenError::invalid("buffer parameter is out of range"))?;
                Ok(self.load(place))
            }
            onda_mir::BufferParamRef::ArrayElement {
                span,
                selector,
                bounds,
            } => {
                let place = self.parameters.get(span.index()).copied().ok_or_else(|| {
                    MirCodegenError::invalid("buffer span parameter is out of range")
                })?;
                let Type::BufferSpan { len, .. } = self.module.program.types[place.ty.index()]
                else {
                    return Err(MirCodegenError::invalid(
                        "buffer collection reference uses a non-span parameter",
                    ));
                };
                let index = self.lower_fixed_index(selector, len as usize, bounds)?;
                let span = self.load(place);
                let mut fields = [
                    self.module.ptr_ty,
                    self.module.ptr_ty,
                    LLVMInt32TypeInContext(self.module.context),
                    LLVMInt32TypeInContext(self.module.context),
                    LLVMFloatTypeInContext(self.module.context),
                    LLVMInt1TypeInContext(self.module.context),
                ];
                let descriptor_ty = LLVMStructTypeInContext(
                    self.module.context,
                    fields.as_mut_ptr(),
                    fields.len() as u32,
                    0,
                );
                let mut descriptor = LLVMGetUndef(descriptor_ty);
                for field in 0..6 {
                    let table = LLVMBuildExtractValue(
                        self.builder,
                        span,
                        field,
                        c_name("buffer_span_table")?.as_ptr(),
                    );
                    let entry = self.offset_buffer_table(table, index, field)?;
                    let (element, _, name) = self.buffer_table_component(field)?;
                    let component =
                        LLVMBuildLoad2(self.builder, element, entry, c_name(name)?.as_ptr());
                    self.mark_external_buffer_descriptor_access(component);
                    let component = if field <= 1 {
                        self.resolve_buffer_pointer(self.builder, component, field == 1)?
                    } else if field == 5 {
                        self.buffer_pointer_is_bound(self.builder, component)?
                    } else {
                        component
                    };
                    descriptor = LLVMBuildInsertValue(
                        self.builder,
                        descriptor,
                        component,
                        field,
                        c_name("buffer_descriptor")?.as_ptr(),
                    );
                }
                Ok(descriptor)
            }
        }
    }

    unsafe fn buffer_param_place(
        &mut self,
        parameter: onda_mir::BufferParamRef,
        ty: onda_mir::TypeId,
    ) -> Result<PlaceRef, MirCodegenError> {
        if matches!(parameter, onda_mir::BufferParamRef::Direct(_)) {
            return self
                .parameters
                .get(parameter.index())
                .copied()
                .ok_or_else(|| MirCodegenError::invalid("buffer parameter is out of range"));
        }
        let descriptor = self.buffer_param_descriptor(parameter)?;
        let ptr = LLVMBuildAlloca(
            self.builder,
            self.module.types.get(ty),
            c_name("buffer_argument")?.as_ptr(),
        );
        LLVMBuildStore(self.builder, descriptor, ptr);
        Ok(PlaceRef {
            ptr,
            ty,
            alignment: self.module.layouts.type_alignments[ty.index()],
        })
    }

    unsafe fn lower_buffer_param_metadata(
        &mut self,
        parameter: onda_mir::BufferParamRef,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        if descriptor_field == 3 {
            if let Some(channels) =
                self.constant_buffer_channels(self.buffer_param_channel_kind(parameter)?)
            {
                return Ok(channels);
            }
        }
        let descriptor = self.buffer_param_descriptor(parameter)?;
        Ok(LLVMBuildExtractValue(
            self.builder,
            descriptor,
            descriptor_field,
            c_name("buffer_metadata")?.as_ptr(),
        ))
    }

    fn buffer_param_channel_kind(
        &self,
        parameter: onda_mir::BufferParamRef,
    ) -> Result<onda_mir::BufferChannels, MirCodegenError> {
        let ty = self.function.params[parameter.index()].ty;
        match self.module.program.types[ty.index()] {
            Type::Buffer { channels, .. } | Type::BufferSpan { channels, .. } => Ok(channels),
            _ => Err(MirCodegenError::invalid(
                "buffer operation uses a non-buffer parameter",
            )),
        }
    }

    unsafe fn constant_buffer_channels(
        &self,
        channels: onda_mir::BufferChannels,
    ) -> Option<LLVMValueRef> {
        match channels {
            onda_mir::BufferChannels::Mono => Some(LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                1,
                0,
            )),
            onda_mir::BufferChannels::Static(channels) => Some(LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                channels as u64,
                0,
            )),
            onda_mir::BufferChannels::Dynamic => None,
        }
    }

    unsafe fn external_buffer_parts(
        &mut self,
        buffer: onda_mir::BufferRef,
    ) -> Result<BufferParts, MirCodegenError> {
        match buffer {
            onda_mir::BufferRef::Direct(buffer_id) => {
                let descriptor = &self.module.program.interface.buffers[buffer_id.index()];
                let element = descriptor.element;
                let declared_channels = descriptor.channels;
                let read_ptr = self.snapshot_direct_buffer_field(buffer_id, 0)?;
                let write_ptr = self.snapshot_direct_buffer_field(buffer_id, 1)?;
                let frames = self.snapshot_direct_buffer_field(buffer_id, 2)?;
                let channels = match declared_channels {
                    onda_mir::BufferChannels::Mono => {
                        LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 1, 0)
                    }
                    onda_mir::BufferChannels::Static(channels) => LLVMConstInt(
                        LLVMInt32TypeInContext(self.module.context),
                        channels as u64,
                        0,
                    ),
                    onda_mir::BufferChannels::Dynamic => {
                        self.snapshot_direct_buffer_field(buffer_id, 3)?
                    }
                };
                Ok(BufferParts {
                    read_ptr,
                    write_ptr,
                    frames,
                    channels,
                    element,
                })
            }
            onda_mir::BufferRef::ArrayElement { .. } => {
                let runtime_index = self.lower_buffer_ref_index(buffer)?;
                self.external_buffer_parts_at(buffer, runtime_index)
            }
        }
    }

    unsafe fn external_buffer_parts_at(
        &mut self,
        buffer: onda_mir::BufferRef,
        runtime_index: LLVMValueRef,
    ) -> Result<BufferParts, MirCodegenError> {
        let descriptor = &self.module.program.interface.buffers[buffer.index()];
        let read_ptr = self.lower_external_buffer_metadata_at(runtime_index, 0)?;
        let write_ptr = self.lower_external_buffer_metadata_at(runtime_index, 1)?;
        let frames = self.lower_external_buffer_metadata_at(runtime_index, 2)?;
        let channels = match descriptor.channels {
            onda_mir::BufferChannels::Mono => {
                LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 1, 0)
            }
            onda_mir::BufferChannels::Static(channels) => LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                channels as u64,
                0,
            ),
            onda_mir::BufferChannels::Dynamic => {
                self.lower_external_buffer_metadata_at(runtime_index, 3)?
            }
        };
        Ok(BufferParts {
            read_ptr,
            write_ptr,
            frames,
            channels,
            element: descriptor.element,
        })
    }

    unsafe fn buffer_param_parts(
        &mut self,
        parameter: onda_mir::BufferParamRef,
    ) -> Result<BufferParts, MirCodegenError> {
        let ty = self.function.params[parameter.index()].ty;
        let element = match self.module.program.types[ty.index()] {
            Type::Buffer { element, .. } | Type::BufferSpan { element, .. } => element,
            _ => {
                return Err(MirCodegenError::invalid(
                    "buffer operation uses a non-buffer parameter",
                ));
            }
        };
        let descriptor = self.buffer_param_descriptor(parameter)?;
        let read_ptr =
            LLVMBuildExtractValue(self.builder, descriptor, 0, c_name("buffer_ptr")?.as_ptr());
        let write_ptr = LLVMBuildExtractValue(
            self.builder,
            descriptor,
            1,
            c_name("buffer_write_ptr")?.as_ptr(),
        );
        let frames = LLVMBuildExtractValue(
            self.builder,
            descriptor,
            2,
            c_name("buffer_frames")?.as_ptr(),
        );
        let channels =
            match self.constant_buffer_channels(self.buffer_param_channel_kind(parameter)?) {
                Some(channels) => channels,
                None => LLVMBuildExtractValue(
                    self.builder,
                    descriptor,
                    3,
                    c_name("buffer_channels")?.as_ptr(),
                ),
            };
        Ok(BufferParts {
            read_ptr,
            write_ptr,
            frames,
            channels,
            element,
        })
    }

    unsafe fn buffer_element_ptr(
        &mut self,
        parts: BufferParts,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        write: bool,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let index = self.lower_value(index)?;
        let index = if bounds == onda_mir::BoundsMode::Clamp {
            self.clamp_dynamic_index(index, parts.frames)?
        } else {
            self.apply_dynamic_bounds(index, parts.frames, bounds)?
        };
        let flat = if let Some(channel) = channel {
            let channel = self.lower_value(channel)?;
            let channel = if bounds == onda_mir::BoundsMode::Clamp {
                self.clamp_dynamic_index(channel, parts.channels)?
            } else {
                self.apply_dynamic_bounds(channel, parts.channels, bounds)?
            };
            let frame_offset = LLVMBuildMul(
                self.builder,
                index,
                parts.channels,
                c_name("buffer_frame_offset")?.as_ptr(),
            );
            LLVMBuildAdd(
                self.builder,
                frame_offset,
                channel,
                c_name("buffer_flat_index")?.as_ptr(),
            )
        } else {
            index
        };
        let base = if write {
            parts.write_ptr
        } else {
            parts.read_ptr
        };
        let flat = self.neutralize_fallback_offset(base, flat, write)?;
        Ok(LLVMBuildGEP2(
            self.builder,
            llvm_scalar_type(self.module.context, parts.element),
            base,
            [flat].as_mut_ptr(),
            1,
            c_name("buffer_element")?.as_ptr(),
        ))
    }

    unsafe fn neutralize_fallback_offset(
        &self,
        pointer: LLVMValueRef,
        offset: LLVMValueRef,
        write: bool,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let fallback = if write {
            self.fallback_buffer_write
        } else {
            self.fallback_buffer_read
        };
        let is_fallback = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            pointer,
            fallback,
            c_name("buffer_is_unbound")?.as_ptr(),
        );
        Ok(LLVMBuildSelect(
            self.builder,
            is_fallback,
            LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0),
            offset,
            c_name("buffer_storage_offset")?.as_ptr(),
        ))
    }

    unsafe fn lower_buffer_load(
        &mut self,
        buffer: onda_mir::BufferRef,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let parts = self.external_buffer_parts(buffer)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds, false)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, parts.element),
            ptr,
            c_name("buffer_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, scalar_store_size(parts.element) as u32);
        self.mark_external_buffer_access(load);
        Ok(load)
    }

    unsafe fn lower_buffer_param_load(
        &mut self,
        parameter: onda_mir::BufferParamRef,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let parts = self.buffer_param_parts(parameter)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds, false)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, parts.element),
            ptr,
            c_name("buffer_param_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, scalar_store_size(parts.element) as u32);
        self.mark_external_buffer_access(load);
        Ok(load)
    }

    unsafe fn lower_buffer_store(
        &mut self,
        buffer: onda_mir::BufferRef,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        value: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(), MirCodegenError> {
        let parts = self.external_buffer_parts(buffer)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds, true)?;
        let value = self.lower_value(value)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, scalar_store_size(parts.element) as u32);
        self.mark_external_buffer_access(store);
        Ok(())
    }

    unsafe fn lower_buffer_param_store(
        &mut self,
        parameter: onda_mir::BufferParamRef,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        value: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(), MirCodegenError> {
        let parts = self.buffer_param_parts(parameter)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds, true)?;
        let value = self.lower_value(value)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, scalar_store_size(parts.element) as u32);
        self.mark_external_buffer_access(store);
        Ok(())
    }

    unsafe fn slice_parts(
        &mut self,
        slice: onda_mir::Value,
    ) -> Result<SliceParts, MirCodegenError> {
        let onda_mir::Value::Local(local) = slice else {
            return Err(MirCodegenError::invalid("slice value is not a local"));
        };
        let ty = self.function.locals[local.index()].ty;
        let Type::Slice { element, .. } = self.module.program.types[ty.index()] else {
            return Err(MirCodegenError::invalid(
                "slice operation uses a non-slice value",
            ));
        };
        let descriptor = self.lower_value(slice)?;
        Ok(SliceParts {
            read_ptr: LLVMBuildExtractValue(
                self.builder,
                descriptor,
                0,
                c_name("slice_read_ptr")?.as_ptr(),
            ),
            write_ptr: LLVMBuildExtractValue(
                self.builder,
                descriptor,
                1,
                c_name("slice_write_ptr")?.as_ptr(),
            ),
            len: LLVMBuildExtractValue(self.builder, descriptor, 2, c_name("slice_len")?.as_ptr()),
            stride_bytes: LLVMBuildExtractValue(
                self.builder,
                descriptor,
                3,
                c_name("slice_stride_bytes")?.as_ptr(),
            ),
            element,
        })
    }

    unsafe fn slice_ptr_at_index(
        &self,
        parts: SliceParts,
        index: LLVMValueRef,
        name: &str,
        write: bool,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let byte_offset = LLVMBuildMul(
            self.builder,
            index,
            parts.stride_bytes,
            c_name(&format!("{name}_byte_offset"))?.as_ptr(),
        );
        Ok(LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            if write {
                parts.write_ptr
            } else {
                parts.read_ptr
            },
            [byte_offset].as_mut_ptr(),
            1,
            c_name(name)?.as_ptr(),
        ))
    }

    unsafe fn slice_element_ptr(
        &mut self,
        slice: onda_mir::Value,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        write: bool,
    ) -> Result<(LLVMValueRef, onda_mir::ScalarType), MirCodegenError> {
        let parts = self.slice_parts(slice)?;
        let index = self.lower_value(index)?;
        let index = self.apply_dynamic_bounds(index, parts.len, bounds)?;
        let ptr = self.slice_ptr_at_index(parts, index, "slice_element", write)?;
        Ok((ptr, parts.element))
    }

    unsafe fn lower_make_slice(
        &mut self,
        source: &onda_mir::SliceSource,
        start: onda_mir::Value,
        len: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let Type::Slice { element, .. } = self.module.program.types[expected.index()] else {
            return Err(MirCodegenError::invalid(
                "make-slice destination is not slice-typed",
            ));
        };
        let start = self.lower_value(start)?;
        let element_size = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            scalar_store_size(element),
            0,
        );
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let (read_base_ptr, write_base_ptr, stride_bytes, source_len) = match source {
            onda_mir::SliceSource::Place(place) => {
                let place = self.lower_place(place)?;
                match self.module.program.types[place.ty.index()] {
                    Type::Array { len, .. } => {
                        let zero = LLVMConstInt(i32_ty, 0, 0);
                        let base = LLVMBuildGEP2(
                            self.builder,
                            self.module.types.get(place.ty),
                            place.ptr,
                            [zero, zero].as_mut_ptr(),
                            2,
                            c_name("slice_array_base")?.as_ptr(),
                        );
                        (
                            base,
                            base,
                            element_size,
                            LLVMConstInt(i32_ty, u64::from(len), 0),
                        )
                    }
                    Type::Slice { .. } => {
                        let descriptor = self.load(place);
                        (
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                0,
                                c_name("slice_base")?.as_ptr(),
                            ),
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                1,
                                c_name("slice_write_base")?.as_ptr(),
                            ),
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                3,
                                c_name("slice_source_stride")?.as_ptr(),
                            ),
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                2,
                                c_name("slice_source_len")?.as_ptr(),
                            ),
                        )
                    }
                    _ => {
                        return Err(MirCodegenError::invalid(
                            "make-slice place source is neither array nor slice",
                        ));
                    }
                }
            }
            onda_mir::SliceSource::Buffer { buffer, channel } => {
                let parts = self.external_buffer_parts(*buffer)?;
                return self.make_buffer_slice(parts, *channel, start, len, bounds, expected);
            }
            onda_mir::SliceSource::BufferParam { parameter, channel } => {
                let parts = self.buffer_param_parts(*parameter)?;
                return self.make_buffer_slice(parts, *channel, start, len, bounds, expected);
            }
            onda_mir::SliceSource::ConstData(data) => {
                let descriptor = &self.module.program.const_data[data.index()];
                let array_ty = LLVMArrayType2(
                    llvm_scalar_type(self.module.context, descriptor.element),
                    descriptor.values.len() as u64,
                );
                let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
                let base = LLVMBuildGEP2(
                    self.builder,
                    array_ty,
                    self.module.const_globals[data.index()],
                    [zero, zero].as_mut_ptr(),
                    2,
                    c_name("slice_const_base")?.as_ptr(),
                );
                (
                    base,
                    base,
                    element_size,
                    LLVMConstInt(i32_ty, descriptor.values.len() as u64, 0),
                )
            }
        };
        let len = self.lower_value(len)?;
        let (start, len) = self.normalize_slice_range(start, len, source_len, bounds)?;
        let start_byte_offset = LLVMBuildMul(
            self.builder,
            start,
            stride_bytes,
            c_name("slice_start_byte_offset")?.as_ptr(),
        );
        let read_ptr = LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            read_base_ptr,
            [start_byte_offset].as_mut_ptr(),
            1,
            c_name("slice_start")?.as_ptr(),
        );
        let write_ptr = LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            write_base_ptr,
            [start_byte_offset].as_mut_ptr(),
            1,
            c_name("slice_write_start")?.as_ptr(),
        );
        self.build_slice_descriptor(expected, read_ptr, write_ptr, len, stride_bytes)
    }

    unsafe fn make_buffer_slice(
        &mut self,
        parts: BufferParts,
        channel: Option<onda_mir::Value>,
        start: LLVMValueRef,
        len: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let element_size = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            scalar_store_size(parts.element),
            0,
        );
        let (base_offset, stride_bytes) = if let Some(channel) = channel {
            let channel = self.lower_value(channel)?;
            let channel = self.clamp_dynamic_index(channel, parts.channels)?;
            let channel_offset = LLVMBuildMul(
                self.builder,
                channel,
                element_size,
                c_name("buffer_slice_channel_offset")?.as_ptr(),
            );
            let stride = LLVMBuildMul(
                self.builder,
                parts.channels,
                element_size,
                c_name("buffer_slice_stride")?.as_ptr(),
            );
            (channel_offset, stride)
        } else {
            (
                LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0),
                element_size,
            )
        };
        let len = self.lower_value(len)?;
        let (start, len) = self.normalize_slice_range(start, len, parts.frames, bounds)?;
        let start_offset = LLVMBuildMul(
            self.builder,
            start,
            stride_bytes,
            c_name("buffer_slice_checked_start_offset")?.as_ptr(),
        );
        let offset = LLVMBuildAdd(
            self.builder,
            base_offset,
            start_offset,
            c_name("buffer_slice_checked_offset")?.as_ptr(),
        );
        let read_offset = self.neutralize_fallback_offset(parts.read_ptr, offset, false)?;
        let write_offset = self.neutralize_fallback_offset(parts.write_ptr, offset, true)?;
        let read_ptr = LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            parts.read_ptr,
            [read_offset].as_mut_ptr(),
            1,
            c_name("buffer_slice_checked_start")?.as_ptr(),
        );
        let write_ptr = LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            parts.write_ptr,
            [write_offset].as_mut_ptr(),
            1,
            c_name("buffer_slice_write_start")?.as_ptr(),
        );
        self.build_slice_descriptor(expected, read_ptr, write_ptr, len, stride_bytes)
    }

    unsafe fn normalize_slice_range(
        &mut self,
        start: LLVMValueRef,
        len: LLVMValueRef,
        source_len: LLVMValueRef,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(LLVMValueRef, LLVMValueRef), MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        match bounds {
            onda_mir::BoundsMode::Unchecked => Ok((start, len)),
            onda_mir::BoundsMode::Clamp => {
                let start_below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("slice_start_below")?.as_ptr(),
                );
                let start_low = LLVMBuildSelect(
                    self.builder,
                    start_below,
                    zero,
                    start,
                    c_name("slice_start_low")?.as_ptr(),
                );
                let start_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    start_low,
                    source_len,
                    c_name("slice_start_above")?.as_ptr(),
                );
                let start = LLVMBuildSelect(
                    self.builder,
                    start_above,
                    source_len,
                    start_low,
                    c_name("slice_start_clamped")?.as_ptr(),
                );
                let len_below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    len,
                    zero,
                    c_name("slice_len_below")?.as_ptr(),
                );
                let len_low = LLVMBuildSelect(
                    self.builder,
                    len_below,
                    zero,
                    len,
                    c_name("slice_len_low")?.as_ptr(),
                );
                let remaining = LLVMBuildSub(
                    self.builder,
                    source_len,
                    start,
                    c_name("slice_remaining")?.as_ptr(),
                );
                let len_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    len_low,
                    remaining,
                    c_name("slice_len_above")?.as_ptr(),
                );
                let len = LLVMBuildSelect(
                    self.builder,
                    len_above,
                    remaining,
                    len_low,
                    c_name("slice_len_clamped")?.as_ptr(),
                );
                Ok((start, len))
            }
            onda_mir::BoundsMode::Checked => {
                let start_negative = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("slice_start_negative")?.as_ptr(),
                );
                let len_negative = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    len,
                    zero,
                    c_name("slice_len_negative")?.as_ptr(),
                );
                let start_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    start,
                    source_len,
                    c_name("slice_start_out_of_range")?.as_ptr(),
                );
                let remaining = LLVMBuildSub(
                    self.builder,
                    source_len,
                    start,
                    c_name("slice_checked_remaining")?.as_ptr(),
                );
                let len_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    len,
                    remaining,
                    c_name("slice_len_out_of_range")?.as_ptr(),
                );
                let invalid_start = LLVMBuildOr(
                    self.builder,
                    start_negative,
                    start_above,
                    c_name("slice_invalid_start")?.as_ptr(),
                );
                let invalid_len = LLVMBuildOr(
                    self.builder,
                    len_negative,
                    len_above,
                    c_name("slice_invalid_len")?.as_ptr(),
                );
                let invalid = LLVMBuildOr(
                    self.builder,
                    invalid_start,
                    invalid_len,
                    c_name("slice_invalid_range")?.as_ptr(),
                );
                self.emit_failure_if(invalid, "slice_range_ok")?;
                Ok((start, len))
            }
        }
    }

    unsafe fn build_slice_descriptor(
        &self,
        ty: onda_mir::TypeId,
        read_ptr: LLVMValueRef,
        write_ptr: LLVMValueRef,
        len: LLVMValueRef,
        stride_bytes: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let mut descriptor = LLVMGetUndef(self.module.types.get(ty));
        descriptor = LLVMBuildInsertValue(
            self.builder,
            descriptor,
            read_ptr,
            0,
            c_name("slice_with_ptr")?.as_ptr(),
        );
        descriptor = LLVMBuildInsertValue(
            self.builder,
            descriptor,
            write_ptr,
            1,
            c_name("slice_with_write_ptr")?.as_ptr(),
        );
        descriptor = LLVMBuildInsertValue(
            self.builder,
            descriptor,
            len,
            2,
            c_name("slice_with_len")?.as_ptr(),
        );
        Ok(LLVMBuildInsertValue(
            self.builder,
            descriptor,
            stride_bytes,
            3,
            c_name("slice_with_stride")?.as_ptr(),
        ))
    }

    unsafe fn lower_slice_load(
        &mut self,
        slice: onda_mir::Value,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (ptr, element) = self.slice_element_ptr(slice, index, bounds, false)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, element),
            ptr,
            c_name("slice_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, 1);
        Ok(load)
    }

    unsafe fn lower_slice_len(
        &mut self,
        slice: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        Ok(self.slice_parts(slice)?.len)
    }

    unsafe fn lower_slice_store(
        &mut self,
        slice: onda_mir::Value,
        index: onda_mir::Value,
        value: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(), MirCodegenError> {
        let (ptr, _) = self.slice_element_ptr(slice, index, bounds, true)?;
        let value = self.lower_value(value)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, 1);
        Ok(())
    }

    unsafe fn lower_slice_fill(
        &mut self,
        destination: onda_mir::Value,
        value: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let destination = self.slice_parts(destination)?;
        let value = self.lower_value(value)?;
        let preheader = LLVMGetInsertBlock(self.builder);
        let condition = append_block(
            self.module.context,
            self.declaration.value,
            "slice_fill_condition",
        )?;
        let body = append_block(
            self.module.context,
            self.declaration.value,
            "slice_fill_body",
        )?;
        let exit = append_block(
            self.module.context,
            self.declaration.value,
            "slice_fill_exit",
        )?;
        LLVMBuildBr(self.builder, condition);

        LLVMPositionBuilderAtEnd(self.builder, condition);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let index = LLVMBuildPhi(self.builder, i32_ty, c_name("slice_fill_index")?.as_ptr());
        let zero = LLVMConstInt(i32_ty, 0, 0);
        LLVMAddIncoming(index, [zero].as_mut_ptr(), [preheader].as_mut_ptr(), 1);
        let in_bounds = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSLT,
            index,
            destination.len,
            c_name("slice_fill_in_bounds")?.as_ptr(),
        );
        LLVMBuildCondBr(self.builder, in_bounds, body, exit);

        LLVMPositionBuilderAtEnd(self.builder, body);
        let ptr = self.slice_ptr_at_index(destination, index, "slice_fill_element", true)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, 1);
        let next = LLVMBuildAdd(
            self.builder,
            index,
            LLVMConstInt(i32_ty, 1, 0),
            c_name("slice_fill_next")?.as_ptr(),
        );
        LLVMBuildBr(self.builder, condition);
        LLVMAddIncoming(index, [next].as_mut_ptr(), [body].as_mut_ptr(), 1);
        LLVMPositionBuilderAtEnd(self.builder, exit);
        Ok(())
    }

    unsafe fn lower_slice_copy(
        &mut self,
        destination: onda_mir::Value,
        source: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let destination = self.slice_parts(destination)?;
        let source = self.slice_parts(source)?;
        if destination.element != source.element {
            return Err(MirCodegenError::invalid(
                "slice copy source and destination element types differ",
            ));
        }
        let destination_shorter = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSLT,
            destination.len,
            source.len,
            c_name("slice_copy_destination_shorter")?.as_ptr(),
        );
        let len = LLVMBuildSelect(
            self.builder,
            destination_shorter,
            destination.len,
            source.len,
            c_name("slice_copy_len")?.as_ptr(),
        );
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let empty = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            len,
            zero,
            c_name("slice_copy_empty")?.as_ptr(),
        );
        let nonempty = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_nonempty",
        )?;
        let merge = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_merge",
        )?;
        LLVMBuildCondBr(self.builder, empty, merge, nonempty);
        LLVMPositionBuilderAtEnd(self.builder, nonempty);

        let element_size = scalar_store_size(destination.element);
        let element_size_i32 = LLVMConstInt(i32_ty, element_size, 0);
        let destination_contiguous = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            destination.stride_bytes,
            element_size_i32,
            c_name("slice_copy_destination_contiguous")?.as_ptr(),
        );
        let source_contiguous = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            source.stride_bytes,
            element_size_i32,
            c_name("slice_copy_source_contiguous")?.as_ptr(),
        );
        let contiguous = LLVMBuildAnd(
            self.builder,
            destination_contiguous,
            source_contiguous,
            c_name("slice_copy_contiguous")?.as_ptr(),
        );
        let contiguous_block = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_memmove",
        )?;
        let strided_block = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_strided",
        )?;
        LLVMBuildCondBr(self.builder, contiguous, contiguous_block, strided_block);

        LLVMPositionBuilderAtEnd(self.builder, contiguous_block);
        let i64_ty = LLVMInt64TypeInContext(self.module.context);
        let len_i64 = LLVMBuildZExt(
            self.builder,
            len,
            i64_ty,
            c_name("slice_copy_len_i64")?.as_ptr(),
        );
        let byte_count = LLVMBuildMul(
            self.builder,
            len_i64,
            LLVMConstInt(i64_ty, element_size, 0),
            c_name("slice_copy_bytes")?.as_ptr(),
        );
        LLVMBuildMemMove(
            self.builder,
            destination.write_ptr,
            1,
            source.read_ptr,
            1,
            byte_count,
        );
        LLVMBuildBr(self.builder, merge);

        LLVMPositionBuilderAtEnd(self.builder, strided_block);
        let intptr_ty = LLVMInt64TypeInContext(self.module.context);
        let destination_address = LLVMBuildPtrToInt(
            self.builder,
            destination.write_ptr,
            intptr_ty,
            c_name("slice_copy_destination_address")?.as_ptr(),
        );
        let source_address = LLVMBuildPtrToInt(
            self.builder,
            source.read_ptr,
            intptr_ty,
            c_name("slice_copy_source_address")?.as_ptr(),
        );
        let same_stride = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            destination.stride_bytes,
            source.stride_bytes,
            c_name("slice_copy_same_stride")?.as_ptr(),
        );
        let one = LLVMConstInt(i32_ty, 1, 0);
        let last_index = LLVMBuildSub(
            self.builder,
            len,
            one,
            c_name("slice_copy_last_index")?.as_ptr(),
        );
        let last_index_i64 = LLVMBuildZExt(
            self.builder,
            last_index,
            intptr_ty,
            c_name("slice_copy_last_index_i64")?.as_ptr(),
        );
        let destination_stride_i64 = LLVMBuildZExt(
            self.builder,
            destination.stride_bytes,
            intptr_ty,
            c_name("slice_copy_destination_stride_i64")?.as_ptr(),
        );
        let source_stride_i64 = LLVMBuildZExt(
            self.builder,
            source.stride_bytes,
            intptr_ty,
            c_name("slice_copy_source_stride_i64")?.as_ptr(),
        );
        let destination_last_offset = LLVMBuildMul(
            self.builder,
            last_index_i64,
            destination_stride_i64,
            c_name("slice_copy_destination_last_offset")?.as_ptr(),
        );
        let source_last_offset = LLVMBuildMul(
            self.builder,
            last_index_i64,
            source_stride_i64,
            c_name("slice_copy_source_last_offset")?.as_ptr(),
        );
        let element_size_i64 = LLVMConstInt(intptr_ty, element_size, 0);
        let destination_end = LLVMBuildAdd(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                destination_address,
                destination_last_offset,
                c_name("slice_copy_destination_last")?.as_ptr(),
            ),
            element_size_i64,
            c_name("slice_copy_destination_end")?.as_ptr(),
        );
        let source_end = LLVMBuildAdd(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                source_address,
                source_last_offset,
                c_name("slice_copy_source_last")?.as_ptr(),
            ),
            element_size_i64,
            c_name("slice_copy_source_end")?.as_ptr(),
        );
        let destination_before_source = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            destination_end,
            source_address,
            c_name("slice_copy_destination_before_source")?.as_ptr(),
        );
        let source_before_destination = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            source_end,
            destination_address,
            c_name("slice_copy_source_before_destination")?.as_ptr(),
        );
        let disjoint = LLVMBuildOr(
            self.builder,
            destination_before_source,
            source_before_destination,
            c_name("slice_copy_disjoint")?.as_ptr(),
        );
        let directional_safe = LLVMBuildOr(
            self.builder,
            same_stride,
            disjoint,
            c_name("slice_copy_directional_safe")?.as_ptr(),
        );
        let unsupported_overlap = LLVMBuildNot(
            self.builder,
            directional_safe,
            c_name("slice_copy_unequal_stride_overlap")?.as_ptr(),
        );
        // A general unequal-stride overlap needs temporary storage. Dynamic
        // stack allocation is not acceptable in realtime code, so the
        // deterministic backend contract rejects that rare shape. Equal
        // strides retain memmove directionality; disjoint unequal strides use
        // the normal forward loop.
        self.emit_failure_if(unsupported_overlap, "slice_copy_strided_safe")?;
        let copy_backward = LLVMBuildAnd(
            self.builder,
            LLVMBuildNot(
                self.builder,
                disjoint,
                c_name("slice_copy_overlaps")?.as_ptr(),
            ),
            LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntUGT,
                destination_address,
                source_address,
                c_name("slice_copy_destination_after_source")?.as_ptr(),
            ),
            c_name("slice_copy_backward")?.as_ptr(),
        );
        let backward = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_backward",
        )?;
        let forward = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_forward",
        )?;
        LLVMBuildCondBr(self.builder, copy_backward, backward, forward);

        LLVMPositionBuilderAtEnd(self.builder, backward);
        self.emit_slice_copy_loop(destination, source, len, true)?;
        LLVMBuildBr(self.builder, merge);

        LLVMPositionBuilderAtEnd(self.builder, forward);
        self.emit_slice_copy_loop(destination, source, len, false)?;
        LLVMBuildBr(self.builder, merge);

        LLVMPositionBuilderAtEnd(self.builder, merge);
        Ok(())
    }

    unsafe fn emit_slice_copy_loop(
        &mut self,
        destination: SliceParts,
        source: SliceParts,
        len: LLVMValueRef,
        backward: bool,
    ) -> Result<(), MirCodegenError> {
        let preheader = LLVMGetInsertBlock(self.builder);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let one = LLVMConstInt(i32_ty, 1, 0);
        // Compute the initial value in the preheader so the PHI remains the
        // first instruction in its block, as required by LLVM IR.
        let initial = if backward {
            LLVMBuildSub(
                self.builder,
                len,
                one,
                c_name("slice_copy_backward_start")?.as_ptr(),
            )
        } else {
            zero
        };
        let condition = append_block(
            self.module.context,
            self.declaration.value,
            if backward {
                "slice_copy_backward_condition"
            } else {
                "slice_copy_forward_condition"
            },
        )?;
        let body = append_block(
            self.module.context,
            self.declaration.value,
            if backward {
                "slice_copy_backward_body"
            } else {
                "slice_copy_forward_body"
            },
        )?;
        let exit = append_block(
            self.module.context,
            self.declaration.value,
            if backward {
                "slice_copy_backward_exit"
            } else {
                "slice_copy_forward_exit"
            },
        )?;
        LLVMBuildBr(self.builder, condition);

        LLVMPositionBuilderAtEnd(self.builder, condition);
        let index = LLVMBuildPhi(
            self.builder,
            i32_ty,
            c_name(if backward {
                "slice_copy_backward_index"
            } else {
                "slice_copy_forward_index"
            })?
            .as_ptr(),
        );
        LLVMAddIncoming(index, [initial].as_mut_ptr(), [preheader].as_mut_ptr(), 1);
        let in_bounds = if backward {
            LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSGE,
                index,
                zero,
                c_name("slice_copy_backward_in_bounds")?.as_ptr(),
            )
        } else {
            LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSLT,
                index,
                len,
                c_name("slice_copy_forward_in_bounds")?.as_ptr(),
            )
        };
        LLVMBuildCondBr(self.builder, in_bounds, body, exit);

        LLVMPositionBuilderAtEnd(self.builder, body);
        self.copy_slice_element(destination, source, index)?;
        let next = if backward {
            LLVMBuildSub(
                self.builder,
                index,
                one,
                c_name("slice_copy_backward_next")?.as_ptr(),
            )
        } else {
            LLVMBuildAdd(
                self.builder,
                index,
                one,
                c_name("slice_copy_forward_next")?.as_ptr(),
            )
        };
        LLVMBuildBr(self.builder, condition);
        let body_block = LLVMGetInsertBlock(self.builder);
        LLVMAddIncoming(index, [next].as_mut_ptr(), [body_block].as_mut_ptr(), 1);
        LLVMPositionBuilderAtEnd(self.builder, exit);
        Ok(())
    }

    unsafe fn copy_slice_element(
        &self,
        destination: SliceParts,
        source: SliceParts,
        index: LLVMValueRef,
    ) -> Result<(), MirCodegenError> {
        let source_ptr = self.slice_ptr_at_index(source, index, "slice_copy_source", false)?;
        let destination_ptr =
            self.slice_ptr_at_index(destination, index, "slice_copy_destination", true)?;
        let value = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, source.element),
            source_ptr,
            c_name("slice_copy_value")?.as_ptr(),
        );
        LLVMSetAlignment(value, 1);
        let store = LLVMBuildStore(self.builder, value, destination_ptr);
        LLVMSetAlignment(store, 1);
        Ok(())
    }

    unsafe fn apply_dynamic_bounds(
        &mut self,
        index: LLVMValueRef,
        len: LLVMValueRef,
        mode: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        match mode {
            onda_mir::BoundsMode::Unchecked => Ok(index),
            onda_mir::BoundsMode::Clamp => {
                let positive = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    len,
                    zero,
                    c_name("dynamic_len_positive")?.as_ptr(),
                );
                let empty = LLVMBuildNot(
                    self.builder,
                    positive,
                    c_name("dynamic_len_empty")?.as_ptr(),
                );
                // Clamp selects the nearest existing element. An empty
                // runtime sequence has no such element, so it must fail rather
                // than fabricate an access to index zero.
                self.emit_failure_if(empty, "dynamic_clamp_nonempty")?;
                self.clamp_dynamic_index(index, len)
            }
            onda_mir::BoundsMode::Checked => {
                let in_bounds = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntULT,
                    index,
                    len,
                    c_name("dynamic_index_in_bounds")?.as_ptr(),
                );
                let invalid = LLVMBuildNot(
                    self.builder,
                    in_bounds,
                    c_name("dynamic_index_out_of_bounds")?.as_ptr(),
                );
                self.emit_failure_if(invalid, "dynamic_bounds_ok")?;
                Ok(index)
            }
        }
    }

    unsafe fn clamp_dynamic_index(
        &self,
        index: LLVMValueRef,
        len: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let below = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSLT,
            index,
            zero,
            c_name("dynamic_index_below")?.as_ptr(),
        );
        let low = LLVMBuildSelect(
            self.builder,
            below,
            zero,
            index,
            c_name("dynamic_index_low")?.as_ptr(),
        );
        let one = LLVMConstInt(i32_ty, 1, 0);
        let max = LLVMBuildSub(
            self.builder,
            len,
            one,
            c_name("dynamic_index_max")?.as_ptr(),
        );
        let above = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSGT,
            low,
            max,
            c_name("dynamic_index_above")?.as_ptr(),
        );
        Ok(LLVMBuildSelect(
            self.builder,
            above,
            max,
            low,
            c_name("dynamic_index_clamped")?.as_ptr(),
        ))
    }

    unsafe fn lower_const_data_load(
        &mut self,
        data: onda_mir::ConstDataId,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let descriptor = &self.module.program.const_data[data.index()];
        let index = self.lower_fixed_index(index, descriptor.values.len(), bounds)?;
        let element_ty = llvm_scalar_type(self.module.context, descriptor.element);
        let array_ty = LLVMArrayType2(element_ty, descriptor.values.len() as u64);
        let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
        let mut indices = [zero, index];
        let ptr = LLVMBuildInBoundsGEP2(
            self.builder,
            array_ty,
            self.module.const_globals[data.index()],
            indices.as_mut_ptr(),
            2,
            c_name("const_data_ptr")?.as_ptr(),
        );
        Ok(LLVMBuildLoad2(
            self.builder,
            element_ty,
            ptr,
            c_name("const_data")?.as_ptr(),
        ))
    }

    unsafe fn lower_place(&mut self, place: &Place) -> Result<PlaceRef, MirCodegenError> {
        let mut lowered = match place.base {
            onda_mir::PlaceBase::Local(local) => self.locals[local.index()],
            onda_mir::PlaceBase::Parameter(parameter) => self.parameters[parameter.index()],
            onda_mir::PlaceBase::State(state) => {
                let state_ptr = load_context_field(
                    self.module,
                    self.builder,
                    self.runtime_context,
                    6,
                    "state_ptr",
                )?;
                PlaceRef {
                    ptr: byte_offset_ptr(
                        self.module.context,
                        self.builder,
                        state_ptr,
                        self.module.layouts.state.offsets[state.index()],
                        "state_slot",
                    )?,
                    ty: self.module.program.state[state.index()].ty,
                    alignment: self.module.layouts.type_alignments
                        [self.module.program.state[state.index()].ty.index()],
                }
            }
            onda_mir::PlaceBase::Param(param) => {
                let params_ptr = load_context_field(
                    self.module,
                    self.builder,
                    self.runtime_context,
                    5,
                    "params_ptr",
                )?;
                let offset = self.module.layouts.params.offsets[param.index()];
                PlaceRef {
                    ptr: byte_offset_ptr(
                        self.module.context,
                        self.builder,
                        params_ptr,
                        offset,
                        "param_slot",
                    )?,
                    ty: self.module.program.interface.params[param.index()].ty,
                    // Parameter storage is a packed byte ABI. Even when an
                    // individual offset is naturally aligned, the host-owned
                    // base pointer itself has no alignment guarantee beyond 1.
                    alignment: 1,
                }
            }
            onda_mir::PlaceBase::EventParam(parameter) => {
                let FunctionKind::Event(_) = self.function.kind else {
                    return Err(MirCodegenError::invalid(
                        "event parameter place appears outside an event handler",
                    ));
                };
                self.event_parameters[parameter.index()]
            }
        };
        for projection in &place.projections {
            match projection {
                Projection::Index { index, bounds } => {
                    lowered = self.project_array(lowered, *index, *bounds)?;
                }
                Projection::Field(_) => {
                    return Err(MirCodegenError::unsupported(
                        "struct field projections are not in the native MIR slice",
                    ));
                }
            }
        }
        Ok(lowered)
    }

    unsafe fn project_array(
        &mut self,
        place: PlaceRef,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<PlaceRef, MirCodegenError> {
        let Type::Array { element, len } = self.module.program.types[place.ty.index()] else {
            return Err(MirCodegenError::invalid(
                "index projection base is not an array",
            ));
        };
        let index = self.lower_fixed_index(index, len as usize, bounds)?;
        let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
        let mut indices = [zero, index];
        let ptr = LLVMBuildGEP2(
            self.builder,
            self.module.types.get(place.ty),
            place.ptr,
            indices.as_mut_ptr(),
            2,
            c_name("array_element")?.as_ptr(),
        );
        Ok(PlaceRef {
            ptr,
            ty: element,
            alignment: place
                .alignment
                .min(self.module.layouts.type_alignments[element.index()]),
        })
    }

    unsafe fn lower_fixed_index(
        &mut self,
        index: onda_mir::Value,
        len: usize,
        mode: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        if mode == onda_mir::BoundsMode::Clamp {
            if let onda_mir::Value::Local(local) = index {
                if let Some(fused) = self.fused_clamped_indices[local.index()] {
                    if let Some(index) = self.lower_fused_clamped_index(fused, len)? {
                        return Ok(index);
                    }
                }
            }
        }
        let index = self.lower_value(index)?;
        self.apply_bounds(index, len, mode)
    }

    unsafe fn lower_fused_clamped_index(
        &mut self,
        fused: FusedClampedIndex,
        len: usize,
    ) -> Result<Option<LLVMValueRef>, MirCodegenError> {
        let max = len
            .checked_sub(1)
            .ok_or_else(|| MirCodegenError::invalid("fixed array has no clampable element"))?;
        let max_i32 = i32::try_from(max).map_err(|_| {
            MirCodegenError::invalid("fixed array index exceeds the i32 MIR domain")
        })?;
        // Every integer through 2^24 is exactly representable in f32. Beyond
        // that boundary, retaining the generic saturating-cast-plus-integer-
        // clamp path avoids rounding the upper bound to an invalid i32 value.
        if fused.scalar == onda_mir::ScalarType::F32 && max_i32 > (1_i32 << 24) {
            return Ok(None);
        }

        let value = self.lower_value(fused.source)?;
        let scalar_ty = llvm_scalar_type(self.module.context, fused.scalar);
        let suffix = if fused.scalar == onda_mir::ScalarType::F64 {
            "f64"
        } else {
            "f32"
        };
        let zero = LLVMConstReal(scalar_ty, 0.0);
        // Every i32 is exactly representable in f64, and the f32 case above
        // explicitly stays within that format's consecutive-integer range.
        let max_float = LLVMConstReal(scalar_ty, f64::from(max_i32));
        // The numeric min/max intrinsics choose the numeric operand when the
        // other is NaN. Consequently this maps every NaN and -infinity to
        // zero, +infinity to the last element, and leaves a finite in-range
        // value ready for poison-free truncation toward zero.
        let maxnum = self.lower_binary_float_intrinsic(
            &format!("llvm.maxnum.{suffix}"),
            scalar_ty,
            value,
            zero,
            "index_nonnegative",
        )?;
        let clamped = self.lower_binary_float_intrinsic(
            &format!("llvm.minnum.{suffix}"),
            scalar_ty,
            maxnum,
            max_float,
            "index_float_clamped",
        )?;
        Ok(Some(LLVMBuildFPToSI(
            self.builder,
            clamped,
            LLVMInt32TypeInContext(self.module.context),
            c_name("index_cast")?.as_ptr(),
        )))
    }

    unsafe fn lower_binary_float_intrinsic(
        &self,
        name: &str,
        scalar_ty: LLVMTypeRef,
        lhs: LLVMValueRef,
        rhs: LLVMValueRef,
        result_name: &str,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let mut parameter_types = [scalar_ty, scalar_ty];
        let fn_ty = LLVMFunctionType(scalar_ty, parameter_types.as_mut_ptr(), 2, 0);
        let function = ensure_named_function(self.module.module, name, fn_ty)?;
        let call = LLVMBuildCall2(
            self.builder,
            fn_ty,
            function,
            [lhs, rhs].as_mut_ptr(),
            2,
            c_name(result_name)?.as_ptr(),
        );
        Ok(call)
    }

    unsafe fn apply_bounds(
        &mut self,
        index: LLVMValueRef,
        len: usize,
        mode: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let max = LLVMConstInt(i32_ty, (len - 1) as u64, 0);
        match mode {
            onda_mir::BoundsMode::Unchecked => Ok(index),
            onda_mir::BoundsMode::Clamp => {
                let below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    index,
                    zero,
                    c_name("index_below")?.as_ptr(),
                );
                let low = LLVMBuildSelect(
                    self.builder,
                    below,
                    zero,
                    index,
                    c_name("index_low")?.as_ptr(),
                );
                let above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    low,
                    max,
                    c_name("index_above")?.as_ptr(),
                );
                Ok(LLVMBuildSelect(
                    self.builder,
                    above,
                    max,
                    low,
                    c_name("index_clamped")?.as_ptr(),
                ))
            }
            onda_mir::BoundsMode::Checked => {
                let in_bounds = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntULT,
                    index,
                    LLVMConstInt(i32_ty, len as u64, 0),
                    c_name("index_in_bounds")?.as_ptr(),
                );
                let invalid = LLVMBuildNot(
                    self.builder,
                    in_bounds,
                    c_name("index_out_of_bounds")?.as_ptr(),
                );
                self.emit_failure_if(invalid, "bounds_ok")?;
                Ok(index)
            }
        }
    }

    fn scalar_type_of_value(
        &self,
        value: onda_mir::Value,
    ) -> Result<onda_mir::ScalarType, MirCodegenError> {
        match value {
            onda_mir::Value::Constant(value) => Ok(value.ty()),
            onda_mir::Value::Local(local) => {
                let ty = self.function.locals[local.index()].ty;
                match self.module.program.types[ty.index()] {
                    Type::Scalar(scalar) => Ok(scalar),
                    _ => Err(MirCodegenError::invalid(format!(
                        "local {} is not scalar",
                        local.raw()
                    ))),
                }
            }
        }
    }

    unsafe fn load(&self, place: PlaceRef) -> LLVMValueRef {
        let load = LLVMBuildLoad2(
            self.builder,
            self.module.types.get(place.ty),
            place.ptr,
            c"load".as_ptr(),
        );
        LLVMSetAlignment(load, place.alignment as u32);
        load
    }

    fn direct_place_integer_range(&self, place: &Place) -> Option<onda_mir::ValueRange> {
        if !place.projections.is_empty() {
            return None;
        }
        match place.base {
            onda_mir::PlaceBase::Local(local) => {
                self.ranges.local(local).map(analyzed_integer_value_range)
            }
            onda_mir::PlaceBase::Parameter(parameter) => self
                .ranges
                .parameter(parameter)
                .map(analyzed_integer_value_range),
            onda_mir::PlaceBase::State(state) => self.module.program.state[state.index()]
                .integer_range
                .map(invariant_value_range),
            // Interface parameter storage contains raw host values. The generated entry-point
            // normalization moves ranged parameters into compiler-owned locals before use.
            onda_mir::PlaceBase::Param(_) => None,
            onda_mir::PlaceBase::EventParam(_) => None,
        }
    }

    unsafe fn mark_integer_range(&self, instruction: LLVMValueRef, range: onda_mir::ValueRange) {
        let Some((scalar, lower, upper)) = llvm_integer_range_encoding(range) else {
            return;
        };
        let ty = llvm_scalar_type(self.module.context, scalar);
        let mut operands = [
            LLVMValueAsMetadata(LLVMConstInt(ty, lower, 0)),
            LLVMValueAsMetadata(LLVMConstInt(ty, upper, 0)),
        ];
        let node = LLVMMDNodeInContext2(self.module.context, operands.as_mut_ptr(), operands.len());
        LLVMSetMetadata(
            instruction,
            self.module.range_metadata_kind,
            LLVMMetadataAsValue(self.module.context, node),
        );
    }

    unsafe fn store(&self, place: PlaceRef, value: LLVMValueRef) {
        let store = LLVMBuildStore(self.builder, value, place.ptr);
        LLVMSetAlignment(store, place.alignment as u32);
    }

    unsafe fn set_fast_math(&self, instruction: LLVMValueRef) {
        let flags = if self.module.fast_math {
            LLVMFastMathAll
        } else {
            LLVMFastMathNone
        };
        if flags != LLVMFastMathNone {
            LLVMSetFastMathFlags(instruction, flags);
        }
    }

    unsafe fn mark_audio_output_access(&self, instruction: LLVMValueRef) {
        let scopes = self.module.host_alias_scopes;
        LLVMSetMetadata(instruction, scopes.alias_scope_kind, scopes.audio_outputs);
    }

    unsafe fn mark_external_buffer_access(&self, instruction: LLVMValueRef) {
        let scopes = self.module.host_alias_scopes;
        LLVMSetMetadata(instruction, scopes.noalias_kind, scopes.buffer_descriptors);
    }

    unsafe fn mark_external_buffer_descriptor_access(&self, instruction: LLVMValueRef) {
        let scopes = self.module.host_alias_scopes;
        LLVMSetMetadata(
            instruction,
            scopes.alias_scope_kind,
            scopes.buffer_descriptors,
        );
        LLVMSetMetadata(instruction, scopes.noalias_kind, scopes.audio_outputs);
        LLVMSetMetadata(
            instruction,
            scopes.invariant_group_kind,
            scopes.invariant_group,
        );
    }
}
