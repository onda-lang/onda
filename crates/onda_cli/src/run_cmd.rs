use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use onda_codegen_llvm::TargetOptLevel;
use onda_daemon::{DaemonConfig, DaemonSession, RunOptions};
use onda_run::{
    append_interleaved_block, format_run_param_info, play_run_realtime, PlaybackLaunch,
    RunHostOptions,
};
use onda_semantics::AnalysisOptions;

use super::diag_print::{format_diagnostics, format_run_build_error, format_single_diagnostic};
use super::{daemon_stdio, DaemonCommand, RunCommand, RunHostKind};

pub(crate) fn run_daemon(cmd: DaemonCommand) -> Result<(), String> {
    match cmd {
        DaemonCommand::Stdio => daemon_stdio::run_stdio_loop(),
        DaemonCommand::Diagnose {
            input,
            sample_rate_hz,
            block_frames,
        } => run_daemon_diagnose(&input, sample_rate_hz, block_frames),
    }
}

pub(crate) fn run_run(cmd: RunCommand) -> Result<(), String> {
    match cmd {
        RunCommand::Play {
            input,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            opt_level,
            input_device,
            output_device,
            fast_math,
            show_meta,
            control_json,
            param_sets,
            buffer_bindings,
        } => play_run_realtime(PlaybackLaunch {
            input,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            opt_level,
            input_device,
            output_device,
            fast_math,
            show_meta,
            control_json,
            param_sets,
            buffer_bindings,
        }),
        RunCommand::Render {
            input,
            output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            opt_level,
            fast_math,
            show_meta,
            param_sets,
            buffer_bindings,
        } => run_daemon_run(DaemonRenderRequest {
            input: &input,
            output: &output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            opt_level,
            fast_math,
            show_meta,
            param_sets: &param_sets,
            buffer_bindings: &buffer_bindings,
        }),
        RunCommand::Window {
            input,
            sample_rate_hz,
            block_frames,
            opt_level,
            input_device,
            output_device,
            fast_math,
            show_meta,
            theme,
            host,
        } => {
            let onda_bin = env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "onda".to_owned());
            run_run_host(
                host,
                input.as_deref(),
                RunHostOptions {
                    sample_rate_hz,
                    block_frames,
                    opt_level: opt_level.as_str().to_owned(),
                    input_device,
                    output_device,
                    fast_math,
                    show_meta,
                    theme,
                    onda_bin,
                },
            )
        }
    }
}

fn run_run_host(
    host: RunHostKind,
    input: Option<&Path>,
    options: RunHostOptions,
) -> Result<(), String> {
    match resolve_run_host_kind(host) {
        RunHostKind::Egui => onda_egui::run_run_egui(input, options),
        RunHostKind::Webview => run_webview_run(input, options),
        RunHostKind::Auto => unreachable!("run host should be resolved before launch"),
    }
}

fn resolve_run_host_kind(host: RunHostKind) -> RunHostKind {
    match host {
        RunHostKind::Auto => default_run_host_kind(),
        other => other,
    }
}

fn default_run_host_kind() -> RunHostKind {
    RunHostKind::Egui
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn run_webview_run(input: Option<&Path>, options: RunHostOptions) -> Result<(), String> {
    onda_webview::run_run_window(input, options)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn run_webview_run(_input: Option<&Path>, _options: RunHostOptions) -> Result<(), String> {
    Err("webview run host is unavailable on this platform/build".to_owned())
}

fn run_daemon_diagnose(
    input: &Path,
    sample_rate_hz: u32,
    block_frames: usize,
) -> Result<(), String> {
    let session = DaemonSession::new(DaemonConfig {
        analysis: AnalysisOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
        },
        run: RunOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
            ..RunOptions::default()
        },
    });
    let snapshot = session.analyze_document(input);
    if snapshot.diagnostics.is_empty() {
        println!("ok");
        return Ok(());
    }
    Err(format_diagnostics(
        "daemon diagnostics",
        &snapshot.diagnostics,
    ))
}

