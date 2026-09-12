use super::*;

fn compile(source: &str) -> onda_mir::Program {
    let parsed = onda_frontend::parse_program(source).expect("data source parses");
    let typed = crate::analyze(parsed).expect("data source analyzes");
    lower_program_to_optimized_mir(&typed)
        .expect("data source lowers")
        .into_program()
}

fn scratch_array_count(program: &onda_mir::Program, len: u32) -> usize {
    program
        .state
        .iter()
        .filter(|slot| {
            slot.persistence == onda_mir::StatePersistence::InstanceScratch
                && matches!(program.types[slot.ty.index()], MirType::Array { len: actual, .. } if actual == len)
        })
        .count()
}

#[test]
fn init_views_share_captured_selections_with_process_and_events() {
    let program = compile(
        r#"
struct Note:
  value = 1.0
  bins: f32[2]
init:
  notes: Note[4]
  index: i32 = 1
  selected = notes[index]
  window: Note[] = notes[index:]
  bins = selected.bins
  fresh: f32[] = [2.0, 3.0]
event change():
  selected.value = 9.0
  fresh[0] = 7.0
sample:
  index = 3
  first = window[0]
  out1 = selected.value + first.value + bins[0] + fresh[0]
"#,
    );
    assert!(program
        .state
        .iter()
        .any(|slot| slot.name.starts_with("__onda_init.selection")));
    assert!(program
        .state
        .iter()
        .any(|slot| slot.name.starts_with("__onda_init.storage")));
    assert!(program
        .state
        .iter()
        .all(|slot| !matches!(program.types[slot.ty.index()], MirType::Slice { .. })));
}

#[test]
fn persistent_views_reject_expired_init_locals_and_pinning() {
    for binding in ["saved = temporary", "saved: Note[] = temporary[:]"] {
        let (declaration, read) = if binding.contains("[]") {
            (
                "temporary: Note[2]",
                "selected = saved[0]\n  out1 = selected.value",
            )
        } else {
            ("temporary = Note()", "out1 = saved.value")
        };
        let source = format!("struct Note:\n  value = 1.0\ninit:\n  stable = Note()\n  kept = stable\n  if true:\n    {declaration}\n  else:\n    {declaration}\n  {binding}\nsample:\n  {read}\n");
        let parsed = onda_frontend::parse_program(&source).unwrap();
        let typed = crate::analyze(parsed).expect("alias types are valid");
        let errors = lower_program_to_optimized_mir(&typed).unwrap_err();
        let error = errors
            .iter()
            .find(|error| error.message.contains("init-local"))
            .unwrap_or_else(|| panic!("{errors:?}"));
        assert!(error.message.contains("'saved'"), "{error:?}");
        assert_eq!(error.location.line, 10, "{error:?}");
        assert!(error.location.column > 0, "{error:?}");
    }
    for declaration in ["pin selected = notes[0]", "pin selected: Note[] = notes[:]"] {
        let source = format!("struct Note:\n  value = 1.0\ninit:\n  notes: Note[2]\n  {declaration}\nsample:\n  out1 = 0.0\n");
        let parsed = onda_frontend::parse_program(&source).unwrap();
        let errors = crate::analyze(parsed).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("cannot be pinned")),
            "{errors:?}"
        );
    }
    compile(
        r#"
struct Note:
  value = 1.0
init:
  if true:
    temporary = Note()
  else:
    temporary = Note()
  saved: Note = temporary
sample:
  out1 = saved.value
"#,
    );
}

#[test]
fn owned_structured_task_frames_use_normal_data_replacement() {
    for owner in [false, true] {
        let body = r#"
init:
  pin result = 0.0
task prepare():
  note: Note = Note(value = 4.0)
  notes: Note[2] = Note(value = 3.0)
  yield
  note.value += 1.0
  selected = notes[1]
  result = note.value + selected.value
block:
  await prepare()
  sample:
    out1 = result
"#;
        let body = if owner {
            format!(
                "proc Worker:\n{}\ninit:\n  worker = Worker()\nsample:\n  out1 = worker()\n",
                body.lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>()
            )
        } else {
            body.to_owned()
        };
        compile(&format!(
            "struct Note:\n  value = 1.0\n  bins: f32[2]\n{body}"
        ));
    }
}

#[test]
fn block_owned_arrays_and_views_survive_await_control_flow() {
    for owner in [false, true] {
        let body = r#"
task prepare():
  yield
block:
  values: f32[2] = [2.0, 3.0]
  notes: Note[2] = Note(value = 4.0)
  selected = notes[1]
  await prepare()
  sample:
    out1 = values[1] + selected.value
"#;
        let body = if owner {
            format!(
                "proc Worker:\n{}\ninit:\n  worker = Worker()\nsample:\n  out1 = worker()\n",
                body.lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>()
            )
        } else {
            body.to_owned()
        };
        let program = compile(&format!("struct Note:\n  value = 1.0\n{body}"));
        assert!(program
            .state
            .iter()
            .all(|slot| !matches!(program.types[slot.ty.index()], MirType::Slice { .. })));
    }
}

