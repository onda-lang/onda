#[test]

fn builtin_math_typed_overloads_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_MATH_TYPED_OVERLOADS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 2.0, 1e-4);

        assert_near(output[base + 1], 2.0, 1e-4);
    }
}

#[test]

fn builtin_intrinsics_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_INTRINSICS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let x = (-0.5_f32).abs()
        + (0.0_f32).cos()
        + (4.0_f32).sqrt()
        + (0.0_f32).exp()
        + (1.0_f32).exp().ln();

    let y = 2.0_f32.powf(3.0) + 3.0_f32.min(4.0) + 3.0_f32.max(4.0) + (2.0_f32).mul_add(3.0, 4.0);

    let z = (1.8_f32).floor() + (1.2_f32).ceil() + (1.6_f32).round() + (1.6_f32).trunc();

    let expected = x + y + z;

    for sample in &output {
        assert_near(*sample, expected, 1e-4);
    }
}

#[test]

fn stdlib_math_is_auto_imported() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MATH_AUTO_IMPORT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn generic_calls_are_specialized_inside_index_and_slice_coordinates() {
    let source = r#"
outs:
  out1
init:
  constructed: f32[1] = [clamp(9.0, 0.0, 6.0)]
sample:
  values: f32[7] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
  selected = values[clamp(8, 0, 6)]
  sliced = values[clamp(1, 0, 6):clamp(2, 0, 7)]
  out1 = selected + sliced[0] + constructed[0]
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

#[test]
fn nested_proc_event_updates_private_param_and_runs_bound_hook() {
    let source = r#"
proc Child:
  params:
    private value = 0.0 => update_cached
  init:
    cached = 0.0
  def update_cached():
    cached = value * 2.0
  event set(value_v: f32):
    value = value_v
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    child = Child()
  outs:
    out1
  sample:
    child.set(0.375)
    out1 = child()

init:
  parent = Parent()
sample:
  out1 = parent()
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn stdlib_math_auto_import_allows_local_symbol_override() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MATH_LOCAL_SYMBOL_WINS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]
fn stdlib_random_generic_rng_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_RANDOM_GENERIC_RNG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut out1_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];
    let mut out2_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];
    let mut out3_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    bind_output(&mut instance, 0, out1_bytes.as_mut_ptr(), out1_bytes.len()).expect("bind out1");
    bind_output(&mut instance, 1, out2_bytes.as_mut_ptr(), out2_bytes.len()).expect("bind out2");
    bind_output(&mut instance, 2, out3_bytes.as_mut_ptr(), out3_bytes.len()).expect("bind out3");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out1 = decode_planar_f64(&out1_bytes);
    let out2 = decode_planar_f64(&out2_bytes);
    let out3 = decode_planar_f64(&out3_bytes);

    let next_state = |state: i64| -> i64 {
        (state
            .wrapping_mul(1_103_515_245_i64)
            .wrapping_add(12_345_i64))
            & 2_147_483_647_i64
    };
    let next_unit = |state: i64| -> f64 { ((state as i32 & 2147483647) as f64) / 2147483647.0_f64 };

    let mut state = 123_i64;
    let mut expected = Vec::with_capacity(frames);
    for _ in 0..frames {
        state = next_state(state);
        let v1 = next_unit(state);
        state = next_state(state);
        let v2 = next_unit(state) * 2.0 - 1.0;
        state = next_state(state);
        let v3 = -2.0 + 4.0 * next_unit(state);
        expected.push((v1, v2, v3));
    }

    for frame in 0..frames {
        assert!(
            (out1[frame] - expected[frame].0).abs() <= 1e-6,
            "expected {} ~= {}",
            out1[frame],
            expected[frame].0
        );
        assert!(
            (out2[frame] - expected[frame].1).abs() <= 1e-6,
            "expected {} ~= {}",
            out2[frame],
            expected[frame].1
        );
        assert!(
            (out3[frame] - expected[frame].2).abs() <= 1e-6,
            "expected {} ~= {}",
            out3[frame],
            expected[frame].2
        );
    }
}

#[test]

fn stdlib_buffer_read_mono_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_READ_MONO_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![1.0_f32, 2.0, 3.0, 4.0];

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

    for sample in &out {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn stdlib_buffer_read_linear_and_cubic_with_channel_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_INTERP_STEREO_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in &out {
        assert_near(*sample, 42.0, 1e-6);
    }
}

