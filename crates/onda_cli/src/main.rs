use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::LazyLock;

mod daemon_stdio;
mod lsp_stdio;

use onda_codegen_llvm::{
    lower_to_object_with_options, lower_to_target_llvm_ir_with_options, CodegenOptions,
    TargetCodeModel, TargetConfig, TargetCpu, TargetOptLevel, TargetRelocMode,
};
use onda_daemon::{DaemonConfig, DaemonSession, RunBuildError, RunOptions};
use onda_frontend::{
    parse_program_file, ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec,
    BufferChannels, BufferElemType, BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp, DeclType,
    Diagnostic, EventParamType, Expr, FieldType, FunctionDef, InitBlock, LogicalOp, PrimitiveType,
    ProcessorDef, Program, SampleBlock, Stmt, StructDef,
};
use onda_run::{
    append_interleaved_block, format_run_param_info, play_run_realtime, PlaybackLaunch,
    RunHostOptions, RunThemeMode,
};
use onda_semantics::{
    analyze_with_options, lower_graphs_for_inspection_with_options, AnalysisOptions,
    TypedArrayInfo, TypedProgram,
};

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_DUR_SECONDS: u32 = 5;
const DEFAULT_BLOCK_FRAMES: usize = 512;
const DEFAULT_PLAY_BLOCK_FRAMES: usize = 128;
const DEFAULT_DAEMON_OUTPUT: &str = "./onda_daemon_out.wav";
const ONDA_VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE_BODY: &str = r#"Commands:
  
  onda compile <input.onda>          Check, inspect, or emit compile artifacts
    
    [--emit <check|llvm-ir|obj>] [--output <path>] [--meta-out <path>]
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math]  
    [--dump-graph] [--ir] [--meta] 
    [--target-spec <path>] [--target-features <feature-list>] 
    [--target-triple <triple>] [--target-cpu <name|host>] [--target-abi <name>] 
    [--reloc-model <default|static|pic|dynamic-no-pic>] 
    [--code-model <default|small|kernel|medium|large>] 
  
  onda run <input.onda>              Open the interactive run window
    
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math] 
    [--input-device <name>] [--output-device <name>]
    [--theme <auto|dark|light>] [--webview] [--meta]
  
  onda run play <input.onda>         Run realtime playback without the UI
    
    [--dur <seconds> | --forever]
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math]
    [--input-device <name>] [--output-device <name>]
    [--set <name=value>] [--control-json] [--meta]
  
  onda run render <input.onda>       Render offline through the run pipeline
    
    [--output <path>] [--dur <seconds>]
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math]
    [--set <name=value>] [--meta] 
  
  onda daemon diagnose <input.onda>  Run daemon-backed analysis and diagnostics
    
    [--sample-rate <hz>] [--block-size <frames>]

  onda daemon stdio                  Start the daemon transport over stdio
  
  onda lsp                           Start the language server over stdio
  
Shared Options:
  
  --sample-rate, --sr    Sample rate in Hz (default: 48000)
  --block-size, -b       Block size in frames (default: 512; run play: 128)
  --opt-level            LLVM optimization level (default: 3)
  --fast-math            Enable LLVM fast-math flags for floating-point operations
  --meta                 Print available metadata for the selected command
  --help, -h             Show this help

Compile Options:
  
  --emit                 Compile artifact for `onda compile`: `check`, `llvm-ir`, or `obj`
  --output, -o           Output path for `llvm-ir` or `obj`
  --meta-out             Write AOT sidecar metadata JSON for `onda compile --emit obj`
  --dump-graph           Print program after graph lowering, before proc desugaring/codegen
  --ir                   Alias for `onda compile --emit llvm-ir`
  --target-triple        LLVM target triple for compile-time IR emission
  --target-spec          TOML target spec for compile-time IR/object emission
  --target-cpu           Target CPU name, or 'host' for the native host CPU
  --target-features      Comma-separated LLVM target feature string
  --target-abi           Optional target ABI name forwarded to LLVM target machine creation
  --reloc-model          Target relocation model for compile-time IR emission
  --code-model           Target code model for compile-time IR emission

Run Options:
  
  --dur, -d              Render/play duration in seconds (default: 5)
  --forever              Render/play with infinite duration
  --output, -o           Output WAV path for `onda run render`
  --input-device         Select audio input device by exact name for run playback
  --output-device        Select audio output device by exact name for run playback
  --set                  Override a scalar run param with `name=value`
  --control-json         Emit run control handshake on stdout and serve param control over localhost
  --theme                Run window theme: `auto`, `dark`, or `light` (default: auto)
  --webview              Use the webview run host instead of egui
"#;

#[allow(dead_code)]
const USAGE_BANNER: &[&str] = &[
    ":-====-:",
    ":+#@@@@@@@@@@@@#+:",
    "+@@@%+=-:.  .:-=+@@@@+",
    "=@@@*.              .*@@@=",
    ".#@@-                    -@@#.",
    ".@@#                        #@@.",
    "@@#                          #@@",
    "+@@        -#@#-               @@+",
    "%@*      =@@*-*@#              *@%",
    "@@=    =@@#.   *@#       +=:   =@@",
    "@@=    +=:      #@*   .@@#=    =@@",
    "%@#              %@*-*@@=      #@#",
    "-@@.              -#@#-       .@@-",
    "%@%                          %@#",
    "@@#                        #@%",
    ".#@@-                    -@@#",
    "=@@@*.              .*@@@=",
    "+@@@%+=-:.  .:-=+@@@@+",
    ":+#@@@@@@@@@@@@#+:",
    ":-====-:",
];

static USAGE: LazyLock<String> = LazyLock::new(build_usage);

fn usage() -> &'static str {
    USAGE.as_str()
}

fn build_usage() -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!("~ onda {ONDA_VERSION} ~\n"));
    out.push('\n');
    out.push_str(USAGE_BODY);
    out
}

