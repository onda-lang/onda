use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn lower_expr(
        &mut self,
        expression: &Expr,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        match expression {
            Expr::Number { value, .. } => Ok(LoweredValue {
                value: Value::Constant(ScalarValue::F64(*value)),
                ty: PrimitiveType::F64,
            }),
            Expr::Int { value, .. } => Ok(LoweredValue {
                value: Value::Constant(ScalarValue::I64(*value)),
                ty: PrimitiveType::I64,
            }),
            Expr::Bool { value, .. } => Ok(LoweredValue {
                value: Value::Constant(ScalarValue::Bool(*value)),
                ty: PrimitiveType::Bool,
            }),
            Expr::Var { name, .. } => self.lower_variable(name, expression.loc(), block),
            Expr::Binary { op, lhs, rhs, .. } => {
                self.lower_binary(*op, lhs, rhs, expression.loc(), block)
            }
            Expr::Compare { op, lhs, rhs, .. } => {
                self.lower_compare(*op, lhs, rhs, expression.loc(), block)
            }
            Expr::Call { func, args, .. } => {
                self.lower_intrinsic(*func, args, expression.loc(), block)
            }
            Expr::UserCall {
                name,
                type_args,
                args,
                ..
            } => {
                if let Some(value) =
                    self.lower_buffer_read_call(name, args, expression.loc(), block)?
                {
                    return Ok(value);
                }
                if let Some(value) =
                    self.lower_buffer_metadata_call(name, args, expression.loc(), block)?
                {
                    return Ok(value);
                }
                let values = self
                    .lower_user_call(name, type_args, args, expression.loc(), block)?
                    .ok_or_else(|| {
                        self.error(
                            format!("no-result function '{name}' used as a value"),
                            expression.loc(),
                        )
                    })?;
                if values.len() != 1 {
                    return Err(self.error(
                        format!(
                            "tuple-returning function '{name}' used where a scalar value is required"
                        ),
                        expression.loc(),
                    ));
                }
                Ok(values[0])
            }
            Expr::Cast { to, expr, .. } => {
                let value = self.lower_expr(expr, block)?;
                self.lower_explicit_cast(value, *to, block, expression.loc())
            }
            Expr::UnaryNot { expr, .. } => {
                let operand = self.lower_expr(expr, block)?;
                let operand = self.coerce(operand, PrimitiveType::Bool, block, expression.loc())?;
                Ok(self.emit_temp(
                    block,
                    PrimitiveType::Bool,
                    Rvalue::Unary {
                        op: UnaryOp::LogicalNot,
                        operand: operand.value,
                    },
                    expression.loc(),
                ))
            }
            Expr::UnaryBitNot { expr, .. } => {
                let operand = self.lower_expr(expr, block)?;
                if !matches!(operand.ty, PrimitiveType::I32 | PrimitiveType::I64) {
                    return Err(self.error(
                        "bitwise not operand is not an integer after semantic analysis",
                        expression.loc(),
                    ));
                }
                Ok(self.emit_temp(
                    block,
                    operand.ty,
                    Rvalue::Unary {
                        op: UnaryOp::BitNot,
                        operand: operand.value,
                    },
                    expression.loc(),
                ))
            }
            Expr::Logical { op, lhs, rhs, .. } => {
                self.lower_logical(*op, lhs, rhs, expression.loc(), block)
            }
            Expr::Index { base, index, .. } => {
                self.lower_index(base, index, expression.loc(), block)
            }
            Expr::ArrayLiteral { .. }
            | Expr::Slice { .. }
            | Expr::ArrayCtor { .. }
            | Expr::Tuple { .. } => Err(self.error(
                "aggregate expression is outside the scalar MIR slice",
                expression.loc(),
            )),
        }
    }

    pub(super) fn lower_value_expr(
        &mut self,
        expression: &Expr,
        block: &mut MirBlock,
    ) -> Result<Vec<LoweredValue>, MirLoweringError> {
        match expression {
            Expr::Tuple { values, .. } => values
                .iter()
                .map(|value| self.lower_expr(value, block))
                .collect(),
            Expr::Var { name, .. } => {
                if let Some(Binding::TupleReferenceParameter(components)) =
                    self.bindings.get(name).cloned()
                {
                    let mut values = Vec::with_capacity(components.len());
                    for (parameter, ty) in components {
                        values.push(self.emit_temp(
                            block,
                            ty,
                            Rvalue::Load(Place {
                                base: PlaceBase::Parameter(parameter),
                                projections: Vec::new(),
                            }),
                            expression.loc(),
                        ));
                    }
                    return Ok(values);
                }
                if let Some(Binding::TupleSliceElementAlias(components)) =
                    self.bindings.get(name).cloned()
                {
                    let mut values = Vec::with_capacity(components.len());
                    for (slice, ty, index) in components {
                        values.push(self.emit_temp(
                            block,
                            ty,
                            Rvalue::SliceLoad {
                                slice: Value::Local(slice),
                                index: Value::Local(index),
                                bounds: BoundsMode::Unchecked,
                            },
                            expression.loc(),
                        ));
                    }
                    return Ok(values);
                }
                if let Some(Binding::Tuple(components)) = self.bindings.get(name).cloned() {
                    return Ok(components
                        .into_iter()
                        .map(|(local, ty)| LoweredValue {
                            value: Value::Local(local),
                            ty,
                        })
                        .collect());
                }
                let state_components = self
                    .runtime_globals
                    .and_then(|globals| globals.state_tuples.get(name).cloned());
                if let Some(components) = state_components {
                    let mut values = Vec::with_capacity(components.len());
                    for (state, ty) in components {
                        values.push(self.emit_temp(
                            block,
                            ty,
                            Rvalue::Load(Place {
                                base: PlaceBase::State(state),
                                projections: Vec::new(),
                            }),
                            expression.loc(),
                        ));
                    }
                    return Ok(values);
                }
                Ok(vec![self.lower_expr(expression, block)?])
            }
            Expr::UserCall {
                name,
                type_args,
                args,
                ..
            } => {
                if let Some(value) =
                    self.lower_buffer_read_call(name, args, expression.loc(), block)?
                {
                    return Ok(vec![value]);
                }
                if let Some(value) =
                    self.lower_buffer_metadata_call(name, args, expression.loc(), block)?
                {
                    return Ok(vec![value]);
                }
                self.lower_user_call(name, type_args, args, expression.loc(), block)?
                    .ok_or_else(|| {
                        self.error(
                            format!("no-result function '{name}' used as a value"),
                            expression.loc(),
                        )
                    })
            }
            _ => Ok(vec![self.lower_expr(expression, block)?]),
        }
    }

    pub(super) fn lower_variable(
        &mut self,
        name: &str,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        if let Some(binding) = self.bindings.get(name).cloned() {
            return match binding {
                Binding::Local(local, ty) => Ok(LoweredValue {
                    value: Value::Local(local),
                    ty,
                }),
                Binding::ReferenceParameter(parameter, ty) => Ok(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::Parameter(parameter),
                        projections: Vec::new(),
                    }),
                    location,
                )),
                Binding::SliceElementAlias {
                    slice,
                    element,
                    index,
                } => Ok(self.emit_temp(
                    block,
                    element,
                    Rvalue::SliceLoad {
                        slice: Value::Local(slice),
                        index: Value::Local(index),
                        bounds: BoundsMode::Unchecked,
                    },
                    location,
                )),
                Binding::EventParameter(parameter, ty) => Ok(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::EventParam(parameter),
                        projections: Vec::new(),
                    }),
                    location,
                )),
                Binding::EventArrayParameter(_, _, _) => Err(self.error(
                    format!("event array parameter '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::BufferParameter(_, _) => Err(self.error(
                    format!("buffer parameter '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::BufferAlias(_, _) => Err(self.error(
                    format!("buffer-reference alias '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::BufferParameterArray(_, _, _) => Err(self.error(
                    format!("buffer collection parameter '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::Array(_, _, _) => Err(self.error(
                    format!("array local '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::ArrayParameter(_, _, _) => Err(self.error(
                    format!("array field '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::Slice(_, _, _) => Err(self.error(
                    format!("slice variable '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::Tuple(_) => Err(self.error(
                    format!("tuple variable '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::TupleReferenceParameter(_) => Err(self.error(
                    format!("tuple field '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::TupleSliceElementAlias(_) => Err(self.error(
                    format!("tuple field alias '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::StructParameter { .. } => Err(self.error(
                    format!("struct parameter '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::StructArrayParameter { .. } => Err(self.error(
                    format!(
                        "struct-array parameter '{name}' used where a scalar value is required"
                    ),
                    location,
                )),
                Binding::ProcArrayParameter { .. } => Err(self.error(
                    format!("proc-array parameter '{name}' used where a scalar value is required"),
                    location,
                )),
                Binding::StructArrayElementAlias { .. } => Err(self.error(
                    format!("struct-array element alias '{name}' used as a scalar value"),
                    location,
                )),
            };
        }

        if let Some((local, ty)) = self.oversampled_inputs.get(name).copied() {
            return Ok(LoweredValue {
                value: Value::Local(local),
                ty,
            });
        }

        if self.const_arrays.contains_key(name) {
            return Err(self.error(
                format!("constant array '{name}' used where a scalar value is required"),
                location,
            ));
        }

        if let Some(globals) = self.runtime_globals {
            if globals.state_tuples.contains_key(name) {
                return Err(self.error(
                    format!("tuple state '{name}' used where a scalar value is required"),
                    location,
                ));
            }
            if globals.state_arrays.contains_key(name) {
                return Err(self.error(
                    format!("array state '{name}' used where a scalar value is required"),
                    location,
                ));
            }
            if globals.input_arrays.contains_key(name)
                || globals.output_arrays.contains_key(name)
                || globals.control_output_arrays.contains_key(name)
                || globals.param_arrays.contains_key(name)
            {
                return Err(self.error(
                    format!("interface array '{name}' used where a scalar value is required"),
                    location,
                ));
            }
            if let Some((state, ty)) = globals.states.get(name).copied() {
                return Ok(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    }),
                    location,
                ));
            }
            if let Some((param, ty)) = globals.params.get(name).copied() {
                return Ok(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::Param(param),
                        projections: Vec::new(),
                    }),
                    location,
                ));
            }
            if let Some((input, ty)) = globals.inputs.get(name).copied() {
                let frame = self.current_frame.ok_or_else(|| {
                    self.error(
                        format!("audio input '{name}' was read outside the sample section"),
                        location,
                    )
                })?;
                return Ok(self.emit_temp(
                    block,
                    ty,
                    Rvalue::InputLoad {
                        input,
                        element: None,
                        bounds: BoundsMode::Unchecked,
                        frame,
                    },
                    location,
                ));
            }
            if globals.outputs.contains_key(name) || globals.control_outputs.contains_key(name) {
                return Err(self.error(
                    format!("output '{name}' cannot be read as a scalar value"),
                    location,
                ));
            }
        }

        let Some(ty) = builtin_constant_type(name) else {
            return Err(self.error(
                format!("unresolved scalar variable '{name}' reached MIR lowering"),
                location,
            ));
        };
        let options = AnalysisOptions {
            sample_rate: self.config.sample_rate,
            block_size: self.config.block_size as usize,
        };
        let Some(value) = builtin_constant_value_f64(name, options) else {
            return Err(self.error(
                format!("builtin constant '{name}' has no compile-time value"),
                location,
            ));
        };
        Ok(LoweredValue {
            value: Value::Constant(scalar_from_f64(value, ty)),
            ty,
        })
    }

    pub(super) fn lower_tuple_index(
        &mut self,
        base: &str,
        index: &Expr,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        if let Some(Binding::TupleSliceElementAlias(components)) = self.bindings.get(base).cloned()
        {
            let component_index = self.constant_tuple_index(base, index, components.len())?;
            let (slice, ty, element_index) = components[component_index];
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::SliceLoad {
                    slice: Value::Local(slice),
                    index: Value::Local(element_index),
                    bounds: BoundsMode::Unchecked,
                },
                location,
            ));
        }
        let Some(Binding::Tuple(components)) = self.bindings.get(base) else {
            return Err(self.error(
                format!("indexed value '{base}' is not a tuple local in this MIR slice"),
                location,
            ));
        };
        let component_index = self.constant_tuple_index(base, index, components.len())?;
        let component = components[component_index];
        Ok(LoweredValue {
            value: Value::Local(component.0),
            ty: component.1,
        })
    }

    pub(super) fn constant_tuple_index(
        &self,
        base: &str,
        index: &Expr,
        component_count: usize,
    ) -> Result<usize, MirLoweringError> {
        if !can_eval_const_expr_exact_int(index) {
            return Err(self.error(
                format!("tuple index for '{base}' is not an exact integer constant"),
                index.loc(),
            ));
        }
        let mut diagnostics = Vec::new();
        let raw = eval_const_expr_i64_exact(
            index,
            AnalysisOptions {
                sample_rate: self.config.sample_rate,
                block_size: self.config.block_size as usize,
            },
            "tuple index during MIR lowering",
            &mut diagnostics,
        )
        .ok_or_else(|| {
            let detail = diagnostics
                .first()
                .map(|diagnostic| diagnostic.message.as_str())
                .unwrap_or("tuple index evaluation failed");
            self.error(detail, index.loc())
        })?;
        usize::try_from(raw)
            .ok()
            .filter(|index| *index < component_count)
            .ok_or_else(|| {
                self.error(
                    format!(
                        "tuple index {raw} is outside tuple '{base}' with {} elements",
                        component_count
                    ),
                    index.loc(),
                )
            })
    }

    pub(super) fn lower_index(
        &mut self,
        base: &str,
        index: &Expr,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        if let Some(Binding::TupleReferenceParameter(components)) = self.bindings.get(base).cloned()
        {
            let component_index = self.constant_tuple_index(base, index, components.len())?;
            let (parameter, ty) = components[component_index];
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::Load(Place {
                    base: PlaceBase::Parameter(parameter),
                    projections: Vec::new(),
                }),
                location,
            ));
        }
        if let Some(Binding::ArrayParameter(parameter, element, _)) =
            self.bindings.get(base).cloned()
        {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                element,
                Rvalue::Load(Place {
                    base: PlaceBase::Parameter(parameter),
                    projections: vec![Projection::Index {
                        index: index_value.value,
                        bounds: BoundsMode::Clamp,
                    }],
                }),
                location,
            ));
        }
        if let Some(Binding::BufferParameter(parameter, element)) = self.bindings.get(base).cloned()
        {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                element,
                Rvalue::BufferParamLoad {
                    parameter: onda_mir::BufferParamRef::Direct(parameter),
                    channel: None,
                    index: index_value.value,
                    bounds: BoundsMode::Clamp,
                },
                location,
            ));
        }
        if let Some(Binding::BufferAlias(reference, element)) = self.bindings.get(base).cloned() {
            let reference = self.materialize_buffer_reference(reference, block, location);
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            let rvalue = match reference {
                MaterializedBufferReference::Interface(buffer) => Rvalue::BufferLoad {
                    buffer,
                    channel: None,
                    index: index_value.value,
                    bounds: BoundsMode::Clamp,
                },
                MaterializedBufferReference::Parameter(parameter) => Rvalue::BufferParamLoad {
                    parameter,
                    channel: None,
                    index: index_value.value,
                    bounds: BoundsMode::Clamp,
                },
            };
            return Ok(self.emit_temp(block, element, rvalue, location));
        }
        if let Some(Binding::Array(local, element, _)) = self.bindings.get(base).cloned() {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                element,
                Rvalue::Load(Place {
                    base: PlaceBase::Local(local),
                    projections: vec![Projection::Index {
                        index: index_value.value,
                        bounds: BoundsMode::Clamp,
                    }],
                }),
                location,
            ));
        }
        if let Some(Binding::Slice(local, element, _)) = self.bindings.get(base).cloned() {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                element,
                Rvalue::SliceLoad {
                    slice: Value::Local(local),
                    index: index_value.value,
                    bounds: BoundsMode::Clamp,
                },
                location,
            ));
        }
        if let Some(Binding::EventArrayParameter(parameter, ty, _)) =
            self.bindings.get(base).cloned()
        {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::Load(Place {
                    base: PlaceBase::EventParam(parameter),
                    projections: vec![Projection::Index {
                        index: index_value.value,
                        bounds: BoundsMode::Clamp,
                    }],
                }),
                location,
            ));
        }
        if matches!(
            self.bindings.get(base),
            Some(Binding::Tuple(_) | Binding::TupleSliceElementAlias(_))
        ) {
            return self.lower_tuple_index(base, index, location, block);
        }
        if let Some(value) =
            self.lower_dynamic_interface_read(base, index, BoundsMode::Clamp, location, block)?
        {
            return Ok(value);
        }
        let state_tuple = self
            .runtime_globals
            .and_then(|globals| globals.state_tuples.get(base).cloned());
        if let Some(components) = state_tuple {
            let component = components[self.constant_tuple_index(base, index, components.len())?];
            return Ok(self.emit_temp(
                block,
                component.1,
                Rvalue::Load(Place {
                    base: PlaceBase::State(component.0),
                    projections: Vec::new(),
                }),
                location,
            ));
        }
        let input_array = self
            .runtime_globals
            .and_then(|globals| globals.input_arrays.get(base).copied());
        if let Some((input, ty, _)) = input_array {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            if let Some((cache, cache_ty, _)) = self.oversampled_input_arrays.get(&input).copied() {
                debug_assert_eq!(cache_ty, ty);
                return Ok(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::Local(cache),
                        projections: vec![Projection::Index {
                            index: index_value.value,
                            bounds: BoundsMode::Clamp,
                        }],
                    }),
                    location,
                ));
            }
            let frame = self.current_frame.ok_or_else(|| {
                self.error(
                    format!("audio input array '{base}' was read outside the sample section"),
                    location,
                )
            })?;
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::InputLoad {
                    input,
                    element: Some(index_value.value),
                    bounds: BoundsMode::Clamp,
                    frame,
                },
                location,
            ));
        }
        let param_array = self
            .runtime_globals
            .and_then(|globals| globals.param_arrays.get(base).copied());
        if let Some((param, ty, _)) = param_array {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::Load(Place {
                    base: PlaceBase::Param(param),
                    projections: vec![Projection::Index {
                        index: index_value.value,
                        bounds: BoundsMode::Clamp,
                    }],
                }),
                location,
            ));
        }
        if self
            .runtime_globals
            .is_some_and(|globals| globals.output_arrays.contains_key(base))
        {
            return Err(self.error(
                format!("audio output array '{base}' cannot be read"),
                location,
            ));
        }
        let state_array = self
            .runtime_globals
            .and_then(|globals| globals.state_arrays.get(base).copied());
        if let Some((state, ty, _)) = state_array {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::Load(Place {
                    base: PlaceBase::State(state),
                    projections: vec![Projection::Index {
                        index: index_value.value,
                        bounds: BoundsMode::Clamp,
                    }],
                }),
                location,
            ));
        }
        let const_array = self.const_arrays.get(base).copied();
        if let Some((data, ty, _)) = const_array {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::ConstDataLoad {
                    data,
                    index: index_value.value,
                    bounds: BoundsMode::Clamp,
                },
                location,
            ));
        }
        let buffer = self
            .runtime_globals
            .and_then(|globals| globals.buffers.get(base).copied());
        if let Some((buffer, ty)) = buffer {
            let index_value = self.lower_expr(index, block)?;
            let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
            return Ok(self.emit_temp(
                block,
                ty,
                Rvalue::BufferLoad {
                    buffer: onda_mir::BufferRef::Direct(buffer),
                    channel: None,
                    index: index_value.value,
                    bounds: BoundsMode::Clamp,
                },
                location,
            ));
        }
        Err(self.error(
            format!("indexed value '{base}' is outside the current MIR boundary"),
            location,
        ))
    }

    pub(super) fn lower_dynamic_interface_read(
        &mut self,
        base: &str,
        index: &Expr,
        bounds: BoundsMode,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<Option<LoweredValue>, MirLoweringError> {
        let kind = match base {
            "ins" => DynamicInterfaceKind::Inputs,
            "params" | "kins" => DynamicInterfaceKind::Params,
            "outs" | "kouts" => {
                return Err(self.error(format!("output view '{base}' is write-only"), location));
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
        let selected =
            self.lower_dynamic_interface_index(index, view.slots.len(), bounds, block)?;
        let result = self.new_local(Some(format!("{base}.selected")), view.element_type);
        let dispatch = self.dynamic_interface_read_dispatch(
            &view.slots,
            0,
            view.slots.len(),
            selected,
            result,
            location,
        )?;
        block.statements.extend(dispatch.statements);
        Ok(Some(LoweredValue {
            value: Value::Local(result),
            ty: view.element_type,
        }))
    }

    pub(super) fn lower_dynamic_interface_index(
        &mut self,
        index: &Expr,
        slot_count: usize,
        bounds: BoundsMode,
        block: &mut MirBlock,
    ) -> Result<Value, MirLoweringError> {
        let index_value = self.lower_expr(index, block)?;
        let index_value = self.coerce(index_value, PrimitiveType::I32, block, index.loc())?;
        if bounds == BoundsMode::Unchecked {
            return Ok(index_value.value);
        }
        let upper = slot_count
            .checked_sub(1)
            .and_then(|upper| i32::try_from(upper).ok())
            .ok_or_else(|| {
                self.error(
                    "dynamic interface slot count is outside the i32 indexing boundary",
                    index.loc(),
                )
            })?;
        Ok(Value::Local(self.clamp_index_to_inclusive_upper(
            index_value.value,
            Value::Constant(ScalarValue::I32(upper)),
            block,
            index.loc(),
        )))
    }

    pub(super) fn dynamic_interface_read_dispatch(
        &mut self,
        slots: &[RuntimeInterfaceEndpoint],
        start: usize,
        end: usize,
        selected: Value,
        result: LocalId,
        location: SourceLoc,
    ) -> Result<MirBlock, MirLoweringError> {
        if start >= end || end > slots.len() {
            return Err(self.error("dynamic interface read dispatch has no endpoint", location));
        }
        let mut block = MirBlock::default();
        if end - start == 1 {
            let endpoint = slots[start].clone();
            let value = self.dynamic_interface_read_rvalue(endpoint, location)?;
            self.push_statement(
                &mut block,
                StatementKind::Assign {
                    destination: Place::local(result),
                    value,
                },
                location,
            );
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
            .dynamic_interface_read_dispatch(slots, start, midpoint, selected, result, location)?;
        let else_block =
            self.dynamic_interface_read_dispatch(slots, midpoint, end, selected, result, location)?;
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

    pub(super) fn dynamic_interface_read_rvalue(
        &self,
        endpoint: RuntimeInterfaceEndpoint,
        location: SourceLoc,
    ) -> Result<Rvalue, MirLoweringError> {
        let fixed_element = |element: Option<u32>| {
            element.map(|element| Value::Constant(ScalarValue::I32(element as i32)))
        };
        match endpoint {
            RuntimeInterfaceEndpoint::Input {
                input,
                element,
                clamped,
            } => {
                if let Some(alias) = clamped {
                    debug_assert!(element.is_none());
                    let base = match self.bindings.get(&alias) {
                        Some(Binding::Local(local, _)) => PlaceBase::Local(*local),
                        _ => self
                            .runtime_globals
                            .and_then(|globals| globals.states.get(&alias))
                            .map(|(state, _)| PlaceBase::State(*state))
                            .ok_or_else(|| {
                                self.error(
                                    format!(
                                        "range-clamped dynamic input alias '{alias}' is unavailable"
                                    ),
                                    location,
                                )
                            })?,
                    };
                    return Ok(Rvalue::Load(Place {
                        base,
                        projections: Vec::new(),
                    }));
                }
                if let Some(element) = element {
                    if let Some((local, _, len)) =
                        self.oversampled_input_arrays.get(&input).copied()
                    {
                        debug_assert!(element < len);
                        return Ok(Rvalue::Load(Place {
                            base: PlaceBase::Local(local),
                            projections: vec![Projection::Index {
                                index: Value::Constant(ScalarValue::I32(element as i32)),
                                bounds: BoundsMode::Unchecked,
                            }],
                        }));
                    }
                } else {
                    if let Some((local, _)) = self.oversampled_input_endpoints.get(&input).copied()
                    {
                        return Ok(Rvalue::Load(Place::local(local)));
                    }
                }
                let frame = self.current_frame.ok_or_else(|| {
                    self.error(
                        "dynamic audio input view was read outside the sample section",
                        location,
                    )
                })?;
                Ok(Rvalue::InputLoad {
                    input,
                    element: fixed_element(element),
                    bounds: BoundsMode::Unchecked,
                    frame,
                })
            }
            RuntimeInterfaceEndpoint::AudioOutput { .. } => {
                Err(self.error("audio output endpoints are write-only", location))
            }
            RuntimeInterfaceEndpoint::Param {
                param,
                element,
                clamped,
            } => {
                if let Some(state) = clamped {
                    debug_assert!(element.is_none());
                    return Ok(Rvalue::Load(Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    }));
                }
                Ok(Rvalue::Load(Place {
                    base: PlaceBase::Param(param),
                    projections: element
                        .map(|element| Projection::Index {
                            index: Value::Constant(ScalarValue::I32(element as i32)),
                            bounds: BoundsMode::Unchecked,
                        })
                        .into_iter()
                        .collect(),
                }))
            }
            RuntimeInterfaceEndpoint::ControlOutput { .. } => {
                Err(self.error("control output endpoints are write-only", location))
            }
        }
    }

    pub(super) fn lower_binary(
        &mut self,
        op: AstBinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        let left = self.lower_expr(lhs, block)?;
        let right = self.lower_expr(rhs, block)?;
        let (left_ty, right_ty) = adapt_binary_operand_types(lhs, rhs, left.ty, right.ty);
        let result_ty = if matches!(
            op,
            AstBinaryOp::BitAnd
                | AstBinaryOp::BitOr
                | AstBinaryOp::BitXor
                | AstBinaryOp::ShiftLeft
                | AstBinaryOp::ShiftRight
        ) {
            merge_integer_types(left_ty, right_ty).ok_or_else(|| {
                self.error(
                    "bitwise operands are not integers after semantic analysis",
                    location,
                )
            })?
        } else {
            self.merge_numeric(left_ty, right_ty, "binary expression", location)?
        };
        let left = self.coerce(left, result_ty, block, lhs.loc())?;
        let right = self.coerce(right, result_ty, block, rhs.loc())?;
        Ok(self.emit_temp(
            block,
            result_ty,
            Rvalue::Binary {
                op: map_binary(op),
                lhs: left.value,
                rhs: right.value,
            },
            location,
        ))
    }

    pub(super) fn lower_compare(
        &mut self,
        op: CmpOp,
        lhs: &Expr,
        rhs: &Expr,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        let left = self.lower_expr(lhs, block)?;
        let right = self.lower_expr(rhs, block)?;
        let (left_ty, right_ty) = adapt_binary_operand_types(lhs, rhs, left.ty, right.ty);
        let operand_ty = if left_ty == PrimitiveType::Bool && right_ty == PrimitiveType::Bool {
            PrimitiveType::Bool
        } else {
            self.merge_numeric(left_ty, right_ty, "comparison", location)?
        };
        let left = self.coerce(left, operand_ty, block, lhs.loc())?;
        let right = self.coerce(right, operand_ty, block, rhs.loc())?;
        Ok(self.emit_temp(
            block,
            PrimitiveType::Bool,
            Rvalue::Compare {
                op: map_compare(op),
                lhs: left.value,
                rhs: right.value,
            },
            location,
        ))
    }

    pub(super) fn lower_logical(
        &mut self,
        op: LogicalOp,
        lhs: &Expr,
        rhs: &Expr,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        let left = self.lower_expr(lhs, block)?;
        let left = self.coerce(left, PrimitiveType::Bool, block, lhs.loc())?;
        let result = self.new_local(None, PrimitiveType::Bool);
        let mut then_block = MirBlock::default();
        let mut else_block = MirBlock::default();

        match op {
            LogicalOp::And => {
                let right = self.lower_expr(rhs, &mut then_block)?;
                let right = self.coerce(right, PrimitiveType::Bool, &mut then_block, rhs.loc())?;
                self.assign_value(&mut then_block, result, right.value, location);
                self.assign_value(
                    &mut else_block,
                    result,
                    Value::Constant(ScalarValue::Bool(false)),
                    location,
                );
            }
            LogicalOp::Or => {
                self.assign_value(
                    &mut then_block,
                    result,
                    Value::Constant(ScalarValue::Bool(true)),
                    location,
                );
                let right = self.lower_expr(rhs, &mut else_block)?;
                let right = self.coerce(right, PrimitiveType::Bool, &mut else_block, rhs.loc())?;
                self.assign_value(&mut else_block, result, right.value, location);
            }
        }

        self.push_statement(
            block,
            StatementKind::If {
                condition: left.value,
                then_block,
                else_block,
            },
            location,
        );
        Ok(LoweredValue {
            value: Value::Local(result),
            ty: PrimitiveType::Bool,
        })
    }

    pub(super) fn lower_intrinsic(
        &mut self,
        function: BuiltinFn,
        args: &[Expr],
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<LoweredValue, MirLoweringError> {
        let mut lowered = Vec::with_capacity(args.len());
        for arg in args {
            lowered.push(self.lower_expr(arg, block)?);
        }
        let adapted_types = adapt_numeric_argument_types(
            args,
            &lowered.iter().map(|value| value.ty).collect::<Vec<_>>(),
        );

        let result_ty = intrinsic_result_type(function, &adapted_types).ok_or_else(|| {
            self.error(
                "intrinsic has invalid operand types after semantic analysis",
                location,
            )
        })?;

        let mut values = Vec::with_capacity(lowered.len());
        for (arg, value) in args.iter().zip(lowered) {
            values.push(self.coerce(value, result_ty, block, arg.loc())?.value);
        }
        if matches!(function, BuiltinFn::RangeClamp | BuiltinFn::RangeWrap)
            && matches!(result_ty, PrimitiveType::I32 | PrimitiveType::I64)
        {
            for index in 1..=2 {
                let mut diagnostics = Vec::new();
                if let Some(value) = eval_const_expr_i64_exact(
                    &args[index],
                    AnalysisOptions {
                        sample_rate: self.config.sample_rate,
                        block_size: self.config.block_size as usize,
                    },
                    "integer binding range bound during MIR lowering",
                    &mut diagnostics,
                ) {
                    values[index] = Value::Constant(match result_ty {
                        PrimitiveType::I32 => ScalarValue::I32(value as i32),
                        PrimitiveType::I64 => ScalarValue::I64(value),
                        _ => unreachable!(),
                    });
                }
            }
        }
        let integer_range = match (function, values.get(1), values.get(2)) {
            (
                BuiltinFn::RangeClamp | BuiltinFn::RangeWrap,
                Some(Value::Constant(min @ (ScalarValue::I32(_) | ScalarValue::I64(_)))),
                Some(Value::Constant(max @ (ScalarValue::I32(_) | ScalarValue::I64(_)))),
            ) if min.ty() == max.ty() => Some(onda_mir::IntegerRangeInvariant {
                min: *min,
                max: *max,
                mode: if function == BuiltinFn::RangeWrap {
                    onda_mir::IntegerRangeMode::Wrap
                } else {
                    onda_mir::IntegerRangeMode::Clamp
                },
            }),
            _ => None,
        };
        let result = self.emit_temp(
            block,
            result_ty,
            Rvalue::Intrinsic {
                intrinsic: map_intrinsic(function),
                args: values,
            },
            location,
        );
        if let Some(range) = integer_range {
            let Value::Local(local) = result.value else {
                unreachable!("emitted intrinsic result is always a local")
            };
            self.locals[local.index()].integer_range = Some(range);
        }
        Ok(result)
    }
}
