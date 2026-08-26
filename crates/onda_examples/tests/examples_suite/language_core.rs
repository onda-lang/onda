use super::*;

#[test]

fn gain_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(GAIN, frames);

    set_param_f32(&mut instance, "gain", 0.5);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32).collect();

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, out) in output.iter().enumerate() {
        assert_near(*out, input[idx] * 0.5, 1e-6);
    }
}

#[test]

fn events_metadata_and_scalar_dispatch_work() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_SCALAR_UPDATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.event_count(), 1);

    assert_eq!(instance.event_name(0), Some("set_amp"));

    assert_eq!(instance.event_index("set_amp"), Some(0));

    assert_eq!(instance.event_payload_bytes(0), Some(4));

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    let payload = 0.75_f32.to_ne_bytes();

    trigger_event_by_index(&mut instance, 0, &payload, None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn init_restores_resettable_runtime_state() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_SCALAR_UPDATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    let payload = 0.5_f32.to_ne_bytes();

    trigger_event_by_index(&mut instance, 0, &payload, None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }

    init(&mut instance, InitMode::PreservePinned).expect("init should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn event_array_payload_dispatch_and_unknown_index_ignore() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_ARRAY_UPDATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.event_count(), 1);

    assert_eq!(instance.event_payload_bytes(0), Some(8));

    trigger_event_by_index(&mut instance, 99, &[1, 2, 3], None)
        .expect("unknown event index should be ignored");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    let mut payload = Vec::new();

    payload.extend_from_slice(&0.25_f32.to_ne_bytes());

    payload.extend_from_slice(&0.75_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, 0, &payload, None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn event_handler_local_array_literal_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_LOCAL_ARRAY_LITERAL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    trigger_event_by_index(&mut instance, 0, &[], None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn event_payload_mismatch_returns_runtime_error() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(EVENT_SCALAR_UPDATE_EXAMPLE, frames);

    let err = trigger_event_by_index(&mut instance, 0, &[], None)
        .expect_err("payload mismatch should return runtime error");

    assert!(
        err.message.contains("expects"),
        "expected payload-size error, got '{}'",
        err.message
    );
}

#[test]

fn proc_event_forwarding_from_top_level_event_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.6_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn proc_event_call_from_top_level_init_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_CALL_FROM_TOP_LEVEL_INIT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.55, 1e-6);
    }
}

#[test]

fn proc_event_call_from_top_level_sample_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_CALL_FROM_TOP_LEVEL_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.35, 1e-6);
    }
}

#[test]

fn proc_event_dynamic_proc_array_call_from_top_level_sample_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_PROC_ARRAY_DYNAMIC_CALL_FROM_TOP_LEVEL_SAMPLE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.42, 1e-6);
    }
}

#[test]

fn proc_array_indexed_event_forwarding_from_top_level_event_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_ARRAY_INDEXED_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.6_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn proc_array_alias_event_forwarding_from_top_level_event_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_ARRAY_ALIAS_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.66_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.66, 1e-6);
    }
}

#[test]

fn proc_event_call_from_parent_proc_init_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_PARENT_INIT_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.62, 1e-6);
    }
}

#[test]

fn proc_event_call_from_parent_proc_block_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_PARENT_BLOCK_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.73, 1e-6);
    }
}

#[test]

fn proc_event_call_from_parent_proc_event_via_top_level_sample_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_PROC_PARENT_EVENT_CALLED_FROM_TOP_LEVEL_SAMPLE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.81, 1e-6);
    }
}

#[test]

fn nested_proc_array_indexed_event_forwarding_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_NESTED_PROC_ARRAY_INDEXED_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.7_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.7, 1e-6);
    }
}

#[test]

fn deep_nested_proc_array_dynamic_index_event_forwarding_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_DEEP_NESTED_PROC_ARRAY_DYNAMIC_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.65_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.65, 1e-6);
    }
}

#[test]

fn deeper_nested_proc_array_dynamic_index_event_forwarding_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_DEEPER_NESTED_PROC_ARRAY_DYNAMIC_FORWARD_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.6_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn proc_array_alias_event_forwarding_from_parent_proc_event_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_PROC_ARRAY_ALIAS_FORWARD_FROM_PARENT_PROC_EVENT_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");

    trigger_event_by_index(&mut instance, idx, &0.68_f32.to_ne_bytes(), None)
        .expect("forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.68, 1e-6);
    }
}

#[test]

fn events_reject_forbidden_writes_and_immutability() {
    let parsed = parse_program(EVENT_WRITE_OUTPUT_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("events should reject output writes");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("cannot assign to output symbol")),
        "expected output-write error, got {:?}",
        errs
    );

    let parsed =
        parse_program(EVENT_WRITE_NON_INIT_STATE_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("events should reject non-init-root writes");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("init-root state") && d.message.contains("lfo")),
        "expected init-root write restriction error, got {:?}",
        errs
    );

    let parsed = parse_program(EVENT_PARAM_IMMUTABLE_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("events should reject param mutation");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("immutable event array parameter")),
        "expected immutable event param error, got {:?}",
        errs
    );
}

#[test]

fn proc_events_reject_expression_position_and_owning_self_calls() {
    let parsed =
        parse_program(PROC_EVENT_EXPRESSION_POSITION_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("proc event expression use should fail");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("statement-only") && d.message.contains("voice.note_on")),
        "expected statement-only proc event error, got {:?}",
        errs
    );

    let parsed =
        parse_program(PROC_EVENT_OWNING_SELF_CALL_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("owning self proc event call should fail");

    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("cannot call event 'Voice.note_on' on the owning proc instance")
        }),
        "expected owning-proc self-call error, got {:?}",
        errs
    );
}

#[test]

