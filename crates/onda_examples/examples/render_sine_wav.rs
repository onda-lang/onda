use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, MirCompileOptions, TargetOptLevel,
};
use onda_frontend::{parse_program, Diagnostic};
use onda_runtime::{bind_output, create_instance_initialized, process_checked, InstanceConfig};
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

const SAMPLE_RATE: u32 = 48_000;
const DURATION_SECONDS: u32 = 3;
const BLOCK_FRAMES: usize = 480;

const SINE: &str = r#"
outs {
  out1
}
params {
  freq = 440.0
}
init {
  phase = 0.0
}
sample {
  phase = phase + freq * TWO_PI / SR
  out1 = sin(phase)
}
"#;

fn main() -> Result<(), Box<dyn Error>> {
    let output_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/sine.wav"));

    let parsed = parse_program(SINE).map_err(|d| format_diagnostics("parse failed", &d))?;
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: SAMPLE_RATE as f32,
            block_size: BLOCK_FRAMES,
        },
    )
    .map_err(|d| format_diagnostics("semantic analysis failed", &d))?;
    let in_channels = typed.ins.len();
    let out_channels = typed.outs.len();
    if out_channels != 1 {
        return Err(format!(
            "expected exactly 1 output channel for this example, got {out_channels}"
        )
        .into());
    }

    let mir = lower_program_to_optimized_mir(&typed)
        .map_err(|errors| format!("MIR lowering failed: {errors:?}"))?;
    let jit = jit_program_from_optimized_mir_with_options(
        mir,
        MirCompileOptions {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .map_err(|d| format_diagnostics("ORC JIT lowering failed", &d))?;

    let mut instance = create_instance_initialized(
        jit,
        InstanceConfig {
            sample_rate: SAMPLE_RATE as f32,
            frames_per_block: BLOCK_FRAMES,
            in_channels,
            out_channels,
        },
    )
    .map_err(|d| format!("instance creation failed: {d:?}"))?;

    let total_frames = SAMPLE_RATE as usize * DURATION_SECONDS as usize;
    if !total_frames.is_multiple_of(BLOCK_FRAMES) {
        return Err(format!(
            "total_frames ({total_frames}) must be a multiple of BLOCK_FRAMES ({BLOCK_FRAMES})"
        )
        .into());
    }

    let blocks = total_frames / BLOCK_FRAMES;
    let mut rendered = Vec::with_capacity(total_frames * out_channels);
    let mut out_bound = vec![0_u8; BLOCK_FRAMES * std::mem::size_of::<f32>()];
    unsafe { bind_output(&mut instance, 0, out_bound.as_mut_ptr(), out_bound.len()) }
        .map_err(|d| format!("bind output failed: {d:?}"))?;

    for _ in 0..blocks {
        process_checked(
            &mut instance,
            BLOCK_FRAMES,
            onda_runtime::ExecutionOutput::none(),
        )
        .map_err(|d| format!("processing failed: {d:?}"))?;
        let out_f32 =
            unsafe { std::slice::from_raw_parts(out_bound.as_ptr().cast::<f32>(), BLOCK_FRAMES) };
        rendered.extend_from_slice(out_f32);
    }

    write_wav_mono_i16(&output_path, &rendered)?;
    println!(
        "Wrote {} seconds of ORC-rendered sine to {}",
        DURATION_SECONDS,
        output_path.display()
    );
    Ok(())
}

fn write_wav_mono_i16(path: &Path, samples: &[f32]) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for sample in samples {
        writer.write_sample(f32_to_i16(*sample))?;
    }
    writer.finalize()?;
    Ok(())
}

fn f32_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32).round() as i16
}

fn format_diagnostics(context: &str, diags: &[Diagnostic]) -> String {
    let mut text = String::from(context);
    for diag in diags {
        text.push_str(&format!(
            "\n- {}:{} [{:?}] {}",
            diag.line, diag.column, diag.code, diag.message
        ));
    }
    text
}