#[test]

fn stdlib_buffer_is_auto_imported_for_arrays_and_buffers() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_AUTO_IMPORT_ARRAY_AND_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0, 30.0, 40.0];

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

    for sample in &out {
        assert_near(*sample, 62.0, 1e-6);
    }
}

#[test]

fn stdlib_lookup_write_array_and_buffer_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_LOOKUP_WRITE_ARRAY_AND_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![0.0_f32; 4];

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

    for sample in &out {
        assert_near(*sample, 6.5, 1e-6);
    }
}

#[test]

fn floor_fract_wrap_numeric_behavior_is_stable() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(FLOOR_FRACT_WRAP_NUMERIC_BEHAVIOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.75, 1e-6);
    }
}

#[test]

fn builtin_int_intrinsics_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_INT_INTRINSICS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 17.0, 1e-6);
    }
}

#[test]

fn float_only_builtin_rejects_integer_arguments() {
    let parsed =
        parse_program(BUILTIN_FLOAT_ONLY_TYPE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject integer argument for float-only builtin"
    );
}

#[test]

fn data_capacity_supports_compile_time_constants() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DATA_CONST_CAPACITY_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 16_000.0,

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
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn data_ctor_capacity_supports_compile_time_constants() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DATA_CTOR_CONST_CAPACITY_EXAMPLE,
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

#[test]

fn block_size_constant_is_available_in_init_and_block() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BLOCK_SIZE_CONST_EXAMPLE,
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
        assert_near(*sample, (frames as f32) * 2.0, 1e-6);
    }
}

#[test]

fn block_size_aliases_are_available() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BLOCK_SIZE_ALIASES_CONST_EXAMPLE,
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
        assert_near(*sample, frames as f32, 1e-6);
    }
}

#[test]

fn block_executes_once_per_process_call() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_EXEC_ONCE_PER_PROCESS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("first process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("second process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn block_assigned_scalar_is_visible_in_nested_sample() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_SCALAR_VISIBLE_IN_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    // We only need to assert the program compiles/runs and generates non-zero audio.

    assert!(output.iter().any(|v| v.abs() > 1e-6));
}

#[test]

fn block_cannot_access_outputs() {
    let parsed = parse_program(BLOCK_IO_FORBIDDEN_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject output access in block"
    );
}

#[test]

fn builtin_const_assignment_is_rejected() {
    let parsed = parse_program(BUILTIN_CONST_ASSIGN_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject builtin constant assignment"
    );
}

#[test]

fn builtin_const_lowercase_assignment_is_rejected() {
    let parsed =
        parse_program(BUILTIN_CONST_ASSIGN_LOWERCASE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject lowercase builtin constant assignment"
    );
}

#[test]

fn typed_narrowing_assignment_is_rejected() {
    let parsed =
        parse_program(TYPED_NARROWING_ASSIGNMENT_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject implicit i64->f32 narrowing"
    );
}

#[test]

fn if_condition_must_be_bool() {
    let parsed = parse_program(IF_CONDITION_BOOL_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject non-bool if condition"
    );
}

#[test]

fn init_if_branch_locals_do_not_introduce_state() {
    let parsed =
        parse_program(IF_BRANCH_TYPE_CONFLICT_ERROR_EXAMPLE).expect("parse should succeed");

    assert!(
        analyze(parsed).is_ok(),
        "branch-local init assignments should stay local and not introduce state"
    );
}

#[test]

fn typed_data_primitive_elements_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_ELEM_PRIMITIVES_OK_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");

    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "typed array elements should preserve runtime values across primitive types"
    );
}

#[test]

fn typed_data_struct_scalar_primitives_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_STRUCT_SCALAR_PRIMITIVES_OK_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");

    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "Struct[N] should support all primitive scalar field types"
    );
}

#[test]

fn data_index_must_be_numeric() {
    let parsed = parse_program(DATA_BOOL_INDEX_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject bool array index"
    );
}

#[test]

