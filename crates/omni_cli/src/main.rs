use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use omni_codegen_llvm::{
    lower_and_jit_with_options, lower_to_llvm_ir_with_options, CompileOptions, ExecutionBackend,
};
use omni_frontend::{parse_program_file, Diagnostic, PrimitiveType};
use omni_runtime::{bind_output, create_instance, process_bound, InstanceConfig};
use omni_semantics::{analyze_with_options, AnalysisOptions, TypedArrayInfo, TypedProgram};

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_DUR_SECONDS: u32 = 5;
const DEFAULT_BLOCK_FRAMES: usize = 512;

const USAGE: &str = r#"Usage:
  omni compile <input.omni> [--ir] [--meta] [--fast-math]
  omni render <input.omni> [--output <path>] [--dur <seconds>] [--sample-rate <hz>] [--block <frames>] [--ir] [--fast-math]

Options:
  --output, -o   Output wav path (default: ./omni_out.wav)
  --dur, -d      Render duration in seconds (default: 5)
  --sample-rate, --sr  Render/output sample rate in Hz (default: 48000)
  --block, -b    Block size in frames (default: 512)
  --ir           Print optimized LLVM IR before compile/render
  --meta         Print declared ins/outs/params metadata
  --fast-math    Enable LLVM fast-math flags for floating-point operations
  --help, -h     Show this help
"#;

enum Command {
    Compile {
        input: PathBuf,
        dump_ir: bool,
        show_meta: bool,
        fast_math: bool,
    },
    Render {
        input: PathBuf,
        output: PathBuf,
        dur_seconds: u32,
        sample_rate_hz: u32,
        block_frames: usize,
        dump_ir: bool,
        fast_math: bool,
    },
}

fn main() {
    let cmd = match parse_args(env::args()) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            process::exit(2);
        }
    };

    let result = match cmd {
        Command::Compile {
            input,
            dump_ir,
            show_meta,
            fast_math,
        } => run_compile(&input, dump_ir, show_meta, fast_math),
        Command::Render {
            input,
            output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            dump_ir,
            fast_math,
        } => run_render(
            &input,
            &output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            dump_ir,
            fast_math,
        ),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.skip(1);
    let Some(cmd) = args.next() else {
        return Err(USAGE.to_owned());
    };
    if cmd == "--help" || cmd == "-h" || cmd == "help" {
        return Err(USAGE.to_owned());
    }

    match cmd.as_str() {
        "compile" => parse_compile_args(args),
        "render" => parse_render_args(args),
        _ => Err(format!("unknown command '{cmd}'\n\n{USAGE}")),
    }
}

fn parse_compile_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(input) = args.next() else {
        return Err(format!("compile requires an input file\n\n{USAGE}"));
    };
    let mut dump_ir = false;
    let mut show_meta = false;
    let mut fast_math = false;
    for arg in args {
        match arg.as_str() {
            "--ir" => dump_ir = true,
            "--meta" => show_meta = true,
            "--fast-math" => fast_math = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }
    Ok(Command::Compile {
        input: PathBuf::from(input),
        dump_ir,
        show_meta,
        fast_math,
    })
}

fn parse_render_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(input) = args.next() else {
        return Err(format!("render requires an input file\n\n{USAGE}"));
    };

    let mut output = PathBuf::from("./omni_out.wav");
    let mut dur_seconds = DEFAULT_DUR_SECONDS;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    let mut dump_ir = false;
    let mut fast_math = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" | "-o" => {
                let Some(value) = args.next() else {
                    return Err("--output requires a file path".to_owned());
                };
                output = PathBuf::from(value);
            }
            "--dur" | "-d" => {
                let Some(value) = args.next() else {
                    return Err("--dur requires a positive integer value".to_owned());
                };
                dur_seconds = parse_dur_seconds(&value)?;
            }
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--ir" => {
                dump_ir = true;
            }
            "--fast-math" => {
                fast_math = true;
            }
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ if arg.starts_with("--output=") => {
                let value = &arg["--output=".len()..];
                if value.is_empty() {
                    return Err("--output requires a file path".to_owned());
                }
                output = PathBuf::from(value);
            }
            _ if arg.starts_with("--dur=") => {
                let value = &arg["--dur=".len()..];
                dur_seconds = parse_dur_seconds(value)?;
            }
            _ if arg.starts_with("--sample-rate=") => {
                let value = &arg["--sample-rate=".len()..];
                sample_rate_hz = parse_sample_rate_hz(value)?;
            }
            _ if arg.starts_with("--sr=") => {
                let value = &arg["--sr=".len()..];
                sample_rate_hz = parse_sample_rate_hz(value)?;
            }
            _ if arg.starts_with("--block=") => {
                let value = &arg["--block=".len()..];
                block_frames = parse_block_frames(value)?;
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }

    Ok(Command::Render {
        input: PathBuf::from(input),
        output,
        dur_seconds,
        sample_rate_hz,
        block_frames,
        dump_ir,
        fast_math,
    })
}

