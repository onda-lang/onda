use onda_frontend::parse_program;
use onda_mir::{format_program, CompileConfig, Function, FunctionKind, Program};

use super::*;
use crate::{analyze, analyze_with_options};

/// Test-only raw access for structural assertions. Production callers retain
/// the proof-carrying `OptimizedProgram` returned by semantic lowering.
fn lower_test_program(program: &TypedProgram) -> Result<Program, Vec<MirLoweringError>> {
    lower_program_to_optimized_mir(program).map(onda_mir::OptimizedProgram::into_program)
}

fn validate(program: &Program) -> Result<(), Vec<onda_mir::ValidationError>> {
    // SAFETY: these tests inspect MIR produced by this module or construct
    // explicit producer fixtures whose unchecked accesses are intentional.
    unsafe { onda_mir::validate_with_producer_proofs(program) }
}

fn formatted_function<'a>(dump: &'a str, name: &str) -> &'a str {
    let marker = format!("\"{name}\"");
    let name_offset = dump
        .find(&marker)
        .unwrap_or_else(|| panic!("missing MIR function '{name}'"));
    let start = dump[..name_offset]
        .rfind("\nfn @")
        .map_or(0, |offset| offset + 1);
    let end = dump[name_offset..]
        .find("\nfn @")
        .map_or(dump.len(), |offset| name_offset + offset);
    &dump[start..end]
}

fn empty_function(name: &str, kind: FunctionKind) -> Function {
    Function {
        name: name.to_owned(),
        kind,
        attributes: compiler_generated_function_attributes(),
        params: Vec::new(),
        results: Vec::new(),
        locals: Vec::new(),
        body: MirBlock::default(),
        source: SourceSpan::UNKNOWN,
    }
}

#[test]
fn top_level_delegate_lowers_to_descriptor_and_publication() {
    let source = r#"
delegate finished(reason: i32)
init:
  result: i32 = 0
event trigger():
  finished(7)
when finished(reason):
  result = reason
sample:
  out1 = f32(result)
"#;
    let parsed = parse_program(source).expect("delegate source should parse");
    let typed = analyze(parsed).expect("delegate source should analyze");
    let mir = lower_test_program(&typed).expect("delegate source should lower");
    assert_eq!(mir.interface.delegates.len(), 1);
    assert_eq!(mir.interface.delegates[0].name, "finished");
    let dump = format_program(&mir);
    assert!(
        dump.contains("publish_delegate @delegate0"),
        "delegate publication must survive MIR optimization:\n{dump}"
    );
}

#[test]
fn print_sites_preserve_lexical_origins_through_nesting_methods_and_specialization() {
    let source = r#"
def report(value):
  print("report", value)

struct Logger:
  value: i32
  def emit(self):
    print("method", self.value)

proc Child:
  sample:
    print("child", 2)
    out1 = 0.0

proc Parent:
  init:
    child = Child()
  sample:
    print("parent", 3)
    out1 = child()

init:
  parent = Parent()
  logger = Logger(value = 7)

sample:
  report(1)
  report(1.5)
  logger.emit()
  out1 = parent()
"#;
    let parsed = parse_program(source).expect("print origin source should parse");
    let typed = analyze(parsed).expect("print origin source should analyze");
    let mir = lower_test_program(&typed).expect("print origin source should lower");

    let sites = |label: &str| {
        mir.log_sites
            .iter()
            .filter(|site| site.label.as_deref() == Some(label))
            .collect::<Vec<_>>()
    };
    let report = sites("report");
    assert_eq!(
        report.len(),
        2,
        "generic helper should emit two concrete sites"
    );
    assert!(report.iter().all(|site| {
        site.lexical_owner == "program"
            && site.declaration.as_deref() == Some("report")
            && site.source.line == 3
    }));
    let method = sites("method");
    assert_eq!(method.len(), 1);
    assert!(method.iter().all(|site| {
        site.lexical_owner == "Logger"
            && site.declaration.as_deref() == Some("emit")
            && site.source.line == 8
    }));
    let child = sites("child");
    assert!(!child.is_empty());
    assert!(child.iter().all(|site| {
        site.lexical_owner == "Child"
            && site.declaration.as_deref() == Some("sample")
            && site.source.line == 12
    }));
    let parent = sites("parent");
    assert!(!parent.is_empty());
    assert!(parent.iter().all(|site| {
        site.lexical_owner == "Parent"
            && site.declaration.as_deref() == Some("sample")
            && site.source.line == 19
    }));
}

#[test]
fn non_generic_print_helper_is_lowered_with_runtime_context() {
    let source = r#"
def report(value: i32):
  print("value", value)

sample:
  report(1)
  out1 = 0.0
"#;
    let parsed = parse_program(source).expect("print helper source should parse");
    let typed = analyze(parsed).expect("print helper source should analyze");
    let report = typed
        .defs
        .iter()
        .find(|function| function.name == "report")
        .expect("typed report helper");
    assert!(!report.runtime_context);
    assert!(report.publishes_print);

    let mir = lower_test_program(&typed).expect("print helper source should lower");
    assert_eq!(mir.log_sites.len(), 1);
    assert_eq!(mir.log_sites[0].label.as_deref(), Some("value"));
    let report = mir
        .functions
        .iter()
        .find(|function| function.name == "report")
        .expect("MIR report helper");
    assert!(report.attributes.runtime_context);
    assert_eq!(report.attributes.origin, onda_mir::FunctionOrigin::Source);
    assert_eq!(report.attributes.inline, onda_mir::InlineHint::Auto);
}

#[test]
fn print_literals_use_ordinary_defaults_and_explicit_types_are_preserved() {
    let source = r#"
init:
  print("defaults", 3, 3.0)
  print("explicit", i64(3), f64(3.0), true)
sample:
  out1 = 0.0
"#;
    let parsed = parse_program(source).expect("print type source should parse");
    let typed = analyze(parsed).expect("print type source should analyze");
    let mir = lower_test_program(&typed).expect("print type source should lower");

    assert_eq!(
        mir.log_sites[0].argument_types,
        vec![onda_mir::ScalarType::I32, onda_mir::ScalarType::F32]
    );
    assert_eq!(mir.log_sites[0].payload_size, 8);
    assert_eq!(
        mir.log_sites[1].argument_types,
        vec![
            onda_mir::ScalarType::I64,
            onda_mir::ScalarType::F64,
            onda_mir::ScalarType::Bool,
        ]
    );
    assert_eq!(mir.log_sites[1].payload_size, 17);
}

#[test]
fn ranged_top_level_params_are_clamped_once_per_export_entry() {
    let source = r#"
params:
  value = 0.5 {0.0, 1.0}
  unused = 0.25 {0.0, 1.0}
outs:
  out1
init:
  cached = value + value
event bang():
  cached = value + value
sample:
  out1 = value + value + cached
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("ranged parameters should lower");
    let dump = format_program(&mir);

    for entry in ["onda_init", "onda_process", "onda_event::bang"] {
        let function = formatted_function(&dump, entry);
        assert_eq!(
            function.matches("load @param0").count(),
            1,
            "{entry} should load the used ranged parameter once:\n{function}"
        );
        assert_eq!(
            function.matches("intrinsic range_clamp(").count(),
            1,
            "{entry} should clamp the used ranged parameter once:\n{function}"
        );
        assert!(
            !function.contains("load @param1"),
            "{entry} should not clamp an unused parameter:\n{function}"
        );
    }
}

#[test]
fn range_clamps_respect_event_and_loop_variable_shadowing() {
    let source = r#"
params:
  i: i32 = 7 {0, 10}
  value: f32 = 0.75 {0.0, 1.0}
outs:
  out1
init:
  cached = 0.0
event set(value: f32):
  cached = value
sample:
  for i in 0..1:
    out1 = cached + f32(i)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("shadowed parameter names should analyze");
    let mir = lower_test_program(&typed).expect("shadowed parameter names should lower");
    let dump = format_program(&mir);

    let process = formatted_function(&dump, "onda_process");
    assert!(
        !process.contains("intrinsic range_clamp("),
        "a loop variable must not be rewritten as the same-named parameter:\n{process}"
    );
    assert!(
        !process.contains("load @param0"),
        "the shadowed top-level parameter must remain unused:\n{process}"
    );

    let event = formatted_function(&dump, "onda_event::set");
    assert!(
        event.contains("load @event_param0"),
        "the event body should read its event parameter:\n{event}"
    );
    assert!(
        !event.contains("intrinsic range_clamp(") && !event.contains("load @param1"),
        "an event parameter must not inherit the same-named top-level parameter range:\n{event}"
    );
}

#[test]
fn underscore_for_variables_and_loop_sugar_lower_normally() {
    let source = r#"
outs:
  out1
sample:
  total = 0
  for _ in 0..2:
    total += 1
  loop 2:
    total += 1
  out1 = f32(total)
"#;
    let typed = analyze(parse_program(source).expect("source should parse"))
        .expect("underscore loop variables should analyze");
    let mir = lower_test_program(&typed).expect("underscore loop variables should lower");
    validate(&mir).expect("underscore loop variables should produce valid MIR");
}

#[test]
fn def_local_ranges_do_not_become_same_named_state_invariants() {
    let source = r#"
ins 3
outs 1

init:
  selected: i32 = 99

def unrelated() -> i32:
  selected: i32 = 0 {3, wrap}
  return selected

sample:
  out1 = ins[selected] + f32(unrelated())
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("same-named bindings should analyze independently");
    let mir = lower_test_program(&typed).expect("same-named bindings should lower safely");
    validate(&mir).expect("same-named range bindings should produce valid MIR");

    let selected = mir
        .state
        .iter()
        .find(|state| state.name == "selected")
        .expect("selected state should exist");
    assert_eq!(selected.integer_range, None);

    let dump = format_program(&mir);
    let process = formatted_function(&dump, "onda_process");
    assert!(
        process.contains("intrinsic range_clamp("),
        "the unbounded state index must retain runtime normalization:\n{process}"
    );
}

#[test]
fn tuple_destructuring_preserves_integer_storage_ranges() {
    let source = r#"
sample:
  values: f32[4] = [10.0, 20.0, 30.0, 40.0]
  clamped: i32 = 0 {4}
  wrapped: i32 = 0 {4, wrap}
  (clamped, wrapped) = (100, -1)
  out1 = values[clamped] + values[wrapped]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("ranged destructuring should analyze");
    let mir = lower_test_program(&typed).expect("ranged destructuring should lower");
    validate(&mir).expect("ranged destructuring MIR should validate");

    let dump = format_program(&mir);
    let process = formatted_function(&dump, "onda_process");
    assert!(
        !process.contains("[i32(100)] unchecked") && !process.contains("[i32(-1)] unchecked"),
        "raw destructured values must not reach proven-unchecked indexing:\n{process}"
    );
    assert!(
        process.contains("intrinsic range_clamp(i32(100), i32(0), i32(3))")
            && process.contains("intrinsic range_wrap(i32(-1), i32(0), i32(3))"),
        "destructuring should enforce both destination storage invariants:\n{process}"
    );
}

#[test]
fn dynamic_input_and_param_reads_use_entry_point_range_clamps() {
    let source = r#"
ins:
  low: f32 = 0.0 {-1.0, 1.0}
  high: f32 = 0.0 {-2.0, 2.0}
kins:
  gain: f32 = 0.5 {0.0, 1.0}
  mix: f32 = 0.5 {0.0, 1.0}
outs:
  out1
init:
  selected: i32 = 1
sample:
  out1 = ins[0] + ins[selected] + params[0] + kins[selected]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("ranged dynamic reads should lower");
    validate(&mir).expect("ranged dynamic-read MIR should validate");

    let dump = format_program(&mir);
    let process = formatted_function(&dump, "onda_process");
    assert_eq!(
        process
            .lines()
            .filter(|line| line.contains("intrinsic range_clamp(") && line.contains("f32("))
            .count(),
        4,
        "each ranged dynamic endpoint should be clamped once:\n{process}"
    );
    assert_eq!(
        process
            .lines()
            .filter(|line| line.contains("intrinsic range_clamp(") && line.contains("i32("))
            .count(),
        2,
        "each unproven dynamic interface selector should be clamped once:\n{process}"
    );
    for raw_endpoint in [
        "load_input @in0",
        "load_input @in1",
        "load @param0",
        "load @param1",
    ] {
        assert_eq!(
            process.matches(raw_endpoint).count(),
            1,
            "the range hoist should be the only raw read of {raw_endpoint}:\n{process}"
        );
    }
    for alias in ["__onda_clamped_in__low", "__onda_clamped_in__high"] {
        assert!(
            process.contains(alias),
            "dynamic dispatch should read clamp alias '{alias}':\n{process}"
        );
    }
    for alias in ["__onda_clamped_param__gain", "__onda_clamped_param__mix"] {
        let state = mir
            .state
            .iter()
            .position(|slot| slot.name == alias)
            .unwrap_or_else(|| panic!("missing clamp alias state '{alias}'"));
        assert!(
            process.contains(&format!("load @state{state}")),
            "dynamic dispatch should read clamp alias '{alias}':\n{process}"
        );
    }
}

