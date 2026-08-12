use std::env;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use onda_codegen_llvm::{
    lower_optimized_mir_and_jit_with_options, MirCompileOptions, MirJitProgram, RuntimeState,
    TargetOptLevel, PROCESSOR_EXECUTION_OK,
};
use onda_frontend::parse_program_file;
use onda_mir::{BufferChannels, ScalarType};
use onda_realtime::configure_current_thread_audio_fp_mode;
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

const SAMPLE_RATE: f32 = 48_000.0;
const WARMUP_BLOCKS: usize = 200;
const BLOCK_LATENCY_SAMPLES: usize = 4_096;
const PARITY_ABSOLUTE_TOLERANCE: f64 = 1e-6;
const PARITY_RELATIVE_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Copy)]
struct SampleSummary {
    median: f64,
    median_absolute_deviation: f64,
    minimum: f64,
    maximum: f64,
}

#[derive(Clone, Copy)]
struct ParitySummary {
    samples: usize,
    maximum_absolute_error: f64,
}

struct NativeBenchmark {
    jit: MirJitProgram,
    params: Vec<u8>,
    state: RuntimeState,
    input_ptrs: Vec<*const u8>,
    _inputs: Vec<Vec<f32>>,
    output_ptrs: Vec<*mut u8>,
    _buffer_storage: Vec<Vec<u64>>,
    buffer_ptrs: Vec<*mut u8>,
    buffer_frames: Vec<i32>,
    buffer_channels: Vec<i32>,
    buffer_sample_rates: Vec<f32>,
    outputs: Vec<Vec<f32>>,
    block_size_u32: u32,
}

