use super::*;

#[test]
fn block_storage_and_sample_owned_processors_compose() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-block-sample-owned-hooks.onda"
    );
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("scoped processor initialization preserves the retained block region");
}

#[test]
fn forwarded_and_indexed_processor_aliases_compose() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-forwarded-alias-joins.onda"
    );
    for source in [
        source.to_owned(),
        source.replace(
            "selected = forwarded\n  else:\n    selected = right.voices[0]",
            "selected = right.voices[0]\n  else:\n    selected = forwarded",
        ),
        source.replace("forwarded = first", "second = first\n  forwarded = second"),
        source.replace("left: Bank, right: Bank", "left, right"),
    ] {
        lower_program_to_optimized_mir(&analyze_source(&source))
            .expect("direct and forwarded selections retain the same nominal processor type");
    }
}

#[test]
fn processor_array_parameters_support_slicing_and_forwarded_subslices() {
    for parameter in ["voices", "voices: Voice[3]", "voices: Voice[]"] {
        for (declaration, argument) in [
            ("voices: Voice[3] = Voice()", "voices"),
            ("bank = Bank()", "bank.voices"),
        ] {
            let source = format!(
                r#"
proc Voice:
  sample:
    out1 = 1.0
proc Bank:
  init:
    voices: Voice[3] = Voice()
def step(voices: Voice[]) -> f32:
  return voices[0]()
def select({parameter}) -> f32:
  window = voices[1:3]
  tail = window[1:]
  return step(tail)
init:
  {declaration}
sample:
  out1 = select({argument})
"#
            );
            lower_program_to_optimized_mir(&analyze_source(&source))
                .expect("whole processor-array parameters support ordinary slice composition");
        }
    }
}

#[test]
fn runtime_owned_processor_copies_keep_resource_environments() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-runtime-owned-resources.onda"
    );
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("runtime declarations retain resources independently of planned storage");
}

#[test]
fn owned_processor_arrays_retain_resource_environments() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-array-resources.onda"
    );
    let direct = source.replace("forward(copies, index)", "step(copies[index])");
    let elements = source.replace(
        "copies: Voice[2] = bank.voices",
        "copies: Voice[2] = [bank.voices[1], bank.voices[0]]",
    );
    let repeated_elements = source.replace(
        "copies: Voice[2] = bank.voices",
        "copies: Voice[2] = [bank.voices[0], bank.voices[0]]",
    );
    let nested_owner = source
        .replace("bank = Bank(", "banks: Bank[2] = Bank(")
        .replace("exercise(bank, index)", "exercise(banks[index], index)")
        .replace("f32(bank.voices[index].count)", "0.0");
    let parameter = source
        .replace("exercise(bank: Bank,", "exercise(voices: Voice[2],")
        .replace("= bank.voices", "= voices")
        .replace("exercise(bank, index)", "exercise(bank.voices, index)");
    let broadcast = source.replace("= bank.voices\n", "= bank.voices[1]\n");
    let mapped = parameter
        .replace(
            "bank = Bank(left = left, right = right)",
            "voices: Voice[2] = [Voice(clip = left), Voice(clip = right)]",
        )
        .replace("bank.voices", "voices")
        .replace("f32(voices[index].count)", "voices[index].out1");
    let repeated = source.replace(
        "  copies: Voice[2] = bank.voices",
        "  first_copy: Voice[2] = bank.voices\n  copies: Voice[2] = first_copy",
    );
    let joined = source
        .replace(
            "exercise(bank: Bank, index: i32)",
            "exercise(bank: Bank, other: Bank, index: i32)",
        )
        .replace(
            "  copies: Voice[2] = bank.voices",
            "  if index == 0:\n    selected = bank.voices\n  else:\n    selected = other.voices\n  copies: Voice[2] = selected",
        )
        .replace("exercise(bank, index)", "exercise(bank, bank, index)");
    for source in [
        source,
        &direct,
        &elements,
        &repeated_elements,
        &nested_owner,
        &parameter,
        &broadcast,
        &mapped,
        &repeated,
        &joined,
    ] {
        lower_program_to_optimized_mir(&analyze_source(source))
            .expect("owned array copies retain selected resource bindings through forwarding");
    }
}