#[test]
fn ranged_top_level_param_clamps_preserve_scalar_types() {
    let source = r#"
params:
  f32_value: f32 = 0.5 {0.0, 1.0}
  f64_value: f64 = 0.5 {0.0, 1.0}
  i32_value: i32 = 5 {0, 10}
  i64_value: i64 = 5 {0, 10}
outs:
  out1
sample:
  out1 = f32_value + f32(f64_value) + f32(i32_value) + f32(i64_value)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("ranged scalar parameters should lower");
    validate(&mir).expect("typed range clamps should produce valid MIR");

    for (name, expected) in [
        ("f32_value", onda_mir::ScalarType::F32),
        ("f64_value", onda_mir::ScalarType::F64),
        ("i32_value", onda_mir::ScalarType::I32),
        ("i64_value", onda_mir::ScalarType::I64),
    ] {
        let alias = format!("__onda_clamped_param__{name}");
        let slot = mir
            .state
            .iter()
            .find(|slot| slot.name == alias)
            .unwrap_or_else(|| panic!("missing clamp alias state '{alias}'"));
        assert_eq!(
            mir.types[slot.ty.index()],
            onda_mir::Type::Scalar(expected),
            "clamp alias for '{name}' must preserve its declared scalar type"
        );
    }
}

#[test]
fn ranged_proc_params_are_clamped_once_when_assigned() {
    let source = r#"
proc Gain:
  params:
    amount = 0.5 {0.0, 1.0}
  outs:
    out1
  sample:
    out1 = amount + amount

params:
  drive = 0.5
outs:
  out1
init:
  gain = Gain()
sample:
  gain.amount = drive
  out1 = gain()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("ranged proc parameters should lower");
    let dump = format_program(&mir);

    let process = formatted_function(&dump, "onda_process");
    assert_eq!(
        process.matches("intrinsic range_clamp(").count(),
        1,
        "the proc-param assignment should clamp once:\n{process}"
    );

    let step = formatted_function(&dump, "Gain.__onda_proc_step");
    assert!(
        !step.contains("intrinsic range_clamp("),
        "reads of the already-constrained proc parameter should not reclamp:\n{step}"
    );
}

fn empty_mir() -> Program {
    let mut mir = Program::new(
        CompileConfig {
            sample_rate: 48_000.0,
            block_size: 64,
        },
        FunctionId::new(0),
        FunctionId::new(1),
    );
    mir.functions
        .push(empty_function("init", FunctionKind::Init));
    mir.functions
        .push(empty_function("process", FunctionKind::Process));
    mir.types.push(MirType::Scalar(ScalarType::I32));
    mir.functions[1].params = onda_mir::process_function_params(TypeId::new(0));
    mir
}

fn normalized_source_paths(paths: &[String]) -> Vec<String> {
    let mut mir = empty_mir();
    mir.source_files = paths
        .iter()
        .cloned()
        .map(|path| SourceFile { path })
        .collect();
    normalize_mir_source_paths(&mut mir);
    mir.source_files
        .into_iter()
        .map(|source| source.path)
        .collect()
}

fn block_loop_count(block: &MirBlock) -> usize {
    block
        .statements
        .iter()
        .map(|statement| match &statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => block_loop_count(then_block) + block_loop_count(else_block),
            StatementKind::Loop { body } => 1 + block_loop_count(body),
            _ => 0,
        })
        .sum()
}

fn block_contains_slice_fill(block: &MirBlock) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => block_contains_slice_fill(then_block) || block_contains_slice_fill(else_block),
            StatementKind::Loop { body } => block_contains_slice_fill(body),
            StatementKind::SliceFill { .. } => true,
            _ => false,
        })
}

fn block_contains_all_bits_zero_state_store(block: &MirBlock) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::State(_),
                        ..
                    },
                value: Rvalue::Use(value),
            } => scalar_value_is_all_bits_zero(*value),
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                block_contains_all_bits_zero_state_store(then_block)
                    || block_contains_all_bits_zero_state_store(else_block)
            }
            StatementKind::Loop { body } => block_contains_all_bits_zero_state_store(body),
            _ => false,
        })
}

fn block_writes_state(block: &MirBlock, state: onda_mir::StateId) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::State(destination),
                        ..
                    },
                ..
            } => *destination == state,
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => block_writes_state(then_block, state) || block_writes_state(else_block, state),
            StatementKind::Loop { body } => block_writes_state(body, state),
            _ => false,
        })
}

#[test]
fn source_paths_are_reproducible_across_checkout_roots() {
    let first = normalized_source_paths(&[
        "/home/alice/work/project/main.onda".to_owned(),
        "/home/alice/work/project/lib/filter.onda".to_owned(),
        "/opt/onda/share/stdlib/std/math.onda".to_owned(),
    ]);
    let second = normalized_source_paths(&[
        "D:\\ci\\build\\project\\main.onda".to_owned(),
        "D:\\ci\\build\\project\\lib\\filter.onda".to_owned(),
        "C:\\tools\\onda\\stdlib\\std\\math.onda".to_owned(),
    ]);
    assert_eq!(
        first,
        vec!["main.onda", "lib/filter.onda", "stdlib/std/math.onda"]
    );
    assert_eq!(second, first);
}

#[test]
fn source_path_normalization_coalesces_files_and_remaps_nested_spans() {
    let mut mir = empty_mir();
    mir.source_files = vec![
        SourceFile {
            path: "/tmp/project/./src/main.onda".to_owned(),
        },
        SourceFile {
            path: "/tmp/project/src/main.onda".to_owned(),
        },
    ];
    mir.functions[0].source.file = Some(SourceFileId::new(1));
    mir.functions[0].body.statements.push(Statement {
        kind: StatementKind::If {
            condition: Value::Constant(ScalarValue::Bool(true)),
            then_block: MirBlock {
                statements: vec![Statement {
                    kind: StatementKind::Return { values: Vec::new() },
                    source: SourceSpan {
                        file: Some(SourceFileId::new(1)),
                        ..SourceSpan::UNKNOWN
                    },
                }],
            },
            else_block: MirBlock::default(),
        },
        source: SourceSpan {
            file: Some(SourceFileId::new(0)),
            ..SourceSpan::UNKNOWN
        },
    });

    normalize_mir_source_paths(&mut mir);

    assert_eq!(mir.source_files.len(), 1);
    assert_eq!(mir.source_files[0].path, "main.onda");
    assert_eq!(mir.functions[0].source.file, Some(SourceFileId::new(0)));
    let outer = &mir.functions[0].body.statements[0];
    assert_eq!(outer.source.file, Some(SourceFileId::new(0)));
    let StatementKind::If { then_block, .. } = &outer.kind else {
        panic!("expected nested conditional");
    };
    assert_eq!(
        then_block.statements[0].source.file,
        Some(SourceFileId::new(0))
    );
}

#[test]
fn source_path_normalization_disambiguates_distinct_matching_suffixes() {
    let mut mir = empty_mir();
    mir.source_files = vec![
        SourceFile {
            path: "/alpha/project/src/shared.onda".to_owned(),
        },
        SourceFile {
            path: "/beta/vendor/src/shared.onda".to_owned(),
        },
    ];
    mir.functions[0].source.file = Some(SourceFileId::new(0));
    mir.functions[1].source.file = Some(SourceFileId::new(1));

    normalize_mir_source_paths(&mut mir);

    assert_eq!(
        mir.source_files
            .iter()
            .map(|source| source.path.as_str())
            .collect::<Vec<_>>(),
        ["external/src/shared.onda", "external/src/shared.onda~2"]
    );
    assert_eq!(mir.functions[0].source.file, Some(SourceFileId::new(0)));
    assert_eq!(mir.functions[1].source.file, Some(SourceFileId::new(1)));
}

fn block_has_call_with_arity(block: &MirBlock, arity: usize) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::Call { args, .. } => args.len() == arity,
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                block_has_call_with_arity(then_block, arity)
                    || block_has_call_with_arity(else_block, arity)
            }
            StatementKind::Loop { body } => block_has_call_with_arity(body, arity),
            _ => false,
        })
}

fn collect_block_calls<'a>(block: &'a MirBlock, calls: &mut Vec<(FunctionId, &'a [CallArgument])>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Call { function, args, .. } => {
                calls.push((*function, args.as_slice()));
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_calls(then_block, calls);
                collect_block_calls(else_block, calls);
            }
            StatementKind::Loop { body } => collect_block_calls(body, calls),
            _ => {}
        }
    }
}

fn block_assigns_state(block: &MirBlock, state: onda_mir::StateId) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                destination.base == PlaceBase::State(state)
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => block_assigns_state(then_block, state) || block_assigns_state(else_block, state),
            StatementKind::Loop { body } => block_assigns_state(body, state),
            _ => false,
        })
}

#[test]
fn lowers_scalar_functions_from_analyzed_source() {
    let source = r#"
outs:
  out1

def scale(x: f32, amount: f32 = 2.0) -> f32:
  value = x * amount
  if value > 1.0:
    value = 1.0
  return value

def render(enabled: bool) -> f32:
  if enabled:
    return scale(amount = 0.5, x = 3.0)
  return scale(0.0)

sample:
  out1 = render(true)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mut mir = empty_mir();

    let ids = lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("scalar functions should lower");
    assert_eq!(ids.len(), 2);
    validate(&mir).expect("lowered MIR should validate");

    let dump = format_program(&mir);
    assert!(dump.contains("fn @fn2 \"scale\" user"));
    assert!(dump.contains("fn @fn3 \"render\" user"));
    assert!(dump.contains("call @fn2"));
    assert!(!dump.contains("intrinsic"));
}

#[test]
fn mixed_scalar_generic_call_uses_one_widened_mir_signature() {
    let source = r#"
outs:
  out1

def choose<T>(x: T, lo: T, hi: T) -> T:
  if x < lo:
    return lo
  if x > hi:
    return hi
  return x

sample:
  x: f32 = f32(16777216.0)
  lo: f64 = f64(16777217.0)
  hi: f64 = f64(16777218.0)
  out1 = f32(choose(x, lo, hi) - lo)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("mixed generic call should analyze");
    let mir = lower_test_program(&typed).expect("mixed generic call should lower");
    validate(&mir).expect("mixed generic MIR should validate");

    let choose = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("choose.__onda_mono__g_f64"))
        .expect("missing widened choose specialization");
    assert_eq!(choose.params.len(), 3);
    assert!(choose
        .params
        .iter()
        .all(|parameter| { mir.types[parameter.ty.index()] == MirType::Scalar(ScalarType::F64) }));
    assert_eq!(choose.results.len(), 1);
    assert_eq!(
        mir.types[choose.results[0].index()],
        MirType::Scalar(ScalarType::F64)
    );
}

#[test]
fn selected_width_literal_call_argument_is_folded_to_a_direct_constant() {
    let source = r#"
outs:
  out1

def take(x: f32) -> f32:
  return x

sample:
  out1 = take(0.25)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("literal call should analyze");
    let mir = lower_test_program(&typed).expect("literal call should lower");
    validate(&mir).expect("literal-call MIR should validate");

    let take_id = FunctionId::new(
        mir.functions
            .iter()
            .position(|function| function.name == "take")
            .expect("take should lower") as u32,
    );
    let process = &mir.functions[mir.entry_points.process.index()];
    let mut calls = Vec::new();
    collect_block_calls(&process.body, &mut calls);
    let (_, args) = calls
        .into_iter()
        .find(|(function, _)| *function == take_id)
        .expect("process should call take");
    assert_eq!(
        args,
        [CallArgument::Value(Value::Constant(ScalarValue::F32(0.25)))]
    );
}

