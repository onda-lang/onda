use super::*;

use onda_frontend::parse_program;
use onda_semantics::{
    analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions, TypedProgram,
};

fn source_program(source: &str, block_size: usize) -> (TypedProgram, Program) {
    let parsed = parse_program(source).expect("source should parse");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size,
        },
    )
    .expect("source should analyze");
    let mir = lower_program_to_optimized_mir(&typed)
        .expect("source should lower to MIR")
        .into_program();
    (typed, mir)
}

fn validate_test_buffer_abi(
    program: &Program,
    pointers: &[*mut u8],
    frames: &[i32],
    channels: &[i32],
    sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    validate_buffer_abi(
        program,
        BufferDescriptorTables::new(pointers, frames, channels, sample_rates),
    )
}

fn trusted_optimized(program: Program) -> Result<onda_mir::OptimizedProgram, Vec<MirCodegenError>> {
    let validated =
        unsafe { onda_mir::validate_owned_with_producer_proofs(program) }.map_err(|errors| {
            errors
                .into_iter()
                .map(|error| MirCodegenError::invalid(error.to_string()))
                .collect::<Vec<_>>()
        })?;
    onda_mir::optimize(validated)
        .map(|(program, _)| program)
        .map_err(|errors| {
            errors
                .into_iter()
                .map(|error| MirCodegenError::invalid(error.to_string()))
                .collect()
        })
}

fn lower_mir_and_jit(program: Program) -> Result<MirJitProgram, Vec<MirCodegenError>> {
    lower_optimized_mir_and_jit(trusted_optimized(program)?)
}

fn lower_mir_and_jit_with_options(
    program: Program,
    options: MirCompileOptions,
) -> Result<MirJitProgram, Vec<MirCodegenError>> {
    lower_optimized_mir_and_jit_with_options(trusted_optimized(program)?, options)
}

fn lower_mir_to_llvm_ir_with_options(
    program: &Program,
    options: MirCompileOptions,
) -> Result<String, Vec<MirCodegenError>> {
    lower_optimized_mir_to_llvm_ir_with_options(&trusted_optimized(program.clone())?, options)
}

fn lower_mir_to_target_llvm_ir(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<String, Vec<MirCodegenError>> {
    lower_optimized_mir_to_target_llvm_ir(&trusted_optimized(program.clone())?, options)
}

fn lower_mir_to_object_artifact(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<crate::AotObjectArtifact, Vec<MirCodegenError>> {
    lower_optimized_mir_to_object_artifact(&trusted_optimized(program.clone())?, options)
}

trait CheckedHostCalls {
    #[allow(clippy::too_many_arguments)]
    fn test_process_checked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        start_frame: usize,
        frames: usize,
        flags: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic>;

    #[allow(clippy::too_many_arguments)]
    fn test_trigger_event_by_index(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic>;
}

impl CheckedHostCalls for MirJitProgram {
    fn test_process_checked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        start_frame: usize,
        frames: usize,
        flags: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        unsafe {
            self.process_checked(
                state,
                params,
                start_frame,
                frames,
                flags,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
                None,
            )
        }
    }

    fn test_trigger_event_by_index(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        unsafe {
            self.trigger_event_by_index(
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
                None,
            )
        }
    }
}

fn run_native_outputs(source: &str, block_size: usize) -> Vec<Vec<f32>> {
    run_native_outputs_with_opt_level(source, block_size, TargetOptLevel::O0)
}

fn run_native_outputs_with_opt_level(
    source: &str,
    block_size: usize,
    opt_level: TargetOptLevel,
) -> Vec<Vec<f32>> {
    let (_, mir) = source_program(source, block_size);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level,
        },
    )
    .expect("native MIR LLVM backend should compile");
    let params = native.default_param_bytes();
    let mut state = native
        .initialize_state(&params)
        .expect("native state should initialize");
    let mut outputs = vec![vec![0.0_f32; block_size]; native.required_out_channels()];
    let output_ptrs = outputs
        .iter_mut()
        .map(|output| output.as_mut_ptr().cast::<u8>())
        .collect::<Vec<_>>();
    let inputs: [*const u8; 0] = [];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            block_size,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &output_ptrs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .expect("native process should run");
    outputs
}

#[test]
fn split_zero_frame_and_flag_segments_execute_expected_hooks() {
    let source = r#"
ins:
  in1 = 0.0

init:
  value = 0.0

block:
  value = value + 1.0
  sample:
    out1 = value + in1
  value = value + 10.0
"#;
    let block_size = 8;
    let (_, mir) = source_program(source, block_size);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    let native_params = native.default_param_bytes();
    let mut native_state = native.initialize_state(&native_params).unwrap();
    let mut native_output = vec![-99.0_f32; block_size];
    let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
    let input = [0.0_f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
    let inputs = [input.as_ptr().cast::<u8>()];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];

    macro_rules! run_segment {
        ($start:expr, $frames:expr, $flags:expr) => {{
            native
                .test_process_checked(
                    &mut native_state,
                    &native_params,
                    $start,
                    $frames,
                    $flags,
                    &inputs,
                    &native_outputs,
                    &buffers,
                    &metadata_i32,
                    &metadata_i32,
                    &metadata_f32,
                )
                .unwrap();
        }};
    }

    run_segment!(0, 3, onda_mir::PROCESS_BEGIN_BLOCK as u32);
    run_segment!(3, 0, 0);
    run_segment!(3, 5, onda_mir::PROCESS_END_BLOCK as u32);
    assert_eq!(native_output, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);

    // Zero-frame calls still run independently gated hooks. Exercise all
    // four legal flag combinations without imposing positional rules.
    run_segment!(0, 0, 0);
    run_segment!(0, 0, onda_mir::PROCESS_BEGIN_BLOCK as u32);
    run_segment!(block_size, 0, onda_mir::PROCESS_END_BLOCK as u32);
    run_segment!(4, 0, onda_mir::PROCESS_FULL_BLOCK as u32);

    native_output.fill(-99.0);
    run_segment!(0, block_size, 0);
    assert_eq!(
        native_output,
        [33.0, 34.0, 35.0, 36.0, 37.0, 38.0, 39.0, 40.0]
    );
}

