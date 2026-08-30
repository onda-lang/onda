use super::*;

use onda_codegen_llvm::jit_program_from_optimized_mir;
use onda_frontend::parse_program;
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

fn assert_send<T: Send>() {}

fn compile_test_program(source: &str, block_size: usize) -> JitProgram {
    let parsed = parse_program(source).expect("test source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size,
        },
    )
    .expect("test source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("test source should lower");
    jit_program_from_optimized_mir(mir).expect("test MIR should compile")
}

fn compile_test_instance(source: &str, block_size: usize, out_channels: usize) -> Instance {
    let program = compile_test_program(source, block_size);
    create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: block_size,
            in_channels: 0,
            out_channels,
        },
    )
    .expect("test instance should initialize")
}

#[test]
fn instance_is_send() {
    assert_send::<Instance>();
}

#[test]
fn initialized_constructor_clears_prints_when_initialization_fails() {
    let program = compile_test_program(
        r#"
params:
  divisor: i32 = 0
init:
  print("before failure", 42)
  value = 1 / divisor
sample:
  out1 = f32(value)
"#,
        1,
    );
    let mut storage = [0_u8; 64];
    let mut prints = PrintBatch::from_storage(&mut storage);
    let error = create_instance_initialized_with_output(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 1,
            in_channels: 0,
            out_channels: 1,
        },
        ExecutionOutput {
            delegate_batch: None,
            print_batch: Some(&mut prints),
        },
    )
    .expect_err("division by zero should fail initialization");

    assert!(error.message.contains("runtime safety check"));
    assert_eq!(
        (
            prints.used_bytes,
            prints.record_count,
            prints.overflow_count
        ),
        (0, 0, 0)
    );
}

#[test]
fn canonical_print_floats_preserve_width_and_boundary_rules() {
    assert_eq!(canonical_f32(1.234567_f32), "1.234567");
    assert_eq!(canonical_f64(-0.0), "-0.0");
    assert_eq!(canonical_f64(1.0e-6), "0.000001");
    assert_eq!(canonical_f64(1.0e-7), "1e-7");
    assert_eq!(canonical_f64(1.0e20), "100000000000000000000.0");
    assert_eq!(canonical_f64(1.0e21), "1e21");
    assert_eq!(canonical_f32(f32::INFINITY), "inf");
    assert_eq!(canonical_f64(f64::NEG_INFINITY), "-inf");
    assert_eq!(canonical_f32(f32::from_bits(0x7fc0_0001)), "NaN");
    assert_eq!(canonical_f32(f32::from_bits(1)), "1e-45");
    assert_eq!(canonical_f64(f64::from_bits(1)), "5e-324");
}

#[test]
fn canonical_print_floats_match_the_shared_randomized_bit_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/onda_processor_abi/test/fixtures/print-float-parity.json"
    ))
    .expect("shared print float fixture should be valid JSON");
    for entry in fixture["f32"].as_array().expect("f32 fixture array") {
        let bits = u32::from_str_radix(entry["bits"].as_str().expect("f32 bits"), 16)
            .expect("valid f32 bits");
        assert_eq!(
            canonical_f32(f32::from_bits(bits)),
            entry["text"].as_str().expect("f32 text")
        );
    }
    for entry in fixture["f64"].as_array().expect("f64 fixture array") {
        let bits = u64::from_str_radix(entry["bits"].as_str().expect("f64 bits"), 16)
            .expect("valid f64 bits");
        assert_eq!(
            canonical_f64(f64::from_bits(bits)),
            entry["text"].as_str().expect("f64 text")
        );
    }
}

#[test]
fn print_labels_escape_every_record_separator() {
    let mut escaped = String::new();
    write_escaped_print_label(
        &mut escaped,
        "\0\\\n\r\t\u{7}\u{b}\u{c}\u{7f}\u{85}\u{2028}\u{2029}sound",
    )
    .expect("writing to a String should succeed");

    assert_eq!(
        escaped,
        "\\0\\\\\\n\\r\\t\\u{7}\\u{b}\\u{c}\\u{7f}\\u{85}\\u{2028}\\u{2029}sound"
    );
}

#[test]
fn decoded_print_formatting_matches_packed_batch_formatting() {
    let mut instance = compile_test_instance(
        r#"
sample:
  print("escaped\nlabel", 1.25, f64(-0.0), 7, i64(-9), true)
  out1 = 0.0
"#,
        1,
        1,
    );
    let mut output = [0.0_f32; 1];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    let mut storage = [0_u8; 256];
    let mut batch = PrintBatch::from_storage(&mut storage);
    process_checked(
        &mut instance,
        1,
        ExecutionOutput {
            delegate_batch: None,
            print_batch: Some(&mut batch),
        },
    )
    .expect("sample should print");

    let packed = format_print_batch(&instance, &batch).expect("packed batch should format");
    let decoded = decode_print_batch(&instance, &batch).expect("batch should decode");

    assert_eq!(format_decoded_print_occurrences(&decoded), packed);
}

#[test]
fn absent_print_storage_does_not_skip_argument_evaluation() {
    let mut instance = compile_test_instance(
        r#"
proc Counter:
  outs:
    out1
  init:
    count: i32 = 0
  sample:
    count += 1
    out1 = f32(count)

outs:
  out1
init:
  counter = Counter()
sample:
  print(counter())
  out1 = counter()
"#,
        1,
        1,
    );
    let mut output = [0.0_f32; 1];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, 1, ExecutionOutput::none())
        .expect("process without print storage should succeed");
    assert_eq!(output, [2.0]);
}

#[test]
fn delegate_batch_iterates_complete_native_records_without_allocation() {
    let mut storage = [0_u8; 29];
    storage[0..4].copy_from_slice(&2_u32.to_ne_bytes());
    storage[4..8].copy_from_slice(&4_u32.to_ne_bytes());
    storage[8..12].copy_from_slice(&7_u32.to_ne_bytes());
    storage[12..16].copy_from_slice(&17_i32.to_ne_bytes());
    storage[16..20].copy_from_slice(&5_u32.to_ne_bytes());
    storage[20..24].copy_from_slice(&1_u32.to_ne_bytes());
    storage[24..28].copy_from_slice(&9_u32.to_ne_bytes());
    storage[28] = 1;

    let mut batch = DelegateBatch::from_storage(&mut storage);
    batch.used_bytes = 29;
    batch.record_count = 2;
    batch.overflow_count = 3;

    let occurrences = batch.occurrences().collect::<Vec<_>>();
    assert_eq!(occurrences.len(), 2);
    assert_eq!(occurrences[0].delegate_index, 2);
    assert_eq!(occurrences[0].sequence, 7);
    assert_eq!(occurrences[0].payload, 17_i32.to_ne_bytes());
    assert_eq!(occurrences[1].delegate_index, 5);
    assert_eq!(occurrences[1].sequence, 9);
    assert_eq!(occurrences[1].payload, [1]);
    assert_eq!(batch.occurrence(1), Some(occurrences[1]));
    assert_eq!(batch.occurrence(2), None);
    assert_eq!(batch.capacity_bytes(), 29);
    assert_eq!(batch.overflow_count, 3);
}