fn proc_events_reject_unknown_targets_and_bad_argument_shapes() {
    let parsed = parse_program(PROC_EVENT_UNKNOWN_IN_PARENT_INIT_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let errs = analyze(parsed).expect_err("unknown proc event target should fail");

    assert!(
        errs.iter().any(|d| d
            .message
            .contains("unknown processor event 'voice.not_real'")),
        "expected unknown proc event error, got {:?}",
        errs
    );

    let parsed = parse_program(PROC_EVENT_MISSING_ARG_IN_TOP_LEVEL_SAMPLE_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let errs = analyze(parsed).expect_err("missing proc event argument should fail");

    assert!(
        errs.iter().any(|d| {
            d.message.contains(
                "processor event call 'voice.note_on(...)' is missing required argument 'value'",
            )
        }),
        "expected missing proc event argument error, got {:?}",
        errs
    );
}

#[test]

fn proc_event_defaults_bind_omitted_args() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"

outs { out1 }

proc Voice {

  outs { out1 }

  events {

    note_on(freq_hz: f32 = 440.0, accent: bool = true) {

      gate = freq_hz

      if (accent) {

        gate = gate + 1.0

      }

    }

  }

  init { gate = 0.0 }

  sample { out1 = gate }

}

init { voice = Voice() }

sample {

  voice.note_on()

  out1 = voice()

}

"#,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 441.0, 1e-6);
    }
}

#[test]

fn proc_builtin_init_reruns_proc_init_with_new_params() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"

outs { out1 }

proc Voice {

  params { freq = 2.0 }

  outs { out1 }

  init {
    coeff = freq * 2.0
    phase = 0.0
  }

  events {
    dirty() {
      phase = 99.0
    }
  }

  sample {
    phase = phase + 1.0
    out1 = coeff + phase
  }

}

events {
  retune(value: f32) {
    voice.dirty()
    voice.init(freq = value)
  }
}

init { voice = Voice(freq = 2.0) }

sample { out1 = voice() }

"#,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 5.0, 1e-6);

    let payload = 3.0_f32.to_ne_bytes();

    trigger_event_by_index(&mut instance, 0, &payload, None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 7.0, 1e-6);
}

#[test]

fn proc_builtin_init_clamps_ranged_param_with_param_type() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"

outs { out1 }

proc Voice {

  params { value = 1.0 {0.0, 5.0} }

  outs { out1 }

  init { stored = value }

  sample { out1 = stored }

}

events {
  retune(value: f32) {
    voice.init(value = value)
  }
}

init { voice = Voice(value = 1.0) }

sample { out1 = voice() }

"#,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    let payload = 20.0_f32.to_ne_bytes();

    trigger_event_by_index(&mut instance, 0, &payload, None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 5.0, 1e-6);
}

#[test]

fn proc_builtin_init_omitted_array_param_uses_default() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"

const Defaults: f32[2] = [1.0, 2.0]

outs { out1 }

proc Voice {

  params { gains: f32[2] = Defaults }

  outs { out1 }

  init { stored = gains[0] + gains[1] }

  sample { out1 = stored }

}

events {
  reset() {
    voice.init()
  }
}

init { voice = Voice(gains = [3.0, 4.0]) }

sample { out1 = voice() }

"#,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 7.0, 1e-6);

    trigger_event_by_index(&mut instance, 0, &[], None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 3.0, 1e-6);
}

#[test]

fn proc_array_builtin_init_reruns_only_selected_slot_init() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"

outs { out1 }

proc Voice {

  params { value = 1.0 }

  outs { out1 }

  init {
    stored = value
    phase = 0.0
  }

  events {
    dirty() {
      phase = 99.0
    }
  }

  sample {
    phase = phase + 1.0
    out1 = stored + phase
  }

}

events {
  retune(slot: i32, value: f32) {
    voices[slot].dirty()
    voices[slot].init(value = value)
  }
}

init { voices: Voice[2] = Voice(value = 1.0) }

sample {
  out1 = voices[0]() + (voices[1]() * 10.0)
}

"#,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 22.0, 1e-6);

    let mut payload = Vec::new();

    payload.extend_from_slice(&1_i32.to_ne_bytes());

    payload.extend_from_slice(&3.0_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, 0, &payload, None).expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 43.0, 1e-6);
}

#[test]

fn proc_event_array_defaults_bind_omitted_args() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"

outs { out1 }

proc Voice {

  outs { out1 }

  events {

    set_curve(values: f32[3] = [0.25, 0.5, 0.75]) {

      sum = values[0] + values[1] + values[2]

    }

  }

  init { sum = 0.0 }

  sample { out1 = sum }

}

init { voice = Voice() }

sample {

  voice.set_curve()

  out1 = voice()

}

"#,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 1.5, 1e-6);
    }
}

#[test]

fn event_defaults_reject_slice_defaults() {
    let parsed = parse_program(
        r#"

outs { out1 }

init { gate = 0.0 }

events {

  load(values: f32[] = [1.0]) {

    gate = values[0]

  }

}

sample { out1 = gate }

"#,
    )
    .expect("parse should succeed");

    let errs = analyze(parsed).expect_err("slice event defaults should fail");

    assert!(
        errs.iter().any(|d| d
            .message
            .contains("default is not supported for slice event params")),
        "expected slice-default diagnostic, got {errs:?}"
    );
}

#[test]

fn proc_event_slice_params_accept_internal_array_sources() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_EVENT_SLICE_FROM_TOP_LEVEL_INIT_STATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_EVENT_SLICE_FROM_PARENT_PROC_STATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn slices_lower_to_array_views_for_direct_calls_and_local_aliases() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_EVENT_DIRECT_SLICE_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 52.0, 1e-6);

    let (mut instance, in_channels, out_channels) =
        compile_instance(LOCAL_SLICE_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 33.0, 1e-6);
}

#[test]

fn slice_assignments_fill_copy_and_preserve_overlap_semantics() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SLICE_FILL_ASSIGN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 7.5, 1e-6);

    let (mut instance, in_channels, out_channels) =
        compile_instance(SLICE_COPY_ASSIGN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6);

    let (mut instance, in_channels, out_channels) =
        compile_instance(SLICE_OVERLAP_COPY_ASSIGN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 10.0, 1e-6);
}

#[test]

fn proc_event_fixed_array_params_accept_internal_array_sources() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_EVENT_FIXED_ARRAY_FROM_TOP_LEVEL_INIT_STATE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);
}

#[test]

