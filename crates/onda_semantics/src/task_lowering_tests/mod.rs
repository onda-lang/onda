use super::*;
use crate::{analyze, lower_program_to_optimized_mir};
use onda_frontend::parse_program;

fn validate(source: &str) -> Vec<Diagnostic> {
    let program = parse_program(source).expect("task source should parse");
    let mut errors = Vec::new();
    validate_task_source_model(&program, &mut errors);
    errors
}

fn dispatch_depth(stmts: &[Stmt]) -> usize {
    stmts
        .iter()
        .map(|stmt| match stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => 1 + dispatch_depth(then_branch).max(dispatch_depth(else_branch)),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

fn mir_call_count(block: &onda_mir::Block, target: onda_mir::FunctionId) -> usize {
    block
        .statements
        .iter()
        .map(|statement| match &statement.kind {
            onda_mir::StatementKind::Call { function, .. } => usize::from(*function == target),
            onda_mir::StatementKind::If {
                then_block,
                else_block,
                ..
            } => mir_call_count(then_block, target) + mir_call_count(else_block, target),
            onda_mir::StatementKind::Loop { body } => mir_call_count(body, target),
            _ => 0,
        })
        .sum()
}

fn mir_slice_fill_count(block: &onda_mir::Block) -> usize {
    block
        .statements
        .iter()
        .map(|statement| match &statement.kind {
            onda_mir::StatementKind::SliceFill { .. } => 1,
            onda_mir::StatementKind::If {
                then_block,
                else_block,
                ..
            } => mir_slice_fill_count(then_block) + mir_slice_fill_count(else_block),
            onda_mir::StatementKind::Loop { body } => mir_slice_fill_count(body),
            _ => 0,
        })
        .sum()
}

#[test]
fn task_dispatch_is_balanced() {
    let arms = (0..64)
        .map(|id| vec![assign_var("selected", Expr::int(id))])
        .collect();
    let dispatch = build_task_dispatch(arms, 0, TASK_NODE_LOCAL);
    assert_eq!(dispatch_depth(&dispatch), 6);
}

#[test]
fn accepts_well_placed_task_controls() {
    let errors = validate(
            "proc P:\n  tasks:\n    load():\n      yield\n      return\n  event restart():\n    load.reset()\n  block:\n    await load()\n    sample:\n      out1 = 0.0\n",
        );
    assert!(errors.is_empty(), "unexpected diagnostics: {errors:?}");
}

#[test]
fn accepts_well_placed_top_level_task_controls() {
    let errors = validate(
            "task load():\n  yield\n  return\nevent restart():\n  load.reset()\nblock:\n  await load()\n  sample:\n    out1 = 0.0\n",
        );
    assert!(errors.is_empty(), "unexpected diagnostics: {errors:?}");
}

#[test]
fn task_loop_control_requires_an_enclosing_loop() {
    let top_level = validate(
            "task load():\n  if true:\n    break\n  continue\nblock:\n  await load()\n  sample:\n    out1 = 0.0\n",
        );
    for keyword in ["break", "continue"] {
        assert!(
            top_level.iter().any(|error| error
                .message
                .contains(&format!("{keyword} is only allowed inside"))),
            "missing {keyword} diagnostic in {top_level:?}"
        );
    }

    let proc_task = validate(
            "proc P:\n  task load():\n    break\n  block:\n    await load()\n    sample:\n      out1 = 0.0\n",
        );
    assert!(proc_task
        .iter()
        .any(|error| error.message.contains("break is only allowed inside")));

    let valid = validate(
            "task load():\n  while true:\n    break\n  for i in 0..2:\n    continue\nblock:\n  await load()\n  sample:\n    out1 = 0.0\n",
        );
    assert!(valid.is_empty(), "unexpected diagnostics: {valid:?}");
}

#[test]
fn task_locals_use_shared_expression_typing() {
    let source = r#"
struct Counter:
  value: i32 = 7

  def read(self) -> i32:
    return self.value

buffers:
  impulse: f32[2]

def clamp_count(value: i32) -> i32:
  return max(value, 1)

init:
  counter = Counter()

task load():
  frames = min(impulse.len(), 480000)
  count = clamp_count(frames)
  field_value = counter.value
  method_value = counter.read()
  yield
  count = count + field_value + method_value

event reload():
  load.reset()

block:
  await load()
  sample:
    out1 = 0.0
"#;

    crate::analyze(onda_frontend::parse_program(source).expect("source should parse"))
        .expect("task locals should follow ordinary expression typing rules");
}

#[test]
fn executable_scopes_share_scalar_inference() {
    let source = r#"
struct Counter:
  value: i32 = 7

  def read(self) -> i32:
    return self.value

def require_i32(value: i32) -> i32:
  return value

def read_counter(counter: Counter) -> i32:
  value = counter.read()
  return require_i32(value)

proc Reader:
  init:
    counter = Counter()
    init_value = counter.read()
    init_checked = require_i32(init_value)

  task prepare():
    task_value = counter.read()
    task_checked = require_i32(task_value)
    yield

  event restart():
    event_value = counter.read()
    event_checked = require_i32(event_value)
    prepare.reset()

  block:
    block_value = counter.read()
    block_checked = require_i32(block_value)
    await prepare()

    sample:
      sample_value = counter.read()
      out1 = f32(require_i32(sample_value) + read_counter(counter))

init:
  reader = Reader()

sample:
  out1 = reader()
"#;

    crate::analyze(onda_frontend::parse_program(source).expect("source should parse"))
        .expect("all executable scopes should share scalar inference");
}

#[test]
fn rejects_invalid_task_control_placement_and_targets() {
    let cases = [
            (
                "proc P:\n  init:\n    yield\n  sample:\n    out1 = 0.0\n",
                "yield is only allowed inside a task body",
            ),
            (
                "proc P:\n  task load():\n    return\n  sample:\n    await load()\n    out1 = 0.0\n",
                "await is only allowed",
            ),
            (
                "proc P:\n  task load():\n    return\n  block:\n    await missing()\n    sample:\n      out1 = 0.0\n",
                "unknown task 'missing'",
            ),
            (
                "proc P:\n  task load():\n    return 1.0\n  sample:\n    out1 = 0.0\n",
                "tasks cannot return a value",
            ),
            (
                "proc P:\n  task load():\n    return\n  sample:\n    load.reset()\n    out1 = 0.0\n",
                "can only be reset from init, event, or block-pre",
            ),
        ];

    for (source, expected) in cases {
        let errors = validate(source);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "missing '{expected}' in {errors:?}"
        );
    }
}

#[test]
fn rejects_task_member_conflicts_and_graph_owners() {
    let conflicts = validate(
            "proc P:\n  task load():\n    return\n  def load():\n    return\n  sample:\n    out1 = 0.0\n",
        );
    assert!(conflicts
        .iter()
        .any(|error| error.message.contains("conflicts with proc-local def")));

    let proc_const_conflict = validate(
        "proc P:\n  const load = 1\n  task load():\n    return\n  sample:\n    out1 = 0.0\n",
    );
    assert!(proc_const_conflict
        .iter()
        .any(|error| error.message.contains("conflicts with constant")));

    let proc_state_conflict = validate(
            "proc P:\n  init:\n    load: i32 = 0\n  task load():\n    return\n  sample:\n    out1 = 0.0\n",
        );
    assert!(proc_state_conflict
        .iter()
        .any(|error| error.message.contains("conflicts with state root")));

    let graph = validate("proc P:\n  task load():\n    return\n  graph:\n    source() >> out1\n");
    assert!(graph
        .iter()
        .any(|error| error.message.contains("tasks together with a graph block")));

    let top_conflict =
        validate("init:\n  load: i32 = 0\ntask load():\n  return\nsample:\n  out1 = 0.0\n");
    assert!(top_conflict
        .iter()
        .any(|error| error.message.contains("conflicts with state root")));

    let top_struct_conflict =
        validate("struct load:\n  value: i32 = 0\ntask load():\n  return\nsample:\n  out1 = 0.0\n");
    assert!(top_struct_conflict
        .iter()
        .any(|error| error.message.contains("conflicts with struct")));

    let top_graph = validate("task load():\n  return\ngraph:\n  source() >> out1\n");
    assert!(top_graph.iter().any(|error| error
        .message
        .contains("tasks cannot be declared together with a graph")));
}

#[test]
fn rejects_task_conflicts_with_inferred_numbered_io() {
    let proc_errors = validate("proc P:\n  task out1():\n    yield\n  sample:\n    out1 = 0.0\n");
    assert!(proc_errors
        .iter()
        .any(|error| error.message.contains("conflicts with output 'out1'")));

    let top_errors = validate("task out1():\n  yield\nsample:\n  out1 = 0.0\n");
    assert!(top_errors
        .iter()
        .any(|error| error.message.contains("conflicts with output 'out1'")));
}

#[test]
fn top_level_tasks_open_the_sample_gate_without_an_explicit_block() {
    let source = "task unused():\n  yield\nsample:\n  out1 = 1.0\n";
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("top-level task without a block should analyze");

    assert!(typed.block_pre.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign {
            target: AssignTarget::Var(name),
            expr: Expr::Bool { value: true, .. },
            ..
        } if name == TASK_AVAILABLE_FIELD
    )));
}