#[test]
fn instance_reports_payload_and_complete_delegate_record_sizes() {
    let instance = compile_test_instance(
        r#"
delegate fixed(value: i32)
delegate dynamic(values: f32[])
sample:
  out1 = 0.0
"#,
        1,
        1,
    );

    assert_eq!(instance.delegate_count(), 2);
    assert_eq!(instance.delegate_name(0), Some("fixed"));
    assert_eq!(instance.delegate_index("dynamic"), Some(1));
    assert_eq!(instance.delegate_payload_bytes(0), Some(4));
    assert_eq!(instance.delegate_payload_min_bytes(0), Some(4));
    assert_eq!(instance.delegate_record_bytes(0), Some(16));
    assert_eq!(instance.delegate_record_min_bytes(0), Some(16));
    assert_eq!(instance.delegate_payload_bytes(1), None);
    assert_eq!(instance.delegate_payload_min_bytes(1), Some(4));
    assert_eq!(instance.delegate_record_bytes(1), None);
    assert_eq!(instance.delegate_record_min_bytes(1), Some(16));
    assert!(instance.delegate_descriptor(2).is_none());
}

#[test]
fn bufferless_instances_have_no_fallback_storage() {
    let parsed = parse_program("sample:\n  out1 = 0.0\n").expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 64,
        },
    )
    .expect("source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("typed program should lower to MIR");
    let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
    let instance = create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 64,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("instance should initialize");

    assert!(instance.buffer_ptrs.is_empty());
}

#[test]
fn creation_defers_full_init_until_after_initial_parameter_configuration() {
    let parsed =
        parse_program("params:\n  gain = 0.25\ninit:\n  value = gain\nsample:\n  out1 = value\n")
            .expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 1,
        },
    )
    .expect("source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("source should lower");
    let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
    let mut instance = create_instance(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 1,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("instance allocation should succeed");
    let mut output = [0.0_f32; 1];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    assert!(!instance.is_initialized());
    assert!(process_checked(&mut instance, 1, ExecutionOutput::none()).is_err());
    assert!(init(&mut instance, InitMode::PreservePinned).is_err());
    set_param_by_index(&mut instance, 0, &0.75_f32.to_ne_bytes())
        .expect("initial parameter should update");
    init(&mut instance, InitMode::Full).expect("full init should succeed");
    assert!(instance.is_initialized());
    process_checked(&mut instance, 1, ExecutionOutput::none())
        .expect("initialized instance should process");
    assert_eq!(output, [0.75]);
}

#[test]
fn dynamic_proc_task_activation_tracks_each_while_condition_evaluation() {
    let sources = [
        r#"
proc Child:
  init:
    pin progress: i32 = 0
  task load():
    progress = 1
    yield
    progress = 2
  block:
    await load()
    sample:
      out1 = f32(progress)

proc Parent:
  init:
    children: Child[2] = Child()
  sample:
    index = 0
    while index < 2 && children[index]() > 0.0:
      index = index + 1
    out1 = f32(index) * 0.25

init:
  parent = Parent()
sample:
  out1 = parent()
"#,
        r#"
proc Child:
  init:
    pin progress: i32 = 0
  task load():
    progress = 1
    yield
    progress = 2
  block:
    await load()
    sample:
      out1 = f32(progress)

init:
  children: Child[2] = Child()
sample:
  index = 0
  while index < 2 && children[index]() > 0.0:
    index = index + 1
  out1 = f32(index) * 0.25
"#,
    ];

    for source in sources {
        let mut instance = compile_test_instance(source, 1, 1);
        let mut output = [0.0_f32; 1];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        for expected in [0.0, 0.25, 0.5] {
            process_checked(&mut instance, 1, ExecutionOutput::none())
                .expect("task block should process");
            assert_eq!(output[0], expected);
        }
    }
}

#[test]
fn dynamic_proc_guards_preserve_short_circuiting_and_evaluate_selectors_once() {
    let source = r#"
proc Child:
  block:
    sample:
      out1 = 0.0

proc Parent:
  init:
    children: Child[2] = Child()
    calls: i32 = 0
  def select() -> i32:
    calls = calls + 1
    return 0
  sample:
    if false && children[select()]() > 0.0:
      calls = calls + 100
    value = children[select()]()
    out1 = f32(calls) * 0.1

init:
  parent = Parent()
sample:
  out1 = parent()
"#;
    let mut instance = compile_test_instance(source, 1, 1);
    let mut output = [0.0_f32; 1];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, 1, ExecutionOutput::none()).expect("task block should process");
    assert_eq!(output[0], 0.1);
}

#[test]
fn top_level_task_declarations_do_not_close_a_standalone_sample_gate() {
    let source = "task unused():\n  yield\nsample:\n  out1 = 1.0\n";
    let mut instance = compile_test_instance(source, 4, 1);
    let mut output = [0.0_f32; 4];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, 4, ExecutionOutput::none())
        .expect("standalone sample should process");
    assert_eq!(output, [1.0; 4]);
}

#[test]
fn state_init_and_restore_preserve_validated_buffer_tables() {
    let parsed = parse_program(
        "buffers:\n  data: f32\ninit:\n  counter = 0.0\nsample:\n  counter = counter + 1.0\n",
    )
    .expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 64,
        },
    )
    .expect("source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("typed program should lower to MIR");
    let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
    let mut instance = create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 64,
            in_channels: 0,
            out_channels: 0,
        },
    )
    .expect("instance should initialize");
    let mut samples = [1.0_f32, 2.0];
    unsafe {
        bind_buffer(
            &mut instance,
            0,
            samples.as_mut_ptr().cast(),
            samples.len(),
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .expect("buffer should bind");
    }
    validate_buffers(&mut instance).expect("buffer should validate");
    let bound_ptr = instance.buffer_ptrs[0];
    let snapshot = instance
        .snapshot_state_bytes()
        .expect("initialized state should snapshot");

    init(&mut instance, InitMode::PreservePinned).expect("init should succeed");
    assert!(instance.buffers_validated);
    assert_eq!(instance.buffer_ptrs[0], bound_ptr);

    instance
        .restore_state_bytes(&snapshot)
        .expect("state should restore");
    assert!(instance.buffers_validated);
    assert_eq!(instance.buffer_ptrs[0], bound_ptr);
}