fn top_level_events_accept_slice_payloads() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_SLICE_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");

    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload = Vec::new();

    payload.extend_from_slice(&(2_i32).to_ne_bytes());

    payload.extend_from_slice(&0.25_f32.to_ne_bytes());

    payload.extend_from_slice(&0.75_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, event_idx, &payload, None)
        .expect("slice event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 2.25, 1e-6);
}

#[test]

fn top_level_events_forward_slice_payloads_to_proc_events() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_SLICE_PROC_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");

    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload = Vec::new();

    payload.extend_from_slice(&(2_i32).to_ne_bytes());

    payload.extend_from_slice(&0.5_f32.to_ne_bytes());

    payload.extend_from_slice(&0.25_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, event_idx, &payload, None)
        .expect("slice forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 2.75, 1e-6);
}

#[test]

fn top_level_slice_event_truncated_payload_returns_runtime_error() {
    let frames = 1;

    let (mut instance, _, _) = compile_instance(TOP_LEVEL_EVENT_SLICE_PARAM_EXAMPLE, frames);

    let event_idx = instance.event_index("load").expect("load event must exist");

    let mut payload = Vec::new();

    payload.extend_from_slice(&(3_i32).to_ne_bytes());

    payload.extend_from_slice(&0.25_f32.to_ne_bytes());

    payload.extend_from_slice(&0.75_f32.to_ne_bytes());

    let err = trigger_event_by_index(&mut instance, event_idx, &payload, None)
        .expect_err("truncated slice payload should fail");

    assert!(
        err.message.contains("payload"),
        "expected payload-related runtime error, got {:?}",
        err
    );
}

#[test]

fn top_level_events_forward_fixed_array_payloads_to_proc_events() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_FIXED_ARRAY_PROC_FORWARD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");

    assert_eq!(instance.event_payload_bytes(event_idx), Some(16));

    let mut payload = Vec::new();

    payload.extend_from_slice(&0.1_f32.to_ne_bytes());

    payload.extend_from_slice(&0.2_f32.to_ne_bytes());

    payload.extend_from_slice(&0.3_f32.to_ne_bytes());

    payload.extend_from_slice(&0.4_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, event_idx, &payload, None)
        .expect("fixed-array forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);
}

#[test]

fn top_level_events_accept_mixed_fixed_and_slice_payloads() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_MIXED_FIXED_AND_SLICE_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");

    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload = Vec::new();

    payload.extend_from_slice(&0.25_f32.to_ne_bytes());

    payload.extend_from_slice(&0.75_f32.to_ne_bytes());

    payload.extend_from_slice(&(2_i32).to_ne_bytes());

    payload.extend_from_slice(&1.5_f32.to_ne_bytes());

    payload.extend_from_slice(&2.5_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, event_idx, &payload, None)
        .expect("mixed fixed/slice event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 4.5, 1e-6);
}

#[test]

fn top_level_events_forward_large_fixed_array_payloads_to_proc_events() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        TOP_LEVEL_EVENT_LARGE_FIXED_ARRAY_PROC_FORWARD_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");

    assert_eq!(instance.event_payload_bytes(event_idx), Some(96000 * 4));

    let mut payload = vec![0_u8; 96000 * 4];

    payload[0..4].copy_from_slice(&0.25_f32.to_ne_bytes());

    payload[(96000 - 1) * 4..96000 * 4].copy_from_slice(&0.75_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, event_idx, &payload, None)
        .expect("large fixed-array forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);
}

#[test]

fn generic_proc_events_accept_generic_slice_params() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EVENT_SLICE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 2.25, 1e-6);
}

#[test]

fn generic_proc_events_accept_generic_slice_and_scalar_params() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EVENT_SLICE_WITH_SCALAR_PARAMS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 2.0, 1e-6);
}

#[test]

fn procs_reject_direct_self_recursive_instantiation() {
    let parsed =
        parse_program(PROC_SELF_RECURSIVE_INSTANCE_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("direct self-recursive proc state should fail");

    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("processor 'Voice' cannot instantiate itself as state symbol 'other'")
        }),
        "expected direct self-recursive proc-instance error, got {:?}",
        errs
    );

    let parsed =
        parse_program(PROC_SELF_RECURSIVE_ARRAY_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("direct self-recursive proc arrays should fail");

    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("processor 'Voice' cannot instantiate itself as processor array 'voices'")
        }),
        "expected direct self-recursive proc-array error, got {:?}",
        errs
    );
}

#[test]

fn events_reject_duplicate_and_conflicting_names() {
    let errs = parse_program(EVENT_DUPLICATE_NAME_ERROR_EXAMPLE)
        .expect_err("duplicate top-level events should fail at parse");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("duplicate event declaration 'ping'")),
        "expected duplicate top-level event error, got {:?}",
        errs
    );

    let errs = parse_program(PROC_EVENT_DUPLICATE_NAME_ERROR_EXAMPLE)
        .expect_err("duplicate proc events should fail at parse");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("duplicate event declaration 'note_on'")),
        "expected duplicate proc event error, got {:?}",
        errs
    );

    let parsed =
        parse_program(PROC_EVENT_NAME_CONFLICT_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("conflicting proc event names should fail");

    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("event name conflicts with an existing callable/endpoint name")
        }),
        "expected proc event name conflict error, got {:?}",
        errs
    );
}

#[test]

fn io_and_param_count_shorthand_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(COUNT_SHORTHAND_IO_PARAMS_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    set_param_f32(&mut instance, "param1", 0.5);

    set_param_f32(&mut instance, "param2", -1.0);

    let input = vec![
        1.0_f32, 10.0_f32, //
        2.0_f32, 20.0_f32, //
        3.0_f32, 30.0_f32, //
        4.0_f32, 40.0_f32,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        1.5_f32, 9.0_f32, //
        2.5_f32, 19.0_f32, //
        3.5_f32, 29.0_f32, //
        4.5_f32, 39.0_f32,
    ];

    for (actual, target) in output.iter().zip(expected.iter()) {
        assert_near(*actual, *target, 1e-6);
    }
}

#[test]