#[test]
fn task_views_capture_selections_and_keep_owned_backing_in_the_frame() {
    compile(
        r#"
struct Note:
  value = 1.0
  bins: f32[2]
init:
  notes: Note[4]
  index: i32 = 1
  choose = true
  result = 0.0
task prepare():
  owned: Note[2] = Note(value = 3.0)
  if choose:
    selected = notes[index]
  else:
    selected = owned[0]
  view: Note[] = notes[index:]
  bins = selected.bins
  yield
  selected.value = 5.0
  first = view[0]
  result = selected.value + first.value + bins[0]
block:
  await prepare()
  sample:
    out1 = result
"#,
    );
}

#[test]
fn proc_views_and_block_owned_data_live_on_each_instance() {
    compile(
        r#"
struct Note:
  value = 1.0
  bins: f32[2]
proc Worker:
  init:
    notes: Note[4]
    index: i32 = 1
    initial = notes[index]
    fresh: f32[] = [2.0, 3.0]
  event change():
    initial.value = 9.0
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
"#,
    );
}

#[test]
fn block_data_views_retain_storage_and_captured_selections() {
    let program = compile(
        r#"
struct Note:
  value = 1.0
  bins: f32[2]
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
"#,
    );
    assert!(program
        .state
        .iter()
        .any(|slot| slot.name.starts_with("__onda_block.storage")));
    assert!(program
        .state
        .iter()
        .any(|slot| slot.name.starts_with("__onda_block.selection")));
    assert!(program
        .state
        .iter()
        .all(|slot| !matches!(program.types[slot.ty.index()], MirType::Slice { .. })));
}

#[test]
fn block_view_selection_storage_is_independent_of_array_length() {
    let compile_size = |size| {
        compile(&format!(
            r#"
struct Note:
  value = 1.0
block:
  notes: Note[{size}]
  selected = notes[1]
  sample:
    out1 = selected.value
"#
        ))
    };
    let small = compile_size(4);
    let large = compile_size(8192);
    let descriptors = |program: &onda_mir::Program| {
        program
            .state
            .iter()
            .filter(|slot| slot.name.starts_with("__onda_block.selection"))
            .count()
    };
    assert!(descriptors(&small) > 0);
    assert_eq!(descriptors(&small), descriptors(&large));
    for program in [&small, &large] {
        let arrays = program
            .state
            .iter()
            .filter(|slot| matches!(program.types[slot.ty.index()], MirType::Array { .. }))
            .collect::<Vec<_>>();
        assert_eq!(
            arrays.len(),
            1,
            "retained backing must not also allocate invocation scratch"
        );
        assert_eq!(arrays[0].persistence, onda_mir::StatePersistence::Snapshot);
    }
}

#[test]
fn fresh_block_slice_backing_is_retained_but_external_views_cannot_escape() {
    compile(
        r#"
block:
  values: f32[] = [2.0, 3.0]
  sample:
    values[0] = values[0] + 1.0
    out1 = values[0]
"#,
    );
    let source = onda_frontend::parse_program(
        r#"
buffers:
  input: f32
block:
  view = input[:]
  sample:
    out1 = view[0]
"#,
    )
    .unwrap();
    let typed = crate::analyze(source).unwrap();
    let errors = lower_program_to_optimized_mir(&typed).unwrap_err();
    assert!(errors
        .iter()
        .any(|error| error.message.contains("cannot survive a process boundary")));
}