#[test]
fn top_level_and_proc_init_observe_current_buffer_bindings() {
    let parsed = parse_program(
        r#"
proc Reader:
  buffers:
    source: f32

  init:
    first = source[0]
    frames = source.len()
    source_bound = source.bound()

  sample:
    value = first + f32(frames)
    if source_bound:
      value = value + 10.0
    out1 = value

buffers:
  source: f32

init:
  selected = source[1]
  source_bound = source.bound()
  reader = Reader(source = source)

event refresh():
  reader.init()

sample:
  value = selected + reader() + source[0]
  if source_bound:
    value = value + 100.0
  out1 = value
"#,
    )
    .expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 1,
        },
    )
    .expect("source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("source should lower");
    let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
    let config = InstanceConfig {
        sample_rate: 48_000.0,
        frames_per_block: 1,
        in_channels: 0,
        out_channels: 1,
    };

    let mut neutral = create_instance_initialized(program.clone(), config)
        .expect("neutral instance should initialize");
    let mut neutral_output = [0.0_f32; 1];
    unsafe {
        bind_output(
            &mut neutral,
            0,
            neutral_output.as_mut_ptr().cast(),
            std::mem::size_of_val(&neutral_output),
        )
        .expect("neutral output should bind");
    }
    process_checked(&mut neutral, 1, ExecutionOutput::none())
        .expect("neutral instance should process");
    assert_eq!(neutral_output, [1.0]);

    let mut bound = create_instance(program, config).expect("instance should allocate");
    let mut samples = [2.0_f32, 5.0];
    let mut bound_output = [0.0_f32; 1];
    unsafe {
        bind_buffer(
            &mut bound,
            0,
            samples.as_mut_ptr().cast(),
            samples.len(),
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .expect("buffer should bind");
        bind_output(
            &mut bound,
            0,
            bound_output.as_mut_ptr().cast(),
            std::mem::size_of_val(&bound_output),
        )
        .expect("bound output should bind");
    }
    init(&mut bound, InitMode::Full).expect("bound instance should initialize");
    process_checked(&mut bound, 1, ExecutionOutput::none()).expect("bound instance should process");
    assert_eq!(bound_output, [121.0]);

    let mut replacement = [7.0_f32, 11.0];
    unsafe {
        bind_buffer(
            &mut bound,
            0,
            replacement.as_mut_ptr().cast(),
            replacement.len(),
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .expect("replacement buffer should bind");
    }
    process_checked(&mut bound, 1, ExecutionOutput::none())
        .expect("replacement binding should be visible without reinitialization");
    assert_eq!(bound_output, [126.0]);

    let refresh = bound.event_index("refresh").expect("refresh event");
    trigger_event_by_index(&mut bound, refresh, &[], ExecutionOutput::none())
        .expect("proc init event should run against the current binding");
    process_checked(&mut bound, 1, ExecutionOutput::none())
        .expect("reinitialized proc should process");
    assert_eq!(bound_output, [131.0]);

    init(&mut bound, InitMode::Full)
        .expect("top-level init should run against the replacement binding");
    process_checked(&mut bound, 1, ExecutionOutput::none())
        .expect("fully reinitialized instance should process");
    assert_eq!(bound_output, [137.0]);
}

#[test]
fn buffer_bound_tracks_direct_forwarded_and_collection_bindings() {
    let source = r#"
proc Probe:
  buffers:
    clips: f32 {2}
  outs:
    out1
  sample:
    value = 0.0
    if clips[0].bound():
      value = value + 1.0
    if clips[1].bound():
      value = value + 2.0
    out1 = value

buffers:
  direct: f32
  bank: f32 {2}

def is_bound(buf: buffer<f32>):
  return buf.bound()

init:
  probe = Probe(clips = bank)

sample:
  value = 0.0
  if direct.bound():
    value = value + 1.0
  if is_bound(bank[0]):
    value = value + 2.0
  if bank[1].bound():
    value = value + 4.0
  out1 = value + 10.0 * probe()
"#;
    let mut instance = compile_test_instance(source, 1, 1);
    let mut output = [0.0_f32; 1];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, 1, ExecutionOutput::none())
        .expect("unbound buffers should process");
    assert_eq!(output, [0.0]);

    let mut direct = [1.0_f32];
    let mut second = [2.0_f32];
    unsafe {
        bind_buffer(
            &mut instance,
            0,
            direct.as_mut_ptr().cast(),
            1,
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .expect("direct buffer should bind");
        bind_buffer(
            &mut instance,
            2,
            second.as_mut_ptr().cast(),
            1,
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .expect("second collection entry should bind");
    }
    process_checked(&mut instance, 1, ExecutionOutput::none())
        .expect("partially bound buffers should process");
    assert_eq!(output, [25.0]);

    let mut first = [3.0_f32];
    unsafe {
        bind_buffer(
            &mut instance,
            0,
            direct.as_mut_ptr().cast(),
            1,
            1,
            0.0,
            PrimitiveType::F32,
        )
        .expect("direct buffer should unbind");
        bind_buffer(
            &mut instance,
            1,
            first.as_mut_ptr().cast(),
            1,
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .expect("first collection entry should bind");
    }
    process_checked(&mut instance, 1, ExecutionOutput::none())
        .expect("rebound buffers should process");
    assert_eq!(output, [36.0]);
}

#[test]
fn cooperative_task_yields_gate_audio_and_survive_default_init() {
    const BLOCK_SIZE: usize = 8;
    let parsed = parse_program(
        r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 1

  event reload():
    load.reset()

  block:
    await load()
    sample:
      out1 = f32(progress)

init:
  loader = Loader()

event reload():
  loader.reload()

sample:
  out1 = loader()
"#,
    )
    .expect("task source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: BLOCK_SIZE,
        },
    )
    .expect("task source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("task source should lower");
    let program = jit_program_from_optimized_mir(mir).expect("task MIR should compile");
    let mut instance = create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: BLOCK_SIZE,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("task instance should initialize");
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("first task block should process");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("completion block should process");
    assert_eq!(output, [2.0; BLOCK_SIZE]);

    let reload = instance.event_index("reload").expect("reload event");
    trigger_event_by_index(&mut instance, reload, &[], ExecutionOutput::none())
        .expect("task reset event should run");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restarted task block should process");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restarted task should complete");
    assert_eq!(output, [4.0; BLOCK_SIZE]);

    init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("default init block should process");
    assert_eq!(output, [4.0; BLOCK_SIZE]);

    init(&mut instance, InitMode::Full).expect("full init should succeed");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("full init block should process");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
}

#[test]
fn task_can_call_child_proc_events() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
proc Child:
  init:
    value: i32 = 0
  event add(amount: i32):
    value += amount
  sample:
    out1 = f32(value)

proc Owner:
  init:
    child = Child()
  task prepare():
    child.add(2)
    yield
    child.add(3)
  block:
    await prepare()
    sample:
      out1 = child()

init:
  owner = Owner()
sample:
  out1 = owner()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("yielding task should process");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("completing task should process");
    assert_eq!(output, [5.0]);
}

#[test]
fn task_can_step_block_rate_child_procs() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
proc Counter:
  kouts:
    value
  init:
    count: i32 = 0
  block:
    count += 1
    value = f32(count)

proc Owner:
  init:
    counter = Counter()
    pin result = 0.0
  def step_counter():
    return counter()
  task prepare():
    result = counter()
    yield
    result += step_counter()
  block:
    await prepare()
    sample:
      out1 = result

init:
  owner = Owner()
sample:
  out1 = owner()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("yielding task should process");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("completing task should process");
    assert_eq!(output, [3.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("completed task should fall through");
    assert_eq!(output, [3.0]);
}

#[test]
fn top_level_task_yields_resumes_and_resets() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
init:
  pin progress: i32 = 0
task prepare():
  progress += 1
  yield
  progress += 1

event retry():
  prepare.reset()

block:
  await prepare()
  sample:
    out1 = f32(progress)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("yielding task should process");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("completing task should process");
    assert_eq!(output, [2.0]);

    let retry = instance.event_index("retry").expect("retry event");
    trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none())
        .expect("task reset should run");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("reset task should yield again");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("reset task should complete again");
    assert_eq!(output, [4.0]);

    init(&mut instance, InitMode::PreservePinned)
        .expect("default init should preserve the completed task");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("default init should preserve the completed task");
    assert_eq!(output, [4.0]);

    init(&mut instance, InitMode::Full).expect("full init should restart the task");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("full init should restart the task");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restarted task should complete from a cleared state");
    assert_eq!(output, [2.0]);

    init(&mut instance, InitMode::PreservePinned)
        .expect("default init should preserve pinned task state");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("default init should preserve the completed task");
    assert_eq!(output, [2.0]);

    init(&mut instance, InitMode::Full).expect("full init should restart after default init");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("full init should restart after default init");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("fully initialized task should complete");
    assert_eq!(output, [2.0]);
}

#[test]
fn task_reset_reinitializes_retained_frame_storage_on_restart() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
init:
  pin result: i32 = 0
task prepare():
  carried: i32[2] = [3, 5]
  yield
  result = carried[0] + carried[1]
event retry():
  prepare.reset()
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("initial task should yield");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("initial task should complete");
    assert_eq!(output, [8.0]);

    let retry = instance.event_index("retry").expect("retry event");
    trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none())
        .expect("task reset should run");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restarted task should yield");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restarted task should complete");
    assert_eq!(output, [8.0]);
}