fn sine_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(SINE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let input = Vec::<f32>::new();

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let phase_step = 440.0_f32 * 6.2831855_f32 / 48_000.0_f32;

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32 * phase_step).sin();

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn one_pole_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(ONE_POLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.1, 1e-6);

    assert!(output[frames - 1] > output[0]);

    assert!(output[frames - 1] < 1.0);
}

#[test]

fn if_statement_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(IF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.25, 1e-6);
    }

    set_param_f32(&mut instance, "gate", 0.0);

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, -0.25, 1e-6);
    }
}

#[test]

fn for_loop_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(FOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn for_loop_accepts_variable_bound() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(FOR_VAR_BOUND_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn for_loop_accepts_parenthesized_expression_bound() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(FOR_PAREN_EXPR_BOUND_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn for_loop_supports_descending_step_and_inclusive_end() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(FOR_DESCENDING_STEP_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn loop_sugar_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(LOOP_SUGAR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn loop_sugar_accepts_variable_bound() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(LOOP_VAR_BOUND_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn init_control_flow_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_CONTROL_FLOW_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.6, 1e-6);
    }
}

#[test]

fn block_nested_branch_state_registration_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_BRANCH_STATE_REGISTRATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn sample_nested_branch_typed_registration_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_BRANCH_TYPED_REGISTRATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn block_loop_control_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_LOOP_CONTROL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn sample_loop_control_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_LOOP_CONTROL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn block_break_outside_loop_is_rejected() {
    let parsed =
        parse_program(BLOCK_BREAK_OUTSIDE_LOOP_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("block break outside loop should fail");

    assert!(
        errs.iter().any(|d| d
            .message
            .contains("break is only allowed inside for/while/loop bodies")),
        "expected block break diagnostic, got {:?}",
        errs
    );
}

#[test]

fn sample_continue_outside_loop_is_rejected() {
    let parsed =
        parse_program(SAMPLE_CONTINUE_OUTSIDE_LOOP_ERROR_EXAMPLE).expect("parse should succeed");

    let errs = analyze(parsed).expect_err("sample continue outside loop should fail");

    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("continue is only allowed inside for/while/loop bodies")
        }),
        "expected sample continue diagnostic, got {:?}",
        errs
    );
}

#[test]

fn def_call_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(DEF_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_monomorphizes_scalar_numeric_calls() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_MONO_NUMERIC_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn def_named_default_args_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_NAMED_DEFAULT_ARGS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.25, 1e-6);
    }
}

#[test]

fn def_overload_by_arity_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_ARITY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn def_overload_exact_typed_beats_untyped() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_TYPED_BEATS_UNTYPED_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 20.0, 1e-6);
    }
}

#[test]

fn def_overload_widening_fallback_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_WIDENING_FALLBACK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn def_overload_i32_numeric_tie_is_ambiguous() {
    let parsed =
        parse_program(DEF_OVERLOAD_I32_AMBIGUOUS_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject ambiguous i32 overload tie (i64 vs f64 widening)"
    );
}

#[test]

fn def_overload_defaults_participate_in_resolution() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_DEFAULTS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn def_overload_defaults_can_be_ambiguous() {
    let parsed =
        parse_program(DEF_OVERLOAD_DEFAULTS_AMBIGUOUS_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject ambiguous overloads when defaults produce equivalent matches"

    );
}

#[test]

fn def_overload_supports_struct_and_scalar_variants() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_STRUCT_AND_SCALAR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

#[test]

fn def_overload_supports_buffer_and_scalar_variants() {
    let parsed =
        parse_program(DEF_OVERLOAD_BUFFER_AND_SCALAR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "semantic analysis should accept overloads that differ by buffer vs scalar parameter types"
    );
}

#[test]

fn struct_methods_support_overloading() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_METHOD_OVERLOAD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn positional_after_named_is_rejected() {
    let parsed =
        parse_program(DEF_POSITIONAL_AFTER_NAMED_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject positional args after named args"
    );
}

#[test]

fn def_cannot_capture_top_level_symbols() {
    let parsed = parse_program(DEF_CANNOT_CAPTURE_TOP_LEVEL_SYMBOLS_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject top-level ins/params/state/buffers referenced in def scope"

    );
}

#[test]

fn def_without_return_defaults_to_zero() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(DEF_NO_RETURN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn def_return_exits_early() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_EARLY_RETURN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_struct_argument_is_passed_by_ref_with_writeback() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARG_BY_REF_WRITEBACK_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];

    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");

    assert_near(output[0], 4.0, 1e-6);
}

#[test]

fn def_struct_arg_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_struct_data_arg_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_DATA_ARG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_struct_array_indexed_arg_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARRAY_INDEXED_ARG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn def_struct_array_inline_field_ref_compiles_and_runs() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARRAY_INLINE_FIELD_REF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 3.0, 1e-6);

    assert_near(output[1], 7.0, 1e-6);
}

#[test]

fn proc_array_indexed_field_assignment_in_def_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INDEXED_FIELD_ASSIGN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_array_param_len_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_PARAM_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 9.0, 1e-6);
    }
}

#[test]

fn struct_array_param_len_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_ARRAY_PARAM_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 9.0, 1e-6);
    }
}

#[test]

