use super::*;

#[test]
fn buffer_count_annotation_context_only_completes_count() {
    let source = "buffers:\n  bank: f32 {";
    let context = CompletionContext::from_source(source, source.len());
    assert!(matches!(
        context.kind,
        CompletionContextKind::BufferCount(BufferCountCompletionKind::Field)
    ));
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  bank: f32 {".len() as u32,
        },
        true,
    );
    assert_eq!(result.items.len(), 1, "items: {:?}", result.items);
    assert_eq!(result.items[0]["label"], "count");
}

#[test]
fn integer_binding_ranges_complete_mode_and_wrap() {
    let source = "sample:\n  a = 0 {1000, w";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  a = 0 {1000, w".len() as u32,
        },
        true,
    );
    assert!(result.items.iter().any(|item| item["label"] == "wrap"));

    let source = "sample:\n  a: i32 = 0 {0..1, m";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  a: i32 = 0 {0..1, m".len() as u32,
        },
        true,
    );
    let mode = encoded_item(&result.items, "mode");
    assert_eq!(mode["insertText"], "mode = ${1|clamp,wrap|}");

    let source = "sample:\n  a: i32 = 0 {0..16, ";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  a: i32 = 0 {0..16, ".len() as u32,
        },
        true,
    );
    assert!(result.items.iter().any(|item| item["label"] == "wrap"));
    assert!(!result.items.iter().any(|item| item["label"] == "count"));
    assert!(!result.items.iter().any(|item| item["label"] == "range"));
}

#[test]
fn input_domains_complete_domain_fields_instead_of_binding_ranges() {
    let source = "ins:\n  cutoff = 440.0 {20, 20000, sc";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  cutoff = 440.0 {20, 20000, sc".len() as u32,
        },
        true,
    );
    assert!(result.items.iter().any(|item| item["label"] == "scale"));
    assert!(!result.items.iter().any(|item| item["label"] == "mode"));
}

#[test]
fn integer_binding_range_mode_values_complete_clamp_and_wrap() {
    let source = "sample:\n  a = 0 {0..1, mode = ";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  a = 0 {0..1, mode = ".len() as u32,
        },
        true,
    );
    assert!(result.items.iter().any(|item| item["label"] == "clamp"));
    assert!(result.items.iter().any(|item| item["label"] == "wrap"));
    assert!(!result.items.iter().any(|item| item["label"] == "mode"));
}

#[test]
fn integer_binding_ranges_complete_named_domains() {
    let source = "sample:\n  a = 0 {ra";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  a = 0 {ra".len() as u32,
        },
        true,
    );
    let range = encoded_item(&result.items, "range");
    assert_eq!(range["insertText"], "range = ${1:0}${2|..,..=|}$3");
}

fn encoded_item<'a>(items: &'a [Value], label: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["label"] == label)
        .expect("completion item should be present")
}

#[test]
fn generated_completion_labels_include_lsp_placeholder() {
    assert!(is_generated_completion_label("__onda_internal"));
    assert!(is_generated_completion_label(COMPLETION_PLACEHOLDER));
    assert!(!is_generated_completion_label("visible"));
}

#[test]
fn plugin_event_helpers_are_separated_and_freeze_the_canonical_surface() {
    assert_eq!(
        PLUGIN_MIDI_EVENT_COMPLETIONS
            .iter()
            .map(|event| event.name)
            .collect::<Vec<_>>(),
        vec![
            "note_on",
            "note_off",
            "poly_pressure",
            "pitch_bend",
            "channel_pressure",
            "cc",
            "program_change",
        ]
    );
    assert_eq!(
        PLUGIN_HOST_CONTEXT_EVENT_COMPLETIONS
            .iter()
            .map(|event| event.name)
            .collect::<Vec<_>>(),
        vec![
            "transport",
            "sample_position",
            "time_position",
            "tempo",
            "musical_position",
            "bar_position",
            "time_signature",
            "loop_region",
            "render_mode",
        ]
    );

    let source = "plugin_";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 0,
            character: source.len() as u32,
        },
        true,
    );
    for group in PLUGIN_EVENT_COMPLETION_GROUPS {
        for event in group.events {
            let label = format!("{}_{}", group.label_prefix, event.name);
            let item = encoded_item(&result.items, &label);
            let insertion = item["insertText"]
                .as_str()
                .expect("snippet should have insertion text");

            assert_eq!(item["kind"], COMPLETION_ITEM_KIND_SNIPPET);
            assert_eq!(item["insertTextFormat"], INSERT_TEXT_FORMAT_SNIPPET);
            assert_eq!(
                insertion,
                format!("event {}({}):\n  $0", event.name, event.params)
            );
        }
    }
    assert!(result
        .items
        .iter()
        .all(|item| item["label"] != "plugin_note_on" && item["label"] != "vst3_midi_events"));
}

