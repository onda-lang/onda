use super::*;

fn lower_array_declarations(capacity: usize) -> ProcessorDesugarResult {
    let source = format!(
        "proc Voice:\n  init:\n    phase = 1.0\n  sample:\n    out1 = phase\n\n\
         proc Bank:\n  init:\n    scalar = Voice()\n    voices: Voice[{capacity}] = Voice()\n  sample:\n    out1 = voices[2]() + scalar()\n\n\
         init:\n  bank = Bank()\n  voices: Voice[{capacity}] = Voice()\n\n\
         sample:\n  out1 = bank() + voices[3]()\n"
    );
    let program = onda_frontend::parse_program(&source).expect("parse processor declarations");
    let inputs = crate::pipeline::preprocess_const_semantics_for_lowering(
        program,
        AnalysisOptions::default(),
    )
    .expect("resolve declaration constants");
    let mut errors = Vec::new();
    let lowered = desugar_processors(
        inputs.program,
        AnalysisOptions::default(),
        &inputs.arrays,
        &inputs.scalars,
        &inputs.scope,
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
    lowered
}

#[test]
fn uniform_arrays_retain_declarations_instead_of_element_registries() {
    let small = lower_array_declarations(4);
    let large = lower_array_declarations(4096);
    for (lowered, capacity) in [(&small, 4), (&large, 4096)] {
        let bank = &lowered.lowering_shapes["Bank"];
        assert_eq!(bank.state.nested_procs.len(), 1);
        assert!(bank.state.nested_procs.contains_key("scalar"));
        let array = &bank.nested_proc_array_slots["voices"];
        assert_eq!(array.len, capacity);
        assert_eq!(array.binding_names().len(), 1);
        assert_eq!(
            lowered.top_level_proc_rewrite.global_proc_instances.len(),
            2
        );
        assert_eq!(
            lowered.top_level_proc_rewrite.global_proc_array_slots["voices"]
                .binding_names()
                .len(),
            1
        );
    }
    assert_eq!(
        small.processor_views.operations.len(),
        large.processor_views.operations.len()
    );
    assert_eq!(small.program.blocks.len(), large.program.blocks.len());
    assert_eq!(
        small.lowering_shapes["Bank"].fields.len(),
        large.lowering_shapes["Bank"].fields.len()
    );
}

#[test]
fn borrowed_owner_array_operations_keep_executable_targets() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-owner-arrays.onda"
    );
    let typed = crate::analyze(onda_frontend::parse_program(source).unwrap()).unwrap();
    crate::lower_program_to_validated_mir(&typed).unwrap();
}

#[test]
fn scalar_child_views_keep_logical_identity_through_nested_owners() {
    let source = r#"
proc Voice:
  sample:
    out1 = 1.0
def step(voice) -> f32:
  return voice()
proc Bank:
  init:
    voice = Voice()
  sample:
    selected = voice
    out1 = step(selected)
proc Rack:
  init:
    bank = Bank()
  sample:
    selected = bank.voice
    out1 = bank() + step(selected)
init:
  rack = Rack()
sample:
  out1 = rack()
"#;
    let typed = crate::analyze(onda_frontend::parse_program(source).unwrap()).unwrap();
    crate::lower_program_to_validated_mir(&typed).unwrap();
}