fn nested_struct_field_assignment_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_STRUCT_FIELD_WRITE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn def_struct_array_runtime_index_matches_all_elements() {
    let src = r#"

struct Pair:

  x



outs:

  out1



def sum_pairs(pairs):

  total = 0.0

  for i in 0..4:

    total = total + pairs[i].x

  return total



init:

  pairs: Pair[4]

  for i in 0..4:

    p = pairs[i]

    p.x = f32(i + 1)



sample:

  out1 = sum_pairs(pairs)

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn def_struct_array_forwarding_matches_all_elements() {
    let src = r#"

struct Pair:

  x



outs:

  out1



def sum_inner(pairs):

  total = 0.0

  for i in 0..4:

    p = pairs[i]

    total = total + p.x

  return total



def sum_outer(pairs):

  return sum_inner(pairs)



init:

  pairs: Pair[4]

  for i in 0..4:

    p = pairs[i]

    p.x = f32(i + 1)



sample:

  out1 = sum_outer(pairs)

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn def_struct_array_multi_layer_methods_and_nested_chain_compile_and_run() {
    let src = r#"

struct Tap:

  gain: f32



  def read(self):

    return self.gain * 2.0



struct Voice:

  tap: Tap

  bias: f32



  def value(self):

    return self.tap.read() + self.bias



outs:

  out1



def read_voice(voice: Voice):

  return voice.value()



def read_outer(voices, idx: i32):

  return read_voice(voices[idx]) + 1.0



init:

  voices: Voice[2]

  voice0 = voices[0]

  voice0.tap.gain = 1.5

  voice0.bias = 0.5

  voice1 = voices[1]

  voice1.tap.gain = 2.0

  voice1.bias = 1.0



sample:

  out1 = read_outer(voices, 1)

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]

fn def_struct_array_chain_of_structs_with_methods_compile_and_run() {
    let src = r#"

struct Tap:

  gain: f32



  def read(self):

    return self.gain * 2.0



struct Voice:

  tap: Tap

  bias: f32



  def value(self):

    return self.tap.read() + self.bias



struct Rack:

  voices: Voice[2]



outs:

  out1



def read_voice(voice: Voice):

  return voice.value()



def read_rack(rack: Rack, idx: i32):

  return read_voice(rack.voices[idx])



def read_outer(racks, rack_idx: i32, voice_idx: i32):

  return read_rack(racks[rack_idx], voice_idx)



init:

  racks: Rack[2]

  rack0 = racks[0]

  voice0 = rack0.voices[0]

  voice0.tap.gain = 1.0

  voice0.bias = 0.5

  rack1 = racks[1]

  voice1 = rack1.voices[1]

  voice1.tap.gain = 2.0

  voice1.bias = 1.0



sample:

  out1 = read_outer(racks, 1, 1)

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn proc_array_init_event_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INIT_EVENT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.3, 1e-6);
    }
}

#[test]

fn proc_array_def_init_matches_inline_loop() {
    let def_src = r#"

import std/osc



const NumOsc = 4



params:

  freq = 50.0 { 1, 1000 }



def initVoices(voices, freq):

  for i in 0..NumOsc:

    h = f32(i + 1)

    voices[i].init(freq = freq * h, amp = 0.12 / h)



init:

  voices: std::osc::Sine[NumOsc]

  initVoices(voices, freq)



sample:

  mix = 0.0

  for i in 0..NumOsc:

    mix = mix + voices[i]()

  out1 = mix

"#;

    let inline_src = r#"

import std/osc



const NumOsc = 4



params:

  freq = 50.0 { 1, 1000 }



init:

  voices: std::osc::Sine[NumOsc]

  for i in 0..NumOsc:

    h = f32(i + 1)

    voices[i].init(freq = freq * h, amp = 0.12 / h)



sample:

  mix = 0.0

  for i in 0..NumOsc:

    mix = mix + voices[i]()

  out1 = mix

"#;

    let frames = 256;

    let (mut def_instance, _, _) = compile_instance(def_src, frames);

    let (mut inline_instance, _, _) = compile_instance(inline_src, frames);

    let mut def_output = vec![0.0_f32; frames];

    let mut inline_output = vec![0.0_f32; frames];

    process_interleaved(&mut def_instance, &[], &mut def_output, frames)
        .expect("def process should succeed");

    process_interleaved(&mut inline_instance, &[], &mut inline_output, frames)
        .expect("inline process should succeed");

    for (idx, (def_sample, inline_sample)) in def_output.iter().zip(&inline_output).enumerate() {
        assert!(
            (*def_sample - *inline_sample).abs() <= 1e-5,
            "sample {idx} differed: def={def_sample}, inline={inline_sample}"
        );
    }
}

#[test]

fn proc_array_def_alias_init_matches_inline_alias_loop() {
    let def_src = r#"

import std/osc



const NumOsc = 4



params:

  freq = 50.0 { 1, 1000 }



def initVoices(voices, freq):

  for i in 0..NumOsc:

    h = f32(i + 1)

    voice = voices[i]

    voice.init(freq = freq * h, amp = 0.12 / h)



init:

  voices: std::osc::Sine[NumOsc]

  initVoices(voices, freq)



sample:

  mix = 0.0

  for i in 0..NumOsc:

    mix = mix + voices[i]()

  out1 = mix

"#;

    let inline_src = r#"

import std/osc



const NumOsc = 4



params:

  freq = 50.0 { 1, 1000 }



init:

  voices: std::osc::Sine[NumOsc]

  for i in 0..NumOsc:

    h = f32(i + 1)

    voice = voices[i]

    voice.init(freq = freq * h, amp = 0.12 / h)



sample:

  mix = 0.0

  for i in 0..NumOsc:

    mix = mix + voices[i]()

  out1 = mix

"#;

    let frames = 256;

    let (mut def_instance, _, _) = compile_instance(def_src, frames);

    let (mut inline_instance, _, _) = compile_instance(inline_src, frames);

    let mut def_output = vec![0.0_f32; frames];

    let mut inline_output = vec![0.0_f32; frames];

    process_interleaved(&mut def_instance, &[], &mut def_output, frames)
        .expect("def process should succeed");

    process_interleaved(&mut inline_instance, &[], &mut inline_output, frames)
        .expect("inline process should succeed");

    for (idx, (def_sample, inline_sample)) in def_output.iter().zip(&inline_output).enumerate() {
        assert!(
            (*def_sample - *inline_sample).abs() <= 1e-5,
            "sample {idx} differed: def={def_sample}, inline={inline_sample}"
        );
    }
}

#[test]

fn proc_array_def_multi_layer_alias_init_and_call_compile_and_run() {
    let src = r#"

proc Voice:

  params:

    gain = 0.0

    bias = 0.0

  outs:

    out1

  sample:

    out1 = gain + bias



outs:

  out1



def seed_slot(voices, idx: i32, gain, bias):

  voice = voices[idx]

  voice.init(gain = gain, bias = bias)



def seed_all(voices):

  seed_slot(voices, 0, 2.0, 0.5)

  seed_slot(voices, 1, 3.0, 1.0)



def read_slot(voices, idx: i32):

  return voices[idx]().out1



def render_all(voices):

  return read_slot(voices, 0) + read_slot(voices, 1)



init:

  voices: Voice[2] = Voice()

  seed_all(voices)



sample:

  out1 = render_all(voices)

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.5, 1e-6);
    }
}

