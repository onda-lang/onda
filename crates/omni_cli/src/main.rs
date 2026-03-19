use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

mod daemon_stdio;
mod lsp_stdio;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use omni_codegen_llvm::{
    lower_and_jit_with_options, lower_to_llvm_ir_with_options, CompileOptions, ExecutionBackend,
};
use omni_daemon::{
    DaemonConfig, DaemonSession, PreviewBufferChannels, PreviewBufferInfo, PreviewBuildError,
    PreviewOptions, PreviewParamInfo,
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
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_DUR_SECONDS: u32 = 5;
const MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK: usize = 64;
const DEFAULT_BLOCK_FRAMES: usize = 512;
const DEFAULT_PLAY_BLOCK_FRAMES: usize = 128;
const DEFAULT_DAEMON_OUTPUT: &str = "./omni_daemon_out.wav";

const USAGE: &str = r#"Usage:
  omni compile <input.omni> [--dump-graph] [--ir] [--meta] [--fast-math]
  omni render <input.omni> [--output <path>] [--dur <seconds>] [--sample-rate <hz>] [--block <frames>] [--dump-graph] [--ir] [--fast-math]
  omni lsp
  omni preview <input.omni> [--sample-rate <hz>] [--block <frames>] [--fast-math] [--input-device <name>] [--output-device <name>]
  omni preview play <input.omni> [--dur <seconds> | --forever] [--sample-rate <hz>] [--block <frames>] [--fast-math] [--meta] [--set <name=value>] [--control-json] [--input-device <name>] [--output-device <name>]
  omni preview render <input.omni> [--output <path>] [--dur <seconds>] [--sample-rate <hz>] [--block <frames>] [--fast-math] [--meta] [--set <name=value>]
  omni daemon diagnose <input.omni> [--sample-rate <hz>] [--block <frames>]
  omni daemon stdio

Options:
  --output, -o   Output wav path (default: ./omni_out.wav)
  --dur, -d      Render duration in seconds (default: 5)
  --sample-rate, --sr  Render/output sample rate in Hz (default: 48000)
  --block, -b    Block size in frames (default: 512; preview play: 128)
  --dump-graph   Print program after graph lowering, before proc desugaring/codegen
  --ir           Print optimized LLVM IR before compile/render
  --meta         Print declared ins/outs/params metadata
  --control-json Emit preview control handshake on stdout and serve param control over localhost
  --input-device Select audio input device by exact name for preview playback
  --output-device Select audio output device by exact name for preview playback
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
    Lsp,
    Preview(PreviewCommand),
    Daemon(DaemonCommand),
}

enum PreviewCommand {
    Play {
        input: PathBuf,
        dur_seconds: Option<u32>,
        sample_rate_hz: u32,
        block_frames: usize,
        input_device: Option<String>,
        output_device: Option<String>,
        fast_math: bool,
        show_meta: bool,
        control_json: bool,
        param_sets: Vec<(String, f64)>,
    },
    Render {
        input: PathBuf,
        output: PathBuf,
        dur_seconds: u32,
        sample_rate_hz: u32,
        block_frames: usize,
        fast_math: bool,
        show_meta: bool,
        param_sets: Vec<(String, f64)>,
    },
    Window {
        input: PathBuf,
        sample_rate_hz: u32,
        block_frames: usize,
        input_device: Option<String>,
        output_device: Option<String>,
        fast_math: bool,
    },
}

enum DaemonCommand {
    Stdio,
    Diagnose {
        input: PathBuf,
        sample_rate_hz: u32,
        block_frames: usize,
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
        Command::Lsp => lsp_stdio::run_stdio_loop(),
        Command::Preview(cmd) => run_preview(cmd),
        Command::Daemon(cmd) => run_daemon(cmd),
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
        "lsp" => parse_lsp_args(args),
        "preview" => parse_preview_args(args),
        "daemon" => parse_daemon_args(args),
        _ => Err(format!("unknown command '{cmd}'\n\n{USAGE}")),
    }
}

fn parse_lsp_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    for arg in args {
        match arg.as_str() {
            "--stdio" => {}
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }
    Ok(Command::Lsp)
}

fn parse_preview_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(subcommand) = args.next() else {
        return Err(format!(
            "preview requires a subcommand or input file\n\n{USAGE}"
        ));
    };
    let preview = match subcommand.as_str() {
        "play" => parse_preview_play_args(args)?,
        "render" => parse_preview_render_args(args)?,
        _ => {
            // Treat as `omni preview <file.omni>` — the windowed preview.
            if subcommand.starts_with('-') {
                return Err(format!("unknown preview option '{subcommand}'\n\n{USAGE}"));
            }
            parse_preview_window_args(subcommand, args)?
        }
    };
    Ok(Command::Preview(preview))
}

fn parse_daemon_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(subcommand) = args.next() else {
        return Err(format!("daemon requires a subcommand\n\n{USAGE}"));
    };
    let daemon = match subcommand.as_str() {
        "stdio" => DaemonCommand::Stdio,
        "diagnose" => parse_daemon_diagnose_args(args)?,
        "play" => {
            return Err(format!(
                "daemon play was renamed to 'omni preview play'\n\n{USAGE}"
            ))
        }
        "preview" => {
            return Err(format!(
                "daemon preview was renamed to 'omni preview render'\n\n{USAGE}"
            ))
        }
        _ => {
            return Err(format!(
                "unknown daemon subcommand '{subcommand}'\n\n{USAGE}"
            ))
        }
    };
    Ok(Command::Daemon(daemon))
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

fn parse_daemon_diagnose_args(
    mut args: impl Iterator<Item = String>,
) -> Result<DaemonCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("daemon diagnose requires an input file\n\n{USAGE}"));
    };
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block=") => {
                block_frames = parse_block_frames(&arg["--block=".len()..])?;
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }
    Ok(DaemonCommand::Diagnose {
        input: PathBuf::from(input),
        sample_rate_hz,
        block_frames,
    })
}

fn parse_preview_render_args(
    mut args: impl Iterator<Item = String>,
) -> Result<PreviewCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("preview render requires an input file\n\n{USAGE}"));
    };

    let mut output = PathBuf::from(DEFAULT_DAEMON_OUTPUT);
    let mut dur_seconds = DEFAULT_DUR_SECONDS;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    let mut fast_math = false;
    let mut show_meta = false;
    let mut param_sets = Vec::new();

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
            "--set" => {
                let Some(value) = args.next() else {
                    return Err("--set requires a name=value pair".to_owned());
                };
                param_sets.push(parse_param_setting(&value)?);
            }
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ if arg.starts_with("--output=") => {
                output = PathBuf::from(&arg["--output=".len()..]);
            }
            _ if arg.starts_with("--dur=") => {
                dur_seconds = parse_dur_seconds(&arg["--dur=".len()..])?;
            }
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block=") => {
                block_frames = parse_block_frames(&arg["--block=".len()..])?;
            }
            _ if arg.starts_with("--set=") => {
                param_sets.push(parse_param_setting(&arg["--set=".len()..])?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }

    Ok(PreviewCommand::Render {
        input: PathBuf::from(input),
        output,
        dur_seconds,
        sample_rate_hz,
        block_frames,
        fast_math,
        show_meta,
        param_sets,
    })
}