fn parse_dur_seconds(value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("invalid duration '{value}', expected positive integer seconds"))?;
    if parsed == 0 {
        return Err("duration must be greater than zero".to_owned());
    }
    Ok(parsed)
}

fn parse_sample_rate_hz(value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("invalid sample rate '{value}', expected positive integer Hz"))?;
    if parsed == 0 {
        return Err("sample rate must be greater than zero".to_owned());
    }
    Ok(parsed)
}

fn parse_block_frames(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid block size '{value}', expected positive integer frames"))?;
    if parsed == 0 {
        return Err("block size must be greater than zero".to_owned());
    }
    Ok(parsed)
}

fn run_compile(
    input: &Path,
    dump_ir: bool,
    show_meta: bool,
    fast_math: bool,
) -> Result<(), String> {
    let typed = parse_and_analyze(input, DEFAULT_SAMPLE_RATE as f32, DEFAULT_BLOCK_FRAMES)?;
    if dump_ir {
        let ir = lower_to_llvm_ir_with_options(
            typed.clone(),
            CompileOptions {
                backend: ExecutionBackend::OrcJit,
                sample_rate: DEFAULT_SAMPLE_RATE as f32,
                block_size: DEFAULT_BLOCK_FRAMES,
                fast_math,
            },
        )
        .map_err(|diags| format_diagnostics("IR lowering failed", &diags))?;
        println!("{ir}");
    }
    if show_meta {
        print_program_meta(&typed);
    }
    println!("OK: {}", input.display());
    Ok(())
}

#[derive(Clone)]
struct CliDeclaredIo {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    offset: usize,
}