#[test]
fn named_slice_arguments_prepare_bounds_in_source_order() {
    let source = r#"
outs:
  out1

def mark(values: f32[], value: i32) -> i32:
  values[0] = f32(value)
  return 0

def consume(a: f32[], b: f32[]) -> f32:
  return 0.0

init:
  values: f32[1] = [0.0]

sample:
  out1 = consume(
    b = values[mark(values, 2):],
    a = values[mark(values, 1):],
  ) + values[0]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("named slice call should analyze");
    let mir = lower_test_program(&typed).expect("named slice call should lower");
    validate(&mir).expect("named slice MIR should validate");

    let mark_id = FunctionId::new(
        mir.functions
            .iter()
            .position(|function| function.name == "mark")
            .expect("mark should lower") as u32,
    );
    let process = &mir.functions[mir.entry_points.process.index()];
    let mut calls = Vec::new();
    collect_block_calls(&process.body, &mut calls);
    let values = calls
        .into_iter()
        .filter(|(function, _)| *function == mark_id)
        .map(|(_, args)| match args.get(1) {
            Some(CallArgument::Value(Value::Constant(ScalarValue::I32(value)))) => *value,
            other => panic!("unexpected mark value argument: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(values, vec![2, 1]);
}

#[test]
fn named_tuple_arguments_prepare_component_calls_in_source_order() {
    let source = r#"
outs:
  out1

def mark_pair(values: f32[], value: i32) -> (f32, f32):
  values[0] = f32(value)
  return (0.0, 0.0)

def consume(a: (f32, f32), b: (f32, f32)) -> f32:
  return 0.0

init:
  values: f32[1] = [0.0]

sample:
  out1 = consume(
    b = mark_pair(values, 2),
    a = mark_pair(values, 1),
  ) + values[0]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("named tuple call should analyze");
    let mir = lower_test_program(&typed).expect("named tuple call should lower");
    validate(&mir).expect("named tuple MIR should validate");

    let mark_id = FunctionId::new(
        mir.functions
            .iter()
            .position(|function| function.name == "mark_pair")
            .expect("mark_pair should lower") as u32,
    );
    let process = &mir.functions[mir.entry_points.process.index()];
    let mut calls = Vec::new();
    collect_block_calls(&process.body, &mut calls);
    let values = calls
        .into_iter()
        .filter(|(function, _)| *function == mark_id)
        .map(|(_, args)| match args.get(1) {
            Some(CallArgument::Value(Value::Constant(ScalarValue::I32(value)))) => *value,
            other => panic!("unexpected mark_pair value argument: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(values, vec![2, 1]);
}

#[test]
fn named_indexed_struct_arguments_prepare_indices_in_source_order() {
    let source = r#"
struct Cell:
  value: f32 = 0.0

outs:
  out1

def mark(values: f32[], value: i32) -> i32:
  values[0] = f32(value)
  return 0

def consume(a: Cell, b: Cell) -> f32:
  return 0.0

init:
  cells: Cell[1]
  values: f32[1] = [0.0]

sample:
  out1 = consume(
    b = cells[mark(values, 2)],
    a = cells[mark(values, 1)],
  ) + values[0]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("named indexed-struct call should analyze");
    let mir = lower_test_program(&typed).expect("named indexed-struct call should lower");
    validate(&mir).expect("named indexed-struct MIR should validate");

    let mark_id = FunctionId::new(
        mir.functions
            .iter()
            .position(|function| function.name == "mark")
            .expect("mark should lower") as u32,
    );
    let process = &mir.functions[mir.entry_points.process.index()];
    let mut calls = Vec::new();
    collect_block_calls(&process.body, &mut calls);
    let values = calls
        .into_iter()
        .filter(|(function, _)| *function == mark_id)
        .map(|(_, args)| match args.get(1) {
            Some(CallArgument::Value(Value::Constant(ScalarValue::I32(value)))) => *value,
            other => panic!("unexpected mark value argument: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(values, vec![2, 1]);
}

#[test]
fn supplied_named_arguments_are_prepared_before_omitted_defaults() {
    let source = r#"
outs:
  out1

def one() -> f32:
  return 1.0

def two() -> f32:
  return 2.0

def combine(a: f32 = one(), b: f32 = 0.0) -> f32:
  return a + b

sample:
  out1 = combine(b = two())
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("defaulted named call should analyze");
    let mir = lower_test_program(&typed).expect("defaulted named call should lower");
    validate(&mir).expect("defaulted named-call MIR should validate");

    let function_id = |expected: &str| {
        FunctionId::new(
            mir.functions
                .iter()
                .position(|function| function.name == expected)
                .unwrap_or_else(|| panic!("missing function {expected}")) as u32,
        )
    };
    let one = function_id("one");
    let two = function_id("two");
    let combine = function_id("combine");
    let process = &mir.functions[mir.entry_points.process.index()];
    let mut calls = Vec::new();
    collect_block_calls(&process.body, &mut calls);
    let order = calls
        .into_iter()
        .map(|(function, _)| function)
        .filter(|function| *function == one || *function == two || *function == combine)
        .collect::<Vec<_>>();
    assert_eq!(order, vec![two, one, combine]);
}

#[test]
fn standalone_function_lowering_materializes_read_only_constant_data() {
    let source = r#"
const Table: f64[2] = [1.0, 2.0]

outs:
  out1

def lookup(index: i32):
  return Table[index]

sample:
  out1 = f32(lookup(1))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mut mir = empty_mir();

    lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("constant-data function should lower");
    validate(&mir).expect("constant-data function MIR should validate");

    assert_eq!(mir.const_data.len(), 1);
    assert_eq!(mir.const_data[0].name, "Table");
    assert_eq!(mir.const_data[0].element, ScalarType::F64);
    let dump = format_program(&mir);
    assert!(dump.contains("load_const_data @data0"));
}

#[test]
fn retains_branch_local_types_and_lowers_short_circuit_control() {
    let source = r#"
outs:
  out1

def choose(flag: bool, other: bool) -> i32:
  if flag:
    result = 1
  else:
    result = 2
  if flag && other:
    result = 3
  return result

sample:
  out1 = f32(choose(true, false))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let function = typed
        .defs
        .iter()
        .find(|function| function.name == "choose")
        .expect("choose should be reachable");
    assert_eq!(
        function.local_scalar_types.get("result"),
        Some(&PrimitiveType::I32)
    );

    let mut mir = empty_mir();
    lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("short-circuit function should lower");
    validate(&mir).expect("lowered MIR should validate");
    let dump = format_program(&mir);
    assert!(!dump.contains("logical_and"));
    assert!(dump.matches("  if ").count() >= 2);
}

#[test]
fn rejects_incompatible_branch_integer_ranges_if_semantic_metadata_is_corrupted() {
    let source = r#"
params:
  select: bool = true

sample:
  if select:
    chosen: i32 = 5 {0..10}
  else:
    chosen: i32 = 6 {0..10}
  out1 = f32(chosen)
"#;
    let mut typed = analyze(parse_program(source).expect("source should parse"))
        .expect("matching branch ranges should analyze");
    let Stmt::If { else_branch, .. } = &mut typed.sample[0] else {
        panic!("sample should start with the conditional");
    };
    let Stmt::Assign {
        expr: Expr::Call { args, .. },
        ..
    } = &mut else_branch[0]
    else {
        panic!("else branch should contain the ranged declaration");
    };
    args[2] = Expr::int(19);

    let errors = lower_test_program(&typed)
        .expect_err("MIR lowering must defend the branch storage-contract invariant");
    assert!(errors.iter().any(|error| error.message.contains(
        "binding 'chosen' reached MIR lowering with incompatible branch integer range contracts"
    )));
}

#[test]
fn scoped_branch_and_loop_bindings_do_not_leak_into_later_bindings() {
    let source = r#"
def scoped(flag: bool) -> f32:
  if flag:
    temp = 0.0
  for i in 0..1:
    temp = f32(i)
  temp = true
  if temp:
    return 1.0
  else:
    return 0.0

outs:
  out1

sample:
  out1 = scoped(in1 > 0.0)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_program_to_raw_mir(&typed).expect("scoped locals should lower");
    validate(&mir).expect("scoped-local MIR should validate");

    let scoped = mir
        .functions
        .iter()
        .find(|function| function.name == "scoped")
        .expect("scoped helper should lower");
    let temp_types = scoped
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("temp"))
        .filter_map(|local| match mir.types[local.ty.index()] {
            MirType::Scalar(ty) => Some(ty),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        temp_types,
        vec![ScalarType::F32, ScalarType::F32, ScalarType::Bool],
        "branch, loop, and outer bindings with the same spelling need distinct MIR locals"
    );
}

#[test]
fn comparison_and_multi_argument_intrinsic_literals_adopt_f32_peer_width() {
    let source = r#"
outs:
  out1

sample:
  x: f32 = f32(16777216.0)
  y = max(x, 16777217.0)
  if x == 16777217.0:
    if y > x:
      out1 = 0.0
    else:
      out1 = 1.0
  else:
    out1 = 0.0
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_program_to_raw_mir(&typed).expect("contextual literals should lower");
    validate(&mir).expect("contextual-literal MIR should validate");

    let process = &mir.functions[mir.entry_points.process.index()];
    let y = process
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("y"))
        .expect("max result should have a named local");
    assert_eq!(mir.types[y.ty.index()], MirType::Scalar(ScalarType::F32));

    let dump = format_program(&mir);
    assert!(
        dump.contains("intrinsic max(") && dump.contains("f32(16777216.0)"),
        "{dump}"
    );
    assert!(
            !dump.contains("f64(16777217.0)"),
            "the contextual literal must be rounded once to f32 before comparison/intrinsic use:\n{dump}"
        );
}

#[test]
fn lowers_directional_for_and_continue_with_increment_epilogue() {
    let source = r#"
outs:
  out1

def sum_to(n: i32) -> i32:
  total: i32 = 0
  for i in 0..n:
    if i == 2:
      continue
    total = total + i
  return total

sample:
  out1 = f32(sum_to(5))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mut mir = empty_mir();
    lower_scalar_user_functions_to_mir(&typed, &mut mir).expect("for loop should lower");
    validate(&mir).expect("lowered MIR should validate");

    let dump = format_program(&mir);
    assert!(dump.contains("loop"));
    assert!(dump.contains("continue"));
    assert!(dump.matches(" = add ").count() >= 2);
}

#[test]
fn lowers_tuple_returns_forwarding_locals_indexing_and_destructuring() {
    let source = r#"
outs:
  out1

def pair(x: f32) -> (f32, i32):
  return (x, 1)

def forward(x: f32) -> (f32, i32):
  return pair(x)

def combine(x: f32) -> f32:
  values = forward(x)
  (left, right) = values
  return left + f32(right) + values[0] + f32(values[1])

def index_pair(x: f32) -> (i32, f32):
  return (1, x)

def direct(x: f32) -> f32:
  (index, fraction) = index_pair(x)
  return f32(index) + fraction

sample:
  out1 = combine(0.5) + direct(0.25)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mut mir = empty_mir();

    lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("tuple-returning functions should lower");
    validate(&mir).expect("tuple-returning MIR should validate");

    let pair = mir
        .functions
        .iter()
        .find(|function| function.name == "pair")
        .expect("pair should be lowered");
    assert_eq!(pair.results.len(), 2);
    assert!(pair.body.statements.iter().any(|statement| matches!(
        statement.kind,
        StatementKind::Return { ref values } if values.len() == 2
    )));

    let forward = mir
        .functions
        .iter()
        .find(|function| function.name == "forward")
        .expect("forward should be lowered");
    assert!(forward.body.statements.iter().any(|statement| matches!(
        statement.kind,
        StatementKind::Call { ref results, .. } if results.len() == 2
    )));

    let combine = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("combine"))
        .expect("combine should be lowered");
    let right = combine
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("right"))
        .expect("destructured right component should be a named local");
    assert_eq!(
        mir.types[right.ty.index()],
        MirType::Scalar(ScalarType::I32)
    );

    let direct = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("direct"))
        .expect("direct tuple destructuring should lower");
    let index = direct
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("index"))
        .expect("directly destructured index should be a named local");
    let fraction = direct
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("fraction"))
        .expect("directly destructured fraction should be a named local");
    assert_eq!(
        mir.types[index.ty.index()],
        MirType::Scalar(ScalarType::I32)
    );
    assert_eq!(
        mir.types[fraction.ty.index()],
        MirType::Scalar(ScalarType::F32)
    );
}

#[test]
fn lowers_tuple_parameters_to_ordered_scalar_mir_parameters() {
    let source = r#"
outs:
  out1

def combine(values: (f32, i32), bias: f32) -> f32:
  return values[0] + f32(values[1]) + bias

sample:
  out1 = combine((0.5, 1), 2.0)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("tuple parameter should lower");
    validate(&mir).expect("tuple-parameter MIR should validate");

    let combine = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("combine"))
        .expect("combine should lower");
    assert_eq!(combine.params.len(), 3);
    assert_eq!(combine.params[0].name, "values.0");
    assert_eq!(combine.params[1].name, "values.1");
    assert_eq!(combine.params[2].name, "bias");

    let caller = mir
        .functions
        .iter()
        .find(|function| function.name == "onda_process")
        .expect("process should be present");
    assert!(block_has_call_with_arity(&caller.body, 3));
}

#[test]
fn scalar_value_parameter_reassignment_updates_its_mutable_entry_copy() {
    let source = r#"
outs:
  out1

def bump(value: i32) -> i32:
  replacement = value + 1
  return value

sample:
  out1 = f32(bump(1))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let mut typed = analyze(parsed).expect("source should analyze");
    let typed_bump = typed
        .defs
        .iter_mut()
        .find(|function| function.name == "bump")
        .expect("bump should be analyzed");
    let Stmt::Assign { target, .. } = &mut typed_bump.body[0] else {
        panic!("bump should begin with an assignment");
    };
    *target = AssignTarget::Var("value".to_owned());
    let mir = lower_test_program(&typed).expect("mutable scalar parameter should lower");
    validate(&mir).expect("mutable scalar-parameter MIR should validate");

    let bump = mir
        .functions
        .iter()
        .find(|function| function.name == "bump")
        .expect("bump should lower");
    let value = LocalId::new(
        bump.locals
            .iter()
            .position(|local| local.name.as_deref() == Some("value"))
            .expect("value should have a named mutable local") as u32,
    );
    assert!(matches!(
        &bump.body.statements[0].kind,
        StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(destination),
                projections,
            },
            value: Rvalue::Load(Place {
                base: PlaceBase::Parameter(parameter),
                projections: parameter_projections,
            }),
        } if *destination == value
            && projections.is_empty()
            && *parameter == ParameterId::new(0)
            && parameter_projections.is_empty()
    ));
    assert_eq!(
        bump.body
            .statements
            .iter()
            .filter(|statement| matches!(
                statement.kind,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Local(destination),
                        ..
                    },
                    ..
                } if destination == value
            ))
            .count(),
        2,
        "entry copy and source reassignment should target the same local"
    );
    let reassigned_from = bump
        .body
        .statements
        .iter()
        .rev()
        .find_map(|statement| match &statement.kind {
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::Local(destination),
                        projections,
                    },
                value: Rvalue::Use(source),
            } if *destination == value && projections.is_empty() => Some(*source),
            _ => None,
        })
        .expect("source reassignment should retain its computed value");
    assert!(matches!(
        &bump.body.statements.last().expect("return is present").kind,
        StatementKind::Return { values }
            if values.as_slice() == [Value::Local(value)]
                || values.as_slice() == [reassigned_from]
    ));
}