fn data_constant_out_of_range_index_is_rejected_in_codegen() {
    let parsed = parse_program(DATA_CONST_OOB_INDEX_ERROR_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let result = lower_typed_and_jit(
        typed,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: 64,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert!(
        result.is_err(),
        "codegen should reject out-of-range constant array index"
    );
}

#[test]

fn def_return_type_is_inferred_from_return_expression() {
    let parsed = parse_program(DEF_RETURN_F64_INFERENCE_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let mydef = typed
        .defs
        .iter()
        .find(|d| d.name == "mydef")
        .expect("mydef should be present");

    assert_eq!(
        mydef.return_ty,
        onda_semantics::ReturnType::Scalar(PrimitiveType::F64)
    );
}

#[test]

fn def_monomorphizes_from_call_arguments_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_MONOMORPHIZES_FROM_CALL_ARGUMENTS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn generic_def_infers_scalar_t_from_concrete_call_return_compile_and_run() {
    let src = r#"

outs:

  out1: i64



def make(flag: f32):

  return i64(42)



def id<T>(x: T):

  return x



sample:

  out1 = id(make(0.0))

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 1);

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 42);
    }
}

#[test]

fn def_monomorphizes_multiple_specializations_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEF_MONOMORPHIZES_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.5, 1e-6);
    }
}

#[test]

fn non_generic_def_rejects_type_args() {
    let parsed =
        parse_program(NON_GENERIC_DEF_WITH_TYPE_ARGS_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic defs"
    );
}

#[test]

fn generic_def_parses_successfully() {
    let parsed = parse_program(
        r#"

outs { out1 }

def identity<T>(x: T) {

  return x

}

sample { out1 = identity(1.5) }

"#,
    );

    assert!(
        parsed.is_ok(),
        "parser should accept generic type params on defs"
    );
}

#[test]

fn generic_struct_ctor_with_explicit_type_args_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_EXPLICIT_TYPE_ARGS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_infers_type_args_from_arguments() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_MISSING_TYPE_ARGS_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_infers_type_args_from_variable_arguments() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_INFER_FROM_VAR_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_defaults_unresolved_inference_to_f32() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_UNRESOLVED_INFERENCE_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_rejects_type_arg_arity_mismatch() {
    let parsed =
        parse_program(GENERIC_STRUCT_TYPE_ARG_ARITY_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject generic struct ctor calls with wrong type argument count"
    );
}

#[test]

fn non_generic_struct_ctor_rejects_type_args() {
    let parsed = parse_program(NON_GENERIC_STRUCT_WITH_TYPE_ARGS_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic struct ctors"
    );
}

#[test]