fn primitive_type_name(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn declared_type_repr(elem: PrimitiveType, len: usize) -> String {
    if len == 1 {
        primitive_type_name(elem).to_owned()
    } else {
        format!("{}[{len}]", primitive_type_name(elem))
    }
}

fn build_declared_ports(
    flat: &[String],
    types: &std::collections::HashMap<String, PrimitiveType>,
    arrays: &std::collections::HashMap<String, TypedArrayInfo>,
) -> Vec<CliDeclaredIo> {
    let arrays_by_offset = arrays
        .iter()
        .map(|(name, info)| (info.offset, (name, info)))
        .collect::<std::collections::HashMap<_, _>>();
    let mut out = Vec::new();
    let mut slot = 0usize;
    while slot < flat.len() {
        if let Some((name, info)) = arrays_by_offset.get(&slot) {
            out.push(CliDeclaredIo {
                name: (*name).clone(),
                elem_ty: info.elem_ty,
                array_len: info.len,
                offset: slot,
            });
            slot += info.len;
            continue;
        }
        let name = flat[slot].clone();
        let ty = *types.get(&name).unwrap_or(&PrimitiveType::F32);
        out.push(CliDeclaredIo {
            name,
            elem_ty: ty,
            array_len: 1,
            offset: slot,
        });
        slot += 1;
    }
    out
}

fn build_declared_params(typed: &TypedProgram) -> Vec<CliDeclaredIo> {
    let arrays_by_offset = typed
        .param_arrays
        .iter()
        .map(|(name, info)| (info.offset, (name, info)))
        .collect::<std::collections::HashMap<_, _>>();
    let mut out = Vec::new();
    let mut slot = 0usize;
    while slot < typed.params.len() {
        if let Some((name, info)) = arrays_by_offset.get(&slot) {
            out.push(CliDeclaredIo {
                name: (*name).clone(),
                elem_ty: info.elem_ty,
                array_len: info.len,
                offset: slot,
            });
            slot += info.len;
            continue;
        }
        let p = &typed.params[slot];
        out.push(CliDeclaredIo {
            name: p.name.clone(),
            elem_ty: p.ty,
            array_len: 1,
            offset: slot,
        });
        slot += 1;
    }
    out
}

fn print_declared_table(label: &str, entries: &[CliDeclaredIo]) {
    println!("{label}:");
    if entries.is_empty() {
        println!("  (none)");
        return;
    }
    for (idx, entry) in entries.iter().enumerate() {
        let ty = declared_type_repr(entry.elem_ty, entry.array_len);
        let bytes = primitive_type_bytes(entry.elem_ty) * entry.array_len;
        println!(
            "  [{idx}] name={} type={} bytes={} offset={}",
            entry.name, ty, bytes, entry.offset
        );
    }
}

fn print_program_meta(typed: &TypedProgram) {
    let ins = build_declared_ports(&typed.ins, &typed.in_types, &typed.in_arrays);
    let outs = build_declared_ports(&typed.outs, &typed.out_types, &typed.out_arrays);
    let params = build_declared_params(typed);
    print_declared_table("ins", &ins);
    print_declared_table("outs", &outs);
    print_declared_table("params", &params);
}

fn run_render(
    input: &Path,
    output: &Path,
    dur_seconds: u32,
    sample_rate_hz: u32,
    block_frames: usize,
    dump_ir: bool,
    fast_math: bool,
) -> Result<(), String> {
    let typed = parse_and_analyze(input, sample_rate_hz as f32, block_frames)?;
    let declared_outs = build_declared_ports(&typed.outs, &typed.out_types, &typed.out_arrays);
    if dump_ir {
        let ir = lower_to_llvm_ir_with_options(
            typed.clone(),
            CompileOptions {
                backend: ExecutionBackend::OrcJit,
                sample_rate: sample_rate_hz as f32,
                block_size: block_frames,
                fast_math,
            },
        )
        .map_err(|diags| format_diagnostics("IR lowering failed", &diags))?;
        println!("{ir}");
    }

    let in_channels = typed.ins.len();
    let out_channels = typed.outs.len();
    if out_channels == 0 {
        return Err("render requires at least one output channel".to_owned());
    }

    let jit = lower_and_jit_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
            fast_math,
        },
    )
    .map_err(|diags| format_diagnostics("ORC JIT lowering failed", &diags))?;

    let mut instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: sample_rate_hz as f32,
            frames_per_block: block_frames,
            in_channels,
            out_channels,
        },
    )
    .map_err(|diag| format_single_diagnostic("instance creation failed", &diag))?;

    let total_frames = sample_rate_hz as usize * dur_seconds as usize;
    let full_blocks = total_frames / block_frames;
    let tail_frames = total_frames % block_frames;

    let mut bound_out_buffers = Vec::with_capacity(declared_outs.len());
    for out_idx in 0..declared_outs.len() {
        let entry = &declared_outs[out_idx];
        let bytes = primitive_type_bytes(entry.elem_ty)
            .saturating_mul(entry.array_len)
            .saturating_mul(block_frames);
        let mut buf = vec![0_u8; bytes];
        bind_output(&mut instance, out_idx, buf.as_mut_ptr(), buf.len())
            .map_err(|diag| format_single_diagnostic("bind_output failed", &diag))?;
        bound_out_buffers.push(buf);
    }

    let mut rendered = Vec::with_capacity(total_frames * out_channels);
    for _ in 0..full_blocks {
        process_bound(&mut instance, block_frames)
            .map_err(|diag| format_single_diagnostic("render failed", &diag))?;
        let out_block = decode_bound_outputs_to_interleaved_f32(
            &declared_outs,
            &bound_out_buffers,
            block_frames,
            out_channels,
        )?;
        rendered.extend(out_block);
    }
    if tail_frames > 0 {
        process_bound(&mut instance, block_frames)
            .map_err(|diag| format_single_diagnostic("render failed", &diag))?;
        let out_block = decode_bound_outputs_to_interleaved_f32(
            &declared_outs,
            &bound_out_buffers,
            block_frames,
            out_channels,
        )?;
        rendered.extend_from_slice(&out_block[..tail_frames * out_channels]);
    }

    write_wav_interleaved_i16(output, out_channels, sample_rate_hz, &rendered)?;
    println!(
        "Rendered {}s @ {} Hz (block {}) to {}",
        dur_seconds,
        sample_rate_hz,
        block_frames,
        output.display()
    );
    Ok(())
}