#[test]
fn whole_tuple_parameter_reassignment_updates_scalarized_entry_copies() {
    let source = r#"
outs:
  out1

def bump(values: (f32, i32)) -> f32:
  replacement = (values[0] + 1.0, values[1] + 1)
  return values[0] + f32(values[1])

sample:
  out1 = bump((0.5, 1))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let mut typed = analyze(parsed).expect("source should analyze");
    let typed_bump = typed
        .defs
        .iter_mut()
        .find(|function| function.name == "bump")
        .expect("bump should be analyzed");
    let Stmt::Assign { target, .. } = &mut typed_bump.body[0] else {
        panic!("bump should begin with an assignment");
    };
    *target = AssignTarget::Var("values".to_owned());
    let mir = lower_test_program(&typed).expect("mutable tuple parameter should lower");
    validate(&mir).expect("mutable tuple-parameter MIR should validate");

    let bump = mir
        .functions
        .iter()
        .find(|function| function.name == "bump")
        .expect("bump should lower");
    let components = ["values.0", "values.1"].map(|name| {
        LocalId::new(
            bump.locals
                .iter()
                .position(|local| local.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("{name} should have a named mutable local"))
                as u32,
        )
    });
    for (parameter_index, component) in components.into_iter().enumerate() {
        assert!(matches!(
            &bump.body.statements[parameter_index].kind,
            StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(destination),
                    projections,
                },
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(parameter),
                    projections: parameter_projections,
                }),
            } if *destination == component
                && projections.is_empty()
                && *parameter == ParameterId::new(parameter_index as u32)
                && parameter_projections.is_empty()
        ));
        assert_eq!(
            bump.body
                .statements
                .iter()
                .filter(|statement| matches!(
                    statement.kind,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::Local(destination),
                            ..
                        },
                        ..
                    } if destination == component
                ))
                .count(),
            2,
            "entry copy and tuple reassignment should target the same component local"
        );
    }
}

#[test]
fn lowers_no_result_function_calls_without_fake_f32_results() {
    let source = r#"
outs:
  out1

def consume(x: f32):
  doubled = x * 2.0

def wrapper():
  consume(1.0)

sample:
  wrapper()
  out1 = 0.0
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let consume = typed
        .defs
        .iter()
        .find(|function| function.name == "consume")
        .expect("consume should be reachable");
    assert!(!consume.returns_value);

    let mut mir = empty_mir();
    lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("no-result scalar function should lower");
    validate(&mir).expect("lowered MIR should validate");
    let lowered = mir
        .functions
        .iter()
        .find(|function| function.name == "consume")
        .expect("consume should be lowered");
    assert!(lowered.results.is_empty());
    let wrapper = mir
        .functions
        .iter()
        .find(|function| function.name == "wrapper")
        .expect("wrapper should be lowered");
    assert!(wrapper.body.statements.iter().any(|statement| matches!(
        statement.kind,
        StatementKind::Call { ref results, .. } if results.is_empty()
    )));
}

#[test]
fn resolves_runtime_sample_rate_at_the_function_oversample_factor() {
    let source = r#"
def current_rate() -> f32:
  return SR

outs:
  out1

sample 2:
  out1 = current_rate()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    assert_eq!(
        typed
            .def_sample_oversample_factors
            .get("current_rate")
            .copied(),
        None,
        "top-level defs inherit the caller context"
    );

    let mut mir = empty_mir();
    lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("oversampled scalar function should lower");
    let dump = format_program(&mir);
    assert!(dump.contains("f32(96000.0)"));
}

#[test]
fn proc_init_bind_hooks_inherit_the_instance_sample_context() {
    let source = r#"
proc Voice {
  params { freq = 48000.0 => update }
  init { cached = 0.0 }
  def update() { cached = freq / SR }
  outs { out1 }
  sample { out1 = cached }
}

outs { out1 }
init { voice = Voice() }
sample 2 { out1 = voice() }
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    assert_eq!(
        typed.proc_instance_oversample_factors.get("voice").copied(),
        Some(2)
    );

    let mir = lower_test_program(&typed).expect("oversampled proc should lower");
    validate(&mir).expect("lowered MIR should validate");
    assert!(mir
        .functions
        .iter()
        .any(|function| function.name.starts_with("Voice.__onda_proc_local__update")));
    let dump = format_program(&mir);
    assert!(dump.contains("f32(96000.0)"), "{dump}");
}

#[test]
fn complete_program_schedules_top_level_oversampling_in_portable_mir() {
    let source = r#"
ins:
  in1

outs:
  out1

sample 2:
  out1 = sin(in1)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("top-level oversampling should lower");
    validate(&mir).expect("top-level oversampling MIR should validate");

    assert_eq!(
        mir.state
            .iter()
            .filter(|state| state.name.starts_with("$oversample."))
            .count(),
        16,
        "one interpolation and one decimation stage each need eight persistent taps"
    );
    let process = &mir.functions[mir.entry_points.process.index()];
    assert_eq!(
        block_loop_count(&process.body),
        2,
        "2x interpolation stays static around one explicit sample oversampling loop"
    );
    assert!(
        process
            .locals
            .iter()
            .filter(|local| matches!(mir.types[local.ty.index()], MirType::Array { len: 2, .. }))
            .count()
            >= 2
    );
    let dump = format_program(&mir);
    assert!(dump.contains("$oversample.input.in1.stage0.a0"));
    assert!(dump.contains("$oversample.output.out1.stage0.a0"));
    assert_eq!(
        dump.matches("intrinsic sin(").count(),
        1,
        "the source sample body should appear once in MIR"
    );
    assert!(dump.contains("f32(0.039151598)"));
    assert!(dump.contains("store_output @out0"));
}

#[test]
fn sinc_stage_schedule_expands_only_one_or_two_iteration_kernels() {
    for (factor, expected_loops, has_strided_stages) in [(4, 2, false), (8, 4, true)] {
        let source = format!(
            r#"
ins {{ in1 }}
outs {{ out1 }}
sample {factor} {{
  out1 = sin(in1)
}}
"#
        );
        let parsed = parse_program(&source).expect("source should parse");
        let typed = analyze(parsed).expect("source should analyze");
        let mir = lower_test_program(&typed).expect("oversampling should lower");
        validate(&mir).expect("oversampling MIR should validate");

        let process = &mir.functions[mir.entry_points.process.index()];
        assert_eq!(
            block_loop_count(&process.body),
            expected_loops,
            "unexpected loop schedule for {factor}x oversampling"
        );
        let has_dynamic_frame = process.locals.iter().any(|local| {
            local.name.as_deref().is_some_and(|name| {
                name.starts_with("$oversample.interpolate.")
                    || name.starts_with("$oversample.decimate.")
            })
        });
        assert_eq!(
            has_dynamic_frame, has_strided_stages,
            "unexpected sinc stage policy for {factor}x oversampling"
        );
        let dump = format_program(&mir);
        assert_eq!(
            dump.matches("intrinsic sin(").count(),
            1,
            "the source sample body should appear once at {factor}x"
        );
    }
}

#[test]
fn complete_program_schedules_proc_oversampling_in_portable_mir() {
    let source = r#"
proc Tone:
  outs:
    out1

  params:
    gain = 0.25

  sample 2:
    out1 = sin(gain)

outs:
  out1

init:
  tone = Tone()

sample:
  out1 = tone()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("processor oversampling should lower");
    validate(&mir).expect("processor oversampling MIR should validate");

    let step = mir
        .functions
        .iter()
        .find(|function| function.name == "Tone.__onda_proc_step")
        .expect("processor step should lower");
    assert_eq!(
        block_loop_count(&step.body),
        1,
        "fixed processor oversampling should remain one explicit MIR loop"
    );
    assert!(step
        .locals
        .iter()
        .any(|local| matches!(mir.types[local.ty.index()], MirType::Array { len: 2, .. })));
    let dump = format_program(&mir);
    assert!(dump.contains("self.__onda_os_down_out__out1__stage0__a0"));
    assert_eq!(
        dump.matches("intrinsic sin(").count(),
        1,
        "the processor sample body should appear once in MIR"
    );
    assert!(dump.contains("f32(0.883005)"));
}

#[test]
fn specializes_a_function_for_each_runtime_compile_context() {
    let source = r#"
def current_rate() -> f32:
  return SR

def relay_rate() -> f32:
  return current_rate()

outs:
  out1

block:
  relay_rate()

sample 2:
  out1 = relay_rate()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mut mir = empty_mir();
    let ids = lower_scalar_user_functions_to_mir(&typed, &mut mir)
        .expect("contextual scalar functions should specialize");

    assert_eq!(ids.len(), 4);
    let dump = format_program(&mir);
    assert!(dump.contains("current_rate.__ctx_sr_473b8000_bs_00000040"));
    assert!(dump.contains("current_rate.__ctx_sr_47bb8000_bs_00000040"));
    assert!(dump.contains("relay_rate.__ctx_sr_473b8000_bs_00000040"));
    assert!(dump.contains("relay_rate.__ctx_sr_47bb8000_bs_00000040"));
    assert!(dump.contains("f32(48000.0)"));
    assert!(dump.contains("f32(96000.0)"));

    for relay in mir
        .functions
        .iter()
        .filter(|function| function.name.starts_with("relay_rate.__ctx"))
    {
        let call = relay
            .body
            .statements
            .iter()
            .find_map(|statement| match statement.kind {
                StatementKind::Call { function, .. } => Some(function),
                _ => None,
            })
            .expect("relay specialization should call current_rate");
        let callee = &mir.functions[call.index()];
        let context_suffix = relay
            .name
            .split_once(".__ctx")
            .map(|(_, suffix)| suffix)
            .expect("relay name should have a context suffix");
        assert!(callee.name.ends_with(context_suffix));
    }
}

#[test]
fn lowers_a_complete_scalar_program_with_canonical_process_loop() {
    let source = r#"
ins:
  in1 = 0.0 {-1.0, 1.0}

outs:
  out1: f32

kouts:
  before: f32
  after: f32

params:
  gain = 0.5 {0.0, 1.0}

def scale(x: f32, amount: f32) -> f32:
  return x * amount

init:
  phase: f32 = 0.0

block:
  before = gain
  sample:
    phase = phase + 1.0
    out1 = scale(in1, gain) + phase
  after = phase
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 44_100.0,
            block_size: 32,
        },
    )
    .expect("source should analyze");
    let mir = lower_test_program(&typed).expect("complete scalar program should lower");
    validate(&mir).expect("complete MIR should validate");

    assert_eq!(mir.config.sample_rate, 44_100.0);
    assert_eq!(mir.config.block_size, 32);
    assert_eq!(mir.interface.inputs.len(), 1);
    assert_eq!(mir.interface.outputs.len(), 1);
    assert_eq!(mir.interface.control_outputs.len(), 2);
    assert_eq!(mir.interface.params.len(), 1);
    let phase_state = mir
        .state
        .iter()
        .position(|state| state.name == "phase")
        .expect("phase should be persistent state");
    assert_eq!(mir.functions[0].kind, onda_mir::FunctionKind::Init);
    assert_eq!(mir.functions[1].kind, onda_mir::FunctionKind::Process);
    assert!(
        block_writes_state(
            &mir.functions[0].body,
            onda_mir::StateId::new(phase_state as u32),
        ),
        "canonical MIR must retain the zero store needed by live init"
    );

    let dump = format_program(&mir);
    assert!(dump.contains("\"phase\""));
    assert!(dump.contains("load @param0"));
    assert!(dump.contains("load_input @in0"));
    assert!(dump.contains("store_output @out0"));
    assert_eq!(dump.matches("store_control_output").count(), 2);
    assert!(dump.contains("loop"));
    assert!(dump.contains("load @p0"));
    assert!(dump.contains("load @p1"));
    assert!(dump.contains("load @p2"));
    assert!(dump.contains("call @fn2"));
}

#[test]
fn lowers_current_schema_segmented_process_parameters_and_logical_frames() {
    let source = r#"
ins:
  in1 = 0.0

outs:
  out1

init:
  total = 0.0

block:
  total = total + 10.0
  sample:
    total = total + in1
    out1 = total
  total = total + 100.0
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("segmented process source should analyze");
    let mir = lower_test_program(&typed).expect("segmented process should lower");
    validate(&mir).expect("segmented process MIR should validate");

    let process = &mir.functions[mir.entry_points.process.index()];
    assert_eq!(
        process
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        onda_mir::PROCESS_PARAM_NAMES
    );
    assert!(process
        .params
        .iter()
        .all(|param| param.mode == onda_mir::PassingMode::Value
            && matches!(
                mir.types[param.ty.index()],
                MirType::Scalar(ScalarType::I32)
            )));
    assert!(process.results.is_empty());

    let logical_frame = process
        .locals
        .iter()
        .position(|local| local.name.as_deref() == Some("$segment.logical_frame"))
        .expect("missing logical frame local");
    fn block_contains_process_frame(block: &MirBlock, frame: LocalId) -> bool {
        block
            .statements
            .iter()
            .any(|statement| match &statement.kind {
                StatementKind::Assign {
                    destination,
                    value: Rvalue::ProcessFrame { .. },
                } => *destination == Place::local(frame),
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    block_contains_process_frame(then_block, frame)
                        || block_contains_process_frame(else_block, frame)
                }
                StatementKind::Loop { body } => block_contains_process_frame(body, frame),
                _ => false,
            })
    }
    fn assert_io_uses_logical_frame(block: &MirBlock, logical_frame: usize) -> usize {
        let mut count = 0;
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Assign {
                    value: Rvalue::InputLoad { frame, .. },
                    ..
                }
                | StatementKind::Assign {
                    value: Rvalue::OutputLoad { frame, .. },
                    ..
                }
                | StatementKind::OutputStore { frame, .. } => {
                    assert_eq!(*frame, Value::Local(LocalId::new(logical_frame as u32)));
                    count += 1;
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    count += assert_io_uses_logical_frame(then_block, logical_frame);
                    count += assert_io_uses_logical_frame(else_block, logical_frame);
                }
                StatementKind::Loop { body } => {
                    count += assert_io_uses_logical_frame(body, logical_frame);
                }
                _ => {}
            }
        }
        count
    }
    assert!(assert_io_uses_logical_frame(&process.body, logical_frame) >= 2);
    assert!(block_contains_process_frame(
        &process.body,
        LocalId::new(logical_frame as u32)
    ));

    let pre_if = process
        .body
        .statements
        .iter()
        .position(|statement| matches!(statement.kind, StatementKind::If { .. }))
        .expect("missing BEGIN-gated block prelude");
    let process_loop = process
        .body
        .statements
        .iter()
        .position(|statement| matches!(statement.kind, StatementKind::Loop { .. }))
        .expect("missing segment loop");
    let post_if = process
        .body
        .statements
        .iter()
        .rposition(|statement| matches!(statement.kind, StatementKind::If { .. }))
        .expect("missing END-gated block postlude");
    assert!(pre_if < process_loop && process_loop < post_if);

    let dump = format_program(&mir);
    assert!(dump.contains("load @p0"));
    assert!(dump.contains("load @p1"));
    assert!(dump.contains("load @p2"));
    assert!(dump.contains("bit_and"));
    assert!(dump.contains("process_frame"));
    assert!(dump.contains("i32(1)"));
    assert!(dump.contains("i32(2)"));
    assert!(!dump.contains("i32(64)"));
}

