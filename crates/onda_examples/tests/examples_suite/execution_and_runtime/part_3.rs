#[test]

fn proc_can_bind_and_read_top_level_buffer() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_BUFFER_MONO_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn proc_buffer_missing_ctor_arg_is_rejected() {
    let parsed =
        parse_program(PROC_BUFFER_MISSING_CTOR_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject proc ctor missing required buffer arg"
    );
}

#[test]

fn proc_ctor_positional_args_are_rejected() {
    let parsed =
        parse_program(PROC_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject positional proc constructor arguments"
    );
}

#[test]

fn nested_proc_ctor_positional_args_are_rejected() {
    let parsed =
        parse_program(PROC_NESTED_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject positional nested proc constructor arguments"
    );
}

#[test]

fn sample_oversample_factor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);

    let (mut proc_instance, proc_in_channels, proc_out_channels) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(proc_in_channels, 0);

    assert_eq!(proc_out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    let mut proc_output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    process_interleaved(&mut proc_instance, &[], &mut proc_output, frames)
        .expect("proc process should succeed");

    for (actual, expected) in output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_factor_is_recorded_in_typed_program() {
    let parsed = parse_program(SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    assert_eq!(typed.sample_oversample_factor, 4);
}

#[test]

fn proc_sample_oversample_factor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);

    let (mut top_level_instance, top_in_channels, top_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_2_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(top_in_channels, 0);

    assert_eq!(top_out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    let mut top_level_output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    process_interleaved(&mut top_level_instance, &[], &mut top_level_output, frames)
        .expect("top-level process should succeed");

    for (actual, expected) in output.iter().zip(top_level_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_factor_32_compiles_and_runs_smoke() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_32_SMOKE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|v| v.is_finite()));

    assert!(output[frames - 1] > output[0]);
}

#[test]

fn proc_sample_oversample_factor_64_compiles_and_runs_smoke() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_FACTOR_64_SMOKE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|v| v.is_finite()));

    assert!(output[frames - 1] > output[0]);
}

#[test]

fn sample_oversample_factor_512_compiles_and_runs_smoke() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_512_SMOKE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|v| v.is_finite()));

    assert!(output[frames - 1] > output[0]);
}

#[test]

fn sample_oversample_invalid_factor_is_rejected() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_INVALID_FACTOR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "invalid oversample factor should be rejected"
    );

    let diags = result.expect_err("expected semantic diagnostics");

    assert!(
        diags.iter().any(|d| {
            d.message.contains("{1,2,4,8,16,32,64,128,256,512}") && d.message.contains("got 3")
        }),
        "expected explicit allowed-factor diagnostic, got: {diags:?}"
    );
}

#[test]

fn sample_oversample_const_expr_factor_is_accepted() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_CONST_EXPR_FACTOR_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("const oversample factor should analyze");

    assert_eq!(typed.sample_oversample_factor, 4);
}

#[test]

fn proc_sample_oversample_namespace_factor_compiles_and_runs() {
    let parsed = parse_program(PROC_SAMPLE_OVERSAMPLE_NAMESPACE_FACTOR_EXAMPLE)
        .expect("parse should succeed");

    let typed = analyze(parsed).expect("namespace oversample factor should analyze");

    assert!(
        typed
            .def_sample_oversample_factors
            .values()
            .any(|&factor| factor == 8),
        "expected lowered proc oversample factor map to contain 8x specialization, got {:?}",
        typed.def_sample_oversample_factors
    );
}

#[test]

fn sample_oversample_non_literal_factor_is_rejected() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_NON_LITERAL_FACTOR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "non-literal oversample factor should be rejected"
    );

    let diags = result.expect_err("expected semantic diagnostics");

    assert!(
        diags.iter().any(|d| {
            d.message
                .contains("compile-time integer constant expression")
                && d.message.contains("{1,2,4,8,16,32,64,128,256,512}")
        }),
        "expected integer-literal diagnostic, got: {diags:?}"
    );
}

#[test]