#[test]

fn def_owner_proc_chain_across_multiple_layers_compile_and_run() {
    let src = r#"

proc Voice:

  params:

    gain = 0.0

  outs:

    out1

  sample:

    out1 = gain



proc Bank:

  params:

    base = 0.0

  outs:

    out1

  init:

    voices: Voice[2] = Voice()

    voices[0].init(gain = base + 1.0)

    voices[1].init(gain = base + 2.0)

  sample:

    out1 = voices[1]()



proc Rack:

  outs:

    out1

  init:

    banks: Bank[2] = [Bank(base = 0.0), Bank(base = 10.0)]

  sample:

    out1 = 0.0



outs:

  out1



def read_bank(rack: Rack, bank_idx: i32):

  return rack.banks[bank_idx]().out1



def read_outer(rack: Rack):

  return read_bank(rack, 1)



init:

  rack = Rack()



sample:

  out1 = read_outer(rack)

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 12.0, 1e-6);
    }
}

#[test]

fn top_level_def_nested_proc_array_dynamic_call_runs_block_hooks_only_for_active_slot_per_block() {
    let src = r#"

proc Voice:

  outs:

    out1

    pre

    post

  init:

    pre_count = 0.0

    post_count = 0.0

  block:

    pre_count = pre_count + 1.0

    sample:

      out1 = 0.0

      pre = pre_count

      post = post_count

    post_count = post_count + 1.0



proc Bank:

  outs:

    out1

  init:

    voices: Voice[2] = [Voice(), Voice()]

    idx: i32 = 0

  sample:

    x = run_selected(self, idx)

    v0 = voices[0]

    v1 = voices[1]

    out1 = x * 0.0 + v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post

    idx = 1 - idx



def run_selected(bank: Bank, idx: i32):

  return bank.voices[idx]().out1



outs:

  out1



init:

  bank = Bank()



sample:

  out1 = bank()

"#;

    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]

fn top_level_def_proc_array_dynamic_call_runs_block_hooks_only_for_active_slot_per_block() {
    let src = r#"

proc Voice:

  outs:

    out1

    pre

    post

  init:

    pre_count = 0.0

    post_count = 0.0

  block:

    pre_count = pre_count + 1.0

    sample:

      out1 = 0.0

      pre = pre_count

      post = post_count

    post_count = post_count + 1.0



outs:

  out1



def run_selected(voices, idx: i32):

  return voices[idx]().out1



init:

  voices: Voice[2] = [Voice(), Voice()]

  idx: i32 = 0



sample:

  x = run_selected(voices, idx)

  v0 = voices[0]

  v1 = voices[1]

  out1 = x * 0.0 + v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post

  idx = 1 - idx

"#;

    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]

fn top_level_def_proc_array_multi_layer_dynamic_call_runs_block_hooks_only_for_active_slot_per_block(
) {
    let src = r#"

proc Voice:

  outs:

    out1

    pre

    post

  init:

    pre_count = 0.0

    post_count = 0.0

  block:

    pre_count = pre_count + 1.0

    sample:

      out1 = 0.0

      pre = pre_count

      post = post_count

    post_count = post_count + 1.0



outs:

  out1



def run_leaf(voices, idx: i32):

  return voices[idx]().out1



def run_mid(voices, idx: i32):

  return run_leaf(voices, idx)



def run_outer(voices, idx: i32):

  return run_mid(voices, idx)



init:

  voices: Voice[2] = [Voice(), Voice()]

  idx: i32 = 0



sample:

  x = run_outer(voices, idx)

  v0 = voices[0]

  v1 = voices[1]

  out1 = x * 0.0 + v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post

  idx = 1 - idx

"#;

    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]

fn def_structural_arg_compiles_with_multiple_matching_structs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCTURAL_ARG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_array_arg_is_passed_by_ref_with_writeback() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_ARRAY_ARG_BY_REF_WRITE_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];

    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");

    assert_near(output[0], 4.0, 1e-6);
}

#[test]

fn def_array_arg_writeback_propagates_through_nested_def_calls() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_ARRAY_ARG_FORWARDING_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];

    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6);
}

#[test]

fn def_accepts_local_sample_array_arguments() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_LOCAL_ARRAY_ARG_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];

    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");

    assert_near(output[0], 2.0, 1e-6);
}

#[test]

fn def_explicit_struct_annotation_is_nominal() {
    let parsed =
        parse_program(DEF_EXPLICIT_STRUCT_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject passing non-matching struct to explicitly typed def parameter"

    );
}

#[test]

fn struct_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn struct_named_default_ctor_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_NAMED_DEFAULT_CTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn namespaced_struct_ctor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NAMESPACE_STRUCT_CTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn namespaced_def_resolution_uses_parent_then_global() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NAMESPACE_DEF_RESOLUTION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 112.0, 1e-6);
    }
}

#[test]

fn top_level_must_use_fully_qualified_namespaced_call() {
    let parsed = parse_program(NAMESPACE_TOP_LEVEL_UNQUALIFIED_CALL_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject unqualified call to namespaced function at top level"
    );
}

#[test]