#[test]
fn top_level_init_respects_explicit_task_reset() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
init:
  pin progress: i32 = 0
  load.reset()

task load():
  progress += 1
  yield
  progress += 1

block:
  await load()
  sample:
    out1 = f32(progress)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(output, [2.0]);

    init(&mut instance, InitMode::PreservePinned)
        .expect("default init should execute explicit task reset");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("explicitly reset task should yield again");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("explicitly reset task should complete again");
    assert_eq!(output, [4.0]);
}

#[test]
fn top_level_task_neutralizes_control_outputs_while_suspended() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
kouts:
  ready

init:
  pin progress: i32 = 0
task load():
  progress += 1
  yield
  progress += 1

event retry():
  load.reset()

block:
  await load()
  ready = f32(progress)
"#,
        BLOCK_SIZE,
        0,
    );
    let ready = instance
        .control_output_index("ready")
        .expect("ready control output");
    let read_ready = |instance: &Instance| {
        let mut bytes = [0_u8; size_of::<f32>()];
        read_control_output_bytes(instance, ready, &mut bytes)
            .expect("control output should be readable");
        f32::from_le_bytes(bytes)
    };

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    assert_eq!(read_ready(&instance), 0.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(read_ready(&instance), 2.0);

    let retry = instance.event_index("retry").expect("retry event");
    trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none())
        .expect("task reset should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("reset task should yield");
    assert_eq!(read_ready(&instance), 0.0);
}

#[test]
fn proc_task_neutralizes_block_timed_outputs_at_the_await_barrier() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
proc Control:
  kouts 1
  init:
    pin value: i32 = 0
  task load():
    yield
    value += 1
  block:
    await load()
    kout1 = f32(value)

init:
  control = Control()

kouts:
  ready

block:
  ready = control().kout1
"#,
        BLOCK_SIZE,
        0,
    );
    let ready = instance
        .control_output_index("ready")
        .expect("ready control output");
    let read_ready = |instance: &Instance| {
        let mut bytes = [0_u8; size_of::<f32>()];
        read_control_output_bytes(instance, ready, &mut bytes)
            .expect("control output should be readable");
        f32::from_le_bytes(bytes)
    };

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("proc task should yield");
    assert_eq!(read_ready(&instance), 0.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("proc task should complete");
    assert_eq!(read_ready(&instance), 1.0);
}

#[test]
fn suspended_proc_tasks_skip_nested_block_post_hooks() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
proc Child:
  init:
    pin counter: i32 = 0
  block:
    counter += 1
    sample:
      out1 = f32(counter)
    counter += 100

proc Parent:
  init:
    child = Child()
  task load():
    yield
  block:
    await load()
    sample:
      out1 = child()

proc Grandparent:
  init:
    parent = Parent()
  sample:
    out1 = parent()

init:
  direct = Parent()
  nested = Grandparent()
sample:
  out1 = direct()
  out2 = nested()
"#,
        BLOCK_SIZE,
        2,
    );
    let mut direct = [99.0_f32; BLOCK_SIZE];
    let mut nested = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            direct.as_mut_ptr().cast(),
            std::mem::size_of_val(&direct),
        )
        .expect("direct output should bind");
        bind_output(
            &mut instance,
            1,
            nested.as_mut_ptr().cast(),
            std::mem::size_of_val(&nested),
        )
        .expect("nested output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("both tasks should yield");
    assert_eq!(direct, [0.0]);
    assert_eq!(nested, [0.0]);

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("both tasks should complete");
    assert_eq!(direct, [1.0]);
    assert_eq!(nested, [1.0]);
}

#[test]
fn top_level_task_suspension_escapes_nested_block_pre_loops() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
init:
  pin progress: i32 = 0
  pin activations: i32 = 0
task load():
  progress += 1
  yield
  progress += 1

block:
  for outer in 0..2:
    for inner in 0..2:
      await load()
  activations += 1

  sample:
    out1 = f32(activations)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should yield inside nested loops");
    assert_eq!(output, [0.0]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete on the next block");
    assert_eq!(output, [1.0]);
}

#[test]
fn live_init_preserves_pinned_state_and_full_init_reinitializes_it() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
outs { out1, out2 }
init {
  pin pinned: i32 = 10
  ordinary: i32 = 20
  pinned += 1
}
sample {
  pinned += 1
  ordinary += 1
  out1 = f32(pinned)
  out2 = f32(ordinary)
}
"#,
        BLOCK_SIZE,
        2,
    );
    let mut pinned = [0.0_f32; BLOCK_SIZE];
    let mut ordinary = [0.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            pinned.as_mut_ptr().cast(),
            std::mem::size_of_val(&pinned),
        )
        .expect("pinned output should bind");
        bind_output(
            &mut instance,
            1,
            ordinary.as_mut_ptr().cast(),
            std::mem::size_of_val(&ordinary),
        )
        .expect("ordinary output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("initial state should process");
    assert_eq!((pinned[0], ordinary[0]), (12.0, 21.0));

    init(&mut instance, InitMode::PreservePinned).expect("ordinary live init should succeed");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("reinitialized state should process");
    assert_eq!((pinned[0], ordinary[0]), (14.0, 21.0));
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("state should advance");
    assert_eq!((pinned[0], ordinary[0]), (15.0, 22.0));

    init(&mut instance, InitMode::PreservePinned).expect("second default init should succeed");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("second init state should process");
    assert_eq!((pinned[0], ordinary[0]), (17.0, 21.0));

    init(&mut instance, InitMode::Full).expect("full live init should succeed");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("fully initialized state should process");
    assert_eq!((pinned[0], ordinary[0]), (12.0, 21.0));
}

