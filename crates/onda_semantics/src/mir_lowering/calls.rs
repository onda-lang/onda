use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn owner_struct_name(&self, root: &str) -> Option<&str> {
        match self.bindings.get(root) {
            Some(Binding::StructParameter { struct_name, .. })
            | Some(Binding::StructArrayElementAlias { struct_name }) => Some(struct_name.as_str()),
            _ => self
                .runtime_globals
                .and_then(|globals| globals.struct_roots.get(root))
                .map(String::as_str),
        }
    }

    pub(super) fn nested_proc_array_source(
        &self,
        base: &str,
        expected_proc: &str,
    ) -> Option<(String, TypedNestedProcArray)> {
        self.nested_proc_array_source_any(base)
            .filter(|(_, array)| array.proc_name == expected_proc)
    }

    pub(super) fn nested_proc_array_source_any(
        &self,
        base: &str,
    ) -> Option<(String, TypedNestedProcArray)> {
        let explicit = base.rsplit_once('.');
        self.nested_proc_arrays.iter().find_map(|array| {
            let owner = if let Some((owner, field)) = explicit {
                (field == array.field_name).then_some(owner)?
            } else {
                (base == array.field_name).then_some("self")?
            };
            (self.owner_struct_name(owner) == Some(array.owner_struct.as_str()))
                .then(|| (owner.to_owned(), array.clone()))
        })
    }

    pub(super) fn nested_proc_slot_call_arguments(
        &self,
        owner: &str,
        slot: &str,
        shapes: &[StructFieldShape],
        location: SourceLoc,
    ) -> Result<Vec<CallArgument>, MirLoweringError> {
        let physical_name = |field_name: &str| format!("{slot}__{field_name}");
        if let Some(Binding::StructParameter { fields, .. }) = self.bindings.get(owner) {
            return shapes
                .iter()
                .map(|shape| match shape {
                    StructFieldShape::Scalar { name, ty } => {
                        let physical = physical_name(name);
                        let Some(StructFieldReference::Scalar {
                            parameter,
                            ty: actual,
                            ..
                        }) = fields.iter().find(|field| {
                            matches!(
                                field,
                                StructFieldReference::Scalar { name, .. } if *name == physical
                            )
                        })
                        else {
                            return Err(self.error(
                                format!(
                                    "nested processor slot '{owner}.{slot}' is missing scalar field '{physical}'"
                                ),
                                location,
                            ));
                        };
                        if actual != ty {
                            return Err(self.error(
                                format!(
                                    "nested processor slot field '{owner}.{physical}' changed type"
                                ),
                                location,
                            ));
                        }
                        Ok(CallArgument::Place(Place {
                            base: PlaceBase::Parameter(*parameter),
                            projections: Vec::new(),
                        }))
                    }
                    StructFieldShape::Array { name, element, len } => {
                        let physical = physical_name(name);
                        let Some(StructFieldReference::Array {
                            parameter,
                            element: actual_element,
                            len: actual_len,
                            ..
                        }) = fields.iter().find(|field| {
                            matches!(
                                field,
                                StructFieldReference::Array { name, .. } if *name == physical
                            )
                        })
                        else {
                            return Err(self.error(
                                format!(
                                    "nested processor slot '{owner}.{slot}' is missing array field '{physical}'"
                                ),
                                location,
                            ));
                        };
                        if actual_element != element || actual_len != len {
                            return Err(self.error(
                                format!(
                                    "nested processor slot array field '{owner}.{physical}' changed shape"
                                ),
                                location,
                            ));
                        }
                        Ok(CallArgument::Place(Place {
                            base: PlaceBase::Parameter(*parameter),
                            projections: Vec::new(),
                        }))
                    }
                })
                .collect();
        }

        if matches!(
            self.bindings.get(owner),
            Some(Binding::StructArrayElementAlias { .. })
        ) {
            return shapes
                .iter()
                .map(|shape| match shape {
                    StructFieldShape::Scalar { name, ty } => {
                        let physical = format!("{owner}.{}", physical_name(name));
                        let Some(Binding::SliceElementAlias {
                            slice,
                            element,
                            index,
                        }) = self.bindings.get(&physical)
                        else {
                            return Err(self.error(
                                format!(
                                    "nested processor element alias '{owner}' is missing scalar field '{physical}'"
                                ),
                                location,
                            ));
                        };
                        if element != ty {
                            return Err(self.error(
                                format!("nested processor alias field '{physical}' changed type"),
                                location,
                            ));
                        }
                        Ok(CallArgument::SliceElement {
                            slice: Value::Local(*slice),
                            index: Value::Local(*index),
                            bounds: BoundsMode::Unchecked,
                        })
                    }
                    StructFieldShape::Array { name, element, .. } => {
                        let physical = format!("{owner}.{}", physical_name(name));
                        let Some(Binding::Slice(slice, actual, access)) =
                            self.bindings.get(&physical)
                        else {
                            return Err(self.error(
                                format!(
                                    "nested processor element alias '{owner}' is missing array field '{physical}'"
                                ),
                                location,
                            ));
                        };
                        if actual != element || *access != onda_mir::AccessMode::ReadWrite {
                            return Err(self.error(
                                format!("nested processor alias array '{physical}' changed type"),
                                location,
                            ));
                        }
                        Ok(CallArgument::SliceWindow {
                            slice: Value::Local(*slice),
                            start: Value::Constant(ScalarValue::I32(0)),
                            bounds: BoundsMode::Unchecked,
                        })
                    }
                })
                .collect();
        }

        let Some(globals) = self.runtime_globals else {
            return Err(self.error(
                format!("cannot resolve nested processor slot '{owner}.{slot}'"),
                location,
            ));
        };
        shapes
            .iter()
            .map(|shape| match shape {
                StructFieldShape::Scalar { name, ty } => {
                    let physical = format!("{owner}.{}", physical_name(name));
                    let Some((state, actual)) = globals.states.get(&physical).copied() else {
                        return Err(self.error(
                            format!(
                                "nested processor slot '{owner}.{slot}' has no scalar state field '{physical}'"
                            ),
                            location,
                        ));
                    };
                    if actual != *ty {
                        return Err(self.error(
                            format!("nested processor state field '{physical}' changed type"),
                            location,
                        ));
                    }
                    Ok(CallArgument::Place(Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    }))
                }
                StructFieldShape::Array { name, element, len } => {
                    let physical = format!("{owner}.{}", physical_name(name));
                    let Some((state, actual_element, actual_len)) =
                        globals.state_arrays.get(&physical).copied()
                    else {
                        return Err(self.error(
                            format!(
                                "nested processor slot '{owner}.{slot}' has no array state field '{physical}'"
                            ),
                            location,
                        ));
                    };
                    if actual_element != *element || actual_len != *len {
                        return Err(self.error(
                            format!("nested processor state array '{physical}' changed shape"),
                            location,
                        ));
                    }
                    Ok(CallArgument::Place(Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    }))
                }
            })
            .collect()
    }

    pub(super) fn lower_nested_proc_array_argument(
        &mut self,
        expected_struct: &str,
        base: &str,
        index: &Expr,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Option<LoweredIndexedStructArgument>, MirLoweringError> {
        let Some((owner, array)) = self.nested_proc_array_source(base, expected_struct) else {
            return Ok(None);
        };
        if array.slots.is_empty() || array.slots.len() > i32::MAX as usize {
            return Err(self.error(
                format!(
                    "nested processor array '{}.{}' has an invalid semantic length {}",
                    array.owner_struct,
                    array.field_name,
                    array.slots.len()
                ),
                location,
            ));
        }
        let raw_index = self.lower_expr(index, block)?;
        let raw_index = self.coerce(raw_index, PrimitiveType::I32, block, index.loc())?;
        let normalized = self.clamp_index_to_length(
            raw_index.value,
            Value::Constant(ScalarValue::I32(array.slots.len() as i32)),
            block,
            location,
        );
        let shapes = self.struct_field_shapes(expected_struct, location)?;
        let alternatives = array
            .slots
            .iter()
            .map(|slot| self.nested_proc_slot_call_arguments(&owner, slot, &shapes, location))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(LoweredIndexedStructArgument::Dispatch {
            index: normalized,
            alternatives,
        }))
    }

    pub(super) fn named_proc_slot_index(
        &self,
        name: &str,
        expected_struct: &str,
    ) -> Option<(String, usize)> {
        if let Some(globals) = self.runtime_globals {
            for (root, (struct_name, len)) in &globals.array_struct_roots {
                if struct_name != expected_struct {
                    continue;
                }
                for index in 0..*len as usize {
                    if name == format!("{root}[{index}]") {
                        return Some((root.clone(), index));
                    }
                }
            }
        }
        for array in self
            .nested_proc_arrays
            .iter()
            .filter(|array| array.proc_name == expected_struct)
        {
            for (index, slot) in array.slots.iter().enumerate() {
                if name == slot
                    && self.owner_struct_name("self") == Some(array.owner_struct.as_str())
                {
                    return Some((array.field_name.clone(), index));
                }
                let suffix = format!(".{slot}");
                let Some(owner) = name.strip_suffix(&suffix) else {
                    continue;
                };
                if self.owner_struct_name(owner) == Some(array.owner_struct.as_str()) {
                    return Some((format!("{owner}.{}", array.field_name), index));
                }
            }
        }
        None
    }

    /// Resolves `array[index]` as an addressable data-struct value at the MIR
    /// boundary. Semantic analysis has already established the nominal struct
    /// type and flattened structure-of-arrays layout; this step only snapshots
    /// and clamps the index once, then packages each typed leaf as a resolved
    /// state place or slice address.
    pub(super) fn lower_indexed_struct_argument(
        &mut self,
        expected_struct: &str,
        expression: &Expr,
        block: &mut MirBlock,
    ) -> Result<Option<LoweredIndexedStructArgument>, MirLoweringError> {
        let synthesized;
        let (base, index) = match expression {
            Expr::Index { base, index, .. } => (base.as_str(), index.as_ref()),
            Expr::Var { name, .. } => {
                let Some((base, index)) = self.named_proc_slot_index(name, expected_struct) else {
                    return Ok(None);
                };
                synthesized = (base, Expr::int(index as i64));
                (synthesized.0.as_str(), &synthesized.1)
            }
            _ => return Ok(None),
        };

        let shapes = self.struct_field_shapes(expected_struct, expression.loc())?;
        let (actual_struct, length, static_length, parameter_fields, runtime_root) =
            if let Some(Binding::StructArrayParameter {
                struct_name,
                length,
                fields,
            }) = self.bindings.get(base).cloned()
            {
                let length = self.struct_array_length_value(length, block, expression.loc());
                (struct_name, length, None, Some(fields), None)
            } else if let Some(Binding::ProcArrayParameter {
                proc_name,
                length,
                fields,
                ..
            }) = self.bindings.get(base).cloned()
            {
                let length = self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::Load(Place {
                        base: PlaceBase::Parameter(length),
                        projections: Vec::new(),
                    }),
                    expression.loc(),
                );
                (proc_name, length.value, None, Some(fields), None)
            } else {
                let Some((struct_name, len)) = self
                    .runtime_globals
                    .and_then(|globals| globals.array_struct_roots.get(base).cloned())
                else {
                    if let Some(argument) = self.lower_nested_proc_array_argument(
                        expected_struct,
                        base,
                        index,
                        block,
                        expression.loc(),
                    )? {
                        return Ok(Some(argument));
                    }
                    return Err(self.error(
                        format!(
                        "indexed struct argument '{base}[...]' is not a resolved struct-array root"
                    ),
                        expression.loc(),
                    ));
                };
                (
                    struct_name,
                    Value::Constant(ScalarValue::I32(len as i32)),
                    Some(len),
                    None,
                    Some(base.to_owned()),
                )
            };

        if actual_struct != expected_struct {
            return Err(self.error(
                format!(
                    "indexed struct argument '{base}[...]' expected '{expected_struct}', got '{actual_struct}'"
                ),
                expression.loc(),
            ));
        }

        let raw_index = self.lower_expr(index, block)?;
        let raw_index = self.coerce(raw_index, PrimitiveType::I32, block, index.loc())?;
        let normalized =
            self.clamp_index_to_length(raw_index.value, length, block, expression.loc());

        let mut fields = Vec::with_capacity(shapes.len());
        for shape in shapes {
            let (field_name, expected_element, width) = match shape {
                StructFieldShape::Scalar { name, ty } => (name, ty, 1),
                StructFieldShape::Array { name, element, len } => (name, element, len),
            };
            let field_base = if let Some(parameter_fields) = &parameter_fields {
                let (_, slice, actual_element) = parameter_fields
                    .iter()
                    .find(|(candidate, _, _)| *candidate == field_name)
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "indexed struct-array parameter '{base}' is missing field '{field_name}'"
                            ),
                            expression.loc(),
                        )
                    })?;
                if *actual_element != expected_element {
                    return Err(self.error(
                        format!(
                            "indexed struct-array parameter '{base}' field '{field_name}' changed element type"
                        ),
                        expression.loc(),
                    ));
                }
                LoweredStructArrayFieldBase::Slice(*slice)
            } else {
                let root = runtime_root
                    .as_ref()
                    .expect("runtime struct-array argument has a state root");
                let flat_name = format!("{root}.{field_name}");
                let (state, actual_element, actual_len) = self
                    .runtime_globals
                    .and_then(|globals| globals.state_arrays.get(&flat_name).copied())
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "indexed struct-array argument '{base}' has no flattened field storage '{flat_name}'"
                            ),
                            expression.loc(),
                        )
                    })?;
                let expected_len = static_length
                    .expect("runtime struct-array argument has a static length")
                    .checked_mul(width)
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "indexed struct-array field '{flat_name}' flattened length overflows u32"
                            ),
                            expression.loc(),
                        )
                    })?;
                if actual_element != expected_element || actual_len != expected_len {
                    return Err(self.error(
                        format!(
                            "indexed struct-array field '{flat_name}' changed type or flattened length"
                        ),
                        expression.loc(),
                    ));
                }
                if width == 1 {
                    LoweredStructArrayFieldBase::State(state)
                } else {
                    let slice = self.emit_slice_temp(
                        block,
                        None,
                        expected_element,
                        onda_mir::AccessMode::ReadWrite,
                        Rvalue::MakeSlice {
                            source: onda_mir::SliceSource::Place(Place {
                                base: PlaceBase::State(state),
                                projections: Vec::new(),
                            }),
                            start: Value::Constant(ScalarValue::I32(0)),
                            len: Value::Constant(ScalarValue::I32(actual_len as i32)),
                            bounds: BoundsMode::Unchecked,
                            access: onda_mir::AccessMode::ReadWrite,
                        },
                        expression.loc(),
                    );
                    let Value::Local(slice) = slice.value else {
                        unreachable!("slice construction always produces a local")
                    };
                    LoweredStructArrayFieldBase::Slice(slice)
                }
            };
            fields.push(LoweredStructArrayField {
                base: field_base,
                width,
            });
        }

        Ok(Some(LoweredIndexedStructArgument::Direct(
            LoweredStructArrayElement {
                index: normalized,
                fields,
            },
        )))
    }

    pub(super) fn append_indexed_struct_call_arguments(
        &mut self,
        argument: LoweredStructArrayElement,
        location: SourceLoc,
        block: &mut MirBlock,
        call_args: &mut Vec<CallArgument>,
    ) {
        for field in argument.fields {
            match field.base {
                LoweredStructArrayFieldBase::State(state) => {
                    debug_assert_eq!(field.width, 1);
                    call_args.push(CallArgument::Place(Place {
                        base: PlaceBase::State(state),
                        projections: vec![Projection::Index {
                            index: Value::Local(argument.index),
                            bounds: BoundsMode::Unchecked,
                        }],
                    }));
                }
                LoweredStructArrayFieldBase::Slice(slice) => {
                    let index = if field.width == 1 {
                        Value::Local(argument.index)
                    } else {
                        self.emit_temp(
                            block,
                            PrimitiveType::I32,
                            Rvalue::Binary {
                                op: MirBinaryOp::Multiply,
                                lhs: Value::Local(argument.index),
                                rhs: Value::Constant(ScalarValue::I32(field.width as i32)),
                            },
                            location,
                        )
                        .value
                    };
                    call_args.push(if field.width == 1 {
                        CallArgument::SliceElement {
                            slice: Value::Local(slice),
                            index,
                            bounds: BoundsMode::Unchecked,
                        }
                    } else {
                        CallArgument::SliceWindow {
                            slice: Value::Local(slice),
                            start: index,
                            bounds: BoundsMode::Unchecked,
                        }
                    });
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn append_proc_array_call_arguments(
        &mut self,
        callee_name: &str,
        param_name: &str,
        expected_proc: &str,
        expected_len: usize,
        expression: &Expr,
        block: &mut MirBlock,
        call_args: &mut Vec<CallArgument>,
    ) -> Result<(), MirLoweringError> {
        let Expr::Var { name: root, .. } = expression else {
            return Err(self.error(
                format!(
                    "call to '{callee_name}' proc-array parameter '{param_name}' requires a direct array variable"
                ),
                expression.loc(),
            ));
        };
        let expected_len = u32::try_from(expected_len).map_err(|_| {
            self.error(
                format!(
                    "call to '{callee_name}' proc-array parameter '{param_name}' length does not fit u32"
                ),
                expression.loc(),
            )
        })?;
        let shapes = self.proc_array_field_shapes(expected_proc, expression.loc())?;

        if let Some(Binding::ProcArrayParameter {
            proc_name: actual_proc,
            fixed_len,
            length,
            active,
            fields,
        }) = self.bindings.get(root).cloned()
        {
            if actual_proc != expected_proc || fixed_len != expected_len {
                return Err(self.error(
                    format!(
                        "call to '{callee_name}' proc-array parameter '{param_name}' expected '{expected_proc}[{expected_len}]', got '{actual_proc}[{fixed_len}]'"
                    ),
                    expression.loc(),
                ));
            }
            let length = self.emit_temp(
                block,
                PrimitiveType::I32,
                Rvalue::Load(Place {
                    base: PlaceBase::Parameter(length),
                    projections: Vec::new(),
                }),
                expression.loc(),
            );
            call_args.push(CallArgument::Value(length.value));
            call_args.push(CallArgument::Value(Value::Local(active)));
            for shape in shapes {
                let (field_name, expected_element) = match shape {
                    StructFieldShape::Scalar { name, ty } => (name, ty),
                    StructFieldShape::Array { name, element, .. } => (name, element),
                };
                let (_, local, actual_element) = fields
                    .iter()
                    .find(|(candidate, _, _)| *candidate == field_name)
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "forwarded proc-array parameter '{root}' is missing field '{field_name}'"
                            ),
                            expression.loc(),
                        )
                    })?;
                if *actual_element != expected_element {
                    return Err(self.error(
                        format!("forwarded proc-array field '{root}.{field_name}' changed type"),
                        expression.loc(),
                    ));
                }
                call_args.push(CallArgument::Value(Value::Local(*local)));
            }
            return Ok(());
        }

        let Some(globals) = self.runtime_globals else {
            return Err(self.error(
                format!("call to '{callee_name}' cannot resolve proc-array argument '{root}'"),
                expression.loc(),
            ));
        };
        let (actual_proc, actual_len) =
            globals
                .array_struct_roots
                .get(root)
                .cloned()
                .ok_or_else(|| {
                    self.error(
                        format!(
                            "call to '{callee_name}' cannot resolve proc-array argument '{root}'"
                        ),
                        expression.loc(),
                    )
                })?;
        if actual_proc != expected_proc || actual_len != expected_len {
            return Err(self.error(
                format!(
                    "call to '{callee_name}' proc-array parameter '{param_name}' expected '{expected_proc}[{expected_len}]', got '{actual_proc}[{actual_len}]'"
                ),
                expression.loc(),
            ));
        }
        call_args.push(CallArgument::Value(Value::Constant(ScalarValue::I32(
            actual_len as i32,
        ))));

        let active_name = runtime_proc_array_active_symbol(root);
        let (_, active_element, active_len) = globals
            .state_arrays
            .get(&active_name)
            .copied()
            .ok_or_else(|| {
                self.error(
                    format!(
                        "proc-array argument '{root}' has no active-slot storage '{active_name}'"
                    ),
                    expression.loc(),
                )
            })?;
        if active_element != PrimitiveType::Bool || active_len != actual_len {
            return Err(self.error(
                format!(
                    "proc-array active-slot storage '{active_name}' must be bool[{actual_len}]"
                ),
                expression.loc(),
            ));
        }
        let active = self.lower_named_slice(
            &active_name,
            None,
            None,
            Some(onda_mir::AccessMode::ReadWrite),
            block,
            expression.loc(),
        )?;
        call_args.push(CallArgument::Value(active.value));

        for shape in shapes {
            let (field_name, expected_element, width) = match shape {
                StructFieldShape::Scalar { name, ty } => (name, ty, 1),
                StructFieldShape::Array { name, element, len } => (name, element, len),
            };
            let flat = format!("{root}.{field_name}");
            let (_, actual_element, flat_len) =
                globals.state_arrays.get(&flat).copied().ok_or_else(|| {
                    self.error(
                        format!("proc-array argument '{root}' has no flattened field '{flat}'"),
                        expression.loc(),
                    )
                })?;
            let expected_flat_len = actual_len.checked_mul(width).ok_or_else(|| {
                self.error(
                    format!("proc-array field '{flat}' flattened length overflows u32"),
                    expression.loc(),
                )
            })?;
            if actual_element != expected_element || flat_len != expected_flat_len {
                return Err(self.error(
                    format!(
                        "proc-array field '{flat}' must be {}[{expected_flat_len}]",
                        expected_element.name()
                    ),
                    expression.loc(),
                ));
            }
            let slice = self.lower_named_slice(
                &flat,
                None,
                None,
                Some(onda_mir::AccessMode::ReadWrite),
                block,
                expression.loc(),
            )?;
            call_args.push(CallArgument::Value(slice.value));
        }
        Ok(())
    }

    pub(super) fn prepare_call_argument(
        &mut self,
        callee_name: &str,
        param_name: &str,
        kind: &TypedFnParam,
        readonly_array_params: &HashSet<String>,
        expression: &Expr,
        block: &mut MirBlock,
    ) -> Result<PreparedCallArgument, MirLoweringError> {
        match kind {
            TypedFnParam::Scalar { .. } => Ok(PreparedCallArgument::Scalar(
                self.lower_expr(expression, block)?,
            )),
            TypedFnParam::Array { elem_ty } => {
                let access = if readonly_array_params.contains(param_name) {
                    onda_mir::AccessMode::ReadOnly
                } else {
                    onda_mir::AccessMode::ReadWrite
                };
                let slice = if let Some(slice) =
                    self.lower_array_value_slice(expression, *elem_ty, access, block)?
                {
                    slice
                } else {
                    self.lower_slice_expression(expression, Some(access), block)?
                };
                if slice.element != *elem_ty {
                    return Err(self.error(
                        format!(
                            "call to '{callee_name}' array parameter '{param_name}' expected {} elements, got {}",
                            elem_ty.name(),
                            slice.element.name()
                        ),
                        expression.loc(),
                    ));
                }
                Ok(PreparedCallArgument::Array(slice))
            }
            TypedFnParam::Tuple { .. } => Ok(PreparedCallArgument::Tuple(
                self.lower_value_expr(expression, block)?,
            )),
            TypedFnParam::Struct { struct_name } => {
                // Nested aliases already carry their evaluated dispatch index.
                // Do not reinterpret them as a fresh indexed access.
                if let Expr::Var { name, .. } = expression {
                    if self.nested_proc_aliases.contains_key(name) {
                        return Ok(PreparedCallArgument::DirectReference(expression.clone()));
                    }
                }
                if let Some(argument) =
                    self.lower_indexed_struct_argument(struct_name, expression, block)?
                {
                    return Ok(PreparedCallArgument::IndexedStruct(argument));
                }
                if !matches!(expression, Expr::Var { .. }) {
                    return Err(self.error(
                        format!(
                            "call to '{callee_name}' struct parameter '{param_name}' requires a struct variable or indexed struct-array element"
                        ),
                        expression.loc(),
                    ));
                }
                Ok(PreparedCallArgument::DirectReference(expression.clone()))
            }
            TypedFnParam::Buffer { .. } => {
                let side_effect_free = matches!(expression, Expr::Var { .. })
                    || matches!(
                        expression,
                        Expr::UserCall { name, .. }
                            if name == PROC_INDEX_BUFFER_SELECT_SENTINEL
                    );
                if !side_effect_free {
                    return Err(self.error(
                        format!(
                            "call to '{callee_name}' buffer parameter '{param_name}' requires a direct buffer reference"
                        ),
                        expression.loc(),
                    ));
                }
                // The selector is compiler-generated metadata. Its base/index
                // fields identify the dispatch prepared by the associated
                // indexed struct argument; buffer lowering deliberately never
                // evaluates them. Slot alternatives are direct resources.
                Ok(PreparedCallArgument::DirectReference(expression.clone()))
            }
            TypedFnParam::ProcArray { .. } | TypedFnParam::StructArray { .. } => {
                if !matches!(expression, Expr::Var { .. }) {
                    return Err(self.error(
                        format!(
                            "call to '{callee_name}' aggregate-reference parameter '{param_name}' requires a direct variable"
                        ),
                        expression.loc(),
                    ));
                }
                Ok(PreparedCallArgument::DirectReference(expression.clone()))
            }
        }
    }

    pub(super) fn lower_user_call(
        &mut self,
        name: &str,
        type_args: &[onda_frontend::CallTypeArg],
        args: &[onda_frontend::CallArg],
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<Option<Vec<LoweredValue>>, MirLoweringError> {
        if !type_args.is_empty() {
            return Err(self.error(
                format!("call to '{name}' still has unresolved type arguments"),
                location,
            ));
        }
        let Some(function_index) = self.function_indices.get(name).copied() else {
            return Err(self.error(
                format!("unresolved direct function call '{name}' reached MIR lowering"),
                location,
            ));
        };
        let callee_context = effective_call_context(
            name,
            args.first().map(|arg| &arg.expr),
            CompileContext::from_config(self.config),
            self.host_config,
            self.oversample_factors,
            self.proc_instance_oversample_factors,
        );
        let function_key = FunctionKey {
            function_index,
            context: callee_context,
        };
        let Some(function_id) = self.function_ids.get(&function_key).copied() else {
            return Err(self.error(
                format!(
                    "missing contextual specialization for call to '{name}' at sample rate {:?} and block size {}",
                    callee_context.config().sample_rate,
                    callee_context.block_size
                ),
                location,
            ));
        };
        let callee = &self.functions[function_index];
        let mut diagnostics = Vec::<Diagnostic>::new();
        let resolved = resolve_call_args_at(
            args,
            &callee.params,
            &callee.param_defaults,
            callee.params.first().map(String::as_str) == Some("self"),
            false,
            &format!("function '{name}' call"),
            location,
            &mut diagnostics,
        );
        if let Some(diagnostic) = diagnostics.into_iter().next() {
            return Err(self.error(
                format!(
                    "call argument normalization failed after semantic analysis: {}",
                    diagnostic.message
                ),
                location,
            ));
        }

        let mut ordered_args = Vec::<Expr>::with_capacity(callee.params.len());
        for index in 0..callee.params.len() {
            let expression = resolved
                .get(index)
                .and_then(|value| *value)
                .or_else(|| callee.param_defaults.get(index).and_then(Option::as_ref))
                .ok_or_else(|| {
                    self.error(
                        format!(
                            "call to '{name}' has no value for parameter '{}'",
                            callee.params[index]
                        ),
                        location,
                    )
                })?;
            ordered_args.push(expression.clone());
        }
        let param_kinds = callee.param_kinds.clone();
        let param_names = callee.params.clone();
        let readonly_array_params = callee.readonly_array_params.clone();
        let returns_value = callee.returns_value;
        let result_types = match &callee.return_ty {
            ReturnType::Scalar(ty) => vec![*ty],
            ReturnType::Tuple(types) => types.clone(),
        };

        // Argument binding determines ABI order, but it must not determine
        // expression evaluation order. Prepare every supplied argument exactly
        // once in textual source order, then evaluate omitted defaults in
        // parameter order. The prepared scalar/tuple/slice/indexed-reference
        // representation can subsequently be marshalled into ABI order without
        // invoking user code again.
        let mut source_param_indices = Vec::with_capacity(args.len());
        let mut next_positional = 0usize;
        for argument in args {
            let parameter_index = if let Some(argument_name) = argument.name.as_deref() {
                param_names
                    .iter()
                    .position(|parameter| parameter == argument_name)
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "call to '{name}' references unknown named argument '{argument_name}' after semantic analysis"
                            ),
                            argument.expr.loc(),
                        )
                    })?
            } else {
                let index = next_positional;
                next_positional += 1;
                index
            };
            source_param_indices.push(parameter_index);
        }

        let mut prepared_args = std::iter::repeat_with(|| None)
            .take(param_kinds.len())
            .collect::<Vec<Option<PreparedCallArgument>>>();
        for (argument, parameter_index) in args.iter().zip(source_param_indices) {
            let prepared = self.prepare_call_argument(
                name,
                &param_names[parameter_index],
                &param_kinds[parameter_index],
                &readonly_array_params,
                &argument.expr,
                block,
            )?;
            if prepared_args[parameter_index].replace(prepared).is_some() {
                return Err(self.error(
                    format!(
                        "call to '{name}' prepared parameter '{}' more than once after semantic argument normalization",
                        param_names[parameter_index]
                    ),
                    argument.expr.loc(),
                ));
            }
        }
        for parameter_index in 0..param_kinds.len() {
            if prepared_args[parameter_index].is_some() {
                continue;
            }
            prepared_args[parameter_index] = Some(self.prepare_call_argument(
                name,
                &param_names[parameter_index],
                &param_kinds[parameter_index],
                &readonly_array_params,
                &ordered_args[parameter_index],
                block,
            )?);
        }

        let mut call_args = Vec::with_capacity(ordered_args.len());
        let mut pending_dispatch = None::<PendingCallDispatch>;
        let mut indexed_struct_selection = None::<LocalId>;
        for (parameter_index, ((expression, kind), param_name)) in ordered_args
            .iter()
            .zip(param_kinds.iter())
            .zip(param_names.iter())
            .enumerate()
        {
            let prepared = prepared_args[parameter_index].take().ok_or_else(|| {
                self.error(
                    format!(
                        "call to '{name}' lost prepared parameter '{param_name}' before MIR ABI lowering"
                    ),
                    expression.loc(),
                )
            })?;
            match kind {
                TypedFnParam::Scalar { ty } => {
                    let param_ty = ty.unwrap_or(PrimitiveType::F32);
                    let PreparedCallArgument::Scalar(value) = prepared else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' scalar parameter '{param_name}' has a non-scalar prepared argument"
                            ),
                            expression.loc(),
                        ));
                    };
                    let inferred = effective_untyped_assignment_type(expression, Some(value.ty))
                        .unwrap_or(value.ty);
                    if ty.is_none() && inferred != PrimitiveType::F32 {
                        return Err(self.error(
                            format!(
                                "call to '{name}' from '{}' requires a {} specialization for untyped parameter '{param_name}'; only the default f32 specialization is available in this MIR slice",
                                self.emitted_name,
                                inferred.name(),
                            ),
                            expression.loc(),
                        ));
                    }
                    if !can_assign_expr_to_type(expression, value.ty, param_ty) {
                        return Err(self.error(
                            format!(
                                "call to '{name}' parameter '{param_name}' cannot implicitly convert {} to {} after semantic analysis",
                                value.ty.name(),
                                param_ty.name()
                            ),
                            expression.loc(),
                        ));
                    }
                    let value = self.coerce(value, param_ty, block, expression.loc())?;
                    call_args.push(CallArgument::Value(value.value));
                }
                TypedFnParam::Array { elem_ty } => {
                    let PreparedCallArgument::Array(slice) = prepared else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' array parameter '{param_name}' has a non-array prepared argument"
                            ),
                            expression.loc(),
                        ));
                    };
                    if slice.element != *elem_ty {
                        return Err(self.error(
                            format!(
                                "call to '{name}' array parameter '{param_name}' expected {} elements, got {}",
                                elem_ty.name(),
                                slice.element.name()
                            ),
                            expression.loc(),
                        ));
                    }
                    call_args.push(CallArgument::Value(slice.value));
                }
                TypedFnParam::Tuple { elem_tys } => {
                    let PreparedCallArgument::Tuple(values) = prepared else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' tuple parameter '{param_name}' has a non-tuple prepared argument"
                            ),
                            expression.loc(),
                        ));
                    };
                    if values.len() != elem_tys.len() {
                        return Err(self.error(
                            format!(
                                "call to '{name}' tuple parameter '{param_name}' expected {} elements, got {}",
                                elem_tys.len(),
                                values.len()
                            ),
                            expression.loc(),
                        ));
                    }
                    for (value, ty) in values.into_iter().zip(elem_tys.iter().copied()) {
                        let value = self.coerce(value, ty, block, expression.loc())?;
                        call_args.push(CallArgument::Value(value.value));
                    }
                }
                TypedFnParam::Buffer { elem_ty, .. } => {
                    let PreparedCallArgument::DirectReference(prepared_expression) = prepared
                    else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' buffer parameter '{param_name}' has a non-reference prepared argument"
                            ),
                            expression.loc(),
                        ));
                    };
                    let expression = &prepared_expression;
                    if let Expr::UserCall {
                        name: selector,
                        args: selector_args,
                        ..
                    } = expression
                    {
                        if selector == PROC_INDEX_BUFFER_SELECT_SENTINEL {
                            let mut choices = Vec::new();
                            for choice in selector_args.iter().filter(|argument| {
                                !matches!(
                                    argument.name.as_deref(),
                                    Some(PROC_INDEX_BASE_ARG | PROC_INDEX_EXPR_ARG)
                                )
                            }) {
                                let Expr::Var {
                                    name: buffer_name, ..
                                } = &choice.expr
                                else {
                                    return Err(self.error(
                                        format!(
                                            "call to '{name}' buffer selector for '{param_name}' contains a non-buffer alternative"
                                        ),
                                        choice.expr.loc(),
                                    ));
                                };
                                let argument = if let Some(Binding::BufferParameter(
                                    parameter,
                                    actual,
                                )) = self.bindings.get(buffer_name).cloned()
                                {
                                    if actual != *elem_ty {
                                        return Err(self.error(
                                            format!(
                                                "call to '{name}' buffer parameter '{param_name}' expected {} elements, got {}",
                                                elem_ty.name(),
                                                actual.name()
                                            ),
                                            choice.expr.loc(),
                                        ));
                                    }
                                    CallArgument::Place(Place {
                                        base: PlaceBase::Parameter(parameter),
                                        projections: Vec::new(),
                                    })
                                } else if let Some((buffer, actual)) = self
                                    .runtime_globals
                                    .and_then(|globals| globals.buffers.get(buffer_name).copied())
                                {
                                    if actual != *elem_ty {
                                        return Err(self.error(
                                            format!(
                                                "call to '{name}' buffer parameter '{param_name}' expected {} elements, got {}",
                                                elem_ty.name(),
                                                actual.name()
                                            ),
                                            choice.expr.loc(),
                                        ));
                                    }
                                    CallArgument::Buffer(buffer)
                                } else {
                                    return Err(self.error(
                                        format!(
                                            "call to '{name}' buffer selector for '{param_name}' references unsupported buffer '{buffer_name}'"
                                        ),
                                        choice.expr.loc(),
                                    ));
                                };
                                choices.push(argument);
                            }
                            if pending_dispatch.is_none() {
                                let Some(index) = indexed_struct_selection else {
                                    return Err(self.error(
                                        format!(
                                            "call to '{name}' has a processor-indexed buffer selector without a matching processor dispatch"
                                        ),
                                        expression.loc(),
                                    ));
                                };
                                pending_dispatch = Some(PendingCallDispatch {
                                    index,
                                    argument_start: call_args.len(),
                                    argument_len: 0,
                                    alternatives: vec![Vec::new(); choices.len()],
                                    slot_arguments: Vec::new(),
                                });
                            }
                            let dispatch = pending_dispatch
                                .as_mut()
                                .expect("processor-indexed buffer dispatch was initialized");
                            if choices.len() != dispatch.alternatives.len() {
                                return Err(self.error(
                                    format!(
                                        "call to '{name}' processor-indexed buffer selector has {} alternatives, expected {}",
                                        choices.len(),
                                        dispatch.alternatives.len()
                                    ),
                                    expression.loc(),
                                ));
                            }
                            let argument_index = call_args.len();
                            call_args.push(choices[0].clone());
                            dispatch.slot_arguments.push((argument_index, choices));
                            continue;
                        }
                    }
                    let Expr::Var {
                        name: buffer_name, ..
                    } = expression
                    else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' buffer parameter '{param_name}' requires a buffer variable"
                            ),
                            expression.loc(),
                        ));
                    };
                    if let Some(Binding::BufferParameter(parameter, actual)) =
                        self.bindings.get(buffer_name).cloned()
                    {
                        if actual != *elem_ty {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' buffer parameter '{param_name}' expected {} elements, got {}",
                                    elem_ty.name(),
                                    actual.name()
                                ),
                                expression.loc(),
                            ));
                        }
                        call_args.push(CallArgument::Place(Place {
                            base: PlaceBase::Parameter(parameter),
                            projections: Vec::new(),
                        }));
                    } else if let Some((buffer, actual)) = self
                        .runtime_globals
                        .and_then(|globals| globals.buffers.get(buffer_name).copied())
                    {
                        if actual != *elem_ty {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' buffer parameter '{param_name}' expected {} elements, got {}",
                                    elem_ty.name(),
                                    actual.name()
                                ),
                                expression.loc(),
                            ));
                        }
                        call_args.push(CallArgument::Buffer(buffer));
                    } else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' buffer parameter '{param_name}' references unsupported buffer '{buffer_name}'"
                            ),
                            expression.loc(),
                        ));
                    }
                }
                TypedFnParam::Struct { struct_name } => {
                    let (prepared_expression, indexed_argument) = match prepared {
                        PreparedCallArgument::DirectReference(expression) => (expression, None),
                        PreparedCallArgument::IndexedStruct(argument) => {
                            (expression.clone(), Some(argument))
                        }
                        _ => {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' struct parameter '{param_name}' has an invalid prepared argument"
                                ),
                                expression.loc(),
                            ));
                        }
                    };
                    let expression = &prepared_expression;
                    if let Expr::Var { name: root, .. } = expression {
                        if let Some(alias) = self.nested_proc_aliases.get(root).cloned() {
                            if alias.struct_name != *struct_name {
                                return Err(self.error(
                                    format!(
                                        "call to '{name}' struct parameter '{param_name}' expected '{struct_name}', got '{}'",
                                        alias.struct_name
                                    ),
                                    expression.loc(),
                                ));
                            }
                            if pending_dispatch.is_some() {
                                return Err(self.error(
                                    format!(
                                        "call to '{name}' contains more than one dynamically indexed nested processor argument"
                                    ),
                                    expression.loc(),
                                ));
                            }
                            let Some(first) = alias.alternatives.first() else {
                                return Err(self.error(
                                    format!(
                                        "call to '{name}' has no nested processor dispatch alternatives"
                                    ),
                                    expression.loc(),
                                ));
                            };
                            let argument_start = call_args.len();
                            let argument_len = first.len();
                            call_args.extend(first.iter().cloned());
                            pending_dispatch = Some(PendingCallDispatch {
                                index: alias.index,
                                argument_start,
                                argument_len,
                                alternatives: alias.alternatives,
                                slot_arguments: Vec::new(),
                            });
                            continue;
                        }
                    }
                    if let Some(argument) = indexed_argument {
                        match argument {
                            LoweredIndexedStructArgument::Direct(argument) => {
                                indexed_struct_selection = Some(argument.index);
                                self.append_indexed_struct_call_arguments(
                                    argument,
                                    expression.loc(),
                                    block,
                                    &mut call_args,
                                );
                            }
                            LoweredIndexedStructArgument::Dispatch {
                                index,
                                alternatives,
                            } => {
                                if pending_dispatch.is_some() {
                                    return Err(self.error(
                                        format!(
                                            "call to '{name}' contains more than one dynamically indexed nested processor argument"
                                        ),
                                        expression.loc(),
                                    ));
                                }
                                let Some(first) = alternatives.first() else {
                                    return Err(self.error(
                                        format!(
                                            "call to '{name}' has no nested processor dispatch alternatives"
                                        ),
                                        expression.loc(),
                                    ));
                                };
                                let argument_start = call_args.len();
                                let argument_len = first.len();
                                call_args.extend(first.iter().cloned());
                                pending_dispatch = Some(PendingCallDispatch {
                                    index,
                                    argument_start,
                                    argument_len,
                                    alternatives,
                                    slot_arguments: Vec::new(),
                                });
                            }
                        }
                        continue;
                    }
                    let Expr::Var { name: root, .. } = expression else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' struct parameter '{param_name}' requires a struct variable or indexed struct-array element"
                            ),
                            expression.loc(),
                        ));
                    };
                    let expected_fields =
                        self.struct_field_shapes(struct_name, expression.loc())?;
                    if let Some(Binding::StructParameter {
                        struct_name: actual_struct,
                        fields,
                    }) = self.bindings.get(root).cloned()
                    {
                        if actual_struct != *struct_name {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' struct parameter '{param_name}' expected '{struct_name}', got '{actual_struct}'"
                                ),
                                expression.loc(),
                            ));
                        }
                        for expected in expected_fields {
                            match expected {
                                StructFieldShape::Scalar {
                                    name: field_name,
                                    ty: expected_ty,
                                } => {
                                    let Some(StructFieldReference::Scalar {
                                        parameter,
                                        ty: actual_ty,
                                        ..
                                    }) = fields.iter().find(|candidate| {
                                        matches!(
                                            candidate,
                                            StructFieldReference::Scalar { name, .. }
                                                if *name == field_name
                                        )
                                    })
                                    else {
                                        return Err(self.error(
                                            format!(
                                                "forwarded struct parameter '{root}' is missing scalar field '{field_name}'"
                                            ),
                                            expression.loc(),
                                        ));
                                    };
                                    if *actual_ty != expected_ty {
                                        return Err(self.error(
                                            format!(
                                                "forwarded struct field '{root}.{field_name}' changed type"
                                            ),
                                            expression.loc(),
                                        ));
                                    }
                                    call_args.push(CallArgument::Place(Place {
                                        base: PlaceBase::Parameter(*parameter),
                                        projections: Vec::new(),
                                    }));
                                }
                                StructFieldShape::Array {
                                    name: field_name,
                                    element: expected_element,
                                    len: expected_len,
                                } => {
                                    let Some(StructFieldReference::Array {
                                        parameter,
                                        element,
                                        len,
                                        ..
                                    }) = fields.iter().find(|candidate| {
                                        matches!(
                                            candidate,
                                            StructFieldReference::Array { name, .. }
                                                if *name == field_name
                                        )
                                    })
                                    else {
                                        return Err(self.error(
                                            format!(
                                                "forwarded struct parameter '{root}' is missing array field '{field_name}'"
                                            ),
                                            expression.loc(),
                                        ));
                                    };
                                    if *element != expected_element || *len != expected_len {
                                        return Err(self.error(
                                            format!(
                                                "forwarded struct array field '{root}.{field_name}' changed type or length"
                                            ),
                                            expression.loc(),
                                        ));
                                    }
                                    call_args.push(CallArgument::Place(Place {
                                        base: PlaceBase::Parameter(*parameter),
                                        projections: Vec::new(),
                                    }));
                                }
                            }
                        }
                    } else if let Some(Binding::StructArrayElementAlias {
                        struct_name: actual_struct,
                    }) = self.bindings.get(root).cloned()
                    {
                        if actual_struct != *struct_name {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' struct parameter '{param_name}' expected '{struct_name}', got '{actual_struct}'"
                                ),
                                expression.loc(),
                            ));
                        }
                        for expected in expected_fields {
                            match expected {
                                StructFieldShape::Scalar {
                                    name: field_name,
                                    ty: expected_ty,
                                } => {
                                    let binding_name = format!("{root}.{field_name}");
                                    let Some(Binding::SliceElementAlias {
                                        slice,
                                        element,
                                        index,
                                    }) = self.bindings.get(&binding_name).cloned()
                                    else {
                                        return Err(self.error(
                                            format!(
                                                "struct-array element alias '{root}' is missing scalar field '{field_name}'"
                                            ),
                                            expression.loc(),
                                        ));
                                    };
                                    if element != expected_ty {
                                        return Err(self.error(
                                            format!(
                                                "struct-array element field '{binding_name}' changed type"
                                            ),
                                            expression.loc(),
                                        ));
                                    }
                                    call_args.push(CallArgument::SliceElement {
                                        slice: Value::Local(slice),
                                        index: Value::Local(index),
                                        bounds: BoundsMode::Unchecked,
                                    });
                                }
                                StructFieldShape::Array {
                                    name: field_name,
                                    element: expected_element,
                                    ..
                                } => {
                                    let binding_name = format!("{root}.{field_name}");
                                    let Some(Binding::Slice(slice, element, access)) =
                                        self.bindings.get(&binding_name).cloned()
                                    else {
                                        return Err(self.error(
                                            format!(
                                                "struct-array element alias '{root}' is missing array field '{field_name}'"
                                            ),
                                            expression.loc(),
                                        ));
                                    };
                                    if element != expected_element
                                        || access != onda_mir::AccessMode::ReadWrite
                                    {
                                        return Err(self.error(
                                            format!(
                                                "struct-array element array field '{binding_name}' changed type or access"
                                            ),
                                            expression.loc(),
                                        ));
                                    }
                                    call_args.push(CallArgument::SliceWindow {
                                        slice: Value::Local(slice),
                                        start: Value::Constant(ScalarValue::I32(0)),
                                        bounds: BoundsMode::Unchecked,
                                    });
                                }
                            }
                        }
                    } else if let Some(globals) = self.runtime_globals {
                        for expected in expected_fields {
                            match expected {
                                StructFieldShape::Scalar {
                                    name: field_name,
                                    ty: expected_ty,
                                } => {
                                    let flat = format!("{root}.{field_name}");
                                    let (state, actual_ty) = globals
                                        .states
                                        .get(&flat)
                                        .copied()
                                        .ok_or_else(|| {
                                            self.error(
                                                format!(
                                                    "call to '{name}' struct argument '{root}' has no scalar state field '{flat}'"
                                                ),
                                                expression.loc(),
                                            )
                                        })?;
                                    if actual_ty != expected_ty {
                                        return Err(self.error(
                                            format!("struct state field '{flat}' changed type"),
                                            expression.loc(),
                                        ));
                                    }
                                    call_args.push(CallArgument::Place(Place {
                                        base: PlaceBase::State(state),
                                        projections: Vec::new(),
                                    }));
                                }
                                StructFieldShape::Array {
                                    name: field_name,
                                    element: expected_element,
                                    len: expected_len,
                                } => {
                                    let flat = format!("{root}.{field_name}");
                                    let (state, element, len) = globals
                                        .state_arrays
                                        .get(&flat)
                                        .copied()
                                        .ok_or_else(|| {
                                            self.error(
                                                format!(
                                                    "call to '{name}' struct argument '{root}' has no array state field '{flat}'"
                                                ),
                                                expression.loc(),
                                            )
                                        })?;
                                    if element != expected_element || len != expected_len {
                                        return Err(self.error(
                                            format!(
                                                "struct array state field '{flat}' changed type or length"
                                            ),
                                            expression.loc(),
                                        ));
                                    }
                                    call_args.push(CallArgument::Place(Place {
                                        base: PlaceBase::State(state),
                                        projections: Vec::new(),
                                    }));
                                }
                            }
                        }
                    } else {
                        return Err(self.error(
                            format!("call to '{name}' cannot resolve struct argument '{root}'"),
                            expression.loc(),
                        ));
                    }
                }
                TypedFnParam::ProcArray { proc_name, len } => {
                    let PreparedCallArgument::DirectReference(prepared_expression) = prepared
                    else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' proc-array parameter '{param_name}' has a non-reference prepared argument"
                            ),
                            expression.loc(),
                        ));
                    };
                    self.append_proc_array_call_arguments(
                        name,
                        param_name,
                        proc_name,
                        *len,
                        &prepared_expression,
                        block,
                        &mut call_args,
                    )?;
                }
                TypedFnParam::StructArray { struct_name } => {
                    let PreparedCallArgument::DirectReference(prepared_expression) = prepared
                    else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' struct-array parameter '{param_name}' has a non-reference prepared argument"
                            ),
                            expression.loc(),
                        ));
                    };
                    let expression = &prepared_expression;
                    let Expr::Var { name: root, .. } = expression else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' struct-array parameter '{param_name}' requires a direct array variable"
                            ),
                            expression.loc(),
                        ));
                    };
                    let shapes = self.struct_field_shapes(struct_name, expression.loc())?;
                    if let Some(Binding::StructArrayParameter {
                        struct_name: actual_struct,
                        length,
                        fields,
                    }) = self.bindings.get(root).cloned()
                    {
                        if actual_struct != *struct_name {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' struct-array parameter '{param_name}' expected '{struct_name}', got '{actual_struct}'"
                                ),
                                expression.loc(),
                            ));
                        }
                        let length =
                            self.struct_array_length_value(length, block, expression.loc());
                        call_args.push(CallArgument::Value(length));
                        for shape in shapes {
                            let (field_name, expected_element) = match shape {
                                StructFieldShape::Scalar { name, ty } => (name, ty),
                                StructFieldShape::Array { name, element, .. } => (name, element),
                            };
                            let (_, local, actual_element) = fields
                                .iter()
                                .find(|(candidate, _, _)| *candidate == field_name)
                                .ok_or_else(|| {
                                    self.error(
                                        format!(
                                            "forwarded struct-array parameter '{root}' is missing field '{field_name}'"
                                        ),
                                        expression.loc(),
                                    )
                                })?;
                            if *actual_element != expected_element {
                                return Err(self.error(
                                    format!(
                                        "forwarded struct-array field '{root}.{field_name}' changed type"
                                    ),
                                    expression.loc(),
                                ));
                            }
                            call_args.push(CallArgument::Value(Value::Local(*local)));
                        }
                    } else if let Some(globals) = self.runtime_globals {
                        let (actual_struct, len) = globals
                            .array_struct_roots
                            .get(root)
                            .cloned()
                            .ok_or_else(|| {
                                self.error(
                                    format!(
                                        "call to '{name}' cannot resolve struct-array argument '{root}'"
                                    ),
                                    expression.loc(),
                                )
                            })?;
                        if actual_struct != *struct_name {
                            return Err(self.error(
                                format!(
                                    "call to '{name}' struct-array parameter '{param_name}' expected '{struct_name}', got '{actual_struct}'"
                                ),
                                expression.loc(),
                            ));
                        }
                        call_args.push(CallArgument::Value(Value::Constant(ScalarValue::I32(
                            len as i32,
                        ))));
                        for shape in shapes {
                            let (field_name, expected_element) = match shape {
                                StructFieldShape::Scalar { name, ty } => (name, ty),
                                StructFieldShape::Array { name, element, .. } => (name, element),
                            };
                            let flat = format!("{root}.{field_name}");
                            let (_, actual_element, _) = globals
                                .state_arrays
                                .get(&flat)
                                .copied()
                                .ok_or_else(|| {
                                    self.error(
                                        format!(
                                            "struct-array argument '{root}' has no flattened field '{flat}'"
                                        ),
                                        expression.loc(),
                                    )
                                })?;
                            if actual_element != expected_element {
                                return Err(self.error(
                                    format!("struct-array field '{flat}' changed type"),
                                    expression.loc(),
                                ));
                            }
                            let slice = self.lower_named_slice(
                                &flat,
                                None,
                                None,
                                Some(onda_mir::AccessMode::ReadWrite),
                                block,
                                expression.loc(),
                            )?;
                            call_args.push(CallArgument::Value(slice.value));
                        }
                    } else {
                        return Err(self.error(
                            format!(
                                "call to '{name}' cannot resolve struct-array argument '{root}'"
                            ),
                            expression.loc(),
                        ));
                    }
                }
            }
        }

        let result = if returns_value {
            let locals = result_types
                .iter()
                .copied()
                .map(|ty| (self.new_local(None, ty), ty))
                .collect::<Vec<_>>();
            self.push_call_with_optional_dispatch(
                block,
                function_id,
                locals.iter().map(|(local, _)| *local).collect(),
                call_args,
                pending_dispatch,
                location,
            )?;
            Some(
                locals
                    .into_iter()
                    .map(|(local, ty)| LoweredValue {
                        value: Value::Local(local),
                        ty,
                    })
                    .collect(),
            )
        } else {
            self.push_call_with_optional_dispatch(
                block,
                function_id,
                Vec::new(),
                call_args,
                pending_dispatch,
                location,
            )?;
            // Source functions without an explicit result retain the language's
            // legacy zero value when called from a value context.  The call
            // itself remains resultless in MIR; the default is a caller-side
            // semantic value rather than a synthetic callee ABI result.
            Some(
                result_types
                    .iter()
                    .copied()
                    .map(|ty| LoweredValue {
                        value: zero_value(ty),
                        ty,
                    })
                    .collect(),
            )
        };
        Ok(result)
    }

    pub(super) fn push_call_with_optional_dispatch(
        &mut self,
        block: &mut MirBlock,
        function: FunctionId,
        results: Vec<LocalId>,
        args: Vec<CallArgument>,
        dispatch: Option<PendingCallDispatch>,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let Some(dispatch) = dispatch else {
            self.push_statement(
                block,
                StatementKind::Call {
                    results,
                    function,
                    args,
                },
                location,
            );
            return Ok(());
        };
        if dispatch.alternatives.is_empty()
            || dispatch
                .alternatives
                .iter()
                .any(|alternative| alternative.len() != dispatch.argument_len)
        {
            return Err(self.error(
                "nested processor dispatch alternatives have inconsistent ABI widths",
                location,
            ));
        }
        let end = dispatch
            .argument_start
            .checked_add(dispatch.argument_len)
            .filter(|end| *end <= args.len())
            .ok_or_else(|| {
                self.error(
                    "nested processor dispatch argument range is invalid",
                    location,
                )
            })?;
        let mut calls = Vec::with_capacity(dispatch.alternatives.len());
        for (slot, alternative) in dispatch.alternatives.into_iter().enumerate() {
            let mut alternative_args = args.clone();
            alternative_args.splice(dispatch.argument_start..end, alternative);
            for (argument_index, choices) in &dispatch.slot_arguments {
                let Some(choice) = choices.get(slot) else {
                    return Err(self.error(
                        "nested processor dispatch has an incomplete slot-dependent argument",
                        location,
                    ));
                };
                let Some(argument) = alternative_args.get_mut(*argument_index) else {
                    return Err(self.error(
                        "nested processor dispatch slot-dependent argument index is invalid",
                        location,
                    ));
                };
                *argument = choice.clone();
            }
            calls.push(alternative_args);
        }
        let chain = self.build_dispatched_call_chain(
            dispatch.index,
            function,
            &results,
            &calls,
            0,
            location,
        );
        block.statements.extend(chain.statements);
        Ok(())
    }

    pub(super) fn build_dispatched_call_chain(
        &mut self,
        index: LocalId,
        function: FunctionId,
        results: &[LocalId],
        calls: &[Vec<CallArgument>],
        slot: usize,
        location: SourceLoc,
    ) -> MirBlock {
        let mut block = MirBlock::default();
        if slot + 1 == calls.len() {
            self.push_statement(
                &mut block,
                StatementKind::Call {
                    results: results.to_vec(),
                    function,
                    args: calls[slot].clone(),
                },
                location,
            );
            return block;
        }
        let condition = self.compare_value(
            &mut block,
            CompareOp::Equal,
            Value::Local(index),
            Value::Constant(ScalarValue::I32(slot as i32)),
            location,
        );
        let mut then_block = MirBlock::default();
        self.push_statement(
            &mut then_block,
            StatementKind::Call {
                results: results.to_vec(),
                function,
                args: calls[slot].clone(),
            },
            location,
        );
        let else_block =
            self.build_dispatched_call_chain(index, function, results, calls, slot + 1, location);
        self.push_statement(
            &mut block,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            },
            location,
        );
        block
    }

    pub(super) fn lower_buffer_metadata_call(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<Option<LoweredValue>, MirLoweringError> {
        if let Some(base) = parse_array_len_instance_base(name) {
            if let Some(Binding::StructArrayParameter { length, .. }) =
                self.bindings.get(base).cloned()
            {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(LoweredValue {
                    value: self.struct_array_length_value(length, block, location),
                    ty: PrimitiveType::I32,
                }));
            }
            if let Some(Binding::ProcArrayParameter { length, .. }) =
                self.bindings.get(base).cloned()
            {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::Load(Place {
                        base: PlaceBase::Parameter(length),
                        projections: Vec::new(),
                    }),
                    location,
                )));
            }
            if let Some((_, array)) = self.nested_proc_array_source_any(base) {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("processor-array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(LoweredValue {
                    value: Value::Constant(ScalarValue::I32(array.slots.len() as i32)),
                    ty: PrimitiveType::I32,
                }));
            }
            if let Some(Binding::ArrayParameter(_, _, len)) = self.bindings.get(base) {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(LoweredValue {
                    value: Value::Constant(ScalarValue::I32(*len as i32)),
                    ty: PrimitiveType::I32,
                }));
            }
            if let Some(Binding::Array(_, _, len)) = self.bindings.get(base) {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(LoweredValue {
                    value: Value::Constant(ScalarValue::I32(*len as i32)),
                    ty: PrimitiveType::I32,
                }));
            }
            if let Some(Binding::Slice(local, _, _)) = self.bindings.get(base).cloned() {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("slice length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::SliceLen(Value::Local(local)),
                    location,
                )));
            }
            if let Some(Binding::EventArrayParameter(_, _, len)) = self.bindings.get(base) {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("event array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(LoweredValue {
                    value: Value::Constant(ScalarValue::I32(*len as i32)),
                    ty: PrimitiveType::I32,
                }));
            }
            let array_len = self
                .const_arrays
                .get(base)
                .map(|(_, _, len)| *len)
                .or_else(|| {
                    self.runtime_globals.and_then(|globals| {
                        globals
                            .state_arrays
                            .get(base)
                            .map(|(_, _, len)| *len)
                            .or_else(|| globals.array_struct_roots.get(base).map(|(_, len)| *len))
                            .or_else(|| globals.input_arrays.get(base).map(|(_, _, len)| *len))
                            .or_else(|| globals.output_arrays.get(base).map(|(_, _, len)| *len))
                            .or_else(|| {
                                globals
                                    .control_output_arrays
                                    .get(base)
                                    .map(|(_, _, len)| *len)
                            })
                            .or_else(|| globals.param_arrays.get(base).map(|(_, _, len)| *len))
                    })
                });
            if let Some(len) = array_len {
                if !args.is_empty() {
                    return Err(self.error(
                        format!("array length call '{name}' unexpectedly has arguments"),
                        location,
                    ));
                }
                return Ok(Some(LoweredValue {
                    value: Value::Constant(ScalarValue::I32(len as i32)),
                    ty: PrimitiveType::I32,
                }));
            }
        }
        let metadata = if let Some(base) = parse_array_len_instance_base(name) {
            Some((base, PrimitiveType::I32, 0_u8))
        } else if let Some(base) = parse_buffer_chans_instance_base(name) {
            Some((base, PrimitiveType::I32, 1_u8))
        } else {
            parse_buffer_samplerate_instance_base(name).map(|base| (base, PrimitiveType::F32, 2_u8))
        };
        let Some((base, ty, operation)) = metadata else {
            return Ok(None);
        };
        if let Some(Binding::BufferParameter(parameter, _)) = self.bindings.get(base).cloned() {
            if !args.is_empty() {
                return Err(self.error(
                    format!("buffer metadata call '{name}' unexpectedly has arguments"),
                    location,
                ));
            }
            let rvalue = match operation {
                0 => Rvalue::BufferParamLen(parameter),
                1 => Rvalue::BufferParamChannels(parameter),
                _ => Rvalue::BufferParamSampleRate(parameter),
            };
            return Ok(Some(self.emit_temp(block, ty, rvalue, location)));
        }
        let buffer = self
            .runtime_globals
            .and_then(|globals| globals.buffers.get(base).copied());
        let Some((buffer, _)) = buffer else {
            return Ok(None);
        };
        if !args.is_empty() {
            return Err(self.error(
                format!("buffer metadata call '{name}' unexpectedly has arguments"),
                location,
            ));
        }
        let rvalue = match operation {
            0 => Rvalue::BufferLen(buffer),
            1 => Rvalue::BufferChannels(buffer),
            _ => Rvalue::BufferSampleRate(buffer),
        };
        Ok(Some(self.emit_temp(block, ty, rvalue, location)))
    }

    pub(super) fn lower_buffer_read_call(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<Option<LoweredValue>, MirLoweringError> {
        let (base, operands, has_channel, bounds) = if name == INTERNAL_BUFFER_READ2_FN {
            let base = call_resource_base(name, args, location)?;
            (base, &args[1..], true, BoundsMode::Clamp)
        } else if name == UNSAFE_READ_FN || name == UNSAFE_READ2_FN {
            let base = call_resource_base(name, args, location)?;
            (
                base,
                &args[1..],
                name == UNSAFE_READ2_FN,
                BoundsMode::Checked,
            )
        } else if let Some(base) = parse_unsafe_read_instance_base(name) {
            (base.to_owned(), args, false, BoundsMode::Checked)
        } else if let Some(base) = parse_unsafe_read2_instance_base(name) {
            (base.to_owned(), args, true, BoundsMode::Checked)
        } else {
            return Ok(None);
        };
        let expected = if has_channel { 2 } else { 1 };
        if operands.len() != expected {
            return Err(self.error(
                format!("resource read '{name}' expected {expected} index arguments"),
                location,
            ));
        }
        let mut lowered = Vec::with_capacity(operands.len());
        for arg in operands {
            let value = self.lower_expr(&arg.expr, block)?;
            lowered.push(self.coerce(value, PrimitiveType::I32, block, arg.expr.loc())?);
        }
        let (channel, index) = if has_channel {
            (Some(lowered[0].value), lowered[1].value)
        } else {
            (None, lowered[0].value)
        };

        if !has_channel {
            if let Some(Binding::ArrayParameter(parameter, ty, _)) =
                self.bindings.get(&base).cloned()
            {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::Parameter(parameter),
                        projections: vec![Projection::Index { index, bounds }],
                    }),
                    location,
                )));
            }
            if let Some(Binding::Array(local, ty, _)) = self.bindings.get(&base).cloned() {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::Local(local),
                        projections: vec![Projection::Index { index, bounds }],
                    }),
                    location,
                )));
            }
            if let Some(Binding::Slice(local, ty, _)) = self.bindings.get(&base).cloned() {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::SliceLoad {
                        slice: Value::Local(local),
                        index,
                        bounds,
                    },
                    location,
                )));
            }
            if let Some(Binding::EventArrayParameter(parameter, ty, _)) =
                self.bindings.get(&base).cloned()
            {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::EventParam(parameter),
                        projections: vec![Projection::Index { index, bounds }],
                    }),
                    location,
                )));
            }
            if let Some((state, ty, _)) = self
                .runtime_globals
                .and_then(|globals| globals.state_arrays.get(&base).copied())
            {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::State(state),
                        projections: vec![Projection::Index { index, bounds }],
                    }),
                    location,
                )));
            }
            if let Some((data, ty, _)) = self.const_arrays.get(&base).copied() {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::ConstDataLoad {
                        data,
                        index,
                        bounds,
                    },
                    location,
                )));
            }
            if let Some((input, ty, _)) = self
                .runtime_globals
                .and_then(|globals| globals.input_arrays.get(&base).copied())
            {
                if let Some((cache, cache_ty, _)) =
                    self.oversampled_input_arrays.get(&input).copied()
                {
                    debug_assert_eq!(cache_ty, ty);
                    return Ok(Some(self.emit_temp(
                        block,
                        ty,
                        Rvalue::Load(Place {
                            base: PlaceBase::Local(cache),
                            projections: vec![Projection::Index { index, bounds }],
                        }),
                        location,
                    )));
                }
                let frame = self.current_frame.ok_or_else(|| {
                    self.error(
                        format!("audio input array '{base}' was read outside the sample section"),
                        location,
                    )
                })?;
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::InputLoad {
                        input,
                        element: Some(index),
                        bounds,
                        frame,
                    },
                    location,
                )));
            }
            if let Some((param, ty, _)) = self
                .runtime_globals
                .and_then(|globals| globals.param_arrays.get(&base).copied())
            {
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::Load(Place {
                        base: PlaceBase::Param(param),
                        projections: vec![Projection::Index { index, bounds }],
                    }),
                    location,
                )));
            }
            if let Some((output, ty, _)) = self
                .runtime_globals
                .and_then(|globals| globals.output_arrays.get(&base).copied())
            {
                if let Some((cache, cache_ty, _)) =
                    self.audio_output_array_caches.get(&output).copied()
                {
                    debug_assert_eq!(cache_ty, ty);
                    return Ok(Some(self.emit_temp(
                        block,
                        ty,
                        Rvalue::Load(Place {
                            base: PlaceBase::Local(cache),
                            projections: vec![Projection::Index { index, bounds }],
                        }),
                        location,
                    )));
                }
                let frame = self.current_frame.ok_or_else(|| {
                    self.error(
                        format!("audio output array '{base}' was read outside the sample section"),
                        location,
                    )
                })?;
                return Ok(Some(self.emit_temp(
                    block,
                    ty,
                    Rvalue::OutputLoad {
                        output,
                        element: Some(index),
                        bounds,
                        frame,
                    },
                    location,
                )));
            }
        }
        if let Some(Binding::BufferParameter(parameter, ty)) = self.bindings.get(&base).cloned() {
            return Ok(Some(self.emit_temp(
                block,
                ty,
                Rvalue::BufferParamLoad {
                    parameter,
                    channel,
                    index,
                    bounds,
                },
                location,
            )));
        }
        let buffer = self
            .runtime_globals
            .and_then(|globals| globals.buffers.get(&base).copied());
        let Some((buffer, ty)) = buffer else {
            return Err(self.error(
                format!("resource read '{name}' references unsupported base '{base}'"),
                location,
            ));
        };
        Ok(Some(self.emit_temp(
            block,
            ty,
            Rvalue::BufferLoad {
                buffer,
                channel,
                index,
                bounds,
            },
            location,
        )))
    }

    pub(super) fn lower_buffer_write_call(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<bool, MirLoweringError> {
        let (base, operands, has_channel, bounds) = if name == INTERNAL_BUFFER_WRITE2_FN {
            let base = call_resource_base(name, args, location)?;
            (base, &args[1..], true, BoundsMode::Clamp)
        } else if name == UNSAFE_WRITE_FN || name == UNSAFE_WRITE2_FN {
            let base = call_resource_base(name, args, location)?;
            (
                base,
                &args[1..],
                name == UNSAFE_WRITE2_FN,
                BoundsMode::Checked,
            )
        } else if let Some(base) = parse_unsafe_write_instance_base(name) {
            (base.to_owned(), args, false, BoundsMode::Checked)
        } else if let Some(base) = parse_unsafe_write2_instance_base(name) {
            (base.to_owned(), args, true, BoundsMode::Checked)
        } else {
            return Ok(false);
        };
        let expected = if has_channel { 3 } else { 2 };
        if operands.len() != expected {
            return Err(self.error(
                format!(
                    "resource write '{name}' expected {} index/value arguments",
                    expected
                ),
                location,
            ));
        }
        let mut indices = Vec::with_capacity(expected - 1);
        for arg in &operands[..expected - 1] {
            let value = self.lower_expr(&arg.expr, block)?;
            indices.push(
                self.coerce(value, PrimitiveType::I32, block, arg.expr.loc())?
                    .value,
            );
        }
        let value_arg = &operands[expected - 1].expr;
        let (channel, index) = if has_channel {
            (Some(indices[0]), indices[1])
        } else {
            (None, indices[0])
        };

        if !has_channel {
            if let Some(Binding::ArrayParameter(parameter, ty, _)) =
                self.bindings.get(&base).cloned()
            {
                let value = self.lower_expr(value_arg, block)?;
                let value = self.coerce(value, ty, block, value_arg.loc())?;
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::Parameter(parameter),
                            projections: vec![Projection::Index { index, bounds }],
                        },
                        value: Rvalue::Use(value.value),
                    },
                    location,
                );
                return Ok(true);
            }
            if let Some(Binding::Array(local, ty, _)) = self.bindings.get(&base).cloned() {
                let value = self.lower_expr(value_arg, block)?;
                let value = self.coerce(value, ty, block, value_arg.loc())?;
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::Local(local),
                            projections: vec![Projection::Index { index, bounds }],
                        },
                        value: Rvalue::Use(value.value),
                    },
                    location,
                );
                return Ok(true);
            }
            if let Some(Binding::Slice(local, ty, access)) = self.bindings.get(&base).cloned() {
                if access != onda_mir::AccessMode::ReadWrite {
                    return Err(self.error(format!("slice '{base}' is read-only"), location));
                }
                let value = self.lower_expr(value_arg, block)?;
                let value = self.coerce(value, ty, block, value_arg.loc())?;
                self.push_statement(
                    block,
                    StatementKind::SliceStore {
                        slice: Value::Local(local),
                        index,
                        value: value.value,
                        bounds,
                    },
                    location,
                );
                return Ok(true);
            }
            if let Some((state, ty, _)) = self
                .runtime_globals
                .and_then(|globals| globals.state_arrays.get(&base).copied())
            {
                let value = self.lower_expr(value_arg, block)?;
                let value = self.coerce(value, ty, block, value_arg.loc())?;
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::State(state),
                            projections: vec![Projection::Index { index, bounds }],
                        },
                        value: Rvalue::Use(value.value),
                    },
                    location,
                );
                return Ok(true);
            }
            if let Some((output, ty, _)) = self
                .runtime_globals
                .and_then(|globals| globals.output_arrays.get(&base).copied())
            {
                let value = self.lower_expr(value_arg, block)?;
                let value = self.coerce(value, ty, block, value_arg.loc())?;
                if let Some((cache, cache_ty, _)) =
                    self.audio_output_array_caches.get(&output).copied()
                {
                    debug_assert_eq!(cache_ty, ty);
                    self.push_statement(
                        block,
                        StatementKind::Assign {
                            destination: Place {
                                base: PlaceBase::Local(cache),
                                projections: vec![Projection::Index { index, bounds }],
                            },
                            value: Rvalue::Use(value.value),
                        },
                        location,
                    );
                    return Ok(true);
                }
                let frame = self.current_frame.ok_or_else(|| {
                    self.error(
                        format!("audio output array '{base}' was written outside sample"),
                        location,
                    )
                })?;
                self.push_statement(
                    block,
                    StatementKind::OutputStore {
                        output,
                        element: Some(index),
                        bounds,
                        frame,
                        value: value.value,
                    },
                    location,
                );
                return Ok(true);
            }
        }
        if let Some(Binding::BufferParameter(parameter, ty)) = self.bindings.get(&base).cloned() {
            let value = self.lower_expr(value_arg, block)?;
            let value = self.coerce(value, ty, block, value_arg.loc())?;
            self.push_statement(
                block,
                StatementKind::BufferParamStore {
                    parameter,
                    channel,
                    index,
                    value: value.value,
                    bounds,
                },
                location,
            );
            return Ok(true);
        }
        let buffer = self
            .runtime_globals
            .and_then(|globals| globals.buffers.get(&base).copied());
        let Some((buffer, ty)) = buffer else {
            return Err(self.error(
                format!("resource write '{name}' references unsupported base '{base}'"),
                location,
            ));
        };
        let value = self.lower_expr(value_arg, block)?;
        let value = self.coerce(value, ty, block, value_arg.loc())?;
        self.push_statement(
            block,
            StatementKind::BufferStore {
                buffer,
                channel,
                index,
                value: value.value,
                bounds,
            },
            location,
        );
        Ok(true)
    }
}
