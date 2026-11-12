use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onda-cli-constants-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("main.onda"), source).unwrap();
        fs::write(
            root.join("main.ondaproject"),
            r#"{"entry":"main.onda","constants":{"Gain":0.25}}"#,
        )
        .unwrap();
        Self(root)
    }

    fn command(&self, command: &[&str], input: &str) -> Command {
        let mut process = Command::new(env!("CARGO_BIN_EXE_onda"));
        process.args(command).arg(self.0.join(input));
        process
    }

    fn asset(&self, name: &str, samples: onda_project::BufferSamples) -> PathBuf {
        let asset = onda_project::BufferAsset::new(1, 1, 48000.0, samples).unwrap();
        let path = self.0.join(name);
        fs::write(&path, onda_project::encode_ondabuffer(&asset).unwrap()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rendering_uses_cli_then_project_then_source_defaults() {
    let fixture = Fixture::new(
        "config const Gain: f32 = 0.125\nconfig const Values: f32[2] = [0.25, 0.5]\nsample:\n  out1 = Gain\n  out2 = Values[1]\n",
    );
    let project_bytes = fs::read(fixture.0.join("main.ondaproject")).unwrap();
    let wav = fixture.0.join("output.wav");
    for (input, overrides, expected) in [
        ("main.onda", false, [4096, 16384]),
        ("main.ondaproject", false, [8192, 16384]),
        ("main.onda", true, [16384, -8192]),
        ("main.ondaproject", true, [16384, -8192]),
    ] {
        let mut command = fixture.command(&["run", "render"], input);
        command
            .args(["--dur=0.001", "--block-size=8", "--output"])
            .arg(&wav);
        if overrides {
            command.args(["--const", "Gain=0.5", "--const=Values=[-0.5, -0.25]"]);
        }
        assert_success(&command.output().unwrap());
        let mut reader = hound::WavReader::open(&wav).unwrap();
        assert_eq!(reader.spec().channels, 2);
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!samples.is_empty());
        assert!(
            samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| *frame == expected),
            "{input}"
        );
    }
    assert_eq!(
        fs::read(fixture.0.join("main.ondaproject")).unwrap(),
        project_bytes
    );
}

#[test]
fn bench_and_diagnose_use_overrides_before_semantic_analysis() {
    let fixture = Fixture::new(
        "config const Gain: f32 = missing_default\nnamespace Check:\n  assert(Gain == 0.5)\nsample:\n  out1 = Gain\n",
    );
    for input in ["main.onda", "main.ondaproject"] {
        let mut bench = fixture.command(&["bench"], input);
        bench.args(["--runs=1", "--blocks=1", "--warmup-blocks=0", "--json"]);
        assert!(!bench.output().unwrap().status.success());
        let output = bench.args(["--const=Gain=0.5"]).output().unwrap();
        assert_success(&output);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["total_blocks"], 1);

        let mut diagnose = fixture.command(&["daemon", "diagnose"], input);
        assert!(!diagnose.output().unwrap().status.success());
        assert_success(&diagnose.args(["--const", "Gain=0.5"]).output().unwrap());
    }
}

#[test]
fn all_compiling_commands_reject_invalid_typed_constants_before_execution() {
    let fixture = Fixture::new(
        "config const Gain: f32 = 0.0\nconst Fixed: i32 = 1\nsample:\n  out1 = Gain\n",
    );
    for command in [
        &["compile"][..],
        &["run", "play"],
        &["run", "render"],
        &["bench"],
        &["daemon", "diagnose"],
    ] {
        for (override_value, expected) in [
            ("Unknown=1", "unknown configuration constant 'Unknown'"),
            ("Gain=true", "Gain"),
            ("Fixed=2", "not host-configurable"),
        ] {
            let output = fixture
                .command(command, "main.onda")
                .args(["--const", override_value])
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(1),
                "{command:?} {override_value}"
            );
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains(expected), "{command:?}: {error}");
            assert!(!error.contains("audio device"), "{error}");
        }
    }
}

