use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use onda_codegen_llvm::TargetOptLevel;
use onda_daemon::{AnalysisSession, RunOptions, RunParamValue, RunSession};
use serde::Serialize;

use crate::diag_print::{format_run_build_error, format_single_diagnostic};

pub(crate) struct BenchOptions {
    pub input: PathBuf,
    pub const_overrides: Vec<(String, String)>,
    pub runs: usize,
    pub blocks: usize,
    pub warmup_blocks: usize,
    pub sample_rate_hz: u32,
    pub block_samples: usize,
    pub opt_level: TargetOptLevel,
    pub fast_math: bool,
    pub show_meta: bool,
    pub json: bool,
    pub param_sets: Vec<(String, RunParamValue)>,
    pub buffer_bindings: Vec<(String, Option<PathBuf>)>,
}

impl BenchOptions {
    pub fn new(input: PathBuf) -> Self {
        Self {
            input,
            const_overrides: Vec::new(),
            runs: 10,
            blocks: 1000,
            warmup_blocks: 100,
            sample_rate_hz: crate::DEFAULT_SAMPLE_RATE,
            block_samples: crate::DEFAULT_BLOCK_SAMPLES,
            opt_level: TargetOptLevel::O3,
            fast_math: false,
            show_meta: false,
            json: false,
            param_sets: Vec::new(),
            buffer_bindings: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.runs == 0 || self.blocks == 0 || self.block_samples == 0 || self.sample_rate_hz == 0
        {
            return Err("runs, blocks, block size, and sample rate must be positive".to_owned());
        }
        self.runs
            .checked_mul(self.blocks)
            .and_then(|blocks| blocks.checked_add(self.warmup_blocks))
            .and_then(|blocks| blocks.checked_mul(self.block_samples))
            .ok_or_else(|| "benchmark frame count exceeds the supported range".to_owned())?;
        Ok(())
    }
}

#[derive(Serialize)]
struct BenchReport<'a> {
    input: &'a std::path::Path,
    measurement: &'static str,
    sample_rate_hz: u32,
    block_size: usize,
    runs: usize,
    blocks_per_run: usize,
    warmup_blocks: usize,
    total_blocks: usize,
    total_frames: usize,
    opt_level: &'static str,
    fast_math: bool,
    elapsed_seconds: f64,
    run_seconds: Vec<f64>,
    min_us_per_block: f64,
    median_us_per_block: f64,
    mean_us_per_block: f64,
    max_us_per_block: f64,
    ns_per_frame: f64,
    frames_per_second: f64,
    realtime_cpu_percent: f64,
}

pub(crate) fn run_bench(options: BenchOptions) -> Result<(), String> {
    options.validate()?;
    onda_realtime::configure_current_thread_audio_fp_mode();
    let run_options = RunOptions {
        sample_rate: options.sample_rate_hz as f32,
        block_size: options.block_samples,
        // Static benchmark overrides must take effect before the first measured block.
        param_smoothing_seconds: 0.0,
        opt_level: options.opt_level,
        fast_math: options.fast_math,
    };
    let project = crate::project_cmd::resolve_run_project(
        &options.input,
        &options.buffer_bindings,
        &options.const_overrides,
        run_options.analysis_options(),
        false,
    )
    .map_err(|error| error.message)?;
    let buffers =
        crate::project_cmd::load_initial_buffers(project.buffers, &options.buffer_bindings)?;
    let mut run = RunSession::prepare_with_inputs_and_initial_buffers(
        &AnalysisSession::new(),
        &project.entry,
        run_options,
        &project.compile_inputs,
        buffers,
    )
    .map_err(|error| format_run_build_error("benchmark build failed", &error))?;
    for (name, value) in &options.param_sets {
        run.set_param(name, *value)
            .map_err(|error| format_single_diagnostic("benchmark parameter failed", &error))?;
    }
    run.restart()
        .map_err(|error| format_single_diagnostic("benchmark initialization failed", &error))?;
    if options.show_meta {
        eprintln!("{}", onda_run::format_run_param_info(&run.param_info()));
    }
    let process = run
        .prepare_processing()
        .map_err(|error| format_single_diagnostic("benchmark preparation failed", &error))?;
    let timings = measure_blocks(options.warmup_blocks, options.runs, options.blocks, process)
        .map_err(|error| format_single_diagnostic("benchmark processing failed", &error))?;
    black_box(&run);
    let report = summarize(&options, &timings);
    if options.json {
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| error.to_string())?
        );
    } else {
        println!("Benchmark: {}", options.input.display());
        println!(
            "{} runs × {} blocks × {} frames at {} Hz ({} warmup blocks)",
            report.runs,
            report.blocks_per_run,
            report.block_size,
            report.sample_rate_hz,
            report.warmup_blocks,
        );
        println!("Native processor; silent inputs; continuous processor state");
        println!(
            "Time/block: median {:.3} µs, mean {:.3} µs, min {:.3} µs, max {:.3} µs (run averages)",
            report.median_us_per_block,
            report.mean_us_per_block,
            report.min_us_per_block,
            report.max_us_per_block,
        );
        println!(
            "{:.3} ns/frame · {:.0} frames/s · {:.2}% realtime DSP CPU (median)",
            report.ns_per_frame, report.frames_per_second, report.realtime_cpu_percent,
        );
        println!(
            "Measured {} blocks / {} frames in {:.6} s",
            report.total_blocks, report.total_frames, report.elapsed_seconds,
        );
    }
    Ok(())
}

