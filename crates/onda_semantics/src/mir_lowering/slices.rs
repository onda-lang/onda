use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn is_slice_expression(&self, expression: &Expr) -> bool {
        match expression {
            Expr::Slice { .. } => true,
            Expr::Var { name, .. } => {
                matches!(
                    self.bindings.get(name),
                    Some(
                        Binding::Array(_, _, _)
                            | Binding::ArrayParameter(_, _, _)
                            | Binding::Slice(_, _, _)
                            | Binding::EventArrayParameter(_, _, _)
                            | Binding::BufferParameter(_, _)
                    )
                ) || self.const_arrays.contains_key(name)
                    || self.runtime_globals.is_some_and(|globals| {
                        globals.state_arrays.contains_key(name)
                            || globals.buffers.contains_key(name)
                    })
            }
            _ => false,
        }
    }

    pub(super) fn lower_slice_expression(
        &mut self,
        expression: &Expr,
        requested_access: Option<onda_mir::AccessMode>,
        block: &mut MirBlock,
    ) -> Result<LoweredSlice, MirLoweringError> {
        match expression {
            Expr::Slice {
                base,
                selector,
                channel,
                start,
                end,
                ..
            } => self.lower_named_slice(
                base,
                SliceSelection {
                    selector: selector.as_deref(),
                    channel: channel.as_deref(),
                    start: start.as_deref(),
                    end: end.as_deref(),
                },
                requested_access,
                block,
                expression.loc(),
            ),
            Expr::Var { name, .. } => self.lower_named_slice(
                name,
                SliceSelection::default(),
                requested_access,
                block,
                expression.loc(),
            ),
            _ => Err(self.error(
                "primitive slice value requires an array, buffer, or slice expression",
                expression.loc(),
            )),
        }
    }

    pub(super) fn lower_array_value_slice(
        &mut self,
        expression: &Expr,
        element: PrimitiveType,
        access: onda_mir::AccessMode,
        block: &mut MirBlock,
    ) -> Result<Option<LoweredSlice>, MirLoweringError> {
        let (len, values) = match expression {
            Expr::ArrayLiteral { values, .. } => (values.len(), Some(values.as_slice())),
            Expr::ArrayCtor { spec, init, .. } => {
                let ArrayElemType::Primitive(actual_element) = spec.elem else {
                    return Ok(None);
                };
                if actual_element != element {
                    return Err(self.error(
                        format!(
                            "array value expected {} elements, got {}",
                            element.name(),
                            actual_element.name()
                        ),
                        expression.loc(),
                    ));
                }
                let mut diagnostics = Vec::new();
                let len = eval_const_expr_i64_exact(
                    &spec.size,
                    AnalysisOptions {
                        sample_rate: self.config.sample_rate,
                        block_size: self.config.block_size as usize,
                    },
                    "array value length during MIR lowering",
                    &mut diagnostics,
                )
                .ok_or_else(|| {
                    self.error(
                        diagnostics
                            .first()
                            .map(|diagnostic| diagnostic.message.clone())
                            .unwrap_or_else(|| {
                                "array value length was not retained as a compile-time integer"
                                    .to_owned()
                            }),
                        spec.size.loc(),
                    )
                })?;
                let len = usize::try_from(len).map_err(|_| {
                    self.error(
                        "array value length is outside the usize boundary",
                        spec.size.loc(),
                    )
                })?;
                if init.as_ref().is_some_and(|values| values.len() != len) {
                    return Err(self.error(
                        format!(
                            "array value initializer expected {len} elements, got {}",
                            init.as_ref().map(Vec::len).unwrap_or_default()
                        ),
                        expression.loc(),
                    ));
                }
                (len, init.as_deref())
            }
            _ => return Ok(None),
        };
        let len = u32::try_from(len)
            .ok()
            .filter(|len| *len > 0 && *len <= i32::MAX as u32)
            .ok_or_else(|| {
                self.error(
                    "array value length must be between 1 and i32::MAX",
                    expression.loc(),
                )
            })?;
        let local = self.new_array_local(None, element, len);
        for index in 0..len {
            let value = if let Some(values) = values {
                let lowered = self.lower_expr(&values[index as usize], block)?;
                self.coerce(lowered, element, block, values[index as usize].loc())?
                    .value
            } else {
                Value::Constant(zero_scalar(element))
            };
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Local(local),
                        projections: vec![Projection::Index {
                            index: Value::Constant(ScalarValue::I32(index as i32)),
                            bounds: BoundsMode::Unchecked,
                        }],
                    },
                    value: Rvalue::Use(value),
                },
                expression.loc(),
            );
        }
        Ok(Some(self.emit_slice_temp(
            block,
            None,
            element,
            access,
            Rvalue::MakeSlice {
                source: onda_mir::SliceSource::Place(Place::local(local)),
                start: Value::Constant(ScalarValue::I32(0)),
                len: Value::Constant(ScalarValue::I32(len as i32)),
                bounds: BoundsMode::Unchecked,
                access,
            },
            expression.loc(),
        )))
    }

    pub(super) fn lower_named_slice(
        &mut self,
        base: &str,
        selection: SliceSelection<'_>,
        requested_access: Option<onda_mir::AccessMode>,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredSlice, MirLoweringError> {
        let SliceSelection {
            selector,
            channel,
            start,
            end,
        } = selection;
        let (source, length, element, source_access) =
            self.slice_base(base, selector, channel, block, location)?;
        let access = requested_access.unwrap_or(source_access);
        if source_access == onda_mir::AccessMode::ReadOnly
            && access == onda_mir::AccessMode::ReadWrite
        {
            return Err(self.error(
                format!("cannot create a writable slice from read-only source '{base}'"),
                location,
            ));
        }

        let length = self.snapshot(length, block, location);
        let start = self.normalize_slice_bound(start, length.value, false, block, location)?;
        let end = self.normalize_slice_bound(end, length.value, true, block, location)?;
        let difference = self.emit_temp(
            block,
            PrimitiveType::I32,
            Rvalue::Binary {
                op: MirBinaryOp::Subtract,
                lhs: end,
                rhs: start,
            },
            location,
        );
        let slice_len = self.new_local(None, PrimitiveType::I32);
        self.assign_value(block, slice_len, difference.value, location);
        let end_before_start = self.compare_value(block, CompareOp::Less, end, start, location);
        let mut empty = MirBlock::default();
        self.assign_value(
            &mut empty,
            slice_len,
            Value::Constant(ScalarValue::I32(0)),
            location,
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition: end_before_start,
                then_block: empty,
                else_block: MirBlock::default(),
            },
            location,
        );
        Ok(self.emit_slice_temp(
            block,
            None,
            element,
            access,
            Rvalue::MakeSlice {
                source,
                start,
                len: Value::Local(slice_len),
                bounds: BoundsMode::Unchecked,
                access,
            },
            location,
        ))
    }

    pub(super) fn slice_base(
        &mut self,
        base: &str,
        selector: Option<&Expr>,
        channel: Option<&Expr>,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<
        (
            onda_mir::SliceSource,
            LoweredValue,
            PrimitiveType,
            onda_mir::AccessMode,
        ),
        MirLoweringError,
    > {
        if let Some(binding) = self.bindings.get(base).cloned() {
            match binding {
                Binding::ArrayParameter(parameter, element, len) => {
                    return Ok((
                        onda_mir::SliceSource::Place(Place {
                            base: PlaceBase::Parameter(parameter),
                            projections: Vec::new(),
                        }),
                        LoweredValue {
                            value: Value::Constant(ScalarValue::I32(len as i32)),
                            ty: PrimitiveType::I32,
                        },
                        element,
                        onda_mir::AccessMode::ReadWrite,
                    ));
                }
                Binding::BufferParameter(parameter, element) => {
                    if selector.is_some() {
                        return Err(self.error(
                            format!("buffer parameter '{base}' is not a buffer collection"),
                            location,
                        ));
                    }
                    let channel = channel
                        .map(|channel| self.lower_expr(channel, block))
                        .transpose()?
                        .map(|value| self.coerce(value, PrimitiveType::I32, block, location))
                        .transpose()?
                        .map(|value| value.value);
                    let length = self.emit_temp(
                        block,
                        PrimitiveType::I32,
                        Rvalue::BufferParamLen(onda_mir::BufferParamRef::Direct(parameter)),
                        location,
                    );
                    return Ok((
                        onda_mir::SliceSource::BufferParam {
                            parameter: onda_mir::BufferParamRef::Direct(parameter),
                            channel,
                        },
                        length,
                        element,
                        onda_mir::AccessMode::ReadWrite,
                    ));
                }
                Binding::BufferAlias(reference, element) => {
                    if selector.is_some() {
                        return Err(self.error(
                            format!("buffer-reference alias '{base}' is not a buffer collection"),
                            location,
                        ));
                    }
                    let channel = channel
                        .map(|channel| self.lower_expr(channel, block))
                        .transpose()?
                        .map(|value| self.coerce(value, PrimitiveType::I32, block, location))
                        .transpose()?
                        .map(|value| value.value);
                    let reference = self.materialize_buffer_reference(reference, block, location);
                    return match reference {
                        MaterializedBufferReference::Interface(buffer) => {
                            let length = self.emit_temp(
                                block,
                                PrimitiveType::I32,
                                Rvalue::BufferLen(buffer),
                                location,
                            );
                            Ok((
                                onda_mir::SliceSource::Buffer { buffer, channel },
                                length,
                                element,
                                onda_mir::AccessMode::ReadWrite,
                            ))
                        }
                        MaterializedBufferReference::Parameter(parameter) => {
                            let length = self.emit_temp(
                                block,
                                PrimitiveType::I32,
                                Rvalue::BufferParamLen(parameter),
                                location,
                            );
                            Ok((
                                onda_mir::SliceSource::BufferParam { parameter, channel },
                                length,
                                element,
                                onda_mir::AccessMode::ReadWrite,
                            ))
                        }
                    };
                }
                Binding::BufferParameterArray(span, element, _len) => {
                    let selector = selector.ok_or_else(|| {
                        self.error(
                            format!(
                                "buffer collection parameter '{base}' requires a slot selector"
                            ),
                            location,
                        )
                    })?;
                    let selector = self.lower_expr(selector, block)?;
                    let selector = self
                        .coerce(selector, PrimitiveType::I32, block, location)?
                        .value;
                    let channel = channel
                        .map(|channel| self.lower_expr(channel, block))
                        .transpose()?
                        .map(|value| self.coerce(value, PrimitiveType::I32, block, location))
                        .transpose()?
                        .map(|value| value.value);
                    let parameter = onda_mir::BufferParamRef::ArrayElement {
                        span,
                        selector,
                        bounds: BoundsMode::Clamp,
                    };
                    let length = self.emit_temp(
                        block,
                        PrimitiveType::I32,
                        Rvalue::BufferParamLen(parameter),
                        location,
                    );
                    return Ok((
                        onda_mir::SliceSource::BufferParam { parameter, channel },
                        length,
                        element,
                        onda_mir::AccessMode::ReadWrite,
                    ));
                }
                Binding::Array(local, element, len) => {
                    if selector.is_some() || channel.is_some() {
                        return Err(self.error(
                            format!("array '{base}' does not support buffer coordinates"),
                            location,
                        ));
                    }
                    return Ok((
                        onda_mir::SliceSource::Place(Place::local(local)),
                        LoweredValue {
                            value: Value::Constant(ScalarValue::I32(len as i32)),
                            ty: PrimitiveType::I32,
                        },
                        element,
                        onda_mir::AccessMode::ReadWrite,
                    ));
                }
                Binding::Slice(local, element, access) => {
                    let length = self.emit_temp(
                        block,
                        PrimitiveType::I32,
                        Rvalue::SliceLen(Value::Local(local)),
                        location,
                    );
                    return Ok((
                        onda_mir::SliceSource::Place(Place::local(local)),
                        length,
                        element,
                        access,
                    ));
                }
                Binding::EventArrayParameter(parameter, element, len) => {
                    return Ok((
                        onda_mir::SliceSource::Place(Place {
                            base: PlaceBase::EventParam(parameter),
                            projections: Vec::new(),
                        }),
                        LoweredValue {
                            value: Value::Constant(ScalarValue::I32(len as i32)),
                            ty: PrimitiveType::I32,
                        },
                        element,
                        onda_mir::AccessMode::ReadOnly,
                    ));
                }
                _ => {}
            }
        }

        if let Some((state, element, len)) = self
            .runtime_globals
            .and_then(|globals| globals.state_arrays.get(base).copied())
        {
            if selector.is_some() || channel.is_some() {
                return Err(self.error(
                    format!("array '{base}' does not support buffer coordinates"),
                    location,
                ));
            }
            return Ok((
                onda_mir::SliceSource::Place(Place {
                    base: PlaceBase::State(state),
                    projections: Vec::new(),
                }),
                LoweredValue {
                    value: Value::Constant(ScalarValue::I32(len as i32)),
                    ty: PrimitiveType::I32,
                },
                element,
                onda_mir::AccessMode::ReadWrite,
            ));
        }
        if let Some((data, element, len)) = self.const_arrays.get(base).copied() {
            if selector.is_some() || channel.is_some() {
                return Err(self.error(
                    format!("constant array '{base}' does not support buffer coordinates"),
                    location,
                ));
            }
            return Ok((
                onda_mir::SliceSource::ConstData(data),
                LoweredValue {
                    value: Value::Constant(ScalarValue::I32(len as i32)),
                    ty: PrimitiveType::I32,
                },
                element,
                onda_mir::AccessMode::ReadOnly,
            ));
        }
        let buffer_array = self
            .runtime_globals
            .and_then(|globals| globals.buffer_arrays.get(base).copied());
        let direct_buffer = self
            .runtime_globals
            .and_then(|globals| globals.buffers.get(base).copied());
        if let Some((buffer, element)) = direct_buffer {
            if selector.is_some() {
                return Err(self.error(
                    format!("buffer '{base}' is not a buffer collection"),
                    location,
                ));
            }
            let channel = channel
                .map(|channel| self.lower_expr(channel, block))
                .transpose()?
                .map(|value| self.coerce(value, PrimitiveType::I32, block, location))
                .transpose()?
                .map(|value| value.value);
            let length = self.emit_temp(
                block,
                PrimitiveType::I32,
                Rvalue::BufferLen(onda_mir::BufferRef::Direct(buffer)),
                location,
            );
            return Ok((
                onda_mir::SliceSource::Buffer {
                    buffer: onda_mir::BufferRef::Direct(buffer),
                    channel,
                },
                length,
                element,
                onda_mir::AccessMode::ReadWrite,
            ));
        }
        if let Some((first, element, len)) = buffer_array {
            let selector = selector.ok_or_else(|| {
                self.error(
                    format!("buffer collection '{base}' requires a slot selector"),
                    location,
                )
            })?;
            let selector = self.lower_expr(selector, block)?;
            let selector = self
                .coerce(selector, PrimitiveType::I32, block, location)?
                .value;
            let channel = channel
                .map(|channel| self.lower_expr(channel, block))
                .transpose()?
                .map(|value| self.coerce(value, PrimitiveType::I32, block, location))
                .transpose()?
                .map(|value| value.value);
            let buffer = onda_mir::BufferRef::ArrayElement {
                first,
                len,
                selector,
                bounds: BoundsMode::Clamp,
            };
            let length = self.emit_temp(
                block,
                PrimitiveType::I32,
                Rvalue::BufferLen(buffer),
                location,
            );
            return Ok((
                onda_mir::SliceSource::Buffer { buffer, channel },
                length,
                element,
                onda_mir::AccessMode::ReadWrite,
            ));
        }
        Err(self.error(
            format!("slice source '{base}' is outside the current MIR boundary"),
            location,
        ))
    }

    pub(super) fn normalize_slice_bound(
        &mut self,
        expression: Option<&Expr>,
        length: Value,
        default_to_len: bool,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Value, MirLoweringError> {
        let initial = if let Some(expression) = expression {
            let value = self.lower_expr(expression, block)?;
            self.coerce(value, PrimitiveType::I32, block, expression.loc())?
                .value
        } else if default_to_len {
            length
        } else {
            Value::Constant(ScalarValue::I32(0))
        };
        let normalized = self.new_local(None, PrimitiveType::I32);
        self.assign_value(block, normalized, initial, location);

        let negative = self.compare_value(
            block,
            CompareOp::Less,
            Value::Local(normalized),
            Value::Constant(ScalarValue::I32(0)),
            location,
        );
        let mut adjust_negative = MirBlock::default();
        let adjusted = self.emit_temp(
            &mut adjust_negative,
            PrimitiveType::I32,
            Rvalue::Binary {
                op: MirBinaryOp::Add,
                lhs: Value::Local(normalized),
                rhs: length,
            },
            location,
        );
        self.assign_value(&mut adjust_negative, normalized, adjusted.value, location);
        self.push_statement(
            block,
            StatementKind::If {
                condition: negative,
                then_block: adjust_negative,
                else_block: MirBlock::default(),
            },
            location,
        );

        let below_zero = self.compare_value(
            block,
            CompareOp::Less,
            Value::Local(normalized),
            Value::Constant(ScalarValue::I32(0)),
            location,
        );
        let mut clamp_low = MirBlock::default();
        self.assign_value(
            &mut clamp_low,
            normalized,
            Value::Constant(ScalarValue::I32(0)),
            location,
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition: below_zero,
                then_block: clamp_low,
                else_block: MirBlock::default(),
            },
            location,
        );

        let above_length = self.compare_value(
            block,
            CompareOp::Greater,
            Value::Local(normalized),
            length,
            location,
        );
        let mut clamp_high = MirBlock::default();
        self.assign_value(&mut clamp_high, normalized, length, location);
        self.push_statement(
            block,
            StatementKind::If {
                condition: above_length,
                then_block: clamp_high,
                else_block: MirBlock::default(),
            },
            location,
        );
        Ok(Value::Local(normalized))
    }

    pub(super) fn assign_slice_alias(
        &mut self,
        name: &str,
        slice: LoweredSlice,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if let Some(binding) = self.bindings.get(name).cloned() {
            let Binding::Slice(local, element, access) = binding else {
                return Err(self.error(
                    format!("slice alias '{name}' conflicts with an existing non-slice binding"),
                    location,
                ));
            };
            if element != slice.element || access != slice.access {
                return Err(self.error(
                    format!("slice alias '{name}' changed element type or access mode"),
                    location,
                ));
            }
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place::local(local),
                    value: Rvalue::Use(slice.value),
                },
                location,
            );
            return Ok(());
        }
        let Value::Local(local) = slice.value else {
            unreachable!("slice construction always produces a local")
        };
        if self.locals[local.index()].name.is_none() {
            self.locals[local.index()].name = Some(name.to_owned());
        }
        self.bindings.insert(
            name.to_owned(),
            Binding::Slice(local, slice.element, slice.access),
        );
        Ok(())
    }

    pub(super) fn lower_slice_assignment(
        &mut self,
        base: &str,
        selection: SliceSelection<'_>,
        expression: &Expr,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let destination = self.lower_named_slice(
            base,
            selection,
            Some(onda_mir::AccessMode::ReadWrite),
            block,
            location,
        )?;
        if self.is_slice_expression(expression) {
            let source = self.lower_slice_expression(
                expression,
                Some(onda_mir::AccessMode::ReadOnly),
                block,
            )?;
            self.push_statement(
                block,
                StatementKind::SliceCopy {
                    destination: destination.value,
                    source: source.value,
                },
                location,
            );
        } else {
            let value = self.lower_expr(expression, block)?;
            let value = self.coerce(value, destination.element, block, expression.loc())?;
            self.push_statement(
                block,
                StatementKind::SliceFill {
                    destination: destination.value,
                    value: value.value,
                },
                location,
            );
        }
        Ok(())
    }

    pub(super) fn assign_index_target(
        &mut self,
        base: &str,
        index: &Expr,
        values: &[LoweredValue],
        block: &mut MirBlock,
        value_location: SourceLoc,
        statement_location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if let Some(Binding::TupleReferenceParameter(components)) = self.bindings.get(base).cloned()
        {
            let component_index = self.constant_tuple_index(base, index, components.len())?;
            let (parameter, ty) = components[component_index];
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Parameter(parameter),
                        projections: Vec::new(),
                    },
                    value: Rvalue::Use(value.value),
                },
                statement_location,
            );
            return Ok(());
        }
        if let Some(Binding::TupleSliceElementAlias(components)) = self.bindings.get(base).cloned()
        {
            let component_index = self.constant_tuple_index(base, index, components.len())?;
            let (slice, ty, element_index) = components[component_index];
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            self.push_statement(
                block,
                StatementKind::SliceStore {
                    slice: Value::Local(slice),
                    index: Value::Local(element_index),
                    value: value.value,
                    bounds: BoundsMode::Unchecked,
                },
                statement_location,
            );
            return Ok(());
        }
        if let Some(Binding::ArrayParameter(parameter, element, _)) =
            self.bindings.get(base).cloned()
        {
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, element, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Parameter(parameter),
                        projections: vec![Projection::Index {
                            index: index_value.value,
                            bounds: BoundsMode::Clamp,
                        }],
                    },
                    value: Rvalue::Use(value.value),
                },
                statement_location,
            );
            return Ok(());
        }
        if let Some(Binding::BufferParameter(parameter, element)) = self.bindings.get(base).cloned()
        {
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, element, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            self.push_statement(
                block,
                StatementKind::BufferParamStore {
                    parameter: onda_mir::BufferParamRef::Direct(parameter),
                    channel: None,
                    index: index_value.value,
                    value: value.value,
                    bounds: BoundsMode::Clamp,
                },
                statement_location,
            );
            return Ok(());
        }
        if let Some(Binding::BufferAlias(reference, element)) = self.bindings.get(base).cloned() {
            let reference = self.materialize_buffer_reference(reference, block, statement_location);
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, element, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            let statement = match reference {
                MaterializedBufferReference::Interface(buffer) => StatementKind::BufferStore {
                    buffer,
                    channel: None,
                    index: index_value.value,
                    value: value.value,
                    bounds: BoundsMode::Clamp,
                },
                MaterializedBufferReference::Parameter(parameter) => {
                    StatementKind::BufferParamStore {
                        parameter,
                        channel: None,
                        index: index_value.value,
                        value: value.value,
                        bounds: BoundsMode::Clamp,
                    }
                }
            };
            self.push_statement(block, statement, statement_location);
            return Ok(());
        }
        if let Some(Binding::Array(local, element, _)) = self.bindings.get(base).cloned() {
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, element, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Local(local),
                        projections: vec![Projection::Index {
                            index: index_value.value,
                            bounds: BoundsMode::Clamp,
                        }],
                    },
                    value: Rvalue::Use(value.value),
                },
                statement_location,
            );
            return Ok(());
        }
        if let Some(Binding::Slice(local, element, access)) = self.bindings.get(base).cloned() {
            if access != onda_mir::AccessMode::ReadWrite {
                return Err(self.error(
                    format!("slice parameter or alias '{base}' is read-only"),
                    statement_location,
                ));
            }
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, element, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            self.push_statement(
                block,
                StatementKind::SliceStore {
                    slice: Value::Local(local),
                    index: index_value.value,
                    value: value.value,
                    bounds: BoundsMode::Clamp,
                },
                statement_location,
            );
            return Ok(());
        }
        if matches!(
            self.bindings.get(base),
            Some(Binding::EventArrayParameter(_, _, _))
        ) {
            return Err(self.error(
                format!("event array parameter '{base}' is read-only"),
                statement_location,
            ));
        }
        if self.assign_dynamic_interface_index(
            base,
            index,
            values,
            block,
            value_location,
            statement_location,
        )? {
            return Ok(());
        }
        let state_tuple = self
            .runtime_globals
            .and_then(|globals| globals.state_tuples.get(base).cloned());
        if let Some(components) = state_tuple {
            let component = components[self.constant_tuple_index(base, index, components.len())?];
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, component.1, block, value_location)?;
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::State(component.0),
                        projections: Vec::new(),
                    },
                    value: Rvalue::Use(value.value),
                },
                statement_location,
            );
            return Ok(());
        }
        let control_output_array = self
            .runtime_globals
            .and_then(|globals| globals.control_output_arrays.get(base).copied());
        if let Some((output, ty, _)) = control_output_array {
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            self.push_statement(
                block,
                StatementKind::ControlOutputStore {
                    output,
                    element: Some(index_value.value),
                    bounds: BoundsMode::Clamp,
                    value: value.value,
                },
                statement_location,
            );
            return Ok(());
        }
        let output_array = self
            .runtime_globals
            .and_then(|globals| globals.output_arrays.get(base).copied());
        if let Some((output, ty, _)) = output_array {
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            if let Some((cache, cache_ty, _)) = self.audio_output_array_caches.get(&output).copied()
            {
                debug_assert_eq!(cache_ty, ty);
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::Local(cache),
                            projections: vec![Projection::Index {
                                index: index_value.value,
                                bounds: BoundsMode::Clamp,
                            }],
                        },
                        value: Rvalue::Use(value.value),
                    },
                    statement_location,
                );
                return Ok(());
            }
            let frame = self.current_frame.ok_or_else(|| {
                self.error(
                    format!("audio output array '{base}' was written outside the sample section"),
                    statement_location,
                )
            })?;
            self.push_statement(
                block,
                StatementKind::OutputStore {
                    output,
                    element: Some(index_value.value),
                    bounds: BoundsMode::Clamp,
                    frame,
                    value: value.value,
                },
                statement_location,
            );
            return Ok(());
        }
        if self.runtime_globals.is_some_and(|globals| {
            globals.input_arrays.contains_key(base) || globals.param_arrays.contains_key(base)
        }) {
            return Err(self.error(
                format!("interface array '{base}' is read-only"),
                statement_location,
            ));
        }
        let state_array = self
            .runtime_globals
            .and_then(|globals| globals.state_arrays.get(base).copied());
        if let Some((state, ty, _)) = state_array {
            let value = self.single_global_value(base, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::State(state),
                        projections: vec![Projection::Index {
                            index: index_value.value,
                            bounds: BoundsMode::Clamp,
                        }],
                    },
                    value: Rvalue::Use(value.value),
                },
                statement_location,
            );
            return Ok(());
        }
        if self.const_arrays.contains_key(base) {
            return Err(self.error(
                format!("constant array '{base}' is read-only"),
                statement_location,
            ));
        }
        let buffer = self
            .runtime_globals
            .and_then(|globals| globals.buffers.get(base).copied());
        let Some((buffer, ty)) = buffer else {
            return Err(self.error(
                format!("indexed assignment target '{base}' is outside the current MIR boundary"),
                statement_location,
            ));
        };
        let value = self.single_global_value(base, values, statement_location)?;
        let value = self.coerce(value, ty, block, value_location)?;
        let index_value = self.lower_expr(index, block)?;
        let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
        self.push_statement(
            block,
            StatementKind::BufferStore {
                buffer: onda_mir::BufferRef::Direct(buffer),
                channel: None,
                index: index_value.value,
                value: value.value,
                bounds: BoundsMode::Clamp,
            },
            statement_location,
        );
        Ok(())
    }

    fn dynamic_interface_write_view(
        &self,
        base: &str,
        location: SourceLoc,
    ) -> Result<Option<RuntimeInterfaceView>, MirLoweringError> {
        let kind = match base {
            "outs" => DynamicInterfaceKind::AudioOutputs,
            "kouts" => DynamicInterfaceKind::ControlOutputs,
            "ins" | "params" | "kins" => {
                return Err(self.error(
                    format!("dynamic interface view '{base}' is read-only"),
                    location,
                ));
            }
            _ => return Ok(None),
        };
        let globals = self.runtime_globals.ok_or_else(|| {
            self.error(
                format!("dynamic interface view '{base}' is unavailable outside runtime lowering"),
                location,
            )
        })?;
        let view = globals.interface_views.get(&kind).cloned().ok_or_else(|| {
            self.error(
                format!("semantic analysis did not resolve dynamic interface view '{base}'"),
                location,
            )
        })?;
        Ok(Some(view))
    }

    pub(super) fn assign_dynamic_interface_index(
        &mut self,
        base: &str,
        index: &Expr,
        values: &[LoweredValue],
        block: &mut MirBlock,
        value_location: SourceLoc,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let Some(view) = self.dynamic_interface_write_view(base, statement_location)? else {
            return Ok(false);
        };
        let value = self.single_global_value(base, values, statement_location)?;
        let value = self.coerce(value, view.element_type, block, value_location)?;
        let selected =
            self.lower_dynamic_interface_index(index, view.slots.len(), BoundsMode::Clamp, block)?;
        let dispatch = self.dynamic_interface_write_dispatch(
            &view.slots,
            0,
            view.slots.len(),
            selected,
            value.value,
            statement_location,
        )?;
        block.statements.extend(dispatch.statements);
        Ok(true)
    }

    pub(super) fn lower_dynamic_interface_write_call(
        &mut self,
        base: &str,
        index: &Expr,
        value: &Expr,
        bounds: BoundsMode,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let Some(view) = self.dynamic_interface_write_view(base, location)? else {
            return Ok(false);
        };
        let selected =
            self.lower_dynamic_interface_index(index, view.slots.len(), bounds, block)?;
        let value_location = value.loc();
        let value = self.lower_expr(value, block)?;
        let value = self.coerce(value, view.element_type, block, value_location)?;
        let dispatch = self.dynamic_interface_write_dispatch(
            &view.slots,
            0,
            view.slots.len(),
            selected,
            value.value,
            location,
        )?;
        block.statements.extend(dispatch.statements);
        Ok(true)
    }

    pub(super) fn dynamic_interface_write_dispatch(
        &mut self,
        slots: &[RuntimeInterfaceEndpoint],
        start: usize,
        end: usize,
        selected: Value,
        value: Value,
        location: SourceLoc,
    ) -> Result<MirBlock, MirLoweringError> {
        if start >= end || end > slots.len() {
            return Err(self.error("dynamic interface write dispatch has no endpoint", location));
        }
        let mut block = MirBlock::default();
        if end - start == 1 {
            let endpoint = slots[start].clone();
            self.push_dynamic_interface_store(&mut block, endpoint, value, location)?;
            return Ok(block);
        }

        let midpoint = start + (end - start) / 2;
        let midpoint_i32 = i32::try_from(midpoint).map_err(|_| {
            self.error(
                "dynamic interface slot index is outside the i32 boundary",
                location,
            )
        })?;
        let condition = self.emit_temp(
            &mut block,
            PrimitiveType::Bool,
            Rvalue::Compare {
                op: CompareOp::Less,
                lhs: selected,
                rhs: Value::Constant(ScalarValue::I32(midpoint_i32)),
            },
            location,
        );
        let then_block = self
            .dynamic_interface_write_dispatch(slots, start, midpoint, selected, value, location)?;
        let else_block =
            self.dynamic_interface_write_dispatch(slots, midpoint, end, selected, value, location)?;
        self.push_statement(
            &mut block,
            StatementKind::If {
                condition: condition.value,
                then_block,
                else_block,
            },
            location,
        );
        Ok(block)
    }

    pub(super) fn push_dynamic_interface_store(
        &mut self,
        block: &mut MirBlock,
        endpoint: RuntimeInterfaceEndpoint,
        value: Value,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let fixed_element = |element: Option<u32>| {
            element.map(|element| Value::Constant(ScalarValue::I32(element as i32)))
        };
        match endpoint {
            RuntimeInterfaceEndpoint::AudioOutput { output, element } => {
                if let Some(element) = element {
                    if let Some((local, _, len)) =
                        self.audio_output_array_caches.get(&output).copied()
                    {
                        debug_assert!(element < len);
                        self.push_statement(
                            block,
                            StatementKind::Assign {
                                destination: Place {
                                    base: PlaceBase::Local(local),
                                    projections: vec![Projection::Index {
                                        index: Value::Constant(ScalarValue::I32(element as i32)),
                                        bounds: BoundsMode::Unchecked,
                                    }],
                                },
                                value: Rvalue::Use(value),
                            },
                            location,
                        );
                        return Ok(());
                    }
                } else {
                    if let Some((local, _)) =
                        self.audio_output_endpoint_caches.get(&output).copied()
                    {
                        self.push_statement(
                            block,
                            StatementKind::Assign {
                                destination: Place::local(local),
                                value: Rvalue::Use(value),
                            },
                            location,
                        );
                        return Ok(());
                    }
                }
                let frame = self.current_frame.ok_or_else(|| {
                    self.error(
                        "dynamic audio output view was written outside the sample section",
                        location,
                    )
                })?;
                self.push_statement(
                    block,
                    StatementKind::OutputStore {
                        output,
                        element: fixed_element(element),
                        bounds: BoundsMode::Unchecked,
                        frame,
                        value,
                    },
                    location,
                );
            }
            RuntimeInterfaceEndpoint::ControlOutput { output, element } => {
                self.push_statement(
                    block,
                    StatementKind::ControlOutputStore {
                        output,
                        element: fixed_element(element),
                        bounds: BoundsMode::Unchecked,
                        value,
                    },
                    location,
                );
            }
            RuntimeInterfaceEndpoint::Input { .. } | RuntimeInterfaceEndpoint::Param { .. } => {
                return Err(self.error("dynamic interface endpoint is read-only", location));
            }
        }
        Ok(())
    }
}