fn generic_struct_multiple_specializations_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn generic_struct_array_field_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_ARRAY_FIELD_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn generic_struct_method_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_METHOD_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn struct_method_local_int_inference_matches_def_for_bitwise_ops() {
    let src = r#"

struct Bits<T>:

  def run(self, n: i32):

    bits = 0

    value = n

    while (value > 1):

      value = value >> 1

      bits = bits + 1

    return f32(bits)

outs { out1 }

init:

  b = Bits<f32>()

sample:

  out1 = b.run(8)

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn struct_method_untyped_numeric_calls_compile_and_run() {
    let src = r#"

struct Math:

  def mix(self, x, y):

    return x * y + x



outs { out1 }

init:

  m = Math()

sample:

  a = m.mix(f32(1.5), f32(2.0))

  b = f32(m.mix(f64(1.25), f64(4.0)))

  out1 = a + b

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.75, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_with_explicit_type_args_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EXPLICIT_TYPE_ARGS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_infers_type_args_from_arguments() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_MISSING_TYPE_ARGS_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_infers_type_args_from_defaults() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_DEFAULT_ONLY_INFERENCE_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_infers_array_generic_type_from_array_variable() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_ARRAY_INFER_FROM_ARRAY_VAR_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_defaults_unresolved_inference_to_f32() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_UNRESOLVED_INFERENCE_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_rejects_type_arg_arity_mismatch() {
    let parsed =
        parse_program(GENERIC_PROC_TYPE_ARG_ARITY_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject generic proc ctor calls with wrong type argument count"
    );
}

#[test]

fn non_generic_proc_ctor_rejects_type_args() {
    let parsed =
        parse_program(NON_GENERIC_PROC_WITH_TYPE_ARGS_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic proc ctors"
    );
}

#[test]

fn generic_proc_multiple_specializations_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn generic_proc_array_decl_types_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_ARRAY_DECL_TYPES_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn generic_proc_init_typed_array_generic_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_INIT_TYPED_ARRAY_GENERIC_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn generic_proc_buffer_decl_type_analyzes_and_codegen_compiles() {
    let parsed = parse_program(GENERIC_PROC_BUFFER_DECL_TYPE_COMPILES_EXAMPLE)
        .expect("parse should succeed");

    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 64,
        },
    )
    .expect("semantic analysis should succeed");

    let result = lower_typed_and_jit(
        typed,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: 64,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert!(
        result.is_ok(),
        "codegen should succeed for generic proc buffer<T> specialization"
    );
}

#[test]

fn proc_state_struct_ctor_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_STATE_STRUCT_CTOR_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_state_generic_struct_ctor_with_explicit_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_STATE_GENERIC_STRUCT_CTOR_EXPLICIT_TYPE_ARGS_OK_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_state_generic_struct_ctor_infers_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_STATE_GENERIC_STRUCT_CTOR_INFERRED_TYPE_ARGS_OK_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn first_assignment_uses_def_return_type_and_alias_keeps_type() {
    let parsed = parse_program(FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE)
        .expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    assert_eq!(state_type_of(&typed, "x"), Some(PrimitiveType::F64));

    assert_eq!(state_type_of(&typed, "z"), Some(PrimitiveType::F64));

    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn first_assignment_from_int_literal_stays_i32() {
    let parsed =
        parse_program(FIRST_ASSIGNMENT_INT_IS_STICKY_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject implicit f32 assignment after x = 0 infers i32"
    );
}

#[test]

fn negative_literals_and_generic_negation_preserve_scalar_types() {
    let parsed = parse_program(NEGATIVE_SCALAR_INFERENCE_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");

    assert_eq!(
        state_type_of(&typed, "inferred_i32"),
        Some(PrimitiveType::I32)
    );
    assert_eq!(
        state_type_of(&typed, "inferred_i64"),
        Some(PrimitiveType::I64)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_i32"),
        Some(PrimitiveType::I32)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_i64"),
        Some(PrimitiveType::I64)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_f32"),
        Some(PrimitiveType::F32)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_f64"),
        Some(PrimitiveType::F64)
    );

    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NEGATIVE_SCALAR_INFERENCE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, -18.0, 1e-6);
    }

    let invalid_bool = parse_program("init { value = -true }\nsample { out1 = 0.0 }")
        .expect("boolean negation should parse before type checking");
    assert!(
        analyze(invalid_bool).is_err(),
        "unary minus must remain invalid for bool"
    );

    let declarations = parse_program(
        r#"
ins:
  inferred_input = -1
  i32_input: i32 = -1
  i64_input: i64 = -1
  f32_input: f32 = -1
  f64_input: f64 = -1

params:
  inferred_param = -1
  i32_param: i32 = -1
  i64_param: i64 = -1
  f32_param: f32 = -1
  f64_param: f64 = -1

sample:
  out1 = 0.0
"#,
    )
    .expect("negative input and parameter defaults should parse");
    let declarations =
        analyze(declarations).expect("negative input and parameter defaults should analyze");

    assert_eq!(
        declarations.in_types.get("inferred_input"),
        Some(&PrimitiveType::F32),
        "untyped inputs retain the language's f32 input default"
    );
    for (name, expected) in [
        ("i32_input", PrimitiveType::I32),
        ("i64_input", PrimitiveType::I64),
        ("f32_input", PrimitiveType::F32),
        ("f64_input", PrimitiveType::F64),
    ] {
        assert_eq!(declarations.in_types.get(name), Some(&expected));
    }
    assert_eq!(
        declarations.param_types.get("inferred_param"),
        Some(&PrimitiveType::I32),
        "untyped parameter defaults infer from the negative integer literal"
    );
    for (name, expected) in [
        ("i32_param", PrimitiveType::I32),
        ("i64_param", PrimitiveType::I64),
        ("f32_param", PrimitiveType::F32),
        ("f64_param", PrimitiveType::F64),
    ] {
        assert_eq!(declarations.param_types.get(name), Some(&expected));
    }
}

#[test]

fn proc_first_assignment_uses_def_return_type() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_FIRST_ASSIGNMENT_FROM_DEF_RETURN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn typed_widening_assignment_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_WIDENING_ASSIGNMENT_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_param_bind_hooks_compile_and_run() {
    let frames = 4;
    let cases = [
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 4.0)
sample:
  out1 = v()
"#,
            2.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 0.0)
  v.gain = 0.25
sample:
  out1 = v()
"#,
            0.5,
        ),
        (
            r#"
proc Voice<T>:
  params:
    gain: T = 0.0 => update
  init:
    cached = 0.0
  def update():
    cached = f32(gain) * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice<f64>(gain = f64(0.25))
sample:
  out1 = v()
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def apply(x: f32):
    cached = x * 2.0
  def update():
    apply(gain)
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 0.25)
sample:
  out1 = v()
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
  voices[0].gain = 0.25
  voice = voices[1]
  voice.gain = 0.5