#[test]
fn fresh_typed_init_data_is_initialized_in_its_persistent_storage() {
    let program = compile(
        r#"
struct Note:
  value = 1.0
  bins: f32[3]
init:
  large: Note[4096]
sample:
  out1 = large[0].value
"#,
    );
    let scratch_arrays = program
        .state
        .iter()
        .filter(|slot| slot.persistence == onda_mir::StatePersistence::InstanceScratch)
        .filter_map(|slot| match program.types[slot.ty.index()] {
            MirType::Array { len, .. } => Some(len),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !scratch_arrays.contains(&4096) && !scratch_arrays.contains(&12288),
        "fresh persistent initialization should not retain a full-size temporary: {scratch_arrays:?}"
    );
}

#[test]
fn branch_joined_struct_views_reuse_the_root_field_descriptors() {
    let program = compile(
        r#"
struct Cell:
  value = 1.0
  pair: f32[2]
params:
  choose: bool = false
init:
  cells: Cell[4]
sample:
  if choose:
    view: Cell[] = cells[0:3]
  else:
    view: Cell[] = cells[1:4]
  out1 = view[0].value
"#,
    );
    let dump = onda_mir::format_program(&program);
    let slice_lengths = dump.matches("slice_len").count();
    assert!(
        slice_lengths <= 4,
        "each branch should join each canonical field descriptor at most once, found {slice_lengths} length reads"
    );
}

#[test]
fn named_returned_storage_supports_nested_selections() {
    compile(
        r#"
struct Filter:
  gain = 1.0
struct Voice:
  filter: Filter
struct Patch:
  voices: Voice[2]
def make_patch() -> Patch:
  return Patch()
sample:
  patch = make_patch()
  voices = patch.voices
  voice = voices[1]
  filter = voice.filter
  filter.gain = 2.0
  out1 = filter.gain
"#,
    );
    assert!(onda_frontend::parse_program("sample:\n  selected = make_patch()[0]\n").is_err());
}

#[test]
fn fixed_array_results_support_generic_elements_and_nominal_annotations() {
    compile(
        r#"
struct Note:
  value = 1.0
def pair<T>(value: T) -> T[2]:
  return [value, value]
def notes(value: Note) -> Note[2]:
  return [value, value]
sample:
  items = notes(Note())
  values = pair<f32>(3.0)
  first = items[0]
  out1 = first.value + values[1]
"#,
    );
}

#[test]
fn resolved_generic_struct_types_work_across_data_signatures() {
    compile(
        r#"
struct Box<T>:
  value: T
def copy(value: Box<f32>) -> Box<f32>:
  return value
def fallback(value: Box<f32>) -> Box<f32>:
  return value
def pair(value: Box<f32>) -> Box<f32>[2]:
  return [value, value]
def last(values: Box<f32>[2]) -> Box<f32>:
  return values[1]
def first(values: Box<f32>[]) -> Box<f32>:
  return values[0]
event inspect(values: Box<f32>[], default_value: Box<f32>):
  selected = copy(default_value)
sample:
  boxes: Box<f32>[2] = pair(Box<f32>(3.0))
  view: Box<f32>[] = boxes[:]
  selected = copy(first(view))
  selected = last(boxes)
  copied: Box<f32> = selected
  defaulted = fallback(Box<f32>(4.0))
  out1 = copied.value + defaulted.value
"#,
    );
}

#[test]
fn struct_return_captures_reference_parameter() {
    let program = compile(
        r#"
struct Note:
  frequency = 440.0
  velocity = 1.0
def duplicate(note: Note) -> Note:
  return note
init:
  original = Note()
sample:
  captured = duplicate(original)
  original.frequency = 220.0
  out1 = captured.frequency
"#,
    );
    let function = program
        .functions
        .iter()
        .find(|function| function.name == "duplicate")
        .unwrap();
    assert!(function.results.is_empty());
    assert!(function
        .params
        .iter()
        .any(|parameter| parameter.name == "__onda_result.frequency"));
}

#[test]
fn runtime_construction_copy_and_replacement_share_storage_rules() {
    compile(
        r#"
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
"#,
    );
}

#[test]
fn typed_data_declaration_rejects_redeclaration() {
    let parsed = onda_frontend::parse_program(
        r#"
struct Note:
  frequency = 440.0
sample:
  note = Note()
  alias = note
  alias: Note = note
  out1 = alias.frequency
"#,
    )
    .unwrap();
    let errors = crate::analyze(parsed).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("must introduce a new name")),
        "{errors:?}"
    );
}

#[test]
fn init_typed_data_declaration_rejects_alias_redeclaration() {
    let parsed = onda_frontend::parse_program(
        r#"
struct Note:
  frequency = 440.0
init:
  note = Note()
  alias = note
  alias: Note = note
sample:
  out1 = alias.frequency
"#,
    )
    .unwrap();
    let errors = crate::analyze(parsed).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("must introduce a new name")),
        "{errors:?}"
    );
}

#[test]
fn generic_typed_data_declaration_rejects_redeclaration() {
    let parsed = onda_frontend::parse_program(
        r#"
struct Box<T>:
  value: T
def reset(box: Box<f32>):
  box: Box<f32>
sample:
  box = Box<f32>(1.0)
  reset(box)
  out1 = box.value
"#,
    )
    .unwrap();
    let errors = crate::analyze(parsed).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("must introduce a new name")),
        "{errors:?}"
    );
}

#[test]
fn generic_struct_constructor_inference_uses_shared_executable_scope_types() {
    compile(
        r#"
params:
  amount: f64 = 1.0
  choose: bool = false
struct Box<T>:
  value: T
struct Source:
  value: f64
  values: f64[2]
def wide() -> f64:
  return 2.0
def wrap(value: f64) -> Box<f64>:
  return Box(value)
sample:
  source: Source
  from_field = Box(source.value)
  from_array_field = Box(source.values[0])
  from_param = Box(amount)
  returned = wide()
  from_return = Box(returned)
  if choose:
    branch_value: f64 = 3.0
  else:
    branch_value: f64 = 4.0
  from_branch = Box(branch_value)
  total: i64 = 0
  for i: i64 in i64(0)..i64(2):
    from_loop = Box(i)
    total += from_loop.value
  wrapped = wrap(amount)
  out1 = f32(from_field.value + from_array_field.value + from_param.value + from_return.value + from_branch.value + wrapped.value) + f32(total)
"#,
    );
}