#[test]
fn rejects_proc_task_constant_conflicts_before_folding_await_markers() {
    let source = r#"
proc P:
  const prepare = 1
  task prepare():
    yield
  block:
    await prepare()
    sample:
      out1 = 0.0
"#;
    let errors = analyze(parse_program(source).expect("task source should parse"))
        .expect_err("task and constant names should conflict");
    assert!(errors.iter().any(|error| error
        .message
        .contains("task 'prepare' in processor 'P' conflicts with local constant")));
    assert!(errors.iter().all(|error| !error
        .message
        .contains("malformed internal task await marker")));
}

#[test]
fn rejects_task_io_access() {
    let errors = analyze(
        parse_program(
            r#"
proc Child:
  sample:
    out1 = in1
proc Owner:
  init:
    child = Child()
    children: Child[2] = Child()
  task load():
    value = in1
    out1 = value
    child()
    children[0]()
  block:
    await load()
    sample:
      out1 = 0.0
"#,
        )
        .expect("task source should parse"),
    )
    .expect_err("task I/O access should fail shared semantic analysis");
    for expected in [
        "unknown symbol 'in1' in expression",
        "I/O symbol 'out1' is only available in block or sample",
    ] {
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "missing '{expected}' in {errors:?}"
        );
    }
}

#[test]
fn task_proc_calls_follow_ordinary_rate_rules() {
    let block_rate = r#"
proc Meter:
  kouts:
    value
  params:
    offset: f64 = 0.0
  block:
    value = f32(1.0 + offset)

proc Owner:
  init:
    meter = Meter()
    meters: Meter[2] = Meter()
    pin result = 0.0
  def read_meter():
    return meter()
  task prepare():
    result = meter(offset = 1.0) + meters[0](offset = 2.0) + read_meter()
    yield
  block:
    await prepare()
    sample:
      out1 = result

init:
  owner = Owner()
sample:
  out1 = owner()
"#;
    let typed = analyze(parse_program(block_rate).expect("task source should parse"))
        .expect("direct, indexed, and def-mediated block-rate proc calls should analyze");
    lower_program_to_optimized_mir(&typed)
        .expect("block-rate proc calls in tasks should lower to valid MIR");

    let sample_rate = r#"
proc Voice:
  sample:
    out1 = 1.0

proc Owner:
  init:
    voice = Voice()
    voices: Voice[2] = Voice()
  def read_voice():
    return voice()
  task prepare():
    __TASK_BODY__
    yield
  block:
    await prepare()
    sample:
      out1 = 0.0

init:
  owner = Owner()
sample:
  out1 = owner()
"#;
    for (kind, body) in [
        ("direct", "value = voice()"),
        ("indexed", "value = voices[0]()"),
        ("def-mediated", "value = read_voice()"),
    ] {
        let source = sample_rate.replace("__TASK_BODY__", body);
        let errors = analyze(parse_program(&source).expect("task source should parse"))
            .expect_err(&format!("{kind} sample-rate proc call should be rejected"));
        assert!(
            errors.iter().any(|error| {
                error.message.contains("sample-rate proc")
                    && error.message.contains("not provably sample-only")
            }),
            "unexpected diagnostics for {kind} call: {errors:?}"
        );
    }
}

