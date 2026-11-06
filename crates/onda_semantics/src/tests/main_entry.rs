use super::*;

fn compile(source: &str) -> (TypedProgram, onda_mir::ValidatedProgram) {
    let typed = analyze_source(source);
    let mir = lower_program_to_validated_mir(&typed).unwrap_or_else(|errors| panic!("{errors:?}"));
    (typed, mir)
}

#[test]
fn main_helpers_capture_authored_state_and_private_params_stay_internal() {
    let (typed, mir) = compile(
        r#"
proc Main:
  params:
    gain = 0.5 {0.0, 1.0, unit = "x"}
    private internal = 2.0
  init:
    state = 1.0
  def next():
    state = state + internal
    return state * gain
  sample:
    out1 = next()
"#,
    );
    assert_eq!(typed.params.len(), 1);
    assert_eq!(mir.interface.params[0].name, "gain");
    assert_eq!(mir.interface.params[0].control.unit.as_deref(), Some("x"));
    assert!(mir
        .state
        .iter()
        .any(|state| state.name.ends_with(".state") && state.authored));
}

#[test]
fn main_projects_logical_arrays_buffers_and_messages() {
    let (typed, mir) = compile(
        r#"
proc Main:
  ins:
    input: f32[2]
  outs:
    output: f32[2]
  params:
    gains: f32[2] = [0.25, 0.75] {0.0, 1.0, unit = "x"}
  buffers:
    data: f32 {2}
  init:
    offset = 0.0
  delegate changed(value: f32)
  event set_offset(gains: f32 = 1.0):
    offset = gains + data[0][0]
    changed(offset)
  sample:
    output[0] = input[0] * gains[0] + offset
    output[1] = input[1] * gains[1] + offset
"#,
    );
    assert_eq!(typed.ins.len(), 2);
    assert_eq!(typed.outs.len(), 2);
    assert_eq!(mir.interface.inputs[0].name, "input");
    assert_eq!(mir.interface.outputs[0].name, "output");
    assert_eq!(mir.interface.params.len(), 1);
    assert_eq!(mir.interface.params[0].name, "gains");
    assert_eq!(mir.interface.buffers.len(), 2);
    assert_eq!(mir.interface.buffers[0].name, "data[0]");
    assert_eq!(mir.interface.buffers[1].name, "data[1]");
    assert_eq!(mir.interface.buffer_arrays[0].name, "data");
    assert_eq!(mir.interface.events.len(), 1);
    assert_eq!(mir.interface.events[0].name, "set_offset");
    assert_eq!(mir.interface.delegates.len(), 1);
}

#[test]
fn main_supports_control_and_event_only_entries() {
    let (typed, mir) = compile("proc Main:\n  block:\n    kout1 = 1.0\n");
    assert_eq!(mir.interface.control_outputs[0].name, "kout1");
    assert!(typed.outs.is_empty());
    for outputs in ["", "  outs 1\n"] {
        compile(&format!(
            "proc Main:\n{outputs}  init:\n    state = 0.0\n  event reset():\n    state = 0.0\n"
        ));
    }
}

#[test]
fn main_keeps_declaration_ranges_and_constructor_defaults_in_their_contexts() {
    let (typed, _) = compile(
        r#"
proc Main:
  params:
    value = SR {0.0, SR, unit = "Hz"}
  sample 4:
    out1 = value
"#,
    );
    assert_eq!(typed.param_default("value"), Some(48_000.0));
    assert_eq!(
        typed.params[0].range.expect("range").max.to_f64(),
        192_000.0
    );
}

#[test]
fn main_graph_exposes_inferred_ports() {
    let (_, mir) = compile(
        r#"
proc Gain:
  sample:
    out1 = in1 * 0.5
proc Main:
  init:
    gain = Gain()
  graph:
    in1 >> gain.in1
    gain.out1 >> out1
"#,
    );
    assert_eq!(mir.interface.inputs[0].name, "in1");
    assert_eq!(mir.interface.outputs[0].name, "out1");
}

