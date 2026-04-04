use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use onda_codegen_llvm::{lower_and_jit_with_options, CompileOptions, ExecutionBackend};
use onda_frontend::{parse_program, Diagnostic};
use onda_runtime::{bind_buffer, bind_output, create_instance, process_bound, InstanceConfig};
use onda_semantics::{analyze_with_options, AnalysisOptions};

const DEFAULT_INPUT: &str = "target/garden.wav";
const DEFAULT_OUTPUT: &str = "target/garden_half_speed.wav";
const BLOCK_FRAMES: usize = 512;
const PLAYBACK_RATE: f32 = 0.5;

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_INPUT));
    let output_path = env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));

    let (mut in_interleaved, in_channels, sample_rate_hz) = read_wav_interleaved_f32(&input_path)?;
    if in_channels == 0 {
        return Err("input wav has zero channels".into());
    }
    let input_frames = in_interleaved.len() / in_channels;
    if input_frames == 0 {
        return Err("input wav has zero frames".into());
    }

    let source = build_halfspeed_program(in_channels);
    let parsed = parse_program(&source).map_err(|d| format_diagnostics("parse failed", &d))?;
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: BLOCK_FRAMES,
        },
    )
    .map_err(|d| format_diagnostics("semantic analysis failed", &d))?;
    let out_channels = typed.outs.len();
    if out_channels != in_channels {
        return Err(format!(
            "internal program mismatch: expected {in_channels} outputs, got {out_channels}"
        )
        .into());
    }

    let jit = lower_and_jit_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: sample_rate_hz as f32,
            block_size: BLOCK_FRAMES,
            fast_math: false,
        },
    )
    .map_err(|d| format_diagnostics("ORC JIT lowering failed", &d))?;

    let mut instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: sample_rate_hz as f32,
            frames_per_block: BLOCK_FRAMES,
            in_channels: 0,
            out_channels,
        },
    )
    .map_err(|d| format!("instance creation failed: {d:?}"))?;

    bind_buffer(
        &mut instance,
        0,
        in_interleaved.as_mut_ptr().cast::<u8>(),
        input_frames,
        in_channels,
        sample_rate_hz as f32,
        onda_frontend::PrimitiveType::F32,
    )
    .map_err(|d| format!("bind buffer failed: {d:?}"))?;

    let mut bound_out = (0..out_channels)
        .map(|_| vec![0_u8; BLOCK_FRAMES * std::mem::size_of::<f32>()])
        .collect::<Vec<_>>();
    for (idx, out_buf) in bound_out.iter_mut().enumerate() {
        bind_output(&mut instance, idx, out_buf.as_mut_ptr(), out_buf.len())
            .map_err(|d| format!("bind output[{idx}] failed: {d:?}"))?;
    }

    let render_frames = ((input_frames as f32) / PLAYBACK_RATE).ceil() as usize;
    let full_blocks = render_frames / BLOCK_FRAMES;
    let tail_frames = render_frames % BLOCK_FRAMES;
    let mut rendered = Vec::<f32>::with_capacity(render_frames * out_channels);

    for _ in 0..full_blocks {
        process_bound(&mut instance, BLOCK_FRAMES)
            .map_err(|d| format!("processing failed: {d:?}"))?;
        append_interleaved_block_from_bound_outputs(
            &bound_out,
            BLOCK_FRAMES,
            out_channels,
            &mut rendered,
        )?;
    }
    if tail_frames > 0 {
        process_bound(&mut instance, BLOCK_FRAMES)
            .map_err(|d| format!("processing failed: {d:?}"))?;
        append_interleaved_block_from_bound_outputs(
            &bound_out,
            tail_frames,
            out_channels,
            &mut rendered,
        )?;
    }

    write_wav_interleaved_i16(&output_path, out_channels, sample_rate_hz, &rendered)?;
    println!(
        "Wrote half-speed playback: '{}' -> '{}'",
        input_path.display(),
        output_path.display()
    );
    Ok(())
}

fn build_halfspeed_program(channels: usize) -> String {
    let buffer_decl = if channels == 1 {
        "src: f32".to_owned()
    } else {
        format!("src: f32[{channels}]")
    };

    let mut reads = String::new();
    if channels == 1 {
        reads.push_str("  out1 = src[read_pos]\n");
    } else {
        for ch in 0..channels {
            reads.push_str(&format!("  out{} = src[{ch}][read_pos]\n", ch + 1));
        }
    }

    format!(
        r#"
buffers {{
  {buffer_decl}
}}
outs {channels}
init {{
  read_pos = 0.0
}}
sample {{
{reads}  read_pos = read_pos + {PLAYBACK_RATE}
}}
"#
    )
}

fn append_interleaved_block_from_bound_outputs(
    bound_out: &[Vec<u8>],
    frames_to_take: usize,
    out_channels: usize,
    dst: &mut Vec<f32>,
) -> Result<(), Box<dyn Error>> {
    if bound_out.len() != out_channels {
        return Err("output buffer count mismatch".into());
    }
    for frame in 0..frames_to_take {
        for chan_buf in bound_out {
            let byte_idx = frame * std::mem::size_of::<f32>();
            let arr: [u8; 4] = chan_buf[byte_idx..byte_idx + 4].try_into()?;
            dst.push(f32::from_ne_bytes(arr));
        }
    }
    Ok(())
}

fn read_wav_interleaved_f32(path: &Path) -> Result<(Vec<f32>, usize, u32), Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err("wav has zero channels".into());
    }
    let sample_rate = spec.sample_rate;

    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => {
            reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?
        }
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|s| s.map(|v| v as f32 / i8::MAX as f32))
            .collect::<Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()?,
        (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / i32::MAX as f32))
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(format!(
                "unsupported wav format: {:?} {} bits",
                spec.sample_format, spec.bits_per_sample
            )
            .into())
        }
    };

    Ok((samples, channels, sample_rate))
}

fn write_wav_interleaved_i16(
    path: &Path,
    channels: usize,
    sample_rate_hz: u32,
    samples: &[f32],
) -> Result<(), Box<dyn Error>> {
    if channels == 0 {
        return Err("cannot write wav with zero channels".into());
    }
    if samples.len() % channels != 0 {
        return Err(format!(
            "sample buffer length {} is not divisible by channel count {}",
            samples.len(),
            channels
        )
        .into());
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let spec = hound::WavSpec {
        channels: u16::try_from(channels)?,
        sample_rate: sample_rate_hz,
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
