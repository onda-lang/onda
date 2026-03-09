use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use omni_codegen_llvm::{
    lower_and_jit_with_options, lower_to_llvm_ir_with_options, CompileOptions, ExecutionBackend,
};
use omni_frontend::{
    parse_program_file, ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec,
    BufferChannels, BufferElemType, BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp, DeclType,
    Diagnostic, EventParamType, Expr, FieldType, FunctionDef, InitBlock, LogicalOp, PrimitiveType,
    ProcessorDef, Program, SampleBlock, Stmt, StructDef,
};
use omni_runtime::{bind_output, create_instance, process_bound, InstanceConfig};
use omni_semantics::{
    analyze_with_options, lower_graphs_for_inspection_with_options, AnalysisOptions,
    TypedArrayInfo, TypedProgram,
};

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_DUR_SECONDS: u32 = 5;
const DEFAULT_BLOCK_FRAMES: usize = 512;

const USAGE: &str = r#"Usage:
  omni compile <input.omni> [--dump-graph] [--ir] [--meta] [--fast-math]
  omni render <input.omni> [--output <path>] [--dur <seconds>] [--sample-rate <hz>] [--block <frames>] [--dump-graph] [--ir] [--fast-math]

Options:
  --output, -o   Output wav path (default: ./omni_out.wav)
  --dur, -d      Render duration in seconds (default: 5)
  --sample-rate, --sr  Render/output sample rate in Hz (default: 48000)
  --block, -b    Block size in frames (default: 512)
  --dump-graph   Print program after graph lowering, before proc desugaring/codegen
  --ir           Print optimized LLVM IR before compile/render
  --meta         Print declared ins/outs/params metadata
  --fast-math    Enable LLVM fast-math flags for floating-point operations
  --help, -h     Show this help
"#;