#[test]
fn plugin_event_helpers_have_plain_text_fallbacks_and_are_top_level_only() {
    let source = "plugin_";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 0,
            character: source.len() as u32,
        },
        false,
    );
    for group in PLUGIN_EVENT_COMPLETION_GROUPS {
        for event in group.events {
            let label = format!("{}_{}", group.label_prefix, event.name);
            let item = encoded_item(&result.items, &label);
            assert!(item.get("insertTextFormat").is_none());
            assert_eq!(
                item["insertText"],
                format!("event {}({}):\n  ", event.name, event.params)
            );
        }
    }

    let nested = "sample:\n  plugin_ = 0.0\n";
    let nested_result = completion_items_for_document_with_index(
        nested,
        None,
        &HashMap::new(),
        None,
        None,
        position_at(nested, "plugin_", "plugin_".len()),
        true,
    );
    assert!(
        PLUGIN_EVENT_COMPLETION_GROUPS
            .iter()
            .all(|group| group.events.iter().all(|event| {
                let label = format!("{}_{}", group.label_prefix, event.name);
                nested_result
                    .items
                    .iter()
                    .all(|item| item["label"] != label)
            })),
        "the event declaration helpers must not be offered inside a runtime block"
    );
}

#[test]
fn delegate_completions_are_labeled_and_child_members_are_subscription_only() {
    let source = "delegate finished(reason: i32)\n\nsample:\n  finished(1)\n  out1 = 0.0\n";
    let parsed = parse_program(source).expect("delegate source should parse");
    let index = CompletionIndex::build(
        Some(&parsed),
        source,
        None,
        CompletionPosition {
            line: 3,
            character: 4,
        },
    );
    let finished = index
        .general_items("fin")
        .into_iter()
        .find(|item| item.label == "finished")
        .expect("owner delegate should be completed");
    assert_eq!(
        finished.detail.as_deref(),
        Some("delegate finished(reason: i32)")
    );

    let source = "proc Child:\n  event start():\n    return\n  delegate stopped(reason: i32)\n  sample:\n    out1 = 0.0\n\ninit:\n  child = Child()\n\nwhen child.stopped(reason):\n  out1 = f32(reason)\n\nsample:\n  out1 = child()\n";
    let parsed = parse_program(source).expect("child delegate source should parse");
    let index = CompletionIndex::build(
        Some(&parsed),
        source,
        None,
        CompletionPosition {
            line: 10,
            character: 11,
        },
    );
    let members = index.member_items("child", "");
    assert!(members.iter().any(|item| {
        item.label == "stopped" && item.detail.as_deref() == Some("delegate stopped(reason: i32)")
    }));
    assert!(members.iter().all(|item| item.label != "start"));
    let body_index = CompletionIndex::build(
        Some(&parsed),
        source,
        None,
        CompletionPosition {
            line: 11,
            character: 19,
        },
    );
    let binding = body_index
        .general_items("rea")
        .into_iter()
        .find(|item| item.label == "reason")
        .expect("when payload binding should be completed in its body");
    assert_eq!(
        binding.detail.as_deref(),
        Some("delegate payload binding: i32")
    );
}

#[test]
fn incomplete_when_member_completion_lists_all_receiver_delegates() {
    let source = "proc Envelope:\n  delegate finished()\n  delegate looped(count: i32)\n  event reset():\n    return\n  sample:\n    out1 = 0.0\n\ninit:\n  env = Envelope()\n\nwhen env.\n\nsample:\n  out1 = env()\n";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        position_at(source, "when env.", "when env.".len()),
        true,
    );
    let labels = result
        .items
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(labels, BTreeSet::from(["finished", "looped"]));
    assert!(result.items.iter().all(|item| item["label"] != "reset"));
}