#[test]
fn lowers_fixed_array_interface_surfaces() {
    let source = r#"
ins:
  source: f32[2] = [0.25, 0.5]

outs:
  stereo: f32[2]

kouts:
  meters: f32[2]

params:
  gains: f32[2] = [0.5, 0.25]

block:
  meters[0] = gains[0]
  meters[1] = gains[1]
  sample:
    stereo[0] = source[0] * gains[0]
    stereo[1] = source[1] * gains[1]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("fixed array surfaces should lower");
    validate(&mir).expect("interface array MIR should validate");

    assert_eq!(mir.interface.inputs.len(), 1);
    assert_eq!(mir.interface.outputs.len(), 1);
    assert_eq!(mir.interface.control_outputs.len(), 1);
    assert_eq!(mir.interface.params.len(), 1);
    assert_eq!(mir.interface.inputs[0].name, "source");
    assert_eq!(mir.interface.outputs[0].name, "stereo");
    assert_eq!(mir.interface.control_outputs[0].name, "meters");
    assert_eq!(mir.interface.params[0].name, "gains");
    assert!(matches!(
        mir.interface.inputs[0].default,
        Some(onda_mir::ConstantValue::Aggregate(ref values)) if values.len() == 2
    ));
    for ty in [
        mir.interface.inputs[0].ty,
        mir.interface.outputs[0].ty,
        mir.interface.control_outputs[0].ty,
        mir.interface.params[0].ty,
    ] {
        assert!(matches!(
            mir.types[ty.index()],
            MirType::Array { len: 2, .. }
        ));
    }

    let dump = format_program(&mir);
    assert!(dump.contains("load_input @in0[i32(0)] unchecked["));
    assert!(dump.contains("load_input @in0[i32(1)] unchecked["));
    assert!(dump.contains("store_output @out0[i32(0)]"));
    assert!(dump.contains("store_output @out0[i32(1)]"));
    assert!(dump.contains("store_control_output @kout0[i32(0)] unchecked"));
    assert!(dump.contains("store_control_output @kout0[i32(1)] unchecked"));
    assert!(dump.contains("load @param0[i32(0)] unchecked"));
    assert!(dump.contains("load @param0[i32(1)] unchecked"));
}

#[test]
fn control_output_stores_are_the_only_writes_to_mirror_state() {
    let source = r#"
kouts:
  meter: f32
  leds: f32[2]

block:
  meter = 0.5
  leds[0] = 0.25
  leds[1] = 0.75
  sample:
    out1 = 0.0
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("control-output source should analyze");
    let mir = lower_test_program(&typed).expect("control-output source should lower");
    validate(&mir).expect("control-output MIR should validate");

    let process = &mir.functions[mir.entry_points.process.index()];
    for output in &mir.interface.control_outputs {
        assert_eq!(
            mir.state[output.mirror.index()].persistence,
            onda_mir::StatePersistence::ControlMirror
        );
        assert!(
            !block_assigns_state(&process.body, output.mirror),
            "ControlOutputStore must be the sole write to '{}'",
            output.name
        );
    }
    let dump = format_program(&mir);
    assert_eq!(dump.matches("store_control_output").count(), 3);
}

#[test]
fn initializes_every_audio_output_slot_to_zero_per_base_sample() {
    let source = r#"
outs:
  main: f32
  pair: f32[2]

sample:
  pair[1] = 1.0
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("output initialization source should analyze");
    let mir = lower_test_program(&typed).expect("output initialization should lower");
    validate(&mir).expect("output initialization MIR should validate");

    let dump = format_program(&mir);
    let zero_initializers = dump
        .lines()
        .filter(|line| line.contains("= f32(0.0)"))
        .count();
    assert_eq!(zero_initializers, 2);
    assert_eq!(dump.matches("store_output ").count(), 3);
}

#[test]
fn lowers_resolved_dynamic_interface_views_once_to_concrete_endpoints() {
    let source = r#"
ins:
  dry: f32 = 0.0
  bands: f32[2] = [0.0, 0.0]

outs:
  main: f32
  pair: f32[2]

kouts:
  meter: f32
  leds: f32[2]

params:
  gain: f32 = 0.5
  controls: f32[2] = [0.25, 0.75]

def pick(value: i32) -> i32:
  return value

def value_once(value: f32) -> f32:
  return value

init:
  selected: i32 = 1

block:
  kouts[pick(selected)] = value_once(params[pick(selected)])
  sample:
    outs[pick(selected)] = value_once(ins[pick(selected)]) * params[pick(selected)]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("dynamic interface views should analyze");
    let mir = lower_test_program(&typed).expect("dynamic interface views should lower");
    validate(&mir).expect("dynamic interface MIR should validate");

    let pick = mir
        .functions
        .iter()
        .position(|function| function.name == "pick")
        .expect("missing pick function");
    let value_once = mir
        .functions
        .iter()
        .position(|function| function.name == "value_once")
        .expect("missing value_once function");
    let dump = format_program(&mir);
    assert_eq!(dump.matches(&format!("call @fn{pick}")).count(), 5);
    assert_eq!(dump.matches(&format!("call @fn{value_once}")).count(), 2);
    assert_eq!(dump.matches("intrinsic range_clamp(").count(), 5);
    assert!(dump
        .lines()
        .filter(|line| line.contains("intrinsic range_clamp("))
        .all(|line| line.contains("i32(0)") && line.contains("i32(2)")));

    for endpoint in [
        "load_input @in0",
        "load_input @in1[i32(0)] unchecked",
        "load_input @in1[i32(1)] unchecked",
        "load @param0",
        "load @param1[i32(0)] unchecked",
        "load @param1[i32(1)] unchecked",
        "store_control_output @kout0",
        "store_control_output @kout1[i32(0)] unchecked",
        "store_control_output @kout1[i32(1)] unchecked",
    ] {
        assert!(
            dump.contains(endpoint),
            "missing dynamic endpoint: {endpoint}"
        );
    }
    assert_eq!(dump.matches("store_output @out0").count(), 1);
    assert_eq!(
        dump.matches("store_output @out1[i32(0)] unchecked").count(),
        1
    );
    assert_eq!(
        dump.matches("store_output @out1[i32(1)] unchecked").count(),
        1
    );
    let meter_state = mir
        .state
        .iter()
        .position(|state| state.name == "meter")
        .expect("missing scalar control-output mirror");
    let leds_state = mir
        .state
        .iter()
        .position(|state| state.name == "leds")
        .expect("missing array control-output mirror");
    assert_eq!(mir.interface.control_outputs[0].mirror.index(), meter_state);
    assert_eq!(mir.interface.control_outputs[1].mirror.index(), leds_state);
    assert_eq!(
        mir.state[meter_state].persistence,
        onda_mir::StatePersistence::ControlMirror
    );
    assert_eq!(
        mir.state[leds_state].persistence,
        onda_mir::StatePersistence::ControlMirror
    );
    assert!(
        !dump.contains(&format!("@state{meter_state} ="))
            && !dump.contains(&format!("@state{leds_state}[i32(0)] unchecked ="))
            && !dump.contains(&format!("@state{leds_state}[i32(1)] unchecked =")),
        "ControlOutputStore is the sole mirror write"
    );
}

#[test]
fn kins_and_params_dynamic_aliases_use_the_same_resolved_param_endpoints() {
    let source = r#"
kins:
  gain: f32 = 0.5
  controls: f32[2] = [0.25, 0.75]

outs:
  out1

init:
  selected: i32 = 1

sample:
  out1 = kins[selected] + params[selected]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("kins alias source should analyze");
    let mir = lower_test_program(&typed).expect("kins alias source should lower");
    validate(&mir).expect("kins alias MIR should validate");

    let dump = format_program(&mir);
    assert_eq!(dump.matches("load @param0").count(), 2);
    assert_eq!(dump.matches("load @param1[i32(0)] unchecked").count(), 2);
    assert_eq!(dump.matches("load @param1[i32(1)] unchecked").count(), 2);
    assert_eq!(dump.matches("intrinsic range_clamp(").count(), 2);
}

#[test]
fn unsafe_dynamic_interface_access_supports_free_and_receiver_syntax() {
    let source = r#"
ins 2
kins 2
outs 2
kouts 2

init:
  selected: i32 = 1

block:
  kouts.write_unsafe(selected, params.read_unsafe(selected))
  sample:
    write_unsafe(outs, selected, read_unsafe(ins, selected) + kins.read_unsafe(selected))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("unsafe dynamic interface access should analyze");
    let mir = lower_test_program(&typed).expect("unsafe dynamic interface access should lower");
    validate(&mir).expect("unsafe dynamic interface MIR should validate");

    let dump = format_program(&mir);
    let process = formatted_function(&dump, "onda_process");
    assert!(!process.contains("intrinsic range_clamp("), "{process}");
    for endpoint in [
        "load_input @in0",
        "load_input @in1",
        "load @param0",
        "load @param1",
        "store_output @out0",
        "store_output @out1",
        "store_control_output @kout0",
        "store_control_output @kout1",
    ] {
        assert!(process.contains(endpoint), "missing {endpoint}:\n{process}");
    }
}

#[test]
fn unsafe_named_interface_arrays_preserve_host_output_stores() {
    let source = r#"
ins:
  source: f32[2]
outs:
  sink: f32[2]
kouts:
  leds: f32[2]
params:
  controls: f32[2] = [0.25, 0.75]

init:
  selected: i32 = 1

block:
  leds.write_unsafe(selected, controls.read_unsafe(selected))
  sample:
    write_unsafe(sink, selected, source.read_unsafe(selected))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("unsafe interface array access should analyze");
    let mir = lower_test_program(&typed).expect("unsafe interface array access should lower");
    validate(&mir).expect("unsafe interface array MIR should validate");

    let process = formatted_function(&format_program(&mir), "onda_process").to_owned();
    assert!(process.contains("load_input @in0["), "{process}");
    assert!(process.contains("load @param0["), "{process}");
    assert!(process.contains("store_output @out0["), "{process}");
    assert!(
        process.contains("store_control_output @kout0["),
        "{process}"
    );
    assert!(!process.contains("] clamp"), "{process}");
}

#[test]
fn ranged_indices_remove_dynamic_interface_normalization() {
    let source = r#"
ins 3
params 3
outs 3
kouts 3

init:
  selected: i32 = 0 {3, wrap}

block:
  kouts[selected] = params[selected]
  sample:
    outs[selected] = ins[selected]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("ranged dynamic interfaces should analyze");
    let mir = lower_test_program(&typed).expect("ranged dynamic interfaces should lower");
    validate(&mir).expect("ranged dynamic-interface MIR should validate");

    let dump = format_program(&mir);
    let process = formatted_function(&dump, "onda_process");
    assert!(
        !process.contains("intrinsic range_clamp("),
        "an in-range selector should not be normalized again:\n{process}"
    );
    for endpoint in [
        "load_input @in0",
        "load_input @in1",
        "load_input @in2",
        "load @param0",
        "load @param1",
        "load @param2",
        "store_output @out0",
        "store_output @out1",
        "store_output @out2",
        "store_control_output @kout0",
        "store_control_output @kout1",
        "store_control_output @kout2",
    ] {
        assert!(
            process.contains(endpoint),
            "missing dynamic endpoint {endpoint}:\n{process}"
        );
    }
}

#[test]
fn ranged_indices_remove_fixed_proc_array_normalization() {
    let source = r#"
proc Voice:
  outs 1
  sample:
    out1 = 1.0

init:
  voices: Voice[4] = Voice()
  selected: i32 = 0 {4, wrap}

sample:
  out1 = voices[selected]()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("ranged proc-array indexing should analyze");
    let mir = lower_test_program(&typed).expect("ranged proc-array indexing should lower");
    validate(&mir).expect("ranged proc-array MIR should validate");

    let dump = format_program(&mir);
    let process = formatted_function(&dump, "onda_process");
    assert!(
        !process.contains("intrinsic range_clamp("),
        "an in-range proc selector should not be normalized again:\n{process}"
    );
}