fn parse_preview_window_args(
    input: String,
    mut args: impl Iterator<Item = String>,
) -> Result<PreviewCommand, String> {
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_PLAY_BLOCK_FRAMES;
    let mut input_device = None;
    let mut output_device = None;
    let mut fast_math = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
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
            "--input-device" => {
                let Some(value) = args.next() else {
                    return Err("--input-device requires a device name".to_owned());
                };
                input_device = Some(value);
            }
            "--output-device" => {
                let Some(value) = args.next() else {
                    return Err("--output-device requires a device name".to_owned());
                };
                output_device = Some(value);
            }
            "--fast-math" => fast_math = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block=") => {
                block_frames = parse_block_frames(&arg["--block=".len()..])?;
            }
            _ if arg.starts_with("--input-device=") => {
                input_device = Some(arg["--input-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--output-device=") => {
                output_device = Some(arg["--output-device=".len()..].to_owned());
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }

    Ok(PreviewCommand::Window {
        input: PathBuf::from(input),
        sample_rate_hz,
        block_frames,
        input_device,
        output_device,
        fast_math,
    })
}

fn parse_preview_play_args(
    mut args: impl Iterator<Item = String>,
) -> Result<PreviewCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("preview play requires an input file\n\n{USAGE}"));
    };

    let mut dur_seconds = Some(DEFAULT_DUR_SECONDS);
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_PLAY_BLOCK_FRAMES;
    let mut input_device = None;
    let mut output_device = None;
    let mut fast_math = false;
    let mut show_meta = false;
    let mut control_json = false;
    let mut param_sets = Vec::new();
    let mut forever = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dur" | "-d" => {
                let Some(value) = args.next() else {
                    return Err("--dur requires a positive integer value".to_owned());
                };
                if forever {
                    return Err("--dur cannot be combined with --forever".to_owned());
                }
                dur_seconds = Some(parse_dur_seconds(&value)?);
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
            "--set" => {
                let Some(value) = args.next() else {
                    return Err("--set requires a name=value pair".to_owned());
                };
                param_sets.push(parse_param_setting(&value)?);
            }
            "--input-device" => {
                let Some(value) = args.next() else {
                    return Err("--input-device requires a device name".to_owned());
                };
                input_device = Some(value);
            }
            "--output-device" => {
                let Some(value) = args.next() else {
                    return Err("--output-device requires a device name".to_owned());
                };
                output_device = Some(value);
            }
            "--forever" => {
                if dur_seconds != Some(DEFAULT_DUR_SECONDS) {
                    return Err("--forever cannot be combined with --dur".to_owned());
                }
                forever = true;
                dur_seconds = None;
            }
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--control-json" => control_json = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ if arg.starts_with("--dur=") => {
                if forever {
                    return Err("--dur cannot be combined with --forever".to_owned());
                }
                dur_seconds = Some(parse_dur_seconds(&arg["--dur=".len()..])?);
            }
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block=") => {
                block_frames = parse_block_frames(&arg["--block=".len()..])?;
            }
            _ if arg.starts_with("--input-device=") => {
                input_device = Some(arg["--input-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--output-device=") => {
                output_device = Some(arg["--output-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--set=") => {
                param_sets.push(parse_param_setting(&arg["--set=".len()..])?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{USAGE}")),
        }
    }

    Ok(PreviewCommand::Play {
        input: PathBuf::from(input),
        dur_seconds,
        sample_rate_hz,
        block_frames,
        input_device,
        output_device,
        fast_math,
        show_meta,
        control_json,
        param_sets,
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

fn parse_param_setting(value: &str) -> Result<(String, f64), String> {
    let Some((name, raw_value)) = value.split_once('=') else {
        return Err(format!(
            "invalid parameter setting '{value}', expected name=value"
        ));
    };
    if name.is_empty() {
        return Err("parameter setting requires a non-empty name".to_owned());
    }
    let parsed = raw_value.parse::<f64>().map_err(|_| {
        format!("invalid parameter value '{raw_value}' for '{name}', expected number")
    })?;
    Ok((name.to_owned(), parsed))
}

fn run_daemon(cmd: DaemonCommand) -> Result<(), String> {
    match cmd {
        DaemonCommand::Stdio => daemon_stdio::run_stdio_loop(),
        DaemonCommand::Diagnose {
            input,
            sample_rate_hz,
            block_frames,
        } => run_daemon_diagnose(&input, sample_rate_hz, block_frames),
    }
}

fn run_preview(cmd: PreviewCommand) -> Result<(), String> {
    match cmd {
        PreviewCommand::Play {
            input,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            input_device,
            output_device,
            fast_math,
            show_meta,
            control_json,
            param_sets,
        } => run_daemon_play(
            &input,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            input_device.as_deref(),
            output_device.as_deref(),
            fast_math,
            show_meta,
            control_json,
            &param_sets,
        ),
        PreviewCommand::Render {
            input,
            output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            fast_math,
            show_meta,
            param_sets,
        } => run_daemon_preview(
            &input,
            &output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            fast_math,
            show_meta,
            &param_sets,
        ),
        PreviewCommand::Window {
            input,
            sample_rate_hz,
            block_frames,
            input_device,
            output_device,
            fast_math,
        } => {
            let omni_bin = env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "omni".to_owned());
            omni_window::run_preview_window(
                &input,
                omni_window::PreviewWindowOptions {
                    sample_rate_hz,
                    block_frames,
                    input_device,
                    output_device,
                    fast_math,
                    omni_bin,
                },
            )
        }
    }
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
        preview: PreviewOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
            ..PreviewOptions::default()
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

fn run_daemon_preview(
    input: &Path,
    output: &Path,
    dur_seconds: u32,
    sample_rate_hz: u32,
    block_frames: usize,
    fast_math: bool,
    show_meta: bool,
    param_sets: &[(String, f64)],
) -> Result<(), String> {
    let mut session = DaemonSession::new(DaemonConfig {
        analysis: AnalysisOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
        },
        preview: PreviewOptions {
            sample_rate: sample_rate_hz as f32,
            block_size: block_frames,
            fast_math,
            ..PreviewOptions::default()
        },
    });

    session
        .start_preview(input)
        .map_err(|err| format_preview_build_error("daemon preview start failed", &err))?;

    if show_meta {
        let info = session
            .preview(input)
            .expect("preview should be active after successful start")
            .param_info();
        if !info.is_empty() {
            println!("{}", format_preview_param_info(&info));
        }
    }

    for (name, value) in param_sets {
        session
            .preview_mut(input)
            .expect("preview should be active while applying params")
            .set_param_f64(name, *value)
            .map_err(|diag| format_single_diagnostic("daemon preview param failed", &diag))?;
    }

    let total_frames = sample_rate_hz as usize * dur_seconds as usize;
    let full_blocks = total_frames / block_frames;
    let tail_frames = total_frames % block_frames;
    let mut rendered = Vec::<f32>::new();

    for _ in 0..full_blocks {
        let block = session
            .render_preview_block(input)
            .map_err(|diag| format_single_diagnostic("daemon preview render failed", &diag))?;
        append_interleaved_block(&mut rendered, &block);
    }
    if tail_frames > 0 {
        let block = session
            .render_preview_block(input)
            .map_err(|diag| format_single_diagnostic("daemon preview render failed", &diag))?;
        let mut interleaved = Vec::<f32>::new();
        append_interleaved_block(&mut interleaved, &block);
        let channels = block.len().max(1);
        rendered.extend_from_slice(&interleaved[..tail_frames * channels]);
    }

    let out_channels = session
        .preview(input)
        .expect("preview should remain active through render")
        .output_channel_count();
    if out_channels == 0 {
        return Err("daemon preview requires at least one output channel".to_owned());
    }

    write_wav_interleaved_i16(output, out_channels, sample_rate_hz, &rendered)?;
    println!(
        "Wrote {} seconds of daemon-preview audio to {}",
        dur_seconds,
        output.display()
    );
    Ok(())
}

fn run_daemon_play(
    input: &Path,
    dur_seconds: Option<u32>,
    sample_rate_hz: u32,
    block_frames: usize,
    input_device: Option<&str>,
    output_device: Option<&str>,
    fast_math: bool,
    show_meta: bool,
    control_json: bool,
    param_sets: &[(String, f64)],
) -> Result<(), String> {
    play_preview_realtime(PlaybackLaunch {
        input: input.to_path_buf(),
        dur_seconds,
        sample_rate_hz,
        block_frames,
        input_device: input_device.map(str::to_owned),
        output_device: output_device.map(str::to_owned),
        fast_math,
        show_meta,
        control_json,
        param_sets: param_sets.to_vec(),
    })
}

fn play_preview_realtime(launch: PlaybackLaunch) -> Result<(), String> {
    let host = cpal::default_host();
    let output_device = find_output_device(&host, launch.output_device.as_deref())?;
    let default_output_config = output_device
        .default_output_config()
        .map_err(|err| format!("failed to query default output config: {err}"))?;

    let output_device_channels = usize::from(default_output_config.channels());
    let mut output_config: cpal::StreamConfig = default_output_config.config();
    output_config.channels = default_output_config.channels();
    output_config.sample_rate = cpal::SampleRate(launch.sample_rate_hz);
    output_config.buffer_size = cpal::BufferSize::Fixed(launch.block_frames as u32);

    let queue_capacity = (launch.block_frames * output_device_channels.max(2) * 16)
        .next_power_of_two()
        .max(1024);
    let sample_queue = Arc::new(SpscSampleRing::new(queue_capacity));
    let input_queue = Arc::new(SpscSampleRing::new(queue_capacity));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let render_error = Arc::new(Mutex::new(None::<String>));
    let error_state = Arc::new(Mutex::new(None::<String>));
    let (startup_tx, startup_rx) = mpsc::channel();
    let (control_tx, control_rx) = if launch.control_json {
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
    let render_thread = spawn_preview_render_thread(
        launch.clone(),
        Arc::clone(&sample_queue),
        Arc::clone(&input_queue),
        Arc::clone(&scope_ring),
        Arc::clone(&stop_flag),
        Arc::clone(&render_error),
        startup_tx,
        control_rx,
    );
    let startup = startup_rx
        .recv()
        .map_err(|_| "preview render thread exited before startup completed".to_owned())??;

    let control_server = if launch.control_json {
        let Some(control_tx) = control_tx else {
            unreachable!("control channel should exist when control json is enabled");
        };
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|err| format!("failed to bind preview control socket: {err}"))?;
        let port = listener
            .local_addr()
            .map_err(|err| format!("failed to query preview control socket: {err}"))?
            .port();
        let startup_message = json!({
            "event": "ready",
            "path": display_path(&startup.path),
            "port": port,
            "params": startup.params.iter().map(preview_param_json).collect::<Vec<_>>(),
            "buffers": startup.buffers.iter().map(preview_buffer_json).collect::<Vec<_>>(),
            "outputChannels": startup.output_channels,
            "inputDevices": startup.input_devices,
            "outputDevices": startup.output_devices,
            "currentInputDevice": startup.current_input_device,
            "currentOutputDevice": startup.current_output_device,
        });
        write_json_line(
            &mut BufWriter::new(std::io::stdout().lock()),
            &startup_message,
        )
        .map_err(|err| format!("failed to write preview control startup event: {err}"))?;
        Some(spawn_preview_control_server(
            listener,
            control_tx,
            Arc::clone(&scope_ring),
            Arc::clone(&stop_flag),
        ))
    } else {
        None
    };

    if launch.show_meta && !startup.params.is_empty() {
        if launch.control_json {
            eprintln!("{}", format_preview_param_info(&startup.params));
        } else {
            println!("{}", format_preview_param_info(&startup.params));
        }
    }
    if startup.output_channels == 0 {
        stop_flag.store(true, Ordering::Release);
        let _ = render_thread.join();
        drop(control_server);
        return Err("daemon play requires at least one output channel".to_owned());
    }

    let input_stream = if startup.input_channels > 0 {
        let input_device = find_input_device(&host, launch.input_device.as_deref())?;
        let default_input_config = input_device
            .default_input_config()
            .map_err(|err| format!("failed to query default input config: {err}"))?;
        let mut input_config: cpal::StreamConfig = default_input_config.config();
        input_config.channels = default_input_config.channels();
        input_config.sample_rate = cpal::SampleRate(launch.sample_rate_hz);
        input_config.buffer_size = cpal::BufferSize::Fixed(launch.block_frames as u32);
        Some(match default_input_config.sample_format() {
            cpal::SampleFormat::F32 => build_input_stream::<f32>(
                &input_device,
                &input_config,
                startup.input_channels,
                Arc::clone(&input_queue),
                make_input_stream_error_handler(Arc::clone(&error_state)),
            )?,
            cpal::SampleFormat::I16 => build_input_stream::<i16>(
                &input_device,
                &input_config,
                startup.input_channels,
                Arc::clone(&input_queue),
                make_input_stream_error_handler(Arc::clone(&error_state)),
            )?,
            cpal::SampleFormat::U16 => build_input_stream::<u16>(
                &input_device,
                &input_config,
                startup.input_channels,
                Arc::clone(&input_queue),
                make_input_stream_error_handler(Arc::clone(&error_state)),
            )?,
            other => {
                stop_flag.store(true, Ordering::Release);
                let _ = render_thread.join();
                drop(control_server);
                return Err(format!(
                    "unsupported input sample format from audio device: {other:?}"
                ));
            }
        })
    } else {
        None
    };

    wait_for_prefill(
        &sample_queue,
        startup.output_channels * launch.block_frames,
        &stop_flag,
        &render_error,
    )?;

    let stream = match default_output_config.sample_format() {
        cpal::SampleFormat::F32 => build_output_stream::<f32>(
            &output_device,
            &output_config,
            output_device_channels,
            startup.output_channels,
            Arc::clone(&sample_queue),
            make_stream_error_handler(Arc::clone(&error_state)),
        )?,
        cpal::SampleFormat::I16 => build_output_stream::<i16>(
            &output_device,
            &output_config,
            output_device_channels,
            startup.output_channels,
            Arc::clone(&sample_queue),
            make_stream_error_handler(Arc::clone(&error_state)),
        )?,
        cpal::SampleFormat::U16 => build_output_stream::<u16>(
            &output_device,
            &output_config,
            output_device_channels,
            startup.output_channels,
            Arc::clone(&sample_queue),
            make_stream_error_handler(Arc::clone(&error_state)),
        )?,
        other => {
            stop_flag.store(true, Ordering::Release);
            let _ = render_thread.join();
            return Err(format!(
                "unsupported output sample format from audio device: {other:?}"
            ));
        }
    };

    if let Some(input_stream) = input_stream.as_ref() {
        input_stream
            .play()
            .map_err(|err| format!("failed to start audio input stream: {err}"))?;
    }
    stream
        .play()
        .map_err(|err| format!("failed to start audio output stream: {err}"))?;
    if launch.control_json {
        eprintln!(
            "{}",
            playback_status_message(&startup.path, launch.dur_seconds)
        );
    } else {
        println!(
            "{}",
            playback_status_message(&startup.path, launch.dur_seconds)
        );
    }

    wait_for_playback_completion(launch.dur_seconds, &stop_flag, &render_error, &error_state)?;

    stop_flag.store(true, Ordering::Release);
    drop(input_stream);
    drop(stream);
    let _ = render_thread.join();
    drop(control_server);

    if let Some(err) = error_state
        .lock()
        .map_err(|_| "failed to read audio stream error state".to_owned())?
        .clone()
    {
        return Err(err);
    }
    if let Some(err) = render_error
        .lock()
        .map_err(|_| "failed to read render thread error state".to_owned())?
        .clone()
    {
        return Err(err);
    }
    Ok(())
}

fn playback_status_message(path: &Path, dur_seconds: Option<u32>) -> String {
    match dur_seconds {
        Some(dur_seconds) => format!("Playing {} for {} seconds", display_path(path), dur_seconds),
        None => format!("Playing {} until stopped", display_path(path)),
    }
}

fn wait_for_playback_completion(
    dur_seconds: Option<u32>,
    stop_flag: &Arc<AtomicBool>,
    render_error: &Arc<Mutex<Option<String>>>,
    error_state: &Arc<Mutex<Option<String>>>,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        if stop_flag.load(Ordering::Acquire) {
            break;
        }
        if let Some(limit) = dur_seconds {
            if start.elapsed() >= Duration::from_secs(u64::from(limit)) {
                break;
            }
        }
        if let Some(err) = render_error
            .lock()
            .map_err(|_| "failed to read render thread error state".to_owned())?
            .clone()
        {
            return Err(err);
        }
        if let Some(err) = error_state
            .lock()
            .map_err(|_| "failed to read audio stream error state".to_owned())?
            .clone()
        {
            return Err(err);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    device_channels: usize,
    source_channels: usize,
    sample_queue: Arc<SpscSampleRing>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                write_output_data::<T>(data, device_channels, source_channels, &sample_queue)
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build audio output stream: {err}"))
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    target_channels: usize,
    input_queue: Arc<SpscSampleRing>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let device_channels = usize::from(config.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                write_input_data::<T>(data, device_channels, target_channels, &input_queue)
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build audio input stream: {err}"))
}

fn make_stream_error_handler(
    error_state: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        if let Ok(mut slot) = error_state.lock() {
            *slot = Some(format!("audio output stream error: {err}"));
        }
    }
}

fn make_input_stream_error_handler(
    error_state: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        if let Ok(mut slot) = error_state.lock() {
            *slot = Some(format!("audio input stream error: {err}"));
        }
    }
}

fn write_output_data<T>(
    data: &mut [T],
    device_channels: usize,
    source_channels: usize,
    sample_queue: &Arc<SpscSampleRing>,
) where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    for frame in data.chunks_mut(device_channels) {
        if source_channels == 1 {
            let sample = sample_queue.pop_one().unwrap_or(0.0);
            for out in frame.iter_mut() {
                *out = T::from_sample(sample);
            }
            continue;
        }

        for (channel_index, sample) in frame.iter_mut().enumerate() {
            let value = if channel_index < source_channels {
                sample_queue.pop_one().unwrap_or(0.0)
            } else {
                0.0
            };
            *sample = T::from_sample(value);
        }
        for _ in device_channels..source_channels {
            let _ = sample_queue.pop_one();
        }
    }
}

fn write_input_data<T>(
    data: &[T],
    device_channels: usize,
    target_channels: usize,
    input_queue: &Arc<SpscSampleRing>,
) where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    if device_channels == 0 || target_channels == 0 {
        return;
    }
    for frame in data.chunks(device_channels) {
        for sample in frame.iter().take(target_channels).copied() {
            if !input_queue.push_one(f32::from_sample(sample)) {
                return;
            }
        }
    }
}

fn append_interleaved_block(rendered: &mut Vec<f32>, block: &[Vec<f32>]) {
    if block.is_empty() {
        return;
    }
    let frames = block[0].len();
    for frame in 0..frames {
        for channel in block {
            rendered.push(channel[frame]);
        }
    }
}

fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned()
}

fn find_output_device(
    host: &cpal::Host,
    requested_name: Option<&str>,
) -> Result<cpal::Device, String> {
    match requested_name {
        Some(name) => host
            .output_devices()
            .map_err(|err| format!("failed to enumerate output devices: {err}"))?
            .find(|device| {
                device
                    .name()
                    .map(|device_name| device_name == name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("output device '{name}' was not found")),
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output audio device available".to_owned()),
    }
}

fn list_input_devices(host: &cpal::Host) -> Vec<String> {
    host.input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|device| device.name().ok())
        .collect()
}

fn list_output_devices(host: &cpal::Host) -> Vec<String> {
    host.output_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|device| device.name().ok())
        .collect()
}

fn find_input_device(
    host: &cpal::Host,
    requested_name: Option<&str>,
) -> Result<cpal::Device, String> {
    match requested_name {
        Some(name) => host
            .input_devices()
            .map_err(|err| format!("failed to enumerate input devices: {err}"))?
            .find(|device| {
                device
                    .name()
                    .map(|device_name| device_name == name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("input device '{name}' was not found")),
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input audio device available".to_owned()),
    }
}

#[derive(Clone)]
struct PlaybackLaunch {
    input: PathBuf,
    dur_seconds: Option<u32>,
    sample_rate_hz: u32,
    block_frames: usize,
    input_device: Option<String>,
    output_device: Option<String>,
    fast_math: bool,
    show_meta: bool,
    control_json: bool,
    param_sets: Vec<(String, f64)>,
}

struct PlaybackStartup {
    path: PathBuf,
    input_channels: usize,
    output_channels: usize,
    params: Vec<PreviewParamInfo>,
    buffers: Vec<PreviewBufferInfo>,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    current_input_device: Option<String>,
    current_output_device: Option<String>,
}

enum PlaybackControlCommand {
    GetParams {
        reply: mpsc::Sender<Result<Vec<PreviewParamInfo>, String>>,
    },
    GetBuffers {
        reply: mpsc::Sender<Result<Vec<PreviewBufferInfo>, String>>,
    },
    SetParam {
        name: String,
        value: f64,
        reply: Option<mpsc::Sender<Result<(), String>>>,
    },
    BindBufferWav {
        name: String,
        path: PathBuf,
        reply: mpsc::Sender<Result<(), String>>,
    },
    ClearBuffer {
        name: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

struct ScopeSnapshot {
    channels: usize,
    samples: Vec<f32>,
}

struct ScopeRing {
    buffer: Vec<f32>,
    channels: usize,
    write_pos: usize,
    frames_written: usize,
}

struct PendingParamUpdate {
    value: f64,
    replies: Vec<mpsc::Sender<Result<(), String>>>,
}

impl ScopeRing {
    fn new(capacity_frames: usize, channels: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity_frames * channels],
            channels,
            write_pos: 0,
            frames_written: 0,
        }
    }

    fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buffer.len();
        if cap == 0 {
            return;
        }
        for (i, &sample) in samples.iter().enumerate() {
            self.buffer[(self.write_pos + i) % cap] = sample;
        }
        self.write_pos = (self.write_pos + samples.len()) % cap;
        self.frames_written += samples.len() / self.channels.max(1);
    }

    fn snapshot(&self, max_frames: usize) -> ScopeSnapshot {
        let total_frames = self.buffer.len() / self.channels.max(1);
        let available = total_frames.min(self.frames_written);
        let frames = max_frames.min(available);
        let sample_count = frames * self.channels;
        let cap = self.buffer.len();
        let start = (self.write_pos + cap - sample_count) % cap;
        let mut samples = Vec::with_capacity(sample_count);
        for i in 0..sample_count {
            samples.push(self.buffer[(start + i) % cap]);
        }
        ScopeSnapshot {
            channels: self.channels,
            samples,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PlaybackControlRequest {
    #[serde(default)]
    id: Option<Value>,
    command: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default, rename = "maxFrames")]
    max_frames: Option<usize>,
}

fn spawn_preview_render_thread(
    launch: PlaybackLaunch,
    sample_queue: Arc<SpscSampleRing>,
    input_queue: Arc<SpscSampleRing>,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
    render_error: Arc<Mutex<Option<String>>>,
    startup_tx: mpsc::Sender<Result<PlaybackStartup, String>>,
    control_rx: Option<mpsc::Receiver<PlaybackControlCommand>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let control_rx = control_rx;
        let mut session = DaemonSession::new(DaemonConfig {
            analysis: AnalysisOptions {
                sample_rate: launch.sample_rate_hz as f32,
                block_size: launch.block_frames,
            },
            preview: PreviewOptions {
                sample_rate: launch.sample_rate_hz as f32,
                block_size: launch.block_frames,
                fast_math: launch.fast_math,
                ..PreviewOptions::default()
            },
        });

        let startup = (|| -> Result<PlaybackStartup, String> {
            session
                .start_preview(&launch.input)
                .map_err(|err| format_preview_build_error("daemon play start failed", &err))?;

            let preview = session
                .preview(&launch.input)
                .expect("preview should be active after successful start");
            let params = if launch.show_meta || launch.control_json {
                preview.param_info()
            } else {
                Vec::new()
            };
            let buffers = if launch.control_json {
                preview.buffer_info()
            } else {
                Vec::new()
            };
            let input_devices = if launch.control_json {
                list_input_devices(&cpal::default_host())
            } else {
                Vec::new()
            };
            let output_devices = if launch.control_json {
                list_output_devices(&cpal::default_host())
            } else {
                Vec::new()
            };
            let input_channels = preview.input_channel_count();
            let output_channels = preview.output_channel_count();
            let path = preview.path().to_path_buf();

            for (name, value) in &launch.param_sets {
                session
                    .preview_mut(&launch.input)
                    .expect("preview should be active while applying params")
                    .set_param_f64(name, *value)
                    .map_err(|diag| format_single_diagnostic("daemon play param failed", &diag))?;
            }

            Ok(PlaybackStartup {
                path,
                input_channels,
                output_channels,
                params,
                buffers,
                input_devices,
                output_devices,
                current_input_device: launch.input_device.clone(),
                current_output_device: launch.output_device.clone(),
            })
        })();

        let render_input_channels = match startup {
            Ok(ref startup) => {
                {
                    const SCOPE_CAPACITY_FRAMES: usize = 4096;
                    let mut ring = scope_ring.lock().unwrap();
                    *ring = ScopeRing::new(SCOPE_CAPACITY_FRAMES, startup.output_channels);
                }
                if startup_tx
                    .send(Ok(PlaybackStartup {
                        path: startup.path.clone(),
                        input_channels: startup.input_channels,
                        output_channels: startup.output_channels,
                        params: startup.params.clone(),
                        buffers: startup.buffers.clone(),
                        input_devices: startup.input_devices.clone(),
                        output_devices: startup.output_devices.clone(),
                        current_input_device: startup.current_input_device.clone(),
                        current_output_device: startup.current_output_device.clone(),
                    }))
                    .is_err()
                {
                    return;
                }
                startup.input_channels
            }
            Err(err) => {
                let _ = startup_tx.send(Err(err));
                return;
            }
        };

        while !stop_flag.load(Ordering::Acquire) {
            if let Some(control_rx) = &control_rx {
                let mut pending_param_updates = HashMap::<String, PendingParamUpdate>::new();
                for _ in 0..MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK {
                    let Ok(command) = control_rx.try_recv() else {
                        break;
                    };
                    match command {
                        PlaybackControlCommand::GetParams { reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .preview(&launch.input)
                                .map(|preview| preview.param_info())
                                .ok_or_else(|| "preview is not active".to_owned());
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::GetBuffers { reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .preview(&launch.input)
                                .map(|preview| preview.buffer_info())
                                .ok_or_else(|| "preview is not active".to_owned());
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::SetParam { name, value, reply } => {
                            let entry = pending_param_updates.entry(name).or_insert_with(|| {
                                PendingParamUpdate {
                                    value,
                                    replies: Vec::new(),
                                }
                            });
                            entry.value = value;
                            if let Some(reply) = reply {
                                entry.replies.push(reply);
                            }
                        }
                        PlaybackControlCommand::BindBufferWav { name, path, reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .preview_mut(&launch.input)
                                .ok_or_else(|| "preview is not active".to_owned())
                                .and_then(|preview| {
                                    preview.bind_buffer_wav_path(&name, &path).map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play bind buffer failed",
                                            &diag,
                                        )
                                    })
                                });
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::ClearBuffer { name, reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .preview_mut(&launch.input)
                                .ok_or_else(|| "preview is not active".to_owned())
                                .and_then(|preview| {
                                    preview.clear_buffer(&name).map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play clear buffer failed",
                                            &diag,
                                        )
                                    })
                                });
                            let _ = reply.send(result);
                        }
                    }
                }
                flush_pending_param_updates(
                    &mut pending_param_updates,
                    &mut session,
                    &launch.input,
                );
            }

            if render_input_channels > 0 {
                let input_channels = render_input_channels;
                let input_samples = launch.block_frames * input_channels;
                let mut captured = vec![0.0_f32; input_samples];
                for sample in &mut captured {
                    *sample = input_queue.pop_one().unwrap_or(0.0);
                }
                if let Some(preview) = session.preview_mut(&launch.input) {
                    preview.set_input_block(&captured, input_channels);
                }
            }

            let block = match session.render_preview_block(&launch.input) {
                Ok(block) => block,
                Err(diag) => {
                    store_thread_error(
                        &render_error,
                        format_single_diagnostic("daemon play render failed", &diag),
                    );
                    stop_flag.store(true, Ordering::Release);
                    break;
                }
            };

            let mut interleaved = Vec::with_capacity(
                block.len() * block.first().map(Vec::len).unwrap_or(launch.block_frames),
            );
            append_interleaved_block(&mut interleaved, &block);

            if let Ok(mut ring) = scope_ring.try_lock() {
                ring.push_interleaved(&interleaved);
            }

            let mut offset = 0;
            while offset < interleaved.len() && !stop_flag.load(Ordering::Acquire) {
                let written = sample_queue.push_slice(&interleaved[offset..]);
                if written == 0 {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                offset += written;
            }
        }
    })
}