#[test]
fn main_exports_const_backed_defaults_and_enum_controls() {
    let (typed, _) = compile(
        r#"
enum Mode:
  Off
  On
const Values = [0.25, 0.75]
const def initial() -> f32:
  return 0.5
proc Main:
  params:
    gain = initial() {0.0, 1.0, step = 0.25}
    values: f32[2] = Values {0.0, 1.0}
    mode: Mode = Mode::On
  sample:
    out1 = gain + values[0] + i32(mode)
"#,
    );
    assert_eq!(typed.param_default("gain"), Some(0.5));
    assert_eq!(typed.param_default("values[1]"), Some(0.75));
    assert_eq!(typed.params[0].control.step_count, Some(4));
}

#[test]
fn proc_metadata_keeps_internal_integer_ranges_independent_of_host_limits() {
    compile(
        r#"
proc Internal:
  params:
    wide: i64 = 0 {0, 9223372036854775807, unit = "ticks"}
    fixed: i32 = 1 {1, 1}
  sample:
    out1 = 0.0
proc Main:
  init:
    internal = Internal()
  sample:
    out1 = internal()
"#,
    );
    let errors = analyze(parse_program("proc Main:\n  params:\n    wide: i64 = 0 {0, 9223372036854775807}\n  sample:\n    out1 = 0.0\n").expect("parse")).expect_err("host range is bounded");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("exact host integer")));
}

#[test]
fn main_fields_do_not_shadow_module_declarations() {
    for (declaration, sample) in [
        ("params:\n    gain = 0.5", "out1 = gain"),
        ("ins:\n    gain", "out1 = gain"),
        ("outs:\n    gain", "gain = 0.5"),
        ("buffers:\n    gain: f32", "out1 = gain[0]"),
    ] {
        compile(&format!("def gain(value):\n  return value * 2.0\nproc Main:\n  {declaration}\n  sample:\n    {sample}\n"));
    }
    let (_, mir) = compile("proc Main:\n  params:\n    Main = 1.0\n  sample:\n    out1 = Main\n");
    assert_eq!(mir.interface.params[0].name, "Main");
    let (_, mir) = compile("def reset(value):\n  return value * 2.0\nproc Main:\n  init:\n    value = 0.0\n  event reset():\n    value = 1.0\n  sample:\n    out1 = value\n");
    assert_eq!(mir.interface.events[0].name, "reset");
}

#[test]
fn main_preserves_single_element_parameter_arrays() {
    let (_, mir) = compile(
        "proc Main:\n  params:\n    gains: f32[1] = [0.5]\n  sample:\n    out1 = gains[0]\n",
    );
    assert_eq!(mir.interface.params[0].name, "gains");
    assert!(matches!(
        mir.types[mir.interface.params[0].ty.index()],
        onda_mir::Type::Array { len: 1, .. }
    ));
}

#[test]
fn main_preserves_parameter_array_element_types() {
    for (ty, values, expression) in [
        ("i32", "[16777217, 16777219]", "f32(values[0])"),
        (
            "i64",
            "[9007199254740993, 9007199254740995]",
            "f32(values[0])",
        ),
        ("bool", "[true, false]", "f32(values[0])"),
        ("Mode", "[Mode::A, Mode::B]", "f32(i32(values[0]))"),
    ] {
        let (_, mir) = compile(&format!("enum Mode:\n  A\n  B\nproc Main:\n  params:\n    values: {ty}[2] = {values}\n  sample:\n    out1 = {expression}\n"));
        assert_eq!(mir.interface.params[0].name, "values");
    }
}

#[test]
fn main_exports_fixed_array_message_defaults() {
    let (_, mir) = compile(
        r#"
proc Main:
  init:
    value = 0.0
  delegate changed(values: f32[2] = [0.25, 0.75])
  event set(values: f32[2] = [0.25, 0.75]):
    value = values[0]
    changed()
  sample:
    out1 = value
"#,
    );
    let expected = Some(onda_mir::ConstantValue::Aggregate(vec![
        onda_mir::ConstantValue::Scalar(onda_mir::ScalarValue::F32(0.25)),
        onda_mir::ConstantValue::Scalar(onda_mir::ScalarValue::F32(0.75)),
    ]));
    assert_eq!(mir.interface.events[0].params[0].default, expected);
}