enum Command {
    Compile {
        input: PathBuf,
        emit: CompileEmit,
        output: Option<PathBuf>,
        meta_out: Option<PathBuf>,
        sample_rate_hz: u32,
        block_frames: usize,
        dump_graph: bool,
        show_meta: bool,
        fast_math: bool,
        target: TargetConfig,
    },
    Lsp,
    Run(RunCommand),
    Daemon(DaemonCommand),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CompileEmit {
    Check,
    LlvmIr,
    Object,
}

enum RunCommand {
    Play {
        input: PathBuf,
        dur_seconds: Option<u32>,
        sample_rate_hz: u32,
        block_frames: usize,
        opt_level: TargetOptLevel,
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
        opt_level: TargetOptLevel,
        fast_math: bool,
        show_meta: bool,
        param_sets: Vec<(String, f64)>,
    },
    Window {
        input: PathBuf,
        sample_rate_hz: u32,
        block_frames: usize,
        opt_level: TargetOptLevel,
        input_device: Option<String>,
        output_device: Option<String>,
        fast_math: bool,
        show_meta: bool,
        theme: RunThemeMode,
        host: RunHostKind,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum RunHostKind {
    Auto,
    Egui,
    Webview,
}

enum DaemonCommand {
    Stdio,
    Diagnose {
        input: PathBuf,
        sample_rate_hz: u32,
        block_frames: usize,
    },
}

#[derive(Debug, Clone)]
struct LoadedTargetSpec {
    target: TargetConfig,
    cpu_explicit: bool,
    features_explicit: bool,
}

impl Default for LoadedTargetSpec {
    fn default() -> Self {
        Self {
            target: TargetConfig::host(),
            cpu_explicit: false,
            features_explicit: false,
        }
    }
}

#[derive(Debug, Clone)]
enum TargetSpecValue {
    String(String),
    Integer(i64),
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
            emit,
            output,
            meta_out,
            sample_rate_hz,
            block_frames,
            dump_graph,
            show_meta,
            fast_math,
            target,
        } => run_compile(
            &input,
            emit,
            output.as_deref(),
            meta_out.as_deref(),
            sample_rate_hz,
            block_frames,
            dump_graph,
            show_meta,
            fast_math,
            target,
        ),
        Command::Lsp => lsp_stdio::run_stdio_loop(),
        Command::Run(cmd) => run_run(cmd),
        Command::Daemon(cmd) => run_daemon(cmd),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn run_run_host(host: RunHostKind, input: &Path, options: RunHostOptions) -> Result<(), String> {
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
fn run_webview_run(input: &Path, options: RunHostOptions) -> Result<(), String> {
    onda_webview::run_run_window(input, options)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn run_webview_run(_input: &Path, _options: RunHostOptions) -> Result<(), String> {
    Err("webview run host is unavailable on this platform/build".to_owned())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.skip(1);
    let Some(cmd) = args.next() else {
        return Err(usage().to_owned());
    };
    if cmd == "--help" || cmd == "-h" || cmd == "help" {
        return Err(usage().to_owned());
    }

    match cmd.as_str() {
        "compile" => parse_compile_args(args),
        "lsp" => parse_lsp_args(args),
        "run" => parse_run_args(args),
        "daemon" => parse_daemon_args(args),
        _ => Err(format!("unknown command '{cmd}'\n{}", usage())),
    }
}

fn parse_lsp_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    for arg in args {
        match arg.as_str() {
            "--stdio" => {}
            "--help" | "-h" => return Err(usage().to_owned()),
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }
    Ok(Command::Lsp)
}

fn parse_run_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(subcommand) = args.next() else {
        return Err(format!(
            "run requires a subcommand or input file\n\n{}",
            usage()
        ));
    };
    let run = match subcommand.as_str() {
        "play" => parse_run_play_args(args)?,
        "render" => parse_run_render_args(args)?,
        _ => {
            // Treat as `onda run <file.onda>` — the windowed run host.
            if subcommand.starts_with('-') {
                return Err(format!("unknown run option '{subcommand}'\n\n{}", usage()));
            }
            parse_run_window_args(subcommand, args)?
        }
    };
    Ok(Command::Run(run))
}

fn parse_daemon_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(subcommand) = args.next() else {
        return Err(format!("daemon requires a subcommand\n\n{}", usage()));
    };
    let daemon = match subcommand.as_str() {
        "stdio" => DaemonCommand::Stdio,
        "diagnose" => parse_daemon_diagnose_args(args)?,
        "play" => {
            return Err(format!(
                "daemon play was renamed to 'onda run play'\n\n{}",
                usage()
            ))
        }
        "run" => {
            return Err(format!(
                "daemon run was renamed to 'onda run render'\n\n{}",
                usage()
            ))
        }
        _ => {
            return Err(format!(
                "unknown daemon subcommand '{subcommand}'\n\n{}",
                usage()
            ))
        }
    };
    Ok(Command::Daemon(daemon))
}

fn parse_compile_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(input) = args.next() else {
        return Err(format!("compile requires an input file\n\n{}", usage()));
    };
    let mut emit = CompileEmit::Check;
    let mut emit_explicit = false;
    let mut output = None::<PathBuf>;
    let mut meta_out = None::<PathBuf>;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    let mut dump_graph = false;
    let mut show_meta = false;
    let mut fast_math = false;
    let mut target_spec_path = None::<PathBuf>;
    let mut target_triple_override = None::<String>;
    let mut target_cpu_override = None::<TargetCpu>;
    let mut target_features_override = None::<String>;
    let mut target_abi_override = None::<String>;
    let mut reloc_model_override = None::<TargetRelocMode>;
    let mut code_model_override = None::<TargetCodeModel>;
    let mut opt_level_override = None::<TargetOptLevel>;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--emit" => {
                let Some(value) = args.next() else {
                    return Err("--emit requires check, llvm-ir, or obj".to_owned());
                };
                if emit == CompileEmit::LlvmIr && !emit_explicit {
                    return Err("cannot use both --ir and --emit".to_owned());
                }
                emit = parse_compile_emit(&value)?;
                emit_explicit = true;
            }
            "--output" | "-o" => {
                let Some(value) = args.next() else {
                    return Err("--output requires a file path".to_owned());
                };
                output = Some(PathBuf::from(value));
            }
            "--meta-out" => {
                let Some(value) = args.next() else {
                    return Err("--meta-out requires a file path".to_owned());
                };
                meta_out = Some(PathBuf::from(value));
            }
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--dump-graph" => dump_graph = true,
            "--ir" => {
                if emit_explicit {
                    return Err("cannot use both --ir and --emit".to_owned());
                }
                emit = CompileEmit::LlvmIr;
            }
            "--meta" => show_meta = true,
            "--fast-math" => fast_math = true,
            "--target-triple" => {
                let Some(value) = args.next() else {
                    return Err("--target-triple requires a target triple".to_owned());
                };
                target_triple_override = Some(value);
            }
            "--target-spec" => {
                let Some(value) = args.next() else {
                    return Err("--target-spec requires a TOML file path".to_owned());
                };
                target_spec_path = Some(PathBuf::from(value));
            }
            "--target-cpu" => {
                let Some(value) = args.next() else {
                    return Err("--target-cpu requires a CPU name or 'host'".to_owned());
                };
                target_cpu_override = Some(parse_target_cpu(&value));
            }
            "--target-features" => {
                let Some(value) = args.next() else {
                    return Err(
                        "--target-features requires a comma-separated feature list".to_owned()
                    );
                };
                target_features_override = Some(value);
            }
            "--target-abi" => {
                let Some(value) = args.next() else {
                    return Err("--target-abi requires a non-empty ABI name".to_owned());
                };
                target_abi_override = Some(value);
            }
            "--reloc-model" => {
                let Some(value) = args.next() else {
                    return Err("--reloc-model requires a value".to_owned());
                };
                reloc_model_override = Some(parse_target_reloc_model(&value)?);
            }
            "--code-model" => {
                let Some(value) = args.next() else {
                    return Err("--code-model requires a value".to_owned());
                };
                code_model_override = Some(parse_target_code_model(&value)?);
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level_override = Some(parse_target_opt_level(&value)?);
            }
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                let value = &arg["--sample-rate=".len()..];
                sample_rate_hz = parse_sample_rate_hz(value)?;
            }
            _ if arg.starts_with("--emit=") => {
                if emit == CompileEmit::LlvmIr && !emit_explicit {
                    return Err("cannot use both --ir and --emit".to_owned());
                }
                let value = &arg["--emit=".len()..];
                emit = parse_compile_emit(value)?;
                emit_explicit = true;
            }
            _ if arg.starts_with("--output=") => {
                let value = &arg["--output=".len()..];
                if value.is_empty() {
                    return Err("--output requires a file path".to_owned());
                }
                output = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--meta-out=") => {
                let value = &arg["--meta-out=".len()..];
                if value.is_empty() {
                    return Err("--meta-out requires a file path".to_owned());
                }
                meta_out = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--sr=") => {
                let value = &arg["--sr=".len()..];
                sample_rate_hz = parse_sample_rate_hz(value)?;
            }
            _ if arg.starts_with("--block-size=") => {
                let value = &arg["--block-size=".len()..];
                block_frames = parse_block_frames(value)?;
            }
            _ if arg.starts_with("--target-triple=") => {
                let value = &arg["--target-triple=".len()..];
                if value.is_empty() {
                    return Err("--target-triple requires a target triple".to_owned());
                }
                target_triple_override = Some(value.to_owned());
            }
            _ if arg.starts_with("--target-spec=") => {
                let value = &arg["--target-spec=".len()..];
                if value.is_empty() {
                    return Err("--target-spec requires a TOML file path".to_owned());
                }
                target_spec_path = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--target-cpu=") => {
                let value = &arg["--target-cpu=".len()..];
                target_cpu_override = Some(parse_target_cpu(value));
            }
            _ if arg.starts_with("--target-features=") => {
                let value = &arg["--target-features=".len()..];
                target_features_override = Some(value.to_owned());
            }
            _ if arg.starts_with("--target-abi=") => {
                let value = &arg["--target-abi=".len()..];
                if value.is_empty() {
                    return Err("--target-abi requires a non-empty ABI name".to_owned());
                }
                target_abi_override = Some(value.to_owned());
            }
            _ if arg.starts_with("--reloc-model=") => {
                let value = &arg["--reloc-model=".len()..];
                reloc_model_override = Some(parse_target_reloc_model(value)?);
            }
            _ if arg.starts_with("--code-model=") => {
                let value = &arg["--code-model=".len()..];
                code_model_override = Some(parse_target_code_model(value)?);
            }
            _ if arg.starts_with("--opt-level=") => {
                let value = &arg["--opt-level=".len()..];
                opt_level_override = Some(parse_target_opt_level(value)?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }
    let LoadedTargetSpec {
        mut target,
        mut cpu_explicit,
        mut features_explicit,
    } = match target_spec_path.as_deref() {
        Some(path) => load_target_spec(path)?,
        None => LoadedTargetSpec::default(),
    };

    if let Some(triple) = target_triple_override {
        target.triple = Some(triple);
    }
    if let Some(cpu) = target_cpu_override {
        target.cpu = cpu;
        cpu_explicit = true;
    }
    if let Some(features) = target_features_override {
        target.features = Some(features);
        features_explicit = true;
    }
    if let Some(abi_name) = target_abi_override {
        target.abi_name = Some(abi_name);
    }
    if let Some(reloc_model) = reloc_model_override {
        target.reloc_model = reloc_model;
    }
    if let Some(code_model) = code_model_override {
        target.code_model = code_model;
    }
    if let Some(opt_level) = opt_level_override {
        target.opt_level = opt_level;
    }

    if target.triple.is_some() && !cpu_explicit {
        target.cpu = TargetCpu::Explicit("generic".to_owned());
    }
    if target.triple.is_some() && !features_explicit && matches!(target.cpu, TargetCpu::Explicit(_))
    {
        target.features = Some(String::new());
    }
    Ok(Command::Compile {
        input: PathBuf::from(input),
        emit,
        output,
        meta_out,
        sample_rate_hz,
        block_frames,
        dump_graph,
        show_meta,
        fast_math,
        target,
    })
}

fn parse_daemon_diagnose_args(
    mut args: impl Iterator<Item = String>,
) -> Result<DaemonCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!(
            "daemon diagnose requires an input file\n\n{}",
            usage()
        ));
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
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }
    Ok(DaemonCommand::Diagnose {
        input: PathBuf::from(input),
        sample_rate_hz,
        block_frames,
    })
}

