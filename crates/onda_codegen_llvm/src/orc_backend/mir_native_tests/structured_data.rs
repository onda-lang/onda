use super::*;

#[test]
fn sequential_fixed_results_reuse_scratch_without_aliasing() {
    let source = r#"
def make(value: f32) -> f32[4096]:
  result: f32[4096]
  result[:] = value
  return result
sample:
  first = make(1.0)
  total = first[0]
  second = make(2.0)
  total += second[0]
  third = make(3.0)
  total += third[0]
  fourth = make(4.0)
  out1 = total + fourth[0]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(
            run_native_outputs_with_opt_level(source, 4, level)[0],
            [10.0; 4]
        );
    }
}

#[test]
fn empty_struct_slice_element_access_fails_without_touching_storage() {
    let sources = [
        r#"
struct Empty {}
sample:
  items: Empty[1]
  view: Empty[] = items[1:1]
  selected = view[0]
  out1 = 1.0
"#,
        r#"
struct Cell:
  value = 5.0
sample:
  items: Cell[1]
  view: Cell[] = items[1:1]
  selected = view[0]
  out1 = selected.value
"#,
    ];
    for source in sources {
        let (_, mir) = source_program(source, 1);
        for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let native = lower_mir_and_jit_with_options(
                mir.clone(),
                MirCompileOptions {
                    fast_math: false,
                    opt_level: level,
                },
            )
            .unwrap();
            let params = native.default_param_bytes();
            let mut state = native.initialize_state(&params).unwrap();
            let mut output = [0.0_f32];
            let outputs = [output.as_mut_ptr().cast::<u8>()];
            let inputs: [*const u8; 0] = [];
            let buffers: [*mut u8; 0] = [];
            let metadata_i32: [i32; 0] = [];
            let metadata_f32: [f32; 0] = [];
            assert!(native
                .test_process_checked(
                    &mut state,
                    &params,
                    0,
                    1,
                    onda_mir::PROCESS_FULL_BLOCK as u32,
                    &inputs,
                    &outputs,
                    &buffers,
                    &metadata_i32,
                    &metadata_i32,
                    &metadata_f32,
                )
                .is_err());
        }
    }
}

#[test]
fn retained_empty_struct_slices_preserve_their_logical_length() {
    let source = r#"
struct Empty {}
def make() -> Empty[3]:
  return [Empty(), Empty(), Empty()]
init:
  view: Empty[] = make()
sample:
  out1 = f32(view.len())
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(
            run_native_outputs_with_opt_level(source, 1, level)[0],
            [3.0]
        );
    }
}

#[test]
fn proc_helpers_create_owned_data_and_captured_aliases() {
    let source = r#"
struct Note:
  value = 1.0
  bins: f32[2]
def make() -> Note[2]:
  return [Note(value = 3.0), Note(value = 5.0)]
proc Worker:
  init:
    notes = make()
    selected = notes[1]
    saved: Note = selected
  def read(notes: f32):
    return selected.value + notes
  sample:
    selected.value += 1.0
    out1 = read(10.0) + saved.value
init:
  worker = Worker()
sample:
  out1 = worker()
"#;
    let nested = source.replace("init:\n  worker = Worker()", "proc Wrapper:\n  init:\n    children: Worker[2]\n  sample:\n    a = children[0]\n    b = children[1]\n    out1 = a() + b()\ninit:\n  worker = Wrapper()");
    for (source, scale) in [(source, 1.0), (nested.as_str(), 2.0)] {
        for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let output = run_native_outputs_with_opt_level(source, 4, level);
            assert_eq!(
                output[0],
                [21.0, 22.0, 23.0, 24.0].map(|value| value * scale)
            );
        }
    }
}

#[test]
fn raw_events_reject_wrapped_tensor_byte_sizes() {
    let (_, mir) = source_program(
        r#"
struct Huge:
  bins: f64[1514507160]
init:
  calls: i32 = 0
event ingest(items: Huge[]):
  calls += 1
sample:
  out1 = f32(calls)
"#,
        1,
    );
    // 1_522_503_868 * 1_514_507_160 == 2^61 + 928, so multiplying
    // by sizeof(f64) would wrap an unchecked u64 byte count to 7_424.
    let mut payload = vec![0; 4 + 7_424];
    payload[..4].copy_from_slice(&1_522_503_868_i32.to_le_bytes());
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let initial = state.bytes().to_vec();
        let status = unsafe {
            native.trigger_event_by_index_unchecked(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                None,
            )
        };
        assert_eq!(
            status,
            onda_processor_abi::PROCESSOR_EXECUTION_INPUT_REJECTED
        );
        assert_eq!(state.bytes(), initial);
    }
}