fn flush_pending_param_updates(
    pending: &mut HashMap<String, PendingParamUpdate>,
    session: &mut DaemonSession,
    input: &Path,
) {
    for (name, update) in std::mem::take(pending) {
        let result = session
            .preview_mut(input)
            .ok_or_else(|| "preview is not active".to_owned())
            .and_then(|preview| {
                preview
                    .set_param_f64(&name, update.value)
                    .map_err(|diag| format_single_diagnostic("daemon play param failed", &diag))
            });
        for reply in update.replies {
            let _ = reply.send(result.clone());
        }
    }
}

fn wait_for_prefill(
    sample_queue: &Arc<SpscSampleRing>,
    min_samples: usize,
    stop_flag: &Arc<AtomicBool>,
    render_error: &Arc<Mutex<Option<String>>>,
) -> Result<(), String> {
    while sample_queue.len() < min_samples && !stop_flag.load(Ordering::Acquire) {
        if let Some(err) = render_error
            .lock()
            .map_err(|_| "failed to read render thread error state".to_owned())?
            .clone()
        {
            return Err(err);
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn store_thread_error(slot: &Arc<Mutex<Option<String>>>, message: String) {
    if let Ok(mut slot) = slot.lock() {
        if slot.is_none() {
            *slot = Some(message);
        }
    }
}

fn preview_param_json(param: &PreviewParamInfo) -> Value {
    json!({
        "index": param.index,
        "name": param.name,
        "type": param.type_repr,
        "value": param.value,
        "default": param.default,
        "rangeMin": param.range_min,
        "rangeMax": param.range_max,
        "scalar": param.scalar,
    })
}

fn preview_buffer_json(buffer: &PreviewBufferInfo) -> Value {
    let (channels_kind, channels_static) = match buffer.channels {
        PreviewBufferChannels::Mono => ("mono", None),
        PreviewBufferChannels::Static(channels) => ("static", Some(channels)),
        PreviewBufferChannels::Dynamic => ("dynamic", None),
    };
    json!({
        "index": buffer.index,
        "name": buffer.name,
        "type": buffer.type_repr,
        "channelsKind": channels_kind,
        "channelsStatic": channels_static,
        "loadedPath": buffer.loaded_path,
    })
}

fn write_json_line(writer: &mut impl Write, value: &Value) -> Result<(), std::io::Error> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn spawn_preview_control_server(
    listener: TcpListener,
    control_tx: mpsc::Sender<PlaybackControlCommand>,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }

        while !stop_flag.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(err) =
                        handle_preview_control_client(stream, &control_tx, &scope_ring, &stop_flag)
                    {
                        eprintln!("preview control client error: {err}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    eprintln!("preview control accept error: {err}");
                    break;
                }
            }
        }
    })
}