#[test]
fn top_level_task_can_call_block_rate_proc() {
    let source = r#"
proc Meter:
  kouts:
    value
  block:
    value = 1.0

init:
  meter = Meter()
  pin result = 0.0
task prepare():
  result = meter()
  yield
block:
  await prepare()
  sample:
    out1 = result
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a top-level task should allow block-rate proc calls");
    lower_program_to_optimized_mir(&typed)
        .expect("a top-level block-rate proc call should lower to valid MIR");
}

#[test]
fn permits_child_proc_events_in_tasks() {
    let source = r#"
proc Child:
  init:
    value: i32 = 0
  event add(amount: i32):
    value += amount
  sample:
    out1 = f32(value)

proc Owner:
  init:
    child = Child()
    children: Child[2] = Child()
  task prepare():
    child.add(1)
    index: i32 = 1
    children[index].add(2)
    child.init()
  block:
    await prepare()
    sample:
      out1 = child() + children[1]()

init:
  owner = Owner()
sample:
  out1 = owner()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("child proc events in tasks should analyze");
    lower_program_to_optimized_mir(&typed)
        .expect("child proc events in tasks should lower to valid MIR");
}

#[test]
fn permits_child_proc_events_in_top_level_tasks() {
    let source = r#"
proc Child:
  init:
    value: i32 = 0
  event add(amount: i32):
    value += amount
  sample:
    out1 = f32(value)

init:
  child = Child()

task prepare():
  child.add(2)
  yield
  child.add(3)

block:
  await prepare()
  sample:
    out1 = child()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("child proc events in top-level tasks should analyze");
    lower_program_to_optimized_mir(&typed)
        .expect("child proc events in top-level tasks should lower to valid MIR");
}