#[test]
fn proc_views_and_owned_data_belong_to_each_instance() {
    let source = r#"
struct Note:
  value = 1.0
  bins: f32[2]
proc Worker:
  init:
    notes: Note[4]
    index: i32 = 1
    initial = notes[index]
    fresh: f32[] = [2.0, 3.0]
  block:
    owned: Note[2] = Note(value = 4.0)
    selected = notes[index]
    kept: Note[] = owned[:]
    sample:
      index = 3
      selected.value += 1.0
      first = kept[0]
      out1 = initial.value + selected.value + first.value + fresh[0]
init:
  a = Worker()
  b = Worker()
sample:
  out1 = a() + b()
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 4, level);
        assert_eq!(output[0], vec![20.0, 24.0, 28.0, 32.0]);
    }
}

#[test]
fn task_views_survive_yield_events_and_instance_relocation() {
    let (_, mir) = source_program(
        r#"
struct Note:
  value = 1.0
init:
  notes: Note[4]
  index: i32 = 1
  choose = false
  result = 0.0
  initial: Note[] = notes[index:]
task prepare():
  owned: Note[2] = Note(value = 3.0)
  if choose:
    selected = notes[index]
  else:
    selected = owned[0]
  view: Note[] = initial
  fresh: f32[] = [2.0, 3.0]
  yield
  selected.value += 4.0
  fresh[0] = fresh[0] + 2.0
  first = view[0]
  result = selected.value + first.value + fresh[0]
event change():
  index = 3
  choose = true
  changed = notes[1]
  changed.value = 11.0
block:
  await prepare()
  sample:
    out1 = result
"#,
        1,
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let render = |state: &mut RuntimeState| {
            let mut output = [0.0_f32];
            native
                .test_process_checked(
                    state,
                    &params,
                    0,
                    1,
                    onda_mir::PROCESS_FULL_BLOCK as u32,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[],
                    &[],
                    &[],
                    &[],
                )
                .unwrap();
            output[0]
        };
        assert_eq!(render(&mut state), 0.0);
        let mut relocated = state.try_clone_with_allocator(None).unwrap();
        unsafe {
            native.trigger_event_by_index(&mut relocated, &params, 0, &[], &[], &[], &[], &[], None)
        }
        .unwrap();
        assert_eq!(render(&mut relocated), 22.0);
        assert_eq!(render(&mut state), 12.0);
    }
}

#[test]
fn init_views_relocate_and_reinitialize_with_their_backing_storage() {
    let (_, mir) = source_program(
        r#"
struct Note:
  value = 1.0
  bins: f32[2]
init:
  pin notes: Note[4]
  index: i32 = 1
  selected = notes[index]
  window: Note[] = notes[index:]
  fresh: f32[] = [2.0, 3.0]
event change():
  selected.value = 9.0
  fresh[0] = 7.0
sample:
  index = 3
  first = window[0]
  out1 = selected.value + first.value + fresh[0]
"#,
        1,
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let render = |state: &mut RuntimeState| {
            let mut output = [0.0_f32];
            native
                .test_process_checked(
                    state,
                    &params,
                    0,
                    1,
                    onda_mir::PROCESS_FULL_BLOCK as u32,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[],
                    &[],
                    &[],
                    &[],
                )
                .unwrap();
            output[0]
        };
        assert_eq!(render(&mut state), 4.0);
        // Both allocations remain live: reconstructed views must address the
        // relocated state, and mutation must not affect the original instance.
        let mut relocated = state.try_clone_with_allocator(None).unwrap();
        unsafe {
            native.trigger_event_by_index(&mut relocated, &params, 0, &[], &[], &[], &[], &[], None)
        }
        .unwrap();
        assert_eq!(render(&mut relocated), 25.0);
        assert_eq!(render(&mut state), 4.0);
        unsafe {
            native.initialize_state_in_place(
                &params,
                &mut relocated,
                false,
                BufferDescriptorTables::new(&[], &[], &[], &[]),
                None,
            )
        }
        .unwrap();
        assert_eq!(render(&mut relocated), 20.0);
        unsafe {
            native.initialize_state_in_place(
                &params,
                &mut relocated,
                true,
                BufferDescriptorTables::new(&[], &[], &[], &[]),
                None,
            )
        }
        .unwrap();
        assert_eq!(render(&mut relocated), 4.0);
    }
}