#[test]
fn incomplete_when_array_completion_distinguishes_array_and_element_receivers() {
    for (receiver, expected_labels, finished_detail) in [
        (
            "envs.",
            &["finished", "looped"][..],
            "delegate finished(index: i32)",
        ),
        (
            "envs.fin",
            &["finished"][..],
            "delegate finished(index: i32)",
        ),
        (
            "envs[1].",
            &["finished", "looped"][..],
            "delegate finished()",
        ),
        ("envs[1].fin", &["finished"][..], "delegate finished()"),
    ] {
        let target = format!("when {receiver}");
        let source = format!(
                "proc Envelope:\n  delegate finished()\n  delegate looped(count: i32)\n  event reset():\n    return\n  sample:\n    out1 = 0.0\n\ninit:\n  envs: Envelope[2] = Envelope()\n\n{target}\n\nsample:\n  out1 = envs[0]()\n"
            );
        let result = completion_items_for_document_with_index(
            &source,
            None,
            &HashMap::new(),
            None,
            None,
            position_at(&source, &target, target.len()),
            true,
        );
        let labels = result
            .items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            labels,
            expected_labels.iter().copied().collect(),
            "unexpected delegate completions for '{target}'"
        );
        assert_eq!(
            encoded_item(&result.items, "finished")["detail"],
            finished_detail,
            "unexpected delegate signature for '{target}'"
        );
    }
}

#[test]
fn print_completion_exposes_its_variadic_callable_shape() {
    let source = "sample:\n  pri\n";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: 5,
        },
        true,
    );
    let print = encoded_item(&result.items, "print");
    assert_eq!(print["kind"], COMPLETION_ITEM_KIND_FUNCTION);
    assert_eq!(
        print["labelDetails"]["detail"],
        "(...values: f32 | f64 | i32 | i64 | bool)"
    );
    assert_eq!(print["insertText"], "print($1)");
    assert_eq!(print["insertTextFormat"], INSERT_TEXT_FORMAT_SNIPPET);
}

#[test]
fn when_bindings_preserve_array_types_and_whole_array_index() {
    let source = r#"proc Child:
  delegate ready(values: i32[])
  sample:
    out1 = 0.0

init:
  children: Child[2] = Child()

when children.ready(index, values):
  count = values.len()

sample:
  out1 = children[0]()
"#;
    let parsed = parse_program(source).expect("handler source should parse");
    let index = CompletionIndex::build(
        Some(&parsed),
        source,
        None,
        position_at(source, "values.len", "values.".len()),
    );
    let index_binding = index
        .general_items("ind")
        .into_iter()
        .find(|item| item.label == "index")
        .expect("whole-array index binding should complete");
    assert_eq!(
        index_binding.detail.as_deref(),
        Some("delegate payload binding: i32")
    );
    let values_binding = index
        .general_items("val")
        .into_iter()
        .find(|item| item.label == "values")
        .expect("slice payload binding should complete");
    assert_eq!(
        values_binding.detail.as_deref(),
        Some("delegate payload binding: i32[]")
    );
    assert!(index
        .member_items("values", "")
        .iter()
        .any(|item| item.label == "len"));

    let target_index = CompletionIndex::build(
        Some(&parsed),
        source,
        None,
        position_at(source, "children.ready", "children.".len()),
    );
    let ready = target_index
        .member_items("children", "")
        .into_iter()
        .find(|item| item.label == "ready")
        .expect("child-array delegate should complete in a when target");
    assert_eq!(
        ready.detail.as_deref(),
        Some("delegate ready(index: i32, values: i32[])")
    );
}

fn position_at(source: &str, needle: &str, token_offset: usize) -> CompletionPosition {
    let byte = source
        .find(needle)
        .map(|idx| idx + token_offset)
        .expect("test needle should exist");
    let line = source[..byte].bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = source[..byte].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    CompletionPosition {
        line,
        character: (byte - line_start) as u32,
    }
}