#[test]
fn live_init_handles_untyped_pinned_declarations() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
    pinned = value + 1.0
  }
}
init {
  amp = 0.0
  pin pinned = 1.0
}
sample { out1 = amp + pinned }
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [0.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    let event = instance.event_index("set_amp").expect("set_amp event");
    trigger_event_by_index(
        &mut instance,
        event,
        &0.5_f32.to_ne_bytes(),
        ExecutionOutput::none(),
    )
    .expect("event should run");
    init(&mut instance, InitMode::PreservePinned).expect("live init should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("state should process");
    assert_eq!(output, [1.5]);
}

#[test]
fn init_preserves_pinned_structs_and_full_init_reinitializes_them() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
struct State:
  value: i32 = 1

init:
  pin one = State()
  pin many: State[2] = State()
  ordinary = State()

event mutate():
  one.value = 10
  many[0].value = 20
  many[1].value = 30
  ordinary.value = 40

sample:
  out1 = f32(one.value + many[0].value + many[1].value + ordinary.value)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [0.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("initial state should process");
    assert_eq!(output, [4.0]);

    let mutate = instance.event_index("mutate").expect("mutate event");
    trigger_event_by_index(&mut instance, mutate, &[], ExecutionOutput::none())
        .expect("mutation should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("mutated state should process");
    assert_eq!(output, [100.0]);

    init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("default init should process");
    assert_eq!(output, [61.0]);

    init(&mut instance, InitMode::Full).expect("full init should succeed");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("full init should process");
    assert_eq!(output, [4.0]);
}

#[test]
fn failed_live_init_invalidates_state_until_full_init_or_restore() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
params:
  divisor: i32 = 1
outs:
  out1
init:
  pin pinned: i32 = 10
  pinned += 1
  quotient: i32 = 10 / divisor
event set_pinned(value: i32):
  pinned = value
sample:
  out1 = f32(pinned)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [0.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    let event = instance
        .event_index("set_pinned")
        .expect("set_pinned event");
    trigger_event_by_index(
        &mut instance,
        event,
        &50_i32.to_ne_bytes(),
        ExecutionOutput::none(),
    )
    .expect("event should run");
    let snapshot = instance
        .snapshot_state_bytes()
        .expect("valid state should snapshot");
    set_param_by_index(&mut instance, 0, &0_i32.to_ne_bytes())
        .expect("divisor parameter should update");

    assert!(
        init(&mut instance, InitMode::PreservePinned).is_err(),
        "division by zero should fail"
    );
    assert!(!instance.is_initialized());
    assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
    assert!(trigger_event_by_index(
        &mut instance,
        event,
        &0_i32.to_ne_bytes(),
        ExecutionOutput::none()
    )
    .is_err());
    assert!(instance.snapshot_state_bytes().is_err());

    set_param_by_index(&mut instance, 0, &1_i32.to_ne_bytes())
        .expect("divisor parameter should recover");
    assert!(
        init(&mut instance, InitMode::PreservePinned).is_err(),
        "preserve-pinned init cannot recover indeterminate pinned state"
    );
    instance
        .restore_state_bytes(&snapshot)
        .expect("a valid snapshot should recover invalid state");
    assert!(instance.is_initialized());
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restored state should process");
    assert_eq!(output, [50.0]);

    set_param_by_index(&mut instance, 0, &0_i32.to_ne_bytes())
        .expect("divisor parameter should update");
    assert!(init(&mut instance, InitMode::PreservePinned).is_err());
    set_param_by_index(&mut instance, 0, &1_i32.to_ne_bytes())
        .expect("divisor parameter should recover");
    init(&mut instance, InitMode::Full).expect("full init should recover invalid state");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("reinitialized state should process");
    assert_eq!(output, [11.0]);
}

#[test]
fn failed_event_invalidates_state_until_full_init_or_restore() {
    let mut instance = compile_test_instance(
        r#"
init:
  held: i32 = 0
event divide(divisor: i32):
  held = 1 / divisor
sample:
  out1 = f32(held)
"#,
        1,
        1,
    );
    let event = instance.event_index("divide").expect("divide event");

    assert!(trigger_event_by_index(
        &mut instance,
        event,
        &0_i32.to_ne_bytes(),
        ExecutionOutput::none()
    )
    .is_err());
    assert!(!instance.is_initialized());
    assert!(trigger_event_by_index(
        &mut instance,
        event,
        &1_i32.to_ne_bytes(),
        ExecutionOutput::none()
    )
    .is_err());
    assert!(init(&mut instance, InitMode::PreservePinned).is_err());

    init(&mut instance, InitMode::Full).expect("full init should recover the instance");
    trigger_event_by_index(
        &mut instance,
        event,
        &1_i32.to_ne_bytes(),
        ExecutionOutput::none(),
    )
    .expect("event should run after recovery");
}

#[test]
fn unchecked_event_failure_invalidates_state_until_full_init() {
    let mut instance = compile_test_instance(
        r#"
init:
  held: i32 = 0
event divide(divisor: i32):
  held = 1 / divisor
sample:
  out1 = f32(held)
"#,
        1,
        1,
    );
    let event = instance.event_index("divide").expect("divide event");
    validate_buffers(&mut instance).expect("buffer descriptors should validate");

    let status = unsafe {
        trigger_event_by_index_unchecked(
            &mut instance,
            event,
            &0_i32.to_ne_bytes(),
            ExecutionOutput::none(),
        )
    }
    .expect("unchecked dispatch should return the generated status");
    assert_eq!(
        status,
        onda_codegen_llvm::PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
    );
    assert!(!instance.is_initialized());
    assert!(trigger_event_by_index(
        &mut instance,
        event,
        &1_i32.to_ne_bytes(),
        ExecutionOutput::none()
    )
    .is_err());
    assert!(init(&mut instance, InitMode::PreservePinned).is_err());

    init(&mut instance, InitMode::Full).expect("full init should recover the instance");
    validate_buffers(&mut instance).expect("buffer descriptors should revalidate");
    let status = unsafe {
        trigger_event_by_index_unchecked(
            &mut instance,
            event,
            &1_i32.to_ne_bytes(),
            ExecutionOutput::none(),
        )
    }
    .expect("event should run after recovery");
    assert_eq!(status, onda_codegen_llvm::PROCESSOR_EXECUTION_OK);
    assert!(instance.is_initialized());
}

#[test]
fn unchecked_process_failure_invalidates_state_until_full_init() {
    const BLOCK_SIZE: usize = 1;
    let mut instance = compile_test_instance(
        r#"
params:
  divisor: i32 = 0
sample:
  out1 = f32(1 / divisor)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [0.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    prepare_unchecked_process(&mut instance).expect("bindings should validate");

    let status = unsafe { process_unchecked(&mut instance, ExecutionOutput::none()) }
        .expect("unchecked processing should return the generated status");
    assert_eq!(
        status,
        onda_codegen_llvm::PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
    );
    assert!(!instance.is_initialized());
    assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
    assert!(init(&mut instance, InitMode::PreservePinned).is_err());

    set_param_by_index(&mut instance, 0, &1_i32.to_ne_bytes())
        .expect("divisor parameter should update");
    init(&mut instance, InitMode::Full).expect("full init should recover the instance");
    prepare_unchecked_process(&mut instance).expect("bindings should revalidate");
    let status = unsafe { process_unchecked(&mut instance, ExecutionOutput::none()) }
        .expect("processing should run after recovery");
    assert_eq!(status, onda_codegen_llvm::PROCESSOR_EXECUTION_OK);
    assert!(instance.is_initialized());
    assert_eq!(output, [1.0]);
}

#[test]
fn cooperative_task_resumes_for_loop_control_state() {
    const BLOCK_SIZE: usize = 4;
    let parsed = parse_program(
        r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    for i in 0..3:
      progress += 1
      yield
  block:
    await load()
    sample:
      out1 = f32(progress)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
    )
    .expect("loop task source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: BLOCK_SIZE,
        },
    )
    .expect("loop task source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("loop task should lower");
    let program = jit_program_from_optimized_mir(mir).expect("loop task MIR should compile");
    let mut instance = create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: BLOCK_SIZE,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("loop task instance should initialize");
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    for _ in 0..3 {
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("yielding block should process");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        output.fill(99.0);
    }
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("completion block should process");
    assert_eq!(output, [3.0; BLOCK_SIZE]);
}

#[test]
fn task_for_bounds_share_ordinary_i32_induction_coercion() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
init:
  pin result: i32 = 0
task prepare():
  for i in (i64(0))..(i64(2)):
    result += i
    yield
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("first loop iteration should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("second loop iteration should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task loop should complete");
    assert_eq!(output, [1.0; BLOCK_SIZE]);
}

#[test]
fn explicit_i64_for_uses_i64_induction_and_overload_resolution() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
def offset(i: i32) -> i64:
  return i64(-100)
def offset(i: i64) -> i64:
  return i - i64(2147483648)
init:
  start: i64 = i64(2147483648)
  end: i64 = i64(2147483650)
sample:
  total: i64 = 0
  for i: i64 in start..end:
    total += offset(i)
  out1 = f32(total)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("i64 loop should process");
    assert_eq!(output, [1.0; BLOCK_SIZE]);
}

#[test]
fn explicit_i64_for_handles_an_inclusive_maximum_endpoint() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
sample:
  count: i32 = 0
  for i: i64 in (i64(9223372036854775806))..=(i64(9223372036854775807)):
    count += 1
  out1 = f32(count)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("maximum-bound loop should process");
    assert_eq!(output, [2.0; BLOCK_SIZE]);
}

#[test]
fn task_for_completes_after_yield_at_inclusive_i64_extrema() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
init:
  pin result: i32 = 0
task prepare():
  for i: i64 in (i64(9223372036854775807))..=(i64(9223372036854775807)):
    result += 1
    yield
  for i: i64 @ -1 in (i64(-9223372036854775807) - i64(1))..=(i64(-9223372036854775807) - i64(1)):
    result += 2
    yield
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("maximum iteration should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("minimum iteration should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("extrema-bound task should complete");
    assert_eq!(output, [3.0; BLOCK_SIZE]);
}

#[test]
fn proc_task_for_preserves_explicit_i64_induction_across_yields() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Loader:
  init:
    pin result: i64 = 0
  task prepare():
    for i: i64 in (i64(2147483648))..(i64(2147483650)):
      result += i - i64(2147483648)
      yield
  block:
    await prepare()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("first i64 iteration should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("second i64 iteration should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("i64 task loop should complete");
    assert_eq!(output, [1.0; BLOCK_SIZE]);
}

#[test]
fn fixed_tuple_task_frame_survives_yield() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
init:
  pin result: f32 = 0.0
task prepare():
  pair = (i32(3), i64(5))
  yield
  result = f32(pair[0]) + f32(pair[1])
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("tuple task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("tuple task should complete");
    assert_eq!(output, [8.0; BLOCK_SIZE]);
}

#[test]
fn proc_init_respects_pinning_and_explicit_task_reset() {
    const BLOCK_SIZE: usize = 4;
    let parsed = parse_program(
        r#"
proc Keeper:
  init:
    pin progress: i32 = 10
    scratch: i32 = 20
  task load():
    progress += 1
    scratch += 1
    yield
    progress += 100
    scratch += 100
  block:
    await load()
    sample:
      out1 = f32(progress + scratch)

proc Resetter:
  init:
    pin progress: i32 = 10
    scratch: i32 = 20
    load.reset()
  task load():
    progress += 1
    scratch += 1
    yield
    progress += 100
    scratch += 100
  block:
    await load()
    sample:
      out1 = f32(progress + scratch)

outs:
  out1
  out2
init:
  keeper = Keeper()
  resetter = Resetter()
event default_init():
  keeper.init()
  resetter.init()
event full_init():
  keeper.init(full = true)
sample:
  out1 = keeper()
  out2 = resetter()
"#,
    )
    .expect("proc init task source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: BLOCK_SIZE,
        },
    )
    .expect("proc init task source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("proc init task should lower");
    let program = jit_program_from_optimized_mir(mir).expect("task MIR should compile");
    let mut instance = create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: BLOCK_SIZE,
            in_channels: 0,
            out_channels: 2,
        },
    )
    .expect("task instance should initialize");
    let mut output1 = [99.0_f32; BLOCK_SIZE];
    let mut output2 = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output1.as_mut_ptr().cast(),
            std::mem::size_of_val(&output1),
        )
        .expect("first output should bind");
        bind_output(
            &mut instance,
            1,
            output2.as_mut_ptr().cast(),
            std::mem::size_of_val(&output2),
        )
        .expect("second output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("both tasks should yield");
    assert_eq!(output1, [0.0; BLOCK_SIZE]);
    assert_eq!(output2, [0.0; BLOCK_SIZE]);

    let default_init = instance
        .event_index("default_init")
        .expect("default init event");
    trigger_event_by_index(&mut instance, default_init, &[], ExecutionOutput::none())
        .expect("default proc init should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("tasks should resume after init");
    assert_eq!(output1, [231.0; BLOCK_SIZE]);
    assert_eq!(output2, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("reset task should complete");
    assert_eq!(output2, [233.0; BLOCK_SIZE]);

    let full_init = instance.event_index("full_init").expect("full init event");
    trigger_event_by_index(&mut instance, full_init, &[], ExecutionOutput::none())
        .expect("forced proc init should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("fully reset task should yield");
    assert_eq!(output1, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("fully reset task should complete");
    assert_eq!(output1, [232.0; BLOCK_SIZE]);
}

#[test]
fn proc_init_reinitializes_declaration_only_fixed_arrays() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Probe:
  init:
    pin prepared: f32[8]
    scratch: f32[8]
  event dirty():
    prepared[0] = 9.0
    scratch[0] = 7.0
  sample:
    out1 = prepared[0] * 10.0 + scratch[0]

proc Parent:
  init:
    probe = Probe()
  event dirty():
    probe.dirty()
  event default_init():
    probe.init()
  event full_init():
    probe.init(full = true)
  sample:
    out1 = probe()

init:
  direct = Probe()
  parent = Parent()
event dirty():
  direct.dirty()
  parent.dirty()
event default_init():
  direct.init()
  parent.default_init()
event full_init():
  direct.init(full = true)
  parent.full_init()
sample:
  out1 = direct()
  out2 = parent()
"#,
        BLOCK_SIZE,
        2,
    );
    let mut direct_output = [99.0_f32; BLOCK_SIZE];
    let mut nested_output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            direct_output.as_mut_ptr().cast(),
            std::mem::size_of_val(&direct_output),
        )
        .expect("direct output should bind");
        bind_output(
            &mut instance,
            1,
            nested_output.as_mut_ptr().cast(),
            std::mem::size_of_val(&nested_output),
        )
        .expect("nested output should bind");
    }

    let dirty = instance.event_index("dirty").expect("dirty event");
    let default_init = instance
        .event_index("default_init")
        .expect("default init event");
    let full_init = instance.event_index("full_init").expect("full init event");

    trigger_event_by_index(&mut instance, dirty, &[], ExecutionOutput::none())
        .expect("arrays should become dirty");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("dirty state should process");
    assert_eq!(direct_output, [97.0; BLOCK_SIZE]);
    assert_eq!(nested_output, [97.0; BLOCK_SIZE]);

    trigger_event_by_index(&mut instance, default_init, &[], ExecutionOutput::none())
        .expect("default proc init should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("default state should process");
    assert_eq!(direct_output, [90.0; BLOCK_SIZE]);
    assert_eq!(nested_output, [90.0; BLOCK_SIZE]);

    trigger_event_by_index(&mut instance, dirty, &[], ExecutionOutput::none())
        .expect("arrays should become dirty");
    trigger_event_by_index(&mut instance, full_init, &[], ExecutionOutput::none())
        .expect("full proc init should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("fully reset state should process");
    assert_eq!(direct_output, [0.0; BLOCK_SIZE]);
    assert_eq!(nested_output, [0.0; BLOCK_SIZE]);
}

#[test]
fn suspended_task_snapshots_resume_once_across_segmented_processing() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 10
  block:
    await load()
    sample:
      out1 = f32(progress)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        BLOCK_SIZE,
        1,
    );
    let reflected_state = (0..instance.state_count())
        .filter_map(|index| instance.state_name(index))
        .collect::<Vec<_>>();
    assert!(
        reflected_state.iter().all(|name| !name.contains("__onda_")),
        "task frame storage must not appear as authored state: {reflected_state:?}"
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked_segment(
        &mut instance,
        0,
        0,
        PROCESS_BEGIN_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("zero-frame begin should advance and yield the task");
    assert_eq!(output, [99.0; BLOCK_SIZE]);
    process_checked_segment(&mut instance, 0, 2, 0, ExecutionOutput::none())
        .expect("first audio segment should observe the yielded task");
    assert_eq!(output, [0.0, 0.0, 99.0, 99.0]);
    init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
    process_checked_segment(
        &mut instance,
        2,
        2,
        PROCESS_END_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("default init must not reopen the task gate within a logical block");
    assert_eq!(output, [0.0; BLOCK_SIZE]);

    let suspended = instance
        .snapshot_state_bytes()
        .expect("initialized state should snapshot");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("next block should complete the task");
    assert_eq!(output, [11.0; BLOCK_SIZE]);

    instance
        .restore_state_bytes(&suspended)
        .expect("suspended task snapshot should restore");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restored task should resume after its yield");
    assert_eq!(output, [11.0; BLOCK_SIZE]);
}

#[test]
fn default_init_does_not_reopen_top_level_task_gate_mid_block() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
init:
  pin progress: i32 = 0
task load():
  progress += 1
  yield
  progress += 10
block:
  await load()
  sample:
    out1 = f32(progress)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked_segment(
        &mut instance,
        0,
        0,
        PROCESS_BEGIN_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("zero-frame begin should advance and yield the task");
    process_checked_segment(&mut instance, 0, 2, 0, ExecutionOutput::none())
        .expect("first audio segment should observe the yielded task");
    assert_eq!(output, [0.0, 0.0, 99.0, 99.0]);

    init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
    process_checked_segment(
        &mut instance,
        2,
        2,
        PROCESS_END_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("default init must not reopen the top-level task gate");
    assert_eq!(output, [0.0; BLOCK_SIZE]);

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("next block should complete the task");
    assert_eq!(output, [11.0; BLOCK_SIZE]);
}

#[test]
fn suspended_task_can_be_bypassed_and_later_resumed() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Loader:
  init:
    enabled: bool = false
    pin progress: i32 = 0
  event set_enabled(value: bool):
    enabled = value
  task load():
    progress += 1
    yield
    progress += 10
  block:
    if enabled:
      await load()
    sample:
      out1 = f32(progress)
init:
  loader = Loader()
event set_enabled(value: bool):
  loader.set_enabled(value)
sample:
  out1 = loader()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    let set_enabled = instance
        .event_index("set_enabled")
        .expect("set_enabled event");

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("disabled task should be bypassed");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    trigger_event_by_index(&mut instance, set_enabled, &[1], ExecutionOutput::none())
        .expect("enable event should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("enabled task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);

    trigger_event_by_index(&mut instance, set_enabled, &[0], ExecutionOutput::none())
        .expect("disable event should run");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("suspended task should not gate a bypassed block");
    assert_eq!(output, [1.0; BLOCK_SIZE]);

    trigger_event_by_index(&mut instance, set_enabled, &[1], ExecutionOutput::none())
        .expect("re-enable event should run");
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should resume and complete");
    assert_eq!(output, [11.0; BLOCK_SIZE]);
}

#[test]
fn task_fixed_array_and_loop_frame_survive_each_yield() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    values: i32[4] = [3, 5, 7, 11]
    total: i32 = 0
    for i in 0..4:
      total += values[i]
      yield
    result = total
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    for expected_yields in 1..=4 {
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task iteration should yield");
        assert_eq!(
            output, [0.0; BLOCK_SIZE],
            "iteration {expected_yields} must remain behind the await barrier"
        );
    }
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete after the loop");
    assert_eq!(output, [26.0; BLOCK_SIZE]);
}

#[test]
fn task_loop_frame_does_not_overwrite_similarly_named_local() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
init:
  pin result: i32 = 0
task prepare():
  i__end: i32 = 99
  for i in 0..2:
    yield
  result = i__end

block:
  await prepare()

sample:
  out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    for _ in 0..2 {
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("loop task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        output.fill(99.0);
    }
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("loop task should complete");
    assert_eq!(output, [99.0; BLOCK_SIZE]);
}

#[test]
fn task_barrier_neutralizes_every_array_output_element() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
outs:
  stereo: f32[2]

task prepare():
  yield

block:
  await prepare()
  sample:
    stereo[0] = 1.0
    stereo[1] = 2.0
"#,
        BLOCK_SIZE,
        2,
    );
    let mut output = [99.0_f32; BLOCK_SIZE * 2];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("array output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE * 2]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(output, [1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
}

#[test]
fn task_return_inside_for_completes_without_undeclared_loop_state() {
    const BLOCK_SIZE: usize = 4;
    let sources = [
        r#"
init:
  pin result: i32 = 0

task prepare():
  for outer in 0..4:
    for inner in 0..4:
      if outer == 1:
        if inner == 2:
          return
      result += 1

block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        r#"
proc Loader:
  init:
    pin result: i32 = 0

  task prepare():
    for outer in 0..4:
      for inner in 0..4:
        if outer == 1:
          if inner == 2:
            return
        result += 1

  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#,
    ];

    for source in sources {
        let mut instance = compile_test_instance(source, BLOCK_SIZE, 1);
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [6.0; BLOCK_SIZE]);
    }
}