#[test]
fn retained_view_checks_restored_coordinates_before_address_formation() {
    let (_, mir) = source_program(
        r#"
init:
  index: i32 = 1
block:
  values = [10.0, 20.0]
  selected = values[index:]
  sample:
    out1 = selected[0]
"#,
        1,
    );
    let coordinate = mir
        .state
        .iter()
        .position(|slot| {
            slot.name.starts_with("__onda_block.selection")
                && matches!(
                    mir.types[slot.ty.index()],
                    onda_mir::Type::Scalar(ScalarType::I32)
                )
        })
        .unwrap();
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut output = [0.0_f32];
        let outputs = [output.as_mut_ptr().cast()];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESS_BEGIN_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(output, [20.0]);
        let offset = native.state_byte_offsets()[coordinate];
        // Simulate a modified snapshot without introducing invalid native pointers.
        (unsafe { state.bytes_mut() })[offset..offset + 4].copy_from_slice(&i32::MAX.to_ne_bytes());
        let error = native.test_process_checked(
            &mut state,
            &params,
            0,
            1,
            onda_mir::PROCESS_END_BLOCK as u32,
            &[],
            &outputs,
            &[],
            &[],
            &[],
            &[],
        );
        assert!(error.is_err());
        assert_eq!(output, [20.0]);
    }
}

#[test]
fn block_branch_selection_stays_live_across_events() {
    let (_, mir) = source_program(
        r#"
struct Note:
  value = 1.0
def make() -> f32[2]:
  return [2.0, 3.0]
init:
  left: Note[2] = Note(value = 10.0)
  right: Note[2] = Note(value = 20.0)
  choose: i32 = 0
  offset: i32 = 1
event alter():
  left[1] = Note(value = 30.0)
  choose = 1
  offset = 0
  scratch = make()
  scratch[0] = 99.0
block:
  if choose == 0:
    selected = left[offset]
  else:
    selected = right[offset]
  kept = make()
  sample:
    out1 = selected.value + kept[0]
"#,
        2,
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut output = [0.0_f32; 2];
        let outputs = [output.as_mut_ptr().cast()];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESS_BEGIN_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        // This event has no borrowed buffers, payload, or output batch.
        unsafe {
            native.trigger_event_by_index(&mut state, &params, 0, &[], &[], &[], &[], &[], None)
        }
        .unwrap();
        native
            .test_process_checked(
                &mut state,
                &params,
                1,
                1,
                onda_mir::PROCESS_END_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(output, [12.0, 32.0]);
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                2,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(output, [22.0, 22.0]);
    }
}

#[test]
fn block_owned_data_and_views_survive_segments_and_instance_relocation() {
    let (_, mir) = source_program(
        r#"
struct Note:
  value = 1.0
init:
  notes: Note[4]
  index: i32 = 1
block:
  selected = notes[index]
  owned = [2.0, 3.0]
  window: f32[] = owned[:]
  sample:
    index = 3
    selected.value = selected.value + 1.0
    window[0] = window[0] + 2.0
    out1 = selected.value + window[0]
  selected.value = selected.value + 10.0
"#,
        4,
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut output = [0.0_f32; 4];
        let outputs = [output.as_mut_ptr().cast()];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                2,
                onda_mir::PROCESS_BEGIN_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        // Retained selection descriptors must resolve against the new address.
        // Keep the original allocation live so stale pointers cannot appear valid.
        let mut relocated = state.try_clone_with_allocator(None).unwrap();
        native
            .test_process_checked(
                &mut relocated,
                &params,
                2,
                2,
                onda_mir::PROCESS_END_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(output, [6.0, 9.0, 12.0, 15.0]);
        output.fill(0.0);
        native
            .test_process_checked(
                &mut state,
                &params,
                2,
                2,
                onda_mir::PROCESS_END_BLOCK as u32,
                &[],
                &outputs,
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(output, [0.0, 0.0, 12.0, 15.0]);
    }
}

#[test]
fn struct_tensor_arguments_preserve_stride_through_mutable_and_readonly_helpers() {
    let (_, mut mir) = source_program(
        r#"
struct Frame:
  samples: f32[2]
def read(frame: Frame):
  return frame.samples[0] + frame.samples[1]
def update(frame: Frame):
  frame.samples[1] = frame.samples[1] + 10.0
  return read(frame)
def bridge(values: f32[]):
  values[0] = values[0]
  frame = Frame()
  return update(frame)
buffers:
  stereo: f32[2]
sample:
  out1 = bridge(stereo[1, :2])
"#,
        1,
    );
    let update = mir
        .functions
        .iter()
        .position(|function| function.name == "update")
        .unwrap();
    assert!(matches!(
        mir.types[mir.functions[update].params[0].ty.index()],
        onda_mir::Type::Slice { .. }
    ));
    let bridge = mir
        .functions
        .iter_mut()
        .find(|function| function.name == "bridge")
        .unwrap();
    let values = bridge
        .locals
        .iter()
        .position(|local| local.name.as_deref() == Some("values"))
        .unwrap();
    let call = bridge
        .body
        .statements
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Call { function, args, .. } if function.index() == update => Some(args),
            _ => None,
        })
        .unwrap();
    // Supply an external strided tensor using the aggregate leaf ABI. This
    // exercises descriptor forwarding independently of owned contiguous layout.
    call[0] = CallArgument::Value(Value::Local(onda_mir::LocalId::new(values as u32)));
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut values = [1.0_f32, 2.0, 3.0, 4.0];
        let mut output = [0.0_f32];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &[],
                &[output.as_mut_ptr().cast()],
                &[values.as_mut_ptr().cast()],
                &[2],
                &[2],
                &[48_000.0],
            )
            .unwrap();
        assert_eq!(values, [1.0, 2.0, 3.0, 14.0]);
        assert_eq!(output, [16.0]);
    }
}