#[test]
fn generic_struct_constructor_inference_is_uniform_in_runtime_handlers() {
    compile(
        r#"
struct Box<T>:
  value: T
proc Worker:
  init:
    result: f64 = 0.0
  delegate changed(value: f64)
  when changed(value):
    boxed = Box(value)
    state_box = Box(result)
    result = boxed.value + state_box.value
  event update(value: f64):
    boxed = Box(value)
    result = boxed.value
    changed(value)
  tasks:
    refresh():
      value: f64 = 2.0
      boxed = Box(value)
      result = boxed.value
      return
  sample:
    boxed = Box(result)
    out1 = f32(boxed.value)
init:
  worker = Worker()
  result: f64 = 0.0
delegate changed(value: f64)
when changed(value):
  boxed = Box(value)
  state_box = Box(result)
  result = boxed.value + state_box.value
event update(value: f64):
  boxed = Box(value)
  result = boxed.value
  changed(value)
tasks:
  refresh():
    value: f64 = 3.0
    boxed = Box(value)
    result = boxed.value
    return
sample:
  boxed = Box(result)
  out1 = f32(boxed.value) + worker()
"#,
    );
}

#[test]
fn generic_proc_constructor_inference_uses_the_same_executable_scope_types() {
    compile(
        r#"
proc Constant<T>:
  params:
    value: T = T(0.0)
  outs<T> 1
  sample:
    out1 = value
params:
  amount: f64 = 1.0
def wide() -> f64:
  return 2.0
init:
  returned = wide()
  source = returned + amount
  constant = Constant(value = source)
sample:
  out1 = f32(constant())
"#,
    );
}

#[test]
fn generic_struct_constructors_bind_nested_and_array_field_types() {
    compile(
        r#"
struct Inner<T>:
  value: T
struct Outer<T>:
  inner: Inner<T>
  values: T[2]
sample:
  values: f64[2] = [1.0, 2.0]
  inner = Inner<f64>(3.0)
  outer = Outer(inner = inner, values = values)
  out1 = f32(outer.inner.value + outer.values[0])
"#,
    );
}

#[test]
fn read_only_struct_event_write_reports_only_the_permission_error() {
    let parsed = onda_frontend::parse_program(
        r#"
struct Note:
  pitch: f32
event mutate(note: Note):
  note.pitch = 440.0
sample:
  out1 = 0.0
"#,
    )
    .unwrap();
    let errors = crate::analyze(parsed).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0]
        .message
        .contains("cannot write through read-only payload parameter 'note'"));
}

#[test]
fn scalar_event_parameter_write_keeps_its_single_immutable_diagnostic() {
    let parsed = onda_frontend::parse_program(
        r#"
event mutate(value: f32):
  value = 1.0
sample:
  out1 = 0.0
"#,
    )
    .unwrap();
    let errors = crate::analyze(parsed).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("cannot write through read-only payload parameter 'value'"),
        "{errors:?}"
    );
}

#[test]
fn struct_array_fields_keep_constant_width_return_signature() {
    let program = compile(
        r#"
struct Spectrum:
  real: f32[4096]
  imaginary: f32[4096]
def duplicate(value: Spectrum) -> Spectrum:
  return value
init:
  source = Spectrum()
sample:
  result = duplicate(source)
  out1 = result.real[0]
"#,
    );
    let function = program
        .functions
        .iter()
        .find(|function| function.name == "duplicate")
        .unwrap();
    assert!(function.params.len() <= 4);
}

#[test]
fn fixed_array_results_can_be_bound_returned_and_forwarded() {
    compile(
        r#"
def make() -> f32[2]:
  return [1.0, 2.0]
def duplicate(values: f32[2]) -> f32[2]:
  return values
def first(values: f32[]):
  return values[0]
sample:
  values = duplicate(make())
  values[0] = 3.0
  out1 = values[0] + first(make())
"#,
    );
}

#[test]
fn disjoint_fixed_results_reuse_prepared_instance_scratch() {
    let program = compile(
        r#"
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
"#,
    );
    assert_eq!(
        scratch_array_count(&program, 4096),
        2,
        "one callee slot and one reused caller slot"
    );
}

#[test]
fn simultaneously_live_fixed_results_keep_distinct_scratch() {
    let program = compile(
        r#"
def make(value: f32) -> f32[4096]:
  result: f32[4096]
  result[:] = value
  return result

sample:
  first = make(1.0)
  second = make(2.0)
  out1 = first[0] + second[0]
"#,
    );
    assert_eq!(
        scratch_array_count(&program, 4096),
        3,
        "one callee slot and two live caller slots"
    );
}