#[test]
fn task_symbols_are_injective_and_authored_state_is_explicit() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
init:
  pin foo____onda_bar: i32 = 0
task foo():
  bar_pc: i32 = 7
  yield
  foo____onda_bar = bar_pc

task foo_local_bar():
  yield

block:
  await foo()
  await foo_local_bar()

sample:
  out1 = f32(foo____onda_bar)
"#,
        BLOCK_SIZE,
        1,
    );
    assert_eq!(instance.state_count(), 1);
    assert_eq!(instance.state_name(0), Some("foo____onda_bar"));

    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("first task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("second task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("both tasks should complete");
    assert_eq!(output, [7.0; BLOCK_SIZE]);
}

#[test]
fn task_bindings_preserve_lexical_shadowing_across_yield() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Loader:
  init:
    pin state_value: i32 = 40
    pin result: i32 = 0
  task load():
    state_value: i32 = 2
    if state_value == 2:
      carried: i32 = 3
      yield
      result = carried
    state_value: bool = true
    if state_value:
      result += 10
  block:
    await load()
    sample:
      out1 = f32(result + state_value)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(output, [53.0; BLOCK_SIZE]);
}

#[test]
fn task_bindings_join_declarations_from_both_if_branches() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
params:
  choose = false
init:
  pin result: i32 = 0