#[test]
fn checked_process_rejects_out_of_block_segments_and_unknown_flags() {
    let (_, mir) = source_program("sample:\n  out1 = 0.0\n", 8);
    let native = lower_mir_and_jit(mir).unwrap();
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let mut output = [0.0_f32; 8];
    let outputs = [output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    let mut run = |start_frame, frames, flags| {
        native.test_process_checked(
            &mut state,
            &params,
            start_frame,
            frames,
            flags,
            &inputs,
            &outputs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
    };
    assert!(run(9, 0, 0).unwrap_err().message.contains("exceeds"));
    assert!(run(7, 2, 0).unwrap_err().message.contains("exceeds"));
    assert!(run(0, 0, 4)
        .unwrap_err()
        .message
        .contains("outside BEGIN_BLOCK/END_BLOCK"));
}

#[test]
fn checked_process_rejects_null_and_misaligned_audio_channels() {
    let (_, mir) = source_program("ins:\n  in1 = 0.0\n\nsample:\n  out1 = in1\n", 8);
    let native = lower_mir_and_jit(mir).unwrap();
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let input = [0.0_f32; 8];
    let mut output = [0.0_f32; 8];
    let valid_inputs = [input.as_ptr().cast::<u8>()];
    let valid_outputs = [output.as_mut_ptr().cast::<u8>()];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    let mut run = |inputs: &[*const u8], outputs: &[*mut u8]| {
        native.test_process_checked(
            &mut state,
            &params,
            0,
            8,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            inputs,
            outputs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
    };

    let null_inputs = [std::ptr::null()];
    assert!(run(&null_inputs, &valid_outputs)
        .unwrap_err()
        .message
        .contains("input channel 0 (`in1`) pointer is null"));

    let null_outputs = [std::ptr::null_mut()];
    assert!(run(&valid_inputs, &null_outputs)
        .unwrap_err()
        .message
        .contains("output channel 0 (`out1`) pointer is null"));

    let mut aligned_storage = [0_u32; 9];
    let misaligned_outputs = [aligned_storage.as_mut_ptr().cast::<u8>().wrapping_add(1)];
    assert!(run(&valid_inputs, &misaligned_outputs)
        .unwrap_err()
        .message
        .contains("requires 4-byte alignment"));
}

#[test]
fn source_input_and_value_call_produces_expected_samples() {
    let source = r#"
params:
  gain = 0.75 { 0.0, 1.0 }

def shape(x: f32, amount: f32) -> f32:
  if (x < 0.0):
    return -x * amount
  return x * amount

sample:
  out1 = shape(in1, gain)
"#;
    let block_size = 8;
    let (_, mir) = source_program(source, block_size);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    let native_params = native.default_param_bytes();
    let mut native_state = native.initialize_state(&native_params).unwrap();
    let input = [-1.0_f32, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0];
    let inputs = [input.as_ptr().cast::<u8>()];
    let mut native_output = vec![0.0_f32; block_size];
    let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    native
        .test_process_checked(
            &mut native_state,
            &native_params,
            0,
            block_size,
            3,
            &inputs,
            &native_outputs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .unwrap();
    assert_eq!(
        native_output,
        [0.75, 0.375, 0.1875, 0.0, 0.1875, 0.375, 0.5625, 0.75]
    );
}

#[test]
fn source_fixed_array_parameter_produces_expected_samples() {
    let source = r#"
params:
  weights: f32[3] = [0.25, 0.5, 1.0]

sample:
  out1 = weights[0] + weights[1] + weights[2]
"#;
    let outputs = run_native_outputs(source, 4);
    assert_eq!(outputs, [vec![1.75; 4]]);
}

#[test]
fn tuple_destructuring_normalizes_ranged_indices_before_unchecked_access() {
    let source = r#"
sample:
  values: f32[4] = [10.0, 20.0, 30.0, 40.0]
  clamped: i32 = 0 {4}
  wrapped: i32 = 0 {4, wrap}
  (clamped, wrapped) = (100, -1)
  out1 = values[clamped] + values[wrapped]
"#;
    let outputs = run_native_outputs_with_opt_level(source, 4, TargetOptLevel::O3);
    assert_eq!(outputs, [vec![80.0; 4]]);
}

#[test]
fn for_induction_does_not_wrap_at_i32_endpoints() {
    let source = r#"
const I32_MIN = -2147483647 - 1
const I32_MIN_PLUS_ONE = -2147483647

sample:
  visits = 0
  for i in 2147483647..=2147483647:
    visits += 1
    continue
  for i @ 2 in 2147483646..=2147483647:
    visits += 1
    continue
  for i @ -1 in I32_MIN..=I32_MIN:
    visits += 1
    continue
  for i @ -2 in I32_MIN_PLUS_ONE..=I32_MIN:
    visits += 1
    continue
  total = 0
  for i @ 3 in 0..=10:
    total += i
  for i @ -3 in 10..=0:
    total += i
  out1 = f32(visits)
  out2 = f32(total)
"#;
    let (_, mir) = source_program(source, 1);
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("constant loops should emit LLVM IR");
    assert!(
        !ir.contains("trunc i64"),
        "constant loops should keep an i32 induction path even at O0: {ir}"
    );
    let outputs = run_native_outputs_with_opt_level(source, 1, TargetOptLevel::O0);
    assert_eq!(outputs, [vec![4.0], vec![40.0]]);
}

#[test]
fn dynamic_for_induction_stays_i32_without_truncation() {
    let source = r#"
params:
  count: i32 = 4

sample:
  total = 0
  for i in 0..count:
    total += i
  out1 = f32(total)
"#;
    let (_, mir) = source_program(source, 1);
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("dynamic loops should emit LLVM IR");
    assert!(
        !ir.contains("trunc i64"),
        "dynamic loops must stay i32: {ir}"
    );
    assert!(
        ir.contains("add i32"),
        "dynamic induction must use i32: {ir}"
    );
    let outputs = run_native_outputs_with_opt_level(source, 1, TargetOptLevel::O0);
    assert_eq!(outputs, [vec![6.0]]);
}

#[test]
fn llvm_receives_proven_storage_and_call_boundary_ranges() {
    let (_, mir) = source_program(
        r#"
params:
  selector: i32 = 0 {min = 0, max = 3}

init:
  cursor: i32 = 0 {-4..4, wrap}

def preserve(index: i32) -> i32:
  return index

sample:
  out1 = f32(selector + preserve(cursor))
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("ranged storage should emit LLVM IR");

    assert!(
        ir.lines().any(|line| {
            line.contains("load i32") && line.contains("%state_slot") && line.contains("!range")
        }),
        "ranged state loads should carry !range metadata: {ir}"
    );
    assert!(
        ir.lines().any(|line| {
            line.contains("load i32") && line.contains("%param_slot") && !line.contains("!range")
        }),
        "raw interface-parameter loads must not carry !range metadata: {ir}"
    );
    assert!(
        ir.lines().any(|line| {
            line.contains("define internal") && line.matches("range(i32 -4, 4)").count() == 2
        }),
        "inferred value parameters and returns should both carry range attributes: {ir}"
    );
    assert!(
        ir.lines().any(|line| {
            line.contains("load i32") && line.contains("%param_0") && line.contains("!range")
        }),
        "loads should retain inferred call-boundary ranges: {ir}"
    );
}

#[test]
fn array_window_accepts_equivalent_duplicate_element_type_ids() {
    let i32_ty = onda_mir::TypeId::new(0);
    let source_element = onda_mir::TypeId::new(1);
    let parameter_element = onda_mir::TypeId::new(2);
    let source_array = onda_mir::TypeId::new(3);
    let parameter_array = onda_mir::TypeId::new(4);
    let mut program = Program::new(
        onda_mir::CompileConfig {
            sample_rate: 48_000.0,
            block_size: 8,
        },
        onda_mir::FunctionId::new(0),
        onda_mir::FunctionId::new(1),
    );
    program.types = vec![
        Type::Scalar(onda_mir::ScalarType::I32),
        Type::Scalar(onda_mir::ScalarType::F32),
        Type::Scalar(onda_mir::ScalarType::F32),
        Type::Array {
            element: source_element,
            len: 4,
        },
        Type::Array {
            element: parameter_element,
            len: 2,
        },
    ];
    program.state.push(onda_mir::StateSlot {
        integer_range: None,
        name: "source".to_owned(),
        ty: source_array,
        persistence: onda_mir::StatePersistence::Snapshot,
        authored: true,
        pinned: false,
    });

    let empty_function = |name: &str, kind| onda_mir::Function {
        name: name.to_owned(),
        kind,
        attributes: onda_mir::FunctionAttributes::default(),
        params: Vec::new(),
        results: Vec::new(),
        locals: Vec::new(),
        body: onda_mir::Block::default(),
        source: onda_mir::SourceSpan::UNKNOWN,
    };
    let init = empty_function("onda_init", FunctionKind::Init);
    let mut process = empty_function("onda_process", FunctionKind::Process);
    process.params = onda_mir::process_function_params(i32_ty);
    process.body.statements.push(onda_mir::Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: onda_mir::FunctionId::new(2),
            args: vec![CallArgument::ArrayWindow {
                array: onda_mir::Place {
                    base: onda_mir::PlaceBase::State(onda_mir::StateId::new(0)),
                    projections: Vec::new(),
                },
                start: onda_mir::Value::Constant(onda_mir::ScalarValue::I32(1)),
                bounds: onda_mir::BoundsMode::Checked,
            }],
        },
        source: onda_mir::SourceSpan::UNKNOWN,
    });
    let mut callee = empty_function("consume_window", FunctionKind::User);
    callee.params.push(onda_mir::FunctionParam {
        integer_range: None,
        name: "window".to_owned(),
        ty: parameter_array,
        mode: onda_mir::PassingMode::ReadOnlyReference,
    });
    program.functions = vec![init, process, callee];

    onda_mir::validate(&program)
        .expect("duplicate scalar type IDs are structurally equivalent in MIR");
    lower_mir_to_llvm_ir_with_options(
        &program,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("LLVM should legalize a structurally equivalent array window");
}

#[test]
fn source_intrinsic_set_executes_through_mir_llvm() {
    let source = r#"
sample:
  x = f32(0.25)
  out1 = sin(x) + cos(x) + tan(x) + tanh(x) + atan(x) + atan2(x, f32(0.5)) + exp(x) + log(f32(1.0) + x) + sqrt(x) + pow(x, f32(2.0)) + abs(-x) + floor(x) + ceil(x) + round(x) + trunc(x) + min(x, f32(0.5)) + max(x, f32(0.5)) + fma(x, f32(2.0), f32(0.125))
"#;
    let outputs = run_native_outputs(source, 2);
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0][0].is_finite());
    assert_eq!(outputs[0][0], outputs[0][1]);
}

#[test]
fn runtime_slice_source_produces_expected_samples() {
    let source = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

def sum_edges(values: f32[]) -> f32:
  return values[0] + values[values.len() - 1]

sample:
  out1 = sum_edges(Table)
"#;
    let outputs = run_native_outputs(source, 4);
    assert_eq!(outputs, [vec![1.25; 4]]);
}

#[test]
fn runtime_buffer_and_forwarded_buffer_parameter_execute_expected_behavior() {
    let source = r#"
outs:
  out1

buffers:
  table: f32

def touch(buf: buffer<f32>, index: i32):
  view = buf[:]
  value = buf[index] + view[index] - view[index]
  buf[index] = value + 1.0
  return value + f32(buf.len()) + f32(buf.chans()) + buf.samplerate()

def forward(buf: buffer<f32>, index: i32):
  return touch(buf, index)

sample:
  out1 = forward(table, 0)
"#;
    let block_size = 4;
    let (_, mir) = source_program(source, block_size);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    assert_eq!(native.buffer_count(), 1);

    let native_params = native.default_param_bytes();
    let mut native_state = native.initialize_state(&native_params).unwrap();
    let mut native_buffer = [2.0_f32, 3.0, 4.0, 5.0];
    let native_buffers = [native_buffer.as_mut_ptr().cast::<u8>()];
    let buffer_frames = [4_i32];
    let buffer_channels = [1_i32];
    let buffer_sample_rates = [100.0_f32];
    let mut native_output = [0.0_f32; 4];
    let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];

    native
        .test_process_checked(
            &mut native_state,
            &native_params,
            0,
            block_size,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &native_outputs,
            &native_buffers,
            &buffer_frames,
            &buffer_channels,
            &buffer_sample_rates,
        )
        .unwrap();

    assert_eq!(native_output, [107.0, 108.0, 109.0, 110.0]);
    assert_eq!(native_buffer, [6.0, 3.0, 4.0, 5.0]);
}

#[test]
fn optimized_process_observes_descriptor_rebinding_between_calls() {
    let source = r#"
buffers:
  bank: f32 {2}

outs:
  out1

sample:
  out1 = bank[0][0] + bank[1][0] * 2.0
"#;
    let block_size = 1;
    let (_, mir) = source_program(source, block_size);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("buffer rebinding source should JIT");
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let mut first = [1.0_f32];
    let mut second = [10.0_f32];
    let mut rebound_first = [3.0_f32];
    let mut rebound_second = [20.0_f32];
    let mut buffers = [
        first.as_mut_ptr().cast::<u8>(),
        second.as_mut_ptr().cast::<u8>(),
    ];
    let frames = [1_i32; 2];
    let channels = [1_i32; 2];
    let sample_rates = [48_000.0_f32; 2];
    let mut output = [0.0_f32];
    let outputs = [output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];

    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            block_size,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &frames,
            &channels,
            &sample_rates,
        )
        .unwrap();
    assert_eq!(output, [21.0]);

    buffers.copy_from_slice(&[
        rebound_first.as_mut_ptr().cast::<u8>(),
        rebound_second.as_mut_ptr().cast::<u8>(),
    ]);
    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            block_size,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &frames,
            &channels,
            &sample_rates,
        )
        .unwrap();
    assert_eq!(output, [43.0]);
}

