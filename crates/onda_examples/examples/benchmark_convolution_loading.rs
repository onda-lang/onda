//! Compares synchronous `set_impulse` with cooperative task loading.
//!
//! Run with:
//! `cargo run --release -p onda_examples --example benchmark_convolution_loading --features llvm-orc -- [frames] [block_size] [repetitions]`

use std::env;
use std::error::Error;
use std::time::Instant;

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, JitProgram, MirCompileOptions, TargetOptLevel,
};
use onda_frontend::{parse_program, PrimitiveType};
use onda_runtime::{
    bind_buffer, bind_output, create_instance, init, process_checked, InitMode, InstanceConfig,
};
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

const SAMPLE_RATE: f32 = 48_000.0;
const MAX_IMPULSE_FRAMES: usize = 480_000;

const SPIKE_SOURCE: &str = r#"
import std/convolution

const FFTSize = 16384
const MaxImpulseFrames = 480000

buffers:
  impulse: f32[2]

init:
  left = std::convolution<FFTSize, MaxImpulseFrames>::ZeroLatencyConvolver()
  right = std::convolution<FFTSize, MaxImpulseFrames>::ZeroLatencyConvolver()
  loaded = false

block:
  if !loaded:
    left.set_impulse(impulse[0, :])
    right.set_impulse(impulse[1, :])
    loaded = true

  sample:
    out1 = left(0.0)
    out2 = right(0.0)
"#;

const TASK_SOURCE: &str = r#"
import std/convolution

const FFTSize = 16384
const MaxImpulseFrames = 480000
const ImpulseLoadSeconds = 0.5

use std::convolution<FFTSize, MaxImpulseFrames> as Convolution

buffers:
  impulse: f32[2]

init:
  left = Convolution::ZeroLatencyConvolver()
  right = Convolution::ZeroLatencyConvolver()

task load_impulse():
  frames = min(impulse.len(), MaxImpulseFrames)
  total_transforms = Convolution::impulse_window_count(frames) * 2
  loading_frames = max(i32(max(ImpulseLoadSeconds, 0.0) * HOST_SR), 1)
  loading_blocks = 1 + (loading_frames - 1) / BS

  left.begin_impulse(frames)
  right.begin_impulse(frames)

  start = 0
  end = 0
  channel = 0
  transform_credit = 0
  blocks_done = 0
  while blocks_done < loading_blocks:
    transform_credit = transform_credit + total_transforms
    while transform_credit >= loading_blocks && start < frames:
      if channel == 0:
        end = Convolution::impulse_window_end(start, frames)
        left.set_impulse_window(start, impulse[0, start:end])
        channel = 1
      else:
        right.set_impulse_window(start, impulse[1, start:end])
        start = end
        channel = 0
      transform_credit = transform_credit - loading_blocks

    blocks_done = blocks_done + 1
    if blocks_done < loading_blocks:
      yield

event reload_impulse():
  load_impulse.reset()

block:
  await load_impulse()

  sample:
    out1 = left(0.0) + 1.0
    out2 = right(0.0) + 1.0
"#;

#[derive(Clone, Copy)]
struct LoadingSummary {
    blocks: usize,
    peak_us: f64,
    total_us: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let impulse_frames = args
        .next()
        .unwrap_or_else(|| "360530".to_owned())
        .parse::<usize>()?;
    let block_size = args
        .next()
        .unwrap_or_else(|| "128".to_owned())
        .parse::<usize>()?;
    let repetitions = args
        .next()
        .unwrap_or_else(|| "7".to_owned())
        .parse::<usize>()?;
    if impulse_frames == 0
        || impulse_frames > MAX_IMPULSE_FRAMES
        || block_size == 0
        || block_size > i32::MAX as usize
        || repetitions == 0
    {
        return Err(
            "impulse frames must be in 1..=480000; block size and repetitions must be positive"
                .into(),
        );
    }

    let spike = compile(SPIKE_SOURCE, block_size)?;
    let task = compile(TASK_SOURCE, block_size)?;
    let spike_summary = measure(spike, impulse_frames, block_size, Some(1), repetitions)?;
    let task_summary = measure(task, impulse_frames, block_size, None, repetitions)?;
    let deadline_us = block_size as f64 * 1_000_000.0 / SAMPLE_RATE as f64;

