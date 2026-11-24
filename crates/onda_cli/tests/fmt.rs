use std::{fs, process::Command};

#[test]
fn fmt_checks_writes_and_leaves_invalid_source_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.on");
    let source = "sample:\n    out1=1+2 # sum\n";
    fs::write(&path, source).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_onda"))
            .arg("fmt")
            .args(args)
            .arg(&path)
            .output()
            .unwrap()
    };

    assert_eq!(run(&["--check"]).status.code(), Some(1));
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    assert!(run(&[]).status.success());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "sample:\n  out1 = 1 + 2  # sum\n"
    );
    assert!(run(&["--check"]).status.success());

    let invalid = "import std/math\nsample:\n  out1 = (1 +\n";
    fs::write(&path, invalid).unwrap();
    let failed = run(&[]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("example.on:"));
    assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
}

#[test]
#[cfg(unix)]
fn fmt_preserves_the_file_behind_hard_links_and_open_handles() {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, PermissionsExt},
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.onda");
    let linked = dir.path().join("linked.onda");
    let source = "sample:\n    out1=0.0   \n";
    fs::write(&path, source).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let before = fs::metadata(&path).unwrap();
    fs::hard_link(&path, &linked).unwrap();
    let mut editor = fs::File::open(&path).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_onda"))
        .arg("fmt")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let formatted = "sample:\n  out1 = 0.0\n";
    assert_eq!(fs::read_to_string(&path).unwrap(), formatted);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.ino(), before.ino());
    assert_eq!(after.mode(), before.mode());
    assert_eq!(after.uid(), before.uid());
    assert_eq!(after.gid(), before.gid());
    assert_eq!(fs::read_to_string(&linked).unwrap(), formatted);
    let mut contents = String::new();
    editor.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, formatted);
}

#[test]
#[cfg(unix)]
fn fmt_formats_the_target_of_a_symbolic_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.onda");
    let linked = dir.path().join("linked.onda");
    fs::write(&path, "sample:\n    out1=0.0\n").unwrap();
    std::os::unix::fs::symlink("example.onda", &linked).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_onda"))
        .arg("fmt")
        .arg(&linked)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fs::symlink_metadata(&linked).unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "sample:\n  out1 = 0.0\n"
    );
}

#[test]
fn fmt_rejects_a_read_only_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("read-only.onda");
    let source = "sample:\n    out1=0.0\n";
    fs::write(&path, source).unwrap();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&path, read_only).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_onda"))
        .arg("fmt")
        .arg(&path)
        .output()
        .unwrap();
    fs::set_permissions(&path, original_permissions).unwrap();

    assert_eq!(result.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
}

#[test]
fn fmt_reports_parser_errors_without_rewriting_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.onda");
    for (source, message) in [
        (
            "sample:\n  out1=9223372036854775808\n",
            "integer literal is out of range",
        ),
        (
            "params:\n  x=1 {min=0,min=1}\nsample:\n  out1=x\n",
            "duplicate parameter domain field 'min'",
        ),
        (
            "const Value=1\nimport missing/module\nconst Value=2\n",
            "duplicate top-level constant 'Value'",
        ),
    ] {
        fs::write(&path, source).unwrap();
        for args in [["fmt"].as_slice(), &["fmt", "--check"]] {
            let result = Command::new(env!("CARGO_BIN_EXE_onda"))
                .args(args)
                .arg(&path)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(1));
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains(message), "{stderr}");
            assert_eq!(fs::read_to_string(&path).unwrap(), source);
        }
    }
}

#[test]
fn fmt_preserves_compilation_when_a_braced_header_needs_wrapping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("header.onda");
    let argument = "argument".repeat(14);
    fs::write(
        &path,
        format!(
            "def shift(x: i32, y: i32):\n  return x + y\nsample:\n  {argument} = 0\n  if (16 >> shift({argument}, 1)) > 0 {{\n    out1=1\n  }}\n"
        ),
    )
    .unwrap();
    let run = |arguments: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_onda"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    let filename = path.to_str().unwrap();
    run(&["compile", filename, "--emit", "check"]);
    run(&["fmt", filename]);
    assert!(fs::read_to_string(&path).unwrap().contains("    out1 = 1"));
    run(&["fmt", "--check", filename]);
    run(&["compile", filename, "--emit", "check"]);
}