#[test]
fn permits_tasks_to_observe_and_write_resettable_state() {
    let source = r#"
proc Loader:
  init:
    progress: i32 = 0
  task load():
    progress += 1
    yield
  block:
    await load()
    sample:
      out1 = 0.0
"#;
    let parsed = parse_program(source).expect("task source should parse");
    analyze(parsed).expect("resettable task state should be accepted");
}

#[test]
fn top_level_task_uses_branch_joined_init_state_types() {
    let source = r#"
params:
  choose_first: bool = true
init:
  if choose_first:
    candidate: i32 = 1
  else:
    candidate: i32 = 2
  carried = candidate
  pin result: i32 = 0
task prepare():
  result = carried
  yield
block:
  await prepare()
  sample:
    out1 = f32(result)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task should see the canonical type of branch-joined init state");
    lower_program_to_optimized_mir(&typed)
        .expect("branch-joined top-level task state should lower to valid MIR");
}

#[test]
fn proc_task_uses_branch_joined_init_state_types() {
    let source = r#"
proc Loader:
  params:
    choose_first: bool = true
  init:
    if choose_first:
      candidate: i64 = 1
    else:
      candidate: i64 = 2
    carried = candidate
    pin result: i64 = 0
  task prepare():
    result = carried
    yield
  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("proc task should see the canonical type of branch-joined init state");
    lower_program_to_optimized_mir(&typed)
        .expect("branch-joined proc task state should lower to valid MIR");
}

#[test]
fn task_cannot_see_branch_local_init_bindings() {
    let source = r#"
params:
  choose_first: bool = true
init:
  if choose_first:
    candidate: i32 = 1
  else:
    candidate: i32 = 2
  carried = candidate
task prepare():
  carried = candidate
  yield
block:
  await prepare()
  sample:
    out1 = f32(carried)
"#;
    let errors = analyze(parse_program(source).expect("task source should parse"))
        .expect_err("branch-local init bindings must not escape into tasks");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("unknown symbol 'candidate'")),
        "unexpected diagnostics: {errors:?}"
    );
}

#[test]
fn lowers_tasks_to_executable_runtime_defs() {
    let source = r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 1

  block:
    await load()

    sample:
      out1 = f32(progress)

init:
  loader = Loader()

sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task source should lower and analyze");
    assert!(typed
        .defs
        .iter()
        .any(|def| def.name.contains(&task_resume_def("load"))));
    assert!(typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_pc_field("load"))));
    let mir =
        lower_program_to_optimized_mir(&typed).expect("lowered task should produce valid MIR");
    assert!(mir
        .state
        .iter()
        .any(|slot| { slot.name.contains(&task_pc_field("load")) && slot.pinned }));
    assert!(mir
        .state
        .iter()
        .any(|slot| { slot.name == "loader.progress" && slot.pinned }));
}

#[test]
fn top_level_task_resume_is_a_shared_compiler_function() {
    let source = r#"
init:
  pin progress: i32 = 0
task prepare():
  progress += 1
  yield
  progress += 1
block:
  await prepare()
  sample:
    out1 = f32(progress)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("task source should lower");

    let resume = mir
        .functions
        .iter()
        .find(|function| function.name == task_resume_def("prepare"))
        .expect("missing shared task resume function");
    assert_eq!(
        resume.attributes.origin,
        onda_mir::FunctionOrigin::CompilerGenerated
    );
    assert_eq!(resume.attributes.inline, onda_mir::InlineHint::Never);

    let result = mir
        .state
        .iter()
        .find(|slot| slot.name == task_runtime_result_field("prepare"))
        .expect("missing task result scratch state");
    assert_eq!(
        result.persistence,
        onda_mir::StatePersistence::InstanceScratch
    );
    assert!(!result.pinned);

    let pc = mir
        .state
        .iter()
        .find(|slot| slot.name == task_pc_field("prepare"))
        .expect("missing task program counter");
    assert_eq!(pc.persistence, onda_mir::StatePersistence::Snapshot);
    assert!(pc.pinned);
}