#[cfg(unix)]
#[test]
fn native_window_forwards_typed_overrides_and_current_bindings_on_reload() {
    use onda_run::{LaunchOverrides, RunController, RunHostOptions};
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new("sample:\n  out1 = 0.0\n");
    let child = fixture.0.join("capture-args.sh");
    let log = fixture.0.join("child-args.txt");
    // Capture the subprocess boundary without requiring a window or audio device.
    fs::write(
        &child,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" END >> \"${0%/*}/child-args.txt\"\nexec sleep 60\n",
    )
    .unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    let mut controller = RunController::new(
        &fixture.0.join("main.onda"),
        RunHostOptions {
            onda_bin: child.to_str().unwrap().to_owned(),
            overrides: LaunchOverrides {
                constants: vec![
                    ("Gain".to_owned(), "0.5".to_owned()),
                    ("Values".to_owned(), "[-0.5, -0.25]".to_owned()),
                ],
                params: vec![
                    ("enabled".into(), onda_daemon::RunParamValue::Bool(true)),
                    (
                        "wide".into(),
                        onda_daemon::RunParamValue::I64(9007199254740993),
                    ),
                    (
                        "rounded".into(),
                        onda_run::parse_run_param_value("9007199254740993.0").unwrap(),
                    ),
                    ("zero".into(), onda_daemon::RunParamValue::F64(-0.0)),
                ],
                buffers: vec![("bank[1]".into(), Some(fixture.0.join("old clip.wav")))],
            },
            ..Default::default()
        },
    )
    .unwrap();
    let wait_for_launches = |controller: &mut RunController, count| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            controller.poll();
            let contents = fs::read_to_string(&log).unwrap_or_default();
            if contents.matches("\nEND\n").count() >= count {
                return contents;
            }
            assert!(
                Instant::now() < deadline,
                "child launch {count} was not captured"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait_for_launches(&mut controller, 1);
    let replacement = fixture.0.join("new clip.wav");
    controller.bind_buffer_file("bank[1]", replacement.to_str().unwrap());
    controller.set_compile_settings(44_100, 512, false).unwrap();
    wait_for_launches(&mut controller, 2);
    controller.clear_buffer("bank[1]");
    fs::write(fixture.0.join("main.onda"), "sample:\n  out1 = 0.25\n").unwrap();
    let contents = wait_for_launches(&mut controller, 3);
    for (index, launch) in contents.split("\nEND\n").take(3).enumerate() {
        let args = launch.lines().collect::<Vec<_>>();
        assert!(args.starts_with(&["run", "play"]));
        assert_eq!(args.iter().filter(|arg| **arg == "--const").count(), 2);
        for expected in ["Gain=0.5", "Values=[-0.5, -0.25]"] {
            assert!(
                args.windows(2).any(|pair| pair == ["--const", expected]),
                "{args:?}"
            );
        }
        for expected in [
            "enabled=true",
            "wide=9007199254740993",
            "rounded=9007199254740992.0",
            "zero=-0.0",
        ] {
            assert!(
                args.windows(2).any(|pair| pair == ["--set", expected]),
                "{args:?}"
            );
        }
        let expected_buffer = match index {
            0 => format!("bank[1]={}", fixture.0.join("old clip.wav").display()),
            1 => format!("bank[1]={}", replacement.display()),
            _ => "bank[1]=".to_owned(),
        };
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--buffer", &expected_buffer]),
            "{args:?}"
        );
    }

    fs::write(
        fixture.0.join("wide.onda"),
        "params:\n  rounded: i64 = 0\nsample:\n  out1 = f32(rounded)\n",
    )
    .unwrap();
    let forwarded = contents
        .lines()
        .find(|arg| arg.starts_with("rounded="))
        .unwrap();
    for setting in ["rounded=9007199254740993.0", forwarded] {
        let output = fixture
            .command(&["run", "render"], "wide.onda")
            .args(["--set", setting, "--dur=0.001", "--output"])
            .arg(fixture.0.join("rejected.wav"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{setting}");
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("must be supplied as decimal integers"));
    }
}

#[test]
fn combined_overrides_use_constant_selected_shapes_before_initialization() {
    let fixture = Fixture::new(
        r#"
config const Count: i32 = 1
enum Mode:
  First
  Second
params:
  levels: f32[Count] = 0.0
  enabled: bool = false
  mode: Mode = Mode::First
  wide: i64 = 0
buffers:
  bank: f32 {Count}
init:
  valid = Count == 2 && levels[1] == 0.5 && enabled && i32(mode) == 1 && wide == 9007199254740993 && bank[1][0] == 0.25
  result = 1 / i32(valid)
sample:
  out1 = f32(result) * 0.25
"#,
    );
    fs::write(fixture.0.join("main.ondaproject"),
        r#"{"entry":"main.onda","constants":{"Count":1},"buffers":{"bank":[null,{"file":"missing.wav"}]}}"#).unwrap();
    let asset = fixture.asset(
        "clip with spaces.ondabuffer",
        onda_project::BufferSamples::F32(vec![0.25]),
    );
    let binding = format!("bank[1]={}", asset.display());
    for input in ["main.onda", "main.ondaproject"] {
        for command in [&["run", "render"][..], &["bench"]] {
            let mut process = fixture.command(command, input);
            process.args([
                "--const=Count=2",
                "--set=levels[1]=0.5",
                "--set=enabled=true",
                "--set=mode=1",
                "--set=wide=9007199254740993",
                "--buffer",
                &binding,
            ]);
            let wav = fixture.0.join("combined.wav");
            if command[0] == "bench" {
                process.args(["--runs=1", "--blocks=1", "--warmup-blocks=0", "--json"]);
            } else {
                process.args(["--dur=0.001", "--output"]).arg(&wav);
            }
            let output = process.output().unwrap();
            assert_success(&output);
            if command[0] == "bench" {
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["total_blocks"], 1);
            } else {
                assert!(hound::WavReader::open(wav)
                    .unwrap()
                    .samples::<i16>()
                    .all(|sample| sample.unwrap() == 8192));
            }
        }
    }
}

#[test]
fn clearing_project_bindings_skips_missing_files_and_validates_slot_names() {
    let fixture = Fixture::new("buffers:\n  clip: f32\nsample:\n  out1 = f32(clip.len())\n");
    fs::write(
        fixture.0.join("main.ondaproject"),
        r#"{"entry":"main.onda","buffers":{"clip":{"file":"missing.wav"}}}"#,
    )
    .unwrap();
    for command in [&["run", "render"][..], &["bench"]] {
        let mut process = fixture.command(command, "main.ondaproject");
        if command[0] == "bench" {
            process.args(["--runs=1", "--blocks=1", "--warmup-blocks=0"]);
        } else {
            process
                .args(["--dur=0.001", "--output"])
                .arg(fixture.0.join("cleared.wav"));
        }
        assert_success(&process.arg("--buffer=clip=").output().unwrap());
        let failure = fixture
            .command(command, "main.onda")
            .arg("--buffer=unknown=")
            .output()
            .unwrap();
        assert_eq!(failure.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&failure.stderr).contains("unknown buffer 'unknown'"));
    }
}

#[test]
fn execution_commands_report_invalid_parameter_and_buffer_bindings() {
    let fixture = Fixture::new(
        "params:\n  levels: f32[2] = 0.0\nbuffers:\n  clip: f32\nsample:\n  out1 = levels[1]\n",
    );
    let wrong_type = fixture.asset(
        "wrong.ondabuffer",
        onda_project::BufferSamples::I32(vec![1]),
    );
    for command in [&["run", "render"][..], &["bench"]] {
        for (option, expected) in [
            ("--set=missing=1".to_owned(), "unknown parameter"),
            ("--set=levels[2]=1".to_owned(), "unknown parameter"),
            (format!("--buffer=clip={}", wrong_type.display()), "expects"),
            ("--buffer=clip[1]=".to_owned(), "unknown buffer"),
        ] {
            let output = fixture
                .command(command, "main.onda")
                .arg(option)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(expected),
                "{output:?}"
            );
        }
    }
}
