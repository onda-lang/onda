use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn playback_reports_project_resolution_failures_before_exiting() {
    for (name, manifest, source, asset, failed_file) in [
        ("manifest", "{", "", None, "project.ondaproject"),
        (
            "entry",
            r#"{"entry":"missing.onda"}"#,
            "",
            None,
            "missing.onda",
        ),
        (
            "parse",
            r#"{"entry":"main.onda","constants":{"Selected":1}}"#,
            "import missing\nconfig const Selected: i32 = 0\n",
            None,
            "missing.onda",
        ),
        (
            "constant",
            r#"{"entry":"main.onda","constants":{"Unknown":1}}"#,
            "config const Selected: i32 = 0\n",
            None,
            "main.onda",
        ),
        (
            "asset",
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.ondabuffer"}}}"#,
            "buffers:\n  clip: buffer<f32>\nsample:\n  out1 = clip[0]\n",
            Some(b"invalid".as_slice()),
            "clip.ondabuffer",
        ),
        (
            "missing_asset",
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.ondabuffer"}}}"#,
            "buffers:\n  clip: buffer<f32>\nsample:\n  out1 = clip[0]\n",
            None,
            "clip.ondabuffer",
        ),
    ] {
        let root = std::env::temp_dir().join(format!(
            "onda-playback-sources-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let input = root.join("project.ondaproject");
        fs::write(&input, manifest).unwrap();
        fs::write(root.join("main.onda"), source).unwrap();
        if let Some(asset) = asset {
            fs::write(root.join("clip.ondabuffer"), asset).unwrap();
        }
        for report_sources in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_onda"));
            command
                .args(["run", "play"])
                .arg(&input)
                .arg("--control-json");
            if report_sources {
                command.arg("--report-sources");
            }
            let output = command.output().unwrap();
            assert!(!output.status.success(), "{name}");
            assert!(!output.stderr.is_empty(), "{name}");
            if !report_sources {
                assert!(output.stdout.is_empty(), "{name}");
                continue;
            }
            let events = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(events.len(), 2, "{name}: discovery and failed reads");
            let failed = &events[1];
            assert_eq!(failed["event"], "sources", "{name}");
            assert_eq!(failed["complete"], false, "{name}");
            assert!(
                failed["files"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(root.join(failed_file))),
                "{name}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