sample:
  out1 = voices[0]() + voices[1]()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
  for i in 0..2:
    voices[i].gain = f32(i + 1) * 0.25
sample:
  out1 = voices[0]() + voices[1]()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 0.25)
  v.init(gain = 0.75)
sample:
  out1 = v()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    a = 0.0 => mark
    b = 0.0 => mark
  init:
    updates = 0.0
  def mark():
    updates = updates + 1.0
  outs:
    out1
  sample:
    out1 = updates

outs:
  out1
init:
  v = Voice(a = 1.0, b = 2.0)
  v.a = 3.0
  v.b = 4.0
sample:
  out1 = v()
"#,
            4.0,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    child = Child()
  def update():
    child.gain = gain
  outs:
    out1
  sample:
    out1 = child()

outs:
  out1
init:
  p = Parent(gain = 4.0)
sample:
  out1 = p()
"#,
            2.0,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    child = Child()
  def push_child():
    child.gain = gain
  def update():
    push_child()
  outs:
    out1
  sample:
    out1 = child()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#,
            0.5,
        ),
        (
            r#"
proc Leaf:
  params:
    gain = 0.0 => update
  init:
    cached = -10.0
  def update():
    cached = gain * 4.0
  outs:
    out1
  sample:
    out1 = cached

proc Mid:
  params:
    gain = 0.0 => update
  init:
    leaf = Leaf()
  def update():
    leaf.gain = gain + 0.25
  outs:
    out1
  sample:
    out1 = leaf()

proc Parent:
  params:
    gain = 0.0 => update
  init:
    mid = Mid()
  def update():
    mid.gain = gain + 0.25
  outs:
    out1
  sample:
    out1 = mid()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#,
            3.0,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    children: Child[2] = Child()
  def update():
    for i in 0..2:
      children[i].gain = gain + f32(i) * 0.25
  outs:
    out1
  sample:
    out1 = children[0]() + children[1]()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(gain = 0.25)
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(gain = 0.25) + v(gain = 0.5)
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  ins:
    x = 0.0
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = x + cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v(v(), gain = 2.0)
"#,
            3.0,
        ),
        (
            r#"
proc Voice:
  ins:
    x = 0.0
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = x + cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v(gain = 2.0, x = v())
"#,
            4.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
sample:
  out1 = voices[0](gain = 0.25) + voices[1](gain = 0.5)
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
sample:
  mix = 0.0
  for i in 0..2:
    mix = mix + voices[i](gain = f32(i + 1) * 0.25)
  out1 = mix
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v() + v(gain = 2.0)
"#,
            3.0,
        ),
        (
            r#"
proc Voice:
  ins:
    x = 0.0
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = x + cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(x = 0.5, gain = 0.25)
"#,
            0.75,
        ),
        (
            r#"
proc Pair:
  params:
    gains: f32[2] = [0.0, 0.0]
  outs:
    out1
  sample:
    out1 = gains[0] + gains[1]

outs:
  out1
init:
  p = Pair()
sample:
  out1 = p(gains = [0.25, 0.75])
"#,
            1.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
sample:
  mix = 0.0
  for i in 0..2:
    voice = voices[i]
    mix = mix + voice(gain = f32(i + 1) * 0.25)
  out1 = mix
"#,
            1.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    child = Child()
  outs:
    out1
  sample:
    out1 = child(gain = 0.25)

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            0.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    children: Child[2] = Child()
  outs:
    out1
  sample:
    mix = 0.0
    for i in 0..2:
      mix = mix + children[i](gain = f32(i + 1) * 0.25)
    out1 = mix

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            1.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    children: Child[2] = Child()
  outs:
    out1
  sample:
    mix = 0.0
    for i in 0..2:
      child = children[i]
      mix = mix + child(gain = f32(i + 1) * 0.25)
    out1 = mix

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            1.5,
        ),
        (
            r#"
proc Leaf:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Mid:
  params:
    gain = 0.0
  init:
    leaf = Leaf()
  outs:
    out1
  sample:
    out1 = leaf(gain = gain)

proc Parent:
  init:
    mid = Mid()
  outs:
    out1
  sample:
    mid.gain = 0.25
    out1 = mid()

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            0.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    children: Child[2] = Child()
  def update():
    for i in 0..2:
      children[i].gain = gain + f32(i)
  outs:
    out1
  sample:
    out1 = children[0]() + children[1]()

outs:
  out1
init:
  p = Parent(gain = 2.0)
sample:
  out1 = p()
"#,
            4.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
graph:
  0.25 >> v.gain
  v.out1 >> out1
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
graph:
  0.25 >> voices[0].gain
  0.5 >> voices[1].gain
  voices[0].out1 + voices[1].out1 >> out1
"#,
            1.5,
        ),
    ];

    for (case_idx, (src, expected)) in cases.into_iter().enumerate() {
        let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

        assert_eq!(in_channels, 0);
        assert_eq!(out_channels, 1);

        let mut output = vec![0.0_f32; frames];

        process_interleaved(&mut instance, &[], &mut output, frames)
            .expect("process should succeed");

        for (sample_idx, sample) in output.iter().enumerate() {
            assert!(
                (*sample - expected).abs() <= 1e-6,
                "case {case_idx}, sample {sample_idx}: expected {sample} ~= {expected}"
            );
        }
    }
}