#[test]
fn dynamic_interfaces_use_top_level_oversampling_caches() {
    let source = r#"
ins 2
outs 2

init:
  selected: i32 = 1

sample 2:
  outs[selected] = ins[selected]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("oversampled dynamic ports should analyze");
    let mir = lower_test_program(&typed).expect("oversampled dynamic ports should lower");
    validate(&mir).expect("oversampled dynamic port MIR should validate");

    let dump = format_program(&mir);
    assert_eq!(dump.matches("intrinsic range_clamp(").count(), 2);
    assert_eq!(dump.matches("load_input ").count(), 2);
    assert_eq!(dump.matches("store_output ").count(), 2);
    assert!(dump.contains("$oversample.input.in1.current"));
    assert!(dump.contains("$oversample.input.in2.current"));
    assert!(dump.contains("$oversample.output.out1.current"));
    assert!(dump.contains("$oversample.output.out2.current"));
}

#[test]
fn fixed_array_interfaces_use_top_level_oversampling_caches() {
    let source = r#"
ins:
  pair: f32[2] = [0.0, 0.0]

outs:
  pair_out: f32[2]

init:
  selected: i32 = 1

sample 2:
  pair_out[selected] = pair[selected]
  pair_out[0] = pair[0]
  outs[selected] = ins[selected]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("oversampled array ports should analyze");
    let mir = lower_test_program(&typed).expect("oversampled array ports should lower");
    validate(&mir).expect("oversampled array port MIR should validate");

    let dump = format_program(&mir);
    assert_eq!(dump.matches("load_input ").count(), 2);
    assert_eq!(dump.matches("store_output ").count(), 2);
    assert!(dump.contains("$oversample.input.pair.current"));
    assert!(dump.contains("$oversample.output.pair_out.current"));
    assert!(dump.contains("$oversample.input.pair[0].stage0.a0"));
    assert!(dump.contains("$oversample.output.pair_out[1].stage0.a0"));
}

#[test]
fn lowers_semantically_specialized_untyped_scalar_calls() {
    let source = r#"
def identity(value):
  return value

outs:
  out1: f64

sample:
  out1 = identity(f64(1))
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed)
        .expect("semantic monomorphization should make the signature concrete");
    let dump = format_program(&mir);
    assert!(dump.contains("identity.__onda_mono__scalar_f64"));
    let identity = mir
        .functions
        .iter()
        .find(|function| function.name == "identity.__onda_mono__scalar_f64")
        .expect("missing concrete f64 identity function");
    assert!(matches!(
        mir.types[identity.params[0].ty.index()],
        MirType::Scalar(ScalarType::F64)
    ));
    assert!(matches!(
        mir.types[identity.results[0].index()],
        MirType::Scalar(ScalarType::F64)
    ));
}

#[test]
fn lowers_scalar_event_interface_and_handler() {
    let source = r#"
init:
  phase = 0.0

event set_phase(step: i32, amount: f32 = 0.5):
  scaled = f32(step) * amount
  phase = scaled

sample:
  out1 = phase
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("scalar event should lower");
    validate(&mir).expect("event MIR should validate");

    assert_eq!(mir.interface.events.len(), 1);
    let event = &mir.interface.events[0];
    assert_eq!(event.name, "set_phase");
    assert_eq!(event.params.len(), 2);
    assert_eq!(
        event.params[1].default,
        Some(onda_mir::ConstantValue::Scalar(ScalarValue::F32(0.5)))
    );
    let handler = &mir.functions[event.handler.index()];
    assert_eq!(
        handler.kind,
        onda_mir::FunctionKind::Event(onda_mir::EventId::new(0))
    );
    let dump = format_program(&mir);
    assert!(dump.contains("event @event0 \"set_phase\""));
    assert!(dump.contains("load @event_param0"));
    assert!(dump.contains("load @event_param1"));
}

#[test]
fn lowers_fixed_array_event_parameters() {
    let source = r#"
init:
  phase = 0.0

event set_curve(values: f32[2] = [0.25, 0.75]):
  phase = values[0] + values[1] + f32(values.len())

sample:
  out1 = phase
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("fixed event array should lower");
    validate(&mir).expect("event-array MIR should validate");

    let event = &mir.interface.events[0];
    assert!(matches!(
        mir.types[event.params[0].ty.index()],
        MirType::Array { len: 2, .. }
    ));
    assert!(matches!(
        event.params[0].default,
        Some(onda_mir::ConstantValue::Aggregate(ref values)) if values.len() == 2
    ));
    let dump = format_program(&mir);
    assert!(dump.contains("load @event_param0[i32(0)] unchecked"));
    assert!(dump.contains("load @event_param0[i32(1)] unchecked"));
    assert!(dump.contains("i32(2)"));
}

#[test]
fn lowers_primitive_slices_and_array_parameters() {
    let source = r#"
def total(values: f32[]):
  sum = 0.0
  for i in 0..(values.len()):
    sum = sum + values[i]
  return sum

def bump(values: f32[]):
  values[0] = values[0] + 1.0

init:
  values: f32[4] = [1.0, 2.0, 3.0, 4.0]

sample:
  middle = values[1:-1]
  bump(middle)
  values[-1:] = middle
  values[0:1] = 0.5
  out1 = total(values[:])
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("primitive slices should lower");
    validate(&mir).expect("slice MIR should validate");

    let total = mir
        .functions
        .iter()
        .find(|function| function.name == "total")
        .expect("missing total function");
    let bump = mir
        .functions
        .iter()
        .find(|function| function.name == "bump")
        .expect("missing bump function");
    assert!(matches!(
        mir.types[total.params[0].ty.index()],
        MirType::Slice {
            access: onda_mir::AccessMode::ReadOnly,
            ..
        }
    ));
    assert!(matches!(
        mir.types[bump.params[0].ty.index()],
        MirType::Slice {
            access: onda_mir::AccessMode::ReadWrite,
            ..
        }
    ));
    let dump = format_program(&mir);
    assert!(dump.contains("make_slice @state0"));
    assert!(dump.contains("slice_len"));
    assert!(dump.contains("load_slice"));
    assert!(dump.contains("store_slice"));
    assert!(dump.contains("slice_fill"));
    assert!(dump.contains("slice_copy"));
}

#[test]
fn lowers_fixed_primitive_local_arrays_as_logical_mir_storage() {
    let source = r#"
def local_total(seed: f32):
  inferred = [seed, 2.0]
  scratch: f32[2]
  inferred[1] = inferred[0] + 1.0
  scratch[0] = inferred[1]
  scratch[:] = inferred[:]
  return scratch[0] + scratch[1] + f32(scratch.len())

outs:
  out1

sample:
  out1 = local_total(3.0)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("local arrays should lower");
    validate(&mir).expect("local-array MIR should validate");

    let function = mir
        .functions
        .iter()
        .find(|function| function.name == "local_total")
        .expect("missing local_total function");
    let arrays = function
        .locals
        .iter()
        .filter(|local| matches!(mir.types[local.ty.index()], MirType::Array { len: 2, .. }))
        .count();
    assert_eq!(arrays, 2);

    let dump = format_program(&mir);
    assert!(dump.contains("\"inferred\": @"));
    assert!(dump.contains("\"scratch\": @"));
    assert!(dump.contains("load %"));
    assert!(dump.contains("make_slice %"));
    assert!(dump.contains("slice_copy"));
}

#[test]
fn prezeroed_state_elision_does_not_remove_local_array_initialization() {
    let source = r#"
def first() -> f32:
  scratch: f32[4]
  return scratch[0]

outs:
  out1

sample:
  out1 = first()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_program_to_raw_mir(&typed).expect("local array should lower");
    validate(&mir).expect("local-array MIR should validate");

    let function = mir
        .functions
        .iter()
        .find(|function| function.name == "first")
        .expect("missing first function");
    let scratch = function
        .locals
        .iter()
        .position(|local| local.name.as_deref() == Some("scratch"))
        .map(|index| LocalId::new(index as u32))
        .expect("missing scratch local");
    let initialized_elements = function
        .body
        .statements
        .iter()
        .filter(|statement| {
            matches!(
                statement.kind,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Local(local),
                        ref projections,
                    },
                    value: Rvalue::Use(value),
                } if local == scratch
                    && projections.len() == 1
                    && scalar_value_is_all_bits_zero(value)
            )
        })
        .count();
    assert_eq!(initialized_elements, 4);
}

#[test]
fn lowers_buffer_parameters_and_forwarding_as_logical_references() {
    let source = r#"
outs:
  out1

buffers:
  table: f32

def touch(buf: buffer<f32>, index: i32):
  view = buf[:]
  value = buf[index] + view[index] - view[index]
  buf[index] = value + 1.0
  if buf.bound():
    value = value + 1.0
  return value + f32(buf.len()) + f32(buf.chans()) + buf.samplerate()

def forward(buf: buffer<f32>, index: i32):
  return touch(buf, index)

sample:
  out1 = forward(table, 0)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("buffer parameters should lower");
    validate(&mir).expect("buffer-parameter MIR should validate");

    let touch = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("touch"))
        .expect("touch should lower");
    assert_eq!(
        touch.params[0].mode,
        onda_mir::PassingMode::ReadWriteReference
    );
    assert!(matches!(
        mir.types[touch.params[0].ty.index()],
        MirType::Buffer {
            element: ScalarType::F32,
            channels: onda_mir::BufferChannels::Mono,
            access: onda_mir::AccessMode::ReadWrite,
        }
    ));

    let dump = format_program(&mir);
    assert!(dump.contains("load_buffer_param @param0"));
    assert!(dump.contains("store_buffer_param @param0"));
    assert!(dump.contains("buffer_len @param0"));
    assert!(dump.contains("buffer_channels @param0"));
    assert!(dump.contains("buffer_sample_rate @param0"));
    assert!(dump.contains("buffer_is_bound @param0"));
    assert!(dump.contains("make_slice @param0"));
    assert!(dump.contains("(place @p0,"));
    assert!(dump.contains("(@buffer0,"));
}

#[test]
fn dynamic_buffer_parameters_accept_any_declared_channel_count() {
    let source = r#"
buffers:
  mono: f32
  stereo: f32[2]

outs:
  out1

def first(buf: buffer<f32[]>):
  return buf[0, 0]

sample:
  out1 = first(mono) + first(stereo)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("dynamic buffer parameter should accept exact buffers");
    let mir = lower_test_program(&typed).expect("dynamic buffer calls should lower");
    validate(&mir).expect("dynamic buffer call MIR should validate");
}

#[test]
fn exact_buffer_parameters_reject_dynamic_channel_counts() {
    let source = r#"
buffers:
  any: f32[]

def first(buf: buffer<f32[2]>):
  return buf[0, 0]

sample:
  out1 = first(any)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let errors = analyze(parsed).expect_err("exact buffer parameter must reject dynamic buffers");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expects static 2 channels")),
        "unexpected diagnostics: {errors:?}"
    );
}

#[test]
fn lowers_buffer_collection_slices_after_selecting_a_buffer() {
    let source = r#"
buffers:
  bank: f32 {2}
  layers: f32[2] {3}

def select() -> i32:
  return 1

def right_channel() -> i32:
  return 1

def first_frame() -> i32:
  return 0

sample:
  mono = bank[1][0:8]
  right = layers[2][1, 0:8]
  dynamic = layers[select()][right_channel(), first_frame():8]
  bank[0][0:4] = 0.0
  layers[1][0, 0:4] = 0.0
  layers[select()][right_channel(), first_frame():4] = 0.0
  out1 = mono[0] + right[0] + dynamic[0]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("buffer collection slices should analyze");
    let mir = lower_test_program(&typed).expect("buffer collection slices should lower");
    validate(&mir).expect("buffer collection slice MIR should validate");
    let dump = format_program(&mir);
    assert!(dump.contains("make_slice @buffer_array(first=0, len=2)[i32(1)] unchecked"));
    assert!(dump.contains("make_slice @buffer_array(first=2, len=3)[i32(2)] unchecked"));
    assert!(dump.contains("slice_fill"));
    for name in ["select", "right_channel", "first_frame"] {
        assert!(
            mir.functions
                .iter()
                .any(|function| function.name.starts_with(name)),
            "slice-coordinate def '{name}' should remain reachable"
        );
    }
}

#[test]
fn unsafe_buffer_access_composes_with_collection_selection_and_aliases() {
    let source = r#"
buffers:
  bank: f32 {2}
  layers: f32[2] {2}

init:
  slot: i32 = 1
  channel: i32 = 1
  frame: i32 = 0

sample:
  selected = bank[slot]
  selected.write_unsafe(frame, 0.25)
  a = selected.read_unsafe(frame)
  b = bank[slot].read_unsafe(frame)
  c = read_unsafe(bank[slot], frame)
  d = bank.read_unsafe(slot, frame)
  layers[slot].write_unsafe(channel, frame, 0.5)
  e = layers[slot].read_unsafe(channel, frame)
  out1 = a + b + c + d + e
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("unsafe selected buffer access should analyze");
    let mir = lower_test_program(&typed).expect("unsafe selected buffer access should lower");
    validate(&mir).expect("unsafe selected buffer MIR should validate");

    let process = formatted_function(&format_program(&mir), "onda_process").to_owned();
    assert!(
        process.contains("@buffer_array(first=0, len=2)"),
        "{process}"
    );
    assert!(
        process.contains("@buffer_array(first=2, len=2)"),
        "{process}"
    );
    assert!(process.contains("] clamp["), "{process}");
    assert!(process.contains("] unchecked["), "{process}");
}