fn sample_oversample_interpolates_input_reads() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE, frames);

    let (mut proc_instance, proc_in_channels, proc_out_channels) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    assert_eq!(proc_in_channels, 1);

    assert_eq!(proc_out_channels, 1);

    let input = vec![0.0_f32, 1.0, 2.0, 3.0];

    let mut output = vec![0.0_f32; frames];

    let mut proc_output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    process_interleaved(&mut proc_instance, &input, &mut proc_output, frames)
        .expect("proc process should succeed");

    for (actual, expected) in output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_passthrough_preserves_more_high_band_level() {
    let frames = 4096;

    let sample_rate = 48_000.0_f32;

    let freq = sample_rate * 0.2;

    let (mut base_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_PASSTHROUGH_1X_EXAMPLE, frames);

    let (mut over_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_PASSTHROUGH_4X_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| f32::sin(2.0 * std::f32::consts::PI * freq * idx as f32 / sample_rate))
        .collect::<Vec<_>>();

    let mut base_output = vec![0.0_f32; frames];

    let mut over_output = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &input, &mut base_output, frames)
        .expect("base passthrough should succeed");

    process_interleaved(&mut over_instance, &input, &mut over_output, frames)
        .expect("oversampled passthrough should succeed");

    let base_rms = rms_after_skip(&base_output, 512);

    let over_rms = rms_after_skip(&over_output, 512);

    let attenuation_db = 20.0 * f32::log10(over_rms / base_rms);

    assert!(

        attenuation_db > -3.0,

        "expected 4x oversampled passthrough to stay within 3 dB at 9.6 kHz, got {attenuation_db} dB"

    );
}

#[test]

fn sample_oversample_reduces_high_frequency_energy_on_nonlinear_patch() {
    let frames = 64;

    let (mut base_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_BASELINE_EXAMPLE, frames);

    let (mut over_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| if idx % 2 == 0 { 1.0_f32 } else { -1.0_f32 })
        .collect::<Vec<_>>();

    let mut base_output = vec![0.0_f32; frames];

    let mut over_output = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &input, &mut base_output, frames)
        .expect("baseline process should succeed");

    process_interleaved(&mut over_instance, &input, &mut over_output, frames)
        .expect("oversampled process should succeed");

    let high_freq_energy = |samples: &[f32]| -> f32 {
        samples
            .windows(2)
            .map(|w| {
                let d = w[1] - w[0];

                d * d
            })
            .sum::<f32>()
    };

    let base_energy = high_freq_energy(&base_output);

    let over_energy = high_freq_energy(&over_output);

    assert!(

        over_energy < base_energy * 0.5,

        "expected oversampling to reduce high-frequency energy, base={base_energy}, oversampled={over_energy}"

    );
}

#[test]

fn sample_oversample_proc_and_top_level_match_on_nonlinear_patch() {
    let frames = 64;

    let (mut top_level_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let (mut proc_instance, _, _) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| if idx % 2 == 0 { 1.0_f32 } else { -1.0_f32 })
        .collect::<Vec<_>>();

    let mut top_level_output = vec![0.0_f32; frames];

    let mut proc_output = vec![0.0_f32; frames];

    process_interleaved(
        &mut top_level_instance,
        &input,
        &mut top_level_output,
        frames,
    )
    .expect("top-level oversampled process should succeed");

    process_interleaved(&mut proc_instance, &input, &mut proc_output, frames)
        .expect("proc oversampled process should succeed");

    for (actual, expected) in top_level_output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_keeps_proc_sine_pitch_constant() {
    let frames = 48_000;

    let sample_rate = 48_000.0_f32;

    let (mut base_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_1X_EXAMPLE, frames);

    let (mut os2_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_2X_EXAMPLE, frames);

    let (mut os4_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_4X_EXAMPLE, frames);

    let mut out_1x = vec![0.0_f32; frames];

    let mut out_2x = vec![0.0_f32; frames];

    let mut out_4x = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &[], &mut out_1x, frames)
        .expect("1x process should succeed");

    process_interleaved(&mut os2_instance, &[], &mut out_2x, frames)
        .expect("2x process should succeed");

    process_interleaved(&mut os4_instance, &[], &mut out_4x, frames)
        .expect("4x process should succeed");

    let f1 = estimate_positive_zero_cross_frequency(&out_1x, sample_rate);

    let f2 = estimate_positive_zero_cross_frequency(&out_2x, sample_rate);

    let f4 = estimate_positive_zero_cross_frequency(&out_4x, sample_rate);

    assert!(
        (f1 - 50.0).abs() < 1.5,
        "expected ~50 Hz at 1x, got {f1} Hz"
    );

    assert!(
        (f2 - f1).abs() < 1.5,
        "expected 2x oversampling pitch to match 1x, got f1={f1}, f2={f2}"
    );

    assert!(
        (f4 - f1).abs() < 1.5,
        "expected 4x oversampling pitch to match 1x, got f1={f1}, f4={f4}"
    );
}