fn measure_blocks<E>(
    warmup_blocks: usize,
    runs: usize,
    blocks: usize,
    mut process: impl FnMut() -> Result<(), E>,
) -> Result<Vec<Duration>, E> {
    for _ in 0..warmup_blocks {
        process()?;
    }
    let mut timings = Vec::new();
    for _ in 0..runs {
        let start = Instant::now();
        for _ in 0..blocks {
            process()?;
        }
        let elapsed = start.elapsed();
        timings.push(elapsed);
    }
    Ok(timings)
}

fn summarize<'a>(options: &'a BenchOptions, timings: &[Duration]) -> BenchReport<'a> {
    let run_seconds = timings
        .iter()
        .map(Duration::as_secs_f64)
        .collect::<Vec<_>>();
    let mut sorted = run_seconds.clone();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    };
    let elapsed_seconds = run_seconds.iter().sum::<f64>();
    let total_blocks = options.runs * options.blocks;
    let seconds_per_frame = median / options.blocks as f64 / options.block_samples as f64;
    BenchReport {
        input: &options.input,
        measurement: "native_processor",
        sample_rate_hz: options.sample_rate_hz,
        block_size: options.block_samples,
        runs: options.runs,
        blocks_per_run: options.blocks,
        warmup_blocks: options.warmup_blocks,
        total_blocks,
        total_frames: total_blocks * options.block_samples,
        opt_level: options.opt_level.as_str(),
        fast_math: options.fast_math,
        elapsed_seconds,
        min_us_per_block: sorted[0] * 1e6 / options.blocks as f64,
        median_us_per_block: median * 1e6 / options.blocks as f64,
        mean_us_per_block: elapsed_seconds * 1e6 / total_blocks as f64,
        max_us_per_block: sorted[sorted.len() - 1] * 1e6 / options.blocks as f64,
        ns_per_frame: seconds_per_frame * 1e9,
        frames_per_second: 1.0 / seconds_per_frame,
        realtime_cpu_percent: seconds_per_frame * options.sample_rate_hz as f64 * 100.0,
        run_seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_exact_block_counts_and_stops_on_failure() {
        let mut calls = 0;
        let timings = measure_blocks(2, 3, 4, || {
            calls += 1;
            Ok::<_, ()>(())
        })
        .unwrap();
        assert_eq!(calls, 14);
        assert_eq!(timings.len(), 3);
        for fail_at in [1, 3, 7] {
            calls = 0;
            let result = measure_blocks(2, 3, 4, || {
                calls += 1;
                if calls == fail_at {
                    Err("failed")
                } else {
                    Ok(())
                }
            });
            assert_eq!(result, Err("failed"));
            assert_eq!(calls, fail_at);
        }
    }

    #[test]
    fn statistics_use_run_averages_and_frame_counts() {
        let mut options = BenchOptions::new("test.onda".into());
        options.runs = 4;
        options.blocks = 10;
        options.block_samples = 100;
        options.sample_rate_hz = 1000;
        let report = summarize(
            &options,
            &[
                Duration::from_millis(4),
                Duration::from_millis(1),
                Duration::from_millis(3),
                Duration::from_millis(2),
            ],
        );
        assert_eq!(report.total_blocks, 40);
        assert_eq!(report.total_frames, 4000);
        assert!((report.elapsed_seconds - 0.01).abs() < 1e-10);
        assert!((report.min_us_per_block - 100.0).abs() < 1e-10);
        assert!((report.median_us_per_block - 250.0).abs() < 1e-10);
        assert!((report.mean_us_per_block - 250.0).abs() < 1e-10);
        assert!((report.max_us_per_block - 400.0).abs() < 1e-10);
        assert!((report.ns_per_frame - 2500.0).abs() < 1e-10);
        assert!((report.frames_per_second - 400_000.0).abs() < 1e-6);
        assert!((report.realtime_cpu_percent - 0.25).abs() < 1e-10);
    }
}
