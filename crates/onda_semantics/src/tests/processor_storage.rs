use super::*;

fn mir_statement_count(program: &onda_mir::Program) -> usize {
    let mut blocks = program
        .functions
        .iter()
        .map(|function| &function.body)
        .collect::<Vec<_>>();
    let mut count = 0;
    while let Some(block) = blocks.pop() {
        count += block.statements.len();
        for statement in &block.statements {
            match &statement.kind {
                onda_mir::StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    blocks.extend([then_block, else_block]);
                }
                onda_mir::StatementKind::Loop { body } => blocks.push(body),
                _ => {}
            }
        }
    }
    count
}

#[test]
fn whole_array_constructor_arguments_keep_capacity_independent_initialization() {
    for (width, values) in [(1, "[0.25]"), (2, "[0.25, 0.75]")] {
        for binding in [values, "gains"] {
            for nested in [false, true] {
                let count = |capacity| {
                    let owner = format!(
                    "init:\n  gains: f32[{width}] = {values}\n  voices: Voice[{capacity}] = Voice(gains = {binding})\nsample:\n  out1 = voices[0]()\n"
                );
                    let source = format!(
                    "proc Voice:\n  params:\n    gains: f32[{width}] = {values}\n  sample:\n    out1 = gains[0]\n{}",
                    if nested {
                        format!("proc Bank:\n{}init:\n  bank = Bank()\nsample:\n  out1 = bank()\n", owner.lines().map(|line| format!("  {line}\n")).collect::<String>())
                    } else {
                        owner
                    }
                );
                    mir_statement_count(
                        &lower_program_to_optimized_mir(&analyze_source(&source)).unwrap(),
                    )
                };
                assert_eq!(count(8), count(256), "{binding}, nested={nested}");
            }
        }
    }
}

#[test]
fn whole_buffer_collection_constructor_arguments_keep_capacity_independent_initialization() {
    for nested in [false, true] {
        let count = |capacity| {
            let source = format!(
                "proc Voice:\n  buffers:\n    clips: f32 {{2}}\n  sample:\n    out1 = clips[0][0] + clips[1][0]\n{}",
                if nested {
                    format!("proc Bank:\n  buffers:\n    clips: f32 {{2}}\n  init:\n    voices: Voice[{capacity}] = Voice(clips = clips)\n  sample:\n    out1 = voices[0]()\nbuffers:\n  clips: f32 {{2}}\ninit:\n  bank = Bank(clips = clips)\nsample:\n  out1 = bank()\n")
                } else {
                    format!("buffers:\n  clips: f32 {{2}}\ninit:\n  voices: Voice[{capacity}] = Voice(clips = clips)\nsample:\n  out1 = voices[0]()\n")
                }
            );
            mir_statement_count(&lower_program_to_optimized_mir(&analyze_source(&source)).unwrap())
        };
        assert_eq!(count(8), count(256), "nested={nested}");
    }
}

#[test]
fn repeated_processor_capture_joins_scale_with_distinct_owners() {
    let count = |depth| {
        let mut source = String::from(
            "proc Voice:\n  delegate fired(value: i32)\n  sample:\n    fired(1)\n    out1 = 1.0\nproc Bank:\n  init:\n    voices: Voice[2] = Voice()\n    count: i32 = 0\n  when voices[0].fired(value):\n    count += value\n  sample:\n    out1 = f32(count)\ndef exercise(left: Bank, right: Bank, choose: bool) -> f32:\n  if choose:\n    a0 = left.voices[0]\n    b0 = right.voices[1]\n  else:\n    a0 = right.voices[1]\n    b0 = left.voices[0]\n",
        );
        for index in 1..=depth {
            let previous = index - 1;
            source.push_str(&format!(
                "  if choose:\n    a{index} = a{previous}\n    b{index} = b{previous}\n  else:\n    a{index} = b{previous}\n    b{index} = a{previous}\n"
            ));
        }
        source.push_str(&format!("  return a{depth}()\ninit:\n  left = Bank()\n  right = Bank()\n  choose = true\nsample:\n  out1 = exercise(left, right, choose)\n  choose = !choose\n"));
        let mir = lower_program_to_optimized_mir(&analyze_source(&source)).unwrap();
        mir_statement_count(&mir)
    };
    let small = count(3);
    let large = count(7);
    assert!(
        large < small * 3,
        "capture choices must stay compact: {small} -> {large}"
    );
}

#[test]
fn processor_array_hook_initialization_has_capacity_independent_mir() {
    let count = |capacity| {
        let source = format!(
            "proc Voice:\n  init:\n    count = 0.0\n  block:\n    count += 1.0\n    sample:\n      out1 = count\n    count += 10.0\ninit:\n  voices: Voice[{capacity}] = Voice()\nsample:\n  out1 = voices[0]()\n"
        );
        let mir = lower_program_to_optimized_mir(&analyze_source(&source)).unwrap();
        mir_statement_count(&mir)
    };
    assert_eq!(
        count(16),
        count(1024),
        "hook initialization must not expand per slot"
    );
}

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