#[test]
fn repeated_top_level_awaits_call_one_shared_resume_body() {
    let source = r#"
init:
  pin progress: i32 = 0
task prepare():
  progress += 1
  yield
  progress += 1
block:
  await prepare()
  await prepare()
  sample:
    out1 = f32(progress)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("repeated awaits should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("task source should lower");
    let resume_index = mir
        .functions
        .iter()
        .position(|function| function.name == task_resume_def("prepare"))
        .expect("missing shared task resume function");
    let resume = onda_mir::FunctionId::new(resume_index as u32);

    assert_eq!(
        mir.functions
            .iter()
            .filter(|function| function.name == task_resume_def("prepare"))
            .count(),
        1
    );
    assert_eq!(mir_call_count(&mir.functions[1].body, resume), 2);
}

#[test]
fn task_reset_does_not_clear_continuation_storage() {
    let source = r#"
init:
  pin result: i32 = 0
task prepare():
  carried: i32[4096]
  carried[0] = 1
  yield
  result = carried[0]
event restart():
  prepare.reset()
block:
  await prepare()
  sample:
    out1 = f32(result)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("array-backed task frame should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("task source should lower");
    let reset = mir
        .functions
        .iter()
        .find(|function| function.name == task_reset_def("prepare"))
        .expect("missing task reset function");

    assert_eq!(mir_slice_fill_count(&reset.body), 0);
    assert!(mir_slice_fill_count(&mir.functions[0].body) > 0);
}

#[test]
fn task_frames_only_store_locals_live_across_yield() {
    let source = r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    scratch: i32 = 10
    carried: i32 = scratch + 1
    yield
    result = carried
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task source should analyze");
    assert!(typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_local_field("load", "carried"))));
    assert!(!typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_local_field("load", "scratch"))));
}

#[test]
fn task_allows_reference_locals_that_are_dead_before_yield() {
    let source = r#"
buffers:
  data: f32

init:
  pin observed: i32 = 0
task load():
  window = data[:]
  observed = window.len()
  yield

block:
  await load()

sample:
  out1 = f32(observed)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a reference local that dies before yield should analyze");
    assert!(!typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_local_field("load", "window"))));
    lower_program_to_optimized_mir(&typed)
        .expect("the ephemeral reference should remain local to one resume arm");
}

#[test]
fn task_allows_reference_locals_created_after_yield() {
    let source = r#"
buffers:
  data: f32

init:
  pin observed: i32 = 0
task load():
  yield
  window = data[:]
  observed = window.len()

block:
  await load()

sample:
  out1 = f32(observed)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a reference local created after yield should analyze");
    lower_program_to_optimized_mir(&typed)
        .expect("the post-yield reference should remain within its resume arm");
}

#[test]
fn task_rejects_reference_locals_that_cross_yield() {
    let source = r#"
buffers:
  data: f32

init:
  pin observed: i32 = 0
task load():
  window = data[:]
  yield
  observed = window.len()

block:
  await load()

sample:
  out1 = f32(observed)
"#;
    let errors = analyze(parse_program(source).expect("task source should parse"))
        .expect_err("a reference local cannot be stored in a task frame");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("window") && error.message.contains("live across a yield")
        }),
        "unexpected diagnostics: {errors:?}"
    );
}

#[test]
fn task_loop_frame_names_do_not_alias_user_locals() {
    let source = r#"
init:
  pin result: i32 = 0
task prepare():
  i__end: i32 = 99
  for i in 0..2:
    yield
  result = i__end

block:
  await prepare()

sample:
  out1 = f32(result)
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("loop bookkeeping should use collision-free names");
    assert!(typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_local_field("prepare", "i__end"))));
    assert!(typed
        .state_vars
        .iter()
        .any(|name| name.contains(&format!("{}_for_0_end", task_symbol_stem("prepare")))));
    lower_program_to_optimized_mir(&typed)
        .expect("distinct user and loop frame fields should lower");
}