#[test]
fn main_array_message_defaults_use_the_constant_evaluator() {
    for default in ["[SR, SR / 2.0]", "Values", "initial()"] {
        let (_, mir) = compile(&format!(
            r#"
const Values: f32[2] = [SR, SR / 2.0]
const def initial() -> f32[2]:
  return Values
proc Main:
  delegate changed(values: f32[2] = {default})
  event set(values: f32[2] = {default}):
    changed(values)
  sample 4:
    out1 = 0.0
"#
        ));
        let expected = Some(onda_mir::ConstantValue::Aggregate(vec![
            onda_mir::ConstantValue::Scalar(onda_mir::ScalarValue::F32(48_000.0)),
            onda_mir::ConstantValue::Scalar(onda_mir::ScalarValue::F32(24_000.0)),
        ]));
        assert_eq!(mir.interface.events[0].params[0].default, expected);
    }
}

#[test]
fn main_array_message_defaults_preserve_element_types() {
    use onda_mir::{ConstantValue, ScalarValue};
    for (ty, default, expected) in [
        (
            "bool",
            "[true, false]",
            vec![ScalarValue::Bool(true), ScalarValue::Bool(false)],
        ),
        (
            "Mode",
            "[Mode::On, Mode::Off]",
            vec![ScalarValue::I32(1), ScalarValue::I32(0)],
        ),
        (
            "i64",
            "[9007199254740993, 9007199254740995]",
            vec![
                ScalarValue::I64(9007199254740993),
                ScalarValue::I64(9007199254740995),
            ],
        ),
    ] {
        let (_, mir) = compile(&format!("enum Mode:\n  Off\n  On\nproc Main:\n  delegate changed(values: {ty}[2] = {default})\n  event set(values: {ty}[2] = {default}):\n    changed(values)\n  sample:\n    out1 = 0.0\n"));
        assert_eq!(
            mir.interface.events[0].params[0].default,
            Some(ConstantValue::Aggregate(
                expected.into_iter().map(ConstantValue::Scalar).collect()
            ))
        );
    }
}

#[test]
fn proc_constructors_preserve_single_element_parameter_arrays() {
    let proc = "proc Voice:\n  params:\n    values: i32[1] = [1]\n  init:\n    initial: i32 = values[0]\n  def current():\n    return values[0] + initial\n  sample:\n    out1 = f32(current())\n";
    for constructor in [
        "voice = Voice(values = [7])",
        "voice: Voice[2] = Voice(values = [7])",
    ] {
        let output = if constructor.contains("[2]") {
            "voice[0]() + voice[1]()"
        } else {
            "voice()"
        };
        // Exercise both top-level construction and construction inside a proc.
        for (prefix, indent) in [("", ""), ("proc Main:\n", "  ")] {
            let source = format!("{proc}{prefix}{indent}init:\n{indent}  {constructor}\n{indent}sample:\n{indent}  out1 = {output}\n");
            compile(&source);
            assert_analyze_error_contains(
                &source.replace("values = [7]", "values = 7"),
                "array argument requires an array literal or array symbol",
            );
        }
    }
}

#[test]
fn proc_constructor_array_arguments_keep_assignment_type_checks() {
    for (source_ty, dest_ty) in [("f64", "i32"), ("i64", "i32"), ("bool", "i32")] {
        let source = format!("params:\n  values: {source_ty}[2]\nproc Voice:\n  params:\n    values: {dest_ty}[2]\n  sample:\n    out1 = 0.0\ninit:\n  voice = Voice(values = values)\nsample:\n  out1 = voice()\n");
        let errors = analyze(parse_program(&source).expect("parse"))
            .expect_err("incompatible array element types");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("struct field init")),
            "{source}\n{errors:?}"
        );
    }
}