#[test]
fn processor_element_copies_keep_deferred_resources() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-element-resources.onda"
    );
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("element copies preserve their selected resource environments");
}

#[test]
fn unused_scalar_child_hooks_do_not_demand_constant_payloads() {
    let source = r#"
const Table: i32[1] = [1 / 0]
proc Voice:
  block:
    sample:
      out1 = 0.0
    print(Table[0])
proc Bank:
  init:
    voice = Voice()
  sample:
    out1 = 0.0
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let ignored_view = source
        .replace(
            "proc Bank:",
            "def ignore(voice) -> f32:\n  return 0.0\nproc Bank:",
        )
        .replace("    out1 = 0.0\ninit:", "    out1 = ignore(voice)\ninit:");
    for source in [source, &ignored_view] {
        lower_program_to_optimized_mir(&analyze_source(source))
            .expect("an unstepped scalar child does not activate its post hook");
    }
}

#[test]
fn unused_processor_array_hooks_do_not_demand_constant_payloads() {
    let source = r#"
const Table: i32[1] = [1 / 0]
proc Voice:
  block:
    sample:
      out1 = 0.0
    print(Table[0])
proc Bank:
  init:
    voices: Voice[2] = Voice()
  sample:
    out1 = 0.0
init:
  unused = Bank()
sample:
  out1 = 0.0
"#;
    let forwarded = source
        .replace(
            "proc Bank:",
            "def step(voice) -> f32:\n  return voice()\ndef forward(voice) -> f32:\n  return step(voice)\nproc Bank:",
        )
        .replace("    out1 = 0.0\ninit:", "    out1 = forward(voices[0])\ninit:");
    for source in [source, &forwarded] {
        lower_program_to_optimized_mir(&analyze_source(source))
            .expect("unused processor lifecycle operations remain unreachable");
    }
    let active = forwarded.replace(
        "\nsample:\n  out1 = 0.0\n",
        "\nsample:\n  out1 = forward(unused.voices[0])\n",
    );
    let errors = analyze(parse_program(&active).unwrap()).unwrap_err();
    assert!(errors
        .iter()
        .any(|error| error.message.contains("division by zero")));
}

#[test]
fn unused_borrowed_processor_helpers_do_not_schedule_boundary_sweeps() {
    let source = r#"
proc Voice:
  block:
    sample:
      out1 = 0.0
    print(1)
def step(voice) -> f32:
  return voice()
def forward(voice) -> f32:
  return step(voice)
proc Bank:
  init:
    voices: Voice[128] = Voice()
  sample:
    out1 = forward(voices[0])
init:
  unused = Bank()
sample:
  out1 = 0.0
"#;
    let mir = lower_program_to_optimized_mir(&analyze_source(source)).unwrap();
    assert!(
        mir.log_sites.is_empty(),
        "unused post hooks remain unreachable"
    );
    let mut blocks = vec![&mir.functions[mir.entry_points.process.index()].body];
    while let Some(block) = blocks.pop() {
        for statement in &block.statements {
            match &statement.kind {
                onda_mir::StatementKind::SliceFill { .. }
                | onda_mir::StatementKind::Assign {
                    value: onda_mir::Rvalue::SliceLoad { .. },
                    ..
                } => panic!("unused processors must not clear or scan activation slots"),
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
}

#[test]
fn processor_view_array_aliases_and_joined_calls_lower() {
    let arrays = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-array-aliases.onda"
    );
    let selected = arrays
        .replace("selected = left.voices", "selected = left.voices[0]")
        .replace("selected = right.voices", "selected = right.voices[1]")
        .replace("return step(selected, 1)", "return selected()");
    for source in [arrays, &selected] {
        lower_program_to_optimized_mir(&analyze_source(source))
            .expect("aliases retain nominal processor operations and captured owners");
    }
    let copied = selected.replace("return selected()", "copy = selected\n  return copy()");
    lower_program_to_optimized_mir(&analyze_source(&copied))
        .expect("copying a joined alias retains its nominal callable surface");
}

#[test]
fn primitive_array_reads_do_not_capture_processor_buffers() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-resource-shadowing.onda"
    );
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("primitive array state leaves lexical buffer shadowing intact");
}

