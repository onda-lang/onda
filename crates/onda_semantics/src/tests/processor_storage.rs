use super::*;

fn bank_source(voices: usize) -> String {
    format!(
        r#"
proc Voice:
  params:
    frequency = 110.0
  init:
    phase = 0.0
    history: f32[4]
  sample:
    phase += frequency / SR
    history[0] = phase
    out1 = history[0]

proc Bank:
  init:
    voices: Voice[{voices}]
  sample:
    total = 0.0
    for i in 0..{voices}:
      total += voices[i]()
    out1 = total

init:
  bank = Bank()
sample:
  out1 = bank()
"#
    )
}

#[test]
fn nested_processor_array_keeps_canonical_tensor_extents() {
    let typed = analyze_source(&bank_source(128));
    let bank = typed
        .aggregate_layouts
        .layout_for_struct("Bank")
        .expect("bank layout");
    let phase = bank
        .leaves
        .iter()
        .find(|leaf| leaf.storage_path == "voices.phase")
        .expect("one phase tensor for all voices");
    assert_eq!(phase.tensor.shape, [128]);
    let history = bank
        .leaves
        .iter()
        .find(|leaf| leaf.storage_path == "voices.history")
        .expect("one history tensor for all voices");
    assert_eq!(history.tensor.shape, [128, 4]);
    lower_program_to_optimized_mir(&typed).expect("nested tensor references lower");
}

#[test]
fn nested_processor_array_mir_does_not_expand_calls_per_slot() {
    let lower = |voices| {
        lower_program_to_optimized_mir(&analyze_source(&bank_source(voices)))
            .expect("processor bank lowers")
    };
    let small = lower(2);
    let large = lower(128);
    let count = |program: &onda_mir::Program| {
        fn block_calls(block: &onda_mir::Block) -> usize {
            block
                .statements
                .iter()
                .map(|statement| match &statement.kind {
                    onda_mir::StatementKind::Call { .. } => 1,
                    onda_mir::StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => block_calls(then_block) + block_calls(else_block),
                    onda_mir::StatementKind::Loop { body } => block_calls(body),
                    _ => 0,
                })
                .sum::<usize>()
        }
        program
            .functions
            .iter()
            .filter(|function| function.name.contains("Bank") && function.name.contains("step"))
            .map(|function| block_calls(&function.body))
            .sum::<usize>()
    };
    assert_eq!(
        count(&small),
        count(&large),
        "bank processing calls scale with source, not capacity"
    );
    assert!(large
        .functions
        .iter()
        .all(|function| function.params.len() < 32));
}

#[test]
fn multiply_nested_processor_arrays_compose_tensor_axes() {
    let source = bank_source(16).replace(
        "init:\n  bank = Bank()\nsample:\n  out1 = bank()",
        "proc Rack:\n  init:\n    banks: Bank[8]\n  sample:\n    out1 = banks[3]()\ninit:\n  rack = Rack()\nsample:\n  out1 = rack()",
    );
    let typed = analyze_source(&source);
    let rack = typed
        .aggregate_layouts
        .layout_for_struct("Rack")
        .expect("rack layout");
    let history = rack
        .leaves
        .iter()
        .find(|leaf| leaf.storage_path == "banks.voices.history")
        .expect("nested history tensor");
    assert_eq!(history.tensor.shape, [8, 16, 4]);
    lower_program_to_optimized_mir(&typed).expect("nested bank references lower");
}