#[test]
fn lowers_data_struct_state_construction_and_scalar_field_methods() {
    let source = r#"
outs:
  out1

struct Inner:
  value: f32 = 1.0

struct Data:
  inner: Inner
  pair: (f32, i32) = (2.0, 3)
  taps: f32[2]

struct Counter:
  value: f32 = 0.0

  def advance(self, amount: f32):
    self.value = self.value + amount
    return self.value

struct Marker:
  value: f32 = 1.0

def relay(counter: Counter, amount: f32):
  return counter.advance(amount)

init:
  data = Data()
  data.taps[0] = 4.0
  counter = Counter()
  markers: Marker[2]
  markers[0].value = 2.0

sample:
  markers[1].value = markers[0].value + 1.0
  out1 = relay(counter, 0.5) + data.inner.value + data.pair[0] + f32(data.pair[1]) + data.taps[0] + markers[1].value
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("data structs should lower");
    validate(&mir).expect("data-struct MIR should validate");

    for state in [
        "data.inner.value",
        "data.pair.__0",
        "data.pair.__1",
        "counter.value",
    ] {
        assert!(
            mir.state.iter().any(|slot| slot.name == state),
            "missing {state}"
        );
    }
    assert!(mir.state.iter().any(|slot| {
        slot.name == "data.taps"
            && matches!(mir.types[slot.ty.index()], MirType::Array { len: 2, .. })
    }));
    assert!(mir.state.iter().any(|slot| {
        slot.name == "markers.value"
            && matches!(mir.types[slot.ty.index()], MirType::Array { len: 2, .. })
    }));

    let advance = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("Counter.advance"))
        .expect("method should lower");
    assert_eq!(
        advance.params[0].mode,
        onda_mir::PassingMode::ReadWriteReference
    );
    assert!(advance.body.statements.iter().any(|statement| matches!(
        statement.kind,
        StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Parameter(_),
                ..
            },
            ..
        }
    )));

    let dump = format_program(&mir);
    assert!(
        dump.contains("$promoted.state") || dump.contains("place @state"),
        "optimized MIR should retain explicit state ownership or its canonical process-local promotion"
    );
    assert!(dump.contains("place @p0"));
}

#[test]
fn lowers_struct_array_constructor_lists_broadcasts_and_element_aliases() {
    let source = r#"
outs:
  out1

struct Meta:
  score: f32 = 0.0

struct Marker:
  value: f32 = 1.0
  meta: Meta
  trail: f32[2]

def update(markers):
  marker = markers[1]
  marker.trail[0] = marker.value
  marker.meta.score = marker.trail[0]
  return marker.meta.score

init:
  markers: Marker[2] = [Marker(value = 2.0), Marker(value = 3.0)]
  broadcast: Marker[2] = Marker(value = 4.0)

sample:
  out1 = update(markers) + broadcast[0].value + broadcast[1].value
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("struct-array aliases should lower");
    validate(&mir).expect("struct-array alias MIR should validate");

    for (name, len) in [
        ("markers.value", 2),
        ("markers.meta.score", 2),
        ("markers.trail", 4),
        ("broadcast.value", 2),
    ] {
        assert!(mir.state.iter().any(|state| {
                state.name == name
                    && matches!(mir.types[state.ty.index()], MirType::Array { len: actual, .. } if actual == len)
            }));
    }

    let dump = format_program(&mir);
    assert!(dump.contains("make_slice"));
    assert!(dump.contains("load_slice"));
    assert!(dump.contains("store_slice"));
}

#[test]
fn lowers_canonical_nested_struct_array_views_across_state_calls_and_aliases() {
    let source = r#"
outs:
  out1

struct Leaf:
  value: f32
  bins: f32[2]

struct Holder:
  leaves: Leaf[2]
  armed: bool

def bump(leaf: Leaf, amount: f32):
  leaf.value = leaf.value + amount
  leaf.bins[1] = leaf.value
  return leaf.bins[1]

def inspect(holder: Holder):
  leaf = holder.leaves[1]
  leaf.value = leaf.value + 0.25
  return leaf.value + bump(holder.leaves[0], 1.0)

init:
  single = Holder()

sample:
  single_leaf = single.leaves[0]
  single_leaf.value = 2.0
  out1 = inspect(single) + bump(single.leaves[1], 0.5) + single_leaf.value
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("nested struct arrays should lower");
    validate(&mir).expect("nested struct-array MIR should validate");

    for (name, len) in [("single.leaves.value", 2), ("single.leaves.bins", 4)] {
        assert!(mir.state.iter().any(|state| {
                state.name == name
                    && matches!(mir.types[state.ty.index()], MirType::Array { len: actual, .. } if actual == len)
            }), "missing canonical state leaf {name}[{len}]");
    }

    let inspect = mir
        .functions
        .iter()
        .find(|function| function.name == "inspect")
        .expect("missing inspect function");
    assert_eq!(
        inspect
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        ["holder.leaves.value", "holder.leaves.bins"]
    );
    assert!(inspect
        .params
        .iter()
        .all(|param| param.mode == onda_mir::PassingMode::ReadWriteReference));

    let dump = format_program(&mir);
    assert!(dump.contains("make_slice @p"));
    assert!(dump.contains("slice_window"));
    assert!(dump.contains("load_slice"));
    assert!(dump.contains("store_slice"));
}

#[test]
fn lowers_direct_indexed_struct_and_proc_array_call_arguments() {
    let source = r#"
struct Cell:
  value: f32 = 1.0
  taps: f32[2]

def read_cell(cell: Cell):
  return cell.value + cell.taps[0]

proc Voice:
  params:
    gain = 0.25

  init:
    phase = 0.0

  sample:
    phase = phase + gain
    out1 = phase

init:
  cells: Cell[2]
  voices: Voice[2] = Voice()
  cursor: i32 = 1

sample:
  out1 = read_cell(cells[cursor]) + voices[cursor]()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed)
        .expect("direct indexed struct and proc-array calls should lower");
    validate(&mir).expect("direct indexed call MIR should validate");

    for (name, len) in [("cells.value", 2), ("cells.taps", 4), ("voices.phase", 2)] {
        assert!(mir.state.iter().any(|state| {
                state.name == name
                    && matches!(mir.types[state.ty.index()], MirType::Array { len: actual, .. } if actual == len)
            }));
    }

    let read_cell = mir
        .functions
        .iter()
        .find(|function| function.name == "read_cell")
        .expect("missing read_cell function");
    assert_eq!(read_cell.params.len(), 2);
    assert!(read_cell
        .params
        .iter()
        .all(|param| param.mode == onda_mir::PassingMode::ReadWriteReference));

    let dump = format_program(&mir);
    assert!(dump.contains("slice_window"));
    assert!(dump.contains("place @state"));
    assert!(dump.contains("make_slice @state"));
}

#[test]
fn unsafe_struct_and_proc_array_reads_preserve_reference_semantics_without_clamps() {
    let source = r#"
outs:
  out1

struct Cell:
  value: f32 = 1.0
  taps: f32[2]

def read_cell(cell: Cell):
  return cell.value + cell.taps[0]

proc Voice:
  params:
    gain = 0.25

  init:
    phase = 0.0

  sample:
    phase = phase + gain
    out1 = phase

init:
  cells: Cell[2]
  voices: Voice[2] = Voice()
  cursor: i32 = 1

sample:
  cell = cells.read_unsafe(cursor)
  voice = read_unsafe(voices, cursor)
  out1 = read_cell(cell) + read_cell(cells.read_unsafe(cursor)) + voice(gain = 0.5)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("unsafe aggregate reads should analyze");
    let mir = lower_test_program(&typed).expect("unsafe aggregate reads should lower");
    validate(&mir).expect("unsafe aggregate-read MIR should validate");

    let process = formatted_function(&format_program(&mir), "onda_process").to_owned();
    assert!(process.contains("load @state"), "{process}");
    assert!(process.contains("slice_window"), "{process}");
    assert!(
        process.contains("place @state3[") && process.contains("] unchecked"),
        "{process}"
    );
    assert!(
        !process.contains("intrinsic range_clamp("),
        "unsafe aggregate selectors must not be normalized:\n{process}"
    );
}

#[test]
fn lowers_forwarded_proc_arrays_with_explicit_slice_abi_and_scratch_state() {
    let source = r#"
proc Voice:
  params:
    gain = 0.25

  init:
    phase = 0.0

  block:
    delta = gain
    sample:
      phase = phase + delta
      out1 = phase

def normalize_index(idx: i32):
  return idx

def step_at(voices, idx: i32):
  return voices[normalize_index(idx)]()

def relay(voices, idx: i32):
  return step_at(voices, idx)

init:
  voices: Voice[2] = Voice()

sample:
  out1 = relay(voices, 1)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("forwarded proc arrays should lower");
    validate(&mir).expect("forwarded proc-array MIR should validate");

    let active_name = runtime_proc_array_active_symbol("voices");
    let active_state = mir
        .state
        .iter()
        .find(|state| state.name == active_name)
        .expect("missing proc-array active-slot state");
    assert_eq!(
        active_state.persistence,
        onda_mir::StatePersistence::InstanceScratch
    );
    assert!(matches!(
        mir.types[active_state.ty.index()],
        MirType::Array {
            element,
            len: 2
        } if matches!(mir.types[element.index()], MirType::Scalar(ScalarType::Bool))
    ));

    let leaf_index = mir
        .functions
        .iter()
        .position(|function| function.name.starts_with("step_at"))
        .expect("missing step_at function");
    let leaf = &mir.functions[leaf_index];
    let normalize_index = mir
        .functions
        .iter()
        .position(|function| function.name == "normalize_index")
        .expect("missing normalize_index function");
    let relay = mir
        .functions
        .iter()
        .find(|function| function.name.starts_with("relay"))
        .expect("missing relay function");
    for function in [leaf, relay] {
        assert!(matches!(
            mir.types[function.params[0].ty.index()],
            MirType::Scalar(ScalarType::I32)
        ));
        assert_eq!(
            function.params[1].name,
            runtime_proc_array_active_symbol("voices")
        );
        assert_eq!(function.params[1].mode, onda_mir::PassingMode::Value);
        assert!(matches!(
            mir.types[function.params[1].ty.index()],
            MirType::Slice {
                element: ScalarType::Bool,
                access: onda_mir::AccessMode::ReadWrite,
            }
        ));
    }
    assert!(relay.body.statements.iter().any(|statement| matches!(
        &statement.kind,
        StatementKind::Call { function, args, .. }
            if function.index() == leaf_index && args.len() == leaf.params.len()
    )));
    assert_eq!(
        leaf.body
            .statements
            .iter()
            .filter(|statement| matches!(
                statement.kind,
                StatementKind::Call { function, .. }
                    if function.index() == normalize_index
            ))
            .count(),
        1,
        "the proc-array index expression must be evaluated exactly once"
    );
}

#[test]
fn lowers_read_only_event_slice_parameters() {
    let source = r#"
init:
  phase = 0.0

event set_curve(values: f32[]):
  phase = values[0]

sample:
  out1 = phase
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("event slices should lower");
    validate(&mir).expect("event slice MIR should validate");
    let event = &mir.interface.events[0];
    assert!(matches!(
        mir.types[event.params[0].ty.index()],
        MirType::Slice {
            element: ScalarType::F32,
            access: onda_mir::AccessMode::ReadOnly,
        }
    ));
    let dump = format_program(&mir);
    assert!(dump.contains("slice<f32, readonly>"));
    assert!(dump.contains("load @event_param0"));
    assert!(dump.contains("load_slice"));
}

#[test]
fn lowers_external_buffer_access_and_metadata() {
    let source = r#"
buffers:
  delay: f32
  bus: f32[2]

init:
  values: f32[2] = [0.25, 0.5]

sample:
  value = delay[0]
  delay[1] = value * 0.5
  two_d = bus[1, 2]
  bus[0, 3] = two_d
  from_state = values[0]
  values[1] = from_state
  if delay.bound():
    value = value + 1.0
  out1 = value + two_d + from_state + f32(delay.len()) + f32(delay.chans()) + delay.samplerate()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("external buffer should lower");
    validate(&mir).expect("buffer MIR should validate");

    assert_eq!(mir.interface.buffers.len(), 2);
    assert_eq!(mir.interface.buffers[0].name, "delay");
    assert_eq!(mir.interface.buffers[0].element, ScalarType::F32);
    assert_eq!(
        mir.interface.buffers[0].channels,
        onda_mir::BufferChannels::Mono
    );
    let dump = format_program(&mir);
    assert!(dump.contains("load_buffer @buffer0"));
    assert!(dump.contains("store_buffer @buffer0"));
    assert!(dump.contains("buffer_len @buffer0"));
    assert!(dump.contains("buffer_channels @buffer0"));
    assert!(dump.contains("buffer_sample_rate @buffer0"));
    assert!(dump.contains("buffer_is_bound @buffer0"));
    assert!(dump.contains("load_buffer @buffer1[i32(1)][i32(2)] clamp"));
    assert!(dump.contains("store_buffer @buffer1[i32(0)][i32(3)] clamp"));
    assert!(dump.contains("] clamp"));
}

#[test]
fn lowers_tuple_state_as_persistent_scalar_components() {
    let source = r#"
init:
  pair = (0.0, 0)

sample:
  out1 = pair[0] + f32(pair[1])
  pair[0] = pair[0] + 0.5
  pair[1] = pair[1] + 1
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("tuple state should lower");
    validate(&mir).expect("tuple-state MIR should validate");

    assert!(mir.state.iter().any(|state| state.name == "pair.__0"));
    assert!(mir.state.iter().any(|state| state.name == "pair.__1"));
    let dump = format_program(&mir);
    assert!(dump.contains("\"pair.__0\""));
    assert!(dump.contains("\"pair.__1\""));
    assert!(dump.contains("store_output @out0"));
}

