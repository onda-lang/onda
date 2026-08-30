use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn single_global_value(
        &self,
        name: &str,
        values: &[LoweredValue],
        location: SourceLoc,
    ) -> Result<LoweredValue, MirLoweringError> {
        if values.len() != 1 {
            return Err(self.error(
                format!(
                    "scalar runtime value '{name}' received {} tuple components",
                    values.len()
                ),
                location,
            ));
        }
        Ok(values[0])
    }

    pub(super) fn assign_destructured_values(
        &mut self,
        targets: &[TupleAssignTarget],
        values: Vec<LoweredValue>,
        block: &mut MirBlock,
        value_location: SourceLoc,
        statement_location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if targets.len() != values.len() {
            return Err(self.error(
                format!(
                    "tuple destructuring arity mismatch after semantic analysis: {} targets, {} values",
                    targets.len(),
                    values.len()
                ),
                statement_location,
            ));
        }
        for (target, value) in targets.iter().zip(values) {
            let Some(target) = target.binding() else {
                continue;
            };
            let (local, target_ty) =
                self.scalar_local_for_destructure(target, value.ty, statement_location)?;
            let value = self.coerce(value, target_ty, block, value_location)?;
            self.assign_value(block, local, value.value, statement_location);
        }
        Ok(())
    }

    pub(super) fn tuple_local(
        &mut self,
        name: &str,
        source_types: &[PrimitiveType],
        location: SourceLoc,
    ) -> Result<Vec<(LocalId, PrimitiveType)>, MirLoweringError> {
        if let Some(binding) = self.bindings.get(name).cloned() {
            return match binding {
                Binding::Tuple(components) => {
                    if components.len() != source_types.len() {
                        Err(self.error(
                            format!(
                                "tuple local '{name}' changed arity from {} to {}",
                                components.len(),
                                source_types.len()
                            ),
                            location,
                        ))
                    } else {
                        Ok(components)
                    }
                }
                Binding::TupleReferenceParameter(_) => Err(self.error(
                    format!("tuple reference field '{name}' requires component assignment"),
                    location,
                )),
                Binding::TupleSliceElementAlias(_) => Err(self.error(
                    format!("tuple element alias '{name}' requires component assignment"),
                    location,
                )),
                Binding::InitAll
                | Binding::ReferenceParameter(_, _)
                | Binding::EventParameter(_, _)
                | Binding::EventArrayParameter(_, _, _)
                | Binding::BufferParameter(_, _)
                | Binding::BufferParameterArray(_, _, _)
                | Binding::BufferAlias(_, _)
                | Binding::Array(_, _, _)
                | Binding::ArrayParameter(_, _, _)
                | Binding::Slice(_, _, _)
                | Binding::Local(_, _)
                | Binding::SliceElementAlias { .. }
                | Binding::StructArrayElementAlias { .. } => Err(self.error(
                    format!("cannot assign a tuple value to scalar local '{name}'"),
                    location,
                )),
                Binding::StructParameter { .. } => Err(self.error(
                    format!("cannot assign a tuple value to struct parameter '{name}'"),
                    location,
                )),
                Binding::StructArrayParameter { .. } => Err(self.error(
                    format!("cannot assign a tuple value to struct-array parameter '{name}'"),
                    location,
                )),
                Binding::ProcArrayParameter { .. } => Err(self.error(
                    format!("cannot assign a tuple value to proc-array parameter '{name}'"),
                    location,
                )),
            };
        }

        let components = source_types
            .iter()
            .copied()
            .enumerate()
            .map(|(index, ty)| (self.new_local(Some(format!("{name}.{index}")), ty), ty))
            .collect::<Vec<_>>();
        self.bindings
            .insert(name.to_owned(), Binding::Tuple(components.clone()));
        Ok(components)
    }

    pub(super) fn scalar_local_for_destructure(
        &mut self,
        name: &str,
        inferred_ty: PrimitiveType,
        location: SourceLoc,
    ) -> Result<(LocalId, PrimitiveType), MirLoweringError> {
        if let Some(binding) = self.bindings.get(name).cloned() {
            return match binding {
                Binding::Local(local, ty) => Ok((local, ty)),
                Binding::InitAll
                | Binding::ReferenceParameter(_, _)
                | Binding::EventParameter(_, _)
                | Binding::EventArrayParameter(_, _, _)
                | Binding::BufferParameter(_, _)
                | Binding::BufferParameterArray(_, _, _)
                | Binding::BufferAlias(_, _)
                | Binding::Array(_, _, _)
                | Binding::ArrayParameter(_, _, _)
                | Binding::Slice(_, _, _)
                | Binding::SliceElementAlias { .. }
                | Binding::StructArrayElementAlias { .. } => Err(self.error(
                    format!("assignment to read-only parameter '{name}' reached MIR lowering"),
                    location,
                )),
                Binding::Tuple(_) => Err(self.error(
                    format!("cannot destructure a scalar component into tuple local '{name}'"),
                    location,
                )),
                Binding::TupleReferenceParameter(_) => Err(self.error(
                    format!("cannot destructure into tuple reference field '{name}'"),
                    location,
                )),
                Binding::TupleSliceElementAlias(_) => Err(self.error(
                    format!("cannot destructure into tuple element alias '{name}'"),
                    location,
                )),
                Binding::StructParameter { .. } => Err(self.error(
                    format!("cannot destructure into struct parameter '{name}'"),
                    location,
                )),
                Binding::StructArrayParameter { .. } => Err(self.error(
                    format!("cannot destructure into struct-array parameter '{name}'"),
                    location,
                )),
                Binding::ProcArrayParameter { .. } => Err(self.error(
                    format!("cannot destructure into proc-array parameter '{name}'"),
                    location,
                )),
            };
        }
        let local = self.new_local(Some(name.to_owned()), inferred_ty);
        self.bindings
            .insert(name.to_owned(), Binding::Local(local, inferred_ty));
        Ok((local, inferred_ty))
    }

    pub(super) fn scalar_local(
        &mut self,
        name: &str,
        inferred_ty: PrimitiveType,
        location: SourceLoc,
    ) -> Result<(LocalId, PrimitiveType), MirLoweringError> {
        if let Some(binding) = self.bindings.get(name).cloned() {
            return match binding {
                Binding::Local(local, ty) => Ok((local, ty)),
                Binding::InitAll
                | Binding::ReferenceParameter(_, _)
                | Binding::EventParameter(_, _)
                | Binding::EventArrayParameter(_, _, _)
                | Binding::BufferParameter(_, _)
                | Binding::BufferParameterArray(_, _, _)
                | Binding::BufferAlias(_, _)
                | Binding::Array(_, _, _)
                | Binding::ArrayParameter(_, _, _)
                | Binding::Slice(_, _, _)
                | Binding::SliceElementAlias { .. }
                | Binding::StructArrayElementAlias { .. } => Err(self.error(
                    format!("assignment to read-only parameter '{name}' reached MIR lowering"),
                    location,
                )),
                Binding::Tuple(_) => Err(self.error(
                    format!("assignment to tuple local '{name}' requires a tuple value"),
                    location,
                )),
                Binding::TupleReferenceParameter(_) => Err(self.error(
                    format!("cannot assign a scalar value to tuple reference field '{name}'"),
                    location,
                )),
                Binding::TupleSliceElementAlias(_) => Err(self.error(
                    format!("cannot assign a scalar value to tuple element alias '{name}'"),
                    location,
                )),
                Binding::StructParameter { .. } => Err(self.error(
                    format!("cannot assign a scalar value to struct parameter '{name}'"),
                    location,
                )),
                Binding::StructArrayParameter { .. } => Err(self.error(
                    format!("cannot assign a scalar value to struct-array parameter '{name}'"),
                    location,
                )),
                Binding::ProcArrayParameter { .. } => Err(self.error(
                    format!("cannot assign a scalar value to proc-array parameter '{name}'"),
                    location,
                )),
            };
        }
        let same_named_local_already_lowered = self
            .locals
            .iter()
            .any(|local| local.name.as_deref() == Some(name));
        let ty = if same_named_local_already_lowered {
            // `local_scalar_types` is keyed by source spelling, while nested
            // branch/loop locals with the same spelling are distinct bindings.
            // Once one such binding has been lowered, the current assignment
            // context is the authoritative type for a fresh sibling/outer
            // binding.
            inferred_ty
        } else {
            self.function
                .local_scalar_types
                .get(name)
                .copied()
                .or_else(|| self.runtime_globals.map(|_| inferred_ty))
                .ok_or_else(|| {
                    self.error(
                        format!(
                            "semantic analysis did not retain a scalar type for local '{name}'"
                        ),
                        location,
                    )
                })?
        };
        let local = self.new_local(Some(name.to_owned()), ty);
        self.bindings
            .insert(name.to_owned(), Binding::Local(local, ty));
        Ok((local, ty))
    }

    pub(super) fn lower_explicit_cast(
        &mut self,
        value: LoweredValue,
        to: PrimitiveType,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredValue, MirLoweringError> {
        if value.ty == to {
            return Ok(value);
        }
        if value.ty == PrimitiveType::Bool {
            let result = self.new_local(None, to);
            self.assign_value(block, result, zero_value(to), location);
            let mut then_block = MirBlock::default();
            self.assign_value(
                &mut then_block,
                result,
                Value::Constant(scalar_from_f64(1.0, to)),
                location,
            );
            self.push_statement(
                block,
                StatementKind::If {
                    condition: value.value,
                    then_block,
                    else_block: MirBlock::default(),
                },
                location,
            );
            return Ok(LoweredValue {
                value: Value::Local(result),
                ty: to,
            });
        }
        if to == PrimitiveType::Bool {
            return Ok(self.emit_temp(
                block,
                PrimitiveType::Bool,
                Rvalue::Compare {
                    op: CompareOp::NotEqual,
                    lhs: value.value,
                    rhs: zero_value(value.ty),
                },
                location,
            ));
        }
        self.coerce(value, to, block, location)
    }

    pub(super) fn coerce(
        &mut self,
        value: LoweredValue,
        to: PrimitiveType,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredValue, MirLoweringError> {
        if value.ty == to {
            return Ok(value);
        }
        if value.ty == PrimitiveType::Bool || to == PrimitiveType::Bool {
            return Err(self.error(
                format!(
                    "invalid scalar conversion from {} to {} reached MIR lowering",
                    value.ty.name(),
                    to.name()
                ),
                location,
            ));
        }
        if let Value::Constant(constant) = value.value {
            let constant = cast_scalar_constant(constant, to).ok_or_else(|| {
                self.error(
                    format!(
                        "invalid constant conversion from {} to {} reached MIR lowering",
                        value.ty.name(),
                        to.name()
                    ),
                    location,
                )
            })?;
            return Ok(LoweredValue {
                value: Value::Constant(constant),
                ty: to,
            });
        }
        let integer_range = match value.value {
            Value::Local(local) => self.locals[local.index()]
                .integer_range
                .and_then(|range| cast_integer_range_invariant(range, to)),
            Value::Constant(_) => None,
        };
        let result = self.emit_temp(
            block,
            to,
            Rvalue::Cast {
                value: value.value,
                to: scalar_type(to),
            },
            location,
        );
        if let (Value::Local(local), Some(range)) = (result.value, integer_range) {
            self.locals[local.index()].integer_range = Some(range);
        }
        Ok(result)
    }

    pub(super) fn merge_numeric(
        &self,
        lhs: PrimitiveType,
        rhs: PrimitiveType,
        context: &str,
        location: SourceLoc,
    ) -> Result<PrimitiveType, MirLoweringError> {
        let mut diagnostics = Vec::new();
        merge_numeric_types(lhs, rhs, context, &mut diagnostics).ok_or_else(|| {
            let detail = diagnostics
                .first()
                .map(|diagnostic| diagnostic.message.as_str())
                .unwrap_or("numeric types could not be merged");
            self.error(format!("{detail} after semantic analysis"), location)
        })
    }

    pub(super) fn snapshot(
        &mut self,
        value: LoweredValue,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> LoweredValue {
        self.emit_temp(block, value.ty, Rvalue::Use(value.value), location)
    }

    pub(super) fn compare_value(
        &mut self,
        block: &mut MirBlock,
        op: CompareOp,
        lhs: Value,
        rhs: Value,
        location: SourceLoc,
    ) -> Value {
        self.emit_temp(
            block,
            PrimitiveType::Bool,
            Rvalue::Compare { op, lhs, rhs },
            location,
        )
        .value
    }

    pub(super) fn emit_temp(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        value: Rvalue,
        location: SourceLoc,
    ) -> LoweredValue {
        let local = self.new_local(None, ty);
        self.push_statement(
            block,
            StatementKind::Assign {
                destination: Place::local(local),
                value,
            },
            location,
        );
        LoweredValue {
            value: Value::Local(local),
            ty,
        }
    }

    pub(super) fn emit_binary_value(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        op: MirBinaryOp,
        lhs: Value,
        rhs: Value,
        location: SourceLoc,
    ) -> Value {
        self.emit_temp(block, ty, Rvalue::Binary { op, lhs, rhs }, location)
            .value
    }

    pub(super) fn load_place_value(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        place: &Place,
        location: SourceLoc,
    ) -> Value {
        self.emit_temp(block, ty, Rvalue::Load(place.clone()), location)
            .value
    }

    pub(super) fn assign_place_value(
        &mut self,
        block: &mut MirBlock,
        place: Place,
        value: Value,
        location: SourceLoc,
    ) {
        self.push_statement(
            block,
            StatementKind::Assign {
                destination: place,
                value: Rvalue::Use(value),
            },
            location,
        );
    }

    pub(super) fn local_array_place(array: LocalId, index: Value) -> Place {
        Place {
            base: PlaceBase::Local(array),
            projections: vec![Projection::Index {
                index,
                bounds: BoundsMode::Unchecked,
            }],
        }
    }

    pub(super) fn load_local_array_value(
        &mut self,
        block: &mut MirBlock,
        array: LocalId,
        index: Value,
        ty: PrimitiveType,
        location: SourceLoc,
    ) -> Value {
        self.load_place_value(block, ty, &Self::local_array_place(array, index), location)
    }

    pub(super) fn store_local_array_value(
        &mut self,
        block: &mut MirBlock,
        array: LocalId,
        index: Value,
        value: Value,
        location: SourceLoc,
    ) {
        self.assign_place_value(
            block,
            Self::local_array_place(array, index),
            value,
            location,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_sinc_multiply_add(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        accum: Value,
        input: Value,
        history: Value,
        coefficient: f64,
        location: SourceLoc,
    ) -> Value {
        let delta =
            self.emit_binary_value(block, ty, MirBinaryOp::Subtract, input, history, location);
        let scaled = self.emit_binary_value(
            block,
            ty,
            MirBinaryOp::Multiply,
            delta,
            float_constant(ty, coefficient),
            location,
        );
        self.emit_binary_value(block, ty, MirBinaryOp::Add, accum, scaled, location)
    }

    pub(super) fn emit_sinc_interpolate(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        taps: &[Place; 8],
        input: Value,
        location: SourceLoc,
    ) -> (Value, Value) {
        let old: [Value; 8] =
            std::array::from_fn(|index| self.load_place_value(block, ty, &taps[index], location));
        let a1 =
            self.emit_sinc_multiply_add(block, ty, old[0], input, old[1], SINC_A1_COEFF, location);
        let a2 =
            self.emit_sinc_multiply_add(block, ty, old[1], a1, old[2], SINC_A2_COEFF, location);
        let a3 =
            self.emit_sinc_multiply_add(block, ty, old[2], a2, old[3], SINC_A3_COEFF, location);
        let b1 =
            self.emit_sinc_multiply_add(block, ty, old[4], input, old[5], SINC_B1_COEFF, location);
        let b2 =
            self.emit_sinc_multiply_add(block, ty, old[5], b1, old[6], SINC_B2_COEFF, location);
        let b3 =
            self.emit_sinc_multiply_add(block, ty, old[6], b2, old[7], SINC_B3_COEFF, location);
        for (place, value) in taps
            .iter()
            .cloned()
            .zip([input, a1, a2, a3, input, b1, b2, b3])
        {
            self.assign_place_value(block, place, value, location);
        }
        (a3, b3)
    }

    pub(super) fn emit_sinc_decimate(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        taps: &[Place; 8],
        input1: Value,
        input2: Value,
        location: SourceLoc,
    ) -> Value {
        let old: [Value; 8] =
            std::array::from_fn(|index| self.load_place_value(block, ty, &taps[index], location));
        let a1 =
            self.emit_sinc_multiply_add(block, ty, old[0], input2, old[1], SINC_A1_COEFF, location);
        let a2 =
            self.emit_sinc_multiply_add(block, ty, old[1], a1, old[2], SINC_A2_COEFF, location);
        let a3 =
            self.emit_sinc_multiply_add(block, ty, old[2], a2, old[3], SINC_A3_COEFF, location);
        let b1 =
            self.emit_sinc_multiply_add(block, ty, old[4], input1, old[5], SINC_B1_COEFF, location);
        let b2 =
            self.emit_sinc_multiply_add(block, ty, old[5], b1, old[6], SINC_B2_COEFF, location);
        let b3 =
            self.emit_sinc_multiply_add(block, ty, old[6], b2, old[7], SINC_B3_COEFF, location);
        for (place, value) in taps
            .iter()
            .cloned()
            .zip([input2, a1, a2, a3, input1, b1, b2, b3])
        {
            self.assign_place_value(block, place, value, location);
        }
        let sum = self.emit_binary_value(block, ty, MirBinaryOp::Add, a3, b3, location);
        self.emit_binary_value(
            block,
            ty,
            MirBinaryOp::Multiply,
            sum,
            float_constant(ty, 0.5),
            location,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_interpolation_frame(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        raw: Value,
        values: LocalId,
        taps: &[Place; 8],
        stage_index: usize,
        index: Value,
        paired_index: Value,
        location: SourceLoc,
    ) {
        let input = if stage_index == 0 {
            raw
        } else {
            self.load_local_array_value(block, values, index, ty, location)
        };
        let (output1, output2) = self.emit_sinc_interpolate(block, ty, taps, input, location);
        self.store_local_array_value(block, values, index, output1, location);
        self.store_local_array_value(block, values, paired_index, output2, location);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_interpolation_stages(
        &mut self,
        block: &mut MirBlock,
        ty: PrimitiveType,
        raw: Value,
        values: LocalId,
        stages: &[[Place; 8]],
        factor: usize,
        location: SourceLoc,
    ) {
        let factor_i32 = i32::try_from(factor)
            .expect("oversampling factor has been checked against the MIR index boundary");
        if factor / 2 > MAX_STATIC_SINC_STAGE_ITERATIONS {
            // Dynamic stages do not establish per-element definite assignment
            // in the validator, so give the scratch array a complete image
            // before the stage graph starts mutating it.
            for index in 0..factor_i32 {
                self.store_local_array_value(
                    block,
                    values,
                    Value::Constant(ScalarValue::I32(index)),
                    zero_value(ty),
                    location,
                );
            }
        }

        // One- and two-iteration sinc stages stay statically expanded: the
        // constant indices let native backends scalar-replace the scratch
        // arrays and measured materially faster for common 2x/4x audio paths.
        // Larger traversals remain explicit strided MIR loops, bounding the
        // frontend schedule while preserving the profitable small kernels.
        let mut step = factor / 2;
        for (stage_index, taps) in stages.iter().enumerate() {
            let stride = step * 2;
            let iterations = factor / stride;
            if iterations <= MAX_STATIC_SINC_STAGE_ITERATIONS {
                let mut frame = 0usize;
                while frame < factor {
                    self.emit_interpolation_frame(
                        block,
                        ty,
                        raw,
                        values,
                        taps,
                        stage_index,
                        Value::Constant(ScalarValue::I32(frame as i32)),
                        Value::Constant(ScalarValue::I32((frame + step) as i32)),
                        location,
                    );
                    frame += stride;
                }
            } else {
                let step_i32 = i32::try_from(step)
                    .expect("interpolation step fits the checked oversampling factor");
                let stride_i32 = i32::try_from(stride)
                    .expect("interpolation stride fits the checked oversampling factor");
                let frame = self.new_local(
                    Some(format!("$oversample.interpolate.stage{stage_index}.frame")),
                    PrimitiveType::I32,
                );
                let mut iteration_body = MirBlock::default();
                let index = Value::Local(frame);
                let paired_index = self.emit_binary_value(
                    &mut iteration_body,
                    PrimitiveType::I32,
                    MirBinaryOp::Add,
                    index,
                    Value::Constant(ScalarValue::I32(step_i32)),
                    location,
                );
                self.emit_interpolation_frame(
                    &mut iteration_body,
                    ty,
                    raw,
                    values,
                    taps,
                    stage_index,
                    index,
                    paired_index,
                    location,
                );
                self.emit_strided_loop(
                    block,
                    frame,
                    factor_i32,
                    stride_i32,
                    iteration_body,
                    location,
                );
            }
            step /= 2;
        }
    }

    pub(super) fn emit_decimation_stages(
        &mut self,
        output: &OversampledOutputRuntime,
        factor: usize,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Value {
        if output.down_stages.is_empty() {
            return self.load_local_array_value(
                block,
                output.values,
                Value::Constant(ScalarValue::I32((factor - 1) as i32)),
                output.ty,
                location,
            );
        }

        let mut reduced = zero_value(output.ty);
        let factor_i32 = i32::try_from(factor)
            .expect("oversampling factor has been checked against the MIR index boundary");
        let mut step = 1usize;
        for (stage_index, taps) in output.down_stages.iter().enumerate() {
            let stride = step * 2;
            let iterations = factor / stride;
            if iterations <= MAX_STATIC_SINC_STAGE_ITERATIONS {
                let mut frame = 0usize;
                while frame < factor {
                    let source1 = self.load_local_array_value(
                        block,
                        output.values,
                        Value::Constant(ScalarValue::I32(frame as i32)),
                        output.ty,
                        location,
                    );
                    let source2 = self.load_local_array_value(
                        block,
                        output.values,
                        Value::Constant(ScalarValue::I32((frame + step) as i32)),
                        output.ty,
                        location,
                    );
                    let value =
                        self.emit_sinc_decimate(block, output.ty, taps, source1, source2, location);
                    if stage_index + 1 == output.down_stages.len() {
                        reduced = value;
                    } else {
                        self.store_local_array_value(
                            block,
                            output.values,
                            Value::Constant(ScalarValue::I32(frame as i32)),
                            value,
                            location,
                        );
                    }
                    frame += stride;
                }
            } else {
                debug_assert!(stage_index + 1 < output.down_stages.len());
                let step_i32 = i32::try_from(step)
                    .expect("decimation step fits the checked oversampling factor");
                let stride_i32 = i32::try_from(stride)
                    .expect("decimation stride fits the checked oversampling factor");
                let frame = self.new_local(
                    Some(format!("$oversample.decimate.stage{stage_index}.frame")),
                    PrimitiveType::I32,
                );
                let mut iteration_body = MirBlock::default();
                let index = Value::Local(frame);
                let source1 = self.load_local_array_value(
                    &mut iteration_body,
                    output.values,
                    index,
                    output.ty,
                    location,
                );
                let paired_index = self.emit_binary_value(
                    &mut iteration_body,
                    PrimitiveType::I32,
                    MirBinaryOp::Add,
                    index,
                    Value::Constant(ScalarValue::I32(step_i32)),
                    location,
                );
                let source2 = self.load_local_array_value(
                    &mut iteration_body,
                    output.values,
                    paired_index,
                    output.ty,
                    location,
                );
                let value = self.emit_sinc_decimate(
                    &mut iteration_body,
                    output.ty,
                    taps,
                    source1,
                    source2,
                    location,
                );
                self.store_local_array_value(
                    &mut iteration_body,
                    output.values,
                    index,
                    value,
                    location,
                );
                self.emit_strided_loop(
                    block,
                    frame,
                    factor_i32,
                    stride_i32,
                    iteration_body,
                    location,
                );
            }
            step *= 2;
        }
        reduced
    }

    pub(super) fn emit_slice_temp(
        &mut self,
        block: &mut MirBlock,
        name: Option<String>,
        element: PrimitiveType,
        access: onda_mir::AccessMode,
        value: Rvalue,
        location: SourceLoc,
    ) -> LoweredSlice {
        let local = self.new_slice_local(name, element, access);
        self.push_statement(
            block,
            StatementKind::Assign {
                destination: Place::local(local),
                value,
            },
            location,
        );
        LoweredSlice {
            value: Value::Local(local),
            element,
            access,
        }
    }

    pub(super) fn assign_value(
        &mut self,
        block: &mut MirBlock,
        local: LocalId,
        value: Value,
        location: SourceLoc,
    ) {
        let value = self.local_assignment_rvalue(local, value);
        self.push_statement(
            block,
            StatementKind::Assign {
                destination: Place::local(local),
                value,
            },
            location,
        );
    }

    fn local_assignment_rvalue(&self, local: LocalId, value: Value) -> Rvalue {
        let Some(range) = self.locals[local.index()].integer_range else {
            return Rvalue::Use(value);
        };
        if value_is_within_integer_range(value, range, &self.locals) {
            return Rvalue::Use(value);
        }

        Rvalue::Intrinsic {
            intrinsic: match range.mode {
                onda_mir::IntegerRangeMode::Clamp => Intrinsic::RangeClamp,
                onda_mir::IntegerRangeMode::Wrap => Intrinsic::RangeWrap,
            },
            args: vec![
                value,
                Value::Constant(range.min),
                Value::Constant(range.max),
            ],
        }
    }

    pub(super) fn new_local(&mut self, name: Option<String>, ty: PrimitiveType) -> LocalId {
        let id = LocalId::new(self.locals.len() as u32);
        let type_id = self.scalar_type_id(ty);
        self.locals.push(onda_mir::Local {
            name,
            ty: type_id,
            integer_range: None,
        });
        id
    }

    pub(super) fn new_array_local(
        &mut self,
        name: Option<String>,
        element: PrimitiveType,
        len: u32,
    ) -> LocalId {
        let id = LocalId::new(self.locals.len() as u32);
        let ty = intern_array_type(self.types, element, len);
        self.locals.push(onda_mir::Local {
            name,
            ty,
            integer_range: None,
        });
        id
    }

    pub(super) fn new_slice_local(
        &mut self,
        name: Option<String>,
        element: PrimitiveType,
        access: onda_mir::AccessMode,
    ) -> LocalId {
        let id = LocalId::new(self.locals.len() as u32);
        let ty = intern_slice_type(self.types, element, access);
        self.locals.push(onda_mir::Local {
            name,
            ty,
            integer_range: None,
        });
        id
    }

    pub(super) fn scalar_type_id(&mut self, ty: PrimitiveType) -> TypeId {
        intern_scalar_type(self.types, ty)
    }

    pub(super) fn push_statement(
        &mut self,
        block: &mut MirBlock,
        kind: StatementKind,
        location: SourceLoc,
    ) {
        let source = self.source_span(location);
        block.statements.push(Statement { kind, source });
    }

    pub(super) fn source_span(&mut self, location: SourceLoc) -> SourceSpan {
        if location.is_zero() {
            return SourceSpan::UNKNOWN;
        }
        let file = location.file().map(|path| {
            let index = self
                .source_files
                .iter()
                .position(|source| source.path == path)
                .unwrap_or_else(|| {
                    let index = self.source_files.len();
                    self.source_files.push(SourceFile { path });
                    index
                });
            SourceFileId::new(index as u32)
        });
        SourceSpan {
            file,
            line: location.line as u32,
            column: location.column as u32,
            end_line: location.end_line as u32,
            end_column: location.end_column as u32,
        }
    }

    pub(super) fn error(
        &self,
        message: impl Into<String>,
        location: SourceLoc,
    ) -> MirLoweringError {
        MirLoweringError::new(message, location)
    }
}

fn value_is_within_integer_range(
    value: Value,
    destination: onda_mir::IntegerRangeInvariant,
    locals: &[onda_mir::Local],
) -> bool {
    let (source_min, source_max) = match value {
        Value::Constant(value) => (value, value),
        Value::Local(local) => match locals[local.index()].integer_range {
            Some(range) => (range.min, range.max),
            None => return false,
        },
    };
    match (destination.min, destination.max, source_min, source_max) {
        (
            ScalarValue::I32(destination_min),
            ScalarValue::I32(destination_max),
            ScalarValue::I32(source_min),
            ScalarValue::I32(source_max),
        ) => source_min >= destination_min && source_max <= destination_max,
        (
            ScalarValue::I64(destination_min),
            ScalarValue::I64(destination_max),
            ScalarValue::I64(source_min),
            ScalarValue::I64(source_max),
        ) => source_min >= destination_min && source_max <= destination_max,
        _ => false,
    }
}

fn cast_integer_range_invariant(
    range: onda_mir::IntegerRangeInvariant,
    to: PrimitiveType,
) -> Option<onda_mir::IntegerRangeInvariant> {
    let (min, max) = match (range.min, range.max, to) {
        (ScalarValue::I32(min), ScalarValue::I32(max), PrimitiveType::I64) => (
            ScalarValue::I64(i64::from(min)),
            ScalarValue::I64(i64::from(max)),
        ),
        (ScalarValue::I64(min), ScalarValue::I64(max), PrimitiveType::I32) => (
            ScalarValue::I32(i32::try_from(min).ok()?),
            ScalarValue::I32(i32::try_from(max).ok()?),
        ),
        (ScalarValue::I32(min), ScalarValue::I32(max), PrimitiveType::I32) => {
            (ScalarValue::I32(min), ScalarValue::I32(max))
        }
        (ScalarValue::I64(min), ScalarValue::I64(max), PrimitiveType::I64) => {
            (ScalarValue::I64(min), ScalarValue::I64(max))
        }
        _ => return None,
    };
    Some(onda_mir::IntegerRangeInvariant {
        min,
        max,
        mode: range.mode,
    })
}
