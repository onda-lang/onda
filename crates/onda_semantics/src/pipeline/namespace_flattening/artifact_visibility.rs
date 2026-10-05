use super::*;

#[test]
fn repeated_namespace_references_import_only_resolved_declarations() {
    let mut source = String::from("namespace Library<N = 2>:\n");
    for index in 0..128 {
        source.push_str(&format!("  const C{index}: i32 = N + {index}\n"));
    }
    source.push_str("namespace Wrapper<N = 2>:\n  const Value: i32 = Library<N>::C0\nsample:\n");
    for _ in 0..256 {
        source.push_str("  out1 = f32(Wrapper<7>::Value + Library<7>::C1)\n");
    }
    let program = onda_frontend::parse_program(&source).unwrap();
    let mut state = NamespaceFlattenState::default();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for block in program.blocks {
        process_top_level_block(
            block,
            &HashMap::new(),
            AnalysisOptions::default(),
            &mut state,
            &mut out,
            &mut errors,
        );
    }
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(state.instantiations.len(), 2);
    assert_eq!(
        state.specialization_artifacts.const_values.keys().count(),
        129
    );
    // Wrapper captures C0 in its own scope. The caller sees only Value and C1,
    // regardless of how many members Library has or how often it is referenced.
    assert_eq!(state.artifacts.const_values.keys().count(), 2);
}

#[test]
fn templates_capture_only_referenced_declarations_in_a_growing_catalog() {
    let mut source = String::new();
    for index in 0..128 {
        source.push_str(&format!("const C{index}: i32 = {index}\nnamespace Library{index}<N = C{index}>:\n  const Value: i32 = N\n  def read() -> i32:\n    return C{index}\n"));
    }
    let program = onda_frontend::parse_program(&source).unwrap();
    let mut state = NamespaceFlattenState::default();
    let mut errors = Vec::new();
    for block in program.blocks {
        process_top_level_block(
            block,
            &HashMap::new(),
            AnalysisOptions::default(),
            &mut state,
            &mut Vec::new(),
            &mut errors,
        );
    }
    assert!(errors.is_empty(), "{errors:?}");
    for index in 0..128 {
        let template = &state.templates[&format!("Library{index}")];
        assert_eq!(template.captured_artifacts.const_values.keys().count(), 1);
        assert!(template.captured_artifacts.contains(&format!("C{index}")));
        for unrelated in 0..index {
            assert!(!template.const_metadata.contains(&format!("C{unrelated}")));
        }
    }
}

#[test]
fn namespace_captures_preserve_runtime_and_type_metadata_references() {
    let source = r#"
const Count: i32 = 2
const Table: i32[2] = [7, 9]
const def size() -> i32:
  return Count
namespace Shape<N = 2>:
  struct Value:
    data: i32[N]
namespace Library<N = Count>:
  namespace S = Shape<size()>
  struct Holder:
    value: i32 = Count
    nested: S::Value
  def read(xs: i32[size()] = Table) -> i32:
    return xs[0]
  proc Voice:
    params:
      gain: i32 = Count
    init:
      values: i32[size()] = Table
    sample:
      out1 = f32(values[0] + gain)
init:
  voice = Library<3>::Voice()
sample:
  out1 = voice() + f32(Library<3>::read())
"#;
    let mut program = onda_frontend::parse_program(source).unwrap();
    let artifacts =
        flatten_namespaces_for_semantics(&mut program, AnalysisOptions::default()).unwrap();
    assert!(artifacts.contains("Table"));
    assert!(crate::analyze(onda_frontend::parse_program(source).unwrap()).is_ok());
}

#[test]
fn generic_type_bindings_do_not_capture_same_named_global_constants() {
    let source = "const Value: bool = true\nnamespace Base:\n  const Value: i32 = 7\nnamespace Library<N = 2>:\n  use Base\n  def read<Value>(value: Value) -> Value:\n    copy: Value = value\n    return copy\n  struct Holder<Value>:\n    value: Value\nsample:\n  out1 = 7.0\n";
    let mut program = onda_frontend::parse_program(source).unwrap();
    assert!(flatten_namespaces_for_semantics(&mut program, AnalysisOptions::default()).is_ok());
}
