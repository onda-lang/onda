use super::*;

impl<'a> FunctionLowerer<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        function: &'a TypedFunction,
        functions: &'a [TypedFunction],
        function_ids: &'a HashMap<FunctionKey, FunctionId>,
        function_indices: &'a HashMap<String, usize>,
        oversample_factors: &'a HashMap<String, usize>,
        proc_instance_oversample_factors: &'a HashMap<String, usize>,
        proc_step_oversample_meta: Option<&'a ProcStepOversampleMeta>,
        structs: &'a HashMap<String, Vec<TypedStructField>>,
        aggregate_layouts: &'a AggregateLayoutTable,
        nested_proc_arrays: &'a [TypedNestedProcArray],
        const_arrays: &'a HashMap<String, (onda_mir::ConstDataId, PrimitiveType, u32)>,
        host_config: onda_mir::CompileConfig,
        config: onda_mir::CompileConfig,
        emitted_name: String,
        types: &'a mut Vec<MirType>,
        source_files: &'a mut Vec<SourceFile>,
    ) -> Self {
        Self {
            function,
            functions,
            function_ids,
            function_indices,
            oversample_factors,
            proc_instance_oversample_factors,
            proc_step_oversample_meta,
            structs,
            aggregate_layouts,
            nested_proc_arrays,
            const_arrays,
            host_config,
            config,
            emitted_name,
            types,
            source_files,
            runtime_globals: None,
            current_frame: None,
            oversampled_inputs: HashMap::new(),
            audio_output_caches: HashMap::new(),
            oversampled_input_endpoints: HashMap::new(),
            audio_output_endpoint_caches: HashMap::new(),
            oversampled_input_arrays: HashMap::new(),
            audio_output_array_caches: HashMap::new(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            bindings: HashMap::new(),
            nested_proc_aliases: HashMap::new(),
            event_slice_parameters: Vec::new(),
            prezeroed_init_state_dirty: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_runtime(
        function: &'a TypedFunction,
        functions: &'a [TypedFunction],
        function_ids: &'a HashMap<FunctionKey, FunctionId>,
        function_indices: &'a HashMap<String, usize>,
        oversample_factors: &'a HashMap<String, usize>,
        proc_instance_oversample_factors: &'a HashMap<String, usize>,
        host_config: onda_mir::CompileConfig,
        config: onda_mir::CompileConfig,
        emitted_name: String,
        globals: &'a RuntimeGlobals,
        types: &'a mut Vec<MirType>,
        source_files: &'a mut Vec<SourceFile>,
    ) -> Self {
        let mut lowerer = Self::new(
            function,
            functions,
            function_ids,
            function_indices,
            oversample_factors,
            proc_instance_oversample_factors,
            None,
            &globals.structs,
            &globals.aggregate_layouts,
            &globals.nested_proc_arrays,
            &globals.const_arrays,
            host_config,
            config,
            emitted_name,
            types,
            source_files,
        );
        lowerer.runtime_globals = Some(globals);
        lowerer
    }

    pub(super) fn with_prezeroed_init_state(mut self) -> Self {
        debug_assert!(self.runtime_globals.is_some());
        self.prezeroed_init_state_dirty = Some(Vec::new());
        self
    }

    pub(super) fn bind_event_params(&mut self, event: &TypedEvent) -> Result<(), MirLoweringError> {
        for (index, param) in event.params.iter().enumerate() {
            let id = onda_mir::EventParamId::new(index as u32);
            if let TypedEventParamType::Slice { elem } = &param.ty {
                self.event_slice_parameters
                    .push((param.name.clone(), id, *elem));
                continue;
            }
            let binding = match &param.ty {
                TypedEventParamType::Scalar(ty) => Binding::EventParameter(id, *ty),
                TypedEventParamType::Array { elem, len } => {
                    let len = u32::try_from(*len).map_err(|_| {
                        self.error(
                            format!(
                                "event '{}' array parameter '{}' length does not fit u32",
                                event.name, param.name
                            ),
                            function_location(self.function),
                        )
                    })?;
                    Binding::EventArrayParameter(id, *elem, len)
                }
                TypedEventParamType::Slice { .. } => unreachable!("handled above"),
            };
            self.bindings.insert(param.name.clone(), binding);
        }
        Ok(())
    }

    pub(super) fn struct_field_shapes(
        &self,
        struct_name: &str,
        location: SourceLoc,
    ) -> Result<Vec<StructFieldShape>, MirLoweringError> {
        let layout = self
            .aggregate_layouts
            .layout_for_struct(struct_name)
            .ok_or_else(|| {
                self.error(
                    format!("struct parameter references unknown type '{struct_name}'"),
                    location,
                )
            })?;
        layout
            .leaves
            .iter()
            .map(|leaf| {
                if leaf.tensor.shape.is_empty() {
                    Ok(StructFieldShape::Scalar {
                        name: leaf.storage_path.clone(),
                        ty: leaf.scalar,
                    })
                } else {
                    let len = u32::try_from(leaf.tensor.element_count).map_err(|_| {
                        self.error(
                            format!(
                                "struct parameter type '{struct_name}' field '{}' flattened length does not fit u32",
                                leaf.storage_path
                            ),
                            location,
                        )
                    })?;
                    Ok(StructFieldShape::Array {
                        name: leaf.storage_path.clone(),
                        element: leaf.scalar,
                        len,
                    })
                }
            })
            .collect()
    }

    pub(super) fn embedded_struct_array_shapes(
        &self,
        struct_name: &str,
        location: SourceLoc,
    ) -> Result<Vec<EmbeddedStructArrayShape>, MirLoweringError> {
        let layout = self
            .aggregate_layouts
            .layout_for_struct(struct_name)
            .ok_or_else(|| {
                self.error(
                    format!("embedded aggregate array references unknown type '{struct_name}'"),
                    location,
                )
            })?;
        let mut seen = HashSet::new();
        let mut arrays = Vec::new();
        for leaf in &layout.leaves {
            let mut path = Vec::new();
            for component in &leaf.path {
                let AggregatePathComponent::Field {
                    name,
                    aggregate,
                    extent,
                } = component
                else {
                    continue;
                };
                path.push(name.as_str());
                let (Some(nested_id), Some(extent)) = (aggregate, extent) else {
                    continue;
                };
                let storage_path = path.join(".");
                if seen.insert(storage_path.clone()) {
                    let nested = self.aggregate_layouts.get(*nested_id).ok_or_else(|| {
                        self.error(
                            format!(
                                "embedded aggregate array '{struct_name}.{storage_path}' has an invalid canonical layout ID"
                            ),
                            location,
                        )
                    })?;
                    let len = u32::try_from(*extent).map_err(|_| {
                        self.error(
                            format!(
                                "embedded aggregate array '{struct_name}.{storage_path}' length does not fit u32"
                            ),
                            location,
                        )
                    })?;
                    let fields = nested
                        .leaves
                        .iter()
                        .map(|nested_leaf| {
                            let width = u32::try_from(nested_leaf.tensor.element_count).map_err(
                                |_| {
                                    self.error(
                                        format!(
                                            "embedded aggregate array '{struct_name}.{storage_path}.{}' flattened element width does not fit u32",
                                            nested_leaf.storage_path
                                        ),
                                        location,
                                    )
                                },
                            )?;
                            Ok(EmbeddedStructArrayFieldShape {
                                outer_name: format!(
                                    "{storage_path}.{}",
                                    nested_leaf.storage_path
                                ),
                                inner_name: nested_leaf.storage_path.clone(),
                                element: nested_leaf.scalar,
                                width,
                            })
                        })
                        .collect::<Result<Vec<_>, MirLoweringError>>()?;
                    arrays.push(EmbeddedStructArrayShape {
                        path: storage_path,
                        struct_name: nested.struct_name.clone(),
                        len,
                        fields,
                    });
                }
                // Deeper aggregate arrays are views of one selected element of
                // this array. They are bound when that element is aliased.
                break;
            }
        }
        Ok(arrays)
    }

    pub(super) fn struct_array_length_value(
        &mut self,
        length: StructArrayLength,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Value {
        match length {
            StructArrayLength::Dynamic(parameter) => {
                self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::Load(Place {
                        base: PlaceBase::Parameter(parameter),
                        projections: Vec::new(),
                    }),
                    location,
                )
                .value
            }
            StructArrayLength::Fixed(len) => Value::Constant(ScalarValue::I32(len as i32)),
        }
    }

    pub(super) fn bind_runtime_embedded_struct_arrays(
        &mut self,
        block: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        let Some(globals) = self.runtime_globals else {
            return Ok(());
        };
        let roots = globals
            .struct_roots
            .iter()
            .map(|(root, struct_name)| (root.clone(), struct_name.clone()))
            .collect::<Vec<_>>();
        for (root, struct_name) in roots {
            for embedded in
                self.embedded_struct_array_shapes(&struct_name, function_location(self.function))?
            {
                let binding_name = format!("{root}.{}", embedded.path);
                if self.bindings.contains_key(&binding_name) {
                    continue;
                }
                let mut fields = Vec::with_capacity(embedded.fields.len());
                for field in embedded.fields {
                    let flat_name = format!("{root}.{}", field.outer_name);
                    let (_, actual_element, actual_len) = self
                        .runtime_globals
                        .and_then(|globals| globals.state_arrays.get(&flat_name).copied())
                        .ok_or_else(|| {
                            self.error(
                                format!(
                                    "embedded aggregate state array '{binding_name}' has no canonical leaf storage '{flat_name}'"
                                ),
                                function_location(self.function),
                            )
                        })?;
                    let expected_len = embedded.len.checked_mul(field.width).ok_or_else(|| {
                        self.error(
                            format!(
                                "embedded aggregate state array '{binding_name}' leaf '{}' length overflows u32",
                                field.inner_name
                            ),
                            function_location(self.function),
                        )
                    })?;
                    if actual_element != field.element || actual_len != expected_len {
                        return Err(self.error(
                            format!(
                                "embedded aggregate state array '{binding_name}' leaf '{}' changed type or flattened length",
                                field.inner_name
                            ),
                            function_location(self.function),
                        ));
                    }
                    let slice = self.lower_named_slice(
                        &flat_name,
                        SliceSelection::default(),
                        Some(onda_mir::AccessMode::ReadWrite),
                        block,
                        function_location(self.function),
                    )?;
                    let Value::Local(local) = slice.value else {
                        unreachable!("slice construction always produces a local")
                    };
                    fields.push((field.inner_name, local, field.element));
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
        }
        Ok(())
    }

    pub(super) fn proc_array_field_shapes(
        &self,
        proc_name: &str,
        location: SourceLoc,
    ) -> Result<Vec<StructFieldShape>, MirLoweringError> {
        self.struct_field_shapes(proc_name, location)
    }

    pub(super) fn lower(mut self) -> Result<onda_mir::Function, MirLoweringError> {
        let mut scalar_parameters = Vec::new();
        let mut slice_parameters = Vec::new();
        let mut tuple_parameters = Vec::new();
        let mut struct_array_parameters = Vec::new();
        let mut proc_array_parameters = Vec::new();
        let mut embedded_struct_array_parameters = Vec::new();
        let mut next_parameter_id = 0_u32;
        for (name, kind) in self
            .function
            .params
            .iter()
            .zip(self.function.param_kinds.iter())
        {
            match kind {
                TypedFnParam::Scalar { ty } => {
                    let ty = ty.unwrap_or(PrimitiveType::F32);
                    let type_id = self.scalar_type_id(ty);
                    self.params.push(onda_mir::FunctionParam {
                        name: name.clone(),
                        ty: type_id,
                        mode: onda_mir::PassingMode::Value,
                    });
                    scalar_parameters.push((name.clone(), ParameterId::new(next_parameter_id), ty));
                    next_parameter_id += 1;
                }
                TypedFnParam::Array { elem_ty } => {
                    let access = if self.function.readonly_array_params.contains(name) {
                        onda_mir::AccessMode::ReadOnly
                    } else {
                        onda_mir::AccessMode::ReadWrite
                    };
                    let type_id = intern_slice_type(self.types, *elem_ty, access);
                    self.params.push(onda_mir::FunctionParam {
                        name: name.clone(),
                        ty: type_id,
                        mode: onda_mir::PassingMode::Value,
                    });
                    slice_parameters.push((
                        name.clone(),
                        ParameterId::new(next_parameter_id),
                        *elem_ty,
                        access,
                    ));
                    next_parameter_id += 1;
                }
                TypedFnParam::Tuple { elem_tys } => {
                    let mut components = Vec::with_capacity(elem_tys.len());
                    for (component_index, ty) in elem_tys.iter().copied().enumerate() {
                        let type_id = self.scalar_type_id(ty);
                        let parameter = ParameterId::new(next_parameter_id);
                        self.params.push(onda_mir::FunctionParam {
                            name: format!("{name}.{component_index}"),
                            ty: type_id,
                            mode: onda_mir::PassingMode::Value,
                        });
                        components.push((parameter, ty));
                        next_parameter_id += 1;
                    }
                    tuple_parameters.push((name.clone(), components));
                }
                TypedFnParam::Buffer { elem_ty, channels } => {
                    let channels = mir_buffer_channels(channels).ok_or_else(|| {
                        self.error(
                            format!("buffer parameter '{name}' channel count does not fit u32"),
                            function_location(self.function),
                        )
                    })?;
                    let type_id = intern_buffer_type(
                        self.types,
                        *elem_ty,
                        channels,
                        onda_mir::AccessMode::ReadWrite,
                    );
                    let parameter = ParameterId::new(next_parameter_id);
                    self.params.push(onda_mir::FunctionParam {
                        name: name.clone(),
                        ty: type_id,
                        mode: onda_mir::PassingMode::ReadWriteReference,
                    });
                    self.bindings
                        .insert(name.clone(), Binding::BufferParameter(parameter, *elem_ty));
                    next_parameter_id += 1;
                }
                TypedFnParam::BufferArray {
                    elem_ty,
                    channels,
                    len,
                } => {
                    let channels = mir_buffer_channels(channels).ok_or_else(|| {
                        self.error(
                            format!("buffer collection parameter '{name}' channel count does not fit u32"),
                            function_location(self.function),
                        )
                    })?;
                    let len = u32::try_from(*len).map_err(|_| {
                        self.error(
                            format!("buffer collection parameter '{name}' count does not fit u32"),
                            function_location(self.function),
                        )
                    })?;
                    if len == 0 {
                        return Err(self.error(
                            format!("buffer collection parameter '{name}' cannot be empty"),
                            function_location(self.function),
                        ));
                    }
                    let type_id = intern_buffer_span_type(
                        self.types,
                        *elem_ty,
                        channels,
                        onda_mir::AccessMode::ReadWrite,
                        len,
                    );
                    let span = ParameterId::new(next_parameter_id);
                    self.params.push(onda_mir::FunctionParam {
                        name: name.clone(),
                        ty: type_id,
                        mode: onda_mir::PassingMode::Value,
                    });
                    next_parameter_id += 1;
                    self.bindings.insert(
                        name.clone(),
                        Binding::BufferParameterArray(span, *elem_ty, len),
                    );
                }
                TypedFnParam::Struct { struct_name } => {
                    let shapes =
                        self.struct_field_shapes(struct_name, function_location(self.function))?;
                    let mut fields = Vec::with_capacity(shapes.len());
                    for shape in shapes {
                        match shape {
                            StructFieldShape::Scalar {
                                name: field_name,
                                ty,
                            } => {
                                let type_id = self.scalar_type_id(ty);
                                let parameter = ParameterId::new(next_parameter_id);
                                self.params.push(onda_mir::FunctionParam {
                                    name: format!("{name}.{field_name}"),
                                    ty: type_id,
                                    mode: onda_mir::PassingMode::ReadWriteReference,
                                });
                                self.bindings.insert(
                                    format!("{name}.{field_name}"),
                                    Binding::ReferenceParameter(parameter, ty),
                                );
                                fields.push(StructFieldReference::Scalar {
                                    name: field_name,
                                    parameter,
                                    ty,
                                });
                                next_parameter_id += 1;
                            }
                            StructFieldShape::Array {
                                name: field_name,
                                element,
                                len,
                            } => {
                                let type_id = intern_array_type(self.types, element, len);
                                let parameter = ParameterId::new(next_parameter_id);
                                self.params.push(onda_mir::FunctionParam {
                                    name: format!("{name}.{field_name}"),
                                    ty: type_id,
                                    mode: onda_mir::PassingMode::ReadWriteReference,
                                });
                                self.bindings.insert(
                                    format!("{name}.{field_name}"),
                                    Binding::ArrayParameter(parameter, element, len),
                                );
                                fields.push(StructFieldReference::Array {
                                    name: field_name,
                                    parameter,
                                    element,
                                    len,
                                });
                                next_parameter_id += 1;
                            }
                        }
                    }
                    if let Some(declarations) = self.structs.get(struct_name).cloned() {
                        for field in &declarations {
                            let TypedFieldType::Tuple(types) = &field.ty else {
                                continue;
                            };
                            let mut components = Vec::with_capacity(types.len());
                            for (index, ty) in types.iter().copied().enumerate() {
                                let flat_name = format!("{}.__{index}", field.name);
                                let Some(StructFieldReference::Scalar {
                                    parameter,
                                    ty: actual,
                                    ..
                                }) = fields.iter().find(|candidate| {
                                    matches!(
                                        candidate,
                                        StructFieldReference::Scalar { name, .. }
                                            if *name == flat_name
                                    )
                                })
                                else {
                                    return Err(self.error(
                                        format!(
                                            "struct tuple field '{name}.{}' is missing component {index}",
                                            field.name
                                        ),
                                        function_location(self.function),
                                    ));
                                };
                                if *actual != ty {
                                    return Err(self.error(
                                        format!(
                                            "struct tuple field '{name}.{}' component {index} changed type",
                                            field.name
                                        ),
                                        function_location(self.function),
                                    ));
                                }
                                components.push((*parameter, ty));
                            }
                            self.bindings.insert(
                                format!("{name}.{}", field.name),
                                Binding::TupleReferenceParameter(components),
                            );
                        }
                        for field in &declarations {
                            let TypedFieldType::Struct = field.ty else {
                                continue;
                            };
                            let Some(nested_struct) = &field.struct_name else {
                                continue;
                            };
                            let nested_shapes = self.struct_field_shapes(
                                nested_struct,
                                function_location(self.function),
                            )?;
                            let mut nested_fields = Vec::with_capacity(nested_shapes.len());
                            for nested_shape in nested_shapes {
                                match nested_shape {
                                    StructFieldShape::Scalar {
                                        name: nested_name,
                                        ty,
                                    } => {
                                        let outer_name = format!("{}.{}", field.name, nested_name);
                                        let Some(StructFieldReference::Scalar {
                                            parameter,
                                            ty: actual,
                                            ..
                                        }) = fields.iter().find(|candidate| {
                                            matches!(
                                                candidate,
                                                StructFieldReference::Scalar { name, .. }
                                                    if *name == outer_name
                                            )
                                        })
                                        else {
                                            return Err(self.error(
                                                format!(
                                                    "nested struct field '{name}.{}' is missing scalar field '{nested_name}'",
                                                    field.name
                                                ),
                                                function_location(self.function),
                                            ));
                                        };
                                        if *actual != ty {
                                            return Err(self.error(
                                                format!(
                                                    "nested struct field '{name}.{}.{nested_name}' changed type",
                                                    field.name
                                                ),
                                                function_location(self.function),
                                            ));
                                        }
                                        nested_fields.push(StructFieldReference::Scalar {
                                            name: nested_name,
                                            parameter: *parameter,
                                            ty,
                                        });
                                    }
                                    StructFieldShape::Array {
                                        name: nested_name,
                                        element,
                                        len,
                                    } => {
                                        let outer_name = format!("{}.{}", field.name, nested_name);
                                        let Some(StructFieldReference::Array {
                                            parameter,
                                            element: actual_element,
                                            len: actual_len,
                                            ..
                                        }) = fields.iter().find(|candidate| {
                                            matches!(
                                                candidate,
                                                StructFieldReference::Array { name, .. }
                                                    if *name == outer_name
                                            )
                                        })
                                        else {
                                            return Err(self.error(
                                                format!(
                                                    "nested struct field '{name}.{}' is missing array field '{nested_name}'",
                                                    field.name
                                                ),
                                                function_location(self.function),
                                            ));
                                        };
                                        if *actual_element != element || *actual_len != len {
                                            return Err(self.error(
                                                format!(
                                                    "nested struct array field '{name}.{}.{nested_name}' changed shape",
                                                    field.name
                                                ),
                                                function_location(self.function),
                                            ));
                                        }
                                        nested_fields.push(StructFieldReference::Array {
                                            name: nested_name,
                                            parameter: *parameter,
                                            element,
                                            len,
                                        });
                                    }
                                }
                            }
                            self.bindings.insert(
                                format!("{name}.{}", field.name),
                                Binding::StructParameter {
                                    struct_name: nested_struct.clone(),
                                    fields: nested_fields,
                                },
                            );
                        }
                    }
                    for embedded in self.embedded_struct_array_shapes(
                        struct_name,
                        function_location(self.function),
                    )? {
                        let mut embedded_fields = Vec::with_capacity(embedded.fields.len());
                        for embedded_field in embedded.fields {
                            let Some(StructFieldReference::Array {
                                parameter,
                                element,
                                len,
                                ..
                            }) = fields.iter().find(|candidate| {
                                matches!(
                                    candidate,
                                    StructFieldReference::Array { name, .. }
                                        if *name == embedded_field.outer_name
                                )
                            })
                            else {
                                return Err(self.error(
                                    format!(
                                        "embedded aggregate array '{name}.{}' is missing canonical leaf '{}'",
                                        embedded.path, embedded_field.outer_name
                                    ),
                                    function_location(self.function),
                                ));
                            };
                            let expected_len = embedded
                                .len
                                .checked_mul(embedded_field.width)
                                .ok_or_else(|| {
                                    self.error(
                                        format!(
                                            "embedded aggregate array '{name}.{}' flattened leaf '{}' length overflows u32",
                                            embedded.path, embedded_field.inner_name
                                        ),
                                        function_location(self.function),
                                    )
                                })?;
                            if *element != embedded_field.element || *len != expected_len {
                                return Err(self.error(
                                    format!(
                                        "embedded aggregate array '{name}.{}' canonical leaf '{}' changed type or flattened length",
                                        embedded.path, embedded_field.inner_name
                                    ),
                                    function_location(self.function),
                                ));
                            }
                            embedded_fields.push(PendingEmbeddedStructArrayField {
                                inner_name: embedded_field.inner_name,
                                parameter: *parameter,
                                total_len: *len,
                                element: *element,
                            });
                        }
                        embedded_struct_array_parameters.push(PendingEmbeddedStructArrayView {
                            name: format!("{name}.{}", embedded.path),
                            struct_name: embedded.struct_name,
                            len: embedded.len,
                            fields: embedded_fields,
                        });
                    }
                    self.bindings.insert(
                        name.clone(),
                        Binding::StructParameter {
                            struct_name: struct_name.clone(),
                            fields,
                        },
                    );
                }
                TypedFnParam::StructArray { struct_name } => {
                    let length_parameter = ParameterId::new(next_parameter_id);
                    let length_type = self.scalar_type_id(PrimitiveType::I32);
                    self.params.push(onda_mir::FunctionParam {
                        name: format!("{name}.len"),
                        ty: length_type,
                        mode: onda_mir::PassingMode::Value,
                    });
                    next_parameter_id += 1;

                    let shapes =
                        self.struct_field_shapes(struct_name, function_location(self.function))?;
                    let mut fields = Vec::with_capacity(shapes.len());
                    for shape in shapes {
                        let (field_name, element) = match shape {
                            StructFieldShape::Scalar { name, ty } => (name, ty),
                            StructFieldShape::Array { name, element, .. } => (name, element),
                        };
                        let parameter = ParameterId::new(next_parameter_id);
                        let ty =
                            intern_slice_type(self.types, element, onda_mir::AccessMode::ReadWrite);
                        self.params.push(onda_mir::FunctionParam {
                            name: format!("{name}.{field_name}"),
                            ty,
                            mode: onda_mir::PassingMode::Value,
                        });
                        let binding_name = format!("{name}.{field_name}");
                        slice_parameters.push((
                            binding_name,
                            parameter,
                            element,
                            onda_mir::AccessMode::ReadWrite,
                        ));
                        fields.push((field_name, parameter, element));
                        next_parameter_id += 1;
                    }
                    struct_array_parameters.push((
                        name.clone(),
                        struct_name.clone(),
                        length_parameter,
                        fields,
                    ));
                }
                TypedFnParam::ProcArray { proc_name, len } => {
                    let fixed_len = u32::try_from(*len).map_err(|_| {
                        self.error(
                            format!("proc-array parameter '{name}' length does not fit u32"),
                            function_location(self.function),
                        )
                    })?;
                    let length_parameter = ParameterId::new(next_parameter_id);
                    let length_type = self.scalar_type_id(PrimitiveType::I32);
                    self.params.push(onda_mir::FunctionParam {
                        name: format!("{name}.len"),
                        ty: length_type,
                        mode: onda_mir::PassingMode::Value,
                    });
                    next_parameter_id += 1;

                    let active_name = runtime_proc_array_active_symbol(name);
                    let active_parameter = ParameterId::new(next_parameter_id);
                    let active_type = intern_slice_type(
                        self.types,
                        PrimitiveType::Bool,
                        onda_mir::AccessMode::ReadWrite,
                    );
                    self.params.push(onda_mir::FunctionParam {
                        name: active_name.clone(),
                        ty: active_type,
                        mode: onda_mir::PassingMode::Value,
                    });
                    slice_parameters.push((
                        active_name,
                        active_parameter,
                        PrimitiveType::Bool,
                        onda_mir::AccessMode::ReadWrite,
                    ));
                    next_parameter_id += 1;

                    let shapes =
                        self.proc_array_field_shapes(proc_name, function_location(self.function))?;
                    let mut fields = Vec::with_capacity(shapes.len());
                    for shape in shapes {
                        let (field_name, element) = match shape {
                            StructFieldShape::Scalar { name, ty } => (name, ty),
                            StructFieldShape::Array { name, element, .. } => (name, element),
                        };
                        let parameter = ParameterId::new(next_parameter_id);
                        let ty =
                            intern_slice_type(self.types, element, onda_mir::AccessMode::ReadWrite);
                        self.params.push(onda_mir::FunctionParam {
                            name: format!("{name}.{field_name}"),
                            ty,
                            mode: onda_mir::PassingMode::Value,
                        });
                        let binding_name = format!("{name}.{field_name}");
                        slice_parameters.push((
                            binding_name,
                            parameter,
                            element,
                            onda_mir::AccessMode::ReadWrite,
                        ));
                        fields.push((field_name, parameter, element));
                        next_parameter_id += 1;
                    }
                    proc_array_parameters.push((
                        name.clone(),
                        proc_name.clone(),
                        fixed_len,
                        length_parameter,
                        fields,
                    ));
                }
            }
        }

        if self.function.returns_value {
            let result_types = match &self.function.return_ty {
                ReturnType::Scalar(result) => vec![*result],
                ReturnType::Tuple(results) => results.clone(),
            };
            for result in result_types {
                let result_type = self.scalar_type_id(result);
                self.results.push(result_type);
            }
        }

        let mut body = MirBlock::default();
        for (name, parameter, ty) in scalar_parameters {
            let local = self.new_local(Some(name.clone()), ty);
            self.push_statement(
                &mut body,
                StatementKind::Assign {
                    destination: Place::local(local),
                    value: Rvalue::Load(Place {
                        base: PlaceBase::Parameter(parameter),
                        projections: Vec::new(),
                    }),
                },
                function_location(self.function),
            );
            self.bindings.insert(name, Binding::Local(local, ty));
        }
        for (name, components) in tuple_parameters {
            let mut locals = Vec::with_capacity(components.len());
            for (component_index, (parameter, ty)) in components.into_iter().enumerate() {
                let local = self.new_local(Some(format!("{name}.{component_index}")), ty);
                self.push_statement(
                    &mut body,
                    StatementKind::Assign {
                        destination: Place::local(local),
                        value: Rvalue::Load(Place {
                            base: PlaceBase::Parameter(parameter),
                            projections: Vec::new(),
                        }),
                    },
                    function_location(self.function),
                );
                locals.push((local, ty));
            }
            self.bindings.insert(name, Binding::Tuple(locals));
        }
        let mut lowered_slice_parameters = HashMap::new();
        for (name, parameter, element, access) in slice_parameters {
            let slice = self.emit_slice_temp(
                &mut body,
                Some(name.clone()),
                element,
                access,
                Rvalue::Load(Place {
                    base: PlaceBase::Parameter(parameter),
                    projections: Vec::new(),
                }),
                function_location(self.function),
            );
            let Value::Local(local) = slice.value else {
                unreachable!("slice temporaries are always locals")
            };
            self.bindings
                .insert(name.clone(), Binding::Slice(local, element, access));
            lowered_slice_parameters.insert(name, (local, element));
        }
        for (name, struct_name, length, fields) in struct_array_parameters {
            let mut lowered_fields = Vec::with_capacity(fields.len());
            for (field_name, _, element) in fields {
                let binding_name = format!("{name}.{field_name}");
                let (local, actual) = lowered_slice_parameters
                    .get(&binding_name)
                    .copied()
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "struct-array parameter '{name}' is missing lowered field '{field_name}'"
                            ),
                            function_location(self.function),
                        )
                    })?;
                if actual != element {
                    return Err(self.error(
                        format!(
                            "struct-array parameter '{name}' field '{field_name}' changed element type"
                        ),
                        function_location(self.function),
                    ));
                }
                lowered_fields.push((field_name, local, element));
            }
            self.bindings.insert(
                name,
                Binding::StructArrayParameter {
                    struct_name,
                    length: StructArrayLength::Dynamic(length),
                    fields: lowered_fields,
                },
            );
        }
        for embedded in embedded_struct_array_parameters {
            let mut fields = Vec::with_capacity(embedded.fields.len());
            for field in embedded.fields {
                let slice = self.emit_slice_temp(
                    &mut body,
                    Some(format!("{}.{}", embedded.name, field.inner_name)),
                    field.element,
                    onda_mir::AccessMode::ReadWrite,
                    Rvalue::MakeSlice {
                        source: onda_mir::SliceSource::Place(Place {
                            base: PlaceBase::Parameter(field.parameter),
                            projections: Vec::new(),
                        }),
                        start: Value::Constant(ScalarValue::I32(0)),
                        len: Value::Constant(ScalarValue::I32(field.total_len as i32)),
                        bounds: BoundsMode::Unchecked,
                        access: onda_mir::AccessMode::ReadWrite,
                    },
                    function_location(self.function),
                );
                let Value::Local(local) = slice.value else {
                    unreachable!("slice construction always produces a local")
                };
                fields.push((field.inner_name, local, field.element));
            }
            self.bindings.insert(
                embedded.name,
                Binding::StructArrayParameter {
                    struct_name: embedded.struct_name,
                    length: StructArrayLength::Fixed(embedded.len),
                    fields,
                },
            );
        }
        for (name, proc_name, fixed_len, length, fields) in proc_array_parameters {
            let active_name = runtime_proc_array_active_symbol(&name);
            let (active, active_element) = lowered_slice_parameters
                .get(&active_name)
                .copied()
                .ok_or_else(|| {
                    self.error(
                        format!(
                            "proc-array parameter '{name}' is missing lowered active-slot storage"
                        ),
                        function_location(self.function),
                    )
                })?;
            if active_element != PrimitiveType::Bool {
                return Err(self.error(
                    format!("proc-array parameter '{name}' active-slot storage is not bool"),
                    function_location(self.function),
                ));
            }

            let mut lowered_fields = Vec::with_capacity(fields.len());
            for (field_name, _, element) in fields {
                let binding_name = format!("{name}.{field_name}");
                let (local, actual) = lowered_slice_parameters
                    .get(&binding_name)
                    .copied()
                    .ok_or_else(|| {
                        self.error(
                            format!(
                                "proc-array parameter '{name}' is missing lowered field '{field_name}'"
                            ),
                            function_location(self.function),
                        )
                    })?;
                if actual != element {
                    return Err(self.error(
                        format!(
                            "proc-array parameter '{name}' field '{field_name}' changed element type"
                        ),
                        function_location(self.function),
                    ));
                }
                lowered_fields.push((field_name, local, element));
            }
            self.bindings.insert(
                name,
                Binding::ProcArrayParameter {
                    proc_name,
                    fixed_len,
                    length,
                    active,
                    fields: lowered_fields,
                },
            );
        }
        for (name, parameter, element) in self.event_slice_parameters.clone() {
            let access = onda_mir::AccessMode::ReadOnly;
            let slice = self.emit_slice_temp(
                &mut body,
                Some(name.clone()),
                element,
                access,
                Rvalue::Load(Place {
                    base: PlaceBase::EventParam(parameter),
                    projections: Vec::new(),
                }),
                function_location(self.function),
            );
            let Value::Local(local) = slice.value else {
                unreachable!("slice temporaries are always locals")
            };
            self.bindings
                .insert(name, Binding::Slice(local, element, access));
        }
        self.bind_runtime_embedded_struct_arrays(&mut body)?;
        if let Some(meta) = self.proc_step_oversample_meta.cloned() {
            let factor = self
                .oversample_factors
                .get(&self.function.name)
                .copied()
                .unwrap_or(1);
            self.lower_oversampled_proc_step(&meta, factor, &mut body)?;
        } else {
            self.lower_statements(&self.function.body, &mut body, ContinueMode::None)?;
        }
        let source = self.source_span(function_location(self.function));
        Ok(onda_mir::Function {
            name: self.emitted_name,
            kind: onda_mir::FunctionKind::User,
            attributes: if self.runtime_globals.is_some() {
                compiler_generated_function_attributes()
            } else {
                source_function_attributes(&self.function.name)
            },
            params: self.params,
            results: self.results,
            locals: self.locals,
            body,
            source,
        })
    }
}