task prepare():
  if choose:
    carried: i32 = 3
  else:
    carried: i32 = 5
  yield
  result = carried
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(output, [5.0; BLOCK_SIZE]);
}

#[test]
fn task_frame_typing_uses_the_selected_overload() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
def value(x: i32) -> i32:
  return x + 1
def value(x: f64) -> f64:
  return x + 2.0
init:
  pin result: i32 = 0
task prepare():
  carried = value(i32(3))
  yield
  result = carried
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(output, [4.0; BLOCK_SIZE]);
}

#[test]
fn task_mutations_of_aggregate_state_accumulate_across_yield() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
struct Accumulator:
  value: i32 = 0

proc Loader:
  init:
    pin accumulator = Accumulator()
  task load():
    accumulator.value += 1
    yield
    accumulator.value += 1
  block:
    await load()
    sample:
      out1 = f32(accumulator.value)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).expect("task should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("task should complete");
    assert_eq!(output, [2.0; BLOCK_SIZE]);
}

#[test]
fn tasks_in_runtime_indexed_proc_arrays_activate_lazily_per_slot() {
    const BLOCK_SIZE: usize = 4;
    let mut instance = compile_test_instance(
        r#"
proc Child:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 10
  block:
    await load()
    sample:
      out1 = f32(progress)
proc Parent:
  init:
    children: Child[2] = Child()
  sample:
    out1 = children[0]() + children[1]()
init:
  parent = Parent()
sample:
  out1 = parent()
"#,
        BLOCK_SIZE,
        1,
    );
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }

    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("both child tasks should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("both child tasks should complete");
    assert_eq!(output, [22.0; BLOCK_SIZE]);

    init(&mut instance, InitMode::PreservePinned)
        .expect("default init should preserve nested task state");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("default init should preserve completed child tasks");
    assert_eq!(output, [22.0; BLOCK_SIZE]);

    init(&mut instance, InitMode::Full).expect("full init should restart nested tasks");
    output.fill(99.0);
    process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
        .expect("restarted child tasks should yield");
    assert_eq!(output, [0.0; BLOCK_SIZE]);
}