#[test]

fn typed_init_f64_state_preserves_precision() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_INIT_F64_PRESERVES_PRECISION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn typed_init_i64_state_preserves_value() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_INIT_I64_PRESERVES_VALUE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn struct_default_i64_preserves_large_value() {
    let src = r#"

struct Box {

  x: i64 = 9007199254740993

}

outs { out1 }

init {

  b = Box()

}

sample {

  if (b.x == i64(9007199254740993)) { out1 = 1.0 } else { out1 = 0.0 }

}

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_default_i64_preserves_large_value() {
    let src = r#"

outs { out1 }

def exact(x: i64 = 9007199254740993) {

  if (x == i64(9007199254740993)) {

    return 1.0

  } else {

    return 0.0

  }

}

sample {

  out1 = exact()

}

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn typed_block_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_BLOCK_DECLARATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn typed_sample_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_SAMPLE_DECLARATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn typed_def_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DEF_DECLARATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn typed_i32_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_I32_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 100.0, 1e-6);
    }
}

#[test]

fn typed_f64_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_F64_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn typed_i64_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_I64_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 100.0, 1e-6);
    }
}

#[test]

fn typed_bool_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_BOOL_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_can_infer_duck_typed_mono_buffer_param() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![2.0_f32, 4.0];

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

    let expected = [2.0_f32, 4.0, 4.0, 4.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn def_duck_typed_buffer_inference_propagates_through_def_calls() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_PROPAGATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![1.5_f32, 3.0];

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

    let expected = [1.5_f32, 3.0, 3.0, 3.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn def_duck_typed_buffer_param_allows_mixed_element_types() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_MIXED_ELEM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut a = vec![1.0_f32];

    bind_buffer(
        &mut instance,
        0,
        a.as_mut_ptr().cast::<u8>(),
        a.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind f32 buffer");

    let mut b = vec![2.0_f64];

    bind_buffer(
        &mut instance,
        1,
        b.as_mut_ptr().cast::<u8>(),
        b.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind f64 buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]

fn def_indexable_param_accepts_array_and_buffer_call_sites() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_INDEXABLE_ARG_ARRAY_AND_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0, 30.0, 40.0];

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

    for sample in out {
        assert_near(sample, 22.0, 1e-6);
    }
}

#[test]

fn def_indexable_param_supports_two_dimensional_buffer_indexing() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_INDEXABLE_ARG_STEREO_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 30.0, 1e-6);
    }
}

#[test]

fn proc_single_out_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SINGLE_OUT_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_single_out_field_access_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SINGLE_OUT_FIELD_ACCESS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_multi_out_call_field_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_MULTI_OUT_CALL_FIELD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]

fn proc_multi_out_field_alias_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_MULTI_OUT_FIELD_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]