fn index_at(source: &str, needle: &str, token_offset: usize) -> CompletionIndex {
    let parsed = parse_program(source).expect("test source should parse");
    CompletionIndex::build(
        Some(&parsed),
        source,
        None,
        position_at(source, needle, token_offset),
    )
}

fn labels(items: Vec<CompletionItem>) -> Vec<String> {
    items.into_iter().map(|item| item.label).collect()
}

#[test]
fn stdlib_discovery_catalog_deduplicates_symbols_without_collapsing_overloads() {
    let index = stdlib_discovery_index().expect("stdlib discovery index");

    let sine_count = index
        .symbols
        .iter()
        .filter(|symbol| symbol.full_name == "std::osc::Sine")
        .count();
    assert_eq!(sine_count, 1, "Sine should be indexed once");

    let read_overloads = index
        .symbols
        .iter()
        .filter(|symbol| symbol.full_name == "read")
        .count();
    assert_eq!(read_overloads, 2, "read overloads should remain distinct");
}

#[test]
fn member_completion_hides_proc_private_members_from_outside() {
    let source = r#"proc Voice:
  params:
    private cutoff = 1000.0
    gain = 1.0
  outs:
    out1
  def helper(x):
    return x
  sample:
    out1 = helper(cutoff) * gain

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice(gain = 0.5)
"#;
    let index = index_at(source, "voice(gain", "voice".len());
    let member_labels = labels(index.member_items("voice", ""));

    assert!(
        !member_labels.iter().any(|label| label == "helper"),
        "external member completion should not include proc-local defs: {member_labels:?}"
    );
    assert!(
        !member_labels.iter().any(|label| label == "cutoff"),
        "external member completion should not include private params: {member_labels:?}"
    );
    assert!(
        member_labels.iter().any(|label| label == "gain"),
        "public params should remain externally visible: {member_labels:?}"
    );
}

#[test]
fn member_completion_includes_builtin_and_lookup_methods_for_buffers() {
    let source = r#"buffers:
  src: f32[]

outs:
  out1

sample:
  out1 = f32(src.len())
"#;
    let index = index_at(source, "src.len", "src".len());
    let items = index.member_items("src", "");
    let member_labels = labels(items.clone());

    for expected in [
        "bound",
        "len",
        "chans",
        "samplerate",
        "read",
        "write",
        "readL",
        "readC",
    ] {
        assert!(
            member_labels.iter().any(|label| label == expected),
            "buffer member completion should include {expected}: {member_labels:?}"
        );
    }

    let read_signatures = items
        .iter()
        .filter(|item| item.label == "read")
        .filter_map(|item| item.label_detail.as_deref())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        read_signatures,
        BTreeSet::from(["(i: i32)", "(ch: i32, i: i32)"])
    );
    assert!(
        !member_labels.iter().any(|label| label == "calcIdx"),
        "non-extension lookup helpers must not appear as buffer methods: {member_labels:?}"
    );

    let incomplete_source = source.replace("src.len()", "src.");
    let result = completion_items_for_document_with_index(
        &incomplete_source,
        None,
        &HashMap::new(),
        None,
        None,
        position_at(&incomplete_source, "src.", "src.".len()),
        true,
    );
    let encoded_labels = result
        .items
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(encoded_labels.contains("samplerate"));
    assert!(encoded_labels.contains("bound"));
    assert!(encoded_labels.contains("readL"));
}

#[test]
fn member_completion_recognizes_proc_buffers_and_typed_buffer_params() {
    let source = r#"def first(buf: buffer<f32[]>):
  return buf[0]

proc Player:
  buffers:
    clip: f32[]
  outs:
    out1
  sample:
    out1 = clip.read(0)
"#;
    let param_index = index_at(source, "buf[0]", "buf".len());
    let param_labels = labels(param_index.member_items("buf", ""));
    assert!(param_labels.iter().any(|label| label == "samplerate"));
    assert!(param_labels.iter().any(|label| label == "bound"));
    assert!(param_labels.iter().any(|label| label == "readL"));

    let proc_index = index_at(source, "clip.read", "clip".len());
    let proc_labels = labels(proc_index.member_items("clip", ""));
    assert!(proc_labels.iter().any(|label| label == "samplerate"));
    assert!(proc_labels.iter().any(|label| label == "bound"));
    assert!(proc_labels.iter().any(|label| label == "read"));
}