#[test]
fn owned_local_scratch_reuse_follows_value_lifetimes() {
    let disjoint = compile(
        r#"
sample:
  first: f32[4096]
  first[:] = 1.0
  total = first[0]
  second: f32[4096]
  second[:] = 2.0
  total += second[0]
  third: f32[4096]
  third[:] = 3.0
  out1 = total + third[0]
"#,
    );
    assert_eq!(scratch_array_count(&disjoint, 4096), 1);

    let overlapping = compile(
        r#"
sample:
  first: f32[4096]
  first[:] = 1.0
  second: f32[4096]
  second[:] = 2.0
  out1 = first[0] + second[0]
"#,
    );
    assert_eq!(scratch_array_count(&overlapping, 4096), 2);

    let view_keeps_backing_live = compile(
        r#"
sample:
  first: f32[4096]
  first[:] = 1.0
  view: f32[] = first[:]
  second: f32[4096]
  second[:] = 2.0
  out1 = view[0] + second[0]
"#,
    );
    assert_eq!(scratch_array_count(&view_keeps_backing_live, 4096), 2);
}

#[test]
fn explicit_non_array_field_slices_remain_rejected_by_init_and_runtime_analysis() {
    for source in [
        r#"
struct Holder:
  value = 2.0
init:
  holder = Holder()
  holder.value[:] = 1.0
sample:
  out1 = holder.value
"#,
        r#"
struct Holder:
  value = 2.0
init:
  holder = Holder()
sample:
  holder.value[:] = 1.0
  out1 = holder.value
"#,
    ] {
        let parsed = onda_frontend::parse_program(source).expect("data source parses");
        let errors = crate::analyze(parsed).expect_err("scalar field slices must be rejected");
        assert!(
            errors.iter().any(|error| error
                .message
                .contains("field 'holder.value' is not array and cannot be sliced")),
            "{errors:?}"
        );
    }
}

#[test]
fn struct_constructors_only_accept_authored_fields() {
    let source = r#"
struct Inner:
  value = 1.0
struct Outer:
  inner: Inner
sample:
  value = Outer(Inner(), 5.0)
  out1 = value.inner.value
"#;
    let errors = crate::analyze(onda_frontend::parse_program(source).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("too many positional arguments")),
        "{errors:?}"
    );
}

#[test]
fn deeply_nested_struct_metadata_is_linear_and_constructors_compile() {
    let depth = 64;
    let mut source = String::new();
    for index in 0..depth {
        source.push_str(&format!("struct S{index}:\n"));
        if index + 1 == depth {
            source.push_str("  value = 1.0\n");
        } else {
            source.push_str(&format!("  next: S{}\n", index + 1));
        }
    }
    source.push_str("sample:\n  value = S0()\n  out1 = 0.0\n");
    let parsed = onda_frontend::parse_program(&source).expect("nested structs parse");
    let typed = crate::analyze(parsed).expect("nested structs analyze");
    assert_eq!(
        typed
            .structs
            .iter()
            .map(|def| def.fields.len())
            .sum::<usize>(),
        depth,
        "typed structs must retain only directly declared fields"
    );
    lower_program_to_optimized_mir(&typed).expect("nested structs lower");
}

#[test]
fn fixed_primitive_array_initialization_is_uniform_across_init_scopes() {
    compile(
        r#"
proc Bank:
  init:
    source: f32[4] = [1.0, 2.0, 3.0, 4.0]
    selected: f32[] = source[1:3]
    saved: f32[2] = selected
    saved = [5.0, 7.0]
  sample:
    out1 = saved[0]

init:
  source: f32[4] = [1.0, 2.0, 3.0, 4.0]
  selected: f32[] = source[1:3]
  saved: f32[2] = selected
  saved = [5.0, 7.0]
  bank = Bank()

sample:
  out1 = saved[0] + bank()
"#,
    );
}

#[test]
fn fixed_primitive_array_init_rejects_mismatched_slices_and_literals() {
    for statement in [
        "saved: f32[3] = source[1:3]",
        "saved: f32[2]\n  saved = [1.0]",
        "saved: f32[2]\n  saved = [1.0, true]",
    ] {
        let source = format!(
            "init:\n  source: f32[4] = [1.0, 2.0, 3.0, 4.0]\n  {statement}\nsample:\n  out1 = 0.0\n"
        );
        assert!(
            crate::analyze(onda_frontend::parse_program(&source).unwrap()).is_err(),
            "invalid init operation analyzed: {source}"
        );
    }
}

#[test]
fn fixed_copy_diagnostics_reject_wrong_shapes_and_slice_rebinding() {
    for statement in [
        "saved: f32[3] = values",
        "saved: f32[2] = 1.0",
        "view = values[:]\n  view = values",
        "values = [1.0]",
        "values = [1.0, true]",
    ] {
        let source = format!("sample:\n  values: f32[2]\n  {statement}\n  out1 = 0.0\n");
        let parsed = onda_frontend::parse_program(&source).expect("data expressions parse");
        assert!(
            crate::analyze(parsed).is_err(),
            "invalid fixed-data operation analyzed: {source}"
        );
    }
}