#[test]
fn large_task_scratch_array_uses_one_body_fill() {
    let source = r#"
task prepare():
  scratch: f32[4096]
  scratch[0] = 1.0

block:
  await prepare()

sample:
  out1 = 0.0
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("large task scratch array should analyze");
    let mir =
        lower_program_to_optimized_mir(&typed).expect("large task scratch array should lower");
    let dump = onda_mir::format_program(mir.as_program());
    assert_eq!(
        dump.matches("slice_fill").count(),
        1,
        "task initialization should remain one operation rather than one CFG node per element"
    );
    let scratch_local = dump
        .lines()
        .find(|line| line.contains("\"scratch\"") && line.trim_start().starts_with("local "))
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("scratch array MIR local");
    assert_eq!(
        dump.matches(&format!("{scratch_local}[")).count(),
        1,
        "declaration-only scratch storage must not emit an unrolled zero store per element"
    );
}

#[test]
fn task_branch_bindings_use_canonical_shape_compatibility() {
    let source = r#"
params:
  choose: bool = false
init:
  pin result = 0.0
task prepare():
  if choose:
    carried: f32[2] = [1.0, 2.0]
  else:
    carried: f32[3] = [3.0, 4.0, 5.0]
  yield
  result = carried[0]
block:
  await prepare()
  sample:
    out1 = result
"#;

    let errors = analyze(parse_program(source).expect("task source should parse"))
        .expect_err("different fixed array shapes must not join across task branches");
    assert!(
            errors.iter().any(|error| {
                error.message
                    == "binding 'carried' has incompatible branch types: arrays have different element types or fixed lengths"
            }),
            "unexpected diagnostics: {errors:?}"
        );
}

#[test]
fn initialized_task_array_frame_preserves_its_values() {
    let source = r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    values: i32[2] = [3, 5]
    yield
    result = values[0] + values[1] + values.len()
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("array task should produce valid MIR");
    let dump = onda_mir::format_program(mir.as_program());
    assert!(dump.contains("i32(3)"));
    assert!(dump.contains("i32(5)"));
}

#[test]
fn task_bodies_receive_proc_and_lexical_constant_folding() {
    let source = r#"
proc Loader:
  const Width = 2
  init:
    pin result: i32 = 0
  task load():
    const Left = 3
    values: i32[Width] = [Left, 5]
    yield
    result = values[0] + values[1]
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task constants should fold before lowering");
    lower_program_to_optimized_mir(&typed)
        .expect("task constants should not survive into runtime MIR");
}

#[test]
fn task_frame_types_infer_from_owner_bindings() {
    let source = r#"
proc Loader:
  params:
    parameter: i32 = 7
  buffers:
    samples: f32
  init:
    scalar: i64 = 3
    values: i32[2] = [5, 11]
    pin result = 0.0
  task load():
    scalar_copy = scalar
    array_copy = values[0]
    parameter_copy = parameter
    buffer_copy = samples[0]
    yield
    result = f32(scalar_copy + array_copy + parameter_copy) + buffer_copy
  block:
    await load()
    sample:
      out1 = result
init:
  loader = Loader(parameter = 7, samples = samples)
buffers:
  samples: f32
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task locals should infer from owner binding types");
    for local in ["scalar_copy", "array_copy", "parameter_copy", "buffer_copy"] {
        assert!(
            typed
                .state_vars
                .iter()
                .any(|name| name.contains(&task_local_field("load", local))),
            "missing frame storage for {local}"
        );
    }
    lower_program_to_optimized_mir(&typed)
        .expect("task buffer parameters and inferred frame types should lower");
}

#[test]
fn task_aggregate_fields_remain_owner_state_and_inherit_pinning() {
    let source = r#"
struct Accumulator:
  value: i32 = 0

proc Loader:
  init:
    pin accumulator = Accumulator()
  task load():
    accumulator.value += 1
    yield
    accumulator.value += 1
  block:
    await load()
    sample:
      out1 = f32(accumulator.value)

init:
  loader = Loader()
  pin top = Accumulator()
sample:
  out1 = loader() + f32(top.value)
"#;
    let typed = analyze(parse_program(source).expect("aggregate task source should parse"))
        .expect("aggregate task state should analyze");
    assert!(!typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_local_field("load", "accumulator"))));
    let mir = lower_program_to_optimized_mir(&typed)
        .expect("aggregate task state should lower to valid MIR");
    for name in ["loader.accumulator.value", "top.value"] {
        let slot = mir
            .state
            .iter()
            .find(|slot| slot.name == name)
            .unwrap_or_else(|| panic!("missing flattened state slot {name}"));
        assert!(slot.pinned);
    }
}