fn handle_preview_control_client(
    stream: TcpStream,
    control_tx: &mpsc::Sender<PlaybackControlCommand>,
    scope_ring: &Arc<Mutex<ScopeRing>>,
    stop_flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|err| format!("failed to set control socket read timeout: {err}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|err| format!("failed to clone control socket: {err}"))?;
    let mut reader = BufReader::new(reader_stream);
    let mut writer = BufWriter::new(stream);

    while !stop_flag.load(Ordering::Acquire) {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(err) => return Err(format!("failed to read control request: {err}")),
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: PlaybackControlRequest = serde_json::from_str(trimmed)
            .map_err(|err| format!("invalid control request json: {err}"))?;
        let response = preview_control_response(request, control_tx, scope_ring);
        if let Some(response) = response {
            write_json_line(&mut writer, &response)
                .map_err(|err| format!("failed to write control response: {err}"))?;
        }
    }

    Ok(())
}

fn preview_control_response(
    request: PlaybackControlRequest,
    control_tx: &mpsc::Sender<PlaybackControlCommand>,
    scope_ring: &Arc<Mutex<ScopeRing>>,
) -> Option<Value> {
    let request_id = request.id;
    let result = match request.command.as_str() {
        "getParams" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetParams { reply: reply_tx })
                .map_err(|_| "preview control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "preview control reply channel closed".to_owned())
                })
                .map(|result| Some(match result {
                    Ok(params) => json!({
                        "id": request_id,
                        "ok": true,
                        "result": {
                            "params": params.iter().map(preview_param_json).collect::<Vec<_>>(),
                        }
                    }),
                    Err(err) => json!({
                        "id": request_id,
                        "ok": false,
                        "error": err,
                    }),
                }))
        }
        "getBuffers" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetBuffers { reply: reply_tx })
                .map_err(|_| "preview control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "preview control reply channel closed".to_owned())
                })
                .map(|result| Some(match result {
                    Ok(buffers) => json!({
                        "id": request_id,
                        "ok": true,
                        "result": {
                            "buffers": buffers.iter().map(preview_buffer_json).collect::<Vec<_>>(),
                        }
                    }),
                    Err(err) => json!({
                        "id": request_id,
                        "ok": false,
                        "error": err,
                    }),
                }))
        }
        "setParam" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "setParam requires 'name'".to_owned())?;
                let raw_value = request
                    .value
                    .ok_or_else(|| "setParam requires 'value'".to_owned())?;
                let value = match raw_value {
                    Value::Bool(value) => {
                        if value {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Number(value) => value
                        .as_f64()
                        .ok_or_else(|| "setParam value must be numeric".to_owned())?,
                    _ => return Err("setParam value must be number or boolean".to_owned()),
                };
                if request_id.is_none() {
                    control_tx
                        .send(PlaybackControlCommand::SetParam {
                            name,
                            value,
                            reply: None,
                        })
                        .map_err(|_| "preview control channel closed".to_owned())?;
                    return Ok(None);
                }
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::SetParam {
                        name,
                        value,
                        reply: Some(reply_tx),
                    })
                    .map_err(|_| "preview control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "preview control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| json!({
                        "id": id,
                        "ok": true,
                    }))),
                    Err(err) => Ok(request_id.clone().map(|id| json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    }))),
                }
            })();
            result
        }
        "bindBufferWav" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "bindBufferWav requires 'name'".to_owned())?;
                let path = request
                    .path
                    .ok_or_else(|| "bindBufferWav requires 'path'".to_owned())?;
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::BindBufferWav {
                        name,
                        path: PathBuf::from(path),
                        reply: reply_tx,
                    })
                    .map_err(|_| "preview control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "preview control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| json!({
                        "id": id,
                        "ok": true,
                    }))),
                    Err(err) => Ok(request_id.clone().map(|id| json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    }))),
                }
            })();
            result
        }
        "getScopeData" => scope_ring
            .lock()
            .map_err(|_| "failed to lock scope ring".to_owned())
            .map(|ring| {
                let snapshot = ring.snapshot(request.max_frames.unwrap_or(2048));
                Some(json!({
                "id": request_id,
                "ok": true,
                "result": {
                    "channels": snapshot.channels,
                    "samples": snapshot.samples,
                }
            }))
            }),
        "clearBuffer" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "clearBuffer requires 'name'".to_owned())?;
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::ClearBuffer {
                        name,
                        reply: reply_tx,
                    })
                    .map_err(|_| "preview control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "preview control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| json!({
                        "id": id,
                        "ok": true,
                    }))),
                    Err(err) => Ok(request_id.clone().map(|id| json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    }))),
                }
            })();
            result
        }
        other => Err(format!("unknown command '{other}'")),
    };

    match result {
        Ok(value) => value,
        Err(err) => request_id.map(|id| json!({
            "id": id,
            "ok": false,
            "error": err,
        })),
    }
}

