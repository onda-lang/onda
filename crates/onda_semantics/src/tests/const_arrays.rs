use super::*;

fn assert_no_const_payload(source: &str) {
    let typed = analyze_source(source);
    assert!(typed.const_arrays.is_empty(), "{source}");
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn configuration_arrays_are_demanded_by_values_and_inspection() {
    for ty in ["i32[1]", "i32[]"] {
        for initializer in ["[1 / 0]", "build()"] {
            let declarations = format!("const def build() -> i32[1]:\n  return [1 / 0]\nconfig const Table: {ty} = {initializer}\n");
            for expression in ["0.0", "f32(Table.len())"] {
                assert_no_const_payload(&format!("{declarations}sample:\n  out1 = {expression}\n"));
            }
            let source = format!("{declarations}sample:\n  out1 = f32(Table[0])\n");
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("division by zero")),
                "{errors:?}"
            );
            let program = parse_program(&format!("{declarations}sample:\n  out1 = 0.0\n")).unwrap();
            let errors = inspect_compile_constants(
                program,
                AnalysisOptions::default(),
                &CompileInputs::default(),
            )
            .unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("division by zero")),
                "{errors:?}"
            );
        }
    }
}

#[test]
fn configuration_inspection_resolves_defaults_and_selected_array_values() {
    let source = "config const Table: i32[2] = [7, 9]\nconfig const Alias: i32[] = Table\nsample:\n  out1 = 0.0\n";
    assert_no_const_payload(source);
    for selected in [
        None,
        Some(vec![TypedConstValue::I32(11), TypedConstValue::I32(13)]),
    ] {
        let overridden = selected.is_some();
        let value = ConstValue::Array {
            elem_ty: PrimitiveType::I32,
            len: 2,
            values: selected
                .unwrap_or_else(|| vec![TypedConstValue::I32(7), TypedConstValue::I32(9)]),
        };
        let mut inputs = CompileInputs::default();
        if overridden {
            inputs.constants.insert("Table".to_owned(), value.clone());
        }
        let program = parse_program(source).unwrap();
        let typed =
            analyze_with_options_and_inputs(program.clone(), AnalysisOptions::default(), &inputs)
                .unwrap();
        assert!(typed.const_arrays.is_empty());
        let descriptors =
            inspect_compile_constants(program, AnalysisOptions::default(), &inputs).unwrap();
        assert_eq!(descriptors.len(), 2);
        assert!(descriptors
            .iter()
            .all(|descriptor| descriptor.value == value));
    }
}

#[test]
fn unused_configuration_arrays_still_check_known_shapes() {
    let source = "config const Table: i32[2] = [1 / 0]\nsample:\n  out1 = 0.0\n";
    let errors = analyze(parse_program(source).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expects 2 elements, got 1")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("division by zero")),
        "{errors:?}"
    );
}

#[test]
fn compile_time_array_intermediates_are_not_retained() {
    let prefix = "const def build() -> i32[131072]:\n  values: i32[131072]\n  values[0] = 7\n  return values\nconst Table = build()\nconst Alias = Table\n";
    for consumer in [
        "sample:\n  out1 = f32(Alias[0])\n",
        "const First = Alias[0]\nsample:\n  out1 = f32(First)\n",
        "def read() -> i32:\n  return Alias[0]\nsample:\n  out1 = f32(read())\n",
        "const Count = Alias[0]\ninit:\n  values: i32[Count]\nsample:\n  out1 = f32(values.len())\n",
    ] {
        assert_no_const_payload(&format!("{prefix}{consumer}"));
    }
    let typed = analyze_source(&format!(
        "{prefix}params:\n  index: i32 = 0\nsample:\n  out1 = f32(Alias[index])\n"
    ));
    assert_eq!(typed.const_arrays.len(), 1);
    assert_eq!(typed.const_arrays[0].name, "Alias");
    assert_eq!(typed.const_arrays[0].values[0], TypedConstValue::I32(7));
    assert_eq!(
        lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .len(),
        1
    );
}

#[test]
fn runtime_parameters_cannot_supply_compile_time_dimensions() {
    let source = "const Count: i32 = 2\ndef read(Count: i32) -> i32:\n  values: i32[Count]\n  return values.len()\nsample:\n  out1 = f32(read(3))\n";
    let errors = analyze(parse_program(source).unwrap()).unwrap_err();
    assert!(
        errors.iter().any(|error| error.message.contains("Count")),
        "{errors:?}"
    );
}

#[test]
fn required_dimensions_can_use_const_array_metadata_without_payloads() {
    let source = "const Table: i32[2] = [1 / 0, 0]\nconst def size(xs: i32[]) -> i32:\n  return xs.len()\nproc Voice:\n  sample 2:\n    values: i32[size(Table)]\n    out1 = f32(values.len())\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
    assert_no_const_payload(source);
}

#[test]
fn indexed_processor_events_preserve_overridden_defaults() {
    let source = r#"
const Broken: i32[1] = [1 / 0]
proc Inner:
  init:
    cached = 0.0
  event reset(value: f32 = f32(Broken[0])):
    cached = value
  sample:
    out1 = cached
proc Voice:
  init:
    inner = Inner()
  event trigger(value: f32):
    inner.reset(value)
  sample:
    out1 = inner()
def send(voices, index: i32, value: f32):
  voices[index].trigger(value)
init:
  voices: Voice[2] = Voice()
event Trigger():
  send(voices, 1, 7.0)
sample:
  out1 = voices[1]()
"#;
    assert_no_const_payload(source);
}

#[test]
fn indexed_processor_parameter_reads_skip_unused_step_values() {
    let source = r#"
const Broken: i32[1] = [1 / 0]
proc Voice:
  params:
    gain = 7.0
  sample:
    if Broken[0] > 0:
      out1 = gain
    out1 = 0.0
def read(voices, index: i32) -> f32:
  return voices[index].gain
init:
  voices: Voice[2] = Voice()
sample:
  out1 = read(voices, 1)
"#;
    assert_no_const_payload(source);
}