#[test]
fn top_level_task_aggregate_fields_remain_owner_state() {
    let source = r#"
struct Accumulator:
  value: i32 = 0

init:
  pin accumulator = Accumulator()
task load():
  accumulator.value += 1
  yield
  accumulator.value += 1
block:
  await load()
  sample:
    out1 = f32(accumulator.value)
"#;
    let typed = analyze(parse_program(source).expect("aggregate task source should parse"))
        .expect("top-level aggregate task state should analyze");
    assert!(!typed
        .state_vars
        .iter()
        .any(|name| name.contains(&task_local_field("load", "accumulator"))));
    let mir = lower_program_to_optimized_mir(&typed)
        .expect("top-level aggregate task state should lower");
    let accumulator = mir
        .state
        .iter()
        .find(|slot| slot.name == "accumulator.value")
        .expect("flattened accumulator state");
    assert!(accumulator.pinned);
}

#[test]
fn proc_task_supports_non_frame_array_locals() {
    let source = r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    values: i32[2] = [3, 5]
    result = values[0] + values[1]
    yield
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("array task source should parse"))
        .expect("non-frame task array should analyze");
    lower_program_to_optimized_mir(&typed).expect("non-frame task array should lower to valid MIR");
}

#[test]
fn tuple_destructured_task_locals_can_cross_yield() {
    let source = r#"
def pair() -> (i32, i32):
  return (3, 5)

proc Loader:
  init:
    pin result: i32 = 0
  task load():
    (left, right) = pair()
    yield
    result = left + right
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let typed = analyze(parse_program(source).expect("tuple task source should parse"))
        .expect("tuple task locals should analyze");
    for local in ["left", "right"] {
        assert!(typed
            .state_vars
            .iter()
            .any(|name| name.contains(&task_local_field("load", local))));
    }
    lower_program_to_optimized_mir(&typed).expect("tuple task locals should lower to valid MIR");
}

#[test]
fn fixed_tuple_task_local_can_cross_yield() {
    let source = r#"
init:
  pin result: f32 = 0.0

task prepare():
  pair = (i32(3), i64(5))
  yield
  result = f32(pair[0]) + f32(pair[1])

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a fixed tuple task local should analyze");
    let mir = lower_program_to_optimized_mir(&typed)
        .expect("a fixed tuple task local should lower to valid MIR");
    let tuple_fields = mir
        .as_program()
        .state
        .iter()
        .filter(|slot| slot.name.contains("prepare_local") && slot.name.contains("pair.__"))
        .collect::<Vec<_>>();
    assert_eq!(tuple_fields.len(), 2);
    assert!(tuple_fields
        .iter()
        .all(|slot| slot.pinned && !slot.authored));
}

#[test]
fn task_for_bounds_use_the_language_induction_coercion() {
    let source = r#"
init:
  pin result: i32 = 0

task prepare():
  for i in (i64(0))..(i64(2)):
    result += i
    yield

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task for bounds should use ordinary loop coercion");
    lower_program_to_optimized_mir(&typed)
        .expect("coerced task for bounds should lower to valid MIR");
}

#[test]
fn task_barriers_neutralize_outputs_by_declared_type_and_shape() {
    let sources = [
        r#"
const Channels = 2

outs:
  ready: bool
  stereo: f32[Channels]

task prepare():
  yield

block:
  await prepare()
  sample:
    ready = true
    stereo[0] = 1.0
    stereo[1] = 2.0
"#,
        r#"
proc Loader:
  outs:
    stereo: f32[2]
  task prepare():
    yield
  block:
    await prepare()
    sample:
      stereo[0] = 1.0
      stereo[1] = 2.0

outs:
  out1
init:
  loader = Loader()
graph:
  loader.stereo[0] >> out1
"#,
    ];

    for source in sources {
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("typed and array outputs should be neutralized correctly");
        lower_program_to_optimized_mir(&typed)
            .expect("typed and array task outputs should lower to valid MIR");
    }
}

#[test]
fn proc_task_barrier_returns_the_declared_scalar_type() {
    let source = r#"
proc Gate:
  outs:
    out1: bool
  task prepare():
    yield
  block:
    await prepare()
    sample:
      out1 = true

outs:
  out1: bool
init:
  gate = Gate()
sample:
  out1 = gate()
"#;

    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a task-gated bool processor output should analyze");
    lower_program_to_optimized_mir(&typed)
        .expect("a task-gated bool processor output should lower to valid MIR");
}

#[test]
fn non_yield_task_with_early_return_keeps_structured_loop_storage() {
    let source = r#"
init:
  pin result: i32 = 0

task prepare():
  for i in 0..4:
    if i == 2:
      return
    result += 1

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("an early task return inside a for loop should analyze");
    let mir = lower_program_to_optimized_mir(&typed)
        .expect("an early task return inside a for loop should lower to valid MIR");
    let generated_stem = task_symbol_stem("prepare");
    assert!(mir
        .state
        .iter()
        .all(|slot| !slot.name.starts_with(&format!("{generated_stem}_for_"))));
}