fn parse_run_render_args(mut args: impl Iterator<Item = String>) -> Result<RunCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("run render requires an input file\n\n{}", usage()));
    };

    let mut output = PathBuf::from(DEFAULT_DAEMON_OUTPUT);
    let mut dur_seconds = DEFAULT_DUR_SECONDS;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    let mut opt_level = TargetOptLevel::O3;
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
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level = parse_target_opt_level(&value)?;
            }
            "--set" => {
                let Some(value) = args.next() else {
                    return Err("--set requires a name=value pair".to_owned());
                };
                param_sets.push(parse_param_setting(&value)?);
            }
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--help" | "-h" => return Err(usage().to_owned()),
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
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ if arg.starts_with("--opt-level=") => {
                opt_level = parse_target_opt_level(&arg["--opt-level=".len()..])?;
            }
            _ if arg.starts_with("--set=") => {
                param_sets.push(parse_param_setting(&arg["--set=".len()..])?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }

    Ok(RunCommand::Render {
        input: PathBuf::from(input),
        output,
        dur_seconds,
        sample_rate_hz,
        block_frames,
        opt_level,
        fast_math,
        show_meta,
        param_sets,
    })
}

fn parse_run_window_args(
    input: String,
    mut args: impl Iterator<Item = String>,
) -> Result<RunCommand, String> {
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_PLAY_BLOCK_FRAMES;
    let mut opt_level = TargetOptLevel::O3;
    let mut input_device = None;
    let mut output_device = None;
    let mut fast_math = false;
    let mut show_meta = false;
    let mut theme = RunThemeMode::Auto;
    let mut host = RunHostKind::Auto;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level = parse_target_opt_level(&value)?;
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
            "--theme" => {
                let Some(value) = args.next() else {
                    return Err("--theme requires one of: auto, dark, light".to_owned());
                };
                theme = parse_run_theme_mode(&value)?;
            }
            "--webview" => host = RunHostKind::Webview,
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ if arg.starts_with("--opt-level=") => {
                opt_level = parse_target_opt_level(&arg["--opt-level=".len()..])?;
            }
            _ if arg.starts_with("--input-device=") => {
                input_device = Some(arg["--input-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--output-device=") => {
                output_device = Some(arg["--output-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--theme=") => {
                theme = parse_run_theme_mode(&arg["--theme=".len()..])?;
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }

    Ok(RunCommand::Window {
        input: PathBuf::from(input),
        sample_rate_hz,
        block_frames,
        opt_level,
        input_device,
        output_device,
        fast_math,
        show_meta,
        theme,
        host,
    })
}

fn parse_run_theme_mode(value: &str) -> Result<RunThemeMode, String> {
    match value {
        "auto" => Ok(RunThemeMode::Auto),
        "dark" => Ok(RunThemeMode::Dark),
        "light" => Ok(RunThemeMode::Light),
        _ => Err(format!(
            "invalid --theme value '{value}'; expected auto, dark, or light"
        )),
    }
}

fn parse_run_play_args(mut args: impl Iterator<Item = String>) -> Result<RunCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("run play requires an input file\n\n{}", usage()));
    };

    let mut dur_seconds = Some(DEFAULT_DUR_SECONDS);
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_PLAY_BLOCK_FRAMES;
    let mut opt_level = TargetOptLevel::O3;
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
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level = parse_target_opt_level(&value)?;
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
            "--help" | "-h" => return Err(usage().to_owned()),
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
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ if arg.starts_with("--opt-level=") => {
                opt_level = parse_target_opt_level(&arg["--opt-level=".len()..])?;
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
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }

    Ok(RunCommand::Play {
        input: PathBuf::from(input),
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

fn parse_compile_emit(value: &str) -> Result<CompileEmit, String> {
    match value {
        "check" => Ok(CompileEmit::Check),
        "llvm-ir" => Ok(CompileEmit::LlvmIr),
        "obj" => Ok(CompileEmit::Object),
        _ => Err(format!(
            "invalid compile emit mode '{value}', expected check|llvm-ir|obj"
        )),
    }
}

fn parse_target_cpu(value: &str) -> TargetCpu {
    if value.eq_ignore_ascii_case("host") {
        TargetCpu::Host
    } else {
        TargetCpu::Explicit(value.to_owned())
    }
}

fn parse_target_reloc_model(value: &str) -> Result<TargetRelocMode, String> {
    match value {
        "default" => Ok(TargetRelocMode::Default),
        "static" => Ok(TargetRelocMode::Static),
        "pic" => Ok(TargetRelocMode::Pic),
        "dynamic-no-pic" => Ok(TargetRelocMode::DynamicNoPic),
        _ => Err(format!(
            "invalid reloc model '{value}', expected default|static|pic|dynamic-no-pic"
        )),
    }
}

fn parse_target_code_model(value: &str) -> Result<TargetCodeModel, String> {
    match value {
        "default" => Ok(TargetCodeModel::Default),
        "small" => Ok(TargetCodeModel::Small),
        "kernel" => Ok(TargetCodeModel::Kernel),
        "medium" => Ok(TargetCodeModel::Medium),
        "large" => Ok(TargetCodeModel::Large),
        _ => Err(format!(
            "invalid code model '{value}', expected default|small|kernel|medium|large"
        )),
    }
}

fn parse_target_opt_level(value: &str) -> Result<TargetOptLevel, String> {
    match value {
        "0" => Ok(TargetOptLevel::O0),
        "1" => Ok(TargetOptLevel::O1),
        "2" => Ok(TargetOptLevel::O2),
        "3" => Ok(TargetOptLevel::O3),
        _ => Err(format!(
            "invalid optimization level '{value}', expected 0|1|2|3"
        )),
    }
}

fn load_target_spec(path: &Path) -> Result<LoadedTargetSpec, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read target spec '{}': {err}", path.display()))?;
    let mut loaded = LoadedTargetSpec::default();
    let mut reloc_model_explicit = false;
    let mut code_model_explicit = false;
    let mut opt_level_explicit = false;

    for (line_idx, line) in text.lines().enumerate() {
        let line_no = line_idx + 1;
        let line = strip_target_spec_comment(line).trim();
        if line.is_empty() {
            continue;
        }

        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(format!(
                "failed to parse target spec '{}': expected 'key = value' at line {}",
                path.display(),
                line_no
            ));
        };
        let key = raw_key.trim();
        let value = parse_target_spec_value(raw_value.trim(), path, line_no)?;

        match key {
            "triple" => {
                if loaded.target.triple.is_some() {
                    return Err(format!(
                        "target spec '{}' defines 'triple' more than once",
                        path.display()
                    ));
                }
                let triple = expect_target_spec_string(&value, path, line_no, "triple")?;
                if triple.trim().is_empty() {
                    return Err(format!(
                        "target spec '{}' has an empty 'triple' field",
                        path.display()
                    ));
                }
                loaded.target.triple = Some(triple);
            }
            "cpu" => {
                if loaded.cpu_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'cpu' more than once",
                        path.display()
                    ));
                }
                let cpu = expect_target_spec_string(&value, path, line_no, "cpu")?;
                if cpu.trim().is_empty() {
                    return Err(format!(
                        "target spec '{}' has an empty 'cpu' field",
                        path.display()
                    ));
                }
                loaded.target.cpu = parse_target_cpu(&cpu);
                loaded.cpu_explicit = true;
            }
            "features" => {
                if loaded.features_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'features' more than once",
                        path.display()
                    ));
                }
                loaded.target.features = Some(expect_target_spec_string(
                    &value, path, line_no, "features",
                )?);
                loaded.features_explicit = true;
            }
            "abi_name" => {
                if loaded.target.abi_name.is_some() {
                    return Err(format!(
                        "target spec '{}' defines 'abi_name' more than once",
                        path.display()
                    ));
                }
                let abi_name = expect_target_spec_string(&value, path, line_no, "abi_name")?;
                let abi_name = abi_name.trim();
                if !abi_name.is_empty() {
                    loaded.target.abi_name = Some(abi_name.to_owned());
                }
            }
            "reloc_model" => {
                if reloc_model_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'reloc_model' more than once",
                        path.display()
                    ));
                }
                let reloc_model = expect_target_spec_string(&value, path, line_no, "reloc_model")?;
                loaded.target.reloc_model = parse_target_reloc_model(&reloc_model)?;
                reloc_model_explicit = true;
            }
            "code_model" => {
                if code_model_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'code_model' more than once",
                        path.display()
                    ));
                }
                let code_model = expect_target_spec_string(&value, path, line_no, "code_model")?;
                loaded.target.code_model = parse_target_code_model(&code_model)?;
                code_model_explicit = true;
            }
            "opt_level" => {
                if opt_level_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'opt_level' more than once",
                        path.display()
                    ));
                }
                loaded.target.opt_level = match value {
                    TargetSpecValue::Integer(value) => parse_target_opt_level(&value.to_string())?,
                    TargetSpecValue::String(value) => parse_target_opt_level(&value)?,
                };
                opt_level_explicit = true;
            }
            _ => {
                return Err(format!(
                    "failed to parse target spec '{}': unsupported key '{}' at line {}",
                    path.display(),
                    key,
                    line_no
                ));
            }
        }
    }

    Ok(loaded)
}

