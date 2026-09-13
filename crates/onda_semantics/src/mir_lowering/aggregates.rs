use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn lower_nested_proc_element_alias(
        &mut self,
        alias: &str,
        base: &str,
        index: &Expr,
        access: IndexAccess,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let Some((owner, array)) = self.nested_proc_array_source_any(base) else {
            return Ok(false);
        };
        if self.bindings.contains_key(alias) || self.nested_proc_aliases.contains_key(alias) {
            return Err(self.error(
                format!(
                    "nested processor element alias '{alias}' conflicts with an existing binding"
                ),
                statement_location,
            ));
        }
        if array.slots.is_empty() || array.slots.len() > i32::MAX as usize {
            return Err(self.error(
                format!(
                    "nested processor array '{}.{}' has an invalid semantic length {}",
                    array.owner_struct,
                    array.field_name,
                    array.slots.len()
                ),
                statement_location,
            ));
        }
        let raw_index = self.lower_expr(index, block)?;
        let raw_index = self.coerce(raw_index, PrimitiveType::I32, block, index.loc())?;
        let normalized = self.materialize_index_to_length(
            raw_index.value,
            Value::Constant(ScalarValue::I32(array.slots.len() as i32)),
            access,
            block,
            statement_location,
        );
        let shapes = self.struct_field_shapes(&array.proc_name, statement_location)?;
        let alternatives = array
            .slots
            .iter()
            .map(|slot| {
                self.nested_proc_slot_call_arguments(
                    &owner,
                    slot,
                    &shapes,
                    block,
                    statement_location,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.nested_proc_aliases.insert(
            alias.to_owned(),
            NestedProcElementAlias {
                struct_name: array.proc_name,
                index: normalized,
                alternatives,
            },
        );
        Ok(true)
    }

    pub(super) fn lower_struct_array_element_alias(
        &mut self,
        alias: &str,
        expression: &Expr,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        self.lower_struct_array_element_alias_impl(
            alias,
            expression,
            None,
            block,
            statement_location,
        )
    }

    pub(super) fn lower_struct_array_field_alias(
        &mut self,
        alias: &str,
        expression: &Expr,
        field: &str,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        self.lower_struct_array_element_alias_impl(
            alias,
            expression,
            Some(field),
            block,
            statement_location,
        )
    }

    fn lower_struct_array_element_alias_impl(
        &mut self,
        alias: &str,
        expression: &Expr,
        only_field: Option<&str>,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let Some(source) = indexed_read_source(expression) else {
            return Ok(false);
        };
        let base = source.base;
        let index = source.index;

        self.prepare_data_array_view(base, block, expression.loc())?;

        let has_direct_source = matches!(
            self.bindings.get(base),
            Some(
                Binding::StructArrayParameter { .. }
                    | Binding::StructArrayStorage { .. }
                    | Binding::ProcArrayParameter { .. }
            )
        ) || self
            .runtime_globals
            .is_some_and(|globals| globals.array_struct_roots.contains_key(base));
        if !has_direct_source {
            return self.lower_nested_proc_element_alias(
                alias,
                base,
                index,
                source.access,
                block,
                statement_location,
            );
        }

        let (struct_name, length, parameter_fields, runtime_root) =
            if let Some(Binding::StructArrayParameter {
                struct_name,
                length,
                fields,
            }) = self.bindings.get(base).cloned()
            {
                let length = if source.access == IndexAccess::Clamp {
                    self.struct_array_length_value(length, block, expression.loc())
                } else {
                    Value::Constant(ScalarValue::I32(0))
                };
                (struct_name, length, Some(fields), None)
            } else if let Some(Binding::ProcArrayParameter {
                proc_name,
                length,
                fields,
                ..
            }) = self.bindings.get(base).cloned()
            {
                let length = if source.access == IndexAccess::Clamp {
                    self.emit_temp(
                        block,
                        PrimitiveType::I32,
                        Rvalue::Load(Place {
                            base: PlaceBase::Parameter(length),
                            projections: Vec::new(),
                        }),
                        expression.loc(),
                    )
                    .value
                } else {
                    Value::Constant(ScalarValue::I32(0))
                };
                (proc_name, length, Some(fields), None)
            } else if let Some(Binding::StructArrayStorage { struct_name, len }) =
                self.bindings.get(base).cloned()
            {
                (
                    struct_name,
                    Value::Constant(ScalarValue::I32(len as i32)),
                    None,
                    Some(base.to_owned()),
                )
            } else {
                let Some((struct_name, len)) = self
                    .runtime_globals
                    .and_then(|globals| globals.array_struct_roots.get(base).cloned())
                else {
                    return Ok(false);
                };
                (
                    struct_name,
                    Value::Constant(ScalarValue::I32(len as i32)),
                    None,
                    Some(base.to_owned()),
                )
            };

        if let Some(existing) = self.bindings.get(alias) {
            match existing {
                Binding::StructView {
                    struct_name: existing,
                } if existing == &struct_name => {}
                _ => {
                    return Err(self.error(
                        format!(
                            "struct-array element alias '{alias}' conflicts with an existing binding"
                        ),
                        statement_location,
                    ));
                }
            }
        }
        let raw_index = self.lower_expr(index, block)?;
        let raw_index = self.coerce(raw_index, PrimitiveType::I32, block, index.loc())?;
        let normalized = self.materialize_index_to_length(
            raw_index.value,
            length,
            source.access,
            block,
            statement_location,
        );

        let shapes = self.struct_field_shapes(&struct_name, expression.loc())?;
        let mut selected_field = false;
        for shape in shapes {
            let scalar = matches!(shape, StructFieldShape::Scalar { .. });
            let (field_name, element, width) = match shape {
                StructFieldShape::Scalar { name, ty } => (name, ty, 1),
                StructFieldShape::Array { name, element, len } => (name, element, len),
            };
            if only_field.is_some_and(|only| only != field_name) {
                continue;
            }
            selected_field = true;
            let slice = if let Some(fields) = &parameter_fields {
                let (_, local, actual_element) = fields
                    .iter()
                    .find(|(candidate, _, _)| *candidate == field_name)
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "struct-array element alias '{alias}' is missing field '{field_name}'"
                            ),
                            expression.loc(),
                        )
                    })?;
                if *actual_element != element {
                    return Err(self.error(
                        format!(
                            "struct-array element alias '{alias}.{field_name}' changed element type"
                        ),
                        expression.loc(),
                    ));
                }
                *local
            } else {
                let root = runtime_root
                    .as_ref()
                    .expect("runtime struct-array alias has a state root");
                let flat_name = format!("{root}.{field_name}");
                let slice = self.lower_named_slice(
                    &flat_name,
                    SliceSelection::default(),
                    Some(onda_mir::AccessMode::ReadWrite),
                    block,
                    expression.loc(),
                )?;
                let Value::Local(local) = slice.value else {
                    unreachable!("slice construction always produces a local")
                };
                local
            };
            let access = self.slice_access(slice);
            let binding_name = format!("{alias}.{field_name}");
            let start = if scalar {
                Value::Local(normalized)
            } else {
                self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::Binary {
                        op: MirBinaryOp::Multiply,
                        lhs: Value::Local(normalized),
                        rhs: Value::Constant(ScalarValue::I32(width as i32)),
                    },
                    expression.loc(),
                )
                .value
            };
            let window = self.emit_slice_temp(
                block,
                Some(binding_name.clone()),
                element,
                access,
                Rvalue::MakeSlice {
                    source: onda_mir::SliceSource::Place(Place::local(slice)),
                    start,
                    len: Value::Constant(ScalarValue::I32(width as i32)),
                    bounds: BoundsMode::Unchecked,
                    access,
                },
                expression.loc(),
            );
            let Value::Local(window) = window.value else {
                unreachable!("slice construction always produces a local")
            };
            if scalar {
                self.bindings.insert(
                    binding_name,
                    Binding::SliceElementAlias {
                        slice: window,
                        element,
                        index: Value::Constant(ScalarValue::I32(0)),
                    },
                );
            } else {
                self.bindings.insert(
                    binding_name,
                    Binding::Slice(window, element, access, Some(width)),
                );
            }
        }

        if let Some(field) = only_field {
            if !selected_field {
                return Err(self.error(
                    format!("struct-array element has no canonical field '{field}'"),
                    expression.loc(),
                ));
            }
            return Ok(true);
        }

        if let Some(declarations) = self.structs.get(&struct_name).cloned() {
            for field in &declarations {
                let TypedFieldType::Tuple(types) = &field.ty else {
                    continue;
                };
                let mut components = Vec::with_capacity(types.len());
                for (index, ty) in types.iter().copied().enumerate() {
                    let component_name = format!("{alias}.{}.__{index}", field.name);
                    let Some(Binding::SliceElementAlias {
                        slice,
                        element,
                        index: element_index,
                    }) = self.bindings.get(&component_name).cloned()
                    else {
                        return Err(self.error(
                            format!(
                                "struct-array tuple alias '{alias}.{}' is missing component {index}",
                                field.name
                            ),
                            expression.loc(),
                        ));
                    };
                    if element != ty {
                        return Err(self.error(
                            format!(
                                "struct-array tuple alias '{alias}.{}' component {index} changed type",
                                field.name
                            ),
                            expression.loc(),
                        ));
                    }
                    components.push((slice, ty, element_index));
                }
                self.bindings.insert(
                    format!("{alias}.{}", field.name),
                    Binding::TupleSliceElementAlias(components),
                );
            }
            for field in &declarations {
                let TypedFieldType::Struct = field.ty else {
                    continue;
                };
                let Some(nested_struct) = &field.struct_name else {
                    continue;
                };
                self.bindings.insert(
                    format!("{alias}.{}", field.name),
                    Binding::StructView {
                        struct_name: nested_struct.clone(),
                    },
                );
            }
        }

        for embedded in self.embedded_struct_array_shapes(&struct_name, expression.loc())? {
            let binding_name = format!("{alias}.{}", embedded.path);
            let mut fields = Vec::with_capacity(embedded.fields.len());
            for field in embedded.fields {
                let source_name = format!("{alias}.{}", field.outer_name);
                let Some(Binding::Slice(slice, element, _, _)) =
                    self.bindings.get(&source_name).cloned()
                else {
                    return Err(self.error(
                        format!(
                            "embedded aggregate array alias '{binding_name}' is missing canonical leaf '{source_name}'"
                        ),
                        expression.loc(),
                    ));
                };
                if element != field.element {
                    return Err(self.error(
                        format!(
                            "embedded aggregate array alias '{binding_name}' leaf '{}' changed type or access",
                            field.inner_name
                        ),
                        expression.loc(),
                    ));
                }
                fields.push((field.inner_name, slice, field.element));
            }
            self.bindings.insert(
                binding_name,
                Binding::StructArrayParameter {
                    struct_name: embedded.struct_name,
                    length: StructArrayLength::Fixed(embedded.len),
                    fields,
                },
            );
        }
        self.bindings
            .insert(alias.to_owned(), Binding::StructView { struct_name });
        Ok(true)
    }

    pub(super) fn materialize_index_to_length(
        &mut self,
        value: Value,
        length: Value,
        access: IndexAccess,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> LocalId {
        if access == IndexAccess::Clamp {
            if let Value::Constant(ScalarValue::I32(length)) = length {
                if length > 0 {
                    return self.clamp_index_to_inclusive_upper(
                        value,
                        Value::Constant(ScalarValue::I32(length - 1)),
                        block,
                        location,
                    );
                }
            }
        }
        let value = match access {
            IndexAccess::Clamp => Rvalue::NormalizeIndex {
                index: value,
                length,
                bounds: BoundsMode::Clamp,
            },
            IndexAccess::Unchecked => Rvalue::Use(value),
        };
        let index = self.emit_temp(block, PrimitiveType::I32, value, location);
        let Value::Local(index) = index.value else {
            unreachable!("normalized index is always a local")
        };
        index
    }

    pub(super) fn clamp_index_to_inclusive_upper(
        &mut self,
        value: Value,
        upper: Value,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> LocalId {
        let normalized = self.emit_temp(
            block,
            PrimitiveType::I32,
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::RangeClamp,
                args: vec![value, Value::Constant(ScalarValue::I32(0)), upper],
            },
            location,
        );
        let Value::Local(normalized) = normalized.value else {
            unreachable!("emitted index clamp result is always a local")
        };
        normalized
    }

    pub(super) fn lower_state_array_initializer(
        &mut self,
        name: &str,
        expression: &Expr,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let state_array = self
            .runtime_globals
            .and_then(|globals| globals.state_arrays.get(name).copied());
        let Some((state, ty, len)) = state_array else {
            return Ok(false);
        };
        let values = match expression {
            Expr::ArrayLiteral { values, .. } => Some(values.as_slice()),
            Expr::ArrayCtor { init, .. } => init.as_deref(),
            _ => return Ok(false),
        };

        if let Some(values) = values {
            if values.len() != len as usize {
                return Err(self.error(
                    format!(
                        "state array '{name}' initializer expected {len} elements, got {}",
                        values.len()
                    ),
                    statement_location,
                ));
            }
            for (index, expression) in values.iter().enumerate() {
                let value = self.lower_expr(expression, block)?;
                let value = self.coerce(value, ty, block, expression.loc())?;
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::State(state),
                            projections: vec![Projection::Index {
                                index: Value::Constant(ScalarValue::I32(index as i32)),
                                bounds: BoundsMode::Unchecked,
                            }],
                        },
                        value: Rvalue::Use(value.value),
                    },
                    statement_location,
                );
            }
        } else {
            self.emit_state_array_fill(state, ty, len, block, statement_location);
        }
        Ok(true)
    }

    pub(super) fn lower_local_array_declaration(
        &mut self,
        name: &str,
        expression: &Expr,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let (element, len, values) = match expression {
            Expr::ArrayLiteral { values, .. } => {
                let Some(first) = values.first() else {
                    return Err(self.error(
                        format!("local array '{name}' cannot be empty"),
                        statement_location,
                    ));
                };
                let mut lowered = Vec::with_capacity(values.len());
                for value in values {
                    lowered.push(self.lower_expr(value, block)?);
                }
                let element = effective_untyped_assignment_type(first, Some(lowered[0].ty))
                    .unwrap_or(lowered[0].ty);
                (element, values.len(), Some(lowered))
            }
            Expr::ArrayCtor { spec, init, .. } => {
                let ArrayElemType::Primitive(element) = spec.elem else {
                    return Ok(false);
                };
                let mut diagnostics = Vec::new();
                let raw_len = eval_const_expr_i64_exact(
                    &spec.size,
                    AnalysisOptions {
                        sample_rate: self.config.sample_rate,
                        block_size: self.config.block_size as usize,
                    },
                    "local array length during MIR lowering",
                    &mut diagnostics,
                )
                .ok_or_else(|| {
                    self.error(
                        diagnostics
                            .first()
                            .map(|diagnostic| diagnostic.message.clone())
                            .unwrap_or_else(|| {
                                format!("local array '{name}' length is not constant")
                            }),
                        spec.size.loc(),
                    )
                })?;
                let len = usize::try_from(raw_len)
                    .ok()
                    .filter(|len| *len > 0 && *len <= i32::MAX as usize)
                    .ok_or_else(|| {
                        self.error(
                            format!("local array '{name}' length must be between 1 and i32::MAX"),
                            spec.size.loc(),
                        )
                    })?;
                let lowered = if let Some(values) = init {
                    if values.len() != len {
                        return Err(self.error(
                            format!(
                                "local array '{name}' initializer expected {len} elements, got {}",
                                values.len()
                            ),
                            statement_location,
                        ));
                    }
                    let mut lowered = Vec::with_capacity(values.len());
                    for value in values {
                        lowered.push(self.lower_expr(value, block)?);
                    }
                    Some(lowered)
                } else {
                    None
                };
                (element, len, lowered)
            }
            _ => return Ok(false),
        };

        let len_u32 = u32::try_from(len).map_err(|_| {
            self.error(
                format!("local array '{name}' length does not fit u32"),
                statement_location,
            )
        })?;
        if self.bindings.contains_key(name) {
            return Err(self.error(
                format!("local array declaration '{name}' conflicts with an existing binding"),
                statement_location,
            ));
        }
        let local = self.new_array_local(Some(name.to_owned()), element, len_u32);
        self.bindings
            .insert(name.to_owned(), Binding::Array(local, element, len_u32));

        let initialize = !matches!(
            expression,
            Expr::ArrayCtor {
                initialize: false,
                ..
            }
        );
        if !initialize {
            return Ok(true);
        }
        let Some(values) = values else {
            self.emit_array_value_fill(
                Place::local(local),
                element,
                Value::Constant(zero_scalar(element)),
                len_u32,
                block,
                statement_location,
            );
            return Ok(true);
        };
        for (index, value) in values.into_iter().enumerate() {
            let value = self.coerce(value, element, block, expression.loc())?.value;
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
                statement_location,
            );
        }
        Ok(true)
    }

    pub(super) fn emit_state_array_fill(
        &mut self,
        state: onda_mir::StateId,
        ty: PrimitiveType,
        len: u32,
        block: &mut MirBlock,
        location: SourceLoc,
    ) {
        self.emit_state_array_value_fill(
            state,
            ty,
            Value::Constant(zero_scalar(ty)),
            len,
            block,
            location,
        );
    }

    pub(super) fn emit_state_array_value_fill(
        &mut self,
        state: onda_mir::StateId,
        ty: PrimitiveType,
        value: Value,
        len: u32,
        block: &mut MirBlock,
        location: SourceLoc,
    ) {
        self.emit_array_value_fill(
            Place {
                base: PlaceBase::State(state),
                projections: Vec::new(),
            },
            ty,
            value,
            len,
            block,
            location,
        );
    }

    pub(super) fn emit_array_value_fill(
        &mut self,
        place: Place,
        ty: PrimitiveType,
        value: Value,
        len: u32,
        block: &mut MirBlock,
        location: SourceLoc,
    ) {
        let destination = self.emit_slice_temp(
            block,
            None,
            ty,
            onda_mir::AccessMode::ReadWrite,
            Rvalue::MakeSlice {
                source: onda_mir::SliceSource::Place(place),
                start: Value::Constant(ScalarValue::I32(0)),
                len: Value::Constant(ScalarValue::I32(len as i32)),
                bounds: BoundsMode::Unchecked,
                access: onda_mir::AccessMode::ReadWrite,
            },
            location,
        );
        self.push_statement(
            block,
            StatementKind::SliceFill {
                destination: destination.value,
                value,
            },
            location,
        );
    }

    pub(super) fn assign_variable_values(
        &mut self,
        name: &str,
        mut values: Vec<LoweredValue>,
        declared_ty: Option<&onda_frontend::DeclType>,
        expression: &Expr,
        block: &mut MirBlock,
        statement_location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if let Some(components) = self.data_tuple_components(name) {
            if components.len() != values.len() {
                return Err(self.error("tuple field assignment changed arity", statement_location));
            }
            for (name, value) in components.iter().zip(values) {
                self.store_data_scalar(name, value, block, statement_location)?;
            }
            return Ok(());
        }
        if values.len() == 1 {
            values[0] =
                self.normalize_data_scalar_store(name, values[0], block, statement_location)?;
            if let Some(Binding::PlaceAlias(place, ty)) = self.bindings.get(name).cloned() {
                let value = self.coerce(values[0], ty, block, expression.loc())?;
                self.assign_place_value(block, place, value.value, statement_location);
                return Ok(());
            }
            if let Some(Binding::SliceElementAlias {
                slice,
                element,
                index,
            }) = self.bindings.get(name).cloned()
            {
                let value = self.coerce(values[0], element, block, expression.loc())?;
                self.push_statement(
                    block,
                    StatementKind::SliceStore {
                        slice: Value::Local(slice),
                        index,
                        value: value.value,
                        bounds: BoundsMode::Unchecked,
                    },
                    statement_location,
                );
                return Ok(());
            }
            if let Some(Binding::ReferenceParameter(parameter, target_ty)) =
                self.bindings.get(name).cloned()
            {
                let value = self.coerce(values[0], target_ty, block, expression.loc())?;
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
            if matches!(
                self.bindings.get(name),
                Some(Binding::Tuple(_) | Binding::StructParameter { .. })
            ) {
                return Err(self.error(
                    format!("cannot assign a scalar value to tuple local '{name}'"),
                    statement_location,
                ));
            }
            let inferred_ty = declared_ty
                .and_then(onda_frontend::DeclType::scalar)
                .or_else(|| effective_untyped_assignment_type(expression, Some(values[0].ty)))
                .unwrap_or(PrimitiveType::F32);
            let (local, target_ty) = self.scalar_local(name, inferred_ty, statement_location)?;
            let value = self.coerce(values[0], target_ty, block, expression.loc())?;
            self.assign_value(block, local, value.value, statement_location);
            return Ok(());
        }

        if let Some(Binding::TupleReferenceParameter(components)) = self.bindings.get(name).cloned()
        {
            if values.len() != components.len() {
                return Err(self.error(
                    format!(
                        "tuple field '{name}' expected {} values, got {}",
                        components.len(),
                        values.len()
                    ),
                    statement_location,
                ));
            }
            for (value, (parameter, ty)) in values.into_iter().zip(components) {
                let value = self.coerce(value, ty, block, expression.loc())?;
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
            }
            return Ok(());
        }

        let source_types = values.iter().map(|value| value.ty).collect::<Vec<_>>();
        let target_types = declared_ty
            .and_then(onda_frontend::DeclType::tuple)
            .unwrap_or(&source_types);
        if values.len() != target_types.len() {
            return Err(self.error(
                format!(
                    "tuple local '{name}' expected {} values, got {}",
                    target_types.len(),
                    values.len()
                ),
                statement_location,
            ));
        }
        let components = self.tuple_local(name, target_types, statement_location)?;
        for (value, (local, target_ty)) in values.into_iter().zip(components) {
            let value = self.coerce(value, target_ty, block, expression.loc())?;
            self.assign_value(block, local, value.value, statement_location);
        }
        Ok(())
    }

    pub(super) fn assign_runtime_global(
        &mut self,
        name: &str,
        values: &[LoweredValue],
        block: &mut MirBlock,
        value_location: SourceLoc,
        statement_location: SourceLoc,
    ) -> Result<bool, MirLoweringError> {
        let Some(globals) = self.runtime_globals else {
            return Ok(false);
        };
        if self.bindings.contains_key(name) {
            return Ok(false);
        }
        if let Some(components) = globals.state_tuples.get(name).cloned() {
            if values.len() != components.len() {
                return Err(self.error(
                    format!(
                        "tuple state '{name}' expected {} components, got {}",
                        components.len(),
                        values.len()
                    ),
                    statement_location,
                ));
            }
            for (value, (state, ty)) in values.iter().copied().zip(components) {
                let value = self.coerce(value, ty, block, value_location)?;
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::State(state),
                            projections: Vec::new(),
                        },
                        value: Rvalue::Use(value.value),
                    },
                    statement_location,
                );
            }
            return Ok(true);
        }
        if globals.state_arrays.contains_key(name) {
            return Err(self.error(
                format!(
                    "whole-array assignment to state '{name}' is only valid for its init declaration"
                ),
                statement_location,
            ));
        }
        if self.const_arrays.contains_key(name) {
            return Err(self.error(
                format!("assignment to constant array '{name}' reached MIR lowering"),
                statement_location,
            ));
        }
        if let Some((local, ty)) = self.audio_output_caches.get(name).copied() {
            let value = self.single_global_value(name, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            self.assign_value(block, local, value.value, statement_location);
            return Ok(true);
        }
        if let Some((output, ty)) = globals.control_outputs.get(name).copied() {
            let value = self.single_global_value(name, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            self.push_statement(
                block,
                StatementKind::ControlOutputStore {
                    output,
                    element: None,
                    bounds: BoundsMode::Unchecked,
                    value: value.value,
                },
                statement_location,
            );
            return Ok(true);
        }
        if let Some((output, ty)) = globals.outputs.get(name).copied() {
            let frame = self.current_frame.ok_or_else(|| {
                self.error(
                    format!("audio output '{name}' was written outside the sample section"),
                    statement_location,
                )
            })?;
            let value = self.single_global_value(name, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            self.push_statement(
                block,
                StatementKind::OutputStore {
                    output,
                    element: None,
                    bounds: BoundsMode::Unchecked,
                    frame,
                    value: value.value,
                },
                statement_location,
            );
            return Ok(true);
        }
        if let Some((state, ty)) = globals.states.get(name).copied() {
            let value = self.single_global_value(name, values, statement_location)?;
            let value = self.coerce(value, ty, block, value_location)?;
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    },
                    value: Rvalue::Use(value.value),
                },
                statement_location,
            );
            return Ok(true);
        }
        if globals.inputs.contains_key(name) {
            return Err(self.error(
                format!("assignment to audio input '{name}' reached MIR lowering"),
                statement_location,
            ));
        }
        if globals.params.contains_key(name) {
            return Err(self.error(
                format!("assignment to parameter '{name}' reached MIR lowering"),
                statement_location,
            ));
        }
        Ok(false)
    }
}