#[test]
fn unsafe_index_completion_respects_known_storage_direction() {
    let source = r#"ins:
  source: f32[4]
outs:
  destination: f32[4]
sample:
  view = source[:]
  destination[0] = view[0]
"#;
    let input_index = index_at(source, "source[:]", "source".len());
    let input_labels = labels(input_index.member_items("source", ""));
    assert!(input_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(!input_labels.iter().any(|label| label == WRITE_UNSAFE_FN));

    let view_index = index_at(source, "view[0]", "view".len());
    let view_labels = labels(view_index.member_items("view", ""));
    assert!(view_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(!view_labels.iter().any(|label| label == WRITE_UNSAFE_FN));

    let incomplete_source = source.replace("destination[0] = view[0]", "destination[0] = view.");
    let result = completion_items_for_document_with_index(
        &incomplete_source,
        None,
        &HashMap::new(),
        None,
        None,
        position_at(&incomplete_source, "view.", "view.".len()),
        true,
    );
    assert!(result
        .items
        .iter()
        .any(|item| item["label"] == READ_UNSAFE_FN));
    assert!(!result
        .items
        .iter()
        .any(|item| item["label"] == WRITE_UNSAFE_FN));

    let output_index = index_at(source, "destination[0]", "destination".len());
    let output_labels = labels(output_index.member_items("destination", ""));
    assert!(!output_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(output_labels.iter().any(|label| label == WRITE_UNSAFE_FN));
}

#[test]
fn unsafe_index_completion_covers_primitive_and_aggregate_arrays() {
    let source = r#"proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1
init:
  values: f32[8]
  voices: Voice[4] = Voice()
sample:
  out1 = values[0] + voices[0]()
"#;
    let primitive_index = index_at(source, "values[0]", "values".len());
    let primitive_labels = labels(primitive_index.member_items("values", ""));
    assert!(primitive_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(primitive_labels
        .iter()
        .any(|label| label == WRITE_UNSAFE_FN));

    let aggregate_index = index_at(source, "voices[0]", "voices".len());
    let aggregate_labels = labels(aggregate_index.member_items("voices", ""));
    assert!(aggregate_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(!aggregate_labels
        .iter()
        .any(|label| label == WRITE_UNSAFE_FN));
}

#[test]
fn buffer_read_results_do_not_inherit_buffer_members() {
    let source = r#"buffers:
  source: f32
  bank: f32 {2}
outs:
  out1
sample:
  indexed_value = source[0]
  free_unsafe_value = read_unsafe(source, 0)
  receiver_unsafe_value = source.read_unsafe(0)
  selected = bank[0]
  out1 = indexed_value + free_unsafe_value + receiver_unsafe_value + selected[0]
"#;
    let index = index_at(source, "out1 = indexed_value", 0);

    for scalar in [
        "indexed_value",
        "free_unsafe_value",
        "receiver_unsafe_value",
    ] {
        assert!(
            index.member_items(scalar, "").is_empty(),
            "scalar buffer read '{scalar}' must not inherit buffer members"
        );
    }

    let selected_labels = labels(index.member_items("selected", ""));
    assert!(selected_labels.iter().any(|label| label == "bound"));
    assert!(selected_labels
        .iter()
        .any(|label| label == ARRAY_LEN_METHOD));
    assert!(selected_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(selected_labels.iter().any(|label| label == WRITE_UNSAFE_FN));
}

#[test]
fn unsafe_aggregate_alias_completion_accepts_free_and_receiver_syntax() {
    let source = r#"proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1
init:
  voices: Voice[4] = Voice()
sample:
  free_voice = read_unsafe(voices, 0)
  receiver_voice = voices.read_unsafe(0)
  out1 = free_voice() + receiver_voice()
"#;
    let index = index_at(source, "out1 = free_voice", 0);

    for alias in ["free_voice", "receiver_voice"] {
        let alias_labels = labels(index.member_items(alias, ""));
        assert!(
            alias_labels.iter().any(|label| label == "out1"),
            "aggregate alias '{alias}' should retain Voice members: {alias_labels:?}"
        );
    }
}

#[test]
fn unsafe_index_completion_uses_only_runtime_surface_names() {
    let source = r#"ins 2
outs 2
sample:
  out1 = 0.0
"#;
    let index = index_at(source, "out1 = 0.0", 0);

    let ins_labels = labels(index.member_items("ins", ""));
    assert!(ins_labels.iter().any(|label| label == READ_UNSAFE_FN));
    assert!(index.member_items("inputs", "").is_empty());

    let outs_labels = labels(index.member_items("outs", ""));
    assert!(outs_labels.iter().any(|label| label == WRITE_UNSAFE_FN));
    assert!(index.member_items("outputs", "").is_empty());
}

#[test]
fn unsafe_index_intrinsics_are_available_in_free_call_completion() {
    let source = "sample:\n  out1 = rea";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  out1 = rea".len() as u32,
        },
        true,
    );
    assert!(result
        .items
        .iter()
        .any(|item| item["label"] == READ_UNSAFE_FN));

    let source = "sample:\n  write_";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: 1,
            character: "  write_".len() as u32,
        },
        true,
    );
    assert!(result
        .items
        .iter()
        .any(|item| item["label"] == WRITE_UNSAFE_FN));
}

#[test]
fn general_completion_keeps_proc_private_members_inside_owner() {
    let source = r#"proc Voice:
  params:
    private cutoff = 1000.0
  outs:
    out1
  def helper(x):
    return x
  sample:
    out1 = helper(cutoff)

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice()
"#;
    let index = index_at(source, "helper(cutoff)", 1);
    let general_labels = labels(index.general_items(""));

    assert!(
        general_labels.iter().any(|label| label == "helper"),
        "proc-local defs should complete inside their owning proc: {general_labels:?}"
    );
    assert!(
        general_labels.iter().any(|label| label == "cutoff"),
        "private params should complete inside their owning proc: {general_labels:?}"
    );
}

#[test]
fn call_arg_completion_hides_external_proc_local_def_params() {
    let source = r#"proc Voice:
  outs:
    out1
  def helper(x):
    return x
  sample:
    out1 = helper(x = 0.0)

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice()
"#;
    let index = index_at(source, "voice()", "voice".len());
    let arg_labels = labels(index.call_arg_items("voice.helper"));

    assert!(
        arg_labels.is_empty(),
        "external proc-local def calls should not expose argument completions: {arg_labels:?}"
    );
}

#[test]
fn top_level_tasks_complete_owner_state_locals_and_control_surface() {
    let source = r#"buffers:
  table: f32
init:
  seed = 1.0
task prepare():
  local = seed
  table[0] = local
  yield
block:
  prepare.reset()
  await prepare()
  sample:
    out1 = table[0]
"#;
    let body_index = index_at(source, "table[0] = local", "table[0] = loc".len());
    let body_labels = labels(body_index.general_items(""));
    for expected in ["table", "seed", "local", "prepare"] {
        assert!(
            body_labels.iter().any(|label| label == expected),
            "task body should complete '{expected}': {body_labels:?}"
        );
    }

    let control_index = index_at(source, "prepare.reset", "prepare.".len());
    let reset = control_index.member_items("prepare", "");
    assert_eq!(labels(reset), vec!["reset"]);
}

#[test]
fn proc_tasks_complete_inside_the_owner_but_not_on_instances() {
    let source = r#"proc Loader:
  buffers:
    table: f32
  init:
    seed = 1.0
  task prepare():
    local = seed
    table[0] = local
    yield
  block:
    await prepare()
    sample:
      out1 = table[0]

init:
  loader = Loader()
sample:
  out1 = loader()
"#;
    let body_index = index_at(source, "table[0] = local", "table[0] = loc".len());
    let body_labels = labels(body_index.general_items(""));
    for expected in ["table", "seed", "local", "prepare"] {
        assert!(
            body_labels.iter().any(|label| label == expected),
            "proc task body should complete '{expected}': {body_labels:?}"
        );
    }

    let external_index = index_at(source, "out1 = loader()", "out1 = loader".len());
    assert!(
        external_index.member_items("loader", "pre").is_empty(),
        "proc tasks are owner-private control symbols"
    );
}