#[test]
fn block_buffer_aliases_preserve_selection_and_resolve_rebound_descriptors() {
    let source = r#"
buffers:
  bank: f32 {2}
params:
  selector: i32 = 1 {0, 1}
block:
  selected = bank[selector]
sample:
  out1 = selected[0]
"#;
    let block_size = 2;
    let (_, mir) = source_program(source, block_size);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("buffer alias source should JIT");
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let mut first = [1.0_f32];
    let mut selected = [10.0_f32];
    let mut rebound_selected = [20.0_f32];
    let mut buffers = [
        first.as_mut_ptr().cast::<u8>(),
        selected.as_mut_ptr().cast::<u8>(),
    ];
    let frames = [1_i32; 2];
    let channels = [1_i32; 2];
    let sample_rates = [48_000.0_f32; 2];
    let mut output = [0.0_f32; 2];
    let outputs = [output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];

    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            1,
            onda_mir::PROCESS_BEGIN_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &frames,
            &channels,
            &sample_rates,
        )
        .unwrap();
    assert_eq!(output, [10.0, 0.0]);

    buffers[1] = rebound_selected.as_mut_ptr().cast::<u8>();
    native
        .test_process_checked(
            &mut state,
            &params,
            1,
            1,
            onda_mir::PROCESS_END_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &frames,
            &channels,
            &sample_rates,
        )
        .unwrap();
    assert_eq!(output, [10.0, 20.0]);
}

#[test]
fn fixed_buffer_arrays_select_contiguous_descriptors_and_forward_elements() {
    let source = r#"
buffers:
  bank: f32 {3}
  stereo: f32[2] {2}
  single: f32 {1}

outs:
  out1

def first(buf: buffer<f32>):
  return buf[0]

init:
  selector: i32 = 99

sample:
  out1 = first(bank[selector]) + stereo[0][1, 0] + f32(bank.len()) + f32(bank[0].len()) + f32(bank[0].chans()) + bank[0].samplerate() + f32(single.len()) + f32(single[0].len())
"#;
    let (_, mir) = source_program(source, 1);
    assert_eq!(mir.interface.buffers.len(), 6);
    assert_eq!(mir.interface.buffer_arrays.len(), 3);
    assert_eq!(mir.interface.buffer_arrays[0].name, "bank");
    assert_eq!(mir.interface.buffer_arrays[0].first.index(), 0);
    assert_eq!(mir.interface.buffer_arrays[0].len, 3);
    assert_eq!(mir.interface.buffer_arrays[2].name, "single");
    assert_eq!(mir.interface.buffer_arrays[2].len, 1);

    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    let native_params = native.default_param_bytes();
    let mut native_state = native.initialize_state(&native_params).unwrap();
    let mut bank0 = [1.0_f32];
    let mut bank1 = [2.0_f32];
    let mut bank2 = [3.0_f32];
    let mut stereo0 = [10.0_f32, 20.0];
    let mut stereo1 = [30.0_f32, 40.0];
    let mut single = [50.0_f32];
    let buffers = [
        bank0.as_mut_ptr().cast::<u8>(),
        bank1.as_mut_ptr().cast::<u8>(),
        bank2.as_mut_ptr().cast::<u8>(),
        stereo0.as_mut_ptr().cast::<u8>(),
        stereo1.as_mut_ptr().cast::<u8>(),
        single.as_mut_ptr().cast::<u8>(),
    ];
    let buffer_frames = [1_i32; 6];
    let buffer_channels = [1_i32, 1, 1, 2, 2, 1];
    let buffer_sample_rates = [100.0_f32; 6];
    let mut output = [0.0_f32];
    let outputs = [output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];

    native
        .test_process_checked(
            &mut native_state,
            &native_params,
            0,
            1,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &buffer_frames,
            &buffer_channels,
            &buffer_sample_rates,
        )
        .unwrap();

    // selector 99 clamps to bank[2].
    assert_eq!(output, [130.0]);
}

#[test]
fn nested_proc_buffer_spans_forward_and_select_in_constant_space() {
    let source = r#"
buffers:
  bank: f32 {4}
outs:
  out1

proc Child:
  params:
    slot: i32 = 1
  buffers:
    clips: f32 {2}
  outs:
    out1
  sample:
    out1 = clips[slot][0]

proc Parent:
  buffers:
    clips: f32 {3}
  init:
    child = Child(clips = clips[1:3])
  outs:
    out1
  sample:
    out1 = child()

init:
  parent = Parent(clips = bank[1:4])
sample:
  out1 = parent()
"#;
    let (_, mir) = source_program(source, 1);
    assert!(mir.functions.iter().any(|function| {
        function.name == "Parent.__onda_proc_step"
            && function.params.iter().any(|parameter| {
                matches!(
                    mir.types[parameter.ty.index()],
                    Type::BufferSpan { len: 3, .. }
                )
            })
    }));

    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let mut bank = [[1.0_f32], [2.0], [3.0], [4.0]];
    let buffers = bank
        .iter_mut()
        .map(|slot| slot.as_mut_ptr().cast::<u8>())
        .collect::<Vec<_>>();
    let frames = [1_i32; 4];
    let channels = [1_i32; 4];
    let sample_rates = [48_000.0_f32; 4];
    let mut output = [0.0_f32];
    let outputs = [output.as_mut_ptr().cast::<u8>()];

    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            1,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &[],
            &outputs,
            &buffers,
            &frames,
            &channels,
            &sample_rates,
        )
        .unwrap();

    assert_eq!(output, [4.0]);
}

#[test]
fn multichannel_buffer_coordinates_clamp_independently() {
    let source = r#"
buffers:
  stereo: f32[2]

sample:
  out1 = stereo[-1, 99] + stereo[99, -1]
"#;
    let (_, mir) = source_program(source, 1);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    let native_params = native.default_param_bytes();
    let mut native_state = native.initialize_state(&native_params).unwrap();
    let mut stereo = [10.0_f32, 20.0, 30.0, 40.0];
    let buffers = [stereo.as_mut_ptr().cast::<u8>()];
    let buffer_frames = [2_i32];
    let buffer_channels = [2_i32];
    let buffer_sample_rates = [48_000.0_f32];
    let mut output = [0.0_f32];
    let outputs = [output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];

    native
        .test_process_checked(
            &mut native_state,
            &native_params,
            0,
            1,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &buffer_frames,
            &buffer_channels,
            &buffer_sample_rates,
        )
        .unwrap();

    assert_eq!(output, [50.0]);
}