fn strip_target_spec_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in line.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '#' => return &line[..idx],
            _ => {}
        }
    }

    line
}

fn parse_target_spec_value(
    raw: &str,
    path: &Path,
    line_no: usize,
) -> Result<TargetSpecValue, String> {
    if raw.starts_with('"') {
        return Ok(TargetSpecValue::String(parse_target_spec_string_literal(
            raw, path, line_no,
        )?));
    }

    let value = raw.parse::<i64>().map_err(|_| {
        format!(
            "failed to parse target spec '{}': expected string or integer literal at line {}",
            path.display(),
            line_no
        )
    })?;
    Ok(TargetSpecValue::Integer(value))
}

fn parse_target_spec_string_literal(
    raw: &str,
    path: &Path,
    line_no: usize,
) -> Result<String, String> {
    if !raw.ends_with('"') || raw.len() < 2 {
        return Err(format!(
            "failed to parse target spec '{}': invalid string literal at line {}",
            path.display(),
            line_no
        ));
    }

    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err(format!(
                "failed to parse target spec '{}': dangling escape in string literal at line {}",
                path.display(),
                line_no
            ));
        };
        match escaped {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            _ => {
                return Err(format!(
                    "failed to parse target spec '{}': unsupported escape '\\{}' at line {}",
                    path.display(),
                    escaped,
                    line_no
                ));
            }
        }
    }

    Ok(out)
}