enum Command {
    Compile {
        input: PathBuf,
        dump_graph: bool,
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
        dump_graph: bool,
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
            dump_graph,
            dump_ir,
            show_meta,
            fast_math,
        } => run_compile(&input, dump_graph, dump_ir, show_meta, fast_math),
        Command::Render {
            input,
            output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            dump_graph,
            dump_ir,
            fast_math,
        } => run_render(
            &input,
            &output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            dump_graph,
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
    let mut dump_graph = false;
    let mut dump_ir = false;
    let mut show_meta = false;
    let mut fast_math = false;
    for arg in args {
        match arg.as_str() {
            "--dump-graph" => dump_graph = true,
            "--ir" => dump_ir = true,
            "--meta" => show_meta = true,
            "--fast-math" => fast_math = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }
    Ok(Command::Compile {
        input: PathBuf::from(input),
        dump_graph,
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
    let mut dump_graph = false;
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
            "--dump-graph" => {
                dump_graph = true;
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
        dump_graph,
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
    dump_graph: bool,
    dump_ir: bool,
    show_meta: bool,
    fast_math: bool,
) -> Result<(), String> {
    if dump_graph {
        let lowered =
            parse_and_lower_graphs(input, DEFAULT_SAMPLE_RATE as f32, DEFAULT_BLOCK_FRAMES)?;
        print!("{}", format_program(&lowered));
    }
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
    dump_graph: bool,
    dump_ir: bool,
    fast_math: bool,
) -> Result<(), String> {
    if dump_graph {
        let lowered = parse_and_lower_graphs(input, sample_rate_hz as f32, block_frames)?;
        print!("{}", format_program(&lowered));
    }
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

fn parse_and_lower_graphs(
    input: &Path,
    sample_rate: f32,
    block_size: usize,
) -> Result<Program, String> {
    let parsed =
        parse_program_file(input).map_err(|diags| format_diagnostics("parse failed", &diags))?;
    lower_graphs_for_inspection_with_options(
        parsed,
        AnalysisOptions {
            sample_rate,
            block_size,
        },
    )
    .map_err(|diags| format_diagnostics("graph lowering failed", &diags))
}

fn format_program(program: &Program) -> String {
    let mut out = String::new();
    for block in program
        .blocks
        .iter()
        .filter(|block| !matches!(block, Block::Def(_)))
    {
        format_block(block, 0, &mut out);
        out.push('\n');
    }
    out
}

fn format_block(block: &Block, indent: usize, out: &mut String) {
    match block {
        Block::Ins(ports) => format_port_block("ins", ports, indent, out),
        Block::Outs(ports) => format_port_block("outs", ports, indent, out),
        Block::Params(params) => format_param_block("params", params, indent, out),
        Block::Const(decl) => push_line(
            out,
            indent,
            &format!("const {} = {}", decl.name, format_expr(&decl.expr)),
        ),
        Block::Events(events) => {
            push_line(out, indent, "events:");
            for event in events {
                format_event(event, indent + 1, out);
            }
        }
        Block::Buffers(buffers) => format_buffer_block("buffers", buffers, indent, out),
        Block::Assert(assert_decl) => {
            push_line(
                out,
                indent,
                &format!("assert({})", format_expr(&assert_decl.expr)),
            );
        }
        Block::Proc(proc) => format_proc(proc, indent, out),
        Block::Struct(def) => format_struct(def, indent, out),
        Block::Def(def) => format_def(def, indent, out),
        Block::Init(init) => format_init_block("init", init, indent, out),
        Block::Block(exec) => format_block_exec(exec, indent, out),
        Block::Sample(sample) => format_sample_block("sample", sample, indent, out),
        Block::Graph(graph) => {
            push_line(out, indent, "graph:");
            for edge in &graph.edges {
                let mut text = String::new();
                if let Some(rate) = edge.rate {
                    text.push_str(match rate {
                        omni_frontend::GraphRate::Block => "@block ",
                        omni_frontend::GraphRate::Sample => "@sample ",
                    });
                }
                text.push_str(&format_expr(&edge.source));
                text.push_str(" >>");
                if let Some(delay) = &edge.delay {
                    text.push('[');
                    text.push_str(&format_expr(delay));
                    text.push(']');
                }
                text.push(' ');
                text.push_str(&format_graph_endpoint(&edge.dest));
                push_line(out, indent + 1, &text);
            }
        }
    }
}

fn format_port_block(
    label: &str,
    ports: &[omni_frontend::PortDecl],
    indent: usize,
    out: &mut String,
) {
    push_line(out, indent, &format!("{label}:"));
    for port in ports {
        push_line(out, indent + 1, &format_port_decl(port));
    }
}

fn format_param_block(
    label: &str,
    params: &[omni_frontend::ParamDecl],
    indent: usize,
    out: &mut String,
) {
    push_line(out, indent, &format!("{label}:"));
    for param in params {
        push_line(out, indent + 1, &format_param_decl(param));
    }
}

fn format_buffer_block(
    label: &str,
    buffers: &[omni_frontend::BufferDecl],
    indent: usize,
    out: &mut String,
) {
    push_line(out, indent, &format!("{label}:"));
    for buffer in buffers {
        let mut text = buffer.name.clone();
        if let Some(ty) = &buffer.ty {
            text.push_str(": ");
            text.push_str(&format_buffer_type(ty));
        }
        push_line(out, indent + 1, &text);
    }
}

fn format_init_block(label: &str, init: &InitBlock, indent: usize, out: &mut String) {
    if let Some(default_ty) = &init.default_ty {
        push_line(
            out,
            indent,
            &format!("{label}<{}>:", format_decl_type(default_ty)),
        );
    } else {
        push_line(out, indent, &format!("{label}:"));
    }
    format_stmt_list(&init.body, indent + 1, out);
}

fn format_sample_block(label: &str, sample: &SampleBlock, indent: usize, out: &mut String) {
    let header = if let Some(factor) = &sample.oversample_factor {
        format!("{label} {}:", format_expr(factor))
    } else {
        format!("{label}:")
    };
    push_line(out, indent, &header);
    format_stmt_list(&sample.body, indent + 1, out);
}

fn format_block_exec(exec: &BlockExec, indent: usize, out: &mut String) {
    push_line(out, indent, "block:");
    if !exec.pre.is_empty() {
        push_line(out, indent + 1, "pre:");
        format_stmt_list(&exec.pre, indent + 2, out);
    }
    if let Some(sample) = &exec.sample {
        format_sample_block("sample", sample, indent + 1, out);
    }
    if !exec.post.is_empty() {
        push_line(out, indent + 1, "post:");
        format_stmt_list(&exec.post, indent + 2, out);
    }
}

fn format_proc(proc: &ProcessorDef, indent: usize, out: &mut String) {
    let header = if proc.type_params.is_empty() {
        format!("proc {}:", proc.name)
    } else {
        format!("proc {}<{}>:", proc.name, proc.type_params.join(", "))
    };
    push_line(out, indent, &header);
    if !proc.ins.is_empty() {
        format_port_block("ins", &proc.ins, indent + 1, out);
    }
    if !proc.outs.is_empty() {
        format_port_block("outs", &proc.outs, indent + 1, out);
    }
    if !proc.params.is_empty() {
        format_param_block("params", &proc.params, indent + 1, out);
    }
    if !proc.events.is_empty() {
        push_line(out, indent + 1, "events:");
        for event in &proc.events {
            format_event(event, indent + 2, out);
        }
    }
    if !proc.buffers.is_empty() {
        format_buffer_block("buffers", &proc.buffers, indent + 1, out);
    }
    if proc.has_init_block || !proc.init.body.is_empty() {
        format_init_block("init", &proc.init, indent + 1, out);
    }
    if proc.has_block_block || !proc.block_pre.is_empty() || !proc.block_post.is_empty() {
        push_line(out, indent + 1, "block:");
        if !proc.block_pre.is_empty() {
            push_line(out, indent + 2, "pre:");
            format_stmt_list(&proc.block_pre, indent + 3, out);
        }
        if !proc.block_post.is_empty() {
            push_line(out, indent + 2, "post:");
            format_stmt_list(&proc.block_post, indent + 3, out);
        }
    }
    if proc.has_sample_block || !proc.sample.is_empty() {
        let header = if let Some(factor) = &proc.sample_oversample_factor {
            format!("sample {}:", format_expr(factor))
        } else {
            "sample:".to_owned()
        };
        push_line(out, indent + 1, &header);
        format_stmt_list(&proc.sample, indent + 2, out);
    }
    for def in &proc.local_defs {
        format_def(def, indent + 1, out);
    }
}

fn format_struct(def: &StructDef, indent: usize, out: &mut String) {
    let header = if def.type_params.is_empty() {
        format!("struct {}:", def.name)
    } else {
        format!("struct {}<{}>:", def.name, def.type_params.join(", "))
    };
    push_line(out, indent, &header);
    for field in &def.fields {
        let mut text = format!("{}: {}", field.name, format_field_type(&field.ty));
        if let Some(default) = &field.default {
            text.push_str(" = ");
            text.push_str(&format_expr(default));
        }
        push_line(out, indent + 1, &text);
    }
    for method in &def.methods {
        format_def(method, indent + 1, out);
    }
}

fn format_def(def: &FunctionDef, indent: usize, out: &mut String) {
    let mut header = format!("def {}", def.name);
    if !def.type_params.is_empty() {
        header.push('<');
        header.push_str(&def.type_params.join(", "));
        header.push('>');
    }
    header.push('(');
    header.push_str(
        &def.params
            .iter()
            .map(|param| {
                let mut text = param.name.clone();
                if let Some(ty) = &param.ty {
                    text.push_str(": ");
                    text.push_str(&format_fn_param_type(ty));
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(&format_expr(default));
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", "),
    );
    header.push_str("):");
    push_line(out, indent, &header);
    format_stmt_list(&def.body, indent + 1, out);
}

fn format_event(event: &omni_frontend::EventDef, indent: usize, out: &mut String) {
    let mut header = format!("{}(", event.name);
    header.push_str(
        &event
            .params
            .iter()
            .map(|param| format!("{}: {}", param.name, format_event_param_type(&param.ty)))
            .collect::<Vec<_>>()
            .join(", "),
    );
    header.push_str("):");
    push_line(out, indent, &header);
    format_stmt_list(&event.body, indent + 1, out);
}

fn format_stmt_list(stmts: &[Stmt], indent: usize, out: &mut String) {
    if stmts.is_empty() {
        push_line(out, indent, "pass");
        return;
    }
    for stmt in stmts {
        format_stmt(stmt, indent, out);
    }
}

fn format_stmt(stmt: &Stmt, indent: usize, out: &mut String) {
    match stmt {
        Stmt::Const { decl, .. } => {
            let mut text = format!("const {}", decl.name);
            if let Some(ty) = decl.ty {
                text.push_str(": ");
                text.push_str(primitive_type_name(ty));
            }
            text.push_str(" = ");
            text.push_str(&format_expr(&decl.expr));
            push_line(out, indent, &text);
        }
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            let lhs = format_assign_target(target);
            let mut text = lhs;
            if *is_typed_decl {
                if let Some(ty) = decl_ty {
                    text.push_str(": ");
                    text.push_str(primitive_type_name(*ty));
                } else if let Some(ty) = generic_decl_ty {
                    text.push_str(": ");
                    text.push_str(ty);
                }
            }
            text.push_str(" = ");
            text.push_str(&format_expr(expr));
            push_line(out, indent, &text);
        }
        Stmt::Expr { expr, .. } => push_line(out, indent, &format_expr(expr)),
        Stmt::Return { expr, .. } => {
            push_line(out, indent, &format!("return {}", format_expr(expr)))
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            push_line(out, indent, &format!("if {}:", format_expr(cond)));
            format_stmt_list(then_branch, indent + 1, out);
            if !else_branch.is_empty() {
                push_line(out, indent, "else:");
                format_stmt_list(else_branch, indent + 1, out);
            }
        }
        Stmt::For {
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
            ..
        } => {
            let mut text = format!("for {} in {}..", var, format_expr(start));
            if *end_inclusive {
                text.push('=');
            }
            text.push_str(&format_expr(end));
            if let Some(step) = step {
                text.push_str(" step ");
                text.push_str(&format_expr(step));
            }
            text.push(':');
            push_line(out, indent, &text);
            format_stmt_list(body, indent + 1, out);
        }
        Stmt::While { cond, body, .. } => {
            push_line(out, indent, &format!("while {}:", format_expr(cond)));
            format_stmt_list(body, indent + 1, out);
        }
        Stmt::Break { .. } => push_line(out, indent, "break"),
        Stmt::Continue { .. } => push_line(out, indent, "continue"),
    }
}

fn format_assign_target(target: &AssignTarget) -> String {
    match target {
        AssignTarget::Var(name) => name.clone(),
        AssignTarget::Index { base, index } => format!("{base}[{}]", format_expr(index)),
        AssignTarget::Slice { base, start, end } => format!(
            "{base}[{}:{}]",
            start
                .as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default(),
            end.as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default()
        ),
    }
}

fn format_graph_endpoint(endpoint: &omni_frontend::GraphEndpoint) -> String {
    match endpoint {
        omni_frontend::GraphEndpoint::Symbol(name) => name.clone(),
        omni_frontend::GraphEndpoint::ProcField { proc, field } => format!("{proc}.{field}"),
        omni_frontend::GraphEndpoint::ProcIndexedField { proc, index, field } => {
            format!("{proc}[{}].{field}", format_expr(index))
        }
    }
}

fn format_expr(expr: &Expr) -> String {
    format_expr_prec(expr, 0)
}

fn format_expr_prec(expr: &Expr, parent_prec: u8) -> String {
    let my_prec = expr_precedence(expr);
    match expr {
        Expr::Number(value) => format_number(*value),
        Expr::Int(value) => value.to_string(),
        Expr::Bool(value) => value.to_string(),
        Expr::ArrayLiteral(values) => format!(
            "[{}]",
            values
                .iter()
                .map(format_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Var(name) => name.clone(),
        Expr::Index { base, index } => format!("{base}[{}]", format_expr(index)),
        Expr::Slice { base, start, end } => format!(
            "{base}[{}:{}]",
            start
                .as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default(),
            end.as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default()
        ),
        Expr::ArrayCtor { spec, init } => {
            let mut text = format!("{}(", format_array_type_spec(spec));
            if let Some(init) = init {
                text.push_str(&init.iter().map(format_expr).collect::<Vec<_>>().join(", "));
            }
            text.push(')');
            text
        }
        Expr::Compare { op, lhs, rhs } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_cmp_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Call { func, args } => format!(
            "{}({})",
            format_builtin_fn(*func),
            args.iter().map(format_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            let mut text = name.clone();
            if !type_args.is_empty() {
                text.push('<');
                text.push_str(
                    &type_args
                        .iter()
                        .map(format_call_type_arg)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                text.push('>');
            }
            text.push('(');
            text.push_str(
                &args
                    .iter()
                    .map(format_call_arg)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push(')');
            text
        }
        Expr::Cast { to, expr } => format!("{}({})", primitive_type_name(*to), format_expr(expr)),
        Expr::UnaryNot { expr } => wrap_if_needed(
            format!("!{}", format_expr_prec(expr, my_prec)),
            my_prec,
            parent_prec,
        ),
        Expr::UnaryBitNot { expr } => wrap_if_needed(
            format!("~{}", format_expr_prec(expr, my_prec)),
            my_prec,
            parent_prec,
        ),
        Expr::Logical { op, lhs, rhs } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_logical_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Binary { op, lhs, rhs } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_binary_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
    }
}

fn expr_precedence(expr: &Expr) -> u8 {
    match expr {
        Expr::Logical {
            op: LogicalOp::Or, ..
        } => 1,
        Expr::Logical {
            op: LogicalOp::And, ..
        } => 2,
        Expr::Binary {
            op: BinaryOp::BitOr,
            ..
        } => 3,
        Expr::Binary {
            op: BinaryOp::BitXor,
            ..
        } => 4,
        Expr::Binary {
            op: BinaryOp::BitAnd,
            ..
        } => 5,
        Expr::Compare { .. } => 6,
        Expr::Binary {
            op: BinaryOp::ShiftLeft | BinaryOp::ShiftRight,
            ..
        } => 7,
        Expr::Binary {
            op: BinaryOp::Add | BinaryOp::Sub,
            ..
        } => 8,
        Expr::Binary {
            op: BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod,
            ..
        } => 9,
        Expr::UnaryNot { .. } | Expr::UnaryBitNot { .. } => 10,
        _ => 11,
    }
}

fn wrap_if_needed(text: String, my_prec: u8, parent_prec: u8) -> String {
    if my_prec < parent_prec {
        format!("({text})")
    } else {
        text
    }
}

fn format_call_arg(arg: &CallArg) -> String {
    match &arg.name {
        Some(name) => format!("{name} = {}", format_expr(&arg.expr)),
        None => format_expr(&arg.expr),
    }
}

fn format_call_type_arg(arg: &CallTypeArg) -> String {
    match arg {
        CallTypeArg::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        CallTypeArg::Generic(name) => name.clone(),
    }
}

fn format_decl_type(ty: &DeclType) -> String {
    match ty {
        DeclType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        DeclType::Generic(name) => name.clone(),
        DeclType::ArrayGeneric { elem, size } => format!("{elem}[{}]", format_expr(size)),
        DeclType::Array { elem, size } => {
            format!("{}[{}]", primitive_type_name(*elem), format_expr(size))
        }
    }
}

fn format_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        FieldType::Generic(name) => name.clone(),
        FieldType::Array(spec) => format_array_type_spec(spec),
    }
}

fn format_array_type_spec(spec: &ArrayTypeSpec) -> String {
    let elem = match &spec.elem {
        ArrayElemType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        ArrayElemType::Struct(name) => name.clone(),
    };
    format!("{elem}[{}]", format_expr(spec.size.as_ref()))
}

fn format_fn_param_type(ty: &omni_frontend::FnParamType) -> String {
    match ty {
        omni_frontend::FnParamType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        omni_frontend::FnParamType::Struct(name) => name.clone(),
        omni_frontend::FnParamType::Buffer(ty) => format_buffer_type(ty),
        omni_frontend::FnParamType::Array(Some(ty)) => format!("{}[]", primitive_type_name(*ty)),
        omni_frontend::FnParamType::Array(None) => "[]".to_owned(),
        omni_frontend::FnParamType::ArrayGeneric(name) => format!("{name}[]"),
        omni_frontend::FnParamType::BareBuffer => "buffer".to_owned(),
    }
}

fn format_buffer_type(ty: &BufferType) -> String {
    let elem = match &ty.elem {
        BufferElemType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        BufferElemType::Generic(name) => name.clone(),
    };
    let channels = match &ty.channels {
        BufferChannels::Mono => String::new(),
        BufferChannels::Static(expr) => format!("[{}]", format_expr(expr)),
        BufferChannels::Dynamic => "[]".to_owned(),
    };
    format!("buffer[{elem}{channels}]")
}

fn format_event_param_type(ty: &EventParamType) -> String {
    match ty {
        EventParamType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        EventParamType::Array { elem, size } => {
            format!("{}[{}]", primitive_type_name(*elem), format_expr(size))
        }
        EventParamType::Slice { elem } => format!("{}[]", primitive_type_name(*elem)),
        EventParamType::GenericSlice { elem } => format!("{elem}[]"),
    }
}

fn format_port_decl(port: &omni_frontend::PortDecl) -> String {
    let mut text = port.name.clone();
    if let Some(ty) = &port.ty {
        text.push_str(": ");
        text.push_str(&format_decl_type(ty));
    }
    if let Some(default) = &port.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    if let Some(range) = &port.range {
        text.push(' ');
        text.push('{');
        if let Some(min) = &range.min {
            text.push_str(&format_expr(min));
            text.push_str(", ");
        }
        text.push_str(&format_expr(&range.max));
        text.push('}');
    }
    text
}

fn format_param_decl(param: &omni_frontend::ParamDecl) -> String {
    let mut text = param.name.clone();
    if let Some(ty) = &param.ty {
        text.push_str(": ");
        text.push_str(&format_decl_type(ty));
    }
    if let Some(default) = &param.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    if let Some(range) = &param.range {
        text.push(' ');
        text.push('{');
        if let Some(min) = &range.min {
            text.push_str(&format_expr(min));
            text.push_str(", ");
        }
        text.push_str(&format_expr(&range.max));
        text.push('}');
    }
    text
}

fn format_builtin_fn(func: BuiltinFn) -> &'static str {
    match func {
        BuiltinFn::Sin => "sin",
        BuiltinFn::Cos => "cos",
        BuiltinFn::Tan => "tan",
        BuiltinFn::Tanh => "tanh",
        BuiltinFn::Atan => "atan",
        BuiltinFn::Atan2 => "atan2",
        BuiltinFn::Exp => "exp",
        BuiltinFn::Log => "log",
        BuiltinFn::Sqrt => "sqrt",
        BuiltinFn::Pow => "pow",
        BuiltinFn::Abs => "abs",
        BuiltinFn::Floor => "floor",
        BuiltinFn::Ceil => "ceil",
        BuiltinFn::Round => "round",
        BuiltinFn::Trunc => "trunc",
        BuiltinFn::Min => "min",
        BuiltinFn::Max => "max",
        BuiltinFn::Fma => "fma",
    }
}

fn format_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight => ">>",
    }
}

fn format_logical_op(op: LogicalOp) -> &'static str {
    match op {
        LogicalOp::And => "&&",
        LogicalOp::Or => "||",
    }
}

fn format_cmp_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn format_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn push_line(out: &mut String, indent: usize, line: &str) {
    out.push_str(&"  ".repeat(indent));
    out.push_str(line);
    out.push('\n');
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

#[cfg(test)]
mod tests {
    use super::{format_expr, parse_args, Command};
    use omni_frontend::{CallArg, Expr};

    #[test]
    fn parse_compile_accepts_dump_graph() {
        let cmd = parse_args(
            ["omni", "compile", "x.omni", "--dump-graph"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("compile args should parse");
        match cmd {
            Command::Compile { dump_graph, .. } => assert!(dump_graph),
            _ => panic!("expected compile command"),
        }
    }

    #[test]
    fn parse_render_accepts_dump_graph() {
        let cmd = parse_args(
            ["omni", "render", "x.omni", "--dump-graph"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("render args should parse");
        match cmd {
            Command::Render { dump_graph, .. } => assert!(dump_graph),
            _ => panic!("expected render command"),
        }
    }

    #[test]
    fn format_expr_prints_named_call_args_with_equals() {
        let expr = Expr::UserCall {
            name: "sat".to_owned(),
            type_args: Vec::new(),
            args: vec![CallArg {
                name: Some("in1".to_owned()),
                expr: Expr::Var("mix.out1".to_owned()),
            }],
        };
        assert_eq!(format_expr(&expr), "sat(in1 = mix.out1)");
    }
}
