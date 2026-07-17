use std::env;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use onda_codegen_llvm::{lower_and_jit_with_options, CompileOptions, TargetOptLevel};
use onda_frontend::{parse_program_file, PrimitiveType};
use onda_runtime::{
    bind_output, create_instance, prepare_unchecked_process, process_checked, process_unchecked,
    Instance, InstanceConfig,
};
use onda_semantics::{analyze_with_options, AnalysisOptions};

const SAMPLE_RATE: f32 = 48_000.0;
const WARMUP_BLOCKS: usize = 200;
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
    let compile_options = CompileOptions {
        sample_rate: SAMPLE_RATE,
        block_size,
        fast_math: false,
        opt_level: TargetOptLevel::O3,
    };
    let (jit, compile_summary) = if validate_only {
        (
            lower_and_jit_with_options(typed, compile_options)
                .map_err(|errors| format!("LLVM JIT lowering failed: {errors:?}"))?,
            None,
        )
    } else {
        let mut samples = Vec::with_capacity(compile_repetitions);
        let mut compiled = None;
        for repetition in 0..=compile_repetitions {
            let typed_sample = typed.clone();
            let started = Instant::now();
            let next = lower_and_jit_with_options(typed_sample, compile_options)
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

    let input_channels = jit.required_in_channels();
    let output_channels = jit.output_count();
    if input_channels != 0 {
        return Err("native benchmark scenarios must not require audio inputs".into());
    }
    if output_channels == 0 {
        return Err("native benchmark scenarios must expose an audio output".into());
    }
    if jit.buffer_count() != 0 {
        return Err("native benchmark scenarios must not require external buffers".into());
    }
    if jit
        .outputs()
        .iter()
        .any(|output| output.elem_ty() != PrimitiveType::F32 || output.array_len() != 1)
    {
        return Err("native benchmark scenarios must expose scalar f32 audio outputs".into());
    }

    let mut instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: SAMPLE_RATE,
            frames_per_block: block_size,
            in_channels: input_channels,
            out_channels: output_channels,
        },
    )
    .map_err(|error| format!("instance creation failed: {error:?}"))?;

    let mut outputs = (0..output_channels)
        .map(|_| vec![0.0_f32; block_size])
        .collect::<Vec<_>>();
    for (index, output) in outputs.iter_mut().enumerate() {
        unsafe {
            bind_output(
                &mut instance,
                index,
                output.as_mut_ptr().cast::<u8>(),
                output.len() * std::mem::size_of::<f32>(),
            )
        }
        .map_err(|error| format!("bind output {index} failed: {error:?}"))?;
    }

    process_checked(&mut instance, block_size)
        .map_err(|error| format!("first process failed: {error:?}"))?;
    let parity = validate_first_block(&outputs, Path::new(&expected_outputs), block_size)?;
    prepare_unchecked_process(&mut instance)
        .map_err(|error| format!("unchecked preparation failed: {error:?}"))?;
    run_unchecked_blocks(&mut instance, WARMUP_BLOCKS, "warmup")?;
    if validate_only {
        let recommended_iterations =
            calibrate_iterations(&mut instance, iterations, minimum_round_ms)?;
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
        run_unchecked_blocks(&mut instance, iterations, "timed")?;
        let elapsed = started.elapsed().as_nanos() as f64;
        samples.push(elapsed / (iterations * block_size) as f64);
        black_box(output_checksum(&outputs));
    }
    let process_summary = summarize_samples(samples)?;
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
        parity.samples,
        parity.maximum_absolute_error,
        output_channels,
    );
    Ok(())
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
    instance: &mut Instance,
    minimum_iterations: usize,
    minimum_round_ms: f64,
) -> Result<usize, Box<dyn Error>> {
    let mut iterations = minimum_iterations;
    loop {
        let started = Instant::now();
        run_unchecked_blocks(instance, iterations, "calibration")?;
        if started.elapsed().as_secs_f64() * 1_000.0 >= minimum_round_ms {
            return Ok(iterations);
        }
        iterations = iterations
            .checked_mul(2)
            .ok_or("benchmark calibration iteration count overflow")?;
    }
}

fn run_unchecked_blocks(
    instance: &mut Instance,
    blocks: usize,
    phase: &str,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..blocks {
        // The output bindings were validated and remain stable for the benchmark lifetime.
        unsafe { process_unchecked(instance) }
            .map_err(|error| format!("{phase} process failed: {error:?}"))?;
    }
    Ok(())
}

fn median_of_sorted(samples: &[f64]) -> f64 {
    let midpoint = samples.len() / 2;
    if samples.len() % 2 == 0 {
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