impl NativeBenchmark {
    fn new(jit: MirJitProgram, block_size: usize) -> Result<Self, Box<dyn Error>> {
        let params = jit.default_param_bytes();
        let state = jit
            .initialize_state(&params)
            .map_err(|error| format!("state initialization failed: {error:?}"))?;
        let inputs = (0..jit.required_in_channels())
            .map(|_| vec![0.0_f32; block_size])
            .collect::<Vec<_>>();
        let input_ptrs = inputs
            .iter()
            .map(|input| input.as_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let mut outputs = (0..jit.required_out_channels())
            .map(|_| vec![0.0_f32; block_size])
            .collect::<Vec<_>>();
        let output_ptrs = outputs
            .iter_mut()
            .map(|output| output.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let buffer_frames_i32 = i32::try_from(block_size)?;
        let mut buffer_storage = Vec::with_capacity(jit.buffer_count());
        let mut buffer_channels = Vec::with_capacity(jit.buffer_count());
        for buffer in &jit.mir().interface.buffers {
            let channels = benchmark_buffer_channels(buffer.channels);
            let bytes = block_size
                .checked_mul(channels)
                .and_then(|elements| elements.checked_mul(scalar_size(buffer.element)))
                .ok_or("benchmark buffer byte size overflow")?;
            buffer_storage.push(vec![0_u64; bytes.div_ceil(std::mem::size_of::<u64>())]);
            buffer_channels.push(i32::try_from(channels)?);
        }
        let buffer_ptrs = buffer_storage
            .iter_mut()
            .map(|storage| storage.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let buffer_count = buffer_ptrs.len();
        Ok(Self {
            jit,
            params,
            state,
            input_ptrs,
            _inputs: inputs,
            output_ptrs,
            _buffer_storage: buffer_storage,
            buffer_ptrs,
            buffer_frames: vec![buffer_frames_i32; buffer_count],
            buffer_channels,
            buffer_sample_rates: vec![SAMPLE_RATE; buffer_count],
            outputs,
            block_size_u32: u32::try_from(block_size)?,
        })
    }

    fn process_checked(&mut self) -> Result<(), Box<dyn Error>> {
        unsafe {
            self.jit.process_checked(
                &mut self.state,
                &self.params,
                0,
                self.block_size_u32 as usize,
                3,
                &self.input_ptrs,
                &self.output_ptrs,
                &self.buffer_ptrs,
                &self.buffer_frames,
                &self.buffer_channels,
                &self.buffer_sample_rates,
            )
        }
        .map_err(|error| format!("checked process failed: {error:?}").into())
    }

    unsafe fn process_unchecked(&mut self) {
        let status = unsafe {
            self.jit.process_unchecked(
                &mut self.state,
                &self.params,
                0,
                self.block_size_u32,
                3,
                &self.input_ptrs,
                &self.output_ptrs,
                &self.buffer_ptrs,
                &self.buffer_frames,
                &self.buffer_channels,
                &self.buffer_sample_rates,
            )
        };
        assert_eq!(status, PROCESSOR_EXECUTION_OK);
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let source = args.next().ok_or("missing source path")?;
    let block_size = args.next().ok_or("missing block size")?.parse::<usize>()?;
    let iterations = args
        .next()
        .ok_or("missing iteration count")?
        .parse::<usize>()?;
    let repetitions = args
        .next()
        .unwrap_or_else(|| "5".to_owned())
        .parse::<usize>()?;
    let compile_repetitions = args
        .next()
        .unwrap_or_else(|| "5".to_owned())
        .parse::<usize>()?;
    let expected_outputs = args.next().ok_or("missing expected-output fixture path")?;
    let minimum_round_ms = args
        .next()
        .unwrap_or_else(|| "50".to_owned())
        .parse::<f64>()?;
    let validate_only = match args.next().as_deref() {
        None => false,
        Some("--validate-only") => true,
        Some(other) => return Err(format!("unknown benchmark mode '{other}'").into()),
    };
    if args.next().is_some() {
        return Err("too many benchmark arguments".into());
    }
    if block_size == 0
        || iterations == 0
        || repetitions == 0
        || compile_repetitions == 0
        || !minimum_round_ms.is_finite()
        || minimum_round_ms <= 0.0
    {
        return Err(
            "block size, iterations, repetitions, compile repetitions, and minimum round time must be positive"
                .into(),
        );
    }

    let parsed = parse_program_file(Path::new(&source))
        .map_err(|errors| format!("parse failed: {errors:?}"))?;
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: SAMPLE_RATE,
            block_size,
        },
    )
    .map_err(|errors| format!("semantic analysis failed: {errors:?}"))?;
    let mir = lower_program_to_optimized_mir(&typed)
        .map_err(|errors| format!("optimized MIR lowering failed: {errors:?}"))?;
    let compile_options = MirCompileOptions {
        fast_math: false,
        opt_level: TargetOptLevel::O3,
    };
    let (jit, compile_summary) = if validate_only {
        (
            lower_optimized_mir_and_jit_with_options(mir, compile_options)
                .map_err(|errors| format!("LLVM JIT lowering failed: {errors:?}"))?,
            None,
        )
    } else {
        let mut samples = Vec::with_capacity(compile_repetitions);
        let mut compiled = None;
        for repetition in 0..=compile_repetitions {
            let started = Instant::now();
            let next = lower_optimized_mir_and_jit_with_options(mir.clone(), compile_options)
                .map_err(|errors| format!("LLVM JIT lowering failed: {errors:?}"))?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            if repetition > 0 {
                samples.push(elapsed_ms);
            }
            compiled = Some(next);
        }
        (
            compiled.expect("positive compile repetition count produces a JIT"),
            Some(summarize_samples(samples)?),
        )
    };

    let output_channels = jit.required_out_channels();
    if output_channels == 0 {
        return Err("native benchmark scenarios must expose an audio output".into());
    }
    if jit.mir().interface.inputs.iter().any(|input| {
        !matches!(
            jit.mir().types.get(input.ty.index()),
            Some(onda_mir::Type::Scalar(onda_mir::ScalarType::F32))
        )
    }) {
        return Err("native benchmark scenarios must expose scalar f32 audio inputs".into());
    }
    if jit.mir().interface.outputs.iter().any(|output| {
        !matches!(
            jit.mir().types.get(output.ty.index()),
            Some(onda_mir::Type::Scalar(onda_mir::ScalarType::F32))
        )
    }) {
        return Err("native benchmark scenarios must expose scalar f32 audio outputs".into());
    }

    configure_current_thread_audio_fp_mode();
    let mut benchmark = NativeBenchmark::new(jit, block_size)?;
    benchmark.process_checked()?;
    let parity =
        validate_first_block(&benchmark.outputs, Path::new(&expected_outputs), block_size)?;
    let cold_block_latencies = measure_block_latencies(&mut benchmark, 15);
    let cold_block_max_us = cold_block_latencies.iter().copied().fold(0.0_f64, f64::max);
    run_unchecked_blocks(&mut benchmark, WARMUP_BLOCKS);
    if validate_only {
        let recommended_iterations =
            calibrate_iterations(&mut benchmark, iterations, minimum_round_ms)?;
        println!(
            concat!(
                "{{",
                "\"parity_samples\":{},",
                "\"parity_max_abs_error\":{:.9},",
                "\"recommended_iterations\":{},",
                "\"outputs\":{}",
                "}}"
            ),
            parity.samples, parity.maximum_absolute_error, recommended_iterations, output_channels,
        );
        return Ok(());
    }

    let mut samples = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let started = Instant::now();
        run_unchecked_blocks(&mut benchmark, iterations);
        let elapsed = started.elapsed().as_nanos() as f64;
        samples.push(elapsed / (iterations * block_size) as f64);
        black_box(output_checksum(&benchmark.outputs));
    }
    let process_summary = summarize_samples(samples)?;
    let block_latencies = measure_block_latencies(&mut benchmark, BLOCK_LATENCY_SAMPLES);
    let block_summary = summarize_samples(block_latencies.clone())?;
    let block_p99_us = percentile(&block_latencies, 0.99);
    let compile_summary = compile_summary.expect("benchmark mode records compile samples");

    println!(
        concat!(
            "{{",
            "\"compile_ms\":{:.6},",
            "\"compile_mad_ms\":{:.6},",
            "\"compile_min_ms\":{:.6},",
            "\"compile_max_ms\":{:.6},",
            "\"process_ns_per_frame\":{:.6},",
            "\"process_mad_ns_per_frame\":{:.6},",
            "\"process_min_ns_per_frame\":{:.6},",
            "\"process_max_ns_per_frame\":{:.6},",
            "\"block_median_us\":{:.6},",
            "\"block_p99_us\":{:.6},",
            "\"block_max_us\":{:.6},",
            "\"cold_block_max_us\":{:.6},",
            "\"parity_samples\":{},",
            "\"parity_max_abs_error\":{:.9},",
            "\"outputs\":{}",
            "}}"
        ),
        compile_summary.median,
        compile_summary.median_absolute_deviation,
        compile_summary.minimum,
        compile_summary.maximum,
        process_summary.median,
        process_summary.median_absolute_deviation,
        process_summary.minimum,
        process_summary.maximum,
        block_summary.median,
        block_p99_us,
        block_summary.maximum,
        cold_block_max_us,
        parity.samples,
        parity.maximum_absolute_error,
        output_channels,
    );
    Ok(())
}

fn benchmark_buffer_channels(channels: BufferChannels) -> usize {
    match channels {
        BufferChannels::Mono => 1,
        BufferChannels::Static(channels) => channels as usize,
        BufferChannels::Dynamic => 2,
    }
}

fn scalar_size(scalar: ScalarType) -> usize {
    match scalar {
        ScalarType::Bool => 1,
        ScalarType::F32 | ScalarType::I32 => 4,
        ScalarType::F64 | ScalarType::I64 => 8,
    }
}

fn validate_first_block(
    outputs: &[Vec<f32>],
    expected_path: &Path,
    block_size: usize,
) -> Result<ParitySummary, Box<dyn Error>> {
    let expected_bytes = fs::read(expected_path)?;
    let expected_len = outputs
        .len()
        .checked_mul(block_size)
        .and_then(|samples| samples.checked_mul(std::mem::size_of::<f32>()))
        .ok_or("expected-output fixture size overflow")?;
    if expected_bytes.len() != expected_len {
        return Err(format!(
            "expected-output fixture has {} bytes, expected {expected_len}",
            expected_bytes.len()
        )
        .into());
    }

    let mut samples = 0;
    let mut maximum_absolute_error = 0.0_f64;
    for (channel, output) in outputs.iter().enumerate() {
        for (frame, actual) in output.iter().copied().enumerate() {
            let index = channel * block_size + frame;
            let start = index * std::mem::size_of::<f32>();
            let expected = f32::from_le_bytes(
                expected_bytes[start..start + std::mem::size_of::<f32>()]
                    .try_into()
                    .expect("validated f32 fixture chunk"),
            );
            if !actual.is_finite() || !expected.is_finite() {
                return Err(format!(
                    "non-finite first-block sample at channel {channel}, frame {frame}: LLVM {actual}, Wasm {expected}"
                )
                .into());
            }
            let actual = f64::from(actual);
            let expected = f64::from(expected);
            let absolute_error = (actual - expected).abs();
            let allowed_error = PARITY_ABSOLUTE_TOLERANCE
                + PARITY_RELATIVE_TOLERANCE * actual.abs().max(expected.abs());
            if absolute_error > allowed_error {
                return Err(format!(
                    "first-block mismatch at channel {channel}, frame {frame}: LLVM {actual}, Wasm {expected}, abs error {absolute_error}, allowed {allowed_error}"
                )
                .into());
            }
            samples += 1;
            maximum_absolute_error = maximum_absolute_error.max(absolute_error);
        }
    }
    Ok(ParitySummary {
        samples,
        maximum_absolute_error,
    })
}

fn summarize_samples(mut samples: Vec<f64>) -> Result<SampleSummary, Box<dyn Error>> {
    if samples.is_empty() {
        return Err("cannot summarize an empty benchmark sample set".into());
    }
    samples.sort_by(f64::total_cmp);
    let median = median_of_sorted(&samples);
    let mut deviations = samples
        .iter()
        .map(|sample| (sample - median).abs())
        .collect::<Vec<_>>();
    deviations.sort_by(f64::total_cmp);
    Ok(SampleSummary {
        median,
        median_absolute_deviation: median_of_sorted(&deviations),
        minimum: samples[0],
        maximum: samples[samples.len() - 1],
    })
}

fn calibrate_iterations(
    benchmark: &mut NativeBenchmark,
    minimum_iterations: usize,
    minimum_round_ms: f64,
) -> Result<usize, Box<dyn Error>> {
    let mut iterations = minimum_iterations;
    loop {
        let started = Instant::now();
        run_unchecked_blocks(benchmark, iterations);
        if started.elapsed().as_secs_f64() * 1_000.0 >= minimum_round_ms {
            return Ok(iterations);
        }
        iterations = iterations
            .checked_mul(2)
            .ok_or("benchmark calibration iteration count overflow")?;
    }
}

fn run_unchecked_blocks(benchmark: &mut NativeBenchmark, blocks: usize) {
    for _ in 0..blocks {
        // Construction and the checked preflight establish the raw processor ABI invariants.
        unsafe { benchmark.process_unchecked() };
    }
}

fn measure_block_latencies(benchmark: &mut NativeBenchmark, blocks: usize) -> Vec<f64> {
    let mut samples = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        let started = Instant::now();
        unsafe { benchmark.process_unchecked() };
        samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    samples
}

fn percentile(samples: &[f64], quantile: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}

fn median_of_sorted(samples: &[f64]) -> f64 {
    let midpoint = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        (samples[midpoint - 1] + samples[midpoint]) * 0.5
    } else {
        samples[midpoint]
    }
}

fn output_checksum(outputs: &[Vec<f32>]) -> f64 {
    outputs
        .iter()
        .flatten()
        .map(|sample| f64::from(*sample))
        .sum()
}