#[test]
fn structured_array_and_slice_contracts_reject_invalid_initializers() {
    for scope in ["init", "sample"] {
        for initializer in ["[Note()]", "[Note(), Other()]"] {
            let source = format!("struct Note:\n  value = 1.0\nstruct Other:\n  value = 2.0\n{scope}:\n  notes: Note[2] = {initializer}\n");
            let parsed = onda_frontend::parse_program(&source).unwrap();
            assert!(crate::analyze(parsed).is_err(), "{source}");
        }
    }
    for body in [
        "notes: Note[2] = [Note()]",
        "notes: Note[2] = [Note(), Other()]",
        "notes: Note[2]\n  notes[0] = Other()",
        "notes: Note[2]\n  notes[:] = Other()",
        "notes: Note[2]\n  view: Other[] = notes[:]",
        "notes: Note[2]\n  view = notes[:]\n  view = notes[:]",
        "notes: Note[2]\n  bound = 1\n  copy: Note[1] = notes[bound:]",
        "values: f32[2]\n  bound = 1\n  copy: f32[1] = values[bound:]",
        "values: f32[2]\n  copy: f32[1] = values[1:1]",
    ] {
        let source = format!("struct Note:\n  value = 1.0\nstruct Other:\n  value = 2.0\nsample:\n  {body}\n  out1 = 0.0\n");
        let parsed = onda_frontend::parse_program(&source).expect("invalid operation should parse");
        assert!(
            crate::analyze(parsed).is_err(),
            "invalid operation was accepted: {source}"
        );
    }
    assert!(onda_frontend::parse_program("sample:\n  view: f32[]\n  out1 = 0.0\n").is_err());
}

#[test]
fn empty_array_literals_cannot_supply_runtime_slice_backing() {
    let cases = [
        (
            r#"
outs 1
sample:
  values: f32[] = []
  out1 = f32(values.len())
"#,
            (4, 19),
        ),
        (
            r#"
def length(values: f32[]) -> i32:
  return values.len()
outs 1
sample:
  out1 = f32(length([]))
"#,
            (6, 21),
        ),
    ];
    for (source, location) in cases {
        let parsed = onda_frontend::parse_program(source).expect("empty literal source parses");
        let errors = crate::analyze(parsed).expect_err("empty literal backing must be rejected");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!((errors[0].line, errors[0].column), location);
        assert!(
            errors.iter().any(|error| error.message
                == "empty array literal cannot provide backing storage for a slice; slice an existing array to create an empty view"),
            "{errors:?}"
        );
    }

    compile(
        r#"
def length(values: f32[]) -> i32:
  return values.len()
outs 1
init:
  source: f32[1]
sample:
  empty: f32[] = source[:0]
  out1 = f32(length(empty))
"#,
    );
}

#[test]
fn fixed_struct_array_initializers_explain_unproven_and_mismatched_slice_lengths() {
    let cases = [
        (
            "def capture(values: Note[]):\n  copy: Note[2] = values\nsample:\n  out1 = 0.0",
            "requires a statically proven exact length",
        ),
        (
            "sample:\n  values: Note[3]\n  copy: Note[2] = values[:]\n  out1 = 0.0",
            "expects 'Note[2]', got 'Note[3]'",
        ),
    ];
    for (body, expected) in cases {
        let source = format!("struct Note:\n  value = 1.0\n{body}\n");
        let errors = crate::analyze(onda_frontend::parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "missing '{expected}' diagnostic: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|error| !error.message.contains("data array element")),
            "slice source was misdiagnosed as an element: {errors:?}"
        );
    }
}

#[test]
fn struct_slice_fill_is_valid_in_top_level_and_proc_init() {
    compile(
        r#"
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
"#,
    );
}

#[test]
fn data_permissions_track_branch_origins_and_independent_copies() {
    let mir = compile(
        r#"
struct Note:
  value = 1.0
def mutate(note: Note):
  note.value = 7.0
def mutate_selected(a: Note, b: Note, choose: bool):
  if choose:
    selected = a
  else:
    selected = b
  mutate(selected)
def read_selected(a: Note, b: Note, choose: bool):
  if choose:
    selected = a
  else:
    selected = b
  return selected.value
def changed_copy(note: Note):
  copy: Note = note
  mutate(copy)
  return copy.value
sample:
  a = Note()
  b = Note()
  mutate_selected(a, b, false)
  out1 = read_selected(a, b, true) + changed_copy(a)
"#,
    );
    let get = |name: &str| {
        mir.functions
            .iter()
            .find(|function| function.name == name)
            .unwrap()
    };
    for name in ["a.value", "b.value"] {
        assert_eq!(
            get("mutate_selected")
                .params
                .iter()
                .find(|param| param.name == name)
                .unwrap()
                .mode,
            onda_mir::PassingMode::ReadWriteReference
        );
        assert_eq!(
            get("read_selected")
                .params
                .iter()
                .find(|param| param.name == name)
                .unwrap()
                .mode,
            onda_mir::PassingMode::ReadOnlyReference
        );
    }
    assert_eq!(
        get("changed_copy").params[0].mode,
        onda_mir::PassingMode::ReadOnlyReference
    );
}