#[test]
fn dynamic_slice_event_payload_executes_and_is_validated() {
    let source = r#"
outs:
  out1

init:
  data: f32[4] = [0.0, 0.0, 0.0, 0.0]
  total = 0.0

events:
  fill(values: f32[]):
    data[:] = 0.0
    data[:] = values[:4]
    total = data[0] + data[1] + data[2] + data[3]

sample:
  out1 = total
"#;
    let (_, mir) = source_program(source, 1);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    assert_eq!(native.event_payload_byte_size(0), None);
    assert_eq!(
        native.event_payload_shape(0),
        Some(MirEventPayloadShape::Dynamic)
    );

    let native_params = native.default_param_bytes();
    let mut native_state = native.initialize_state(&native_params).unwrap();
    let mut payload = Vec::new();
    payload.extend_from_slice(&4_i32.to_ne_bytes());
    for value in [10.0_f32, 20.0, 30.0, 40.0] {
        payload.extend_from_slice(&value.to_ne_bytes());
    }
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    native
        .test_trigger_event_by_index(
            &mut native_state,
            &native_params,
            0,
            &payload,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .unwrap();

    let mut native_output = [0.0_f32; 1];
    let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
    let inputs: [*const u8; 0] = [];
    native
        .test_process_checked(
            &mut native_state,
            &native_params,
            0,
            1,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &native_outputs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .unwrap();
    assert_eq!(native_output, [100.0]);

    let invalid_payloads = [
        vec![0_u8; 2],
        (-1_i32).to_ne_bytes().to_vec(),
        {
            let mut truncated = 2_i32.to_ne_bytes().to_vec();
            truncated.extend_from_slice(&1.0_f32.to_ne_bytes());
            truncated
        },
        {
            let mut trailing = 0_i32.to_ne_bytes().to_vec();
            trailing.push(0);
            trailing
        },
    ];
    for invalid in invalid_payloads {
        assert!(native
            .test_trigger_event_by_index(
                &mut native_state,
                &native_params,
                0,
                &invalid,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .is_err());
    }
    let oversized = i32::MAX.to_ne_bytes();
    let error = native
        .test_trigger_event_by_index(
            &mut native_state,
            &native_params,
            0,
            &oversized,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .expect_err("dynamic event slice byte extent must fit i32");
    assert!(error.message.contains("byte extent exceeds i32"));
}

#[test]
fn control_output_uses_target_aligned_state_storage() {
    let source = r#"
kouts:
  meter: f64

block:
  meter = f64(3.25)
"#;
    let (_, mir) = source_program(source, 4);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .unwrap();
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let inputs: [*const u8; 0] = [];
    let outputs: [*mut u8; 0] = [];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            4,
            3,
            &inputs,
            &outputs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .unwrap();
    let offset = native.control_output_storage_byte_offset(0).unwrap();
    assert_eq!(native.control_output_storage_byte_offsets(), &[offset]);
    assert!(native.state_byte_offsets().contains(&offset));
    assert_eq!(offset % std::mem::align_of::<f64>(), 0);
    let value = f64::from_ne_bytes(state.bytes()[offset..offset + 8].try_into().unwrap());
    assert!((value - 3.25).abs() < f64::EPSILON);
}

#[test]
fn integer_and_nan_edge_semantics_execute_without_llvm_poison() {
    let shifted = run_native_outputs(
        r#"
params:
  count: i32 = 32

sample:
  out1 = f32((i32(1) << count) + (i32(-8) >> count))
"#,
        1,
    );
    assert_eq!(shifted[0], [-7.0]);

    let divided = run_native_outputs(
        r#"
params:
  divisor: i32 = -1

sample:
  minimum = i32(-2147483647) - i32(1)
  out1 = f32(minimum / divisor)
  out2 = f32(minimum % divisor)
"#,
        1,
    );
    assert_eq!(divided[0], [-2_147_483_648.0]);
    assert_eq!(divided[1], [0.0]);

    let nan = run_native_outputs(
        r#"
params:
  value = 0.0

sample:
  invalid = value / value
  out1 = 0.0
  if (invalid != invalid):
    out1 = f32(i32(invalid)) + 1.0
"#,
        1,
    );
    assert_eq!(nan[0], [1.0]);
}

#[test]
fn ranged_params_map_nan_to_minimum_with_and_without_fast_math() {
    let (_, mir) = source_program(
        r#"
params:
  value = 0.5 {-1.0, 1.0}

sample:
  out1 = value
"#,
        1,
    );
    let inputs: [*const u8; 0] = [];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    for fast_math in [false, true] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("ranged-param source should compile");
        let mut params = native.default_param_bytes();
        params[..4].copy_from_slice(&f32::NAN.to_ne_bytes());
        let mut state = native
            .initialize_state(&params)
            .expect("ranged-param state should initialize");
        let mut output = [0.0_f32];
        let outputs = [output.as_mut_ptr().cast::<u8>()];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &inputs,
                &outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .expect("ranged-param process should run");
        assert_eq!(output, [-1.0]);
    }
}

#[test]
fn numeric_edge_lowering_has_explicit_llvm_semantics() {
    let (_, mir) = source_program(
        r#"
params:
  count: i32 = 32
  divisor: i32 = -1
  value = 0.0

sample:
  shifted = i32(1) << count
  divided = shifted / divisor
  invalid = value / value
  converted = i32(invalid)
  if (invalid != invalid):
    out1 = f32(divided + converted)
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("numeric edge MIR should emit LLVM IR");
    assert!(ir.contains("and i32"));
    assert!(ir.contains(", 31"));
    assert!(ir.contains("fcmp une float"));
    assert!(ir.contains("@llvm.fptosi.sat.i32.f32"));
    assert!(!ir.contains("@llvm.trap"));
    assert!(ir.contains("i32 @onda_process("));
    assert!(ir.contains("sdiv i32"));

    let mut target = crate::TargetConfig::host();
    target.opt_level = TargetOptLevel::O0;
    let targeted_ir = lower_mir_to_target_llvm_ir(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            target,
        },
    )
    .expect("numeric edge MIR should emit targeted LLVM IR");
    assert!(!targeted_ir.contains("@llvm.trap"));
    assert!(targeted_ir.contains("i32 @onda_process("));
}

#[test]
fn generated_runtime_failure_returns_through_user_calls() {
    let (_, mir) = source_program(
        r#"
params:
  divisor: i32 = 0

def quotient(value: i32, by: i32):
  return value / by

sample:
  out1 = f32(quotient(i32(1), divisor))
"#,
        1,
    );
    let native = lower_mir_and_jit(mir).expect("failing source should JIT");
    let params = native.default_param_bytes();
    let mut state = native
        .initialize_state(&params)
        .expect("failing source should initialize");
    let inputs: [*const u8; 0] = [];
    let mut output = [0.0_f32];
    let outputs = [output.as_mut_ptr().cast::<u8>()];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    let error = native
        .test_process_checked(
            &mut state,
            &params,
            0,
            1,
            onda_mir::PROCESS_FULL_BLOCK as u32,
            &inputs,
            &outputs,
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .expect_err("division by zero should return a runtime failure");
    assert!(error.message.contains("runtime safety check"));
}

#[test]
fn recoverable_failure_helpers_remain_willreturn() {
    let (_, mut mir) = source_program(
        r#"
params:
  divisor: i32 = 1

def quotient(value: i32, by: i32):
  return value / by

sample:
  out1 = f32(quotient(i32(1), divisor))
"#,
        1,
    );
    let helper = mir
        .functions
        .iter_mut()
        .enumerate()
        .find_map(|(index, function)| {
            matches!(function.kind, FunctionKind::User).then(|| {
                function.attributes.inline = onda_mir::InlineHint::Never;
                index
            })
        })
        .expect("source should lower one user helper");
    let effects = onda_mir::analyze_effects(&mir);
    let helper_effects = effects.function(onda_mir::FunctionId::new(
        u32::try_from(helper).expect("function id should fit u32"),
    ));
    assert!(helper_effects.may_fail);
    assert!(!helper_effects.may_not_return);
    let symbol = format!("@__onda_mir_fn_{helper}");
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("failure-capable helper should emit LLVM IR");
    let definition = ir
        .lines()
        .find(|line| line.starts_with("define internal") && line.contains(&symbol))
        .expect("noinline helper definition");
    let attribute_id = definition
        .rsplit_once(" #")
        .and_then(|(_, suffix)| suffix.strip_suffix(" {"))
        .expect("helper definition should reference an attribute group");
    let attributes = ir
        .lines()
        .find(|line| line.starts_with(&format!("attributes #{attribute_id} =")))
        .expect("helper attribute group");
    assert!(attributes.contains("willreturn"), "{attributes}");
}

#[test]
fn generated_runtime_failure_returns_from_init_and_events() {
    let (_, init_mir) = source_program(
        r#"
params:
  divisor: i32 = 0

init:
  held = i32(1) / divisor

sample:
  out1 = f32(held)
"#,
        1,
    );
    let init_native = lower_mir_and_jit(init_mir).expect("failing init should JIT");
    let init_error = init_native
        .initialize_state(&init_native.default_param_bytes())
        .expect_err("division by zero in init should return a runtime failure");
    assert!(init_error.message.contains("runtime safety check"));

    let (_, event_mir) = source_program(
        r#"
init:
  held = i32(0)

event divide(divisor: i32) {
  held = i32(1) / divisor
}

sample:
  out1 = f32(held)
"#,
        1,
    );
    let event_native = lower_mir_and_jit(event_mir).expect("failing event should JIT");
    let params = event_native.default_param_bytes();
    let mut state = event_native
        .initialize_state(&params)
        .expect("event source should initialize");
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    let error = event_native
        .test_trigger_event_by_index(
            &mut state,
            &params,
            0,
            &0_i32.to_ne_bytes(),
            &buffers,
            &metadata_i32,
            &metadata_i32,
            &metadata_f32,
        )
        .expect_err("division by zero in an event should return a runtime failure");
    assert!(error.message.contains("runtime safety check"));
}

#[test]
fn non_failing_noinline_helpers_need_no_failure_propagation() {
    let (_, mut mir) = source_program(
        r#"
ins:
  in1

def ratio(x: f32):
  return x / 2.0

def pick(values: f32[4], index: i32):
  return values[index]

sample:
  values = [1.0, 2.0, 3.0, 4.0]
  out1 = ratio(in1) + pick(values, i32(in1))
"#,
        64,
    );
    let helper_ids = mir
        .functions
        .iter_mut()
        .enumerate()
        .filter_map(|(index, function)| {
            if matches!(function.kind, FunctionKind::User) {
                function.attributes.inline = onda_mir::InlineHint::Never;
                Some(index)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(helper_ids.len(), 2);

    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("non-failing noinline helpers should emit optimized LLVM IR");
    for helper in helper_ids {
        assert!(ir.lines().any(|line| {
            line.contains("call") && line.contains(&format!("@__onda_mir_fn_{helper}"))
        }));
    }
    assert!(!ir.contains("runtime_failure"));
    assert!(!ir.contains("call_failure"));
}

#[test]
fn fused_float_cast_and_fixed_clamp_preserve_edge_semantics() {
    let (_, mir) = source_program(
        r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

params:
  index = 0.0

def lookup(index):
  return Table[index]

sample:
  out1 = lookup(index)
"#,
        1,
    );
    let cases = [
        (f32::NAN, 10.0),
        (f32::from_bits(0x7f80_0001), 10.0),
        (f32::from_bits(0xff80_0001), 10.0),
        (f32::NEG_INFINITY, 10.0),
        (-3.5, 10.0),
        (-0.5, 10.0),
        (-0.0, 10.0),
        (0.999, 10.0),
        (1.0, 20.0),
        (1.999, 20.0),
        (2.0, 30.0),
        (2.999, 30.0),
        (f32::MAX, 30.0),
        (f32::INFINITY, 30.0),
    ];
    let inputs: [*const u8; 0] = [];
    let buffers: [*mut u8; 0] = [];
    let metadata_i32: [i32; 0] = [];
    let metadata_f32: [f32; 0] = [];
    for fast_math in [false, true] {
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("fused fixed-index source should compile");
        for (index, expected) in cases {
            let mut params = native.default_param_bytes();
            params[..4].copy_from_slice(&index.to_ne_bytes());
            let mut state = native
                .initialize_state(&params)
                .expect("fixed-index state should initialize");
            let mut output = [0.0_f32];
            let outputs = [output.as_mut_ptr().cast::<u8>()];
            native
                .test_process_checked(
                    &mut state,
                    &params,
                    0,
                    1,
                    onda_mir::PROCESS_FULL_BLOCK as u32,
                    &inputs,
                    &outputs,
                    &buffers,
                    &metadata_i32,
                    &metadata_i32,
                    &metadata_f32,
                )
                .expect("fixed-index edge case should process");
            assert_eq!(
                output[0], expected,
                "unexpected output for index {index:?} with fast_math={fast_math}"
            );
        }
    }
}

#[test]
fn optimized_fixed_float_index_fuses_saturation_with_clamp() {
    let (_, mir) = source_program(
        r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

params:
  index = 0.0

def lookup(index):
  return Table[index]

sample:
  out1 = lookup(index)
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("fused fixed-index source should emit LLVM IR");
    assert!(ir.contains("@llvm.maxnum.f32"));
    assert!(ir.contains("@llvm.minnum.f32"));
    assert!(ir.contains("fptosi float"));
    assert!(
        !ir.contains("@llvm.fptosi.sat.i32.f32"),
        "dead standalone saturation should disappear after fixed-index fusion"
    );

    let (_, wide_mir) = source_program(
        r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

params:
  index: f64 = f64(0.0)

def lookup(index: f64):
  return Table[index]

sample:
  out1 = lookup(index)
"#,
        1,
    );
    let wide_ir = lower_mir_to_llvm_ir_with_options(
        &wide_mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("f64 fixed-index source should emit LLVM IR");
    assert!(wide_ir.contains("@llvm.maxnum.f64"));
    assert!(wide_ir.contains("@llvm.minnum.f64"));
    assert!(wide_ir.contains("fptosi double"));
    assert!(!wide_ir.contains("@llvm.fptosi.sat.i32.f64"));
}

#[test]
fn optimized_range_wrap_uses_same_width_unsigned_remainders() {
    let (_, mir) = source_program(
        r#"
params:
  step: i32 = 1

init:
  index: i32 = 0 {0..1001, wrap}

sample:
  index += step
  out1 = f32(index)
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("range-wrapped state should emit optimized LLVM IR");
    assert!(ir.contains("range_wrap_slow"), "{ir}");
    assert!(ir.contains("urem i32"), "{ir}");
    assert!(!ir.contains("srem"), "{ir}");
    assert!(!ir.contains("urem i128"), "{ir}");
    assert!(ir.contains("icmp ult i32"), "{ir}");
    assert!(
        !ir.contains("%range_wrap_distance ="),
        "the zero lower bound should eliminate the distance subtraction: {ir}"
    );
    let branch = ir
        .find("br i1 %range_wrap_in_range")
        .expect("range check branch");
    let remainder = ir.find("urem i32").expect("slow-path remainder");
    assert!(
        branch < remainder,
        "the range check must dominate the remainder: {ir}"
    );

    let (_, i64_mir) = source_program(
        r#"
params:
  step: i64 = 1

init:
  index: i64 = 0 {-1000..1001, wrap}

sample:
  index += step
  out1 = f32(index)
"#,
        1,
    );
    let i64_ir = lower_mir_to_llvm_ir_with_options(
        &i64_mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("i64 range-wrapped state should emit optimized LLVM IR");
    assert!(i64_ir.contains("urem i64"), "{i64_ir}");
    assert!(!i64_ir.contains("srem"), "{i64_ir}");
    assert!(!i64_ir.contains("urem i128"), "{i64_ir}");
}

#[test]
fn range_wrap_slow_paths_preserve_inclusive_signed_semantics() {
    let outputs = run_native_outputs(
        r#"
init:
  descending: i32 = -2 {-2..3, wrap}
  ascending: i32 = 2 {-2..3, wrap}

sample:
  descending -= 2
  ascending += 2
  out1 = f32(descending)
  out2 = f32(ascending)
"#,
        5,
    );
    assert_eq!(outputs[0], [1.0, -1.0, 2.0, 0.0, -2.0]);
    assert_eq!(outputs[1], [-1.0, 1.0, -2.0, 0.0, 2.0]);

    let wide_outputs = run_native_outputs(
        r#"
params:
  below_step: i64 = -9223372036854775807
  above_step: i64 = 9223372036854775807

init:
  below: i64 = -1 {-9223372036854775807..3, wrap}
  above: i64 = 0 {-9223372036854775807..3, wrap}

sample:
  below += below_step
  above += above_step
  out1 = f32(below)
  out2 = f32(above)
"#,
        1,
    );
    assert_eq!(wide_outputs[0], [2.0]);
    assert_eq!(wide_outputs[1], [-3.0]);
}

#[test]
fn fused_index_provenance_counts_read_write_call_arguments() {
    let (_, mut mir) = source_program(
        r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

def lookup(index):
  return Table[index]

sample:
  out1 = lookup(1.0)
"#,
        1,
    );
    let (function_index, cast_local, source_local, source) = mir
        .functions
        .iter()
        .enumerate()
        .find_map(|(function_index, function)| {
            function.body.statements.iter().find_map(|statement| {
                let StatementKind::Assign {
                    destination,
                    value:
                        Rvalue::Cast {
                            value: onda_mir::Value::Local(source_local),
                            to: onda_mir::ScalarType::I32,
                        },
                } = &statement.kind
                else {
                    return None;
                };
                let onda_mir::PlaceBase::Local(cast_local) = destination.base else {
                    return None;
                };
                destination.projections.is_empty().then_some((
                    function_index,
                    cast_local,
                    *source_local,
                    statement.source,
                ))
            })
        })
        .expect("lookup MIR should contain a float-to-i32 index cast");
    assert!(
        fused_clamped_index_sources(&mir, &mir.functions[function_index])[cast_local.index()]
            .is_some()
    );

    let source_ty = mir.functions[function_index].locals[source_local.index()].ty;
    let mut mutator = mir.functions[function_index].clone();
    mutator.name = "__test_mutate_index_source".to_owned();
    mutator.params = vec![onda_mir::FunctionParam {
        integer_range: None,
        name: "value".to_owned(),
        ty: source_ty,
        mode: onda_mir::PassingMode::ReadWriteReference,
    }];
    mutator.results.clear();
    mutator.locals.clear();
    mutator.body = Block::default();
    let mutator_id = onda_mir::FunctionId::new(mir.functions.len() as u32);
    mir.functions.push(mutator);
    mir.functions[function_index]
        .body
        .statements
        .push(onda_mir::Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: mutator_id,
                args: vec![CallArgument::Place(Place::local(source_local))],
            },
            source,
        });
    assert!(
        fused_clamped_index_sources(&mir, &mir.functions[function_index])[cast_local.index()]
            .is_none(),
        "a mutable-reference call must invalidate immutable cast provenance"
    );
}

#[test]
fn raw_checked_buffer_abi_rejects_wrapping_extents_and_bad_rates() {
    let (_, mir) = source_program(
        r#"
buffers:
  data: f64[]
sample:
  out1 = 0.0
"#,
        1,
    );
    let mut storage = [0_u64; 1];
    let pointer = storage.as_mut_ptr().cast::<u8>();
    let pointers = [pointer];
    let overflow = validate_test_buffer_abi(&mir, &pointers, &[i32::MAX], &[2], &[48_000.0])
        .expect_err("wrapping buffer element count must be rejected");
    assert!(overflow.message.contains("exceeds i32"));

    let byte_overflow =
        validate_test_buffer_abi(&mir, &pointers, &[i32::MAX / 8 + 1], &[1], &[48_000.0])
            .expect_err("f64 byte extent must fit i32 even when element count does");
    assert!(byte_overflow.message.contains("byte extent"));

    for sample_rate in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let error = validate_test_buffer_abi(&mir, &pointers, &[1], &[1], &[sample_rate])
            .expect_err("invalid sample rate metadata must be rejected");
        assert!(error.message.contains("finite positive sample rate"));
    }
}

#[test]
fn clamped_buffer_accesses_omit_empty_range_guards() {
    let (_, mir) = source_program(
        r#"
buffers:
  data: f32
sample:
  out1 = data[0]
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("buffer read MIR should emit LLVM IR");
    assert!(!ir.contains("buffer_total_len"));
    assert!(ir.contains("dynamic_index_clamped"));
    assert!(!ir.contains("dynamic_len_positive"));
    assert!(!ir.contains("dynamic_clamp_nonempty"));
}

#[test]
fn direct_buffer_descriptors_are_snapshotted_outside_the_sample_loop() {
    let (_, mir) = source_program(
        r#"
buffers:
  data: f32[]
sample:
  out1 = data[0, 0] + data[1, 1] + f32(data.len()) + f32(data.chans()) + data.samplerate()
"#,
        8,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("direct buffer reads should emit LLVM IR");

    for (name, load) in [
        ("read pointer", "buffer_ptr"),
        ("frame count", "buffer_frames"),
        ("channel count", "buffer_channels"),
        ("sample rate", "buffer_sample_rate"),
    ] {
        assert_eq!(
            ir.lines()
                .filter(|line| line.contains(load) && line.contains(" = load "))
                .count(),
            1,
            "direct buffer {name} should be loaded once per process entry"
        );
    }
    assert!(ir.contains("buffer_load") && ir.contains("align 4"));
    assert!(
        ir.contains("!noalias !") && ir.contains("!alias.scope !"),
        "external-buffer, descriptor, and audio-output accesses should carry host-region scopes"
    );
    let metadata_id = |line: &str, attachment: &str| {
        line.split_once(attachment)
            .and_then(|(_, suffix)| {
                suffix
                    .split(|character: char| !character.is_ascii_digit())
                    .next()
            })
            .filter(|id| !id.is_empty())
            .and_then(|id| id.parse::<usize>().ok())
            .expect("metadata attachment should have a numeric id")
    };
    let descriptor_scope = ir
        .lines()
        .find(|line| {
            line.contains("buffer_ptr")
                && line.contains(" = load ")
                && line.contains("!alias.scope !")
        })
        .map(|line| metadata_id(line, "!alias.scope !"))
        .expect("buffer descriptor load should have an alias scope");
    let buffer_noalias = ir
        .lines()
        .find(|line| line.contains("buffer_load") && line.contains("!noalias !"))
        .map(|line| metadata_id(line, "!noalias !"))
        .expect("buffer sample load should have a noalias scope");
    let output_scope = ir
        .lines()
        .find(|line| line.contains("store float") && line.contains("!alias.scope !"))
        .map(|line| metadata_id(line, "!alias.scope !"))
        .expect("audio output store should have an alias scope");
    assert_eq!(
        buffer_noalias, descriptor_scope,
        "external-buffer samples must only claim disjointness from descriptor tables"
    );
    assert_ne!(
        buffer_noalias, output_scope,
        "the processor ABI permits audio output and external-buffer sample storage to alias"
    );
}

#[test]
fn constant_and_invariant_buffer_collection_descriptors_are_hoisted() {
    let (_, mir) = source_program(
        r#"
buffers:
  mono: f32 {4}
  fixed: f32[2] {4}
  dynamic: f32[] {4}

init:
  index = 0
  slot = 1

sample:
  out1 = mono[0][index] + fixed[slot][1, index] + dynamic[slot][1, index] + dynamic[slot].samplerate()
  index = index + 1
"#,
        8,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("buffer collection reads should emit LLVM IR");

    for (name, load, expected) in [
        ("read pointer", "buffer_ptr", 3),
        ("frame count", "buffer_frames", 3),
        ("dynamic channel count", "buffer_channels", 1),
        ("sample rate", "buffer_sample_rate", 1),
    ] {
        assert_eq!(
            ir.lines()
                .filter(|line| line.contains(load) && line.contains(" = load "))
                .count(),
            expected,
            "each selected collection {name} should be loaded once per process entry"
        );
    }
    assert!(
        ir.contains("onda.buffer_descriptors"),
        "collection descriptor loads should carry their own host-region alias scope"
    );
}

#[test]
fn forwarded_constant_and_invariant_buffer_collection_descriptors_are_hoisted() {
    let (_, mir) = source_program(
        r#"
buffers:
  bank: f32[] {8}

outs:
  out1

proc Reader:
  params:
    slot: i32 = 2
  buffers:
    clips: f32[] {6}
  outs:
    out1
  init:
    frame: i32 = 0
  sample:
    out1 = clips[0][1, frame] + clips[slot][1, frame]
    frame = frame + 1

init:
  reader = Reader(slot = 2, clips = bank[1:7])

sample:
  out1 = reader()
"#,
        8,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("forwarded buffer collection reads should emit LLVM IR");

    for (name, load) in [
        ("read pointer", "buffer_ptr"),
        ("frame count", "buffer_frames"),
        ("dynamic channel count", "buffer_channels"),
    ] {
        assert_eq!(
                ir.lines()
                    .filter(|line| line.contains(load) && line.contains(" = load "))
                    .count(),
                2,
                "each constant or invariant forwarded collection {name} should be loaded once per process entry"
            );
    }
    assert!(
        ir.contains("onda.buffer_descriptors"),
        "forwarded collection descriptor loads should retain the host-region alias scope"
    );
    assert!(
        ir.contains("!invariant.group"),
        "descriptor loads should express pointer-scoped call invariance"
    );
    assert!(
        ir.contains("llvm.launder.invariant.group.p0"),
        "each entry-point call should establish a fresh descriptor invariant group"
    );
    assert!(
        !ir.contains("!invariant.load"),
        "descriptor bindings may change between entry-point calls"
    );
}

#[test]
fn sample_varying_forwarded_buffer_collection_selection_remains_dynamic() {
    let (_, mir) = source_program(
        r#"
buffers:
  bank: f32[] {8}

outs:
  out1

proc Reader:
  buffers:
    clips: f32[] {6}
  outs:
    out1
  init:
    frame: i32 = 0
    slot: i32 = 0
  sample:
    out1 = clips[slot][1, frame]
    frame = frame + 1
    slot = slot + 1

init:
  reader = Reader(clips = bank[1:7])

sample:
  out1 = reader()
"#,
        8,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("sample-varying forwarded collection reads should emit LLVM IR");

    for (name, load) in [
        ("read pointer", "buffer_ptr"),
        ("frame count", "buffer_frames"),
        ("dynamic channel count", "buffer_channels"),
    ] {
        assert!(
            ir.lines()
                .filter(|line| line.contains(load) && line.contains(" = load "))
                .count()
                > 1,
            "sample-varying forwarded collection {name} must remain inside the sample loop"
        );
    }
}

#[test]
fn direct_buffer_accesses_expose_validated_natural_alignment() {
    let (_, mir) = source_program(
        r#"
buffers:
  data: f64
sample:
  value = data[0]
  data[1] = value
  out1 = f32(value)
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("aligned f64 buffer access should emit LLVM IR");
    assert!(ir
        .lines()
        .any(|line| line.contains("buffer_load") && line.contains("align 8")));
    assert!(ir
        .lines()
        .any(|line| line.contains("store double") && line.contains("align 8")));
}

#[test]
fn raw_checked_buffer_abi_accepts_null_unbound_descriptors() {
    let (_, mir) = source_program(
        r#"
buffers:
  data: f32
sample:
  out1 = 0.0
"#,
        1,
    );
    let mut storage = [0_u32; 1];
    let pointer = storage.as_mut_ptr().cast::<u8>();
    validate_test_buffer_abi(&mir, &[pointer], &[1], &[1], &[48_000.0])
        .expect("positive, non-null buffer binding should be accepted");
    validate_test_buffer_abi(&mir, &[std::ptr::null_mut()], &[1], &[1], &[48_000.0])
        .expect("positive null buffer descriptor should represent an unbound buffer");

    let null = std::ptr::null_mut();
    for (pointer, frames, channels) in [(null, 0, 0), (pointer, 0, 0), (pointer, 1, 0)] {
        let error = validate_test_buffer_abi(&mir, &[pointer], &[frames], &[channels], &[48_000.0])
            .expect_err("raw processor ABI requires every descriptor to be prepared");
        assert!(error.message.contains("requires positive dimensions"));
    }

    let error = validate_test_buffer_abi(&mir, &[null], &[0], &[0], &[f32::NAN])
        .expect_err("invalid sample-rate metadata must be rejected");
    assert!(error.message.contains("finite positive sample rate"));

    let error = validate_test_buffer_abi(&mir, &[pointer], &[1], &[2], &[48_000.0])
        .expect_err("non-empty bindings must honor declared channel constraints");
    assert!(
        error.message.contains("requires 1 channels"),
        "{}",
        error.message
    );

    let mut aligned_storage = [0_u32; 2];
    let misaligned = aligned_storage.as_mut_ptr().cast::<u8>().wrapping_add(1);
    let error = validate_test_buffer_abi(&mir, &[misaligned], &[1], &[1], &[48_000.0])
        .expect_err("non-empty bindings must honor scalar alignment");
    assert!(error.message.contains("requires 4-byte alignment"));
}

#[test]
fn raw_abi_uses_null_pointers_for_absent_surfaces() {
    assert!(abi_const_ptr::<u8>(&[]).is_null());
    assert!(abi_mut_ptr::<u8>(&mut []).is_null());

    let values = [1_u8];
    let mut mutable_values = [1_u8];
    assert_eq!(abi_const_ptr(&values), values.as_ptr());
    assert_eq!(
        abi_mut_ptr(&mut mutable_values),
        mutable_values.as_mut_ptr()
    );
}

#[test]
fn physical_state_region_size_is_rounded_to_its_alignment() {
    let (_, mir) = source_program(
        r#"
init:
  wide: f64 = 1.0
  narrow: f32 = 2.0

sample:
  wide = wide + f64(narrow)
  out1 = f32(wide)
"#,
        1,
    );
    let native = lower_mir_and_jit_with_options(
        mir.clone(),
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("mixed-alignment state should JIT");
    assert_eq!(native.state_byte_offsets(), &[0, 8]);
    assert_eq!(native.state_alignment_bytes(), 8);
    assert_eq!(native.state_size_bytes(), 16);
    assert_eq!(
        native.state_size_bytes() % native.state_alignment_bytes(),
        0
    );
    let mut target = crate::TargetConfig::host();
    target.opt_level = TargetOptLevel::O0;
    let artifact = lower_mir_to_object_artifact(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            target,
        },
    )
    .expect("mixed-alignment AOT artifact should emit");
    assert_eq!(
        artifact.metadata.runtime.state_size_bytes,
        native.state_size_bytes()
    );
    assert_eq!(
        artifact.metadata.runtime.state_align_bytes,
        native.state_alignment_bytes()
    );
}

#[test]
fn overlapping_slice_copy_is_memmove_safe_without_dynamic_alloca() {
    let source = r#"
sample:
  forward = [1.0, 2.0, 3.0, 4.0]
  forward[1:4] = forward[0:3]
  backward = [1.0, 2.0, 3.0, 4.0]
  backward[0:3] = backward[1:4]
  out1 = forward[0] + forward[1] * 10.0 + forward[2] * 100.0 + forward[3] * 1000.0
  out2 = backward[0] + backward[1] * 10.0 + backward[2] * 100.0 + backward[3] * 1000.0
"#;
    let outputs = run_native_outputs(source, 1);
    assert_eq!(outputs[0], [3211.0]);
    assert_eq!(outputs[1], [4432.0]);

    let (_, mir) = source_program(source, 1);
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("slice copy MIR should emit LLVM IR");
    assert!(ir.contains("@llvm.memmove"));
    assert!(ir.contains("slice_copy_unequal_stride_overlap"));
    assert!(!ir.contains("slice_copy_temporary"));
    assert!(
        !ir.lines()
            .any(|line| line.contains("alloca i8") && line.contains(", i")),
        "slice copy must not introduce a runtime-sized stack allocation"
    );
}

#[test]
fn function_inline_hints_control_o3_helper_shape() {
    let (_, mut mir) = source_program(
        r#"
ins:
  in1
params:
  amount = 0.25

def shape(x: f32, amount: f32):
  return (x + amount) * (x - amount)

sample:
  out1 = shape(in1, amount)
"#,
        64,
    );
    let function_index = mir
        .functions
        .iter()
        .position(|function| {
            matches!(function.kind, FunctionKind::User) && function.name.contains("shape")
        })
        .expect("source helper should lower to a MIR user function");
    let symbol = format!("@__onda_mir_fn_{function_index}");

    mir.functions[function_index].attributes.inline = onda_mir::InlineHint::Never;
    let never_ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("noinline helper should emit");
    assert!(never_ir.contains("noinline"));
    assert!(never_ir
        .lines()
        .any(|line| { line.contains("call") && line.contains(&symbol) }));

    mir.functions[function_index].attributes.inline = onda_mir::InlineHint::Always;
    let always_ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("alwaysinline helper should emit");
    assert!(!always_ir
        .lines()
        .any(|line| { line.contains("call") && line.contains(&symbol) }));
    assert!(!always_ir
        .lines()
        .any(|line| { line.starts_with("define internal") && line.contains(&symbol) }));
}

#[test]
fn live_init_keeps_zero_state_initialization_in_llvm() {
    let (_, mir) = source_program(
        r#"
init:
  value = 0.0

sample:
  out1 = value
"#,
        1,
    );
    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("zero-initialized state should emit LLVM IR");
    let init = ir
        .split("define i32 @onda_processor_init")
        .nth(1)
        .and_then(|tail| tail.split("\n}").next())
        .expect("onda_processor_init definition");
    assert!(
        init.contains("state_slot"),
        "live init must restore an explicitly zero-initialized state:\n{init}"
    );
}

#[test]
fn full_init_does_not_preclear_pinned_zero_arrays() {
    let sources = [
        r#"
init:
  pin data: f32[4096]

sample:
  out1 = data[0]
"#,
        r#"
proc Loader:
  init:
    pin data: f32[4096]

  sample:
    out1 = data[0]

init:
  loader = Loader()

sample:
  out1 = loader()
"#,
    ];

    for source in sources {
        let (_, mir) = source_program(source, 1);
        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("pinned zero array should emit LLVM IR");
        let init = ir
            .split("@onda_processor_init")
            .nth(1)
            .and_then(|tail| tail.split("\n}").next())
            .expect("onda_processor_init definition");
        let memset_count = init
            .lines()
            .filter(|line| line.contains("call void @llvm.memset"))
            .count();
        assert_eq!(
            memset_count, 1,
            "the pinned array initializer must be the only full-size clear:\n{init}"
        );
    }
}

#[test]
fn packed_parameters_and_reference_calls_carry_sound_alignment_facts() {
    let (_, mir) = source_program(
        r#"
ins:
  in1
params:
  tag: i32 = 0
  gain: f64 = 0.5

def identity(value: f32):
  return value

sample:
  value = in1
  out1 = f32(gain) + identity(value) + f32(tag)
"#,
        1,
    );
    let mut mir = mir;
    let function_index = mir
        .functions
        .iter()
        .position(|function| {
            matches!(function.kind, FunctionKind::User) && function.name.contains("identity")
        })
        .expect("identity helper should lower");
    mir.functions[function_index].params[0].mode = onda_mir::PassingMode::ReadOnlyReference;
    fn rewrite_reference_call(block: &mut Block, target: onda_mir::FunctionId) -> bool {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::Call { function, args, .. } if *function == target => {
                    let local = match &args[0] {
                        CallArgument::Value(onda_mir::Value::Local(local)) => *local,
                        _ => panic!("identity argument should be a local value"),
                    };
                    args[0] = CallArgument::Place(Place::local(local));
                    return true;
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    if rewrite_reference_call(then_block, target)
                        || rewrite_reference_call(else_block, target)
                    {
                        return true;
                    }
                }
                StatementKind::Loop { body } => {
                    if rewrite_reference_call(body, target) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    let target = onda_mir::FunctionId::new(function_index as u32);
    assert!(
        mir.functions
            .iter_mut()
            .any(|function| rewrite_reference_call(&mut function.body, target)),
        "identity call should be rewritten to a reference argument"
    );
    let native = lower_mir_and_jit_with_options(
        mir.clone(),
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("packed parameter MIR should JIT");
    assert_eq!(native.param_byte_offsets(), &[0, 4]);
    assert_eq!(native.param_byte_size(), 12);

    let ir = lower_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("reference parameter MIR should emit LLVM IR");
    assert!(ir
        .lines()
        .any(|line| { line.contains("load double") && line.contains("align 1") }));
    let reference_definition = ir
        .lines()
        .find(|line| line.starts_with("define internal") && line.contains("@__onda_mir_fn_"))
        .expect("reference-taking user function definition");
    for fact in [
        "captures(none)",
        "nonnull",
        "readonly",
        "align 1",
        "dereferenceable(4)",
    ] {
        assert!(
            reference_definition.contains(fact),
            "missing reference ABI fact '{fact}' in {reference_definition}"
        );
    }
}

#[test]
fn object_artifact_sidecar_matches_native_control_and_event_layouts() {
    let (_, mir) = source_program(
        r#"
kouts { meter: f64 }

init { held = 1.25 }

block { meter = f64(held) }

events {
  fixed(head: f32[2] = [0.25, 0.5], stamp: i64 = i64(7)) {
    held = head[0] + f32(stamp)
  }

  dynamic(head: f32[2], tail: f32[], stamp: i64) {
    held = head[0] + tail[0] + f32(stamp)
  }
}
"#,
        64,
    );
    let native = lower_mir_and_jit_with_options(
        mir.clone(),
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O0,
        },
    )
    .expect("MIR should JIT for layout comparison");
    let mut target = crate::TargetConfig::host();
    target.opt_level = TargetOptLevel::O0;
    let artifact = lower_mir_to_object_artifact(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            target,
        },
    )
    .expect("MIR object and sidecar should emit");

    assert!(!artifact.object_bytes.is_empty());
    assert_eq!(artifact.metadata.format, crate::PROCESSOR_ARTIFACT_FORMAT);
    assert_eq!(artifact.metadata.abi_version, crate::PROCESSOR_ABI_VERSION);
    assert_eq!(artifact.metadata.artifact_kind, "relocatable_object");
    assert_eq!(artifact.metadata.backend, "llvm");
    assert_eq!(
        artifact.metadata.mir_schema_version,
        onda_mir::MIR_SCHEMA_VERSION
    );
    assert_eq!(
        artifact.metadata.format_version,
        crate::AOT_METADATA_FORMAT_VERSION
    );
    assert_eq!(artifact.metadata.compile.sample_rate, 48_000.0);
    assert_eq!(artifact.metadata.compile.block_size, 64);
    assert_eq!(artifact.metadata.exports.init, "onda_processor_init");
    assert_eq!(artifact.metadata.exports.process, "onda_process");
    assert_eq!(artifact.metadata.target.pointer_model, "native_address");
    assert_eq!(artifact.metadata.target.calling_convention, "c");
    assert!(artifact.metadata.target.pointer_width_bits >= 32);
    assert!(!artifact.metadata.target.data_layout.is_empty());
    assert!(matches!(
        artifact.metadata.integration.profile,
        crate::aot_artifact::AotIntegrationProfile::NativeRelocatableObject { .. }
    ));
    assert_eq!(
        artifact.metadata.exports.events,
        ["onda_event_0", "onda_event_1"]
    );
    assert_eq!(
        artifact.metadata.runtime.state_size_bytes,
        native.state_size_bytes()
    );
    assert_eq!(
        artifact.metadata.runtime.state_align_bytes,
        native.state_alignment_bytes()
    );
    assert!(artifact.metadata.runtime.param_align_bytes >= 1);
    assert_eq!(artifact.metadata.runtime.state_initialization, "zeroed");
    assert_eq!(
        artifact.metadata.runtime.snapshot_format_version,
        crate::AOT_SNAPSHOT_FORMAT_VERSION
    );
    assert_eq!(
        artifact.metadata.runtime.snapshot_byte_order,
        "little_endian"
    );
    assert_eq!(
        artifact.metadata.runtime.snapshot_restore_base,
        "post_init_physical_state_image"
    );
    assert!(
        artifact.metadata.runtime.snapshot_size_bytes <= artifact.metadata.runtime.state_size_bytes
    );

    let meter = artifact
        .metadata
        .metadata
        .control_outputs
        .first()
        .expect("meter sidecar descriptor");
    assert_eq!(meter.name, "meter");
    assert_eq!(meter.type_repr, "f64");
    assert_eq!(meter.slot_offset, 0);
    assert_eq!(meter.byte_offset, Some(0));
    assert_eq!(meter.byte_size, 8);
    assert_eq!(
        meter.state_byte_offset,
        native.control_output_storage_byte_offset(0)
    );

    let fixed = &artifact.metadata.metadata.events[0];
    assert_eq!(fixed.name, "fixed");
    assert_eq!(fixed.payload_size_bytes, native.event_payload_byte_size(0));
    assert_eq!(fixed.payload_size_bytes, Some(16));
    assert_eq!(fixed.params[0].type_repr, "f32[2]");
    assert_eq!(fixed.params[0].byte_offset, Some(0));
    assert_eq!(fixed.params[0].byte_size, Some(8));
    assert!(fixed.params[0].has_default);
    assert_eq!(
        fixed.params[0].default_reprs,
        Some(vec!["0.25".to_owned(), "0.5".to_owned()])
    );
    assert_eq!(fixed.params[1].type_repr, "i64");
    assert_eq!(fixed.params[1].byte_offset, Some(8));
    assert_eq!(fixed.params[1].byte_size, Some(8));
    assert!(fixed.params[1].has_default);
    assert_eq!(fixed.params[1].default_reprs, Some(vec!["7".to_owned()]));

    let dynamic = &artifact.metadata.metadata.events[1];
    assert_eq!(dynamic.name, "dynamic");
    assert_eq!(
        dynamic.payload_size_bytes,
        native.event_payload_byte_size(1)
    );
    assert_eq!(dynamic.payload_size_bytes, None);
    assert_eq!(dynamic.params[0].byte_offset, Some(0));
    assert_eq!(dynamic.params[0].byte_size, Some(8));
    assert_eq!(dynamic.params[1].type_repr, "f32[]");
    assert!(dynamic.params[1].is_slice);
    assert_eq!(dynamic.params[1].byte_offset, Some(8));
    assert_eq!(dynamic.params[1].byte_size, None);
    assert_eq!(dynamic.params[2].byte_offset, None);
    assert_eq!(dynamic.params[2].byte_size, Some(8));
}

#[test]
fn wasm_aot_artifact_is_relocatable_and_declares_linker_contract() {
    let (_, mir) = source_program(
        r#"
init { phase = 0.0 }
sample { out1 = phase }
"#,
        64,
    );
    let artifact = lower_mir_to_object_artifact(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            target: crate::TargetConfig::for_triple("wasm32-unknown-unknown"),
        },
    )
    .expect("LLVM should emit a relocatable wasm32 processor object");

    assert!(artifact.object_bytes.starts_with(b"\0asm\x01\0\0\0"));
    assert!(artifact
        .object_bytes
        .windows(b"linking".len())
        .any(|window| window == b"linking"));
    assert_eq!(artifact.metadata.target.pointer_width_bits, 32);
    assert_eq!(artifact.metadata.target.byte_order, "little_endian");
    assert_eq!(
        artifact.metadata.target.pointer_model,
        "linear_memory_offset"
    );
    assert!(matches!(
        artifact.metadata.integration.profile,
        crate::aot_artifact::AotIntegrationProfile::WebassemblyRelocatableObject {
            no_entry: true,
            export_memory: true,
            ..
        }
    ));
    assert_eq!(
        artifact.metadata.integration.required_symbols,
        ["onda_processor_init", "onda_process"]
    );
}

#[test]
fn aot_snapshot_manifest_maps_persistent_segments_only() {
    let i32_ty = onda_mir::TypeId::new(0);
    let f32_ty = onda_mir::TypeId::new(1);
    let f64_ty = onda_mir::TypeId::new(2);
    let mut mir = Program::new(
        onda_mir::CompileConfig {
            sample_rate: 48_000.0,
            block_size: 64,
        },
        onda_mir::FunctionId::new(0),
        onda_mir::FunctionId::new(1),
    );
    mir.types = vec![
        Type::Scalar(onda_mir::ScalarType::I32),
        Type::Scalar(onda_mir::ScalarType::F32),
        Type::Scalar(onda_mir::ScalarType::F64),
    ];
    mir.state = vec![
        onda_mir::StateSlot {
            integer_range: None,
            name: "phase".to_owned(),
            ty: f32_ty,
            persistence: onda_mir::StatePersistence::Snapshot,
            authored: true,
            pinned: false,
        },
        onda_mir::StateSlot {
            integer_range: None,
            name: "meter".to_owned(),
            ty: f64_ty,
            persistence: onda_mir::StatePersistence::ControlMirror,
            authored: true,
            pinned: false,
        },
        onda_mir::StateSlot {
            integer_range: None,
            name: "$scratch".to_owned(),
            ty: i32_ty,
            persistence: onda_mir::StatePersistence::InstanceScratch,
            authored: false,
            pinned: false,
        },
        onda_mir::StateSlot {
            integer_range: None,
            name: "history".to_owned(),
            ty: f64_ty,
            persistence: onda_mir::StatePersistence::Snapshot,
            authored: true,
            pinned: true,
        },
    ];
    mir.interface.control_outputs.push(onda_mir::ControlOutput {
        name: "meter".to_owned(),
        ty: f64_ty,
        mirror: onda_mir::StateId::new(1),
    });
    let empty_function = |name: &str, kind| onda_mir::Function {
        name: name.to_owned(),
        kind,
        attributes: onda_mir::FunctionAttributes::default(),
        params: Vec::new(),
        results: Vec::new(),
        locals: Vec::new(),
        body: onda_mir::Block::default(),
        source: onda_mir::SourceSpan::UNKNOWN,
    };
    let init = empty_function("onda_init", FunctionKind::Init);
    let mut process = empty_function("onda_process", FunctionKind::Process);
    process.params = onda_mir::process_function_params(i32_ty);
    mir.functions = vec![init, process];

    let mut target = crate::TargetConfig::host();
    target.opt_level = TargetOptLevel::O0;
    let artifact = lower_mir_to_object_artifact(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            target,
        },
    )
    .expect("state snapshot manifest should emit");

    assert_eq!(artifact.metadata.runtime.snapshot_size_bytes, 12);
    assert_eq!(
        artifact.metadata.runtime.snapshot_byte_order,
        "little_endian"
    );
    assert_eq!(
        artifact.metadata.runtime.snapshot_restore_base,
        "post_init_physical_state_image"
    );
    let states = &artifact.metadata.metadata.states;
    assert_eq!(
        states
            .iter()
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>(),
        ["phase", "history"]
    );
    assert_eq!(states[0].type_repr, "f32");
    assert_eq!(states[0].element_size_bytes, 4);
    assert_eq!(states[0].packed_snapshot_byte_offset, 0);
    assert_eq!(states[0].physical_state_byte_offset, 0);
    assert_eq!(states[0].byte_size, 4);
    assert_eq!(states[1].type_repr, "f64");
    assert_eq!(states[1].element_size_bytes, 8);
    assert_eq!(states[1].packed_snapshot_byte_offset, 4);
    assert_ne!(
        states[1].packed_snapshot_byte_offset,
        states[1].physical_state_byte_offset
    );
    assert_eq!(states[1].byte_size, 8);
    assert!(states
        .iter()
        .all(|state| state.name != "meter" && state.name != "$scratch"));
}
