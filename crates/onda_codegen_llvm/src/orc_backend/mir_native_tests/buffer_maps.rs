use super::*;

fn mapped_buffer_program() -> Program {
    let (_, mut program) = source_program(
        r#"
buffers:
  bank: f32 {4}
outs:
  out1
proc Child:
  params:
    slot: i32 = 1
  buffers:
    clips: f32 {2}
  outs:
    out1
  sample:
    clips[slot][0] = clips[slot][0] + 1.0
    out1 = clips[slot][0]
proc Parent:
  buffers:
    clips: f32 {3}
  init:
    child = Child(clips = clips[1:3])
  outs:
    out1
  sample:
    out1 = child()
init:
  parent = Parent(clips = bank[1:4])
sample:
  out1 = parent()
"#,
        1,
    );
    for ty in &mut program.types {
        if let Type::BufferSpan {
            element,
            channels,
            access,
            len,
        } = *ty
        {
            *ty = Type::BufferMap {
                element,
                channels,
                access,
                len,
            };
        }
    }
    // The selected resources are compatible despite an unrelated descriptor
    // interleaved between them in the interface table.
    program.interface.buffers[1].element = onda_mir::ScalarType::I32;
    program.interface.buffer_arrays.clear();
    program.buffer_maps.push(onda_mir::BufferMap {
        buffers: [0, 2, 0, 3]
            .into_iter()
            .map(onda_mir::BufferId::new)
            .collect(),
    });
    fn rewrite_argument(argument: &mut CallArgument) {
        if let CallArgument::BufferSpan(onda_mir::BufferSpanRef::Interface { first, len }) =
            argument
        {
            *argument = CallArgument::BufferSpan(onda_mir::BufferSpanRef::MappedInterface {
                map: onda_mir::BufferMapId::new(0),
                start: first.raw(),
                len: *len,
            });
        }
    }
    fn rewrite(block: &mut onda_mir::Block) {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::Call { args, .. } => {
                    for argument in args {
                        rewrite_argument(argument);
                    }
                }
                StatementKind::Assign {
                    value: Rvalue::MakeBuffer { source, .. },
                    ..
                } => rewrite_argument(source),
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    rewrite(then_block);
                    rewrite(else_block);
                }
                StatementKind::Loop { body } => rewrite(body),
                _ => {}
            }
        }
    }
    for function in &mut program.functions {
        rewrite(&mut function.body);
    }
    program
}

#[test]
fn immutable_resource_maps_forward_subviews_and_observe_rebinding() {
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let program = mapped_buffer_program();
        assert_eq!(
            onda_mir::analyze_buffer_writes(&program).unwrap().buffers(),
            &[true, false, false, true]
        );
        let native = lower_mir_and_jit_with_options(
            program,
            MirCompileOptions {
                math: Default::default(),
                fast_math: false,
                opt_level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut bank = [[1.0_f32], [2.0], [3.0], [4.0]];
        let mut buffers = bank
            .iter_mut()
            .map(|entry| entry.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let frames = [1_i32; 4];
        let channels = [1_i32; 4];
        let sample_rates = [48_000.0_f32; 4];
        let mut output = [0.0_f32];
        let outputs = [output.as_mut_ptr().cast::<u8>()];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESSOR_FULL_BLOCK as u32,
                &[],
                &outputs,
                &buffers,
                &frames,
                &channels,
                &sample_rates,
            )
            .unwrap();
        assert_eq!(output, [5.0]);
        assert_eq!(bank[0], [1.0]);
        let mut rebound = [20.0_f32];
        buffers[3] = rebound.as_mut_ptr().cast();
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESSOR_FULL_BLOCK as u32,
                &[],
                &outputs,
                &buffers,
                &frames,
                &channels,
                &sample_rates,
            )
            .unwrap();
        assert_eq!(output, [21.0]);
        assert_eq!(bank[3], [5.0]);
        assert_eq!(rebound, [21.0]);
    }
}