struct SpscSampleRing {
    capacity: usize,
    mask: usize,
    slots: Box<[UnsafeCell<f32>]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
}

unsafe impl Send for SpscSampleRing {}
unsafe impl Sync for SpscSampleRing {}

impl SpscSampleRing {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        let slots = std::iter::repeat_with(|| UnsafeCell::new(0.0))
            .take(capacity)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            capacity,
            mask: capacity - 1,
            slots,
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
        }
    }

    fn len(&self) -> usize {
        let write = self.write_index.load(Ordering::Acquire);
        let read = self.read_index.load(Ordering::Acquire);
        write.saturating_sub(read)
    }

    fn push_slice(&self, input: &[f32]) -> usize {
        let write = self.write_index.load(Ordering::Relaxed);
        let read = self.read_index.load(Ordering::Acquire);
        let available = self.capacity.saturating_sub(write.saturating_sub(read));
        let count = input.len().min(available);
        for (offset, sample) in input.iter().copied().take(count).enumerate() {
            let index = (write + offset) & self.mask;
            // SAFETY: producer is single-writer and only writes slots not yet published via write_index.
            unsafe { *self.slots[index].get() = sample };
        }
        if count != 0 {
            self.write_index.store(write + count, Ordering::Release);
        }
        count
    }

    fn push_one(&self, sample: f32) -> bool {
        let write = self.write_index.load(Ordering::Relaxed);
        let read = self.read_index.load(Ordering::Acquire);
        if self.capacity.saturating_sub(write.saturating_sub(read)) == 0 {
            return false;
        }
        let index = write & self.mask;
        // SAFETY: producer is single-writer and only writes slots not yet published via write_index.
        unsafe { *self.slots[index].get() = sample };
        self.write_index.store(write + 1, Ordering::Release);
        true
    }

    fn pop_one(&self) -> Option<f32> {
        let read = self.read_index.load(Ordering::Relaxed);
        let write = self.write_index.load(Ordering::Acquire);
        if read == write {
            return None;
        }
        let index = read & self.mask;
        // SAFETY: consumer is single-reader and only reads slots published via write_index.
        let sample = unsafe { *self.slots[index].get() };
        self.read_index.store(read + 1, Ordering::Release);
        Some(sample)
    }
}