fn import_and_include_resolve_transitively_from_entry_file() {
    let dir = mk_temp_dir("import_include");

    let main = dir.join("main.onda");

    let filter = dir.join("filter.onda");

    let shared = dir.join("shared.onda");

    fs::write(
        &shared,
        r#"

def shared(x) {

  return x * 2.0

}

"#,
    )
    .expect("write shared");

    fs::write(
        &filter,
        r#"

include "./shared.onda"

namespace DSP:

  struct S:

    x: f32 = 1.0

  def run(v):

    return shared(v) + 1.0

"#,
    )
    .expect("write filter");

    fs::write(
        &main,
        r#"

import filter

outs { out1 }

init {

  s = DSP::S()

}

sample {

  out1 = DSP::run(2.0) + s.x

}

"#,
    )
    .expect("write main");

    let parsed = parse_program_file(&main).expect("parse program file");

    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,

            block_size: 64,
        },
    )
    .expect("semantic analysis");

    let jit = lower_typed_and_jit(
        typed,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: 64,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("jit lowering");

    let mut instance = create_instance_initialized(
        jit,
        InstanceConfig {
            sample_rate: 48_000.0,

            frames_per_block: 64,

            in_channels: 0,

            out_channels: 1,
        },
    )
    .expect("instance");

    let mut output = vec![0.0_f32; 64];

    process_interleaved(&mut instance, &[], &mut output, 64).expect("process");

    assert_near(output[0], 6.0, 1e-6);

    fs::remove_dir_all(&dir).ok();
}

#[test]

fn builtin_std_import_resolves_without_local_std_path() {
    let dir = mk_temp_dir("builtin_std_import");

    let main = dir.join("main.onda");

    let shadow_std_dir = dir.join("std");

    fs::create_dir_all(&shadow_std_dir).expect("create local std dir");

    fs::write(
        shadow_std_dir.join("osc.onda"),
        r#"

def broken() {

  return unknown_symbol

}

"#,
    )
    .expect("write local shadow std file");

    fs::write(
        &main,
        r#"

import std/osc

outs { out1 }

init { o = std::osc::Sine(freq = 220.0) }

sample { out1 = o() }

"#,
    )
    .expect("write main");

    let parsed = parse_program_file(&main).expect("parse program file");

    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,

            block_size: 64,
        },
    )
    .expect("semantic analysis");

    fs::remove_dir_all(&dir).ok();
}

#[test]

fn struct_initialization_in_sample_is_rejected() {
    let parsed = parse_program(STRUCT_INIT_IN_SAMPLE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject struct ctor in sample"
    );
}

#[test]

fn data_read_write_clamps_and_truncates() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 12.0, 1e-6);
    }
}

#[test]

fn indexed_data_read_write_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(INDEXED_DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn indexed_access_supports_struct_field_data() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INDEXED_STRUCT_FIELD_DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn indexed_access_supports_typed_local_array_in_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INDEXED_TYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn data_len_returns_data_capacity() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(DATA_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn data_len_supports_struct_data_field_receiver() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_LEN_STRUCT_FIELD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn data_len_rejects_non_data_receiver() {
    let parsed =
        parse_program(DATA_LEN_INVALID_RECEIVER_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject x.len() for scalar x"
    );
}

#[test]

fn data_struct_elements_support_alias_field_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 1.5;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn struct_field_data_struct_elements_support_alias_field_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 3.0;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn init_struct_field_data_struct_elements_support_alias_field_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn def_struct_field_data_struct_elements_support_alias_field_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn def_struct_field_nested_data_struct_elements_support_alias_field_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEF_STRUCT_FIELD_NESTED_DATA_STRUCT_ELEM_ALIAS_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn data_struct_inline_field_read_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_INLINE_FIELD_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    for sample in &output[1..] {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn data_struct_inline_array_field_read_compiles_and_runs() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_INLINE_ARRAY_FIELD_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 2.0, 1e-6);

    assert_near(output[1], 4.0, 1e-6);
}

#[test]

fn init_struct_inline_field_read_compiles_and_runs() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_STRUCT_INLINE_FIELD_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn data_struct_nested_data_fields_support_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = 1.0 + ((idx + 1) as f32) * 0.25;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn struct_field_data_struct_nested_data_fields_support_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_FIELD_DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 0.5;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn data_struct_nested_struct_data_fields_support_recursive_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 0.25;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn struct_field_data_struct_nested_struct_data_fields_support_recursive_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        STRUCT_FIELD_DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 1.0;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn primitive_data_local_alias_binding_is_rejected() {
    let parsed = parse_program(DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_ok(),

        "semantic analysis should allow primitive array indexed reads as scalar copies via 'x = buf[i]'"

    );
}

#[test]

fn primitive_struct_field_data_local_alias_binding_is_rejected() {
    let parsed =
        parse_program(STRUCT_DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_ok(),

        "semantic analysis should allow primitive struct-array indexed reads as scalar copies via 'x = v.delay[i]'"

    );
}

#[test]

fn init_array_index_scalar_copy_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn def_struct_array_index_scalar_copy_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn typed_local_array_declaration_in_sample_is_allowed() {
    let parsed = parse_program(DATA_INIT_IN_SAMPLE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "semantic analysis should allow primitive T[N] declarations in sample"
    );
}

#[test]

fn typed_local_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn untyped_local_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(UNTYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn typed_local_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn untyped_local_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(UNTYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn typed_local_i32_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_I32_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn typed_local_bool_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_BOOL_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn typed_local_array_initializer_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_INIT_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn top_level_param_array_defaults_and_set_param_slots_work() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_PARAM_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    set_param_f32_array(&mut instance, "mix", &[1.5, 0.75]);

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.25, 1e-6);
    }
}

#[test]

fn declared_param_metadata_reports_array_as_single_entry() {
    let frames = 4;

    let (instance, _in_channels, _out_channels) =
        compile_instance(TOP_LEVEL_PARAM_ARRAY_EXAMPLE, frames);

    assert_eq!(instance.param_count(), 1);

    assert_eq!(instance.param_index("mix"), Some(0));

    assert_eq!(instance.param_name(0), Some("mix"));

    assert_eq!(instance.param_type(0).as_deref(), Some("f32[2]"));

    assert_eq!(instance.param_type_bytes(0), Some(8));
}

#[test]

fn declared_io_metadata_reports_arrays_as_single_entries() {
    let frames = 4;

    let (instance, _in_channels, _out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(instance.input_count(), 1);

    assert_eq!(instance.input_name(0), Some("in1"));

    assert_eq!(instance.input_type(0).as_deref(), Some("f32[2]"));

    assert_eq!(instance.input_type_bytes(0), Some(8));
}

#[test]

fn top_level_input_array_reads_indexed_channels() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 1);

    let input = vec![
        1.0_f32, 0.5, //
        2.0, 1.0, //
        -1.0, 2.0, //
        0.25, -0.5,
    ];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [2.5_f32, 5.0, 0.0, 0.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn top_level_output_array_writes_indexed_channels() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_OUTPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 0.25, 1e-6);

        assert_near(output[base + 1], 0.75, 1e-6);
    }
}