#[test]
fn local_resource_descriptors_keep_binding_origins_and_sample_writes() {
    let mut program = mapped_buffer_program();
    let mut functions = std::mem::take(&mut program.functions);
    for function in &mut functions {
        let parameters = function.params.clone();
        for (index, parameter) in parameters.iter().enumerate() {
            let Some((_, _, _, len, _)) = program.types[parameter.ty.index()].buffer_collection()
            else {
                continue;
            };
            let local = onda_mir::LocalId::new(function.locals.len() as u32);
            function.locals.push(onda_mir::Local {
                name: None,
                ty: parameter.ty,
                integer_range: None,
            });
            function.body.statements.insert(
                0,
                onda_mir::Statement {
                    source: onda_mir::SourceSpan::UNKNOWN,
                    kind: StatementKind::Assign {
                        destination: Place::local(local),
                        value: Rvalue::MakeBuffer {
                            source: CallArgument::BufferSpan(onda_mir::BufferSpanRef::Parameter {
                                span: onda_mir::ParameterId::new(index as u32),
                                start: 0,
                                len,
                            }),
                            ty: parameter.ty,
                        },
                    },
                },
            );
            fn rewrite(block: &mut onda_mir::Block, parameter: usize, local: onda_mir::LocalId) {
                for statement in &mut block.statements {
                    match &mut statement.kind {
                        StatementKind::Call { args, .. } => {
                            for arg in args {
                                if let CallArgument::BufferSpan(
                                    onda_mir::BufferSpanRef::Parameter { span, start, len },
                                ) = arg
                                {
                                    if span.index() == parameter {
                                        *arg = CallArgument::BufferSpan(
                                            onda_mir::BufferSpanRef::Local {
                                                collection: local,
                                                start: onda_mir::Value::Constant(
                                                    onda_mir::ScalarValue::I32(*start as i32),
                                                ),
                                                len: *len,
                                                bounds: onda_mir::BoundsMode::Checked,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        StatementKind::BufferParamStore {
                            parameter: reference,
                            ..
                        }
                        | StatementKind::Assign {
                            value:
                                Rvalue::BufferParamLoad {
                                    parameter: reference,
                                    ..
                                },
                            ..
                        } => {
                            if let onda_mir::BufferParamRef::ArrayElement {
                                span,
                                selector,
                                bounds,
                            } = reference
                            {
                                if span.index() == parameter {
                                    *reference = onda_mir::BufferParamRef::LocalElement {
                                        collection: local,
                                        selector: *selector,
                                        bounds: *bounds,
                                    };
                                }
                            }
                        }
                        StatementKind::If {
                            then_block,
                            else_block,
                            ..
                        } => {
                            rewrite(then_block, parameter, local);
                            rewrite(else_block, parameter, local);
                        }
                        StatementKind::Loop { body } => rewrite(body, parameter, local),
                        _ => {}
                    }
                }
            }
            // Leave the first materialization rooted in the original parameter.
            let first = function.body.statements.remove(0);
            rewrite(&mut function.body, index, local);
            function.body.statements.insert(0, first);
        }
    }
    program.functions = functions;
    assert_eq!(
        onda_mir::analyze_buffer_writes(&program).unwrap().buffers(),
        &[true, false, false, true]
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            program.clone(),
            MirCompileOptions {
                math: Default::default(),
                fast_math: false,
                opt_level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut bank = [[1.0_f32], [2.0], [3.0], [4.0]];
        let buffers = bank
            .iter_mut()
            .map(|entry| entry.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let mut output = [0.0_f32];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESSOR_FULL_BLOCK as u32,
                &[],
                &[output.as_mut_ptr().cast()],
                &buffers,
                &[1; 4],
                &[1; 4],
                &[48_000.0; 4],
            )
            .unwrap();
        assert_eq!(output, [5.0]);
    }
}

#[test]
fn finite_span_choices_forward_without_gathering_and_observe_rebinding() {
    let (_, program) = source_program(
        include_str!(
            "../../../../../packages/onda_binaryen_web/test/fixtures/processor-view-resource-spans.onda"
        ),
        4,
    );
    assert!(program.buffer_maps.is_empty());
    assert_rebound_buffer_program(program, [[3.0, 7.0, 3.0, 7.0], [3.0, 23.0, 3.0, 23.0]]);
}

#[test]
fn parameter_collection_elements_keep_typed_choices_when_forwarded() {
    let (_, program) = source_program(
        include_str!(
            "../../../../../packages/onda_binaryen_web/test/fixtures/processor-view-resource-elements.onda"
        ),
        4,
    );
    assert_eq!(
        onda_mir::analyze_buffer_writes(&program).unwrap().buffers(),
        &[false, true, false, true]
    );
    assert_rebound_buffer_program(program, [[3.0, 5.0, 4.0, 6.0], [5.0, 21.0, 6.0, 22.0]]);
}

fn assert_rebound_buffer_program(program: Program, expected_blocks: [[f32; 4]; 2]) {
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            program.clone(),
            MirCompileOptions {
                math: Default::default(),
                fast_math: false,
                opt_level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut bank = [[1.0_f32], [2.0], [3.0], [4.0]];
        let mut buffers = bank
            .iter_mut()
            .map(|entry| entry.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let mut replacement = [20.0_f32];
        let frames = [1_i32; 4];
        let channels = [1_i32; 4];
        let sample_rates = [48_000.0_f32; 4];
        let mut output = [0.0_f32; 4];
        let outputs = [output.as_mut_ptr().cast::<u8>()];
        for (round, expected) in expected_blocks.into_iter().enumerate() {
            if round == 1 {
                buffers[3] = replacement.as_mut_ptr().cast();
            }
            native
                .test_process_checked(
                    &mut state,
                    &params,
                    0,
                    4,
                    onda_mir::PROCESSOR_FULL_BLOCK as u32,
                    &[],
                    &outputs,
                    &buffers,
                    &frames,
                    &channels,
                    &sample_rates,
                )
                .unwrap();
            assert_eq!(output, expected, "{opt_level:?}");
        }
    }
}

#[test]
fn deep_owner_callbacks_use_each_owners_current_resource_bindings() {
    let (_, program) = source_program(
        include_str!(
            "../../../../../packages/onda_binaryen_web/test/fixtures/processor-view-deep-resources.onda"
        ),
        4,
    );
    assert_eq!(
        onda_mir::analyze_buffer_writes(&program).unwrap().buffers(),
        &[true; 7]
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            program.clone(),
            MirCompileOptions {
                math: Default::default(),
                fast_math: false,
                opt_level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut original = [[0.0_f32]; 7];
        let mut buffers = original
            .iter_mut()
            .map(|entry| entry.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let mut replacement = [
            [100.0_f32],
            [100.0],
            [100.0],
            [100.0],
            [10.0],
            [20.0],
            [1000.0],
        ];
        let mut output = [0.0_f32; 4];
        for (round, expected) in [[2.0, 22.0, 31.0, 121.0], [1012.0, 1232.0, 1251.0, 1541.0]]
            .into_iter()
            .enumerate()
        {
            if round == 1 {
                buffers = replacement
                    .iter_mut()
                    .map(|entry| entry.as_mut_ptr().cast::<u8>())
                    .collect();
            }
            native
                .test_process_checked(
                    &mut state,
                    &params,
                    0,
                    4,
                    onda_mir::PROCESSOR_FULL_BLOCK as u32,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &buffers,
                    &[1; 7],
                    &[1; 7],
                    &[48_000.0; 7],
                )
                .unwrap();
            assert_eq!(output, expected, "{opt_level:?}, round {round}");
        }
        assert_eq!(
            original.map(|entry| entry[0]),
            [2.0, 7.0, 2.0, 7.0, 9.0, 9.0, 121.0]
        );
        assert_eq!(
            replacement.map(|entry| entry[0]),
            [102.0, 107.0, 102.0, 107.0, 19.0, 29.0, 1541.0]
        );
    }
}

#[test]
fn local_child_borrows_keep_owner_resources_when_buffer_payload_names_shadow_them() {
    let (_, program) = source_program(
        r#"
proc Voice:
  buffers:
    source: f32
  sample:
    source[0] += 1.0
    out1 = source[0]
def step(voice) -> f32:
  return voice()
proc Bank:
  buffers:
    source: f32
    payload: f32
  init:
    voice = Voice(source = source)
  def exercise(source: buffer<f32>) -> f32:
    selected = voice
    child = step(selected)
    source[0] += 100.0
    return child + source[0]
  sample:
    out1 = exercise(payload)
buffers:
  owned: f32
  payload: f32
init:
  bank = Bank(source = owned, payload = payload)
sample:
  out1 = bank()
"#,
        4,
    );
    assert_eq!(
        onda_mir::analyze_buffer_writes(&program).unwrap().buffers(),
        &[true, true]
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            program.clone(),
            MirCompileOptions {
                math: Default::default(),
                fast_math: false,
                opt_level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut owned = [1.0_f32];
        let mut payload = [10.0_f32];
        let mut output = [0.0_f32; 4];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                4,
                onda_mir::PROCESSOR_FULL_BLOCK as u32,
                &[],
                &[output.as_mut_ptr().cast()],
                &[owned.as_mut_ptr().cast(), payload.as_mut_ptr().cast()],
                &[1; 2],
                &[1; 2],
                &[48_000.0; 2],
            )
            .unwrap();
        assert_eq!(output, [112.0, 213.0, 314.0, 415.0], "{opt_level:?}");
        assert_eq!(owned, [5.0]);
        assert_eq!(payload, [410.0]);
    }
}
