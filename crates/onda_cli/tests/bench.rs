use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onda-bench-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("main.onda"), source).unwrap();
        Self(root)
    }

    fn bench(&self, input: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_onda"))
            .arg("bench")
            .arg(self.0.join(input))
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn json_report(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn bench_source_sets_params_and_typed_buffers_and_keeps_json_clean() {
    let fixture = Fixture::new(
        r#"
params:
  gain = 0.0
  gains: f32[2] = [0.0, 0.0]
  wide: i64 = 0
buffers:
  data: buffer<i32>
init:
  print("init")
block:
  valid = gain == 0.75 && gains[1] == 0.25 && wide == 9007199254740993 && data[0] == 7
  check = 1 / i32(valid)
sample:
  print("sample", check)
  out1 = f32(check) + in1
"#,
    );
    let asset =
        onda_project::BufferAsset::new(1, 1, 48_000.0, onda_project::BufferSamples::I32(vec![7]))
            .unwrap();
    fs::write(
        fixture.0.join("data.ondabuffer"),
        onda_project::encode_ondabuffer(&asset).unwrap(),
    )
    .unwrap();
    let binding = format!("data={}", fixture.0.join("data.ondabuffer").display());
    let output = fixture.bench(
        "main.onda",
        &[
            "--runs",
            "3",
            "--blocks=4",
            "--warmup-blocks",
            "2",
            "--block-size",
            "16",
            "--sr=44100",
            "--set",
            "gain=0.75",
            "--set=gains[1]=0.25",
            "--set=wide=9007199254740993",
            "--buffer",
            &binding,
            "--opt-level=2",
            "--fast-math",
            "--meta",
            "--json",
        ],
    );
    let report = json_report(&output);
    assert_eq!(report["measurement"], "native_processor");
    assert_eq!(report["runs"], 3);
    assert_eq!(report["blocks_per_run"], 4);
    assert_eq!(report["warmup_blocks"], 2);
    assert_eq!(report["total_blocks"], 12);
    assert_eq!(report["total_frames"], 192);
    assert_eq!(report["block_size"], 16);
    assert_eq!(report["sample_rate_hz"], 44100);
    assert_eq!(report["opt_level"], "2");
    assert_eq!(report["fast_math"], true);
    assert_eq!(report["run_seconds"].as_array().unwrap().len(), 3);
    assert!(report["median_us_per_block"].as_f64().unwrap() > 0.0);
    assert!(String::from_utf8_lossy(&output.stderr).contains("gain"));

    // Missing overrides reach the generated integer-division failure.
    let failure = fixture.bench(
        "main.onda",
        &["--runs=1", "--blocks=1", "--warmup-blocks=0", "--json"],
    );
    assert_eq!(failure.status.code(), Some(1));
    assert!(failure.stdout.is_empty());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("benchmark processing failed"));
}

#[test]
fn bench_honors_project_constants_and_assets_and_buffer_overrides() {
    let fixture = Fixture::new(
        r#"
config const Selected: i32 = 0
buffers:
  data: buffer<i32>
block:
  valid = Selected == 3 && data[0] == 7
  check = 1 / i32(valid)
sample:
  out1 = f32(check)
"#,
    );
    let project = serde_json::json!({
        "entry": "main.onda",
        "constants": {"Selected": 3},
        "buffers": {"data": {"inline": {
            "element": "i32", "channels": 1, "sample_rate": 48000, "values": [7],
        }}},
    });
    fs::write(fixture.0.join("main.ondaproject"), project.to_string()).unwrap();
    let output = fixture.bench(
        "main.ondaproject",
        &["-n", "1", "--blocks", "2", "--warmup-blocks=0", "-b", "8"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Native processor;"));
    assert!(text.contains("realtime DSP CPU"));
    assert!(text.contains("Measured 2 blocks / 16 frames"));

    let asset =
        onda_project::BufferAsset::new(1, 1, 48_000.0, onda_project::BufferSamples::I32(vec![7]))
            .unwrap();
    fs::write(
        fixture.0.join("override.ondabuffer"),
        onda_project::encode_ondabuffer(&asset).unwrap(),
    )
    .unwrap();
    let mut overridden = project;
    overridden["buffers"]["data"] = serde_json::json!({"file": "missing.ondabuffer"});
    fs::write(fixture.0.join("main.ondaproject"), overridden.to_string()).unwrap();
    let binding = format!(
        "--buffer=data={}",
        fixture.0.join("override.ondabuffer").display()
    );
    let output = fixture.bench(
        "main.ondaproject",
        &[
            "--runs=1",
            "--blocks=1",
            "--warmup-blocks=0",
            &binding,
            "--json",
        ],
    );
    assert_eq!(json_report(&output)["total_blocks"], 1);
}

#[test]
fn bench_preserves_state_across_warmup_and_runs() {
    let fixture = Fixture::new(
        "init:\n  count = 0\nblock:\n  count += 1\n  check = 1 / (8 - count)\nsample:\n  out1 = f32(check)\n",
    );
    let output = fixture.bench(
        "main.onda",
        &["--runs=2", "--blocks=3", "--warmup-blocks=1", "--json"],
    );
    assert_eq!(json_report(&output)["total_blocks"], 6);
    let output = fixture.bench(
        "main.onda",
        &["--runs=2", "--blocks=3", "--warmup-blocks=2", "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("benchmark processing failed"));
}

#[test]
fn bench_reports_setup_errors_without_results() {
    let fixture = Fixture::new("sample:\n  out1 = 0.0\n");
    for args in [
        vec!["--set=unknown=1"],
        vec!["--buffer=unknown=missing.wav"],
    ] {
        let mut options = vec!["--runs=1", "--blocks=1", "--warmup-blocks=0", "--json"];
        options.extend(args);
        let output = fixture.bench("main.onda", &options);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    fs::write(fixture.0.join("main.onda"), "this is invalid Onda").unwrap();
    let output = fixture.bench("main.onda", &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("benchmark build failed"));
}

#[test]
fn bench_rejects_invalid_counts_and_supports_help() {
    let overflow = usize::MAX.to_string();
    for args in [
        vec![],
        vec!["input.onda", "--runs=0"],
        vec!["input.onda", "--blocks=-1"],
        vec!["input.onda", "--warmup-blocks=1.5"],
        vec!["input.onda", "--runs", &overflow, "--blocks=2"],
        vec!["input.onda", "--set=bad"],
        vec!["input.onda", "--buffer=clip"],
        vec!["input.onda", "--runs"],
        vec!["input.onda", "--unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_onda"))
            .arg("bench")
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(!output.stderr.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_onda"))
        .args(["bench", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("onda bench <input>"));
}
