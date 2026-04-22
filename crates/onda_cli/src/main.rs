use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::env;
use std::io::BufWriter;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

mod args;
mod compile_cmd;
mod daemon_stdio;
mod diag_print;
mod formatting;
mod lsp_stdio;
mod run_cmd;
mod run_control;
mod run_realtime;
mod run_signal;

use args::parse_args;
use compile_cmd::run_compile;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use onda_codegen_llvm::{TargetConfig, TargetOptLevel};
use onda_daemon::{
    DaemonConfig, DaemonSession, RunBufferInfo, RunEventInfo, RunOptions, RunParamInfo,
};
use onda_run::{available_audio_devices, RunThemeMode};
use onda_semantics::AnalysisOptions;
use run_cmd::{run_daemon, run_run};
use run_control::{
    run_buffer_json, run_event_json, run_param_json, spawn_run_control_server, write_json_line,
    PlaybackControlCommand, ScopeRing,
};
use serde_json::json;

use diag_print::{format_run_build_error, format_single_diagnostic};

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_DUR_SECONDS: u32 = 5;
const MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK: usize = 64;
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

#[cfg(test)]
mod main_tests;
