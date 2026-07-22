use super::{
    compile_cmd, parse_args, run_compile, Command, CompileEmit, DaemonCommand, RunCommand,
    RunHostKind, DEFAULT_PLAY_BLOCK_FRAMES,
};
use onda_codegen_llvm::{
    TargetCodeModel, TargetConfig, TargetCpu, TargetOptLevel, TargetRelocMode,
    AOT_METADATA_FORMAT_VERSION, AOT_SNAPSHOT_FORMAT_VERSION, PROCESSOR_ABI_VERSION,
};
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
fn parse_compile_accepts_mir_emit() {
    let cmd = parse_args(
        ["onda", "compile", "x.onda", "--emit", "mir"]
            .into_iter()
            .map(str::to_owned),
    )
    .expect("MIR emit args should parse");
    match cmd {
        Command::Compile { emit, .. } => assert_eq!(emit, CompileEmit::Mir),
        _ => panic!("expected compile command"),
    }
}

#[test]
fn parse_compile_accepts_mir_json_emit() {
    let cmd = parse_args(
        ["onda", "compile", "x.onda", "--emit", "mir-json"]
            .into_iter()
            .map(str::to_owned),
    )
    .expect("MIR JSON emit args should parse");
    match cmd {
        Command::Compile { emit, .. } => assert_eq!(emit, CompileEmit::MirJson),
        _ => panic!("expected compile command"),
    }
}

#[test]
fn parse_compile_accepts_mir_messagepack_emit() {
    let cmd = parse_args(
        ["onda", "compile", "x.onda", "--emit", "mir-messagepack"]
            .into_iter()
            .map(str::to_owned),
    )
    .expect("MIR MessagePack emit args should parse");
    match cmd {
        Command::Compile { emit, .. } => assert_eq!(emit, CompileEmit::MirMessagePack),
        _ => panic!("expected compile command"),
    }
}

#[test]
fn compile_emits_complete_portable_mir_slice() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "onda-mir-source-{}-{stamp}.onda",
        std::process::id()
    ));
    let output_path = std::env::temp_dir().join(format!(
        "onda-mir-output-{}-{stamp}.mir",
        std::process::id()
    ));
    std::fs::write(
        &source_path,
        "const Table: f32[2] = [0.25, 0.5]\ninit:\n  taps: f32[2] = [1.0, 2.0]\n  phase = 0.0\nevent reset(values: f32[2] = [0.0, 0.0]):\n  phase = values[0]\nsample:\n  phase = phase + Table[0]\n  taps[0] = phase\n  out1 = taps[0]\n",
    )
    .expect("source should write");

    let result = run_compile(compile_cmd::CompileRequest {
        input: &source_path,
        emit: CompileEmit::Mir,
        output: Some(&output_path),
        meta_out: None,
        sample_rate_hz: 48_000,
        block_frames: 32,
        dump_graph: false,
        show_meta: false,
        fast_math: false,
        target: TargetConfig::host(),
    });
    let dump = std::fs::read_to_string(&output_path).expect("MIR output should exist");
    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&output_path);

    result.expect("portable source should emit MIR");
    assert!(dump.contains("entry init=@fn0 process=@fn1"));
    assert!(dump.contains("const_data @data0 \"Table\""));
    assert!(dump.contains("event @event0 \"reset\""));
    assert!(dump.contains("store_output @out0"));
    assert!(dump.contains("config sample_rate=48000.0 block_size=32"));
}

#[test]
fn compile_emits_versioned_mir_json() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "onda-mir-json-source-{}-{stamp}.onda",
        std::process::id()
    ));
    let output_path = std::env::temp_dir().join(format!(
        "onda-mir-json-output-{}-{stamp}.json",
        std::process::id()
    ));
    std::fs::write(
        &source_path,
        "init:\n  phase = 0.0\nsample:\n  out1 = phase\n",
    )
    .expect("source should write");

    let result = run_compile(compile_cmd::CompileRequest {
        input: &source_path,
        emit: CompileEmit::MirJson,
        output: Some(&output_path),
        meta_out: None,
        sample_rate_hz: 48_000,
        block_frames: 32,
        dump_graph: false,
        show_meta: false,
        fast_math: false,
        target: TargetConfig::host(),
    });
    let json = std::fs::read_to_string(&output_path).expect("MIR JSON output should exist");
    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&output_path);

    result.expect("portable source should emit MIR JSON");
    let mir = unsafe { onda_mir::from_json_with_producer_proofs(&json) }
        .expect("trusted CLI output should decode as MIR JSON");
    assert_eq!(mir.schema_version, onda_mir::MIR_SCHEMA_VERSION);
    unsafe { onda_mir::validate_with_producer_proofs(&mir) }
        .expect("CLI output should contain valid producer MIR");
}