#[test]
fn lowers_primitive_state_and_constant_arrays() {
    let source = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

init:
  taps: f32[3] = [1.0, 2.0, 3.0]
  counters: i32[4]
  inferred = [1, 2, 3]
  cursor = 1

sample:
  out1 = taps[cursor] + Table[cursor] + f32(inferred[cursor]) + f32(taps.len()) + f32(Table.len())
  taps[cursor] = taps[cursor] * 0.5
  counters[cursor] = counters[cursor] + 1
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_test_program(&typed).expect("primitive arrays should lower");
    validate(&mir).expect("array MIR should validate");

    let taps = mir
        .state
        .iter()
        .find(|state| state.name == "taps")
        .expect("typed array state should exist");
    assert!(matches!(
        mir.types[taps.ty.index()],
        MirType::Array { len: 3, .. }
    ));
    let inferred = mir
        .state
        .iter()
        .find(|state| state.name == "inferred")
        .expect("literal-inferred array state should exist");
    let MirType::Array { element, len: 3 } = mir.types[inferred.ty.index()] else {
        panic!("inferred state should have a three-element array type");
    };
    assert_eq!(mir.types[element.index()], MirType::Scalar(ScalarType::I32));
    assert_eq!(mir.const_data.len(), 1);
    assert_eq!(mir.const_data[0].name, "Table");
    assert_eq!(mir.const_data[0].element, ScalarType::F32);
    assert_eq!(mir.const_data[0].values.len(), 3);

    let dump = format_program(&mir);
    assert!(dump.contains("const_data @data0 \"Table\""));
    assert!(dump.contains("load_const_data @data0"));
    assert!(dump.contains("@state") && dump.contains("] clamp"));
    assert!(block_contains_slice_fill(&mir.functions[0].body));
}

#[test]
fn raw_init_emits_zero_initializers_needed_by_live_reinitialization() {
    let source = r#"
struct Defaults:
  gain: f32 = 0.0
  pair: (f32, i32) = (0.0, 0)
  taps: f32[4]

struct Seed:
  value: f32 = 1.0

  def touch(self):
    self.value = self.value + 1.0

outs:
  out1

init:
  seed = Seed()
  seed.touch()
  scalar = 0.0
  pair = (0.0, 0)
  diff: f32[16384]
  fdn: f32[262144]
  pre: f32[32768]
  explicit: f32[4] = [0.0, 1.0, 0.0, 2.0]
  defaults = Defaults()

sample:
  out1 = seed.value + scalar + pair[0] + f32(pair[1]) + diff[0] + fdn[0] + pre[0] + explicit[0] + defaults.gain + defaults.pair[0] + f32(defaults.pair[1]) + defaults.taps[0]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("source should analyze");
    let mir = lower_program_to_raw_mir(&typed).expect("large state should lower");
    validate(&mir).expect("large-state MIR should validate");

    let init = &mir.functions[mir.entry_points.init.index()];
    assert!(
        block_contains_slice_fill(&init.body),
        "array state declarations must be restored during live init"
    );
    assert!(
        block_contains_all_bits_zero_state_store(&init.body),
        "zero-valued state declarations must be restored during live init"
    );

    let explicit = mir
        .state
        .iter()
        .position(|state| state.name == "explicit")
        .map(|index| onda_mir::StateId::new(index as u32))
        .expect("missing explicit state array");
    let explicit_stores = init
        .body
        .statements
        .iter()
        .filter(|statement| {
            matches!(
                statement.kind,
                StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::State(state),
                        ..
                    },
                    ..
                } if state == explicit
            )
        })
        .count();
    assert_eq!(explicit_stores, 4);
}

#[test]
fn lowers_block_scoped_buffer_aliases_with_persistent_dynamic_selectors() {
    let source = r#"
buffers:
  bank: f32 {2}
params:
  slot: i32 = 0 {0, 1}
block:
  selected = bank[slot]
  frames = selected.len()
  bound = selected.bound()
sample:
  value = selected[0]
  selected[frames - 1] = value
  out1 = value + f32(bound)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("buffer-reference alias should analyze");
    let mir = lower_test_program(&typed).expect("buffer-reference alias should lower");
    validate(&mir).expect("buffer-reference alias MIR should validate");

    assert!(mir
        .state
        .iter()
        .any(|state| state.name == "__onda_buffer_alias_selector_selected"));
    let dump = format_program(&mir);
    assert!(dump.contains("buffer_len @buffer_array(first=0, len=2)"));
    assert!(dump.contains("buffer_is_bound @buffer_array(first=0, len=2)"));
    assert!(dump.contains("load_buffer @buffer_array(first=0, len=2)"));
    assert!(dump.contains("store_buffer @buffer_array(first=0, len=2)"));
}

#[test]
fn ranged_indices_remove_fixed_buffer_collection_selector_clamps() {
    let source = r#"
params:
  slot: i32 = 0 {0, 3}
buffers:
  bank: f32 {4}
sample:
  out1 = bank[slot][0]
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("ranged buffer collection indexing should analyze");
    assert!(
        typed
            .state_integer_ranges
            .contains_key("__onda_clamped_param__slot"),
        "the generated range-clamp alias should retain its semantic range"
    );
    let mir = lower_test_program(&typed).expect("ranged buffer collection indexing should lower");
    validate(&mir).expect("ranged buffer collection MIR should validate");
    assert!(
        mir.state
            .iter()
            .find(|state| state.name == "__onda_clamped_param__slot")
            .and_then(|state| state.integer_range)
            .is_some(),
        "the generated range-clamp alias state should retain its range"
    );

    let dump = format_program(&mir);
    assert!(
        dump.contains("@buffer_array(first=0, len=4)") && dump.contains("] unchecked[i32(0)] clamp"),
        "the fixed collection selector should be unchecked while its runtime frame remains clamped:\n{dump}"
    );
}

#[test]
fn lowers_processor_block_buffer_aliases_across_generated_runtime_functions() {
    let source = r#"
buffers:
  bank: f32[] {2}

proc Reader:
  params:
    slot: i32 = 0 {0, 1}
  buffers:
    clips: f32[] {2}
  outs:
    out1
  block:
    selected = clips[slot]
    frames = selected.len()
    bound = selected.bound()
    sample:
      out1 = selected[0, frames - 1] + f32(bound)

init:
  reader = Reader(clips = bank)
sample:
  out1 = reader()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("processor buffer-reference alias should analyze");
    let mir = lower_test_program(&typed).expect("processor buffer-reference alias should lower");
    validate(&mir).expect("processor buffer-reference alias MIR should validate");

    assert!(mir.state.iter().any(|state| state
        .name
        .ends_with(".__onda_buffer_alias_selector_selected")));
    let dump = format_program(&mir);
    let step = formatted_function(&dump, "Reader.__onda_proc_step");
    assert!(step.contains("load_buffer_param @buffer_param_span"));
    let block_pre = formatted_function(&dump, "Reader.__onda_proc_block_pre");
    assert!(block_pre.contains("buffer_len @buffer_param_span"));
    assert!(block_pre.contains("buffer_is_bound @buffer_param_span"));
}

#[test]
fn lowers_indexed_buffer_receivers_for_lookup_methods() {
    let source = r#"
import std/lookup
buffers:
  bank: f32[2] {2}
sample:
  out1 = bank[1].readL(0, 0.5)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("indexed buffer receiver should analyze");
    let mir = lower_test_program(&typed).expect("indexed buffer receiver should lower");
    validate(&mir).expect("indexed buffer receiver MIR should validate");
    assert!(format_program(&mir).contains("@buffer_array(first=0, len=2)[i32(1)] unchecked"));
}

#[test]
fn rejects_integer_lookup_specializations_that_call_float_only_builtins() {
    let source = r#"
params:
  layer: i32
  position: i32

buffers:
  layers: f32[] {4}

block:
  source = layers[layer]

  sample:
    from_i32 = source.readL(0, position)
    from_i64 = source.readL(0, i64(position))
    out1 = from_i32 + from_i64
"#;
    let parsed = parse_program(source).expect("source should parse");
    let errors = analyze(parsed).expect_err("integer lookup positions should fail semantically");
    for (expected, call_line) in [("got I32", 13), ("got I64", 14)] {
        let error = errors
            .iter()
            .find(|error| error.message.contains(expected))
            .unwrap_or_else(|| panic!("missing '{expected}' diagnostic: {errors:?}"));
        assert_eq!(error.line, call_line, "diagnostic should point at its call");
        assert_ne!(
            error.file.as_deref(),
            Some("<std/lookup.onda>"),
            "diagnostic should not point into generated stdlib code"
        );
    }
}

#[test]
fn lowers_floating_positions_through_nested_lookup_specializations() {
    let source = r#"
buffers:
  source: f32[]

sample:
  from_f32 = source.readL(0, f32(1))
  from_f64 = source.readL(0, f64(1))
  out1 = from_f32 + f32(from_f64)
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("floating lookup positions should analyze");
    let mir = lower_test_program(&typed).expect("floating lookup positions should lower");
    validate(&mir).expect("floating lookup position MIR should validate");
}

#[test]
fn proc_buffer_collections_lower_static_subspans_and_dynamic_access() {
    let source = r#"
buffers:
  bank: f32 {8}
outs:
  out1

def first(buf: buffer<f32>):
  return buf[0]

def first_slice(xs: f32[]):
  return xs[0]

proc Reader:
  params:
    slot: i32 = 0
  buffers:
    clips: f32 {6}
  outs:
    out1
  sample:
    out1 = clips[slot][0] + first(clips[slot]) + first_slice(clips[slot][0:1]) + f32(clips.len()) + f32(clips[slot].len())

init:
  reader = Reader(slot = 2, clips = bank[1:7])
sample:
  out1 = reader()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("buffer collection subspan should analyze");
    let mir = lower_test_program(&typed).expect("buffer collection subspan should lower");
    validate(&mir).expect("buffer collection MIR should validate");

    let step = mir
        .functions
        .iter()
        .find(|function| function.name == "Reader.__onda_proc_step")
        .expect("missing reader step function");
    assert_eq!(
        step.params
            .iter()
            .filter(|parameter| {
                matches!(
                    mir.types[parameter.ty.index()],
                    onda_mir::Type::BufferSpan { len: 6, .. }
                )
            })
            .count(),
        1,
        "fixed collection must lower to one constant-size descriptor span"
    );
    assert!(
        format_program(&mir).contains("@buffer_param_span"),
        "dynamic proc-local selection should remain explicit in MIR"
    );
}

#[test]
fn proc_scalar_buffer_accepts_selected_top_level_collection_slot() {
    let source = r#"
buffers:
  bank: f32 {8}
outs:
  out1
proc Reader:
  buffers:
    clip: f32
  outs:
    out1
  sample:
    out1 = clip[0]
init:
  reader = Reader(clip = bank[2])
sample:
  out1 = reader()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("selected buffer slot should analyze");
    let mir = lower_test_program(&typed).expect("selected buffer slot should lower");
    validate(&mir).expect("selected buffer slot MIR should validate");
}

#[test]
fn proc_buffer_collection_count_mismatch_is_rejected() {
    let source = r#"
buffers:
  bank: f32 {8}
outs:
  out1
proc Reader:
  buffers:
    clips: f32 {6}
  outs:
    out1
  sample:
    out1 = clips[0][0]
init:
  reader = Reader(clips = bank)
sample:
  out1 = reader()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let errors = analyze(parsed).expect_err("mismatched collection counts must fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expects 6 buffers, got 8")),
        "missing precise collection-count diagnostic: {errors:?}"
    );
}

#[test]
fn proc_buffer_collection_subspan_requires_static_in_bounds_exact_range() {
    let cases = [
        ("bank[4:10]", "is outside 0..8"),
        ("bank[1:6]", "expects 6 buffers, got 5"),
        ("bank[offset:offset + 6]", "compile-time slice start bound"),
    ];
    for (binding, expected) in cases {
        let source = format!(
            r#"
buffers:
  bank: f32 {{8}}
params:
  offset: i32 = 0
outs:
  out1
proc Reader:
  buffers:
    clips: f32 {{6}}
  outs:
    out1
  sample:
    out1 = clips[0][0]
init:
  reader = Reader(clips = {binding})
sample:
  out1 = reader()
"#
        );
        let parsed = parse_program(&source).expect("source should parse");
        let errors = analyze(parsed).expect_err("invalid collection subspan must fail");
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "missing '{expected}' diagnostic for {binding}: {errors:?}"
        );
    }
}

#[test]
fn proc_buffer_collections_forward_through_nested_procs_without_copying() {
    let source = r#"
buffers:
  bank: f32 {8}
outs:
  out1
proc Child:
  buffers:
    clips: f32 {4}
  outs:
    out1
  sample:
    out1 = clips[2][0]
proc Parent:
  buffers:
    clips: f32 {6}
  init:
    child = Child(clips = clips[1:5])
  outs:
    out1
  sample:
    out1 = child()
init:
  parent = Parent(clips = bank[1:7])
sample:
  out1 = parent()
"#;
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze(parsed).expect("nested buffer subspans should analyze");
    let mir = lower_test_program(&typed).expect("nested buffer subspans should lower");
    validate(&mir).expect("nested buffer subspan MIR should validate");
    assert!(mir.functions.iter().any(|function| {
        function.name == "Parent.__onda_proc_step"
            && function
                .params
                .iter()
                .filter(|parameter| {
                    matches!(
                        mir.types[parameter.ty.index()],
                        onda_mir::Type::BufferSpan { len: 6, .. }
                    )
                })
                .count()
                == 1
    }));
}