#[test]
fn copy_groups_check_all_overlaps_before_writing_any_leaf() {
    let source = r#"
buffers:
  mono: f32
  stereo: f32[2]
def copy(a: f32[], b: f32[], c: f32[], d: f32[]):
  a[:] = b
  c[:] = d
sample:
  copy(mono[1:3], mono[:2], mono[:2], stereo[0, :2])
  out1 = 0.0
"#;
    let (_, mut mir) = source_program(source, 1);
    let function = mir
        .functions
        .iter_mut()
        .find(|function| function.name == "copy")
        .unwrap();
    // Form one aggregate operation from two independent leaf copies. All
    // descriptor preparation stays before the group, as in data lowering.
    let mut copies = Vec::new();
    let mut location = None;
    function.body.statements.retain(|statement| {
        if let StatementKind::SliceCopy { copies: leaves } = &statement.kind {
            copies.extend(leaves.iter().cloned());
            location = Some(statement.source);
            false
        } else {
            true
        }
    });
    assert_eq!(copies.len(), 2);
    function.body.statements.push(onda_mir::Statement {
        kind: StatementKind::SliceCopy { copies },
        source: location.unwrap(),
    });
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut values = [1.0_f32, 2.0, 3.0, 4.0];
        let mut output = [0.0_f32];
        let pointer = values.as_mut_ptr().cast::<u8>();
        let result = native.test_process_checked(
            &mut state,
            &params,
            0,
            1,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &[],
            &[output.as_mut_ptr().cast()],
            &[pointer, pointer],
            &[4, 2],
            &[1, 2],
            &[48_000.0; 2],
        );
        assert!(result.is_err(), "unequal-stride overlap must fail");
        assert_eq!(values, [1.0, 2.0, 3.0, 4.0]);
    }
}

#[test]
fn fixed_data_parameters_retain_shape_for_copies_and_returns() {
    let source = r#"
struct Note:
  value = 1.0
  tap: f32[1]
def read(note: Note):
  return note.value + note.tap[0]
def saved(values: Note[2]) -> Note[2]:
  copy: Note[2] = values
  values = [Note(value = 3.0), Note(value = 5.0)]
  return copy
def primitive(values: f32[2]) -> f32[2]:
  return values
sample:
  values = [Note(), Note()]
  copy = saved(values)
  numbers = primitive([7.0, 11.0])
  out1 = read(copy[0]) + copy[1].value + read(values[0]) + values[1].value + numbers[0] + numbers[1]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![28.0; 8]);
    }
}

#[test]
fn primitive_array_replacement_captures_literals_without_redirecting() {
    let source = r#"
struct History:
  values: i32[2]
def replace(values: i32[2]):
  values = [values[1], values[0]]
sample:
  original: i32[2] = [3, 5]
  alias = original
  replace(alias)
  original = [original[1] + 7, original[0] + 11]
  history = History()
  history.values = [13, 17]
  out1 = f32(alias[0] + alias[1] + history.values[0] + history.values[1])
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![56.0; 8]);
    }
}

#[test]
fn structured_data_aliases_preserve_ranged_field_stores() {
    let source = r#"
struct Counter:
  value: i32 = 0 {range = 0..=3}
init:
  state = Counter()
  choose_first: bool = false
sample:
  a = Counter()
  b = Counter()
  if choose_first:
    selected = a
  else:
    selected = b
  selected.value = 9
  live = state
  live.value = 9
  out1 = f32(a.value + b.value + state.value)
  choose_first = !choose_first
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![6.0; 8]);
    }
}

