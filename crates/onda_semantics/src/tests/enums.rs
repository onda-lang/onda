use super::*;

const MODES: &str = "enum Mode:\n  LowPass\n  HighPass\n  BandPass {label = \"Band-pass\"}\n";

#[test]
fn enum_tuple_annotations_share_scalar_resolution_in_every_context() {
    compile(
        r#"
namespace DSP<N = 2>:
  enum Mode:
    A
    B
  def initial() -> (Mode, i32):
    pair: (Mode, i32) = (Mode::B, N)
    return pair
namespace D = DSP<4>
use D::Mode as Choice
def identity(pair: (Choice, i32)) -> (Choice, i32):
  local: (Choice, i32) = pair
  return local
proc Selector<T>:
  init:
    current: (T, i32) = (T(0), 0)
  delegate changed(pair: (T, i32))
  event select(pair: (T, i32)):
    current = pair
    changed(current)
  sample:
    out1 = i32(current[0]) + current[1]
init:
  current: (Choice, i32) = D::initial()
  selector = Selector<Choice>()
delegate changed(pair: (Choice, i32))
when changed(pair):
  current = identity(pair)
when selector.changed(pair):
  changed(pair)
event select(pair: (Choice, i32) = (Choice::A, 0)):
  selector.select(pair)
block:
  local: (Choice, i32) = identity(current)
  sample:
    value: (Choice, i32) = local
    out1 = i32(value[0]) + selector()
"#,
    );
}

#[test]
fn enum_tuple_type_arguments_infer_and_specialize_without_losing_identity() {
    compile(&format!("{MODES}\ndef identity<T>(pair: (T, i32)) -> (T, i32):\n  local: (T, i32) = pair\n  return local\ninit:\n  a = identity((Mode::HighPass, 7))\n  b = identity<Mode>((Mode::BandPass, 8))\nsample:\n  out1 = i32(a[0]) + i32(b[0])\n"));
    let source = format!("{MODES}\nenum Other:\n  A\ndef select<T>(pair: (T, T)) -> T:\n  return pair[0]\ninit:\n  value = select((Mode::HighPass, Other::A))\nsample:\n  out1 = 0.0\n");
    assert!(analyze(onda_frontend::parse_program(&source).unwrap()).is_err());
}

#[test]
fn enum_tuple_annotations_reject_non_scalar_types_and_preserve_nominality() {
    for body in [
        "struct Data:\n  value = 0.0\nevent select(pair: (Data, i32)):\n  print(pair[1])\n",
        "def select(pair: (Missing, i32)):\n  return pair[1]\n",
        "init:\n  pair: (Mode, i32) = (Other::A, 0)\n",
        "def select(pair: (Mode, i32)) -> Mode:\n  return pair[0]\ninit:\n  value = select((Other::A, 0))\n",
        "event select(pair: (Mode, i32) = (Other::A, 0)):\n  print(pair[1])\n",
        "delegate changed(pair: (Mode, i32))\nblock:\n  changed((Other::A, 0))\n",
    ] {
        let source = format!("{MODES}\nenum Other:\n  A\n{body}\nsample:\n  out1 = 0.0\n");
        let program = onda_frontend::parse_program(&source).unwrap();
        assert!(analyze(program).is_err(), "{source}");
    }
}

fn compile(source: &str) -> TypedProgram {
    let program = analyze_source(source);
    lower_program_to_optimized_mir(&program).unwrap_or_else(|errors| panic!("{errors:?}"));
    program
}

#[test]
fn enum_controls_state_functions_and_events_lower_to_i32() {
    let program = compile(&format!("{MODES}\nparams:\n  mode = Mode::HighPass\ninit:\n  current: Mode = Mode::LowPass\ndef identity(value: Mode) -> Mode:\n  return value\nevent set_mode(value: Mode):\n  current = identity(value)\nsample:\n  if mode == Mode::BandPass:\n    out1 = i32(current)\n  else:\n    out1 = i32(mode)\n"));
    assert!(matches!(program.params[0].ty, PrimitiveType::Enum(_)));
    assert_eq!(program.params[0].default.to_f64(), 1.0);
    assert_eq!(
        program.enums[0].members[2].label.as_deref(),
        Some("Band-pass")
    );
    assert_eq!(program.params[0].range.unwrap().max.to_f64(), 2.0);
}

