use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: &str = "params:\n  divisor: i32 = 0\ninit:\n  value = i32(1) / divisor\nsample:\n  out1 = f32(value)\n";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "onda-run-startup-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        fs::write(root.join("main.onda"), SOURCE).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn metadata_does_not_execute_faulting_initialization() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_onda"))
        .args(["daemon", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        json!({"id":1,"command":"run_metadata","path":fixture.0.join("main.onda")})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["params"][0]["name"], "divisor");
    assert_eq!(reply["result"]["params"][0]["defaultRepr"], "0");
}

#[test]
fn render_and_bench_apply_overrides_before_initialization() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("main.onda"), "params:\n  divisor: i32 = 0\nbuffers:\n  data: buffer<i32>\ninit:\n  value = i32(2) / divisor / data[0]\nsample:\n  out1 = f32(value)\n").unwrap();
    let asset =
        onda_project::BufferAsset::new(1, 1, 48_000.0, onda_project::BufferSamples::I32(vec![2]))
            .unwrap();
    let asset_path = fixture.0.join("data.ondabuffer");
    fs::write(
        &asset_path,
        onda_project::encode_ondabuffer(&asset).unwrap(),
    )
    .unwrap();
    let binding = format!("data={}", asset_path.display());
    let wav = fixture.0.join("render.wav");
    let output = Command::new(env!("CARGO_BIN_EXE_onda"))
        .args(["run", "render"])
        .arg(fixture.0.join("main.onda"))
        .args(["--dur", "0.01", "--set", "divisor=1", "--output"])
        .arg(&wav)
        .arg("--buffer")
        .arg(&binding)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut reader = hound::WavReader::open(wav).unwrap();
    assert!(reader
        .samples::<i16>()
        .all(|sample| sample.unwrap() == i16::MAX));
    let output = Command::new(env!("CARGO_BIN_EXE_onda"))
        .arg("bench")
        .arg(fixture.0.join("main.onda"))
        .args([
            "--runs=1",
            "--blocks=1",
            "--warmup-blocks=0",
            "--set",
            "divisor=1",
            "--json",
        ])
        .arg("--buffer")
        .arg(&binding)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["total_blocks"], 1);
}