#[test]
fn return_only_loop_frames_are_not_persisted_by_a_later_yield() {
    let source = r#"
init:
  pin result: i32 = 0

task prepare():
  for i in 0..4:
    if i == result:
      return
  yield

block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a return-only loop before a yield should analyze");
    let mir = lower_program_to_optimized_mir(&typed)
        .expect("a return-only loop before a yield should lower to valid MIR");
    let frame_prefix = format!("{}_for_", task_symbol_stem("prepare"));
    assert!(mir
        .state
        .iter()
        .all(|slot| !slot.name.starts_with(&frame_prefix)));
}

#[test]
fn task_frame_locals_use_inferred_callable_return_types() {
    let sources = [
        r#"
def value():
  result: i64 = 2
  return result

init:
  pin result: i64 = 0

task prepare():
  carried = value()
  yield
  result = carried

block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        r#"
def source_value():
  local: i64 = 2
  return local

proc Loader:
  outs:
    out1
  init:
    pin result: i64 = 0
  def value():
    return source_value()
  task prepare():
    carried = value()
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        r#"
proc Loader:
  outs:
    out1
  init:
    source: i64 = 2
    pin result: i64 = 0
  def value():
    return source
  task prepare():
    carried = value()
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        r#"
struct Counter:
  value: i64 = 2
  def read(self):
    return self.value

init:
  counter = Counter()
  pin result: i64 = 0

task prepare():
  carried = counter.read()
  yield
  result = carried

block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
    ];

    for source in sources {
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("task frame storage should use inferred callable returns");
        lower_program_to_optimized_mir(&typed)
            .expect("inferred callable task locals should lower to valid MIR");
    }
}

#[test]
fn tasks_can_publish_readonly_delegate_array_payloads() {
    let typed = analyze(
        parse_program(
            r#"
const Values: i32[2] = [3, 5]
delegate progress(values: i32[2])
task worker():
  progress(Values)
  yield
block:
  await worker()
  sample:
    out1 = 0.0
"#,
        )
        .expect("task delegate source should parse"),
    )
    .expect("task should accept a const array as a readonly delegate payload");
    lower_program_to_optimized_mir(&typed)
        .expect("readonly task delegate payload should lower to valid MIR");
}

#[test]
fn proc_task_frame_typing_uses_the_selected_global_overload() {
    let sources = [
        r#"
def value(x: i32) -> i32:
  return x + 1
def value(x: f64) -> f64:
  return x + 2.0
proc Loader:
  outs:
    out1
  init:
    pin result: i32 = 0
  task prepare():
    carried = value(i32(3))
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        r#"
def value(x: f64) -> f64:
  return x + 2.0
def value(x: i32) -> i32:
  return x + 1
proc Loader:
  outs:
    out1
  init:
    pin result: i32 = 0
  task prepare():
    carried = value(i32(3))
    yield
    result = carried
  block:
    await prepare()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
    ];

    for source in sources {
        let typed = analyze(parse_program(source).expect("task source should parse"))
            .expect("proc task frame typing should be independent of overload order");
        lower_program_to_optimized_mir(&typed)
            .expect("selected proc task overload should lower to valid MIR");
    }
}

#[test]
fn task_frame_typing_uses_the_selected_method_overload() {
    let source = r#"
struct Calculator:
  def value(self, x: i32) -> i32:
    return x + 1
  def value(self, x: f64) -> f64:
    return x + 2.0
init:
  calculator = Calculator()
  pin result: i32 = 0
task prepare():
  carried = calculator.value(i32(3))
  yield
  result = carried
block:
  await prepare()
  sample:
    out1 = f32(result)
"#;

    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("task frame typing should use ordinary method overload selection");
    lower_program_to_optimized_mir(&typed)
        .expect("selected task method overload should lower to valid MIR");
}

#[test]
fn proc_block_task_barrier_neutralizes_block_timed_outputs() {
    let source = r#"
proc Control:
  kouts 1
  init:
    pin value: i32 = 0
  task load():
    yield
    value += 1
  block:
    await load()
    kout1 = f32(value)

init:
  control = Control()

block:
  kout1 = control().kout1
"#;
    let typed = analyze(parse_program(source).expect("task source should parse"))
        .expect("a task barrier should support block-timed proc outputs");
    lower_program_to_optimized_mir(&typed)
        .expect("block-timed proc task outputs should lower to valid MIR");
}