    println!(
        concat!(
            "impulse_frames={} block_size={} repetitions={}\n",
            "audio_block_deadline_us={:.3}\n",
            "set_impulse: blocks=1 peak_us={:.3} total_us={:.3}\n",
            "task_load: blocks={} peak_us={:.3} total_us={:.3}\n",
            "peak_reduction={:.2}x total_cost_ratio={:.2}x ready_after_ms={:.3}"
        ),
        impulse_frames,
        block_size,
        repetitions,
        deadline_us,
        spike_summary.peak_us,
        spike_summary.total_us,
        task_summary.blocks,
        task_summary.peak_us,
        task_summary.total_us,
        spike_summary.peak_us / task_summary.peak_us,
        task_summary.total_us / spike_summary.total_us,
        task_summary.blocks as f64 * block_size as f64 * 1_000.0 / SAMPLE_RATE as f64,
    );
    Ok(())
}

fn compile(source: &str, block_size: usize) -> Result<JitProgram, Box<dyn Error>> {
    let parsed = parse_program(source).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: SAMPLE_RATE,
            block_size,
        },
    )
    .map_err(|errors| format!("semantic analysis failed: {errors:?}"))?;
    let mir = lower_program_to_optimized_mir(&typed)
        .map_err(|errors| format!("MIR lowering failed: {errors:?}"))?;
    jit_program_from_optimized_mir_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .map_err(|errors| format!("JIT lowering failed: {errors:?}").into())
}

fn measure(
    program: JitProgram,
    impulse_frames: usize,
    block_size: usize,
    fixed_loading_blocks: Option<usize>,
    repetitions: usize,
) -> Result<LoadingSummary, Box<dyn Error>> {
    let mut peaks = Vec::with_capacity(repetitions);
    let mut totals = Vec::with_capacity(repetitions);
    let mut measured_blocks = None;
    let mut impulse = vec![0.0_f32; impulse_frames * 2];
    let mut left_output = vec![0.0_f32; block_size];
    let mut right_output = vec![0.0_f32; block_size];

    for _ in 0..repetitions {
        left_output.fill(0.0);
        right_output.fill(0.0);
        let mut instance = create_instance(
            program.clone(),
            InstanceConfig {
                sample_rate: SAMPLE_RATE,
                frames_per_block: block_size,
                in_channels: 0,
                out_channels: 2,
            },
        )
        .map_err(|error| format!("instance creation failed: {error:?}"))?;
        unsafe {
            bind_buffer(
                &mut instance,
                0,
                impulse.as_mut_ptr().cast(),
                impulse_frames,
                2,
                SAMPLE_RATE,
                PrimitiveType::F32,
            )
            .map_err(|error| format!("buffer binding failed: {error:?}"))?;
            bind_output(
                &mut instance,
                0,
                left_output.as_mut_ptr().cast(),
                std::mem::size_of_val(left_output.as_slice()),
            )
            .map_err(|error| format!("left output binding failed: {error:?}"))?;
            bind_output(
                &mut instance,
                1,
                right_output.as_mut_ptr().cast(),
                std::mem::size_of_val(right_output.as_slice()),
            )
            .map_err(|error| format!("right output binding failed: {error:?}"))?;
        }
        init(&mut instance, InitMode::Full)
            .map_err(|error| format!("instance initialization failed: {error:?}"))?;

        let mut peak_us = 0.0_f64;
        let started = Instant::now();
        let maximum_blocks = fixed_loading_blocks.unwrap_or(impulse_frames * 2 + 3);
        let mut blocks = 0;
        while blocks < maximum_blocks {
            let block_started = Instant::now();
            process_checked(&mut instance, block_size, None)
                .map_err(|error| format!("processing failed: {error:?}"))?;
            peak_us = peak_us.max(block_started.elapsed().as_secs_f64() * 1_000_000.0);
            blocks += 1;

            let complete = fixed_loading_blocks
                .map(|expected| blocks == expected)
                .unwrap_or(left_output[0] != 0.0);
            if complete {
                break;
            }
        }
        if blocks == maximum_blocks && fixed_loading_blocks.is_none() && left_output[0] == 0.0 {
            return Err("task did not finish within its conservative block bound".into());
        }
        if measured_blocks
            .replace(blocks)
            .is_some_and(|value| value != blocks)
        {
            return Err("task completion block changed between repetitions".into());
        }
        peaks.push(peak_us);
        totals.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }

    peaks.sort_by(f64::total_cmp);
    totals.sort_by(f64::total_cmp);
    Ok(LoadingSummary {
        blocks: measured_blocks.unwrap_or_default(),
        peak_us: peaks[repetitions / 2],
        total_us: totals[repetitions / 2],
    })
}