#[test]
fn failed_task_invalidates_instance_until_full_init_or_restore() {
    const BLOCK_SIZE: usize = 4;
    let parsed = parse_program(
        r#"
proc Loader:
  init:
    bad_zero: i32 = 0
  event retry():
    load.reset()
  task load():
    ignored: i32 = 1 / bad_zero
    yield
  block:
    await load()
    sample:
      out1 = 1.0
init:
  loader = Loader()
event retry():
  loader.retry()
sample:
  out1 = loader()
"#,
    )
    .expect("failing task source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: BLOCK_SIZE,
        },
    )
    .expect("failing task source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("failing task should lower");
    let program = jit_program_from_optimized_mir(mir).expect("failing task MIR should compile");
    let mut instance = create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: BLOCK_SIZE,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("failing task instance should initialize");
    let mut output = [99.0_f32; BLOCK_SIZE];
    unsafe {
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .expect("output should bind");
    }
    assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
    output.fill(99.0);
    assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
    assert_eq!(output, [99.0; BLOCK_SIZE]);

    let retry = instance.event_index("retry").expect("retry event");
    assert!(trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none()).is_err());

    init(&mut instance, InitMode::Full).expect("full init should recover the instance");
    assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
}

#[test]
fn cloned_programs_process_concurrently_after_the_original_owner_is_dropped() {
    const INSTANCE_COUNT: usize = 8;
    const BLOCK_SIZE: usize = 64;

    let parsed = parse_program("sample:\n  out1 = 0.25\n").expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: BLOCK_SIZE,
        },
    )
    .expect("source should analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("typed program should lower to MIR");
    let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
    let config = InstanceConfig {
        sample_rate: 48_000.0,
        frames_per_block: BLOCK_SIZE,
        in_channels: 0,
        out_channels: 1,
    };
    let instances = (0..INSTANCE_COUNT)
        .map(|_| {
            create_instance_initialized(program.clone(), config)
                .expect("instance should initialize")
        })
        .collect::<Vec<_>>();

    drop(program);

    let threads = instances
        .into_iter()
        .map(|mut instance| {
            std::thread::spawn(move || {
                let mut output = vec![0.0_f32; BLOCK_SIZE];
                unsafe {
                    bind_output(
                        &mut instance,
                        0,
                        output.as_mut_ptr().cast(),
                        std::mem::size_of_val(output.as_slice()),
                    )
                    .expect("output should bind");
                }
                prepare_unchecked_process(&mut instance)
                    .expect("unchecked processing should prepare");
                for _ in 0..32 {
                    unsafe {
                        process_unchecked(&mut instance, ExecutionOutput::none())
                            .expect("concurrent JIT processing should succeed");
                    }
                    assert!(output.iter().all(|sample| *sample == 0.25));
                    output.fill(0.0);
                }
            })
        })
        .collect::<Vec<_>>();

    for thread in threads {
        thread.join().expect("processing thread should not panic");
    }
}

#[test]
fn checked_bindings_reject_misaligned_primitive_addresses() {
    let storage = [0_u64; 2];
    let aligned = storage.as_ptr().cast::<u8>();
    let misaligned = unsafe { aligned.add(1) };

    for ty in [
        PrimitiveType::F32,
        PrimitiveType::F64,
        PrimitiveType::I32,
        PrimitiveType::I64,
    ] {
        validate_pointer_alignment(aligned, ty, "test", "value")
            .expect("u64 storage should satisfy primitive alignment");
        let error = validate_pointer_alignment(misaligned, ty, "test", "value")
            .expect_err("offset byte pointer must be rejected");
        assert!(error.message.contains("alignment"));
    }
    validate_pointer_alignment(misaligned, PrimitiveType::Bool, "test", "value")
        .expect("byte elements have alignment one");
}

#[test]
fn checked_buffer_bindings_reject_wrapping_element_counts() {
    let error = validate_buffer_byte_extent(i32::MAX, 2, PrimitiveType::F32, "huge")
        .expect_err("buffer byte extent must fit the generated i32 ABI");
    assert!(error.message.contains("exceeds i32 runtime limit"));
    let f64_error = validate_buffer_byte_extent(i32::MAX / 8 + 1, 1, PrimitiveType::F64, "wide")
        .expect_err("f64 byte extent must fit i32 even when element count does");
    assert!(f64_error.message.contains("byte extent"));
    assert_eq!(
        validate_buffer_byte_extent(1024, 2, PrimitiveType::F32, "ok").unwrap(),
        8192
    );
}