#[test]
fn structured_data_branch_alias_selects_storage_without_changing_candidates() {
    let source = r#"
struct Note:
  frequency = 2.0
  history: f32[2]
init:
  choose_first: bool = false
sample:
  a = Note(frequency = 3.0)
  b = Note(frequency = 5.0)
  if choose_first:
    selected = a
  else:
    selected = b
  selected.frequency = 7.0
  selected.history[0] = 11.0
  out1 = a.frequency + b.frequency + a.history[0] * 2.0 + b.history[0]
  choose_first = !choose_first
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(
            output[0],
            vec![21.0, 34.0, 21.0, 34.0, 21.0, 34.0, 21.0, 34.0]
        );
    }
}

#[test]
fn structured_data_constructor_fields_capture_in_source_order() {
    let source = r#"
struct Note:
  frequency = 2.0
struct Pair:
  first: Note
  second = 0.0
def mutate(note: Note):
  note.frequency = 5.0
  return 7.0
def read(note: Note):
  return note.frequency
sample:
  note = Note()
  pair = Pair(first = note, second = mutate(note))
  out1 = pair.first.frequency + pair.second + read(Note(frequency = 11.0))
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![20.0; 8]);
    }
}

#[test]
fn structured_data_constructor_preserves_explicit_nested_value() {
    let source = r#"
struct Note:
  value: f32
struct Pair:
  first: f32
  second: Note
sample:
  note = Note(0.1)
  captured = Pair(0.3, note)
  out1 = captured.second.value
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![0.1; 8]);
    }
}

#[test]
fn structured_data_fixed_arrays_copy_and_replace_without_redirecting() {
    let source = r#"
def make() -> f32[2]:
  return [3.0, 5.0]
sample:
  original = make()
  alias = original
  saved: f32[2] = original
  replacement = make()
  replacement[0] = 7.0
  original = replacement
  out1 = alias[0] + saved[0]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![10.0; 8]);
    }
}

#[test]
fn structured_data_helpers_initialize_persistent_state() {
    let source = r#"
struct Filter:
  gain = 2.0
  history: f32[4]
def make(gain) -> Filter:
  return Filter(gain = gain)
init:
  original = make(3.0)
  saved: Filter = original
  original = make(5.0)
sample:
  out1 = original.gain + saved.gain
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![8.0; 8]);
    }
}

#[test]
fn structured_data_nested_defaults_and_tuple_fields_execute() {
    let source = r#"
struct Filter:
  gain = 2.0
  pair: (f32, f32) = (3.0, 5.0)
  history: f32[1]
struct Patch:
  filter: Filter
  voices: Filter[2]
def make() -> Patch:
  return Patch()
sample:
  patch = make()
  alias = patch.filter
  alias.pair = (7.0, 11.0)
  alias.pair[0] = 13.0
  selected = patch.voices[1]
  selected.history[0] = 17.0
  saved: Filter = selected
  selected.gain = 19.0
  out1 = patch.filter.pair[0] + alias.pair[1] + saved.gain + saved.history[0]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![43.0; 8]);
    }
}

#[test]
fn structured_data_returns_copies_and_replacement_execute() {
    let source = r#"
struct Note:
  frequency = 440.0
  velocity = 1.0
def make(frequency) -> Note:
  return Note(frequency = frequency)
def reset(note: Note):
  note = Note()
sample:
  original = make(220.0)
  alias = original
  saved: Note = original
  alias = make(110.0)
  reset(original)
  out1 = saved.frequency + alias.frequency
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![660.0; 8]);
    }
}

#[test]
fn structured_data_array_fields_return_independent_contents() {
    let source = r#"
struct Spectrum:
  real: f32[8]
  imaginary: f32[8]
def duplicate(value: Spectrum) -> Spectrum:
  return value
init:
  source = Spectrum()
  source.real[0] = 3.0
sample:
  result = duplicate(source)
  source.real[0] = source.real[0] + 1.0
  out1 = result.real[0]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
    }
}

#[test]
fn structured_data_runtime_struct_arrays_construct_copy_and_replace() {
    let source = r#"
struct Note:
  value = 2.0
  history: f32[2]
struct Bank:
  notes: Note[2]
def make() -> Note[2]:
  notes: Note[2] = Note(value = 3.0)
  return notes
def total(notes: Note[]):
  result = 0.0
  for i in 0..notes.len():
    note = notes[i]
    result = result + note.value
  return result
sample:
  notes: Note[2] = make()
  copy: Note[2] = notes
  notes[0] = Note(value = 7.0)
  bank = Bank(notes = copy)
  first = bank.notes[0]
  first.history[0] = 11.0
  other = bank.notes[1]
  out1 = total(notes) + total(copy) + first.history[0] + other.history[0]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![27.0; 8]);
    }
}

