use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::LazyLock;

mod args;
mod compile_cmd;
mod daemon_stdio;
mod diag_print;
mod project_cmd;
mod run_cmd;

use args::parse_args;
use compile_cmd::run_compile;
use onda_codegen_llvm::{TargetConfig, TargetOptLevel};
use onda_run::{RunThemeMode, DEFAULT_REALTIME_BLOCK_FRAMES};
use run_cmd::{run_daemon, run_run};

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_DUR_SECONDS: u32 = 5;
const DEFAULT_BLOCK_FRAMES: usize = 512;
const DEFAULT_PLAY_BLOCK_FRAMES: usize = DEFAULT_REALTIME_BLOCK_FRAMES;
const DEFAULT_DAEMON_OUTPUT: &str = "./onda_daemon_out.wav";
const ONDA_VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE_BODY: &str = r#"Commands:
  
  onda                              Open the interactive run window

  onda project <directory>           Create or package an Onda project
    [--from <input.onda>] [--buffer <name=path>]

  onda compile <input>               Check, inspect, or emit compile artifacts
    
    [--emit <check|mir|mir-json|mir-messagepack|llvm-ir|obj>] [--output <path>] [--meta-out <path>]
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math]  
    [--dump-graph] [--ir] [--meta] 
    [--target-spec <path>] [--target-features <feature-list>] 
    [--target-triple <triple>] [--target-cpu <name|host>] [--target-abi <name>] 
    [--reloc-model <default|static|pic|dynamic-no-pic>] 
    [--code-model <default|small|kernel|medium|large>] 
  
  onda run [input]                   Open the interactive run window
    
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math] 
    [--input-device <name>] [--output-device <name>]
    [--theme <auto|dark|light>] [--webview] [--meta]
  
  onda run play <input>              Run realtime playback without the UI
    
    [--dur <seconds> | --forever]
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math]
    [--input-device <name>] [--output-device <name>]
    [--set <name=value>] [--buffer <name=path>] [--control-json] [--meta]
  
  onda run render <input>            Render offline through the run pipeline
    
    [--output <path>] [--dur <seconds>]
    [--sample-rate <hz>] [--block-size <frames>]
    [--opt-level <0|1|2|3>] [--fast-math]
    [--set <name=value>] [--buffer <name=path>] [--meta]
  
  onda daemon diagnose <input>       Run daemon-backed analysis and diagnostics
    
    [--sample-rate <hz>] [--block-size <frames>]

  onda daemon stdio                  Start the daemon transport over stdio
  
  onda lsp                           Start the language server over stdio
  
Shared Options:
  
  --sample-rate, --sr    Sample rate in Hz (default: 48000)
  --block-size, -b       Block size in frames (default: 512; run/play: 256)
  --opt-level            LLVM optimization level (default: 3)
  --fast-math            Enable LLVM fast-math flags for floating-point operations
  --meta                 Print available metadata for the selected command
  --help, -h             Show this help

Project Options:

  --from                 Capture an existing Onda entry and its exact reachable source graph
  --buffer               Package a declared buffer from WAV or .ondabuffer with `name=path`

Compile Options:
  
  --emit                 Compile artifact: `check`, `mir`, `mir-json`, `mir-messagepack`, `llvm-ir`, or `obj`
  --output, -o           Output path for `mir`, `mir-json`, `mir-messagepack`, `llvm-ir`, or `obj`
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
  --buffer               Bind a declared buffer to a WAV or .ondabuffer file with `name=path`
  --control-json         Emit run control handshake on stdout and serve param control over localhost
  --theme                Run window theme: `auto`, `dark`, or `light` (default: auto)
  --webview              Use the webview run host instead of egui
"#;

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
    Project {
        destination: PathBuf,
        source: Option<PathBuf>,
        buffer_bindings: Vec<(String, PathBuf)>,
    },
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
    Mir,
    MirJson,
    MirMessagePack,
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
        buffer_bindings: Vec<(String, PathBuf)>,
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
        buffer_bindings: Vec<(String, PathBuf)>,
    },
    Window {
        input: Option<PathBuf>,
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

fn main() {
    let cmd = match parse_args(env::args()) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            process::exit(2);
        }
    };

    let result = match cmd {
        Command::Project {
            destination,
            source,
            buffer_bindings,
        } => project_cmd::run_project(&destination, source.as_deref(), &buffer_bindings),
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
        } => run_compile(compile_cmd::CompileRequest {
            input: &input,
            emit,
            output: output.as_deref(),
            meta_out: meta_out.as_deref(),
            sample_rate_hz,
            block_frames,
            dump_graph,
            show_meta,
            fast_math,
            target,
        }),
        Command::Lsp => onda_lsp::run_stdio_loop(),
        Command::Run(cmd) => run_run(cmd),
        Command::Daemon(cmd) => run_daemon(cmd),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        process::exit(1);
    }
}

#[cfg(test)]
mod main_tests;
