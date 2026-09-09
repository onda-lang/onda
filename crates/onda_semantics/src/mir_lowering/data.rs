use super::*;

/// All fixed-data operations use the canonical leaf plan. Scalars stay scalar
/// references; tensor extents remain array references, never expanded arguments.
impl FunctionLowerer<'_> {
    /// Resolve an indexed field through the same captured element view used by
    /// aggregate arguments. Returned arrays have no flattened source binding.
    pub(super) fn lower_indexed_data_field(
        &mut self,
        expression: &Expr,
        block: &mut MirBlock,
    ) -> Result<Option<Expr>, MirLoweringError> {
        let Expr::UserCall {
            name, args, loc, ..
        } = expression
        else {
            return Ok(None);
        };
        if name == crate::proc_state_rewrite::STRUCT_ARRAY_FIELD_INDEX_SENTINEL {
            let Some((base, index, field, field_index)) =
                crate::array_structs::extract_safi_args(args)
            else {
                return Err(self.error("malformed indexed data array field", expression.loc()));
            };
            let selection = crate::indexed_read_expr(base, index, IndexAccess::Clamp, *loc);
            let alias = self.fresh_data_name();
            if !self.lower_struct_array_element_alias(
                &alias,
                &selection,
                block,
                expression.loc(),
            )? {
                return Ok(None);
            }
            return Ok(Some(Expr::Index {
                loc: *loc,
                base: format!("{alias}.{field}"),
                index: Box::new(field_index),
            }));
        }
        if name.strip_prefix(crate::proc_state_rewrite::PROC_FIELD_SENTINEL_PREFIX)
            != Some(PROC_INDEX_CALL_SENTINEL)
        {
            return Ok(None);
        }
        let Some((base, index, field, access)) =
            crate::array_structs::extract_proc_index_field_args(args)
        else {
            return Err(self.error("malformed indexed data field", expression.loc()));
        };
        let selection = crate::indexed_read_expr(base, index, access, *loc);
        let alias = self.fresh_data_name();
        if !self.lower_struct_array_element_alias(&alias, &selection, block, expression.loc())? {
            return Ok(None);
        }
        Ok(Some(Expr::Var {
            name: format!("{alias}.{field}"),
            loc: *loc,
        }))
    }

    fn data_shapes(
        &self,
        data: &DataType,
        loc: SourceLoc,
    ) -> Result<Vec<StructFieldShape>, MirLoweringError> {
        match data {
            DataType::Struct(name) => self.struct_field_shapes(name, loc),
            DataType::Array {
                element: ArrayElemType::Primitive(element),
                len,
            } => Ok(vec![StructFieldShape::Array {
                name: String::new(),
                element: *element,
                len: self.data_extent(*len, loc)?,
            }]),
            DataType::Array {
                element: ArrayElemType::Struct(name),
                len,
            } => {
                let outer = self.data_extent(*len, loc)?;
                self.struct_field_shapes(name, loc)?
                    .into_iter()
                    .map(|shape| {
                        let (name, element, width) = match shape {
                            StructFieldShape::Scalar { name, ty } => (name, ty, 1),
                            StructFieldShape::Array { name, element, len } => (name, element, len),
                        };
                        let len = outer
                            .checked_mul(width)
                            .filter(|len| *len <= i32::MAX as u32)
                            .ok_or_else(|| {
                                self.error("data tensor exceeds the supported index range", loc)
                            })?;
                        Ok(StructFieldShape::Array { name, element, len })
                    })
                    .collect()
            }
        }
    }

    pub(super) fn data_extent(&self, len: usize, loc: SourceLoc) -> Result<u32, MirLoweringError> {
        u32::try_from(len)
            .ok()
            .filter(|len| *len > 0 && *len <= i32::MAX as u32)
            .ok_or_else(|| self.error("data extent must be between 1 and i32::MAX", loc))
    }

    fn data_leaf_name(root: &str, path: &str) -> String {
        if path.is_empty() {
            root.to_owned()
        } else {
            format!("{root}.{path}")
        }
    }

    pub(super) fn fresh_data_name(&mut self) -> String {
        let id = self.next_data_id;
        self.next_data_id += 1;
        format!("__onda_data_{id}")
    }

    pub(super) fn bind_data_result_parameters(
        &mut self,
        data: DataType,
    ) -> Result<(), MirLoweringError> {
        let loc = function_location(self.function);
        for shape in self.data_shapes(&data, loc)? {
            let parameter = ParameterId::new(self.params.len() as u32);
            let (name, ty, binding) = match shape {
                StructFieldShape::Scalar { name, ty } => (
                    Self::data_leaf_name("__onda_result", &name),
                    self.scalar_type_id(ty),
                    Binding::ReferenceParameter(parameter, ty),
                ),
                StructFieldShape::Array { name, element, len } => (
                    Self::data_leaf_name("__onda_result", &name),
                    intern_array_type(self.types, element, len),
                    Binding::ArrayParameter(parameter, element, len),
                ),
            };
            self.params.push(onda_mir::FunctionParam {
                name: name.clone(),
                ty,
                mode: onda_mir::PassingMode::ResultReference,
                integer_range: None,
            });
            self.bindings.insert(name, binding);
        }
        Ok(())
    }

    pub(super) fn allocate_data(
        &mut self,
        name: &str,
        data: &DataType,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        for shape in self.data_shapes(data, loc)? {
            let (name, binding) = match shape {
                StructFieldShape::Scalar { name: path, ty } => {
                    let name = Self::data_leaf_name(name, &path);
                    let local = self.new_local(Some(name.clone()), ty);
                    if let DataType::Struct(struct_name) = data {
                        self.locals[local.index()].integer_range =
                            crate::resolve_struct_field_decl(struct_name, &path, self.structs)
                                .and_then(|field| field.integer_range.as_ref())
                                .and_then(typed_integer_range_invariant);
                    }
                    (name, Binding::PlaceAlias(Place::local(local), ty))
                }
                StructFieldShape::Array {
                    name: path,
                    element,
                    len,
                } => {
                    let name = Self::data_leaf_name(name, &path);
                    let local = self.new_array_local(Some(name.clone()), element, len);
                    (name, Binding::Array(local, element, len))
                }
            };
            self.bindings.insert(name, binding);
        }
        self.bind_data_root(name, data, loc)?;
        Ok(())
    }

    pub(super) fn bind_data_root(
        &mut self,
        name: &str,
        data: &DataType,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let binding = match data {
            DataType::Struct(struct_name) => {
                for embedded in self.embedded_struct_array_shapes(struct_name, loc)? {
                    self.bindings.insert(
                        Self::data_leaf_name(name, &embedded.path),
                        Binding::StructArrayStorage {
                            struct_name: embedded.struct_name,
                            len: embedded.len,
                        },
                    );
                }
                Binding::StructView {
                    struct_name: struct_name.clone(),
                }
            }
            DataType::Array {
                element: ArrayElemType::Struct(struct_name),
                len,
            } => Binding::StructArrayStorage {
                struct_name: struct_name.clone(),
                len: self.data_extent(*len, loc)?,
            },
            DataType::Array { .. } => return Ok(()),
        };
        self.bindings.insert(name.to_owned(), binding);
        Ok(())
    }

    pub(super) fn bind_data_alias(
        &mut self,
        name: &str,
        source: &str,
        data: &DataType,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        for shape in self.data_shapes(data, loc)? {
            let (path, extent) = match shape {
                StructFieldShape::Scalar { name, .. } => (name, None),
                StructFieldShape::Array { name, len, .. } => (name, Some(len)),
            };
            let source = Self::data_leaf_name(source, &path);
            if let Some(len) = extent {
                if let Some(Binding::Slice(local, element, access, _)) =
                    self.bindings.get(&source).cloned()
                {
                    self.bindings.insert(
                        Self::data_leaf_name(name, &path),
                        Binding::Slice(local, element, access, Some(len)),
                    );
                    continue;
                }
                let slice =
                    self.lower_slice_expression(&Expr::var(&source).with_loc(loc), None, block)?;
                let Value::Local(local) = slice.value else {
                    unreachable!("prepared slice is local")
                };
                self.bindings.insert(
                    Self::data_leaf_name(name, &path),
                    Binding::Slice(local, slice.element, slice.access, Some(len)),
                );
                continue;
            }
            let binding = self
                .bindings
                .get(&source)
                .cloned()
                .or_else(|| {
                    let (state, ty) = self.runtime_globals?.states.get(&source)?;
                    Some(Binding::PlaceAlias(
                        Place {
                            base: PlaceBase::State(*state),
                            projections: Vec::new(),
                        },
                        *ty,
                    ))
                })
                .ok_or_else(|| {
                    self.error(
                        format!("data alias source '{source}' has no reference binding"),
                        loc,
                    )
                })?;
            self.bindings
                .insert(Self::data_leaf_name(name, &path), binding);
        }
        self.bind_data_root(name, data, loc)?;
        Ok(())
    }

    pub(super) fn append_data_result_arguments(
        &self,
        name: &str,
        data: &DataType,
        args: &mut Vec<CallArgument>,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        for shape in self.data_shapes(data, loc)? {
            let path = match shape {
                StructFieldShape::Scalar { name, .. } | StructFieldShape::Array { name, .. } => {
                    name
                }
            };
            let field = Self::data_leaf_name(name, &path);
            let base = match self.bindings.get(&field) {
                Some(Binding::PlaceAlias(place, _)) => {
                    args.push(CallArgument::Place(place.clone()));
                    continue;
                }
                Some(Binding::Local(local, _) | Binding::Array(local, _, _)) => {
                    PlaceBase::Local(*local)
                }
                Some(
                    Binding::ReferenceParameter(parameter, _)
                    | Binding::ArrayParameter(parameter, _, _),
                ) => PlaceBase::Parameter(*parameter),
                _ => {
                    return Err(self.error(
                        format!("data result field '{field}' has no owned storage"),
                        loc,
                    ))
                }
            };
            args.push(CallArgument::Place(Place {
                base,
                projections: Vec::new(),
            }));
        }
        Ok(())
    }

    pub(super) fn data_type_of(&self, expr: &Expr) -> Option<DataType> {
        if let Some(selection) = indexed_read_source(expr) {
            if let Some(Binding::StructArrayParameter { struct_name, .. }) =
                self.bindings.get(selection.base)
            {
                return Some(DataType::Struct(struct_name.clone()));
            }
            if let Some(DataType::Array {
                element: ArrayElemType::Struct(name),
                ..
            }) = self.data_type_of(&Expr::var(selection.base))
            {
                return Some(DataType::Struct(name));
            }
        }
        match expr {
            Expr::ArrayCtor {
                spec,
                initialize: true,
                ..
            } => Some(DataType::Array {
                element: spec.elem.clone(),
                len: crate::def_semantics::const_positive_usize_for_call_type(&spec.size)?,
            }),
            Expr::ArrayLiteral { values, .. } => {
                let DataType::Struct(name) = self.data_type_of(values.first()?)? else {
                    return None;
                };
                Some(DataType::Array {
                    element: ArrayElemType::Struct(name),
                    len: values.len(),
                })
            }
            Expr::UserCall { name, .. } if self.structs.contains_key(name) => {
                Some(DataType::Struct(name.clone()))
            }
            Expr::UserCall { name, .. } => match &self
                .functions
                .get(*self.function_indices.get(name)?)?
                .return_ty
            {
                ReturnType::Data(data) => Some(data.clone()),
                _ => None,
            },
            Expr::Var { name, .. } => {
                match self.bindings.get(name) {
                    Some(
                        Binding::StructParameter { struct_name, .. }
                        | Binding::StructView { struct_name },
                    ) => return Some(DataType::Struct(struct_name.clone())),
                    Some(
                        Binding::Array(_, element, len)
                        | Binding::ArrayParameter(_, element, len)
                        | Binding::EventArrayParameter(_, element, len)
                        | Binding::Slice(_, element, _, Some(len)),
                    ) => {
                        return Some(DataType::Array {
                            element: ArrayElemType::Primitive(*element),
                            len: *len as usize,
                        })
                    }
                    Some(
                        Binding::StructArrayParameter {
                            struct_name,
                            length: StructArrayLength::Fixed(len),
                            ..
                        }
                        | Binding::StructArrayStorage { struct_name, len },
                    ) => {
                        return Some(DataType::Array {
                            element: ArrayElemType::Struct(struct_name.clone()),
                            len: *len as usize,
                        });
                    }
                    _ => {}
                }
                if let Some(globals) = self.runtime_globals {
                    if let Some((struct_name, len)) = globals.array_struct_roots.get(name) {
                        return Some(DataType::Array {
                            element: ArrayElemType::Struct(struct_name.clone()),
                            len: *len as usize,
                        });
                    }
                    if let Some(struct_name) = globals.struct_roots.get(name) {
                        return Some(DataType::Struct(struct_name.clone()));
                    }
                    if let Some((_, element, len)) = globals.state_arrays.get(name) {
                        return Some(DataType::Array {
                            element: ArrayElemType::Primitive(*element),
                            len: *len as usize,
                        });
                    }
                }
                let (root, path) = name.split_once('.')?;
                let DataType::Struct(struct_name) = self.data_type_of(&Expr::var(root))? else {
                    return None;
                };
                let field = crate::resolve_struct_field_decl(&struct_name, path, self.structs)?;
                Self::field_data_type(field)
            }
            _ => None,
        }
    }

    fn field_data_type(field: &TypedStructField) -> Option<DataType> {
        match field.ty {
            TypedFieldType::Struct => Some(DataType::Struct(field.struct_name.clone()?)),
            TypedFieldType::Array(len) => Some(DataType::Array {
                element: field
                    .array_elem_struct
                    .as_ref()
                    .map(|name| ArrayElemType::Struct(name.clone()))
                    .unwrap_or(ArrayElemType::Primitive(
                        field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                    )),
                len,
            }),
            _ => None,
        }
    }

    /// Tuple fields use the same scalar leaf bindings as their container.
    pub(super) fn data_tuple_components(&self, name: &str) -> Option<Vec<String>> {
        let (root, path) = name.split_once('.')?;
        let DataType::Struct(struct_name) = self.data_type_of(&Expr::var(root))? else {
            return None;
        };
        let field = crate::resolve_struct_field_decl(&struct_name, path, self.structs)?;
        let TypedFieldType::Tuple(types) = &field.ty else {
            return None;
        };
        Some(
            (0..types.len())
                .map(|index| format!("{name}.__{index}"))
                .collect(),
        )
    }

    pub(super) fn normalize_data_scalar_store(
        &mut self,
        name: &str,
        value: LoweredValue,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<LoweredValue, MirLoweringError> {
        let range = name.split_once('.').and_then(|(root, path)| {
            let DataType::Struct(struct_name) = self.data_type_of(&Expr::var(root))? else {
                return None;
            };
            crate::resolve_struct_field_decl(&struct_name, path, self.structs)?
                .integer_range
                .as_ref()
                .and_then(|range| {
                    typed_integer_range_invariant(range).map(|invariant| (range.ty, invariant))
                })
        });
        let Some((ty, range)) = range else {
            return Ok(value);
        };
        let value = self.coerce(value, ty, block, loc)?;
        if super::values::value_is_within_integer_range(value.value, range, &self.locals) {
            return Ok(value);
        }
        Ok(self.emit_temp(
            block,
            ty,
            Rvalue::Intrinsic {
                intrinsic: match range.mode {
                    onda_mir::IntegerRangeMode::Clamp => Intrinsic::RangeClamp,
                    onda_mir::IntegerRangeMode::Wrap => Intrinsic::RangeWrap,
                },
                args: vec![
                    value.value,
                    Value::Constant(range.min),
                    Value::Constant(range.max),
                ],
            },
            loc,
        ))
    }

    pub(super) fn prepare_data_array_view(
        &mut self,
        name: &str,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if matches!(
            self.bindings.get(name),
            Some(Binding::StructArrayParameter { .. })
        ) {
            return Ok(());
        }
        if self
            .runtime_globals
            .is_some_and(|globals| globals.array_struct_roots.contains_key(name))
        {
            return Ok(());
        }
        let Some(DataType::Array {
            element: ArrayElemType::Struct(struct_name),
            len,
        }) = self.data_type_of(&Expr::var(name))
        else {
            return Ok(());
        };
        let mut fields = Vec::new();
        for shape in self.struct_field_shapes(&struct_name, loc)? {
            let (path, element) = match shape {
                StructFieldShape::Scalar { name, ty } => (name, ty),
                StructFieldShape::Array { name, element, .. } => (name, element),
            };
            let slice = self.lower_slice_expression(
                &Expr::var(Self::data_leaf_name(name, &path)),
                None,
                block,
            )?;
            let Value::Local(local) = slice.value else {
                unreachable!()
            };
            fields.push((path, local, element));
        }
        self.bindings.insert(
            name.to_owned(),
            Binding::StructArrayParameter {
                struct_name,
                length: StructArrayLength::Fixed(self.data_extent(len, loc)?),
                fields,
            },
        );
        Ok(())
    }

    pub(super) fn lower_data_expr(
        &mut self,
        expr: &Expr,
        data: &DataType,
        block: &mut MirBlock,
    ) -> Result<String, MirLoweringError> {
        if let Expr::Var { name, .. } = expr {
            return Ok(name.clone());
        }
        let name = self.fresh_data_name();
        if self.lower_struct_array_element_alias(&name, expr, block, expr.loc())? {
            return Ok(name);
        }
        if matches!(expr, Expr::Slice { .. }) {
            if self.lower_struct_slice_alias(&name, expr, block)? {
                return Ok(name);
            }
            let slice = self.lower_slice_expression(expr, None, block)?;
            let Value::Local(local) = slice.value else {
                unreachable!()
            };
            let DataType::Array { len, .. } = data else {
                unreachable!()
            };
            self.bindings.insert(
                name.clone(),
                Binding::Slice(
                    local,
                    slice.element,
                    slice.access,
                    Some(self.data_extent(*len, expr.loc())?),
                ),
            );
            return Ok(name);
        }
        if let DataType::Array {
            element: ArrayElemType::Primitive(element),
            len,
        } = data
        {
            if let Some(slice) = self.lower_array_value_slice(
                expr,
                *element,
                onda_mir::AccessMode::ReadWrite,
                block,
            )? {
                let Value::Local(local) = slice.value else {
                    unreachable!()
                };
                self.bindings.insert(
                    name.clone(),
                    Binding::Slice(
                        local,
                        slice.element,
                        slice.access,
                        Some(self.data_extent(*len, expr.loc())?),
                    ),
                );
                return Ok(name);
            }
        }
        self.allocate_data(&name, data, expr.loc())?;
        match expr {
            Expr::UserCall {
                name: constructor,
                args,
                ..
            } if self.structs.contains_key(constructor) => {
                self.initialize_data_struct(&name, constructor, args, block, expr.loc())?;
            }
            Expr::UserCall {
                name: callee,
                type_args,
                args,
                ..
            } => {
                self.lower_user_call_into(callee, type_args, args, Some(&name), expr.loc(), block)?;
            }
            Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. } => {
                let DataType::Array { element, len } = data else {
                    unreachable!()
                };
                if let ArrayElemType::Struct(struct_name) = element {
                    self.initialize_data_array(&name, struct_name, *len, expr, block)?;
                    return Ok(name);
                }
                unreachable!("primitive literals were materialized above");
            }
            _ => {
                return Err(self.error(
                    "expression does not select or construct fixed data",
                    expr.loc(),
                ))
            }
        }
        Ok(name)
    }

    /// Canonical data leaves have independent storage: an alias can overlap
    /// the corresponding leaf of another view, never an unrelated field.
    /// Capture scalar values, preflight every array copy, then commit. Each
    /// array uses memmove semantics without a second fixed scratch allocation.
    pub(super) fn copy_data(
        &mut self,
        destination: &str,
        source: &str,
        data: &DataType,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if destination == source {
            return Ok(());
        }
        let mut scalars = Vec::new();
        let mut copies = Vec::new();
        for shape in self.data_shapes(data, loc)? {
            match shape {
                StructFieldShape::Scalar { name, .. } => {
                    let value = self.lower_expr(
                        &Expr::var(Self::data_leaf_name(source, &name)).with_loc(loc),
                        block,
                    )?;
                    scalars.push((Self::data_leaf_name(destination, &name), value));
                }
                StructFieldShape::Array { name, .. } => {
                    let source = self.lower_slice_expression(
                        &Expr::var(Self::data_leaf_name(source, &name)).with_loc(loc),
                        Some(onda_mir::AccessMode::ReadOnly),
                        block,
                    )?;
                    let destination = self.lower_slice_expression(
                        &Expr::var(Self::data_leaf_name(destination, &name)).with_loc(loc),
                        Some(onda_mir::AccessMode::ReadWrite),
                        block,
                    )?;
                    copies.push(onda_mir::SliceCopy {
                        destination: destination.value,
                        source: source.value,
                    });
                }
            }
        }
        if !copies.is_empty() {
            self.push_statement(block, StatementKind::SliceCopy { copies }, loc);
        }
        for (name, value) in scalars {
            self.store_data_scalar(&name, value, block, loc)?;
        }
        Ok(())
    }

    pub(super) fn store_data_scalar(
        &mut self,
        name: &str,
        value: LoweredValue,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if !self.assign_runtime_global(name, &[value], block, loc, loc)? {
            self.assign_variable_values(
                name,
                vec![value],
                None,
                &Expr::var(name).with_loc(loc),
                block,
                loc,
            )?;
        }
        Ok(())
    }

    fn initialize_data_struct(
        &mut self,
        target: &str,
        constructor: &str,
        args: &[onda_frontend::CallArg],
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let fields = self
            .structs
            .get(constructor)
            .ok_or_else(|| self.error("unknown data constructor", loc))?
            .iter()
            .filter(|field| crate::data_construction::is_authored_struct_field(field))
            .cloned()
            .collect::<Vec<_>>();
        let indices = crate::data_construction::constructor_fields(&fields, args)
            .map_err(|(message, loc)| self.error(message, loc))?;
        let mut supplied = HashSet::new();
        // Capture authored operands in source order, including named operands.
        for (index, arg) in indices.into_iter().zip(args) {
            supplied.insert(index);
            self.initialize_data_field(target, &fields[index], Some(&arg.expr), block, loc)?;
        }
        for (index, field) in fields.iter().enumerate() {
            if !supplied.contains(&index) {
                self.initialize_data_field(target, field, field.default.as_ref(), block, loc)?;
            }
        }
        Ok(())
    }

    fn initialize_data_field(
        &mut self,
        root: &str,
        field: &TypedStructField,
        expr: Option<&Expr>,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let target = Self::data_leaf_name(root, &field.name);
        if let Some(data) = Self::field_data_type(field) {
            if let Some(expr) = expr {
                let source = self.lower_data_expr(expr, &data, block)?;
                self.copy_data(&target, &source, &data, block, loc)?;
            } else if let DataType::Struct(name) = data {
                self.initialize_data_struct(&target, &name, &[], block, loc)?;
            } else if let DataType::Array {
                element: ArrayElemType::Struct(name),
                len,
            } = data
            {
                self.initialize_default_struct_array(&target, &name, len, block, loc)?;
            } else {
                for shape in self.data_shapes(&data, loc)? {
                    let StructFieldShape::Array { name, element, .. } = shape else {
                        unreachable!()
                    };
                    let destination = self.lower_slice_expression(
                        &Expr::var(Self::data_leaf_name(&target, &name)),
                        Some(onda_mir::AccessMode::ReadWrite),
                        block,
                    )?;
                    self.push_statement(
                        block,
                        StatementKind::SliceFill {
                            destination: destination.value,
                            value: Value::Constant(zero_scalar(element)),
                        },
                        loc,
                    );
                }
            }
        } else {
            match &field.ty {
                TypedFieldType::Scalar(ty) => {
                    let value = if let Some(expr) = expr {
                        self.lower_expr(expr, block)?
                    } else {
                        LoweredValue {
                            value: Value::Constant(zero_scalar(*ty)),
                            ty: *ty,
                        }
                    };
                    let value = self.coerce(value, *ty, block, loc)?;
                    self.store_data_scalar(&target, value, block, loc)?;
                }
                TypedFieldType::Tuple(types) => {
                    let values = if let Some(expr) = expr {
                        self.lower_value_expr(expr, block)?
                    } else {
                        types
                            .iter()
                            .map(|ty| LoweredValue {
                                value: Value::Constant(zero_scalar(*ty)),
                                ty: *ty,
                            })
                            .collect()
                    };
                    for (index, (value, ty)) in values.into_iter().zip(types).enumerate() {
                        let value = self.coerce(value, *ty, block, loc)?;
                        self.store_data_scalar(&format!("{target}.__{index}"), value, block, loc)?;
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn initialize_data_array(
        &mut self,
        target: &str,
        struct_name: &str,
        len: usize,
        expr: &Expr,
        block: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        let values = match expr {
            Expr::ArrayCtor { init: None, .. } => {
                return self.initialize_default_struct_array(
                    target,
                    struct_name,
                    len,
                    block,
                    expr.loc(),
                );
            }
            Expr::ArrayCtor {
                init: Some(values), ..
            }
            | Expr::ArrayLiteral { values, .. } => values,
            _ => unreachable!(),
        };
        let data = DataType::Struct(struct_name.to_owned());
        if let (true, [value]) = (
            matches!(
                expr,
                Expr::ArrayCtor {
                    init_is_value: true,
                    ..
                }
            ),
            values.as_slice(),
        ) {
            if self.is_struct_array_expression(value) {
                let array = DataType::Array {
                    element: ArrayElemType::Struct(struct_name.to_owned()),
                    len,
                };
                let source = self.lower_data_expr(value, &array, block)?;
                return self.copy_data(target, &source, &array, block, expr.loc());
            }
            if matches!(expr, Expr::ArrayCtor { .. }) {
                let source = self.lower_data_expr(value, &data, block)?;
                // Capture once before touching any element of the destination.
                let captured = self.fresh_data_name();
                self.allocate_data(&captured, &data, expr.loc())?;
                self.copy_data(&captured, &source, &data, block, expr.loc())?;
                return self.fill_data_array(
                    target,
                    &captured,
                    struct_name,
                    Value::Constant(ScalarValue::I32(self.data_extent(len, expr.loc())? as i32)),
                    block,
                    expr.loc(),
                );
            }
        }
        self.prepare_data_array_view(target, block, expr.loc())?;
        for (index, value) in values.iter().enumerate() {
            let source = self.lower_data_expr(value, &data, block)?;
            let element = self.fresh_data_name();
            let selection = Expr::Index {
                loc: expr.loc().into(),
                base: target.to_owned(),
                index: Box::new(Expr::int(index as i64)),
            };
            self.lower_struct_array_element_alias(&element, &selection, block, expr.loc())?;
            self.copy_data(&element, &source, &data, block, expr.loc())?;
        }
        Ok(())
    }

    fn initialize_default_struct_array(
        &mut self,
        target: &str,
        struct_name: &str,
        len: usize,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let data = DataType::Struct(struct_name.to_owned());
        let source = self.fresh_data_name();
        self.allocate_data(&source, &data, loc)?;
        self.initialize_data_struct(&source, struct_name, &[], block, loc)?;
        self.fill_data_array(
            target,
            &source,
            struct_name,
            Value::Constant(ScalarValue::I32(self.data_extent(len, loc)? as i32)),
            block,
            loc,
        )
    }

    pub(super) fn fill_data_array(
        &mut self,
        target: &str,
        source: &str,
        struct_name: &str,
        length: Value,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let data = DataType::Struct(struct_name.to_owned());
        self.prepare_data_array_view(target, block, loc)?;
        let index_name = self.fresh_data_name();
        let index = self.new_local(Some(index_name.clone()), PrimitiveType::I32);
        if let Value::Constant(ScalarValue::I32(len)) = length {
            self.locals[index.index()].integer_range = Some(onda_mir::IntegerRangeInvariant {
                min: ScalarValue::I32(0),
                max: ScalarValue::I32(len.max(1) - 1),
                mode: onda_mir::IntegerRangeMode::Clamp,
            });
        }
        self.bindings.insert(
            index_name.clone(),
            Binding::Local(index, PrimitiveType::I32),
        );
        self.assign_value(block, index, Value::Constant(ScalarValue::I32(0)), loc);
        let mut body = MirBlock::default();
        let done = self.compare_value(
            &mut body,
            CompareOp::GreaterEqual,
            Value::Local(index),
            length,
            loc,
        );
        let mut exit = MirBlock::default();
        self.push_statement(&mut exit, StatementKind::Break, loc);
        self.push_statement(
            &mut body,
            StatementKind::If {
                condition: done,
                then_block: exit,
                else_block: MirBlock::default(),
            },
            loc,
        );
        let element = self.fresh_data_name();
        let selection = Expr::Index {
            loc: loc.into(),
            base: target.to_owned(),
            index: Box::new(Expr::var(index_name)),
        };
        if !self.lower_struct_array_element_alias(&element, &selection, &mut body, loc)? {
            return Err(self.error("data array has no element view", loc));
        }
        self.copy_data(&element, source, &data, &mut body, loc)?;
        let last = match length {
            Value::Constant(ScalarValue::I32(len)) if len > 0 => {
                Some(Value::Constant(ScalarValue::I32(len - 1)))
            }
            _ => None,
        };
        self.emit_for_latch(
            &mut body,
            index,
            Value::Constant(ScalarValue::I32(1)),
            last,
            loc,
        );
        self.push_statement(block, StatementKind::Loop { body }, loc);
        Ok(())
    }
}