#[test]
fn proc_retention_respects_backing_lifetimes() {
    let source = r#"
struct Note:
  value = 1.0
proc Worker:
  init:
    if true:
      temporary = Note()
    else:
      temporary = Note()
    saved = temporary
  sample:
    out1 = saved.value
init:
  worker = Worker()
sample:
  out1 = worker()
"#;
    let errors = crate::analyze(onda_frontend::parse_program(source).unwrap()).unwrap_err();
    let error = errors
        .iter()
        .find(|error| error.message.contains("init-local"))
        .unwrap_or_else(|| panic!("{errors:?}"));
    assert!(error.message.contains("'saved'"), "{error:?}");
    assert_eq!(error.line, 10, "{error:?}");
    assert!(error.column > 0, "{error:?}");
    compile(
        r#"
proc Worker:
  buffers:
    source: f32
  block:
    view = source[:]
    count = view.len()
    sample:
      out1 = f32(count)
buffers:
  input: f32
init:
  worker = Worker(source = input)
sample:
  out1 = worker()
"#,
    );
}

#[test]
fn retained_view_metadata_is_independent_of_array_extent() {
    let source = |len| {
        format!(
            r#"
struct Note:
  value = 1.0
  bins: f32[2]
proc Worker:
  init:
    notes: Note[{len}]
    index: i32 = 1
    initial = notes[index]
  block:
    selected = notes[index]
    sample:
      out1 = selected.value + initial.value
init:
  worker = Worker()
sample:
  out1 = worker()
"#
        )
    };
    let small = compile(&source(4));
    let large = compile(&source(4096));
    assert_eq!(small.state.len(), large.state.len());
    assert_eq!(small.functions.len(), large.functions.len());
    assert_eq!(
        small
            .functions
            .iter()
            .map(|function| function.locals.len())
            .collect::<Vec<_>>(),
        large
            .functions
            .iter()
            .map(|function| function.locals.len())
            .collect::<Vec<_>>()
    );
}

#[test]
fn message_payload_permissions_follow_aliases_and_transitive_calls() {
    for body in [
        "alias = values\n  alias[0] = 9",
        "alias = values[:1]\n  alias[:] = [9]",
        "mutate(values)",
        "alias = values\n  forward(alias)",
    ] {
        let source = format!(
            r#"
def mutate(values: i32[]):
  values[0] = 9
def forward(values: i32[]):
  mutate(values)
delegate changed(values: i32[])
when changed(values):
  {body}
init:
  values: i32[2] = [1, 2]
sample:
  changed(values)
  out1 = 0.0
"#
        );
        let errors = crate::analyze(onda_frontend::parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("read-only")),
            "{errors:?}"
        );
    }
    compile(
        r#"
def mutate(values: i32[]):
  values[0] = 9
delegate changed(values: i32[2])
when changed(values):
  independent: i32[2] = values
  mutate(independent)
init:
  values: i32[2] = [1, 2]
sample:
  changed(values)
  out1 = 0.0
"#,
    );
}

#[test]
fn nominal_event_payloads_borrow_canonical_fixed_tensors() {
    compile(
        r#"
struct Note:
  gain: f64
  bins: f32[2]
def read(note: Note) -> f64:
  return note.gain + f64(note.bins[0])
init:
  result: f64 = 0.0
event changed(note: Note):
  result = read(note)
sample:
  out1 = f32(result)
"#,
    );
}

#[test]
fn nominal_delegate_payloads_forward_live_references() {
    compile(
        r#"
struct Note:
  gain: f64
  bins: f32[2]
init:
  note: Note
  result: f64 = 0.0
delegate changed(note: Note)
when changed(next):
  result = next.gain + f64(next.bins[0])
sample:
  changed(note)
  out1 = f32(result)
"#,
    );
}

#[test]
fn proc_events_forward_nominal_payloads_to_delegates() {
    compile(
        r#"
struct Note:
  gain: f64
  bins: f32[2]
proc Worker:
  init:
    note: Note
  delegate changed(note: Note)
  event change(next: Note):
    note = next
    changed(note)
  sample:
    out1 = f32(note.gain)
init:
  worker = Worker()
  result: f64 = 0.0
when worker.changed(next):
  result = next.gain
event changed(note: Note):
  worker.change(note)
sample:
  out1 = worker() + f32(result)
"#,
    );
}