#[test]
fn compile_emits_versioned_mir_messagepack() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "onda-mir-messagepack-source-{}-{stamp}.onda",
        std::process::id()
    ));
    let output_path = std::env::temp_dir().join(format!(
        "onda-mir-messagepack-output-{}-{stamp}.msgpack",
        std::process::id()
    ));
    std::fs::write(
        &source_path,
        "init:\n  phase = 0.0\nsample:\n  out1 = phase\n",
    )
    .expect("source should write");

    let result = run_compile(compile_cmd::CompileRequest {
        input: &source_path,
        emit: CompileEmit::MirMessagePack,
        output: Some(&output_path),
        meta_out: None,
        sample_rate_hz: 48_000,
        block_frames: 32,
        dump_graph: false,
        show_meta: false,
        fast_math: false,
        target: TargetConfig::host(),
    });
    let bytes = std::fs::read(&output_path).expect("MIR MessagePack output should exist");
    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&output_path);

    result.expect("portable source should emit MIR MessagePack");
    let mir = unsafe { onda_mir::from_messagepack_with_producer_proofs(&bytes) }
        .expect("trusted CLI output should decode as MIR MessagePack");
    assert_eq!(mir.schema_version, onda_mir::MIR_SCHEMA_VERSION);
}

#[test]
fn compile_object_writes_mir_native_metadata_sidecar() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "onda-object-source-{}-{stamp}.onda",
        std::process::id()
    ));
    let object_path = std::env::temp_dir().join(format!(
        "onda-object-output-{}-{stamp}.o",
        std::process::id()
    ));
    let metadata_path = std::env::temp_dir().join(format!(
        "onda-object-metadata-{}-{stamp}.json",
        std::process::id()
    ));
    std::fs::write(
        &source_path,
        r#"
kouts { meter: f64 }
init { held = 0.5 }
block { meter = f64(held) }
events {
  load(values: f32[2]) { held = values[0] + values[1] }
}
"#,
    )
    .expect("source should write");

    let result = run_compile(compile_cmd::CompileRequest {
        input: &source_path,
        emit: CompileEmit::Object,
        output: Some(&object_path),
        meta_out: Some(&metadata_path),
        sample_rate_hz: 48_000,
        block_frames: 64,
        dump_graph: false,
        show_meta: false,
        fast_math: false,
        target: TargetConfig::host(),
    });
    let object = std::fs::read(&object_path).expect("object output should exist");
    let metadata_bytes = std::fs::read(&metadata_path).expect("metadata sidecar should exist");
    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&object_path);
    let _ = std::fs::remove_file(&metadata_path);

    result.expect("source should emit an object and metadata sidecar");
    assert!(!object.is_empty());
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata_bytes).expect("metadata should be valid JSON");
    assert_eq!(metadata["format"], "onda-processor");
    assert_eq!(metadata["format_version"], AOT_METADATA_FORMAT_VERSION);
    assert_eq!(metadata["artifact_kind"], "relocatable_object");
    assert_eq!(metadata["abi_version"], PROCESSOR_ABI_VERSION);
    assert_eq!(metadata["backend"], "llvm");
    assert_eq!(metadata["mir_schema_version"], onda_mir::MIR_SCHEMA_VERSION);
    assert_eq!(metadata["target"]["pointer_model"], "native_address");
    assert!(metadata["target"]["pointer_width_bits"].as_u64().unwrap() >= 32);
    assert_eq!(
        metadata["integration"]["profile"]["kind"],
        "native_relocatable_object"
    );
    assert_eq!(metadata["integration"]["required_symbols"][0], "onda_init");
    assert_eq!(metadata["compile"]["sample_rate"], 48_000.0);
    assert_eq!(metadata["compile"]["block_size"], 64);
    assert_eq!(metadata["exports"]["events"][0], "onda_event_0");
    assert_eq!(metadata["metadata"]["control_outputs"][0]["name"], "meter");
    let control_state_offset = metadata["metadata"]["control_outputs"][0]["state_byte_offset"]
        .as_u64()
        .expect("control output should expose its state offset");
    let state_size = metadata["runtime"]["state_size_bytes"]
        .as_u64()
        .expect("sidecar should expose state size");
    let state_align = metadata["runtime"]["state_align_bytes"]
        .as_u64()
        .expect("sidecar should expose state alignment");
    let snapshot_size = metadata["runtime"]["snapshot_size_bytes"]
        .as_u64()
        .expect("sidecar should expose packed snapshot size");
    assert!(control_state_offset < state_size);
    assert!(state_align >= 1);
    assert!(metadata["runtime"]["param_align_bytes"].as_u64().unwrap() >= 1);
    assert!(snapshot_size <= state_size);
    assert_eq!(metadata["runtime"]["state_initialization"], "zeroed");
    assert_eq!(
        metadata["runtime"]["snapshot_format_version"],
        AOT_SNAPSHOT_FORMAT_VERSION
    );
    assert_eq!(metadata["metadata"]["events"][0]["payload_size_bytes"], 8);
    assert_eq!(
        metadata["metadata"]["events"][0]["params"][0]["byte_size"],
        8
    );
    assert!(metadata["metadata"]["events"][0]["params"][0]["default_reprs"].is_null());
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
            "--buffer",
            "table=samples.wav",
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
            buffer_bindings,
            ..
        }) => {
            assert!(show_meta);
            assert!(control_json);
            assert!(fast_math);
            assert_eq!(param_sets, vec![("gain".to_owned(), 0.5)]);
            assert_eq!(
                buffer_bindings,
                vec![("table".to_owned(), PathBuf::from("samples.wav"))]
            );
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
            "--buffer=table=samples.wav",
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
            buffer_bindings,
            ..
        }) => {
            assert!(show_meta);
            assert!(fast_math);
            assert_eq!(param_sets, vec![("gain".to_owned(), 0.5)]);
            assert_eq!(
                buffer_bindings,
                vec![("table".to_owned(), PathBuf::from("samples.wav"))]
            );
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
    assert_eq!(
        onda_lsp::formatting::format_expr(&expr),
        "sat(in1 = mix.out1)"
    );
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
        onda_lsp::formatting::format_program(&program),
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
        onda_lsp::formatting::format_program(&program),
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

    let formatted = onda_lsp::formatting::format_program(&program);
    assert!(formatted.contains("  ins<f64> N\n"));
    assert!(formatted.contains("  outs<i32> 1\n"));
    assert!(formatted.contains("  params<bool> N\n"));
    assert!(formatted.contains("  buffers[f32] N\n"));
}

#[test]
fn format_program_preserves_kins_and_param_bind_hooks() {
    let program = parse_program(
        r#"
kins<f64> 2

proc Voice:
  params:
    gain = 1.0 {0.0, 1.0} => update
    pin coeffs: f32[2] = [0.5, 0.25]
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice()
"#,
    )
    .expect("program should parse");

    let formatted = onda_lsp::formatting::format_program(&program);
    assert!(formatted.contains("kins<f64> 2\n"));
    assert!(formatted.contains("    gain = 1.0 {0.0, 1.0} => update\n"));
    assert!(formatted.contains("    pin coeffs: f32[2] = [0.5, 0.25]\n"));
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
        editor_visible: true,
    };

    let snippet = super::diag_print::format_diag_snippet(&diag).expect("snippet should render");
    assert!(snippet.contains("  2 |   out1 = missing + 1.0"));
    assert!(snippet.contains("   |          ^^^^^^^"));

    let _ = std::fs::remove_file(path);
}