#[test]

fn proc_sample_oversample_keeps_local_sine_pitch_constant() {
    let frames = 48_000;

    let sample_rate = 48_000.0_f32;

    let (mut base_instance, _, _) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_1X_EXAMPLE, frames);

    let (mut os8_instance, _, _) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_8X_EXAMPLE, frames);

    let mut out_1x = vec![0.0_f32; frames];

    let mut out_8x = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &[], &mut out_1x, frames)
        .expect("1x process should succeed");

    process_interleaved(&mut os8_instance, &[], &mut out_8x, frames)
        .expect("8x process should succeed");

    let f1 = estimate_positive_zero_cross_frequency(&out_1x, sample_rate);

    let f8 = estimate_positive_zero_cross_frequency(&out_8x, sample_rate);

    assert!(
        (f1 - 50.0).abs() < 1.5,
        "expected ~50 Hz at 1x, got {f1} Hz"
    );

    assert!(
        (f8 - f1).abs() < 1.5,
        "expected proc sample 8 pitch to match 1x, got f1={f1}, f8={f8}"
    );
}

#[test]
#[ignore = "perf benchmark; run manually"]
fn sample_oversample_n4_performance_budget_benchmark() {
    const FRAMES: usize = 128;

    const WARMUP_ITERS: usize = 256;

    const TIMED_ITERS: usize = 4096;

    const TARGET_RATIO: f64 = 2.5;

    let (mut baseline, baseline_in_channels, baseline_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_PERF_BASELINE_EXAMPLE, FRAMES);

    let (mut oversampled, os_in_channels, os_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_PERF_N4_EXAMPLE, FRAMES);

    assert_eq!(baseline_in_channels, 1);

    assert_eq!(baseline_out_channels, 1);

    assert_eq!(os_in_channels, 1);

    assert_eq!(os_out_channels, 1);

    let input = (0..FRAMES)
        .map(|idx| ((idx % 97) as f32 / 48.0) - 1.0)
        .collect::<Vec<_>>();

    let mut baseline_output = vec![0.0_f32; FRAMES];

    let mut os_output = vec![0.0_f32; FRAMES];

    let baseline_secs = benchmark_process_runtime(
        &mut baseline,
        &input,
        &mut baseline_output,
        FRAMES,
        WARMUP_ITERS,
        TIMED_ITERS,
    );

    let os_secs = benchmark_process_runtime(
        &mut oversampled,
        &input,
        &mut os_output,
        FRAMES,
        WARMUP_ITERS,
        TIMED_ITERS,
    );

    let ratio = os_secs / baseline_secs.max(f64::EPSILON);

    eprintln!(
        "oversample N=4 runtime ratio: {:.3}x (baseline={:.6}s, os4={:.6}s, frames={}, iters={})",
        ratio, baseline_secs, os_secs, FRAMES, TIMED_ITERS
    );

    if std::env::var("ONDA_ENFORCE_OVERSAMPLE_PERF_BUDGET")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        assert!(
            ratio <= TARGET_RATIO,
            "oversample N=4 runtime ratio {:.3}x exceeded target {:.3}x",
            ratio,
            TARGET_RATIO
        );
    }
}

#[test]

fn proc_array_input_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INPUT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_array_input_from_local_array_symbol_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INPUT_VAR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.3, 1e-6);
    }
}

#[test]

fn proc_array_output_field_read_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_OUTPUT_INDEXED_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_array_param_constructor_args_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_PARAM_CTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