#[test]
fn tuple_message_values_and_defaults_survive_routing() {
    compile(
        r#"
proc Worker:
  init:
    result: f64 = 0.0
  delegate changed(pair: (i32, f64))
  event change(pair: (i32, f64) = (3, 5.0)):
    a, b = pair
    result = f64(a) + b
    changed(pair)
  sample:
    out1 = f32(result)
init:
  worker = Worker()
  result: f64 = 0.0
delegate changed(pair: (i32, f64))
when worker.changed(pair):
  changed(pair)
when changed(pair):
  a, b = pair
  result = f64(a) + b
event change(pair: (i32, f64) = (7, 11.0)):
  worker.change(pair)
sample:
  worker.change()
  out1 = worker() + f32(result)
"#,
    );
}

#[test]
fn runtime_struct_slices_route_through_events_delegates_and_proc_handlers() {
    let source = r#"
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
delegate configured(notes: Note[])
when configured(notes):
  saved[:] = notes[:]
  count = notes.len()
when bank.accepted(notes):
  configured(notes)
event configure(notes: Note[]):
  bank.configure(notes)
sample:
  note = saved[0]
  out1 = f32(note.gain) + f32(count)
"#;
    let mir = compile(source);
    assert_eq!(mir.interface.events[0].params.len(), 4);
}

#[test]
fn borrowed_data_parameters_reject_defaults() {
    for source in [
        "struct N:\n  x = 1.0\ndef f(value: N = N()):\n  return value.x\nsample:\n  out1 = 0.0\n",
        "struct N:\n  x = 1.0\ndef f(values: N[2] = N()):\n  return values[0].x\nsample:\n  out1 = 0.0\n",
        "struct N:\n  x = 1.0\nevent set(value: N = N()):\n  selected = value\nsample:\n  out1 = 0.0\n",
        "struct N:\n  x = 1.0\ndelegate sent(value: N = N())\nsample:\n  out1 = 0.0\n",
        "struct N:\n  x = 1.0\nproc P:\n  event set(value: N = N()):\n    selected = value\n  sample:\n    out1 = 0.0\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
        "struct N:\n  x = 1.0\nproc P:\n  delegate sent(values: N[2] = N())\n  sample:\n    out1 = 0.0\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
    ] {
        let parsed = onda_frontend::parse_program(source).unwrap();
        let errors = crate::analyze(parsed).unwrap_err();
        assert!(
            errors.iter().any(|error| error
                .message
                .contains("cannot have a default")),
            "{errors:?}"
        );
    }
}

#[test]
fn named_struct_array_field_chains_and_explicit_constructor_arguments_compile() {
    let source = r#"
struct Note:
  velocity = 1.0
  bins: f32[2]
struct Patch:
  notes: Note[2]
struct Settings:
  gain = 1.0
def apply(settings: Settings):
  settings.gain *= 0.5
sample:
  current: Patch
  current.notes[0].velocity = 0.75
  apply(Settings())
  out1 = current.notes[0].velocity + current.notes[0].bins[1]
"#;
    compile(source);

    compile(
        r#"
struct Note:
  velocity = 1.0
struct Patch:
  notes: Note[2]
proc Holder:
  init:
    current: Patch
  sample:
    current.notes[0].velocity = 0.25
    out1 = current.notes[0].velocity
init:
  holder = Holder()
sample:
  out1 = holder()
"#,
    );
}

#[test]
fn aggregate_constructor_and_primitive_default_errors_keep_element_and_shape_checks() {
    for (source, expected) in [
        (
            "struct N:\n  a: f32[2]\nsample:\n  n = N(a = [3.0])\n  out1 = 0.0\n",
            "expects 2 elements",
        ),
        (
            "struct N:\n  a: f32[2]\nsample:\n  n = N(a = [true, false])\n  out1 = 0.0\n",
            "array initializer",
        ),
        (
            "def f(a: f32[2] = [1.0]):\n  return a[0]\nsample:\n  out1 = f()\n",
            "expects array length 2",
        ),
        (
            "def f(a: f32[2] = [unknown, 1.0]):\n  return a[0]\nsample:\n  out1 = f()\n",
            "non-constant symbol",
        ),
    ] {
        let parsed = onda_frontend::parse_program(source).unwrap();
        let errors = crate::analyze(parsed).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "{expected}: {errors:?}"
        );
    }
}

#[test]
fn call_result_field_selection_uses_source_level_diagnostic() {
    let parsed = onda_frontend::parse_program(
        "struct Box:\n  value: f32\ndef make() -> Box:\n  return Box(1.0)\nsample:\n  out1 = make().value\n",
    )
    .unwrap();
    let errors = crate::analyze(parsed).unwrap_err();
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("field selection on function result 'make(...)' is not supported; bind the result first")
    }), "{errors:?}");
    assert!(errors
        .iter()
        .all(|error| !error.message.contains("__onda_proc_field__")));
}