fn expect_target_spec_string(
    value: &TargetSpecValue,
    path: &Path,
    line_no: usize,
    key: &str,
) -> Result<String, String> {
    match value {
        TargetSpecValue::String(value) => Ok(value.clone()),
        TargetSpecValue::Integer(_) => Err(format!(
            "failed to parse target spec '{}': '{}' must be a string at line {}",
            path.display(),
            key,
            line_no
        )),
    }
}

fn default_object_output_path(input: &Path, target_triple: &str) -> PathBuf {
    let ext = if is_coff_target_triple(target_triple) {
        "obj"
    } else {
        "o"
    };
    input.with_extension(ext)
}

fn default_metadata_output_path(object_path: &Path) -> PathBuf {
    object_path.with_extension("onda.json")
}

fn is_coff_target_triple(target_triple: &str) -> bool {
    let triple = target_triple.to_ascii_lowercase();
    triple.contains("windows") || triple.contains("msvc")
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

fn run_run(cmd: RunCommand) -> Result<(), String> {
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
        } => run_daemon_play(
            &input,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            opt_level,
            input_device.as_deref(),
            output_device.as_deref(),
            fast_math,
            show_meta,
            control_json,
            &param_sets,
        ),
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
        } => run_daemon_run(
            &input,
            &output,
            dur_seconds,
            sample_rate_hz,
            block_frames,
            opt_level,
            fast_math,
            show_meta,
            &param_sets,
        ),
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
                &input,
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