#[test]
fn borrowed_children_keep_hygienic_lexical_resource_captures() {
    let source = r#"
proc Voice:
  buffers:
    source: f32
  sample:
    out1 = source[0]
def step(voice) -> f32:
  return voice()
proc Reader:
  buffers:
    source: f32
  init:
    voice = Voice(source = source)
  def select() -> f32:
    source: f32 = 2.0
    return step(voice) + source
  sample:
    out1 = select()
buffers:
  clip: f32
init:
  reader = Reader(source = clip)
sample:
  out1 = reader()
"#;
    let tuple = source.replace("source: f32 = 2.0", "source, _ = (2.0, 3.0)");
    for source in [source, &tuple] {
        lower_program_to_optimized_mir(&analyze_source(source))
            .expect("resource parameters do not collide with a helper's local declarations");
    }
}

#[test]
fn indexed_processor_calls_accept_buffer_collection_elements() {
    let source = r#"
proc Voice:
  buffers:
    source: f32
  sample:
    out1 = source[0]
buffers:
  clips: f32 {4}
init:
  voices: Voice[2] = [Voice(source = clips[1]), Voice(source = clips[3])]
  index: i32 = 1
sample:
  out1 = voices[0]() + voices[index]()
"#;
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("direct processor calls preserve selected buffer bindings");
}

#[test]
fn indexed_processor_buffer_elements_preserve_owner_callbacks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-indexed-buffer-callbacks.onda"
    );
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("callback wrappers select indexed resources through typed descriptors");
}

const SCALAR_CHILD: &str = r#"
proc Voice:
  sample:
    out1 = 1.0
proc Bank:
  init:
    voice = Voice()
  sample:
    out1 = voice()
"#;

#[test]
fn top_level_scalar_children_forward_from_init_aliases_and_sample_arguments() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-arguments.onda"
    );
    let typed = analyze_source(source);
    let bank = typed.aggregate_layouts.layout_for_struct("Bank").unwrap();
    assert!(!bank
        .leaves
        .iter()
        .any(|leaf| leaf.storage_path.starts_with("voice.")));
    lower_program_to_optimized_mir(&typed)
        .expect("top-level child arguments use logical types and borrow physical storage");
}

#[test]
fn borrowed_processor_resource_selectors_remap_enclosing_array_fields() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owner-array-selectors.onda"
    );
    let scalar = source
        .replace("voices: Voice[2] =", "voice =")
        .replace("bank.voices[0]", "bank.voice");
    for source in [source, &scalar] {
        lower_program_to_optimized_mir(&analyze_source(source))
            .expect("owner array selectors resolve against the captured owner");
    }
}

#[test]
fn joined_processor_views_keep_branch_local_owned_roots() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-local-owners.onda"
    );
    lower_program_to_optimized_mir(&analyze_source(source))
        .expect("joined child views retain the selected owned storage");
}

#[test]
fn joined_processor_views_keep_distinct_branch_local_owner_types() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-distinct-local-owners.onda"
    );
    let arrays = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-distinct-local-array-owners.onda"
    );
    for source in [
        source.to_owned(),
        source.replace("step(selected)", "selected()"),
        arrays.to_owned(),
    ] {
        lower_program_to_optimized_mir(&analyze_source(&source))
            .expect("joined local owners forward storage without reading inactive fields");
    }
}