fn format_preview_build_error(context: &str, err: &PreviewBuildError) -> String {
    match err {
        PreviewBuildError::Diagnostics(diags) => format_diagnostics(context, diags),
        PreviewBuildError::Runtime(diag) => format_single_diagnostic(context, diag),
    }
}

fn format_preview_param_info(params: &[PreviewParamInfo]) -> String {
    let mut lines = Vec::with_capacity(params.len() + 1);
    lines.push("Preview params:".to_owned());
    for param in params {
        let range = match (param.range_min, param.range_max) {
            (Some(min), Some(max)) => format!(" [{min}, {max}]"),
            (None, Some(max)) => format!(" [.., {max}]"),
            _ => String::new(),
        };
        let default = param
            .default
            .map(|value| format!(" = {value}"))
            .unwrap_or_default();
        let scalar = if param.scalar { "" } else { " (non-scalar)" };
        lines.push(format!(
            "  {}: {}{}{}{}",
            param.name, param.type_repr, default, range, scalar
        ));
    }
    lines.join("\n")
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
                text.push_str(&format_graph_destinations(&edge.dests));
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
        AssignTarget::Tuple(names) => format!("({})", names.join(", ")),
    }
}

fn format_graph_endpoint(endpoint: &omni_frontend::GraphEndpoint) -> String {
    match endpoint {
        omni_frontend::GraphEndpoint::Symbol { name, .. } => name.clone(),
        omni_frontend::GraphEndpoint::ProcField { proc, field, .. } => {
            format!("{proc}.{field}")
        }
        omni_frontend::GraphEndpoint::ProcIndexedField {
            proc, index, field, ..
        } => {
            format!("{proc}[{}].{field}", format_expr(index))
        }
    }
}

