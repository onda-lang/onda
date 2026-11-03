use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn main_project_buffers_and_compile_metadata_use_authored_names() {
    let root = std::env::temp_dir().join(format!(
        "onda-main-project-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos(),
    ));
    fs::create_dir(&root).expect("create project directory");
    fs::write(
        root.join("main.onda"),
        r#"
proc Main:
  ins:
    signal: f32
    bus: f32[2]
  outs:
    output: f32
    channels: f32[2]
  params:
    gain = 0.5
    gains: f32[2] = [0.25, 0.75]
  buffers:
    data: f32
    bank: f32 {2}
  sample:
    output = data[0] * gain + signal
    channels[0] = bank[0][0] * gains[0] + bus[0]
    channels[1] = bank[1][0] * gains[1] + bus[1]
"#,
    )
    .expect("write project source");
    let asset = serde_json::json!({
        "inline": {
            "element": "f32",
            "channels": 1,
            "sample_rate": 48000,
            "values": [1.0]
        }
    });
    let project = root.join("main.ondaproject");
    fs::write(
        &project,
        serde_json::json!({
            "entry": "main.onda",
            "buffers": {"data": asset, "bank": [asset, asset]}
        })
        .to_string(),
    )
    .expect("write project manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_onda"))
        .arg("compile")
        .arg(&project)
        .arg("--meta")
        .output()
        .expect("compile Main project");
    fs::remove_dir_all(root).expect("remove project directory");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = String::from_utf8(output.stdout).expect("UTF-8 metadata");
    for (name, ty) in [
        ("signal", "f32"),
        ("bus", "f32[2]"),
        ("output", "f32"),
        ("channels", "f32[2]"),
        ("gain", "f32"),
        ("gains", "f32[2]"),
    ] {
        assert!(
            metadata.contains(&format!("name={name} type={ty} ")),
            "{metadata}"
        );
    }
    assert!(!metadata.contains("__onda_"), "{metadata}");
}