#[test]
fn processor_local_helpers_capture_resources_transitively_and_respect_shadowing() {
    let typed = analyze_source(
        r#"
proc Voice:
  buffers:
    source: f32
  sample:
    out1 = source[0]
def step(voice) -> f32:
  return voice()
proc Bank:
  buffers:
    source: f32
  init:
    voice = Voice(source = source)
  def outer() -> f32:
    return middle()
  def middle() -> f32:
    return inner()
  def inner() -> f32:
    return step(voice)
  def shadow(voice: f32) -> f32:
    return voice
  sample:
    out1 = outer() + shadow(1.0)
buffers:
  clip: f32
init:
  bank = Bank(source = clip)
sample:
  out1 = bank()
"#,
    );
    for (name, captures) in [
        ("outer", true),
        ("middle", true),
        ("inner", true),
        ("shadow", false),
    ] {
        let helper = typed.defs.iter().find(|def| {
            typed.processor_views.operation(&def.name).is_some_and(|operation| {
                operation.projection.owner == "Bank"
                    && operation.projection.edges.is_empty()
                    && matches!(&operation.kind, crate::processor_views::ProcessorOperation::Local(local) if local == name)
            })
        }).unwrap();
        assert_eq!(
            helper
                .param_kinds
                .iter()
                .any(|kind| matches!(kind, TypedFnParam::Buffer { .. })),
            captures
        );
    }
    lower_program_to_optimized_mir(&typed).expect("transitive resource signatures match calls");
}

#[test]
fn borrowed_scalar_child_calls_write_the_owner() {
    let source = format!(
        r#"
{SCALAR_CHILD}
def step(bank: Bank) -> f32:
  return bank.voice()
init:
  bank = Bank()
sample:
  out1 = step(bank)
"#
    );
    let typed = analyze_source(&source);
    let step = typed.defs.iter().find(|def| def.name == "step").unwrap();
    assert!(!step.readonly_data_params.contains("bank"));
    lower_program_to_optimized_mir(&typed)
        .expect("scalar child calls retain writable owner storage");
}

#[test]
fn borrowed_scalar_child_cached_reads_remain_read_only() {
    let source = format!(
        r#"
{SCALAR_CHILD}
def cached(bank: Bank) -> f32:
  return bank.voice.out1
init:
  bank = Bank()
sample:
  bank()
  out1 = cached(bank)
"#
    );
    let typed = analyze_source(&source);
    let cached = typed.defs.iter().find(|def| def.name == "cached").unwrap();
    assert!(cached.readonly_data_params.contains("bank"));
    lower_program_to_optimized_mir(&typed)
        .expect("cached scalar child fields borrow the owner's existing storage");
}

#[test]
fn processor_permissions_follow_forwarding_chains_in_either_definition_order() {
    for readonly in [false, true] {
        for reverse in [false, true] {
            let mut source = SCALAR_CHILD.to_owned();
            let mut indices = (0..64).collect::<Vec<_>>();
            if reverse {
                indices.reverse();
            }
            for index in indices {
                let expression = if index < 63 {
                    format!("forward_{}(bank)", index + 1)
                } else if readonly {
                    "bank.voice.out1".to_owned()
                } else {
                    "bank.voice()".to_owned()
                };
                source.push_str(&format!(
                    "\ndef forward_{index}(bank: Bank) -> f32:\n  return {expression}\n"
                ));
            }
            source.push_str("init:\n  bank = Bank()\nsample:\n  out1 = forward_0(bank)\n");
            let typed = analyze_source(&source);
            let helpers = typed
                .defs
                .iter()
                .filter(|def| def.name.starts_with("forward_"))
                .collect::<Vec<_>>();
            assert_eq!(helpers.len(), 64);
            for helper in helpers {
                assert_eq!(
                    helper.readonly_data_params.contains("bank"),
                    readonly,
                    "{}, reverse={reverse}",
                    helper.name
                );
            }
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn processor_permission_cycles_converge_before_recursion_is_rejected() {
    let source = format!(
        "{SCALAR_CHILD}\ndef first(bank: Bank) -> f32:\n  return second(bank)\ndef second(bank: Bank) -> f32:\n  bank.voice()\n  return first(bank)\ninit:\n  bank = Bank()\nsample:\n  out1 = first(bank)\n"
    );
    let errors = analyze(parse_program(&source).unwrap()).expect_err("recursive runtime calls");
    assert!(errors.iter().any(|error| error
        .message
        .contains("recursive runtime def cycle is not realtime-safe: first -> second -> first")));
}