fn run_daemon_run(
    input: &Path,
    output: &Path,
    dur_seconds: u32,
    sample_rate_hz: u32,
    block_frames: usize,
    opt_level: TargetOptLevel,
    fast_math: bool,
    show_meta: bool,
    param_sets: &[(String, f64)],
) -> Result<(), String> {
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

fn run_daemon_play(
    input: &Path,
    dur_seconds: Option<u32>,
    sample_rate_hz: u32,
    block_frames: usize,
    opt_level: TargetOptLevel,
    input_device: Option<&str>,
    output_device: Option<&str>,
    fast_math: bool,
    show_meta: bool,
    control_json: bool,
    param_sets: &[(String, f64)],
) -> Result<(), String> {
    play_run_realtime(PlaybackLaunch {
        input: input.to_path_buf(),
        dur_seconds,
        sample_rate_hz,
        block_frames,
        opt_level,
        input_device: input_device.map(str::to_owned),
        output_device: output_device.map(str::to_owned),
        fast_math,
        show_meta,
        control_json,
        param_sets: param_sets.to_vec(),
    })
}

fn format_run_build_error(context: &str, err: &RunBuildError) -> String {
    match err {
        RunBuildError::Diagnostics(diags) => format_diagnostics(context, diags),
        RunBuildError::Runtime(diag) => format_single_diagnostic(context, diag),
    }
}

fn run_compile(
    input: &Path,
    emit: CompileEmit,
    output: Option<&Path>,
    meta_out: Option<&Path>,
    sample_rate_hz: u32,
    block_frames: usize,
    dump_graph: bool,
    show_meta: bool,
    fast_math: bool,
    target: TargetConfig,
) -> Result<(), String> {
    if dump_graph {
        let lowered = parse_and_lower_graphs(input, sample_rate_hz as f32, block_frames)?;
        print!("{}", format_program(&lowered));
    }
    let typed = parse_and_analyze(input, sample_rate_hz as f32, block_frames)?;
    if show_meta {
        print_program_meta(&typed);
    }
    let codegen_options = CodegenOptions {
        sample_rate: sample_rate_hz as f32,
        block_size: block_frames,
        fast_math,
        target,
    };

    match emit {
        CompileEmit::Check => {
            if output.is_some() {
                return Err("--output is only valid with --emit llvm-ir or --emit obj".to_owned());
            }
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            if !codegen_options.target.is_host_default() {
                lower_to_target_llvm_ir_with_options(typed, codegen_options).map_err(|diags| {
                    format_diagnostics("target codegen validation failed", &diags)
                })?;
            }
            println!("OK: {}", input.display());
        }
        CompileEmit::LlvmIr => {
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            let ir = lower_to_target_llvm_ir_with_options(typed, codegen_options)
                .map_err(|diags| format_diagnostics("IR lowering failed", &diags))?;
            if let Some(path) = output {
                fs::write(path, ir.as_bytes()).map_err(|err| {
                    format!("failed to write LLVM IR '{}': {err}", path.display())
                })?;
                println!("Wrote LLVM IR: {}", path.display());
            } else {
                println!("{ir}");
            }
        }
        CompileEmit::Object => {
            let artifact = lower_to_object_with_options(typed, codegen_options)
                .map_err(|diags| format_diagnostics("object emission failed", &diags))?;
            let object_path = output.map(Path::to_path_buf).unwrap_or_else(|| {
                default_object_output_path(input, &artifact.metadata.target.triple)
            });
            fs::write(&object_path, &artifact.object_bytes).map_err(|err| {
                format!(
                    "failed to write object file '{}': {err}",
                    object_path.display()
                )
            })?;

            let metadata_path = meta_out
                .map(Path::to_path_buf)
                .unwrap_or_else(|| default_metadata_output_path(&object_path));
            let metadata_json = serde_json::to_string_pretty(&artifact.metadata)
                .map_err(|err| format!("failed to encode metadata JSON: {err}"))?;
            fs::write(&metadata_path, metadata_json.as_bytes()).map_err(|err| {
                format!(
                    "failed to write metadata sidecar '{}': {err}",
                    metadata_path.display()
                )
            })?;

            println!("Wrote object: {}", object_path.display());
            println!("Wrote metadata: {}", metadata_path.display());
        }
    }
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
                        onda_frontend::GraphRate::Block => "@block ",
                        onda_frontend::GraphRate::Sample => "@sample ",
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
    ports: &[onda_frontend::PortDecl],
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
    params: &[onda_frontend::ParamDecl],
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
    buffers: &[onda_frontend::BufferDecl],
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

fn format_event(event: &onda_frontend::EventDef, indent: usize, out: &mut String) {
    let mut header = format!("{}(", event.name);
    header.push_str(
        &event
            .params
            .iter()
            .map(|param| {
                let mut text = format!("{}: {}", param.name, format_event_param_type(&param.ty));
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

fn format_graph_endpoint(endpoint: &onda_frontend::GraphEndpoint) -> String {
    match endpoint {
        onda_frontend::GraphEndpoint::Symbol { name, .. } => name.clone(),
        onda_frontend::GraphEndpoint::ProcField { proc, field, .. } => {
            format!("{proc}.{field}")
        }
        onda_frontend::GraphEndpoint::ProcIndexedField {
            proc, index, field, ..
        } => {
            format!("{proc}[{}].{field}", format_expr(index))
        }
    }
}

fn format_graph_destinations(dests: &[onda_frontend::GraphEndpoint]) -> String {
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

fn format_fn_param_type(ty: &onda_frontend::FnParamType) -> String {
    match ty {
        onda_frontend::FnParamType::Primitive(ty) => primitive_type_name(*ty).to_owned(),
        onda_frontend::FnParamType::Struct(name) => name.clone(),
        onda_frontend::FnParamType::Buffer(ty) => format_buffer_type(ty),
        onda_frontend::FnParamType::Array(Some(ty)) => format!("{}[]", primitive_type_name(*ty)),
        onda_frontend::FnParamType::Array(None) => "[]".to_owned(),
        onda_frontend::FnParamType::ArrayGeneric(name) => format!("{name}[]"),
        onda_frontend::FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        } => {
            let type_str = if let Some(prim) = elem {
                primitive_type_name(*prim).to_owned()
            } else if let Some(g) = generic_name {
                g.clone()
            } else {
                "?".to_owned()
            };
            format!("{type_str}[{size:?}]")
        }
        onda_frontend::FnParamType::BareBuffer => "buffer".to_owned(),
        onda_frontend::FnParamType::Tuple(elems) => {
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

fn format_port_decl(port: &onda_frontend::PortDecl) -> String {
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

fn format_param_decl(param: &onda_frontend::ParamDecl) -> String {
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

fn format_number(value: f64) -> String {
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
        format_diag_snippet, format_expr, format_program, parse_args, Command, CompileEmit,
        DaemonCommand, RunCommand, RunHostKind, DEFAULT_PLAY_BLOCK_FRAMES,
    };
    use onda_codegen_llvm::{TargetCodeModel, TargetCpu, TargetOptLevel, TargetRelocMode};
    use onda_frontend::{
        Block, CallArg, Diagnostic, Expr, GraphBlock, GraphEdge, GraphEndpoint, Program,
    };
    use onda_run::RunThemeMode;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_temp_target_spec(contents: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "onda-target-spec-{}-{stamp}.toml",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("target spec should write");
        path
    }

    #[test]
    fn parse_compile_accepts_dump_graph() {
        let cmd = parse_args(
            ["onda", "compile", "x.onda", "--dump-graph"]
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
    fn parse_compile_accepts_target_codegen_flags() {
        let cmd = parse_args(
            [
                "onda",
                "compile",
                "x.onda",
                "--sample-rate",
                "44100",
                "--block-size",
                "256",
                "--target-triple",
                "aarch64-unknown-linux-gnu",
                "--target-cpu",
                "cortex-a72",
                "--target-features",
                "+neon,+fp-armv8",
                "--target-abi",
                "aapcs",
                "--reloc-model",
                "pic",
                "--code-model",
                "small",
                "--opt-level",
                "2",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("compile target args should parse");
        match cmd {
            Command::Compile {
                sample_rate_hz,
                block_frames,
                target,
                ..
            } => {
                assert_eq!(sample_rate_hz, 44_100);
                assert_eq!(block_frames, 256);
                assert_eq!(target.triple.as_deref(), Some("aarch64-unknown-linux-gnu"));
                assert_eq!(target.cpu, TargetCpu::Explicit("cortex-a72".to_owned()));
                assert_eq!(target.features.as_deref(), Some("+neon,+fp-armv8"));
                assert_eq!(target.abi_name.as_deref(), Some("aapcs"));
                assert_eq!(target.reloc_model, TargetRelocMode::Pic);
                assert_eq!(target.code_model, TargetCodeModel::Small);
                assert_eq!(target.opt_level, TargetOptLevel::O2);
            }
            _ => panic!("expected compile command"),
        }
    }

    #[test]
    fn parse_compile_defaults_cross_target_cpu_to_generic() {
        let cmd = parse_args(
            [
                "onda",
                "compile",
                "x.onda",
                "--target-triple",
                "aarch64-unknown-linux-gnu",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("compile target args should parse");
        match cmd {
            Command::Compile { target, .. } => {
                assert_eq!(target.cpu, TargetCpu::Explicit("generic".to_owned()));
                assert_eq!(target.features.as_deref(), Some(""));
            }
            _ => panic!("expected compile command"),
        }
    }

    #[test]
    fn parse_compile_loads_target_spec_and_applies_cli_overrides() {
        let spec_path = write_temp_target_spec(
            r#"
triple = "aarch64-unknown-linux-gnu"
cpu = "generic"
features = "+neon"
abi_name = ""
reloc_model = "pic"
code_model = "small"
opt_level = 1
"#,
        );
        let spec_arg = spec_path.to_string_lossy().to_string();
        let result = parse_args(
            [
                "onda",
                "compile",
                "x.onda",
                "--target-spec",
                &spec_arg,
                "--target-cpu",
                "cortex-a72",
                "--opt-level",
                "3",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let _ = std::fs::remove_file(&spec_path);

        let cmd = result.expect("compile target spec args should parse");
        match cmd {
            Command::Compile { target, .. } => {
                assert_eq!(target.triple.as_deref(), Some("aarch64-unknown-linux-gnu"));
                assert_eq!(target.cpu, TargetCpu::Explicit("cortex-a72".to_owned()));
                assert_eq!(target.features.as_deref(), Some("+neon"));
                assert_eq!(target.reloc_model, TargetRelocMode::Pic);
                assert_eq!(target.code_model, TargetCodeModel::Small);
                assert_eq!(target.opt_level, TargetOptLevel::O3);
            }
            _ => panic!("expected compile command"),
        }
    }

    #[test]
    fn parse_compile_target_spec_defaults_cross_target_cpu_to_generic() {
        let spec_path = write_temp_target_spec(
            r#"
triple = "wasm32-unknown-unknown"
"#,
        );
        let spec_arg = spec_path.to_string_lossy().to_string();
        let result = parse_args(
            ["onda", "compile", "x.onda", "--target-spec", &spec_arg]
                .into_iter()
                .map(str::to_owned),
        );
        let _ = std::fs::remove_file(&spec_path);

        let cmd = result.expect("compile target spec args should parse");
        match cmd {
            Command::Compile { target, .. } => {
                assert_eq!(target.triple.as_deref(), Some("wasm32-unknown-unknown"));
                assert_eq!(target.cpu, TargetCpu::Explicit("generic".to_owned()));
                assert_eq!(target.features.as_deref(), Some(""));
            }
            _ => panic!("expected compile command"),
        }
    }

    #[test]
    fn parse_compile_rejects_duplicate_target_spec_default_keys() {
        let spec_path = write_temp_target_spec(
            r#"
triple = "wasm32-unknown-unknown"
reloc_model = "default"
reloc_model = "default"
"#,
        );
        let spec_arg = spec_path.to_string_lossy().to_string();
        let err = match parse_args(
            ["onda", "compile", "x.onda", "--target-spec", &spec_arg]
                .into_iter()
                .map(str::to_owned),
        ) {
            Ok(_) => panic!("compile should reject duplicate target spec keys"),
            Err(err) => err,
        };
        let _ = std::fs::remove_file(&spec_path);

        assert!(err.contains("defines 'reloc_model' more than once"));
    }

    #[test]
    fn parse_compile_accepts_object_emit_and_meta_out() {
        let cmd = parse_args(
            [
                "onda",
                "compile",
                "x.onda",
                "--emit",
                "obj",
                "--output",
                "x.o",
                "--meta-out",
                "x.onda.json",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("compile object args should parse");
        match cmd {
            Command::Compile {
                emit,
                output,
                meta_out,
                ..
            } => {
                assert_eq!(emit, CompileEmit::Object);
                assert_eq!(output.as_deref(), Some(Path::new("x.o")));
                assert_eq!(meta_out.as_deref(), Some(Path::new("x.onda.json")));
            }
            _ => panic!("expected compile command"),
        }
    }

    #[test]
    fn parse_compile_rejects_ir_and_emit_together() {
        let err = match parse_args(
            ["onda", "compile", "x.onda", "--ir", "--emit", "obj"]
                .into_iter()
                .map(str::to_owned),
        ) {
            Ok(_) => panic!("compile should reject --ir and --emit together"),
            Err(err) => err,
        };
        assert!(err.contains("cannot use both --ir and --emit"));
    }

    #[test]
    fn parse_daemon_diagnose_accepts_block_and_sample_rate() {
        let cmd = parse_args(
            [
                "onda",
                "daemon",
                "diagnose",
                "x.onda",
                "--block-size",
                "256",
                "--sr",
                "44100",
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
        let cmd = parse_args(["onda", "daemon", "stdio"].into_iter().map(str::to_owned))
            .expect("daemon stdio args should parse");
        match cmd {
            Command::Daemon(DaemonCommand::Stdio) => {}
            _ => panic!("expected daemon stdio command"),
        }
    }

    #[test]
    fn parse_lsp_command() {
        let cmd = parse_args(["onda", "lsp"].into_iter().map(str::to_owned))
            .expect("lsp args should parse");
        match cmd {
            Command::Lsp => {}
            _ => panic!("expected lsp command"),
        }
    }

    #[test]
    fn parse_lsp_command_accepts_stdio_flag() {
        let cmd = parse_args(["onda", "lsp", "--stdio"].into_iter().map(str::to_owned))
            .expect("lsp --stdio args should parse");
        match cmd {
            Command::Lsp => {}
            _ => panic!("expected lsp command"),
        }
    }

    #[test]
    fn parse_run_play_accepts_meta_and_param_sets() {
        let cmd = parse_args(
            [
                "onda",
                "run",
                "play",
                "x.onda",
                "--meta",
                "--control-json",
                "--set",
                "gain=0.5",
                "--fast-math",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("run play args should parse");
        match cmd {
            Command::Run(RunCommand::Play {
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
            _ => panic!("expected run play command"),
        }
    }

    #[test]
    fn parse_run_command_alias_accepts_window_play_and_render() {
        let window = parse_args(["onda", "run", "x.onda"].into_iter().map(str::to_owned))
            .expect("run window args should parse");
        match window {
            Command::Run(RunCommand::Window { .. }) => {}
            _ => panic!("expected run window command"),
        }

        let play = parse_args(
            ["onda", "run", "play", "x.onda"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run play args should parse");
        match play {
            Command::Run(RunCommand::Play { .. }) => {}
            _ => panic!("expected run play command"),
        }

        let render = parse_args(
            ["onda", "run", "render", "x.onda"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run render args should parse");
        match render {
            Command::Run(RunCommand::Render { .. }) => {}
            _ => panic!("expected run render command"),
        }
    }

    #[test]
    fn parse_run_play_accepts_forever() {
        let cmd = parse_args(
            ["onda", "run", "play", "x.onda", "--forever"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run play --forever should parse");
        match cmd {
            Command::Run(RunCommand::Play {
                dur_seconds,
                block_frames,
                ..
            }) => {
                assert_eq!(dur_seconds, None);
                assert_eq!(block_frames, DEFAULT_PLAY_BLOCK_FRAMES);
            }
            _ => panic!("expected run play command"),
        }
    }

    #[test]
    fn parse_run_render_accepts_meta_and_param_sets() {
        let cmd = parse_args(
            [
                "onda",
                "run",
                "render",
                "x.onda",
                "--meta",
                "--set",
                "gain=0.5",
                "--fast-math",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("run render args should parse");
        match cmd {
            Command::Run(RunCommand::Render {
                show_meta,
                fast_math,
                param_sets,
                ..
            }) => {
                assert!(show_meta);
                assert!(fast_math);
                assert_eq!(param_sets, vec![("gain".to_owned(), 0.5)]);
            }
            _ => panic!("expected run render command"),
        }
    }

    #[test]
    fn parse_run_commands_accept_opt_level() {
        let window = parse_args(
            ["onda", "run", "x.onda", "--opt-level", "1"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run window args should parse");
        match window {
            Command::Run(RunCommand::Window { opt_level, .. }) => {
                assert_eq!(opt_level, TargetOptLevel::O1);
            }
            _ => panic!("expected run window command"),
        }

        let play = parse_args(
            ["onda", "run", "play", "x.onda", "--opt-level", "2"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run play args should parse");
        match play {
            Command::Run(RunCommand::Play { opt_level, .. }) => {
                assert_eq!(opt_level, TargetOptLevel::O2);
            }
            _ => panic!("expected run play command"),
        }

        let render = parse_args(
            ["onda", "run", "render", "x.onda", "--opt-level", "0"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run render args should parse");
        match render {
            Command::Run(RunCommand::Render { opt_level, .. }) => {
                assert_eq!(opt_level, TargetOptLevel::O0);
            }
            _ => panic!("expected run render command"),
        }
    }

    #[test]
    fn parse_run_window_accepts_webview_flag() {
        let cmd = parse_args(
            ["onda", "run", "x.onda", "--webview"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run window args should parse");
        match cmd {
            Command::Run(RunCommand::Window { host, .. }) => {
                assert_eq!(host, RunHostKind::Webview);
            }
            _ => panic!("expected run window command"),
        }
    }

    #[test]
    fn parse_run_window_accepts_theme_flag() {
        let cmd = parse_args(
            ["onda", "run", "x.onda", "--theme", "dark"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run window args should parse");
        match cmd {
            Command::Run(RunCommand::Window { theme, .. }) => {
                assert_eq!(theme, RunThemeMode::Dark);
            }
            _ => panic!("expected run window command"),
        }
    }

    #[test]
    fn parse_run_window_accepts_meta_flag() {
        let cmd = parse_args(
            ["onda", "run", "x.onda", "--meta"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("run window args should parse");
        match cmd {
            Command::Run(RunCommand::Window { show_meta, .. }) => {
                assert!(show_meta);
            }
            _ => panic!("expected run window command"),
        }
    }

    #[test]
    fn parse_daemon_play_reports_rename() {
        let err = match parse_args(
            ["onda", "daemon", "play", "x.onda"]
                .into_iter()
                .map(str::to_owned),
        ) {
            Ok(_) => panic!("daemon play should report rename"),
            Err(err) => err,
        };
        assert!(err.contains("onda run play"));
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
        let path = dir.join("onda_cli_diag_range_test.onda");
        std::fs::write(&path, "sample:\n  out1 = missing + 1.0\n").expect("write test source");

        let diag = Diagnostic {
            code: onda_frontend::DiagCode::Semantic,
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
}
