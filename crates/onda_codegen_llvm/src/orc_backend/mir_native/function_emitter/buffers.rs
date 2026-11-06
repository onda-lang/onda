use super::*;
use onda_mir::BoundsMode;

impl FunctionEmitter<'_, '_> {
    pub(super) unsafe fn lower_buffer_value(
        &mut self,
        source: &CallArgument,
        ty: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        match source {
            CallArgument::Buffer(buffer) => self.build_external_buffer_descriptor(*buffer, ty),
            CallArgument::BufferParam(reference) => self.buffer_param_descriptor(*reference),
            CallArgument::BufferSpan(span) => self.build_buffer_span(*span, ty),
            CallArgument::Value(value) => self.lower_value(*value),
            CallArgument::Place(place) => {
                let place = self.lower_place(place)?;
                Ok(self.load(place))
            }
            _ => Err(MirCodegenError::invalid(
                "make_buffer source is not a resource descriptor",
            )),
        }
    }
    pub(super) unsafe fn buffer_argument_place(
        &self,
        descriptor: LLVMValueRef,
        ty: onda_mir::TypeId,
    ) -> Result<PlaceRef, MirCodegenError> {
        // Allocate once per invocation, even when this call site is inside a
        // loop. Entry-block allocas also remain eligible for LLVM promotion.
        let entry = LLVMGetEntryBasicBlock(self.declaration.value);
        let first = LLVMGetFirstInstruction(entry);
        if first.is_null() {
            LLVMPositionBuilderAtEnd(self.prologue_builder, entry);
        } else {
            LLVMPositionBuilderBefore(self.prologue_builder, first);
        }
        let ptr = LLVMBuildAlloca(
            self.prologue_builder,
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

    pub(super) unsafe fn build_external_buffer_descriptor(
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

    pub(super) unsafe fn lower_buffer_ref_index(
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

    pub(super) unsafe fn lower_external_buffer_metadata(
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

    pub(super) unsafe fn lower_external_buffer_channels(
        &mut self,
        buffer: onda_mir::BufferRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let channels = self.module.program.interface.buffers[buffer.index()].channels;
        if !matches!(channels, onda_mir::BufferChannels::Dynamic)
            && matches!(
                buffer,
                onda_mir::BufferRef::ArrayElement {
                    bounds: BoundsMode::Checked,
                    ..
                }
            )
        {
            self.lower_buffer_ref_index(buffer)?;
        }
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

    pub(super) unsafe fn lower_external_buffer_metadata_at(
        &mut self,
        index: LLVMValueRef,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        self.load_external_buffer_metadata_at(self.builder, index, descriptor_field)
    }

    pub(super) unsafe fn lower_external_buffer_is_bound(
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

    pub(super) unsafe fn lower_external_buffer_is_bound_at(
        &self,
        index: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let raw = self.load_external_buffer_metadata_at(self.builder, index, 5)?;
        self.buffer_pointer_is_bound(self.builder, raw)
    }

    pub(super) unsafe fn buffer_pointer_is_bound(
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

    pub(super) unsafe fn load_external_buffer_metadata_at(
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

    pub(super) unsafe fn resolve_buffer_pointer(
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

    pub(super) unsafe fn snapshot_direct_buffer_field(
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

        let entry = self.context_prologue;
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

    pub(super) unsafe fn buffer_table_component(
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

    pub(super) unsafe fn offset_buffer_table(
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

    pub(super) unsafe fn build_buffer_span(
        &mut self,
        span: onda_mir::BufferSpanRef,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let Some((_, _, _, expected_len, mapped)) =
            self.module.program.types[expected.index()].buffer_collection()
        else {
            return Err(MirCodegenError::invalid(
                "buffer span argument targets a non-collection parameter",
            ));
        };
        let (start, len, source_span, map) = match span {
            onda_mir::BufferSpanRef::Interface { first, len } => (
                LLVMConstInt(
                    LLVMInt32TypeInContext(self.module.context),
                    u64::from(first.raw()),
                    0,
                ),
                len,
                None,
                None,
            ),
            onda_mir::BufferSpanRef::MappedInterface { map, start, len } => (
                LLVMConstInt(
                    LLVMInt32TypeInContext(self.module.context),
                    u64::from(start),
                    0,
                ),
                len,
                None,
                Some(map),
            ),
            onda_mir::BufferSpanRef::Parameter { span, start, len } => {
                let place = self.parameters[span.index()];
                (
                    LLVMConstInt(
                        LLVMInt32TypeInContext(self.module.context),
                        u64::from(start),
                        0,
                    ),
                    len,
                    Some(self.load(place)),
                    None,
                )
            }
            onda_mir::BufferSpanRef::Local {
                collection,
                start,
                len,
                bounds,
            } => {
                let place = self.locals[collection.index()];
                let (_, _, _, source_len, _) = self.module.program.types[place.ty.index()]
                    .buffer_collection()
                    .ok_or_else(|| {
                        MirCodegenError::invalid("local buffer collection has invalid type")
                    })?;
                let start = self.lower_value(start)?;
                let start = self.normalize_fixed_window_start(start, source_len - len, bounds)?;
                (start, len, Some(self.load(place)), None)
            }
        };
        if len != expected_len {
            return Err(MirCodegenError::invalid(
                "buffer collection length differs from parameter contract",
            ));
        }
        let index = start;
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
            let base = if mapped {
                base
            } else {
                self.offset_buffer_table(base, index, field)?
            };
            value = LLVMBuildInsertValue(
                self.builder,
                value,
                base,
                field,
                c_name("buffer_span")?.as_ptr(),
            );
        }
        if mapped {
            let base = if let Some(source) = source_span {
                LLVMBuildExtractValue(self.builder, source, 6, c_name("buffer_map")?.as_ptr())
            } else {
                self.module.buffer_map_globals[map
                    .ok_or_else(|| {
                        MirCodegenError::invalid("mapped collection has no resource map")
                    })?
                    .index()]
            };
            let element = LLVMInt32TypeInContext(self.module.context);
            let map = LLVMBuildGEP2(
                self.builder,
                element,
                base,
                [index].as_mut_ptr(),
                1,
                c_name("buffer_map_window")?.as_ptr(),
            );
            value = LLVMBuildInsertValue(
                self.builder,
                value,
                map,
                6,
                c_name("buffer_map_span")?.as_ptr(),
            );
        }
        Ok(value)
    }

    pub(super) unsafe fn buffer_param_descriptor(
        &mut self,
        reference: onda_mir::BufferParamRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let place = if let Some(parameter) = reference.parameter() {
            self.parameters[parameter.index()]
        } else {
            self.locals[reference
                .local()
                .ok_or_else(|| MirCodegenError::invalid("buffer reference has no source"))?
                .index()]
        };
        let Some((selector, bounds)) = reference.selection() else {
            return Ok(self.load(place));
        };
        let Some((_, _, _, len, mapped)) =
            self.module.program.types[place.ty.index()].buffer_collection()
        else {
            return Err(MirCodegenError::invalid(
                "buffer collection reference uses a non-span parameter",
            ));
        };
        let index = self.lower_fixed_index(selector, len as usize, bounds)?;
        let span = self.load(place);
        let index = if mapped {
            let map = LLVMBuildExtractValue(self.builder, span, 6, c_name("buffer_map")?.as_ptr());
            let element = LLVMInt32TypeInContext(self.module.context);
            let entry = LLVMBuildGEP2(
                self.builder,
                element,
                map,
                [index].as_mut_ptr(),
                1,
                c_name("buffer_map_entry")?.as_ptr(),
            );
            LLVMBuildLoad2(
                self.builder,
                element,
                entry,
                c_name("buffer_map_index")?.as_ptr(),
            )
        } else {
            index
        };
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
            let component = LLVMBuildLoad2(self.builder, element, entry, c_name(name)?.as_ptr());
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

    pub(super) unsafe fn buffer_param_place(
        &mut self,
        parameter: onda_mir::BufferParamRef,
        ty: onda_mir::TypeId,
    ) -> Result<PlaceRef, MirCodegenError> {
        if parameter.selection().is_none() {
            return Ok(if let Some(parameter) = parameter.parameter() {
                self.parameters[parameter.index()]
            } else {
                self.locals[parameter
                    .local()
                    .ok_or_else(|| MirCodegenError::invalid("buffer reference has no source"))?
                    .index()]
            });
        }
        let descriptor = self.buffer_param_descriptor(parameter)?;
        self.buffer_argument_place(descriptor, ty)
    }

    pub(super) unsafe fn lower_buffer_param_metadata(
        &mut self,
        parameter: onda_mir::BufferParamRef,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        if descriptor_field == 3 {
            if let Some(channels) =
                self.constant_buffer_channels(self.buffer_param_channel_kind(parameter)?)
            {
                if let Some((selector, BoundsMode::Checked)) = parameter.selection() {
                    let ty = parameter.ty(self.function).expect("typed buffer reference");
                    let (_, _, _, len, _) = self.module.program.types[ty.index()]
                        .buffer_collection()
                        .expect("selected buffer collection");
                    self.lower_fixed_index(selector, len as usize, BoundsMode::Checked)?;
                }
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

    pub(super) fn buffer_param_channel_kind(
        &self,
        parameter: onda_mir::BufferParamRef,
    ) -> Result<onda_mir::BufferChannels, MirCodegenError> {
        let ty = parameter
            .ty(self.function)
            .ok_or_else(|| MirCodegenError::invalid("buffer reference source is missing"))?;
        match self.module.program.types[ty.index()] {
            Type::Buffer { channels, .. }
            | Type::BufferSpan { channels, .. }
            | Type::BufferMap { channels, .. } => Ok(channels),
            _ => Err(MirCodegenError::invalid(
                "buffer operation uses a non-buffer parameter",
            )),
        }
    }

    pub(super) unsafe fn constant_buffer_channels(
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

    pub(super) unsafe fn external_buffer_parts(
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

    pub(super) unsafe fn external_buffer_parts_at(
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

    pub(super) unsafe fn buffer_param_parts(
        &mut self,
        parameter: onda_mir::BufferParamRef,
    ) -> Result<BufferParts, MirCodegenError> {
        let ty = parameter
            .ty(self.function)
            .ok_or_else(|| MirCodegenError::invalid("buffer reference source is missing"))?;
        let element = match self.module.program.types[ty.index()] {
            Type::Buffer { element, .. }
            | Type::BufferSpan { element, .. }
            | Type::BufferMap { element, .. } => element,
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

    pub(super) unsafe fn buffer_element_ptr(
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

    pub(super) unsafe fn neutralize_fallback_offset(
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

    pub(super) unsafe fn lower_buffer_load(
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

    pub(super) unsafe fn lower_buffer_param_load(
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

    pub(super) unsafe fn lower_buffer_store(
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

    pub(super) unsafe fn lower_buffer_param_store(
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
}