#[test]
fn proc_array_dynamic_index_reads_use_clamped_path() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_DYNAMIC_INDEX_CLAMP_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn proc_array_constant_index_out_of_range_is_rejected() {
    let parsed = parse_program(PROC_ARRAY_CONSTANT_INDEX_OOB_REJECTED_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject out-of-range constant proc-array indexing"
    );
}

#[test]

fn proc_instance_array_indexed_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_field_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_FIELD_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_with_oversampled_callee_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_OVERSAMPLED_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert!(
            sample.is_finite() && *sample > 0.0 && *sample < 2.0,
            "expected finite oversampled dynamic dispatch output in (0,2), got {}",
            sample
        );
    }
}

#[test]

fn proc_instance_array_indexed_field_call_dynamic_index_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_FIELD_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_selects_slot_buffer_binding() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];

    let mut buf2 = vec![0.75_f32; frames];

    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");

    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2.as_mut_ptr().cast::<u8>(),
        buf2.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_alias_call_dynamic_index_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_alias_out_read_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_ALIAS_OUT_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn nested_proc_instance_array_indexed_alias_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        NESTED_PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_len_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_uses_rebound_buffer_on_process_checked() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];

    let mut buf2_old = vec![0.75_f32; frames];

    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");

    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked with old buf2");

    let out_old = decode_planar_f32(&out_bytes);

    for sample in out_old {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked with new buf2");

    let out_new = decode_planar_f32(&out_bytes);

    for sample in out_new {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]
fn checked_end_segment_uses_rebound_proc_array_buffer() {
    let frames = 4;
    let segment_frames = 2;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2_old = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind old buf2");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        0,
        segment_frames,
        PROCESS_BEGIN_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process begin segment with old buf2");

    let mut buf2_new = vec![0.5_f32; frames];
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind new buf2 before end segment");

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        segment_frames,
        segment_frames,
        PROCESS_END_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process end segment after rebind");
    let out = decode_planar_f32(&out_bytes);
    assert_near(out[0], 0.0, 1e-6);
    assert_near(out[1], 0.0, 1e-6);
    assert_near(out[2], 0.5, 1e-6);
    assert_near(out[3], 0.5, 1e-6);
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_uses_validated_rebound_buffer_on_process_unchecked(
) {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];

    let mut buf2_old = vec![0.75_f32; frames];

    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");

    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked with old buf2");

    let out_seed = decode_planar_f32(&out_bytes);

    for sample in out_seed {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    validate_buffers(&mut instance).expect("validate buffers after rebind");

    validate_outputs(&mut instance).expect("validate outputs");

    // MIR buffer arguments are transient symbolic resources. Once validation publishes the
    // rebound host table, unchecked processing observes that current table rather than a
    // pointer-bearing processor cache.
    unsafe {
        process_unchecked(&mut instance, onda_runtime::ExecutionOutput::none())
            .expect("unchecked process after rebind");
    }

    let out_unchecked = decode_planar_f32(&out_bytes);

    for sample in out_unchecked {
        assert_near(sample, 0.5, 1e-6);
    }

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked uses current binding");

    let out_checked = decode_planar_f32(&out_bytes);

    for sample in out_checked {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]
fn prepare_unchecked_process_uses_current_proc_array_buffer_binding() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2_old = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    prepare_unchecked_process(&mut instance).expect("prepare unchecked with old buf2");
    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            frames,
            PROCESS_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("unchecked process with old buf2");
    }
    let out_old = decode_planar_f32(&out_bytes);
    for sample in out_old {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    prepare_unchecked_process(&mut instance).expect("prepare unchecked after rebind");
    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            frames,
            PROCESS_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("unchecked process with refreshed refs");
    }
    let out_new = decode_planar_f32(&out_bytes);
    for sample in out_new {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]

fn nested_proc_instance_array_indexed_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn deep_nested_proc_instance_array_dynamic_index_chain_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEEP_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 50.5, 1e-6);
    }
}

#[test]

fn deeper_nested_proc_instance_array_dynamic_index_chain_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEEPER_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 55.5, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_BROADCAST_CTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_array_literal_arg_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_LITERAL_ARG_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.8, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_array_symbol_arg_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_SYMBOL_ARG_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.8, 1e-6);
    }
}

#[test]