fn proc_param_mutation_is_immediate() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_PARAM_MUTATION_IMMEDIATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn proc_init_block_is_optional() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_OPTIONAL_INIT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_state_typed_in_init_keeps_type_in_sample() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_TYPED_STATE_PRESERVED_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    assert_near(output[1], 2.0, 1e-6);

    assert_near(output[2], 3.0, 1e-6);

    assert_near(output[3], 4.0, 1e-6);
}

#[test]

fn proc_i32_array_increment_keeps_integer_inference() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_I32_ARRAY_INCREMENT_PRESERVED_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    assert_near(output[1], 2.0, 1e-6);

    assert_near(output[2], 3.0, 1e-6);

    assert_near(output[3], 4.0, 1e-6);
}

#[test]

fn proc_data_len_method_matches_top_level_behavior() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DATA_LEN_METHOD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn proc_block_wraps_sample_once_per_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_BLOCK_WRAPS_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output_a, frames).expect("process should succeed");

    for sample in &output_a {
        assert_near(*sample, 6.0, 1e-6);
    }

    let mut output_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output_b, frames).expect("process should succeed");

    for sample in &output_b {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]
fn checked_segment_processing_runs_top_level_block_hooks_once_per_logical_block() {
    let frames = 4;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        0,
        segment_frames,
        PROCESS_BEGIN_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process first segment");
    let first = decode_planar_f32(&out_bytes);
    assert_near(first[0], 100.0, 1e-6);
    assert_near(first[1], 100.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        segment_frames,
        segment_frames,
        PROCESS_END_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process final segment");
    let second = decode_planar_f32(&out_bytes);
    assert_near(second[0], 0.0, 1e-6);
    assert_near(second[1], 0.0, 1e-6);
    assert_near(second[2], 100.0, 1e-6);
    assert_near(second[3], 100.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        0,
        frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process next block");
    let next = decode_planar_f32(&out_bytes);
    for sample in next {
        assert_near(sample, 201.0, 1e-6);
    }
}

#[test]
fn unchecked_segment_processing_runs_top_level_block_hooks_once_per_logical_block() {
    let frames = 4;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    prepare_unchecked_process(&mut instance).expect("prepare unchecked process");

    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            segment_frames,
            PROCESS_BEGIN_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("process first unchecked segment");
    }
    let first = decode_planar_f32(&out_bytes);
    assert_near(first[0], 100.0, 1e-6);
    assert_near(first[1], 100.0, 1e-6);

    out_bytes.fill(0);
    unsafe {
        process_unchecked_segment(
            &mut instance,
            segment_frames,
            segment_frames,
            PROCESS_END_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("process final unchecked segment");
    }
    let second = decode_planar_f32(&out_bytes);
    assert_near(second[0], 0.0, 1e-6);
    assert_near(second[1], 0.0, 1e-6);
    assert_near(second[2], 100.0, 1e-6);
    assert_near(second[3], 100.0, 1e-6);

    out_bytes.fill(0);
    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            frames,
            PROCESS_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("process next unchecked block");
    }
    let next = decode_planar_f32(&out_bytes);
    for sample in next {
        assert_near(sample, 201.0, 1e-6);
    }
}

#[test]
fn checked_segment_processing_uses_start_frame_for_io_addressing() {
    let frames = 4;
    let segment_start = 2;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
ins { in1 }
outs { out1 }
sample {
  out1 = in1 + 10.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = [1.0_f32, 2.0, 3.0, 4.0];
    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_input(
        &mut instance,
        0,
        input.as_ptr().cast::<u8>(),
        input.len() * std::mem::size_of::<f32>(),
    )
    .expect("bind input");
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        segment_start,
        segment_frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process segment");
    let output = decode_planar_f32(&out_bytes);
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    assert_near(output[2], 13.0, 1e-6);
    assert_near(output[3], 14.0, 1e-6);
}

#[test]
fn checked_oversampled_segment_processing_uses_start_frame_for_io_addressing() {
    let frames = 4;
    let segment_start = 2;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
outs { out1 }
sample 2 {
  out1 = 7.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        segment_start,
        segment_frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process oversampled segment");
    let output = decode_planar_f32(&out_bytes);
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    assert!(
        output[2].abs() > 1e-6,
        "expected oversampled segment to write frame 2, got {output:?}"
    );
    assert!(
        output[3].abs() > 1e-6,
        "expected oversampled segment to write frame 3, got {output:?}"
    );
}

#[test]
fn segmented_dynamic_proc_array_hooks_span_one_logical_block() {
    let frames = 4;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
proc Voice {
  outs { out1, pre, post }
  init {
    pre_count = 0.0
    post_count = 0.0
  }
  block {
    pre_count = pre_count + 1.0
    sample {
      out1 = 0.0
      pre = pre_count
      post = post_count
    }
    post_count = post_count + 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 1
  i: i32 = 0
}
sample {
  if i < 2 {
    out1 = 0.0
  } else {
    voices[idx]()
    v1 = voices[1]
    out1 = v1.pre * 100.0 + v1.post
  }
  i = i + 1
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        0,
        segment_frames,
        PROCESS_BEGIN_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process inactive first segment");
    let first = decode_planar_f32(&out_bytes);
    assert_near(first[0], 0.0, 1e-6);
    assert_near(first[1], 0.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        segment_frames,
        segment_frames,
        PROCESS_END_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process active final segment");
    let second = decode_planar_f32(&out_bytes);
    assert_near(second[0], 0.0, 1e-6);
    assert_near(second[1], 0.0, 1e-6);
    assert_near(second[2], 100.0, 1e-6);
    assert_near(second[3], 100.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        0,
        frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process next block");
    let next = decode_planar_f32(&out_bytes);
    for sample in next {
        assert_near(sample, 201.0, 1e-6);
    }
}

#[test]

fn proc_without_block_has_only_step_entrypoint() {
    let parsed = parse_program(PROC_OPTIONAL_INIT_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let def_names = typed
        .defs
        .iter()
        .map(|d| d.name.as_str())
        .collect::<std::collections::HashSet<_>>();

    assert!(def_names.contains("NoInitProc.__onda_proc_step"));

    assert!(!def_names.contains("NoInitProc.__onda_proc_block_pre"));

    assert!(!def_names.contains("NoInitProc.__onda_proc_block_post"));
}

#[test]

fn proc_nested_block_runs_when_outer_has_no_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn proc_outer_without_user_block_gets_effective_block_entrypoints_when_needed() {
    let parsed =
        parse_program(PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let def_names = typed
        .defs
        .iter()
        .map(|d| d.name.as_str())
        .collect::<std::collections::HashSet<_>>();

    assert!(def_names.contains("OuterProc.__onda_proc_block_pre"));

    assert!(def_names.contains("OuterProc.__onda_proc_block_post"));
}

#[test]

fn proc_array_dynamic_index_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_EXAMPLE,
        frames,
    );

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

fn proc_array_dynamic_index_assignment_call_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE,
        frames,
    );

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

fn nested_proc_array_dynamic_index_assignment_call_runs_block_hooks_only_for_active_slot_per_block()
{
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        NESTED_PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE,
        frames,
    );

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

fn proc_array_dynamic_index_block_hooks_use_same_clamped_slot_for_guard_and_call() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_CLAMPED_INDEX_CONSISTENCY_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 100.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 201.0, 1e-6);
}

#[test]

fn proc_array_dynamic_loop_scalar_sugar_into_block_proc_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    mix = mix + voices[i]()

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop scalar sugar");
}

#[test]

fn proc_array_dynamic_loop_explicit_out1_into_block_proc_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    mix = mix + voices[i]().out1

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop explicit out1");
}

#[test]

fn proc_array_dynamic_loop_statement_call_then_field_read_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    voices[i]()

    mix = mix + voices[i].out1

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop statement call then field read");
}

#[test]

fn proc_array_dynamic_loop_alias_call_into_block_proc_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    v = voices[i]

    mix = mix + v()

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop alias call");
}

#[test]

fn proc_array_dynamic_loop_inside_explicit_top_level_block_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"block {

  sample {

    mix = 0.0

    for i in 0..10 {

      mix = mix + voices[i]()

    }

    out1 = mix

  }

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop explicit top-level block");
}

#[test]

fn proc_array_dynamic_multi_call_expression_preserves_left_to_right_call_eval_order() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_INDEX_MULTI_CALL_EXPR_EVAL_ORDER_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 12.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 34.0, 1e-6);
}

#[test]

fn proc_array_dynamic_five_call_expression_preserves_left_to_right_eval_order() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_INDEX_FIVE_CALL_EXPR_EVAL_ORDER_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 12345.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 67900.0, 1e-6);
}