struct DaemonRenderRequest<'a> {
    input: &'a Path,
    output: &'a Path,
    dur_seconds: u32,
    sample_rate_hz: u32,
    block_frames: usize,
    opt_level: TargetOptLevel,
    fast_math: bool,
    show_meta: bool,
    param_sets: &'a [(String, f64)],
    buffer_bindings: &'a [(String, PathBuf)],
}

fn run_daemon_run(request: DaemonRenderRequest<'_>) -> Result<(), String> {
    let DaemonRenderRequest {
        input,
        output,
        dur_seconds,
        sample_rate_hz,
        block_frames,
        opt_level,
        fast_math,
        show_meta,
        param_sets,
        buffer_bindings,
    } = request;
    let mut session = DaemonSession::new(DaemonConfig {
        analysis: AnalysisOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
        },
        run: RunOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
            fast_math,
            opt_level,
            ..RunOptions::default()
        },
    });

    session
        .start_run(input)
        .map_err(|err| format_run_build_error("daemon run start failed", &err))?;

    if show_meta {
        let info = session
            .run(input)
            .expect("run should be active after successful start")
            .param_info();
        if !info.is_empty() {
            println!("{}", format_run_param_info(&info));
        }
    }

    for (name, value) in param_sets {
        session
            .run_mut(input)
            .expect("run should be active while applying params")
            .set_param_f64(name, *value)
            .map_err(|diag| format_single_diagnostic("daemon run param failed", &diag))?;
    }

    for (name, path) in buffer_bindings {
        session
            .run_mut(input)
            .expect("run should be active while binding buffers")
            .bind_buffer_wav_path(name, path)
            .map_err(|diag| format_single_diagnostic("daemon run buffer failed", &diag))?;
    }

    let total_frames = sample_rate_hz as usize * dur_seconds as usize;
    let full_blocks = total_frames / block_frames;
    let tail_frames = total_frames % block_frames;
    let mut rendered = Vec::<f32>::new();

    for _ in 0..full_blocks {
        let block = session
            .render_run_block(input)
            .map_err(|diag| format_single_diagnostic("daemon run render failed", &diag))?;
        append_interleaved_block(&mut rendered, &block);
    }
    if tail_frames > 0 {
        let block = session
            .render_run_block(input)
            .map_err(|diag| format_single_diagnostic("daemon run render failed", &diag))?;
        let mut interleaved = Vec::<f32>::new();
        append_interleaved_block(&mut interleaved, &block);
        let channels = block.len().max(1);
        rendered.extend_from_slice(&interleaved[..tail_frames * channels]);
    }

    let out_channels = session
        .run(input)
        .expect("run should remain active through render")
        .output_channel_count();
    if out_channels == 0 {
        return Err("daemon run requires at least one output channel".to_owned());
    }

    write_wav_interleaved_i16(output, out_channels, sample_rate_hz, &rendered)?;
    println!(
        "Wrote {} seconds of daemon-run audio to {}",
        dur_seconds,
        output.display()
    );
    Ok(())
}

fn write_wav_interleaved_i16(
    path: &Path,
    channels: usize,
    sample_rate_hz: u32,
    samples: &[f32],
) -> Result<(), String> {
    if channels == 0 {
        return Err("cannot write wav with zero channels".to_owned());
    }
    if !samples.len().is_multiple_of(channels) {
        return Err(format!(
            "sample buffer length {} is not divisible by channel count {}",
            samples.len(),
            channels
        ));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create output directory '{}': {err}",
                    parent.display()
                )
            })?;
        }
    }

    let channel_u16 = u16::try_from(channels)
        .map_err(|_| format!("channel count {channels} exceeds wav limit"))?;

    let spec = hound::WavSpec {
        channels: channel_u16,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|err| format!("failed to create wav '{}': {err}", path.display()))?;
    for sample in samples {
        writer
            .write_sample(f32_to_i16(*sample))
            .map_err(|err| format!("failed to write wav sample: {err}"))?;
    }
    writer
        .finalize()
        .map_err(|err| format!("failed to finalize wav '{}': {err}", path.display()))?;
    Ok(())
}

fn f32_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32).round() as i16
}
