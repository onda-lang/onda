use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn lower_oversampled_proc_step(
        &mut self,
        meta: &ProcStepOversampleMeta,
        factor: usize,
        destination: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        let location = function_location(self.function);
        if factor <= 1 || !factor.is_power_of_two() {
            return Err(self.error(
                format!(
                    "processor step '{}' has invalid oversampling factor {factor}",
                    self.function.name
                ),
                location,
            ));
        }
        if self.function.returns_value {
            return Err(self.error(
                format!(
                    "oversampled processor step '{}' unexpectedly returns a value",
                    self.function.name
                ),
                location,
            ));
        }
        let expected_stages = factor.trailing_zeros() as usize;
        let factor_u32 = u32::try_from(factor).map_err(|_| {
            self.error(
                format!(
                    "processor step '{}' oversampling factor {factor} does not fit u32",
                    self.function.name
                ),
                location,
            )
        })?;
        let factor_i32 = i32::try_from(factor).map_err(|_| {
            self.error(
                format!(
                    "processor step '{}' oversampling factor {factor} exceeds the MIR index boundary",
                    self.function.name
                ),
                location,
            )
        })?;

        let mut input_names = meta.input_state_fields.keys().cloned().collect::<Vec<_>>();
        input_names.sort();
        let mut inputs = Vec::with_capacity(input_names.len());
        for name in input_names {
            let (parameter_local, ty) = match self.bindings.get(&name).cloned() {
                Some(Binding::Local(local, ty)) => (local, ty),
                _ => {
                    return Err(self.error(
                        format!(
                            "processor step '{}' is missing scalar input parameter '{name}'",
                            self.function.name
                        ),
                        location,
                    ));
                }
            };
            let raw = self.new_local(Some(format!("$oversample.{name}.raw")), ty);
            self.assign_value(destination, raw, Value::Local(parameter_local), location);
            let stage_meta = &meta.input_state_fields[&name].up_stages;
            let values = if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
                if stage_meta.len() != expected_stages {
                    return Err(self.error(
                        format!(
                            "processor step '{}' input '{name}' has {} interpolation stages, expected {expected_stages}",
                            self.function.name,
                            stage_meta.len()
                        ),
                        location,
                    ));
                }
                let values = self.new_array_local(
                    Some(format!("$oversample.{name}.values")),
                    ty,
                    factor_u32,
                );
                let stages = stage_meta
                    .iter()
                    .map(|stage| self.proc_sinc_stage_places(stage, ty, location))
                    .collect::<Result<Vec<_>, _>>()?;
                self.emit_interpolation_stages(
                    destination,
                    ty,
                    Value::Local(raw),
                    values,
                    &stages,
                    factor,
                    location,
                );
                Some(values)
            } else {
                if !stage_meta.is_empty() {
                    return Err(self.error(
                        format!(
                            "processor step '{}' non-floating input '{name}' unexpectedly has interpolation state",
                            self.function.name
                        ),
                        location,
                    ));
                }
                None
            };
            inputs.push((
                parameter_local,
                OversampledInputRuntime {
                    ty,
                    raw,
                    values,
                    current: None,
                },
            ));
        }

        let mut output_names = meta.output_state_fields.keys().cloned().collect::<Vec<_>>();
        output_names.sort();
        let mut outputs = Vec::with_capacity(output_names.len());
        for name in output_names {
            let binding_name = format!("self.{name}");
            let (place, ty) = self.reference_binding_place(&binding_name, location)?;
            let stage_meta = &meta.output_state_fields[&name].down_stages;
            let down_stages = if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
                if stage_meta.len() != expected_stages {
                    return Err(self.error(
                        format!(
                            "processor step '{}' output '{name}' has {} decimation stages, expected {expected_stages}",
                            self.function.name,
                            stage_meta.len()
                        ),
                        location,
                    ));
                }
                stage_meta
                    .iter()
                    .map(|stage| self.proc_sinc_stage_places(stage, ty, location))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                if !stage_meta.is_empty() {
                    return Err(self.error(
                        format!(
                            "processor step '{}' non-floating output '{name}' unexpectedly has decimation state",
                            self.function.name
                        ),
                        location,
                    ));
                }
                Vec::new()
            };
            let values =
                self.new_array_local(Some(format!("$oversample.{name}.values")), ty, factor_u32);
            outputs.push(OversampledOutputRuntime {
                ty,
                destination: OversampledOutputDestination::Place(place),
                values,
                down_stages,
            });
        }

        // Keep the oversample schedule explicit in MIR. Statically cloning a
        // substantial DSP body `factor` times makes every backend inherit one
        // frontend unroll decision and can multiply code size dramatically.
        // Initialize the output scratch arrays up front so validation does not
        // need to prove that a dynamic loop index visits every element.
        for output in &outputs {
            for index in 0..factor_i32 {
                self.store_local_array_value(
                    destination,
                    output.values,
                    Value::Constant(ScalarValue::I32(index)),
                    zero_value(output.ty),
                    location,
                );
            }
        }

        let substep = self.new_local(Some("$oversample.substep".to_owned()), PrimitiveType::I32);
        let mut substep_body = MirBlock::default();
        let index = Value::Local(substep);
        for (parameter_local, input) in &inputs {
            let value = input.values.map_or(Value::Local(input.raw), |values| {
                self.load_local_array_value(&mut substep_body, values, index, input.ty, location)
            });
            self.assign_value(&mut substep_body, *parameter_local, value, location);
        }
        self.lower_statements(&self.function.body, &mut substep_body, ContinueMode::None)?;
        for output in &outputs {
            let OversampledOutputDestination::Place(place) = &output.destination else {
                unreachable!("processor oversampling outputs target processor state")
            };
            let value = self.load_place_value(&mut substep_body, output.ty, place, location);
            self.store_local_array_value(&mut substep_body, output.values, index, value, location);
        }
        self.emit_counted_loop(destination, substep, factor_i32, substep_body, location);

        for output in &outputs {
            let decimated = self.emit_decimation_stages(output, factor, destination, location);
            let OversampledOutputDestination::Place(place) = &output.destination else {
                unreachable!("processor oversampling outputs target processor state")
            };
            self.assign_place_value(destination, place.clone(), decimated, location);
        }
        Ok(())
    }

    pub(super) fn emit_counted_loop(
        &mut self,
        destination: &mut MirBlock,
        counter: LocalId,
        limit: i32,
        iteration_body: MirBlock,
        location: SourceLoc,
    ) {
        self.emit_strided_loop(destination, counter, limit, 1, iteration_body, location);
    }

    pub(super) fn emit_strided_loop(
        &mut self,
        destination: &mut MirBlock,
        counter: LocalId,
        limit: i32,
        step: i32,
        mut iteration_body: MirBlock,
        location: SourceLoc,
    ) {
        debug_assert!(limit > 0);
        debug_assert!(step > 0);
        self.assign_value(
            destination,
            counter,
            Value::Constant(ScalarValue::I32(0)),
            location,
        );
        let mut loop_body = MirBlock::default();
        let in_range = self.compare_value(
            &mut loop_body,
            CompareOp::Less,
            Value::Local(counter),
            Value::Constant(ScalarValue::I32(limit)),
            location,
        );
        let next_counter = self.emit_binary_value(
            &mut iteration_body,
            PrimitiveType::I32,
            MirBinaryOp::Add,
            Value::Local(counter),
            Value::Constant(ScalarValue::I32(step)),
            location,
        );
        self.assign_value(&mut iteration_body, counter, next_counter, location);
        let mut finished = MirBlock::default();
        self.push_statement(&mut finished, StatementKind::Break, location);
        self.push_statement(
            &mut loop_body,
            StatementKind::If {
                condition: in_range,
                then_block: iteration_body,
                else_block: finished,
            },
            location,
        );
        self.push_statement(
            destination,
            StatementKind::Loop { body: loop_body },
            location,
        );
    }

    pub(super) fn reference_binding_place(
        &self,
        name: &str,
        location: SourceLoc,
    ) -> Result<(Place, PrimitiveType), MirLoweringError> {
        match self.bindings.get(name) {
            Some(Binding::ReferenceParameter(parameter, ty)) => Ok((
                Place {
                    base: PlaceBase::Parameter(*parameter),
                    projections: Vec::new(),
                },
                *ty,
            )),
            _ => Err(self.error(
                format!(
                    "processor step '{}' is missing scalar state field '{name}'",
                    self.function.name
                ),
                location,
            )),
        }
    }

    pub(super) fn proc_sinc_stage_places(
        &self,
        stage: &ProcSincStageStateFields,
        expected_ty: PrimitiveType,
        location: SourceLoc,
    ) -> Result<[Place; 8], MirLoweringError> {
        let names = [
            &stage.a0, &stage.a1, &stage.a2, &stage.a3, &stage.b0, &stage.b1, &stage.b2, &stage.b3,
        ];
        let mut places = Vec::with_capacity(8);
        for name in names {
            let binding_name = format!("self.{name}");
            let (place, ty) = self.reference_binding_place(&binding_name, location)?;
            if ty != expected_ty {
                return Err(self.error(
                    format!(
                        "processor step '{}' filter state '{binding_name}' has type {}, expected {}",
                        self.function.name,
                        ty.name(),
                        expected_ty.name()
                    ),
                    location,
                ));
            }
            places.push(place);
        }
        places.try_into().map_err(|_| {
            self.error(
                format!(
                    "processor step '{}' filter stage does not contain eight state taps",
                    self.function.name
                ),
                location,
            )
        })
    }

    pub(super) fn lower_process(
        mut self,
        block_pre: &[Stmt],
        sample: &[Stmt],
        block_post: &[Stmt],
        _block_size: u32,
        sample_oversample_factor: usize,
    ) -> Result<onda_mir::Function, MirLoweringError> {
        let mut body = MirBlock::default();
        let process_location = block_pre
            .first()
            .or_else(|| sample.first())
            .or_else(|| block_post.first())
            .map(Stmt::loc)
            .unwrap_or(SourceLoc::ZERO);
        let load_process_param = |index: usize| {
            Rvalue::Load(Place {
                base: PlaceBase::Parameter(ParameterId::new(index as u32)),
                projections: Vec::new(),
            })
        };
        let _start_frame = self.emit_temp(
            &mut body,
            PrimitiveType::I32,
            load_process_param(onda_mir::PROCESS_START_FRAME_PARAM_INDEX),
            process_location,
        );
        let frames = self.emit_temp(
            &mut body,
            PrimitiveType::I32,
            load_process_param(onda_mir::PROCESS_FRAMES_PARAM_INDEX),
            process_location,
        );
        let flags = self.emit_temp(
            &mut body,
            PrimitiveType::I32,
            load_process_param(onda_mir::PROCESS_FLAGS_PARAM_INDEX),
            process_location,
        );

        let begin_bits = self.emit_temp(
            &mut body,
            PrimitiveType::I32,
            Rvalue::Binary {
                op: MirBinaryOp::BitAnd,
                lhs: flags.value,
                rhs: Value::Constant(ScalarValue::I32(onda_mir::PROCESS_BEGIN_BLOCK)),
            },
            process_location,
        );
        let begin = self.compare_value(
            &mut body,
            CompareOp::NotEqual,
            begin_bits.value,
            Value::Constant(ScalarValue::I32(0)),
            process_location,
        );
        let mut block_pre_body = MirBlock::default();
        self.lower_statements(block_pre, &mut block_pre_body, ContinueMode::None)?;
        self.push_statement(
            &mut body,
            StatementKind::If {
                condition: begin,
                then_block: block_pre_body,
                else_block: MirBlock::default(),
            },
            process_location,
        );

        let frame = self.new_local(Some("$segment.frame".to_owned()), PrimitiveType::I32);
        self.assign_value(
            &mut body,
            frame,
            Value::Constant(ScalarValue::I32(0)),
            process_location,
        );

        let mut loop_body = MirBlock::default();
        let frame_in_range = self.compare_value(
            &mut loop_body,
            CompareOp::Less,
            Value::Local(frame),
            frames.value,
            process_location,
        );
        let mut sample_block = MirBlock::default();
        let logical_frame = self.new_local(
            Some("$segment.logical_frame".to_owned()),
            PrimitiveType::I32,
        );
        self.push_statement(
            &mut sample_block,
            StatementKind::Assign {
                destination: Place::local(logical_frame),
                value: Rvalue::ProcessFrame {
                    offset: Value::Local(frame),
                },
            },
            process_location,
        );
        self.current_frame = Some(Value::Local(logical_frame));
        let sample_result = if sample_oversample_factor.max(1) > 1 {
            self.lower_top_level_oversampled_sample(
                sample,
                &mut sample_block,
                logical_frame,
                sample_oversample_factor.max(1),
                process_location,
            )
        } else {
            self.begin_audio_output_frame(&mut sample_block, process_location)?;
            let result = self
                .lower_statements(sample, &mut sample_block, ContinueMode::None)
                .and_then(|()| {
                    self.commit_audio_output_frame(
                        &mut sample_block,
                        Value::Local(logical_frame),
                        process_location,
                    )
                });
            self.clear_audio_output_caches();
            result
        };
        self.current_frame = None;
        sample_result?;
        self.push_statement(
            &mut sample_block,
            StatementKind::Assign {
                destination: Place::local(frame),
                value: Rvalue::Binary {
                    op: MirBinaryOp::Add,
                    lhs: Value::Local(frame),
                    rhs: Value::Constant(ScalarValue::I32(1)),
                },
            },
            process_location,
        );
        let mut finished = MirBlock::default();
        self.push_statement(&mut finished, StatementKind::Break, process_location);
        self.push_statement(
            &mut loop_body,
            StatementKind::If {
                condition: frame_in_range,
                then_block: sample_block,
                else_block: finished,
            },
            process_location,
        );
        self.push_statement(
            &mut body,
            StatementKind::Loop { body: loop_body },
            process_location,
        );

        let end_bits = self.emit_temp(
            &mut body,
            PrimitiveType::I32,
            Rvalue::Binary {
                op: MirBinaryOp::BitAnd,
                lhs: flags.value,
                rhs: Value::Constant(ScalarValue::I32(onda_mir::PROCESS_END_BLOCK)),
            },
            process_location,
        );
        let end = self.compare_value(
            &mut body,
            CompareOp::NotEqual,
            end_bits.value,
            Value::Constant(ScalarValue::I32(0)),
            process_location,
        );
        let mut block_post_body = MirBlock::default();
        self.lower_statements(block_post, &mut block_post_body, ContinueMode::None)?;
        self.push_statement(
            &mut body,
            StatementKind::If {
                condition: end,
                then_block: block_post_body,
                else_block: MirBlock::default(),
            },
            process_location,
        );
        let source = self.source_span(process_location);
        let i32_type = intern_scalar_type(self.types, PrimitiveType::I32);
        Ok(onda_mir::Function {
            name: self.emitted_name,
            kind: onda_mir::FunctionKind::Process,
            attributes: compiler_generated_function_attributes(),
            params: onda_mir::process_function_params(i32_type),
            results: Vec::new(),
            locals: self.locals,
            body,
            source,
        })
    }

    pub(super) fn lower_top_level_oversampled_sample(
        &mut self,
        sample: &[Stmt],
        destination: &mut MirBlock,
        frame: LocalId,
        factor: usize,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let globals = self.runtime_globals.ok_or_else(|| {
            self.error(
                "top-level oversampling requires runtime interface metadata",
                location,
            )
        })?;
        let mut input_specs = globals
            .inputs
            .iter()
            .map(|(name, (input, ty))| {
                (
                    name.clone(),
                    *input,
                    None,
                    *ty,
                    globals
                        .top_level_oversampling
                        .inputs
                        .get(name)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        for (name, (input, ty, len)) in &globals.input_arrays {
            for element in 0..*len {
                let surface = format!("{name}[{element}]");
                input_specs.push((
                    surface.clone(),
                    *input,
                    Some(element),
                    *ty,
                    globals
                        .top_level_oversampling
                        .inputs
                        .get(&surface)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        input_specs.sort_by_key(|(_, input, element, _, _)| (input.raw(), element.unwrap_or(0)));
        let mut output_specs = globals
            .outputs
            .iter()
            .map(|(name, (output, ty))| {
                (
                    name.clone(),
                    *output,
                    None,
                    *ty,
                    globals
                        .top_level_oversampling
                        .outputs
                        .get(name)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        for (name, (output, ty, len)) in &globals.output_arrays {
            for element in 0..*len {
                let surface = format!("{name}[{element}]");
                output_specs.push((
                    surface.clone(),
                    *output,
                    Some(element),
                    *ty,
                    globals
                        .top_level_oversampling
                        .outputs
                        .get(&surface)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        output_specs.sort_by_key(|(_, output, element, _, _)| (output.raw(), element.unwrap_or(0)));
        let mut input_array_caches = globals
            .input_arrays
            .iter()
            .map(|(name, (input, ty, len))| (name.clone(), *input, *ty, *len))
            .collect::<Vec<_>>();
        input_array_caches.sort_by_key(|(_, input, _, _)| input.raw());
        let mut output_array_caches = globals
            .output_arrays
            .iter()
            .map(|(name, (output, ty, len))| (name.clone(), *output, *ty, *len))
            .collect::<Vec<_>>();
        output_array_caches.sort_by_key(|(_, output, _, _)| output.raw());

        let factor_u32 = u32::try_from(factor).map_err(|_| {
            self.error(
                format!("top-level oversampling factor {factor} does not fit u32"),
                location,
            )
        })?;
        let factor_i32 = i32::try_from(factor).map_err(|_| {
            self.error(
                format!("top-level oversampling factor {factor} exceeds the MIR index boundary"),
                location,
            )
        })?;
        for (name, input, ty, len) in input_array_caches {
            let cache =
                self.new_array_local(Some(format!("$oversample.input.{name}.current")), ty, len);
            self.oversampled_input_arrays
                .insert(input, (cache, ty, len));
        }
        for (name, output, ty, len) in output_array_caches {
            let cache =
                self.new_array_local(Some(format!("$oversample.output.{name}.current")), ty, len);
            self.audio_output_array_caches
                .insert(output, (cache, ty, len));
        }
        let mut inputs = Vec::with_capacity(input_specs.len());
        for (name, input, element, ty, stages) in input_specs {
            let raw = self.new_local(Some(format!("$oversample.input.{name}.raw")), ty);
            self.push_statement(
                destination,
                StatementKind::Assign {
                    destination: Place::local(raw),
                    value: Rvalue::InputLoad {
                        input,
                        element: element
                            .map(|element| Value::Constant(ScalarValue::I32(element as i32))),
                        bounds: BoundsMode::Unchecked,
                        frame: Value::Local(frame),
                    },
                },
                location,
            );
            let values = if stages.is_empty() {
                None
            } else {
                let values = self.new_array_local(
                    Some(format!("$oversample.input.{name}.values")),
                    ty,
                    factor_u32,
                );
                self.emit_interpolation_stages(
                    destination,
                    ty,
                    Value::Local(raw),
                    values,
                    &stages.iter().map(mir_sinc_stage_places).collect::<Vec<_>>(),
                    factor,
                    location,
                );
                Some(values)
            };
            let current = if let Some(element) = element {
                let (cache, _, _) = self.oversampled_input_arrays[&input];
                Place {
                    base: PlaceBase::Local(cache),
                    projections: vec![Projection::Index {
                        index: Value::Constant(ScalarValue::I32(element as i32)),
                        bounds: BoundsMode::Unchecked,
                    }],
                }
            } else {
                let cache = self.new_local(Some(format!("$oversample.input.{name}.current")), ty);
                self.oversampled_inputs.insert(name.clone(), (cache, ty));
                self.oversampled_input_endpoints.insert(input, (cache, ty));
                Place::local(cache)
            };
            inputs.push(OversampledInputRuntime {
                ty,
                raw,
                values,
                current: Some(current),
            });
        }

        let mut outputs = Vec::with_capacity(output_specs.len());
        for (name, output, element, ty, stages) in output_specs {
            let current = if let Some(element) = element {
                let (cache, _, _) = self.audio_output_array_caches[&output];
                Place {
                    base: PlaceBase::Local(cache),
                    projections: vec![Projection::Index {
                        index: Value::Constant(ScalarValue::I32(element as i32)),
                        bounds: BoundsMode::Unchecked,
                    }],
                }
            } else {
                let current =
                    self.new_local(Some(format!("$oversample.output.{name}.current")), ty);
                self.audio_output_caches.insert(name.clone(), (current, ty));
                self.audio_output_endpoint_caches
                    .insert(output, (current, ty));
                Place::local(current)
            };
            let values = self.new_array_local(
                Some(format!("$oversample.output.{name}.values")),
                ty,
                factor_u32,
            );
            outputs.push(OversampledOutputRuntime {
                ty,
                destination: OversampledOutputDestination::Interface {
                    output,
                    element,
                    current,
                },
                values,
                down_stages: stages.iter().map(mir_sinc_stage_places).collect(),
            });
        }

        // Preserve the fixed oversampling schedule as one counted MIR loop so
        // each backend can make its own target-aware unroll decision. The
        // scratch arrays start fully initialized because validation cannot
        // infer that a dynamic loop index visits every element.
        for output in &outputs {
            for index in 0..factor_i32 {
                self.store_local_array_value(
                    destination,
                    output.values,
                    Value::Constant(ScalarValue::I32(index)),
                    zero_value(output.ty),
                    location,
                );
            }
        }

        let host_config = self.config;
        self.config.sample_rate = self.host_config.sample_rate * factor as f32;
        let substep = self.new_local(Some("$oversample.substep".to_owned()), PrimitiveType::I32);
        let mut substep_body = MirBlock::default();
        let index = Value::Local(substep);
        for input in &inputs {
            let value = input.values.map_or(Value::Local(input.raw), |values| {
                self.load_local_array_value(&mut substep_body, values, index, input.ty, location)
            });
            let current = input
                .current
                .clone()
                .expect("top-level oversampling inputs have current cache places");
            self.assign_place_value(&mut substep_body, current, value, location);
        }
        for output in &outputs {
            let OversampledOutputDestination::Interface { current, .. } = &output.destination
            else {
                unreachable!("top-level oversampling outputs target interface cache places")
            };
            self.assign_place_value(
                &mut substep_body,
                current.clone(),
                zero_value(output.ty),
                location,
            );
        }

        if let Err(error) = self.lower_statements(sample, &mut substep_body, ContinueMode::None) {
            self.config = host_config;
            self.oversampled_inputs.clear();
            self.audio_output_caches.clear();
            self.oversampled_input_endpoints.clear();
            self.audio_output_endpoint_caches.clear();
            self.oversampled_input_arrays.clear();
            self.audio_output_array_caches.clear();
            return Err(error);
        }
        for output in &outputs {
            let OversampledOutputDestination::Interface { current, .. } = &output.destination
            else {
                unreachable!("top-level oversampling outputs target interface cache places")
            };
            let current_value =
                self.load_place_value(&mut substep_body, output.ty, current, location);
            self.store_local_array_value(
                &mut substep_body,
                output.values,
                index,
                current_value,
                location,
            );
        }
        self.emit_counted_loop(destination, substep, factor_i32, substep_body, location);

        self.config = host_config;
        self.oversampled_inputs.clear();
        self.audio_output_caches.clear();
        self.oversampled_input_endpoints.clear();
        self.audio_output_endpoint_caches.clear();
        self.oversampled_input_arrays.clear();
        self.audio_output_array_caches.clear();

        for output in &outputs {
            let decimated = self.emit_decimation_stages(output, factor, destination, location);
            let OversampledOutputDestination::Interface {
                output: output_id,
                element,
                ..
            } = output.destination
            else {
                unreachable!("top-level oversampling outputs target the program interface")
            };
            self.push_statement(
                destination,
                StatementKind::OutputStore {
                    output: output_id,
                    element: element
                        .map(|element| Value::Constant(ScalarValue::I32(element as i32))),
                    bounds: BoundsMode::Unchecked,
                    frame: Value::Local(frame),
                    value: decimated,
                },
                location,
            );
        }
        Ok(())
    }
}