#[test]
fn enums_preserve_nominal_identity() {
    for body in [
        "init:\n  value: Mode = 0\n",
        "enum Other:\n  A\ninit:\n  value: Mode = Other::A\n",
        "sample:\n  out1 = Mode::LowPass + 1\n",
        "sample:\n  out1 = f32(Mode::LowPass)\n",
        "sample:\n  out1 = Mode::LowPass < Mode::HighPass\n",
    ] {
        let source = format!("{MODES}\n{body}");
        assert!(
            analyze(onda_frontend::parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn enum_struct_fields_and_arrays_work_in_messages() {
    compile(&format!("{MODES}\nstruct Settings:\n  mode: Mode = Mode::HighPass\n  modes: Mode[2]\ninit:\n  current: Settings\n  modes: Mode[2] = [Mode::LowPass, Mode::HighPass]\nevent configure(settings: Settings):\n  current = settings\nevent select(values: Mode[]):\n  modes[0] = values[0]\ndelegate changed(settings: Settings)\nsample:\n  out1 = i32(current.mode) + i32(modes[0])\n"));
}

#[test]
fn enum_namespaces_and_type_inference() {
    compile("namespace DSP:\n  enum Mode:\n    A\n    B\n  def identity(value: Mode) -> Mode:\n    return value\nuse DSP::Mode as Choice\nparams:\n  mode: Choice = Choice::B\ninit:\n  value = DSP::identity(mode)\nsample:\n  out1 = i32(value)\n");
}

#[test]
fn enum_constants_and_structural_specializations_preserve_identity() {
    compile(&format!("{MODES}\nenum Shape:\n  Sine\n  Saw\nconst DefaultMode = Mode::HighPass\nconst Modes = [Mode::LowPass, Mode::BandPass]\nconst def choose(mode: Mode) -> Mode:\n  return mode\nconst Selected = choose(DefaultMode)\ndef identity(value):\n  return value\nparams:\n  mode: Mode = Selected\n  shape = Shape::Saw\n  modes: Mode[2] = Modes\nsample:\n  a = identity(mode)\n  b = identity(shape)\n  out1 = i32(a) + i32(b) + i32(modes[1])\n"));
}

#[test]
fn enum_types_are_distinct_in_namespace_specializations() {
    compile("namespace DSP<N = 2>:\n  enum Mode:\n    A\n    B\n  def identity(value: Mode) -> Mode:\n    return value\nnamespace D = DSP<3>\nparams:\n  mode: D::Mode = D::Mode::B\nsample:\n  out1 = i32(D::identity(mode))\n");
}

#[test]
fn enum_parameters_and_numeric_builtins_reject_integer_substitution() {
    for body in [
        "params:\n  value: Mode = 0\n",
        "params:\n  value: i32 = Mode::LowPass\n",
        "params:\n  value: f32 = Mode::LowPass\n",
        "const Invalid: i32 = Mode::LowPass\n",
        "const Invalid = f32(Mode::LowPass)\n",
        "const Invalid = Mode(1.5)\n",
        "const Invalid = Mode(true)\n",
        "const Invalid = ~Mode::LowPass\n",
        "const Invalid = Mode::LowPass < Mode::HighPass\n",
        "const Invalid = abs(Mode::LowPass)\n",
        "ins:\n  mode = Mode::LowPass\n",

        "params:\n  mode = Mode::LowPass\nsample:\n  out1 = abs(mode)\n",
        "params:\n  mode = Mode::LowPass\nsample:\n  out1 = sin(mode)\n",
        "params:\n  mode = Mode::LowPass\nsample:\n  out1 = mode == 0\n",
        "init:\n  current = Mode::LowPass\nevent set_mode(mode: Mode):\n  current = mode\nevent invalid():\n  set_mode(1)\n",
    ] {
        let source = format!("{MODES}\n{body}");
        let errors = analyze(onda_frontend::parse_program(&source).unwrap()).expect_err(&source);
        assert!(errors.iter().all(|error| !error.message.contains("duplicate block")), "{errors:?}");
    }
}

#[test]
fn enum_defaults_tuple_fields_and_processor_parameters() {
    compile(&format!("{MODES}\nstruct Settings:\n  pair: (Mode, i32) = (Mode::HighPass, 7)\nproc Selector:\n  params:\n    mode: Mode = Mode::HighPass\n  sample:\n    out1 = i32(mode)\ninit:\n  current: Mode\n  settings: Settings\n  selector = Selector(mode = Mode::BandPass)\nevent update(mode: Mode):\n  current = mode\n  selector.mode = mode\nsample:\n  out1 = selector(mode = current) + i32(settings.pair[0])\n"));
}

#[test]
fn enum_audio_ports_are_rejected() {
    for declaration in [
        "ins:\n  mode: Mode",
        "ins:\n  modes: Mode[2]",
        "outs:\n  mode: Mode",
        "outs:\n  modes: Mode[2]",
    ] {
        let source = format!("{MODES}\n{declaration}\nsample:\n  out1 = 0.0\n");
        let errors = analyze(onda_frontend::parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("audio ports cannot use enum types")),
            "{errors:?}"
        );
    }
}

#[test]
fn enum_processor_inputs_are_rejected() {
    for declaration in [
        "ins:\n    mode: Mode",
        "ins:\n    modes: Mode[2]",
        "ins<Mode>:\n    mode",
    ] {
        let source = format!(
            "{MODES}\nproc Selector:\n  {declaration}\n  sample:\n    out1 = 0.0\ninit:\n  selector = Selector()\nsample:\n  out1 = 0.0\n"
        );
        let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("audio ports cannot use enum types")),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn enum_generic_processor_inputs_are_rejected() {
    for declaration in [
        "ins:\n    mode: T",
        "ins:\n    modes: T[2]",
        "ins<T>:\n    mode",
    ] {
        for constructor in ["Selector<Mode>()", "Selector(choice = Mode::HighPass)"] {
            let source = format!(
                "{MODES}\nproc Selector<T>:\n  {declaration}\n  params:\n    choice: T\n  sample:\n    out1 = 0.0\ninit:\n  selector = {constructor}\nsample:\n  out1 = 0.0\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("audio ports cannot use enum types")),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn numeric_processor_inputs_accept_enum_parameters() {
    compile(&format!(
        "{MODES}\nproc Selector<T>:\n  ins<T>:\n    value\n  params:\n    mode = Mode::HighPass\n  sample:\n    out1 = value + i32(mode)\ninit:\n  selector = Selector<f32>()\nsample:\n  out1 = selector(0.25)\n"
    ));
}

#[test]
fn enum_names_cannot_conflict_with_other_declarations() {
    for body in [
        "struct Mode:\n  value: i32\n",
        "def Mode(value):\n  return value\n",
        "const Mode = 7\n",
    ] {
        let source = format!("{MODES}\n{body}");
        let errors = analyze(onda_frontend::parse_program(&source).unwrap())
            .expect_err("enum declaration collision");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("conflicts with another declaration")),
            "{errors:?}"
        );
    }
}

#[test]
fn explicit_enum_type_arguments_use_the_nominal_scalar() {
    compile(&format!("{MODES}\ndef identity<T>(value: T) -> T:\n  return value\nstruct Box<T>:\n  value: T\nparams:\n  mode = Mode::HighPass\ninit:\n  boxed = Box<Mode>(mode)\nsample:\n  out1 = i32(identity<Mode>(mode)) + i32(boxed.value)\n"));
    compile("namespace DSP:\n  enum Mode:\n    A\n    B\nuse DSP::Mode as Choice\ndef identity<T>(value: T) -> T:\n  return value\nsample:\n  out1 = i32(identity<Choice>(Choice::B))\n");
}

#[test]
fn namespace_template_enums_work_in_generic_struct_defaults_and_methods() {
    compile("namespace Tables<N = 2>:\n  enum Mode:\n    A\n    B\n  struct Setting<T>:\n    mode = Mode::B\n    def read(self):\n      return self.mode\nnamespace A = Tables<4>\nnamespace B = Tables<8>\ninit:\n  a = A::Setting<f32>()\n  b = B::Setting<f64>()\nsample:\n  out1 = i32(a.read()) + i32(b.read())\n");
}

#[test]
fn typed_enum_constants_and_configuration_use_i32_host_values() {
    let source = format!("{MODES}\nconst Default: Mode = Mode::HighPass\nconst Defaults: Mode[] = [Default, Mode::LowPass]\nconfig const Selected: Mode = Default\nconfig const Choices: Mode[2] = Defaults\nparams:\n  selected = Selected\n  choices: Mode[2] = Choices\nsample:\n  out1 = i32(selected) + i32(choices[0]) + i32(choices[1])\n");
    let program = parse_program(&source).unwrap();
    let options = AnalysisOptions::default();
    let inputs = compile_inputs_from_literals(
        &program,
        [
            ("Selected".into(), "99".into()),
            ("Choices".into(), "[-99, 1]".into()),
        ],
        options,
    )
    .unwrap();
    let descriptors = inspect_compile_constants(program.clone(), options, &inputs).unwrap();
    assert_eq!(
        descriptors[0].enumeration.as_ref().unwrap().members.len(),
        3
    );
    let ConstValue::Scalar(value) = descriptors[0].value else {
        panic!("scalar enum selection")
    };
    assert_eq!(value.to_f64(), 2.0);
    let ConstValue::Array { values, .. } = &descriptors[1].value else {
        panic!("enum array selection")
    };
    assert_eq!(
        values
            .iter()
            .map(|value| value.to_f64())
            .collect::<Vec<_>>(),
        [0.0, 1.0]
    );
    let typed = analyze_with_options_and_inputs(program, options, &inputs).unwrap();
    lower_program_to_optimized_mir(&typed).unwrap();
}

#[test]
fn enum_const_annotations_resolve_namespace_aliases_and_reject_other_enums() {
    compile("namespace DSP:\n  enum Mode:\n    A\n    B\nuse DSP::Mode as Choice\nconst Default: Choice = Choice::B\nparams:\n  mode = Default\nsample:\n  out1 = i32(mode)\n");
    let source = format!(
        "{MODES}\nenum Other:\n  A\nconst Invalid: Mode = Other::A\nsample:\n  out1 = 0.0\n"
    );
    assert!(analyze(parse_program(&source).unwrap()).is_err());
}

#[test]
fn enum_const_annotations_work_in_namespace_templates() {
    compile("namespace DSP<N = 2>:\n  enum Mode:\n    A\n    B\n  const Default: Mode = Mode::B\n  const Choices: Mode[2] = [Mode::A, Default]\nnamespace D = DSP<4>\nsample:\n  out1 = i32(D::Default) + i32(D::Choices[0])\n");
}

#[test]
fn enum_names_respect_generic_parameter_scope() {
    compile("enum T:\n  A\n  B\nsample:\n  out1 = 0.0\n");
    compile("enum Mode:\n  A\n  B\ndef identity<Mode>(value: Mode) -> Mode:\n  local: Mode = Mode(value)\n  return local\nstruct Box<Mode>:\n  value: Mode\n  def read(self) -> Mode:\n    return self.value\nproc Selector<Mode>:\n  params<Mode>:\n    value = 0.0\n  sample:\n    out1 = value\ninit:\n  box = Box<f32>(0.5)\n  selector = Selector<f32>()\nsample:\n  out1 = identity<f32>(box.read()) + selector()\n");
}

#[test]
fn namespace_template_enum_conversions_and_const_functions_are_checked() {
    let program = compile("namespace DSP<N = 2>:\n  enum Mode:\n    A\n    B\n  const def identity(value: Mode) -> Mode:\n    return value\n  const Selected: Mode = identity(Mode(N))\nnamespace D = DSP<99>\nparams:\n  mode = D::Selected\nsample:\n  out1 = i32(mode)\n");
    assert_eq!(program.params[0].default.to_f64(), 1.0);
}

#[test]
fn enum_type_arguments_work_in_nominal_type_annotations() {
    compile(&format!("{MODES}\nstruct Box<T>:\n  value: T\nstruct Container:\n  boxed: Box<Mode>\ndef read(boxed: Box<Mode>) -> Mode:\n  return boxed.value\ndef make(value: Mode) -> Box<Mode>:\n  return Box<Mode>(value)\ninit:\n  boxed: Box<Mode> = make(Mode::BandPass)\n  container: Container\nsample:\n  out1 = i32(read(boxed)) + i32(container.boxed.value)\n"));
}

#[test]
fn inferred_enum_type_arguments_preserve_nominal_identity() {
    compile(&format!("{MODES}\ndef choose<T>(a: T, b: T) -> T:\n  return b\nstruct Box<T>:\n  value: T\ninit:\n  boxed = Box(Mode::HighPass)\nsample:\n  out1 = i32(choose(Mode::LowPass, boxed.value))\n"));
    let source = format!("{MODES}\nenum Other:\n  A\ndef choose<T>(a: T, b: T) -> T:\n  return b\nsample:\n  out1 = i32(choose(Mode::LowPass, Other::A))\n");
    assert!(analyze(parse_program(&source).unwrap()).is_err());
}

#[test]
fn enum_generic_types_resolve_aliases_and_preserve_exact_arguments() {
    compile("namespace DSP<N = 1, M = 2>:\n  enum Mode:\n    A\n    B\nstruct Box<T>:\n  value: T\ninit:\n  boxed: Box<DSP<1, 2>::Mode> = Box<DSP<1, 2>::Mode>(DSP<1, 2>::Mode::B)\nsample:\n  out1 = i32(boxed.value)\n");
    compile("namespace DSP:\n  enum Mode:\n    A\n    B\nstruct Box<T>:\n  value: T\ninit:\n  boxed: Box<DSP::Mode> = Box<DSP::Mode>(DSP::Mode::B)\nsample:\n  out1 = i32(boxed.value)\n");
    compile("namespace DSP:\n  enum Mode:\n    A\n    B\nuse DSP::Mode as Choice\nstruct Box<T>:\n  value: T\nstruct Pair<T, U>:\n  a: Box<T>\n  b: U\ndef read<T>(box: Box<T>) -> T:\n  return box.value\ninit:\n  boxes: Box<Choice>[2]\n  pair: Pair<Choice, f32>\nsample:\n  out1 = i32(read(boxes[0])) + i32(pair.a.value)\n");
    let source = format!("{MODES}\nenum Other:\n  A\nstruct Box<T>:\n  value: T\ndef read(box: Box<Mode>) -> Mode:\n  return box.value\ninit:\n  box = Box<Other>(Other::A)\nsample:\n  out1 = i32(read(box))\n");
    assert!(analyze(parse_program(&source).unwrap()).is_err());
}

#[test]
fn enum_generic_processor_arguments_can_be_inferred() {
    compile(&format!("{MODES}\nproc Selector<T>:\n  params:\n    mode: T\n  sample:\n    out1 = i32(mode)\ninit:\n  selector = Selector(mode = Mode::HighPass)\nsample:\n  out1 = selector()\n"));
}

#[test]
fn template_enum_checking_keeps_aliases_and_nested_identities() {
    let program = compile("namespace DSP<N = 2>:\n  namespace Choices:\n    enum Mode:\n      A\n      B\n  use Choices::Mode as Choice\n  const def select(value: Choice) -> Choice:\n    local: Choice = Choice(i32(value))\n    return local\n  const Selected: Choice = select(Choice(N))\nnamespace A = DSP<0>\nnamespace B = DSP<99>\nparams:\n  a = A::Selected\n  b = B::Selected\nsample:\n  out1 = i32(a) + i32(b)\n");
    assert_eq!(program.params[0].default.to_f64(), 0.0);
    assert_eq!(program.params[1].default.to_f64(), 1.0);
    assert_ne!(program.params[0].ty, program.params[1].ty);
}

#[test]
fn unused_template_enum_bodies_are_type_checked_without_evaluation() {
    compile("namespace DSP<N = 0>:\n  enum Mode:\n    A\n    B\n  const def select() -> Mode:\n    return Mode(1 / N)\n  const Selected: Mode = select()\nsample:\n  out1 = 0.0\n");
    for declaration in [
        "const Invalid: Mode = Mode(0.5)",
        "const def invalid(value: Mode) -> Mode:\n    return 0",
        "enum Other:\n    A\n  const Invalid: Mode = Other::A",
    ] {
        let source = format!("namespace DSP<N = 2>:\n  enum Mode:\n    A\n    B\n  {declaration}\nsample:\n  out1 = 0.0\n");
        assert!(
            analyze(parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
}