#[test]
fn structured_data_array_broadcast_captures_once_and_element_target_precedes_rhs() {
    let source = r#"
struct Counter:
  value: i32 = 0
struct Note:
  value = 2.0
def next(counter: Counter):
  old = counter.value
  counter.value = old + 1
  return old
def make(counter: Counter):
  counter.value = counter.value + 1
  return Note(value = f32(counter.value))
sample:
  counter = Counter()
  notes: Note[3] = make(counter)
  notes[next(counter)] = make(counter)
  a = notes[0]
  b = notes[1]
  c = notes[2]
  out1 = a.value + b.value * 10.0 + c.value * 100.0 + f32(counter.value) * 1000.0
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![3131.0; 8]);
    }
}

#[test]
fn structured_data_slices_capture_bounds_and_forward_views() {
    let source = r#"
struct Note:
  value = 2.0
  history: f32[2]
def sum(notes: Note[]):
  result = 0.0
  for i in 0..notes.len():
    note = notes[i]
    result = result + note.value + note.history[0]
  return result
sample:
  notes: Note[4]
  end = 3
  view: Note[] = notes[1:end]
  end = 1
  first = view[0]
  first.value = 5.0
  first.history[0] = 7.0
  out1 = sum(view) + sum(notes[3:1]) + sum(notes[-10:1]) + f32(view.len())
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![18.0; 8]);
    }
}

#[test]
fn structured_data_slice_copy_and_fill_preserve_overlap_and_tail() {
    let source = r#"
struct Note:
  value = 0.0
  history: f32[2]
sample:
  notes: Note[4] = [Note(value = 1.0), Note(value = 2.0), Note(value = 3.0), Note(value = 4.0)]
  first = notes[0]
  first.history[0] = 7.0
  notes[1:] = notes[:3]
  source = notes[1]
  notes[2:] = source
  source.value = 9.0
  a = notes[0]
  b = notes[1]
  c = notes[2]
  d = notes[3]
  empty = notes[3:1]
  empty[:] = Note(value = 50.0)
  notes[:3] = notes[3:]
  out1 = a.value + b.value * 10.0 + c.value * 100.0 + d.value * 1000.0 + c.history[0]
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![1198.0; 8]);
    }
}

#[test]
fn structured_data_slice_fill_works_in_top_level_and_proc_init() {
    let source = r#"
struct Note:
  value = 0.0

proc Bank:
  init:
    notes: Note[2]
    notes[:] = Note(value = 3.0)
  sample:
    first = notes[0]
    out1 = first.value

init:
  notes: Note[2]
  fill = Note(value = 4.0)
  notes[:] = fill
  bank = Bank()

sample:
  first = notes[0]
  out1 = first.value + bank()
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![7.0; 8]);
    }
}

#[test]
fn structured_data_typed_slices_retain_fresh_backing() {
    let source = r#"
struct Note:
  value = 2.0
def make() -> Note[2]:
  return [Note(value = 3.0), Note(value = 5.0)]
def values() -> f32[2]:
  return [7.0, 11.0]
sample:
  notes: Note[] = make()
  numbers: f32[] = values()
  first = notes[0]
  first.value = 13.0
  numbers[0] = 17.0
  out1 = first.value + numbers[0] + numbers[1] + f32(notes.len())
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![43.0; 8]);
    }
}

#[test]
fn structured_data_branch_slices_keep_candidate_views_and_selected_length() {
    let source = r#"
struct Note:
  value = 2.0
init:
  choose = false
sample:
  notes: Note[4]
  left = notes[:1]
  right = notes[2:]
  if choose:
    selected = left
  else:
    selected = right
  selected[:] = Note(value = 3.0)
  a = left[0]
  b = right[0]
  out1 = a.value + b.value * 10.0 + f32(selected.len()) * 100.0
  choose = !choose
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(
            output[0],
            vec![232.0, 123.0, 232.0, 123.0, 232.0, 123.0, 232.0, 123.0]
        );
    }
}

