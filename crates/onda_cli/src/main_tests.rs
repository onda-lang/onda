use super::{
    parse_args, Command, CompileEmit, DaemonCommand, RunCommand, RunHostKind,
    DEFAULT_PLAY_BLOCK_FRAMES,
};
use onda_codegen_llvm::{TargetCodeModel, TargetCpu, TargetOptLevel, TargetRelocMode};
use onda_frontend::{
    parse_program, Block, CallArg, Diagnostic, Expr, GraphBlock, GraphEdge, GraphEndpoint, Program,
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
    let cmd =
        parse_args(["onda", "lsp"].into_iter().map(str::to_owned)).expect("lsp args should parse");
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
    assert_eq!(super::formatting::format_expr(&expr), "sat(in1 = mix.out1)");
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
        super::formatting::format_program(&program),
        "graph:\n  src >> { out1, mix.in1 }\n\n"
    );
}

#[test]
fn format_program_preserves_deferred_count_shorthand_sections() {
    let program = parse_program(
        r#"
const N = 2
ins<f64> N
outs<i32> 1:
  out1
params<bool> 3
buffers[f32] N
sample:
  out1 = 0
"#,
    )
    .expect("program should parse");

    assert_eq!(
        super::formatting::format_program(&program),
        "const N = 2\n\nins<f64> N\n\nouts<i32> 1:\n  out1: i32\n\nparams<bool> 3\n\nbuffers[f32] N\n\nsample:\n  out1 = 0\n\n"
    );
}

#[test]
fn format_program_preserves_proc_deferred_count_shorthand_sections() {
    let program = parse_program(
        r#"
proc Voice:
  const N = 2
  ins<f64> N
  outs<i32> 1
  params<bool> N
  buffers[f32] N
  sample:
    out1 = 0

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice.out1
"#,
    )
    .expect("program should parse");

    let formatted = super::formatting::format_program(&program);
    assert!(formatted.contains("  ins<f64> N\n"));
    assert!(formatted.contains("  outs<i32> 1\n"));
    assert!(formatted.contains("  params<bool> N\n"));
    assert!(formatted.contains("  buffers[f32] N\n"));
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

    let snippet = super::diag_print::format_diag_snippet(&diag).expect("snippet should render");
    assert!(snippet.contains("  2 |   out1 = missing + 1.0"));
    assert!(snippet.contains("   |          ^^^^^^^"));

    let _ = std::fs::remove_file(path);
}