fn decode_bound_outputs_to_interleaved_f32(
    declared_outs: &[CliDeclaredIo],
    bound_out_buffers: &[Vec<u8>],
    frames: usize,
    out_channels: usize,
) -> Result<Vec<f32>, String> {
    if declared_outs.len() != bound_out_buffers.len() {
        return Err("output binding metadata/buffer count mismatch".to_owned());
    }
    let mut out_interleaved = vec![0.0_f32; frames.saturating_mul(out_channels)];
    for out_idx in 0..declared_outs.len() {
        let entry = &declared_outs[out_idx];
        let buf = &bound_out_buffers[out_idx];
        let elem_bytes = primitive_type_bytes(entry.elem_ty);
        let expected = elem_bytes
            .saturating_mul(entry.array_len)
            .saturating_mul(frames);
        if buf.len() != expected {
            return Err(format!(
                "output '{}' buffer size {} does not match expected {}",
                entry.name,
                buf.len(),
                expected
            ));
        }
        for ch in 0..entry.array_len {
            let dst_channel = entry.offset.saturating_add(ch);
            if dst_channel >= out_channels {
                continue;
            }
            for frame in 0..frames {
                let src_idx = (ch * frames + frame) * elem_bytes;
                let sample =
                    decode_value_to_f32(entry.elem_ty, &buf[src_idx..src_idx + elem_bytes])?;
                out_interleaved[frame * out_channels + dst_channel] = sample;
            }
        }
    }
    Ok(out_interleaved)
}

fn decode_value_to_f32(ty: PrimitiveType, bytes: &[u8]) -> Result<f32, String> {
    match ty {
        PrimitiveType::F32 => {
            let arr: [u8; 4] = bytes
                .try_into()
                .map_err(|_| "invalid f32 width in output buffer".to_owned())?;
            Ok(f32::from_ne_bytes(arr))
        }
        PrimitiveType::F64 => {
            let arr: [u8; 8] = bytes
                .try_into()
                .map_err(|_| "invalid f64 width in output buffer".to_owned())?;
            Ok(f64::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::I32 => {
            let arr: [u8; 4] = bytes
                .try_into()
                .map_err(|_| "invalid i32 width in output buffer".to_owned())?;
            Ok(i32::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::I64 => {
            let arr: [u8; 8] = bytes
                .try_into()
                .map_err(|_| "invalid i64 width in output buffer".to_owned())?;
            Ok(i64::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::Bool => {
            let b = *bytes
                .first()
                .ok_or_else(|| "invalid bool width in output buffer".to_owned())?;
            Ok(if b == 0 { 0.0 } else { 1.0 })
        }
    }
}

fn parse_and_analyze(
    input: &Path,
    sample_rate: f32,
    block_size: usize,
) -> Result<TypedProgram, String> {
    let parsed =
        parse_program_file(input).map_err(|diags| format_diagnostics("parse failed", &diags))?;
    analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate,
            block_size,
        },
    )
    .map_err(|diags| format_diagnostics("semantic analysis failed", &diags))
}

fn format_diagnostics(context: &str, diags: &[Diagnostic]) -> String {
    let mut text = String::from(context);
    for diag in diags {
        text.push_str(&format!("\n- {}", format_single_diag_line(diag)));
        if !diag.trace.is_empty() {
            text.push_str("\n  trace:");
            for trace in diag.trace.iter().rev() {
                text.push_str(&format!("\n    - {trace}"));
            }
        }
        if let Some(snippet) = format_diag_snippet(diag) {
            text.push_str(&format!("\n{snippet}"));
        }
    }
    text
}

fn format_single_diagnostic(context: &str, diag: &Diagnostic) -> String {
    let mut out = format!("{context}\n- {}", format_single_diag_line(diag));
    if !diag.trace.is_empty() {
        out.push_str("\n  trace:");
        for trace in diag.trace.iter().rev() {
            out.push_str(&format!("\n    - {trace}"));
        }
    }
    if let Some(snippet) = format_diag_snippet(diag) {
        out.push_str(&format!("\n{snippet}"));
    }
    out
}

fn format_single_diag_line(diag: &Diagnostic) -> String {
    let location = match diag.file.as_deref() {
        Some(file) if diag.line > 0 => format!("{file}:{}:{}", diag.line, diag.column.max(1)),
        Some(file) => format!("{file}:0:0"),
        None if diag.line > 0 => format!("{}:{}", diag.line, diag.column.max(1)),
        None => "0:0".to_owned(),
    };
    format!("{location} [{:?}] {}", diag.code, diag.message)
}

fn format_diag_snippet(diag: &Diagnostic) -> Option<String> {
    if diag.message.contains('\n') {
        return None;
    }
    let file = diag.file.as_deref()?;
    if file.starts_with('<') || diag.line == 0 {
        return None;
    }
    let path = Path::new(file);
    let source = fs::read_to_string(path).ok()?;
    let line_idx = diag.line.checked_sub(1)?;
    let line_text = source.lines().nth(line_idx)?;
    let col = diag.column.max(1);
    let caret_pad = " ".repeat(col.saturating_sub(1));
    Some(format!(
        "  --> {file}:{}:{}\n   |\n{:>4} | {}\n   | {}^",
        diag.line, col, diag.line, line_text, caret_pad
    ))
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
    if samples.len() % channels != 0 {
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