fn untyped_init_array_first_element_type_is_enforced() {
    let parsed = parse_program(UNTYPED_INIT_ARRAY_FIRST_ELEMENT_TYPE_MISMATCH_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject untyped init array literals whose later elements are not assignable to the first element type"

    );
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_mixed_buffer_array_arg_analyzes() {
    let parsed = parse_program(PROC_INSTANCE_ARRAY_BROADCAST_CTOR_MIXED_BUFFER_ARRAY_ARG_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_ok(),

        "semantic analysis should allow broadcast processor-array ctor with scalar and per-slot buffer arguments"

    );
}

#[test]

fn nested_proc_init_untyped_array_symbol_arg_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INIT_UNTYPED_ARRAY_SYMBOL_ARG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_const_expr_size_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn nested_proc_instance_array_const_expr_size_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_initializer_arity_is_rejected() {
    let parsed = parse_program(TOP_LEVEL_PROC_INSTANCE_ARRAY_INIT_ARITY_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject mismatched constructor count for top-level processor arrays"

    );
}

#[test]

fn proc_nested_state_persists_across_samples() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_NESTED_STATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.25, 1e-6);

    assert_near(output[1], 0.5, 1e-6);

    assert_near(output[2], 0.75, 1e-6);

    assert_near(output[3], 1.0, 1e-6);
}

#[test]

fn proc_deep_nested_state_persists_across_samples() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DEEP_NESTED_STATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.5, 1e-6);

    assert_near(output[1], 1.0, 1e-6);

    assert_near(output[2], 1.5, 1e-6);

    assert_near(output[3], 2.0, 1e-6);
}

#[test]

fn proc_deep_nested_buffer_binding_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DEEP_NESTED_BUFFER_BIND_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![3.0_f32, 4.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [3.0_f32, 4.0, 4.0, 4.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn top_level_init_block_is_optional() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_OPTIONAL_INIT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn struct_method_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_METHOD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-5);

    assert_near(output[1], 0.0, 1e-5);

    assert_near(output[2], -1.0, 1e-5);

    assert_near(output[3], 0.0, 1e-5);
}

#[test]

fn struct_method_data_write_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_METHOD_DATA_WRITE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn struct_method_requires_self_param() {
    let parsed =
        parse_program(STRUCT_METHOD_SELF_REQUIRED_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject struct method without self first parameter"
    );
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_gain_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        GAIN,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

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

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_sine_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        SINE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

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

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_one_pole_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        ONE_POLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

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

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_if_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        IF_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

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

#[cfg(feature = "llvm-orc")]
#[test]
fn scoped_branch_and_loop_locals_do_not_reuse_a_later_bool_binding() {
    let source = r#"
outs:
  out1

sample:
  if in1 > 0.0:
    temp = 0.0
  for i in 0..1:
    temp = f32(i)
  temp = true
  if temp:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, -1.0, 0.5, -0.5];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_eq!(output, vec![1.0; frames]);
}

#[cfg(feature = "llvm-orc")]
#[test]
fn comparison_literal_uses_the_concrete_f32_peer_width_at_runtime() {
    let source = r#"
outs:
  out1

sample:
  x: f32 = f32(16777216.0)
  if x == 16777217.0:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_eq!(output, vec![1.0; frames]);
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_for_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        FOR_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_def_call_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DEF_CALL_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_def_return_exits_early() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DEF_EARLY_RETURN_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_struct_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        STRUCT_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_struct_reserved_method_names_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        RESERVED_METHOD_NAMES_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn analyze_supports_typed_init_generic_struct_ctor_decl() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data::Data<f32> = std::data::Data()

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(result.is_ok(), "semantic analysis should succeed");
}

#[test]

fn analyze_supports_typed_init_generic_struct_default_ctor_decl() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data::Data<f32>

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(result.is_ok(), "semantic analysis should succeed");
}

#[test]

fn analyze_rejects_typed_init_generic_struct_default_ctor_decl_without_type_args() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data::Data

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject generic struct ctor without type args when inference cannot resolve"

    );
}

#[test]

fn analyze_rejects_typed_init_namespace_instantiated_struct_default_ctor_decl_without_type_args() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data<SR, 1>::Data

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject generic struct ctor without type args when inference cannot resolve"

    );
}