fn format_graph_destinations(dests: &[omni_frontend::GraphEndpoint]) -> String {
    match dests {
        [] => String::new(),
        [dest] => format_graph_endpoint(dest),
        _ => format!(
            "{{ {} }}",
            dests
                .iter()
                .map(format_graph_endpoint)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_expr(expr: &Expr) -> String {
    format_expr_prec(expr, 0)
}

fn format_expr_prec(expr: &Expr, parent_prec: u8) -> String {
    let my_prec = expr_precedence(expr);
    match expr {
        Expr::Number { value, .. } => format_number(*value),
        Expr::Int { value, .. } => value.to_string(),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::ArrayLiteral { values, .. } => format!(
            "[{}]",
            values
                .iter()
                .map(format_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Var { name, .. } => name.clone(),
        Expr::Index { base, index, .. } => format!("{base}[{}]", format_expr(index)),
        Expr::Slice {
            base, start, end, ..
        } => format!(
            "{base}[{}:{}]",
            start
                .as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default(),
            end.as_ref()
                .map(|expr| format_expr(expr))
                .unwrap_or_default()
        ),
        Expr::ArrayCtor { spec, init, .. } => {
            let mut text = format!("{}(", format_array_type_spec(spec));
            if let Some(init) = init {
                text.push_str(&init.iter().map(format_expr).collect::<Vec<_>>().join(", "));
            }
            text.push(')');
            text
        }
        Expr::Compare { op, lhs, rhs, .. } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_cmp_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Call { func, args, .. } => format!(
            "{}({})",
            format_builtin_fn(*func),
            args.iter().map(format_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
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
        Expr::Cast { to, expr, .. } => {
            format!("{}({})", primitive_type_name(*to), format_expr(expr))
        }
        Expr::UnaryNot { expr, .. } => wrap_if_needed(
            format!("!{}", format_expr_prec(expr, my_prec)),
            my_prec,
            parent_prec,
        ),
        Expr::UnaryBitNot { expr, .. } => wrap_if_needed(
            format!("~{}", format_expr_prec(expr, my_prec)),
            my_prec,
            parent_prec,
        ),
        Expr::Logical { op, lhs, rhs, .. } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_logical_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Binary { op, lhs, rhs, .. } => wrap_if_needed(
            format!(
                "{} {} {}",
                format_expr_prec(lhs, my_prec),
                format_binary_op(*op),
                format_expr_prec(rhs, my_prec + 1)
            ),
            my_prec,
            parent_prec,
        ),
        Expr::Tuple { values, .. } => format!(
            "({})",
            values
                .iter()
                .map(format_expr)
                .collect::<Vec<_>>()
                .join(", ")
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
        DeclType::Tuple(elems) => format!(
            "({})",
            elems
                .iter()
                .map(|t| primitive_type_name(*t).to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::Scalar(ty) => primitive_type_name(*ty).to_owned(),
        FieldType::Generic(name) => name.clone(),
        FieldType::Array(spec) => format_array_type_spec(spec),
        FieldType::Tuple(elem_tys) => {
            let elems: Vec<String> = elem_tys
                .iter()
                .map(|ty| primitive_type_name(*ty).to_owned())
                .collect();
            format!("({})", elems.join(", "))
        }
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
        omni_frontend::FnParamType::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(|p| primitive_type_name(*p))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
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
    let start_col = diag.column.max(1);
    let underline_len = if diag.end_line == diag.line && diag.end_column > start_col {
        diag.end_column.saturating_sub(start_col)
    } else {
        1
    };
    let caret_pad = " ".repeat(start_col.saturating_sub(1));
    let underline = "^".repeat(underline_len.max(1));
    Some(format!(
        "  --> {file}:{}:{}\n   |\n{:>4} | {}\n   | {}{}",
        diag.line, start_col, diag.line, line_text, caret_pad, underline
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
    use super::{
        format_diag_snippet, format_expr, format_program, parse_args, preview_control_response,
        Command, DaemonCommand, PlaybackControlCommand, PlaybackControlRequest, PreviewCommand,
        ScopeRing, DEFAULT_PLAY_BLOCK_FRAMES,
    };
    use omni_frontend::{
        Block, CallArg, Diagnostic, Expr, GraphBlock, GraphEdge, GraphEndpoint, Program,
    };
    use serde_json::Value;
    use std::sync::{mpsc, Arc, Mutex};

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
    fn parse_daemon_diagnose_accepts_block_and_sample_rate() {
        let cmd = parse_args(
            [
                "omni", "daemon", "diagnose", "x.omni", "--block", "256", "--sr", "44100",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("daemon diagnose args should parse");
        match cmd {
            Command::Daemon(DaemonCommand::Diagnose {
                block_frames,
                sample_rate_hz,
                ..
            }) => {
                assert_eq!(block_frames, 256);
                assert_eq!(sample_rate_hz, 44_100);
            }
            _ => panic!("expected daemon diagnose command"),
        }
    }

    #[test]
    fn parse_daemon_stdio_command() {
        let cmd = parse_args(["omni", "daemon", "stdio"].into_iter().map(str::to_owned))
            .expect("daemon stdio args should parse");
        match cmd {
            Command::Daemon(DaemonCommand::Stdio) => {}
            _ => panic!("expected daemon stdio command"),
        }
    }

    #[test]
    fn parse_lsp_command() {
        let cmd = parse_args(["omni", "lsp"].into_iter().map(str::to_owned))
            .expect("lsp args should parse");
        match cmd {
            Command::Lsp => {}
            _ => panic!("expected lsp command"),
        }
    }

    #[test]
    fn parse_lsp_command_accepts_stdio_flag() {
        let cmd = parse_args(["omni", "lsp", "--stdio"].into_iter().map(str::to_owned))
            .expect("lsp --stdio args should parse");
        match cmd {
            Command::Lsp => {}
            _ => panic!("expected lsp command"),
        }
    }

    #[test]
    fn parse_preview_play_accepts_meta_and_param_sets() {
        let cmd = parse_args(
            [
                "omni",
                "preview",
                "play",
                "x.omni",
                "--meta",
                "--control-json",
                "--set",
                "gain=0.5",
                "--fast-math",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("preview play args should parse");
        match cmd {
            Command::Preview(PreviewCommand::Play {
                show_meta,
                control_json,
                fast_math,
                param_sets,
                ..
            }) => {
                assert!(show_meta);
                assert!(control_json);
                assert!(fast_math);
                assert_eq!(param_sets, vec![("gain".to_owned(), 0.5)]);
            }
            _ => panic!("expected preview play command"),
        }
    }

    #[test]
    fn parse_preview_play_accepts_forever() {
        let cmd = parse_args(
            ["omni", "preview", "play", "x.omni", "--forever"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("preview play --forever should parse");
        match cmd {
            Command::Preview(PreviewCommand::Play {
                dur_seconds,
                block_frames,
                ..
            }) => {
                assert_eq!(dur_seconds, None);
                assert_eq!(block_frames, DEFAULT_PLAY_BLOCK_FRAMES);
            }
            _ => panic!("expected preview play command"),
        }
    }

    #[test]
    fn parse_preview_render_accepts_meta_and_param_sets() {
        let cmd = parse_args(
            [
                "omni",
                "preview",
                "render",
                "x.omni",
                "--meta",
                "--set",
                "gain=0.5",
                "--fast-math",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("preview render args should parse");
        match cmd {
            Command::Preview(PreviewCommand::Render {
                show_meta,
                fast_math,
                param_sets,
                ..
            }) => {
                assert!(show_meta);
                assert!(fast_math);
                assert_eq!(param_sets, vec![("gain".to_owned(), 0.5)]);
            }
            _ => panic!("expected preview render command"),
        }
    }

    #[test]
    fn parse_daemon_play_reports_rename() {
        let err = match parse_args(
            ["omni", "daemon", "play", "x.omni"]
                .into_iter()
                .map(str::to_owned),
        ) {
            Ok(_) => panic!("daemon play should report rename"),
            Err(err) => err,
        };
        assert!(err.contains("omni preview play"));
    }

    #[test]
    fn format_expr_prints_named_call_args_with_equals() {
        let expr = Expr::UserCall {
            loc: Default::default(),
            name: "sat".to_owned(),
            type_args: Vec::new(),
            args: vec![CallArg {
                name: Some("in1".to_owned()),
                expr: Expr::var("mix.out1"),
            }],
        };
        assert_eq!(format_expr(&expr), "sat(in1 = mix.out1)");
    }

    #[test]
    fn format_program_prints_graph_fanout_destinations() {
        let program = Program {
            blocks: vec![Block::Graph(GraphBlock {
                loc: Default::default(),
                edges: vec![GraphEdge {
                    loc: Default::default(),
                    rate: None,
                    source: Expr::var("src"),
                    delay: None,
                    dests: vec![
                        GraphEndpoint::Symbol {
                            loc: Default::default(),
                            name: "out1".to_owned(),
                        },
                        GraphEndpoint::ProcField {
                            loc: Default::default(),
                            proc: "mix".to_owned(),
                            field: "in1".to_owned(),
                        },
                    ],
                }],
            })],
        };

        assert_eq!(
            format_program(&program),
            "graph:\n  src >> { out1, mix.in1 }\n\n"
        );
    }

    #[test]
    fn format_diag_snippet_underlines_same_line_ranges() {
        let dir = std::env::temp_dir();
        let path = dir.join("omni_cli_diag_range_test.omni");
        std::fs::write(&path, "sample:\n  out1 = missing + 1.0\n").expect("write test source");

        let diag = Diagnostic {
            code: omni_frontend::DiagCode::Semantic,
            message: "unknown symbol 'missing'".to_owned(),
            line: 2,
            column: 10,
            end_line: 2,
            end_column: 17,
            file: Some(path.to_string_lossy().into_owned()),
            trace: Vec::new(),
        };

        let snippet = format_diag_snippet(&diag).expect("snippet should render");
        assert!(snippet.contains("  2 |   out1 = missing + 1.0"));
        assert!(snippet.contains("   |          ^^^^^^^"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_set_param_notification_enqueues_without_waiting_for_reply() {
        let (control_tx, control_rx) = mpsc::channel();
        let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
        let response = preview_control_response(
            PlaybackControlRequest {
                id: None,
                command: "setParam".to_owned(),
                name: Some("gain".to_owned()),
                path: None,
                value: Some(Value::from(0.5)),
                max_frames: None,
            },
            &control_tx,
            &scope_ring,
        );

        assert!(response.is_none());
        match control_rx.try_recv().expect("setParam should be queued") {
            PlaybackControlCommand::SetParam { name, value, reply } => {
                assert_eq!(name, "gain");
                assert_eq!(value, 0.5);
                assert!(reply.is_none());
            }
            _ => panic!("expected setParam command"),
        }
    }
}