#[test]

fn graph_implicitly_steps_proc_nodes_and_fanout_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_IMPLICIT_PROC_FANOUT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 0.5, 1e-6);

        assert_near(output[base + 1], 0.5, 1e-6);
    }
}

#[test]

fn graph_delayed_feedback_persists_across_process_calls() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_DELAY_FEEDBACK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut first = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut first, frames)
        .expect("first process should succeed");

    let expected_first = [1.0_f32, 2.0, 3.0, 4.0];

    for (sample, target) in first.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let mut second = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut second, frames)
        .expect("second process should succeed");

    let expected_second = [5.0_f32, 6.0, 7.0, 8.0];

    for (sample, target) in second.iter().zip(expected_second) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_sample_override_for_param_destinations_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PARAM_SAMPLE_OVERRIDE_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![0.1_f32, 0.2, 0.3, 0.4];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (sample, target) in output.iter().zip(input) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_fanout_destinations_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(GRAPH_FANOUT_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, -0.5, 0.0, 2.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [0.5_f32, -0.25, 0.0, 1.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_proc_bundle_destinations_run_for_proc_and_proc_array_slot_sources() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_BUNDLE_FANOUT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = [
        0.25_f32, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_proc_array_indexed_param_destinations_and_output_sources_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        GRAPH_PROC_ARRAY_PARAM_DEST_AND_OUTPUT_SOURCE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in output.as_chunks::<3>().0 {
        assert_near(frame[0], 0.25, 1e-6);

        assert_near(frame[1], 0.75, 1e-6);

        assert_near(frame[2], 0.75, 1e-6);
    }
}

#[test]

fn graph_array_expressions_run_element_wise() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_EXPR_EXAMPLE, frames);

    assert_eq!(in_channels, 4);

    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 4.0, 2.0, 8.0, //
        2.0, 5.0, 4.0, 10.0, //
        3.0, 6.0, 6.0, 12.0, //
        4.0, 7.0, 8.0, 14.0,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        1.0_f32 * 0.5 + 2.0 * 0.25,
        4.0 * 0.5 + 8.0 * 0.25,
        2.0 * 0.5 + 4.0 * 0.25,
        5.0 * 0.5 + 10.0 * 0.25,
        3.0 * 0.5 + 6.0 * 0.25,
        6.0 * 0.5 + 12.0 * 0.25,
        4.0 * 0.5 + 8.0 * 0.25,
        7.0 * 0.5 + 14.0 * 0.25,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_array_delays_persist_and_shift_element_wise() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_DELAY_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    let first_input = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    let mut first_output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &first_input, &mut first_output, frames)
        .expect("first process should succeed");

    let expected_first = [
        0.0_f32, 0.0, //
        1.0, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];

    for (sample, target) in first_output.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let second_input = vec![
        5.0_f32, 50.0, //
        6.0, 60.0, //
        7.0, 70.0, //
        8.0, 80.0,
    ];

    let mut second_output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &second_input, &mut second_output, frames)
        .expect("second process should succeed");

    let expected_second = [
        4.0_f32, 40.0, //
        5.0, 50.0, //
        6.0, 60.0, //
        7.0, 70.0,
    ];

    for (sample, target) in second_output.iter().zip(expected_second) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_scalar_broadcast_to_array_outputs_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_BROADCAST_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 0.25, 1e-6);
    }
}

#[test]

fn graph_receiver_delay_runs_as_one_sample_delay() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_RECEIVER_DELAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut first = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut first, frames)
        .expect("first process should succeed");

    let expected_first = [0.0_f32, 1.0, 1.0, 1.0];

    for (sample, target) in first.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let mut second = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut second, frames)
        .expect("second process should succeed");

    for sample in second {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]

fn graph_slice_sources_route_runtime_channels() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_SLICE_SOURCE_EXAMPLE, frames);

    assert_eq!(in_channels, 4);

    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 10.0, 100.0, 1000.0, //
        2.0, 20.0, 200.0, 2000.0, //
        3.0, 30.0, 300.0, 3000.0, //
        4.0, 40.0, 400.0, 4000.0,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        10.0_f32, 100.0, //
        20.0, 200.0, //
        30.0, 300.0, //
        40.0, 400.0,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn proc_local_graphs_compile_and_run_through_top_level_graphs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_LOCAL_GRAPH_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        10.0_f32, 1.0, //
        20.0, 2.0, //
        30.0, 3.0, //
        40.0, 4.0,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_scalar_broadcast_to_proc_input_arrays_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_INPUT_ARRAY_BROADCAST_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]

fn graph_scalar_broadcast_to_proc_param_arrays_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_PARAM_ARRAY_BROADCAST_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]

fn graph_proc_named_ports_accept_numbered_aliases() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_NAMED_PORT_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]

fn graph_top_level_named_io_accept_numbered_aliases() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_TOP_LEVEL_NAMED_IO_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![0.25_f32, -0.5, 1.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (sample, target) in output.iter().zip(input) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_top_level_io_is_inferred_from_graph_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_TOP_LEVEL_IO_INFERENCE_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![0.5_f32, -0.25, 0.0, 1.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [0.25_f32, -0.125, 0.0, 0.5];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_proc_io_is_inferred_from_proc_graph_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_IO_INFERENCE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]

fn graph_proc_custom_io_names_require_declarations() {
    let parsed = parse_program(GRAPH_PROC_CUSTOM_IO_NAMES_REQUIRE_DECLS_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let errs = analyze(parsed).expect_err("undeclared custom graph proc IO should fail");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("not a declared output"))
            && errs.iter().any(|d| d.message.contains("unknown endpoint")),
        "expected graph undeclared-io diagnostic, got {errs:?}"
    );
}