#[test]
fn structured_data_fixed_copies_accept_proven_slice_lengths() {
    let source = r#"
struct Note:
  value = 2.0
sample:
  values: f32[4] = [1.0, 2.0, 3.0, 4.0]
  view: f32[] = values[1:3]
  captured: f32[2] = view
  direct: f32[2] = values[-2:]
  notes: Note[3]
  selected = notes[1:]
  saved: Note[2] = selected
  selected[:] = Note(value = 5.0)
  note = saved[0]
  values[:] = 9.0
  out1 = captured[0] + captured[1] + direct[0] + direct[1] + note.value
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], vec![14.0; 8]);
    }
}

#[test]
fn structured_events_and_delegates_keep_live_views_and_capture_host_records() {
    let (_, mir) = source_program(
        r#"
struct Item:
  enabled: bool
  gain: f64
  bins: f32[2]
  pair: (i32, i64)
struct Patch:
  items: Item[2]
  tail: i32
init:
  patch: Patch
  observed: f64 = 0.0
delegate configured(next: Patch)
when configured(next):
  writable = patch.items[0]
  writable.gain += 1.0
  borrowed = next.items[0]
  observed = borrowed.gain
event configure(next: Patch):
  patch = next
  configured(patch)
sample:
  first = patch.items[0]
  out1 = f32(first.gain + observed)
"#,
        1,
    );
    let mut payload = vec![1, 0];
    for value in [4.0_f64, 6.0] {
        payload.extend(value.to_le_bytes());
    }
    for value in [1.0_f32, 2.0, 3.0, 4.0] {
        payload.extend(value.to_le_bytes());
    }
    for value in [5_i32, 7] {
        payload.extend(value.to_le_bytes());
    }
    for value in [11_i64, i64::MIN] {
        payload.extend(value.to_le_bytes());
    }
    payload.extend(13_i32.to_le_bytes());
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        for capacity in [0, 73, 74] {
            let mut state = native.initialize_state(&params).unwrap();
            let mut storage = vec![0xa5; capacity];
            let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
            let mut execution = onda_processor_abi::ExecutionOutput {
                delegate_batch: &mut batch,
                ..onda_processor_abi::ExecutionOutput::none()
            };
            unsafe {
                native.trigger_event_by_index(
                    &mut state,
                    &params,
                    0,
                    &payload,
                    &[],
                    &[],
                    &[],
                    &[],
                    (capacity != 0).then_some(&mut execution),
                )
            }
            .unwrap();
            let mut output = [0.0_f32];
            native
                .test_process_checked(
                    &mut state,
                    &params,
                    0,
                    1,
                    3,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[],
                    &[],
                    &[],
                    &[],
                )
                .unwrap();
            assert_eq!(output, [10.0]);
            if capacity == 74 {
                assert_eq!(batch.record_count, 1);
                assert_eq!(&storage[12..], &payload);
            } else if capacity != 0 {
                assert_eq!(batch.record_count, 0);
                assert_eq!(batch.overflow_count, 1);
                assert_eq!(storage, vec![0xa5; capacity]);
            }
        }
    }
}

#[test]
fn raw_events_reject_before_mutation_and_prepare_normalized_aligned_input() {
    let (_, mir) = source_program(
        r#"
struct Note:
  enabled: bool
  mode: i64 = 0 {range = -3..=3, mode = wrap}
  gain: f64
init:
  calls: i32 = 0
  observed: f64 = 0.0
delegate changed(value: f64)
event configure(prefix: bool, values: i32[], note: Note):
  calls += 1
  observed = f64(note.mode) + note.gain + f64(values.len())
  if prefix && note.enabled:
    observed += 10.0
  changed(observed)
sample:
  out1 = f32(observed) + f32(calls)
"#,
        1,
    );
    let mut payload = vec![255];
    payload.extend(2_i32.to_le_bytes());
    payload.extend(11_i32.to_le_bytes());
    payload.extend(13_i32.to_le_bytes());
    payload.push(2);
    payload.extend(i64::MIN.to_le_bytes());
    payload.extend(0.5_f64.to_le_bytes());
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let initial = state.bytes().to_vec();
        let mut storage = [0xa5; 40];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        batch.used_bytes = 7;
        batch.record_count = 3;
        batch.overflow_count = 5;
        let mut execution = onda_processor_abi::ExecutionOutput {
            delegate_batch: &mut batch,
            ..onda_processor_abi::ExecutionOutput::none()
        };
        let mut malformed = (0..payload.len())
            .map(|len| payload[..len].to_vec())
            .collect::<Vec<_>>();
        let mut trailing = payload.clone();
        trailing.push(0);
        malformed.push(trailing);
        for length in [-1, i32::MAX] {
            let mut invalid = payload.clone();
            invalid[1..5].copy_from_slice(&length.to_le_bytes());
            malformed.push(invalid);
        }
        for invalid in malformed {
            let status = unsafe {
                native.trigger_event_by_index_unchecked(
                    &mut state,
                    &params,
                    0,
                    &invalid,
                    &[],
                    &[],
                    &[],
                    &[],
                    Some(&mut execution),
                )
            };
            assert_eq!(
                status,
                onda_processor_abi::PROCESSOR_EXECUTION_INPUT_REJECTED
            );
            assert_eq!(state.bytes(), initial);
            assert_eq!(
                (batch.used_bytes, batch.record_count, batch.overflow_count),
                (7, 3, 5)
            );
            assert_eq!(storage, [0xa5; 40]);
        }
        // Capacity rejection follows the same path and does not poison the instance.
        state.event_workspace = crate::RuntimeBuffer::default();
        let status = unsafe {
            native.trigger_event_by_index_unchecked(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                Some(&mut execution),
            )
        };
        assert_eq!(
            status,
            onda_processor_abi::PROCESSOR_EXECUTION_INPUT_REJECTED
        );
        assert_eq!(state.bytes(), initial);
        state.reserve_event_workspace(64).unwrap();
        batch.used_bytes = 0;
        batch.record_count = 0;
        let status = unsafe {
            native.trigger_event_by_index_unchecked(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                Some(&mut execution),
            )
        };
        assert_eq!(status, 0);
        assert_eq!(payload[0], 255);
        assert_eq!(payload[13], 2);
        assert_eq!(batch.record_count, 1);
        let normalized = -3 + (i128::from(i64::MIN) + 3).rem_euclid(7);
        assert_eq!(
            f64::from_le_bytes(storage[12..20].try_into().unwrap()),
            normalized as f64 + 12.5
        );
    }
}

#[test]
fn runtime_struct_message_slices_share_one_length_and_forward_without_copies() {
    let (_, mir) = source_program(
        r#"
struct Note:
  enabled: bool
  gain: f64
  bins: f32[2]
proc Bank:
  delegate accepted(notes: Note[])
  event configure(notes: Note[]):
    accepted(notes)
  sample:
    out1 = 0.0
init:
  bank = Bank()
  saved: Note[2]
  count: i32 = 0
delegate configured(notes: Note[], tail: f64)
when configured(notes, tail):
  saved[:] = notes[:]
  count = notes.len()
when bank.accepted(notes):
  configured(notes, 7.0)
event configure(notes: Note[], tail: f64):
  bank.configure(notes)
sample:
  note = saved[0]
  out1 = f32(note.gain) + f32(count)
"#,
        1,
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: level,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        for length in [0_i32, 1, 2, 1024] {
            let mut payload = length.to_le_bytes().to_vec();
            payload.extend(std::iter::repeat_n(1, length as usize));
            for _ in 0..length {
                payload.extend(4.0_f64.to_le_bytes());
            }
            for _ in 0..length * 2 {
                payload.extend(2.0_f32.to_le_bytes());
            }
            payload.extend(7.0_f64.to_le_bytes());
            let mut state = native.initialize_state(&params).unwrap();
            let mut storage = vec![0; payload.len() + 12];
            let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
            let mut execution = onda_processor_abi::ExecutionOutput {
                delegate_batch: &mut batch,
                ..onda_processor_abi::ExecutionOutput::none()
            };
            unsafe {
                native.trigger_event_by_index(
                    &mut state,
                    &params,
                    0,
                    &payload,
                    &[],
                    &[],
                    &[],
                    &[],
                    Some(&mut execution),
                )
            }
            .unwrap();
            assert_eq!(batch.record_count, 1);
            assert_eq!(&storage[12..], payload);
        }
    }
}

#[test]
fn nested_message_arguments_execute_independently() {
    let source = include_str!(
        "../../../../../packages/onda_binaryen_web/test/fixtures/structured-message-routing.onda"
    );
    let source = source.replace(
        "sample:\n  note = saved.notes[0]",
        "sample:\n  bank.configure(Patch(pair = (7, 0.5)))\n  configured(Patch(pair = (3, 0.25)))\n  note = saved.notes[0]",
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(&source, 4, level);
        assert_eq!(output[0], [4.0; 4]);
    }
}

#[test]
fn branch_aliases_and_explicit_aggregate_arguments_preserve_writes() {
    let source = include_str!(
        "../../../../../packages/onda_binaryen_web/test/fixtures/structured-data-regressions.onda"
    );
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let output = run_native_outputs_with_opt_level(source, 8, level);
        assert_eq!(output[0], [60.0; 8]);
        assert_eq!(output[1], [26.0; 8]);
    }
}
