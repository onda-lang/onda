use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::{
    ArrayElemType, AssignTarget, BinaryOp, Block, BufferElemType, BuiltinFn, CallArg, CallTypeArg,
    ConstDecl, ConstType, DeclType, EventParamType, Expr, FieldType, FnParamType,
    FnReturnScalarType, FnReturnType, GraphEndpoint, GraphRate, LogicalOp, NamespaceItem,
    OutputTiming, ParamScale, PrimitiveType, Stmt, INTERNAL_BARE_RETURN_FN, INTERNAL_TASK_AWAIT_FN,
    INTERNAL_TASK_YIELD_FN,
};

use super::{
    load_program_file, load_program_file_from_snapshot, load_program_file_from_virtual_sources,
    parse_program, parse_program_file, parse_program_file_with_overlays, parse_program_with_path,
    rewrite_source_references, SourceReferenceKind, SourceReferenceRewrite, SourceResolution,
    UnresolvedSourceResolution, GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL,
    GRAPH_PROC_FIELD_INDEX_EXPR_ARG, METHOD_RECEIVER_ARG, PROC_FIELD_SENTINEL_ARG,
    PROC_FIELD_SENTINEL_PREFIX, PROC_INDEX_BASE_ARG, PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG,
};

fn mk_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("onda_frontend_{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, text).expect("write test file");
}

#[cfg(unix)]
#[test]
fn filesystem_source_loading_rejects_symlinks() {
    use std::os::unix::fs::symlink;

    let dir = mk_temp_dir("source_resolution_import_symlink");
    let main = dir.join("main.onda");
    let target = dir.join("implementation.on");
    let candidate = dir.join("module.onda");
    write_file(&main, "import module\n");
    write_file(&target, "const value = 1.0\n");
    symlink(&target, &candidate).expect("create import symlink");

    let error = load_program_file(&main).expect_err("source symlinks must be rejected");
    assert!(
        error.diagnostics[0].message.contains("symlink component"),
        "unexpected diagnostic: {}",
        error.diagnostics[0].message
    );

    let entry_alias = dir.join("entry.onda");
    symlink(&main, &entry_alias).expect("create entry symlink");
    let error = load_program_file(&entry_alias).expect_err("entry symlinks must be rejected");
    assert!(error.diagnostics[0].message.contains("symlink component"));

    let normalized_alias = dir.join("missing/../entry.onda");
    let error = super::ensure_no_symlink_components(&normalized_alias)
        .expect_err("a missing prefix must not hide a symlink after normalization");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    let outside = dir.join("outside");
    let nested = outside.join("nested");
    fs::create_dir_all(&nested).expect("create symlink traversal target");
    write_file(
        &outside.join("escaped.onda"),
        "outs 1\nsample:\n  out1 = 0.0\n",
    );
    let directory_alias = dir.join("directory_alias");
    symlink(&nested, &directory_alias).expect("create directory symlink");
    let traversing_entry = directory_alias.join("../escaped.onda");
    let error = load_program_file(&traversing_entry)
        .expect_err("parent components must not hide traversed symlinks");
    assert!(error.diagnostics[0].message.contains("symlink component"));

    write_file(&main, "include \"directory_alias/../escaped.onda\"\n");
    let error = load_program_file(&main)
        .expect_err("include normalization must not hide traversed symlinks");
    assert!(error.diagnostics[0].message.contains("symlink component"));

    write_file(&main, "import directory_alias/../escaped\n");
    let error = load_program_file(&main)
        .expect_err("import normalization must not hide traversed symlinks");
    assert!(error.diagnostics[0].message.contains("symlink component"));

    fs::remove_dir_all(dir).ok();
}

fn assert_deferred_int_count(expr: &Option<Expr>, expected: i64) {
    fn expr_int_value(expr: &Expr) -> Option<i64> {
        match expr {
            Expr::Int { value, .. } => Some(*value),
            Expr::Cast { expr, .. } => expr_int_value(expr),
            Expr::Binary { op, lhs, rhs, .. } => {
                let lhs = expr_int_value(lhs)?;
                let rhs = expr_int_value(rhs)?;
                match op {
                    BinaryOp::Add => Some(lhs + rhs),
                    BinaryOp::Sub => Some(lhs - rhs),
                    BinaryOp::Mul => Some(lhs * rhs),
                    BinaryOp::Div if rhs != 0 => Some(lhs / rhs),
                    BinaryOp::Mod if rhs != 0 => Some(lhs % rhs),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    assert!(
        expr.as_ref().and_then(expr_int_value) == Some(expected),
        "expected deferred count {expected}, got {expr:?}"
    );
}

#[test]
fn parse_program_file_supports_on_imports_and_includes() {
    let dir = mk_temp_dir("on_imports_and_includes");
    let main = dir.join("main.on");
    let dep = dir.join("dep.on");
    let lib = dir.join("lib.on");

    write_file(
        &main,
        r#"
include "./dep.on"
import lib
outs:
  out1
sample:
  out1 = dep_value + lib_value
"#,
    );
    write_file(
        &dep,
        r#"
const dep_value = 1.0
"#,
    );
    write_file(
        &lib,
        r#"
const lib_value = 2.0
"#,
    );

    let program = parse_program_file(&main).expect(".on program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    let Expr::Binary { lhs, rhs, .. } = expr else {
        panic!("expected binary expression");
    };
    assert!(matches!(
        lhs.as_ref(),
        Expr::Var { name, .. } if name == "dep_value"
    ));
    assert!(matches!(
        rhs.as_ref(),
        Expr::Var { name, .. } if name == "lib_value"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn source_manifest_tracks_entry_and_transitive_user_sources() {
    let dir = mk_temp_dir("source_manifest");
    let main = dir.join("main.onda");
    let included = dir.join("shared.onda");
    let imported = dir.join("dsp.onda");
    let nested = dir.join("nested.onda");

    write_file(
        &main,
        "include \"./shared.onda\"\nimport dsp\nimport std/math\nouts 1\nsample:\n  out1 = value\n",
    );
    write_file(&included, "import nested\nconst shared = 1.0\n");
    write_file(&nested, "const nested = 2.0\n");
    write_file(&imported, "const value = 3.0\n");

    let loaded = load_program_file(&main).expect("program should load");
    assert_eq!(
        loaded.sources.files,
        vec![
            fs::canonicalize(&main).expect("canonical entry"),
            fs::canonicalize(&included).expect("canonical include"),
            fs::canonicalize(&nested).expect("canonical nested import"),
            fs::canonicalize(&imported).expect("canonical import"),
        ]
    );
    assert_eq!(
        loaded
            .sources
            .documents
            .iter()
            .map(|document| document.contents.as_str())
            .collect::<Vec<_>>(),
        vec![
            "include \"./shared.onda\"\nimport dsp\nimport std/math\nouts 1\nsample:\n  out1 = value\n",
            "import nested\nconst shared = 1.0\n",
            "const nested = 2.0\n",
            "const value = 3.0\n",
        ]
    );
    assert_eq!(
        loaded.sources.resolutions,
        vec![
            SourceResolution {
                source: fs::canonicalize(&main).expect("canonical entry"),
                kind: SourceReferenceKind::Include,
                specifier: "./shared.onda".to_owned(),
                target: fs::canonicalize(&included).expect("canonical include"),
            },
            SourceResolution {
                source: fs::canonicalize(&included).expect("canonical include"),
                kind: SourceReferenceKind::Import,
                specifier: "nested".to_owned(),
                target: fs::canonicalize(&nested).expect("canonical nested import"),
            },
            SourceResolution {
                source: fs::canonicalize(&main).expect("canonical entry"),
                kind: SourceReferenceKind::Import,
                specifier: "dsp".to_owned(),
                target: fs::canonicalize(&imported).expect("canonical import"),
            },
        ]
        .into_boxed_slice()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn source_manifest_is_available_when_a_dependency_fails_to_parse() {
    let dir = mk_temp_dir("source_manifest_failure");
    let main = dir.join("main.onda");
    let imported = dir.join("dsp.onda");
    let nested = dir.join("nested.onda");

    write_file(&main, "import dsp\nouts 1\nsample:\n  out1 = 0.0\n");
    write_file(&imported, "import nested\nconst value = 1.0\n");
    write_file(&nested, "this is not valid onda\n");

    let error = load_program_file(&main).expect_err("nested source should fail");
    assert!(!error.diagnostics.is_empty());
    assert_eq!(
        error.sources.files,
        vec![
            fs::canonicalize(&main).expect("canonical entry"),
            fs::canonicalize(&imported).expect("canonical import"),
            fs::canonicalize(&nested).expect("canonical nested import"),
        ]
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn source_manifest_tracks_unresolved_user_source_candidates_separately() {
    let dir = mk_temp_dir("source_manifest_unresolved");
    let main = dir.join("main.onda");
    write_file(&main, "import missing/module\n");

    let error = load_program_file(&main).expect_err("missing import should fail");
    assert_eq!(
        error.sources.files,
        vec![fs::canonicalize(&main).expect("canonical entry")]
    );
    assert_eq!(
        error.sources.unresolved_files,
        vec![
            dir.join("missing/module.onda"),
            dir.join("missing/module.on"),
        ]
    );
    assert_eq!(
        error.sources.unresolved_resolutions,
        vec![UnresolvedSourceResolution {
            source: fs::canonicalize(&main).expect("canonical entry"),
            kind: SourceReferenceKind::Import,
            specifier: "missing/module".to_owned(),
            candidates: vec![
                dir.join("missing/module.onda"),
                dir.join("missing/module.on"),
            ],
        }]
        .into_boxed_slice()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn source_manifest_reports_the_exact_unresolved_include_candidate() {
    let dir = mk_temp_dir("source_manifest_unresolved_include");
    let main = dir.join("main.onda");
    write_file(&main, "include \"missing/shared.onda\"\n");

    let error = load_program_file(&main).expect_err("missing include should fail");
    assert_eq!(
        error.sources.unresolved_files,
        vec![dir.join("missing/shared.onda")]
    );
    assert_eq!(
        error.sources.unresolved_resolutions,
        vec![UnresolvedSourceResolution {
            source: fs::canonicalize(&main).expect("canonical entry"),
            kind: SourceReferenceKind::Include,
            specifier: "missing/shared.onda".to_owned(),
            candidates: vec![dir.join("missing/shared.onda")],
        }]
        .into_boxed_slice()
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn virtual_source_manifest_uses_normalized_project_paths() {
    let root = PathBuf::from("project");
    let sources = std::collections::HashMap::from([
        (
            root.join("main.onda"),
            "include \"./shared.onda\"\nimport dsp/filter\nouts 1\nsample:\n  out1 = value\n"
                .to_owned(),
        ),
        (root.join("shared.onda"), "const shared = 1.0\n".to_owned()),
        (
            root.join("dsp/filter.onda"),
            "const value = 2.0\n".to_owned(),
        ),
    ]);

    let loaded = load_program_file_from_virtual_sources(&root, &root.join("./main.onda"), &sources)
        .expect("virtual project should load");
    assert_eq!(
        loaded.sources.files,
        vec![
            root.join("main.onda"),
            root.join("shared.onda"),
            root.join("dsp/filter.onda"),
        ]
    );
}

#[test]
fn snapshot_replays_recorded_resolutions_without_filesystem_paths() {
    let entry = PathBuf::from("C:/original/project/main.onda");
    let dependency = PathBuf::from("/another-machine/shared/filter.onda");
    let sources = std::collections::HashMap::from([
        (
            entry.clone(),
            "include \"/absolute/shared/filter.onda\"\nouts 1\nsample:\n  out1 = value\n"
                .to_owned(),
        ),
        (dependency.clone(), "const value = 0.25\n".to_owned()),
    ]);
    let resolutions = [SourceResolution {
        source: entry.clone(),
        kind: SourceReferenceKind::Include,
        specifier: "/absolute/shared/filter.onda".to_owned(),
        target: dependency.clone(),
    }];

    let loaded = load_program_file_from_snapshot(&entry, &sources, &resolutions)
        .expect("recorded source graph should replay");
    assert_eq!(loaded.sources.files, vec![entry, dependency]);
    assert_eq!(loaded.sources.resolutions.as_ref(), resolutions.as_slice());
}

#[test]
fn rewrites_only_parsed_source_reference_specifiers() {
    let source = concat!(
        "# include \"leave-this.onda\"\r\n",
        "sample:\r\n",
        "  out1 = 0.0\r\n",
        "include \"old/shared.onda\" # preserve this comment\r\n",
        "import old/module\r\n",
        "import std/math\r\n",
    );
    let rewritten = rewrite_source_references(
        Path::new("C:/saved/main.onda"),
        source,
        &[
            SourceReferenceRewrite {
                kind: SourceReferenceKind::Include,
                specifier: "old/shared.onda".to_owned(),
                replacement: "external/shared.onda".to_owned(),
            },
            SourceReferenceRewrite {
                kind: SourceReferenceKind::Import,
                specifier: "old/module".to_owned(),
                replacement: "sources/module".to_owned(),
            },
        ],
    )
    .expect("rewrite source references");

    assert_eq!(
        rewritten,
        concat!(
            "# include \"leave-this.onda\"\r\n",
            "sample:\r\n",
            "  out1 = 0.0\r\n",
            "include \"external/shared.onda\" # preserve this comment\r\n",
            "import sources/module\r\n",
            "import std/math\r\n",
        )
    );
}

#[test]
fn source_reference_rewrite_rejects_an_incomplete_graph() {
    let error =
        rewrite_source_references(Path::new("main.onda"), "include \"dependency.onda\"\n", &[])
            .expect_err("missing replacement should fail");
    assert!(error[0]
        .message
        .contains("no replacement was provided for include"));
}

#[test]
fn parse_program_rejects_import_suffix_for_on_modules() {
    let err =
        parse_program("import lib.on\n").expect_err("import with .on suffix should be rejected");
    assert!(err.iter().any(|diag| diag
        .message
        .contains("import expects module path without '.onda' or '.on' suffix")));
}

#[test]
fn parse_program_rejects_non_onda_or_on_include_suffix() {
    let err = parse_program("include \"./lib.txt\"\n")
        .expect_err("include with unsupported suffix should be rejected");
    assert!(err.iter().any(|diag| diag
        .message
        .contains("include path must end with '.onda' or '.on'")));
}

#[test]
fn parses_gain_program() {
    let src = r#"
ins {
  in1
}
outs {
  out1
}
params {
  gain = 0.5
}
sample {
  out1 = in1 * gain
}
"#;

    let program = parse_program(src).expect("gain should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Ins(_))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Outs(_))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Params(_))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Sample(_))));
}

#[test]
fn parses_sine_program() {
    let src = r#"
outs {
  out1
}
params {
  freq = 440.0
}
init {
  phase = 0.0
}
sample {
  phase = phase + freq / 48000.0
  out1 = sin(phase)
}
"#;

    let program = parse_program(src).expect("sine should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Init(_))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Sample(_))));
}

#[test]
fn parses_one_pole_program() {
    let src = r#"
inputs {
  in1
}
outputs {
  out1
}
params {
  a = 0.1
}
init {
  z = 0.0
}
sample {
  z = z + a * (in1 - z)
  out1 = z
}
"#;

    let program = parse_program(src).expect("one-pole should parse");
    assert_eq!(program.blocks.len(), 5);
}

#[test]
fn parses_expr_precedence() {
    let src = r#"
outs {
  out1
}
sample {
  out1 = a + b * c
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("first statement should be assignment"),
    };
    match expr {
        Expr::Binary {
            op: BinaryOp::Add,
            lhs: _,
            rhs,
            ..
        } => match rhs.as_ref() {
            Expr::Binary {
                op: BinaryOp::Mul, ..
            } => {}
            _ => panic!("rhs should be multiplication"),
        },
        _ => panic!("top-level should be addition"),
    }
}

#[test]
fn prefix_operators_bind_tighter_than_every_infix_tier() {
    let src = r#"
outs 8
sample {
  out1 = -a + b
  out2 = -a * b
  out3 = !flag && other
  out4 = ~bits & mask
  out5 = -(a + b) * c
  out6 = !-a
  out7 = -f(a) * b
  out8 = ~values[i] & mask
}
"#;
    let program = parse_program(src).expect("prefix precedence program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    let assigned_expr = |index: usize| match &sample[index] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("statement {index} should be an assignment"),
    };
    let is_negated_var = |expr: &Expr, expected: &str| {
        matches!(
            expr,
            Expr::Binary {
                op: BinaryOp::Sub,
                lhs,
                rhs,
                ..
            } if matches!(lhs.as_ref(), Expr::Int { value, .. } if *value == 0)
                && matches!(rhs.as_ref(), Expr::Var { name, .. } if name == expected)
        )
    };

    assert!(matches!(
        assigned_expr(0),
        Expr::Binary {
            op: BinaryOp::Add,
            lhs,
            ..
        } if is_negated_var(lhs, "a")
    ));
    assert!(matches!(
        assigned_expr(1),
        Expr::Binary {
            op: BinaryOp::Mul,
            lhs,
            ..
        } if is_negated_var(lhs, "a")
    ));
    assert!(matches!(
        assigned_expr(2),
        Expr::Logical {
            op: LogicalOp::And,
            lhs,
            ..
        } if matches!(lhs.as_ref(), Expr::UnaryNot { expr, .. }
            if matches!(expr.as_ref(), Expr::Var { name, .. } if name == "flag"))
    ));
    assert!(matches!(
        assigned_expr(3),
        Expr::Binary {
            op: BinaryOp::BitAnd,
            lhs,
            ..
        } if matches!(lhs.as_ref(), Expr::UnaryBitNot { expr, .. }
            if matches!(expr.as_ref(), Expr::Var { name, .. } if name == "bits"))
    ));
    assert!(matches!(
        assigned_expr(4),
        Expr::Binary {
            op: BinaryOp::Mul,
            lhs,
            ..
        } if matches!(lhs.as_ref(), Expr::Binary {
            op: BinaryOp::Sub,
            rhs,
            ..
        } if matches!(rhs.as_ref(), Expr::Binary { op: BinaryOp::Add, .. }))
    ));
    assert!(matches!(
        assigned_expr(5),
        Expr::UnaryNot { expr, .. } if is_negated_var(expr, "a")
    ));
    assert!(matches!(
        assigned_expr(6),
        Expr::Binary {
            op: BinaryOp::Mul,
            lhs,
            ..
        } if matches!(lhs.as_ref(), Expr::Binary {
            op: BinaryOp::Sub,
            rhs,
            ..
        } if matches!(rhs.as_ref(), Expr::UserCall { name, .. } if name == "f"))
    ));
    assert!(matches!(
        assigned_expr(7),
        Expr::Binary {
            op: BinaryOp::BitAnd,
            lhs,
            ..
        } if matches!(lhs.as_ref(), Expr::UnaryBitNot { expr, .. }
            if matches!(expr.as_ref(), Expr::Index { base, .. } if base == "values"))
    ));
}

#[test]
fn negative_literals_preserve_their_numeric_kind() {
    let program = parse_program(
        r#"
init {
  integer = -1
  float = -1.5
}
sample { out1 = 0.0 }
"#,
    )
    .expect("negative literals should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(&init.body),
            _ => None,
        })
        .expect("init block");

    assert!(matches!(
        &init[0],
        Stmt::Assign {
            expr: Expr::Int { value: -1, .. },
            ..
        }
    ));
    assert!(matches!(
        &init[1],
        Stmt::Assign {
            expr: Expr::Number { value, .. },
            ..
        } if (*value + 1.5).abs() < f64::EPSILON
    ));
}

#[test]
fn parses_modulo_with_mul_div_precedence() {
    let src = r#"
outs {
  out1
}
sample {
  out1 = a + b % c * d
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("first statement should be assignment"),
    };
    let Expr::Binary {
        op: BinaryOp::Add,
        rhs,
        ..
    } = expr
    else {
        panic!("top-level should be addition");
    };
    let Expr::Binary {
        op: BinaryOp::Mul,
        lhs: mul_lhs,
        ..
    } = rhs.as_ref()
    else {
        panic!("rhs should be multiplication");
    };
    let Expr::Binary {
        op: BinaryOp::Mod, ..
    } = mul_lhs.as_ref()
    else {
        panic!("left side of multiplication should be modulo");
    };
}

#[test]
fn parses_bitwise_precedence() {
    let src = r#"
outs {
  out1
}
sample {
  out1 = a | b & c << d
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("first statement should be assignment"),
    };
    let Expr::Binary {
        op: BinaryOp::BitOr,
        rhs,
        ..
    } = expr
    else {
        panic!("top-level should be bitwise or");
    };
    let Expr::Binary {
        op: BinaryOp::BitAnd,
        rhs: and_rhs,
        ..
    } = rhs.as_ref()
    else {
        panic!("rhs should be bitwise and");
    };
    let Expr::Binary {
        op: BinaryOp::ShiftLeft,
        ..
    } = and_rhs.as_ref()
    else {
        panic!("right side of bitwise and should be shift-left");
    };
}

#[test]
fn parses_unary_bit_not_expression() {
    let src = r#"
outs {
  out1
}
sample {
  out1 = ~a
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("first statement should be assignment"),
    };
    match expr {
        Expr::UnaryBitNot { .. } => {}
        _ => panic!("expression should parse as unary bit-not"),
    }
}

#[test]
fn rejects_top_level_assert_block() {
    let src = r#"
assert(1 < 2)
outs {
  out1
}
sample {
  out1 = 0.0
}
"#;
    assert!(
        parse_program(src).is_err(),
        "top-level assert should be rejected"
    );
}

#[test]
fn parses_namespaced_assert_after_template_instantiation() {
    let src = r#"
namespace FFT<N = 4> {
  assert((N & (N - 1)) == 0)
  struct Tag {
    value
  }
}
outs { out1 }
init {
  tag: FFT<8>::Tag
}
sample {
  out1 = 0.0
}
"#;

    let program = parse_program(src).expect("program should parse");
    let ns = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "FFT" => Some(ns),
            _ => None,
        })
        .expect("FFT namespace");
    assert!(ns
        .items
        .iter()
        .any(|item| matches!(item, NamespaceItem::Assert(_))));
    assert!(
        ns.items
            .iter()
            .any(|item| matches!(item, NamespaceItem::Struct(s) if s.name == "Tag")),
        "expected namespaced struct item"
    );
}

#[test]
fn parses_sin_call_expression() {
    let src = r#"
outs {
  out1
}
sample {
  out1 = sin(a + b)
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("first statement should be assignment"),
    };
    match expr {
        Expr::Call { .. } => {}
        _ => panic!("top-level should be a function call"),
    }
}

#[test]
fn parses_variadic_builtin_call_expression() {
    let src = r#"
outs {
  out1
}
sample {
  out1 = fma(a, b, c)
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        _ => panic!("first statement should be assignment"),
    };
    match expr {
        Expr::Call {
            func: BuiltinFn::Fma,
            args,
            ..
        } => assert_eq!(args.len(), 3),
        _ => panic!("top-level should be an fma builtin call"),
    }
}

#[test]
fn parses_multiline_method_and_function_calls() {
    let src = r#"
outs { out1 }
struct Pair {
  a: f32
  b: f32
  def set(self, a, b) {
    self.a = a
    self.b = b
  }
}
init {
  p = Pair()
}
sample {
  p.set(
    1.0,
    2.0,
  )
  out1 = max(
    p.a,
    p.b,
  )
}
"#;
    let program = parse_program(src).expect("multiline calls should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert!(matches!(
        &sample[0],
        Stmt::Expr {
            expr: Expr::UserCall { .. },
            ..
        }
    ));
    assert!(matches!(
        &sample[1],
        Stmt::Assign {
            expr: Expr::Call { .. },
            ..
        }
    ));
}

#[test]
fn parses_multiline_named_calls_in_indentation_blocks() {
    let src = r#"
import std/env
outs:
  out1
init:
  env = std::env::DecayEnv(
    decay_s = 0.05,
    trigger = 1.0,
  )
sample:
  out1 = env()
"#;
    let program = parse_program(src).expect("multiline named calls should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(block) => Some(block),
            _ => None,
        })
        .expect("init block");
    match &init.body[0] {
        Stmt::Assign {
            expr: Expr::UserCall { args, .. },
            ..
        } => {
            assert_eq!(args.len(), 2);
            assert_eq!(args[0].name.as_deref(), Some("decay_s"));
            assert_eq!(args[1].name.as_deref(), Some("trigger"));
        }
        other => panic!("expected init call assignment, got {other:?}"),
    }
}

#[test]
fn parses_semicolon_separated_statements() {
    let src = r#"
outs { out1 }
params { x = 1.0; y = 2.0 }
sample { out1 = x; out1 = out1 + y }
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 2);
}

#[test]
fn parses_if_statement() {
    let src = r#"
outs { out1 }
sample {
  if (x > 0.0) { out1 = x } else { out1 = 0.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[0] {
        Stmt::If { .. } => {}
        _ => panic!("expected if statement"),
    }
}

#[test]
fn parses_if_elif_else_statement_as_nested_if() {
    let src = r#"
outs { out1 }
sample {
  if (x > 0.0) { out1 = 1.0 } elif (x > -1.0) { out1 = 0.5 } else { out1 = 0.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::If { else_branch, .. } = &sample[0] else {
        panic!("expected top-level if statement");
    };
    assert_eq!(else_branch.len(), 1, "expected single nested elif");
    let Stmt::If {
        else_branch: nested_else,
        ..
    } = &else_branch[0]
    else {
        panic!("expected nested if for elif");
    };
    assert_eq!(nested_else.len(), 1, "expected trailing else branch");
}

#[test]
fn parses_if_elif_else_without_parentheses() {
    let src = r#"
outs { out1 }
sample {
  if x > 0.0 { out1 = 1.0 } elif x > -1.0 { out1 = 0.5 } else { out1 = 0.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::If { else_branch, .. } = &sample[0] else {
        panic!("expected top-level if statement");
    };
    assert_eq!(else_branch.len(), 1, "expected single nested elif");
    let Stmt::If {
        else_branch: nested_else,
        ..
    } = &else_branch[0]
    else {
        panic!("expected nested if for elif");
    };
    assert_eq!(nested_else.len(), 1, "expected trailing else branch");
}

#[test]
fn parses_for_statement() {
    let src = r#"
outs { out1 }
sample {
  for i in 0..4 { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[0] {
        Stmt::For { .. } => {}
        _ => panic!("expected for statement"),
    }
}

#[test]
fn parses_for_statement_with_variable_bound() {
    let src = r#"
outs { out1 }
sample {
  n = 4
  for i in 0..n { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[1] {
        Stmt::For { start, end, .. } => {
            assert!(matches!(start, Expr::Int { value: 0, .. }));
            assert!(matches!(end, Expr::Var { name: v, .. } if v == "n"));
        }
        _ => panic!("expected for statement"),
    }
}

#[test]
fn parses_for_statement_with_parenthesized_expression_bound() {
    let src = r#"
outs { out1 }
sample {
  n = 5
  for i in 0..(n - 1) { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[1] {
        Stmt::For { end, .. } => {
            assert!(matches!(end, Expr::Binary { .. }));
        }
        _ => panic!("expected for statement"),
    }
}

#[test]
fn parses_for_statement_with_explicit_step_prefix() {
    let src = r#"
outs { out1 }
sample {
  for i @ -1 in 10..0 { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[0] {
        Stmt::For {
            step,
            start,
            end,
            end_inclusive,
            ..
        } => {
            assert!(matches!(step, Some(Expr::Int { value: -1, .. })));
            assert!(matches!(start, Expr::Int { value: 10, .. }));
            assert!(matches!(end, Expr::Int { value: 0, .. }));
            assert!(!end_inclusive);
        }
        _ => panic!("expected for statement"),
    }
}

#[test]
fn parses_explicit_for_induction_type() {
    let src = r#"
outs { out1 }
sample {
  for i: i64 in 0..2 { out1 = out1 + f32(i) }
}
"#;
    let program = parse_program(src).expect("typed for loop should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::For { var, var_ty, .. } = &sample[0] else {
        panic!("expected for statement");
    };
    assert_eq!(var, "i");
    assert_eq!(*var_ty, PrimitiveType::I64);
}

#[test]
fn defaults_for_induction_type_to_i32() {
    let src = r#"
outs { out1 }
sample {
  for i in 0..2 { out1 = out1 + f32(i) }
}
"#;
    let program = parse_program(src).expect("default for loop should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::For { var_ty, .. } = &sample[0] else {
        panic!("expected for statement");
    };
    assert_eq!(*var_ty, PrimitiveType::I32);
}

#[test]
fn rejects_non_integer_for_induction_type() {
    let errors = parse_program(
        r#"
sample {
  for i: f64 in 0..2 { value = i }
}
"#,
    )
    .expect_err("floating-point for induction should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("must be i32 or i64")));
}

#[test]
fn parses_for_statement_with_inclusive_end() {
    let src = r#"
outs { out1 }
sample {
  for i in 0..=4 { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[0] {
        Stmt::For { end_inclusive, .. } => assert!(*end_inclusive),
        _ => panic!("expected for statement"),
    }
}

#[test]
fn parses_loop_statement_as_for_sugar() {
    let src = r#"
outs { out1 }
sample {
  loop 4 { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[0] {
        Stmt::For {
            var, start, end, ..
        } => {
            assert_eq!(var, "_");
            assert!(matches!(start, Expr::Int { value: 0, .. }));
            assert!(matches!(end, Expr::Int { value: 4, .. }));
        }
        _ => panic!("expected for statement from loop sugar"),
    }
}

#[test]
fn parses_while_statement() {
    let src = r#"
outs { out1 }
sample {
  while x < 4 { out1 = out1 + 1.0 }
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    match &sample[0] {
        Stmt::While { .. } => {}
        _ => panic!("expected while statement"),
    }
}

#[test]
fn parses_break_and_continue_statements() {
    let src = r#"
outs { out1 }
sample {
  for i in 0..8 {
    if i < 2 { continue } else { break }
  }
  out1 = 0.0
}
"#;

    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::For { body, .. } = &sample[0] else {
        panic!("expected for statement");
    };
    let Stmt::If {
        then_branch,
        else_branch,
        ..
    } = &body[0]
    else {
        panic!("expected if in for body");
    };
    assert!(matches!(then_branch[0], Stmt::Continue { .. }));
    assert!(matches!(else_branch[0], Stmt::Break { .. }));
}

#[test]
fn parses_grouped_and_standalone_proc_tasks() {
    let src = r#"
proc Loader:
  tasks:
    load():
      for i in 0..4:
        yield
      return

  task clear():
    return

  block:
    await load()
    sample:
      out1 = 0.0
"#;
    let program = parse_program(src).expect("tasks should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("loader proc");

    assert_eq!(proc.tasks.len(), 2);
    assert_eq!(proc.tasks[0].name, "load");
    assert!(matches!(proc.tasks[0].body[0], Stmt::For { .. }));
    assert!(matches!(
        proc.tasks[0].body[1],
        Stmt::Return {
            expr: Expr::UserCall { ref name, .. },
            ..
        } if name == INTERNAL_BARE_RETURN_FN
    ));
    assert_eq!(proc.tasks[1].name, "clear");
    assert!(matches!(
        proc.block_pre[0],
        Stmt::Expr {
            expr: Expr::UserCall { ref name, .. },
            ..
        } if name == INTERNAL_TASK_AWAIT_FN
    ));
}

#[test]
fn rejects_duplicate_proc_tasks_across_declaration_forms() {
    let src = r#"
proc Loader:
  tasks:
    load():
      yield
  task load():
    return
  sample:
    out1 = 0.0
"#;
    let diagnostics = parse_program(src).expect_err("duplicate tasks should fail");
    assert!(diagnostics
        .iter()
        .any(|diag| diag.message.contains("duplicate task declaration 'load'")));
}

#[test]
fn parses_grouped_and_standalone_top_level_tasks() {
    let src = r#"
tasks:
  load():
    yield

task clear():
  return

block:
  await load()
  sample:
    out1 = 0.0
"#;
    let program = parse_program(src).expect("top-level tasks should parse");
    let tasks = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Tasks(tasks) => Some(tasks),
            _ => None,
        })
        .expect("top-level task block");

    assert_eq!(tasks.tasks.len(), 2);
    assert_eq!(tasks.tasks[0].name, "load");
    assert!(matches!(
        tasks.tasks[0].body[0],
        Stmt::Expr {
            expr: Expr::UserCall { ref name, .. },
            ..
        } if name == INTERNAL_TASK_YIELD_FN
    ));
    assert_eq!(tasks.tasks[1].name, "clear");
}

#[test]
fn rejects_duplicate_top_level_tasks_across_declaration_forms() {
    let src = r#"
tasks:
  load():
    yield
task load():
  return
"#;
    let diagnostics = parse_program(src).expect_err("duplicate tasks should fail");
    assert!(diagnostics
        .iter()
        .any(|diag| diag.message.contains("duplicate task declaration 'load'")));
}

#[test]
fn rejects_task_parameters() {
    assert!(parse_program("proc P:\n  task load(x):\n    return\n").is_err());
    assert!(parse_program("task load(x):\n  return\n").is_err());
}

#[test]
fn parses_indentation_while_statement() {
    let src = r#"
outs:
  out1
sample:
  while x < 4:
    out1 = out1 + 1.0
"#;
    let program = parse_program(src).expect("indentation while should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert!(matches!(sample[0], Stmt::While { .. }));
}

#[test]
fn rejects_reserved_keywords_as_identifiers() {
    let keywords = [
        "if", "elif", "else", "for", "in", "while", "loop", "break", "continue", "return", "await",
        "yield", "task", "tasks", "assert", "import", "include", "use", "as", "pub", "private",
        "pin", "config", "true", "false",
    ];

    for keyword in keywords {
        let src = format!("params:\n  {keyword} = 1.0\nsample:\n  out1 = 0.0\n");
        assert!(
            parse_program(&src).is_err(),
            "reserved keyword '{keyword}' should not parse as an identifier"
        );
    }
}

#[test]
fn rejects_compiler_reserved_identifier_prefix() {
    let cases = [
        "const __onda_value = 1.0\n",
        "struct __onda_Struct:\n  value: f32\n",
        "proc __onda_Proc:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\n",
        "namespace __onda_namespace:\n  const value = 1.0\n",
        "params:\n  __onda_param = 1.0\n",
        "events:\n  __onda_event():\n    value = 1.0\n",
        "event set(__onda_value: f32):\n  value = __onda_value\n",
        "def identity(__onda_value: f32):\n  return __onda_value\n",
        "struct Box:\n  __onda_field: f32\n",
        "sample:\n  __onda_local = 1.0\n",
        "sample:\n  for __onda_index in 0..1:\n    value = 1.0\n",
    ];

    for src in cases {
        assert!(
            parse_program(src).is_err(),
            "the '__onda_' prefix should be reserved, source parsed: {src}"
        );
    }
}

#[test]
fn rejects_pin_keyword_as_identifier() {
    let cases = [
        "params:\n  private = 1.0\n",
        "params:\n  private gain = 1.0\n",
        "sample:\n  pin = 1.0\n",
        "proc Voice:\n  params:\n    private = 1.0\n  outs:\n    out1\n  sample:\n    out1 = 0.0\n",
    ];

    for src in cases {
        assert!(
            parse_program(src).is_err(),
            "modifier keywords should remain reserved, source parsed: {src}"
        );
    }
}

#[test]
fn parses_def_and_call() {
    let src = r#"
outs { out1 }
def add2(a, b) {
  x = a + b
  return x
}
sample {
  out1 = add2(0.25, 0.5)
}
"#;
    let program = parse_program(src).expect("program should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
}

#[test]
fn parses_struct_and_field_access() {
    let src = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair(0.5, 0.25)
}
sample {
  out1 = p.a + p.b
}
"#;
    let program = parse_program(src).expect("program should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
}

#[test]
fn parses_struct_methods_after_fields() {
    let src = r#"
outs { out1 }
struct Voice {
  phase: f32
  gain: f32
  def tick(self, freq) {
    self.phase = self.phase + freq
    return self.phase * self.gain
  }
}
init {
  v = Voice(0.0, 0.5)
}
sample {
  out1 = Voice.tick(v, 1.0)
}
"#;
    let program = parse_program(src).expect("program should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");
    assert_eq!(st.methods.len(), 1);
    assert_eq!(st.methods[0].name, "tick");
}

#[test]
fn parses_struct_fields_without_explicit_type_as_f32() {
    let src = r#"
outs { out1 }
struct Tap {
  delay_samples
  gain
}
init {
  t = Tap()
}
sample {
  out1 = t.delay_samples + t.gain
}
"#;
    let program = parse_program(src).expect("program should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");
    assert_eq!(st.fields.len(), 2);
    assert!(matches!(
        st.fields[0].ty,
        FieldType::Scalar(crate::ast::PrimitiveType::F32)
    ));
    assert!(matches!(
        st.fields[1].ty,
        FieldType::Scalar(crate::ast::PrimitiveType::F32)
    ));
}

#[test]
fn infers_struct_field_type_from_default_when_untyped() {
    let src = r#"
outs { out1 }
struct X {
  field1 = 0.0
  field2 = 0
  field3: f64 = 0.0
  field4: i64 = 0
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("program should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");
    assert_eq!(st.fields.len(), 4);
    assert!(matches!(
        st.fields[0].ty,
        FieldType::Scalar(crate::ast::PrimitiveType::F32)
    ));
    assert!(matches!(
        st.fields[1].ty,
        FieldType::Scalar(crate::ast::PrimitiveType::I32)
    ));
    assert!(matches!(
        st.fields[2].ty,
        FieldType::Scalar(crate::ast::PrimitiveType::F64)
    ));
    assert!(matches!(
        st.fields[3].ty,
        FieldType::Scalar(crate::ast::PrimitiveType::I64)
    ));
}

#[test]
fn parses_array_ctor_and_index_access() {
    let src = r#"
outs { out1 }
init {
  buf: f32[8]
}
sample {
  out1 = buf[1.5]
  buf[2] = out1
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 2);
}

#[test]
fn parses_slice_expressions_with_omitted_and_negative_bounds() {
    let src = r#"
outs { out1 }
init {
  buf: f32[8]
}
sample {
  a = buf[:]
  b = buf[2:]
  c = buf[:-1]
  d = buf[1:-2]
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 4);

    let slice_exprs = sample
        .iter()
        .map(|stmt| match stmt {
            Stmt::Assign { expr, .. } => expr,
            other => panic!("expected assignment stmt, got {other:?}"),
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        slice_exprs[0],
        Expr::Slice {
            base,
            start: None,
            end: None,
            ..
        } if base == "buf"
    ));
    assert!(matches!(
        slice_exprs[1],
        Expr::Slice {
            base,
            start: Some(_),
            end: None,
            ..
        } if base == "buf"
    ));
    assert!(matches!(
        slice_exprs[2],
        Expr::Slice {
            base,
            start: None,
            end: Some(_),
            ..
        } if base == "buf"
    ));
    assert!(matches!(
        slice_exprs[3],
        Expr::Slice {
            base,
            start: Some(_),
            end: Some(_),
            ..
        } if base == "buf"
    ));
}

#[test]
fn parses_slice_assignment_targets() {
    let src = r#"
outs { out1 }
init {
  buf: f32[8]
}
sample {
  buf[1:-1] = 0.0
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 1);
    match &sample[0] {
        Stmt::Assign {
            target_loc,
            target,
            expr,
            ..
        } => {
            let target_loc = target_loc.as_ref().expect("slice target location");
            assert_eq!((target_loc.line, target_loc.column), (7, 3));
            assert_eq!(target_loc.end_line(), 7);
            match target {
                AssignTarget::Slice {
                    base, start, end, ..
                } => {
                    assert_eq!(base, "buf");
                    assert!(start.is_some());
                    assert!(end.is_some());
                }
                other => panic!("expected slice assignment target, got {other:?}"),
            }
            assert!(matches!(
                expr,
                Expr::Number { value: v, .. } if (*v - 0.0).abs() <= 1e-6
            ));
        }
        other => panic!("expected assignment stmt, got {other:?}"),
    }
}

#[test]
fn parses_call_statement() {
    let src = r#"
outs { out1 }
struct Voice {
  phase: f32
  def process(self) {
    self.phase = self.phase + 1.0
  }
}

init {
  v = Voice(0.0)
}
sample {
  v.process()
  out1 = v.phase
}
"#;
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 2);
    match &sample[0] {
        Stmt::Expr { .. } => {}
        _ => panic!("first statement should be call expression statement"),
    }
}

#[test]
fn parses_print_statements_and_decodes_labels() {
    let program = parse_program(
        "outs:\n  out1\nsample:\n  print()\n  print(value)\n  print(\"line\\n\\t\\\"\\\\\")\n  print(\"value\", value, true)\n  out1 = 0.0\n",
    )
    .expect("print statements should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(statements) => Some(statements),
            _ => None,
        })
        .expect("sample block");
    assert!(matches!(
        &sample[0],
        Stmt::Print { label: None, values, .. } if values.is_empty()
    ));
    assert!(matches!(
        &sample[1],
        Stmt::Print { label: None, values, .. } if values.len() == 1
    ));
    assert!(matches!(
        &sample[2],
        Stmt::Print { label: Some(label), values, .. }
            if label == "line\n\t\"\\" && values.is_empty()
    ));
    assert!(matches!(
        &sample[3],
        Stmt::Print { label: Some(label), values, .. }
            if label == "value" && values.len() == 2
    ));
}

#[test]
fn print_is_not_an_expression_or_declarable_name() {
    assert!(parse_program("sample:\n  value = print(1)\n").is_err());
    assert!(parse_program("def print(value):\n  return value\n").is_err());
}

#[test]
fn parses_proc_block() {
    let src = r#"
proc Gain {
  ins { in1 }
  params { gain = 2.0 }
  outs { out1 }
  init { }
  sample { out1 = in1 * gain }
}
outs { out1 }
init { p = Gain() }
sample { out1 = p(0.5) }
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");
    assert_eq!(proc.name, "Gain");
    assert_eq!(proc.ins.len(), 1);
    assert_eq!(proc.outs.len(), 1);
    assert_eq!(proc.params.len(), 1);
}

#[test]
fn preserves_proc_level_consts_for_semantics() {
    let src = r#"
proc Voice {
  const N = 2
  const Z = 1
  ins N
  outs N
  sample {
    out1 = in1
    out2 = f32(Z)
  }
}
outs { out1 }
sample { out1 = 0.0 }
"#;

    let program = parse_program(src).expect("proc-level consts should parse and rewrite");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");

    assert_eq!(proc.consts.len(), 2, "proc consts should be retained");
    assert_eq!(proc.consts[0].name, "N");
    assert_eq!(proc.consts[1].name, "Z");
    assert!(proc.ins.is_empty());
    assert!(proc.outs.is_empty());
    assert!(matches!(
        proc.ins_deferred_count.as_ref(),
        Some(Expr::Var { name, .. }) if name == "N"
    ));
    assert!(matches!(
        proc.outs_deferred_count.as_ref(),
        Some(Expr::Var { name, .. }) if name == "N"
    ));
    assert!(matches!(
        &proc.sample[1],
        Stmt::Assign {
            expr:
                Expr::Cast {
                    expr,
                    to: PrimitiveType::F32,
                    ..
                },
            ..
        } if matches!(&**expr, Expr::Var { name, .. } if name == "Z")
    ));
}

#[test]
fn preserves_proc_level_consts_using_namespace_consts_for_semantics() {
    let src = r#"
namespace Synth<N = 2> {
  const Base = N + 1

  proc Voice {
    const Count = Base + 1
    ins Count
    outs Count
    sample {
      out1 = in1
      out2 = in2
      out3 = f32(Count)
      out4 = f32(Base)
    }
  }
}
outs { out1 }
init { v = Synth<2>::Voice() }
sample {
  v(1.0, 2.0, 3.0, 4.0)
  out1 = v.out1
}
"#;

    let program =
        parse_program(src).expect("proc-level consts should be able to reference namespace consts");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Synth" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Proc(p) if p.name == "Voice" => Some(p),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("expected namespaced proc block");

    assert_eq!(proc.consts.len(), 1, "proc consts should be retained");
    assert_eq!(proc.consts[0].name, "Count");
    assert!(matches!(
        &proc.consts[0].expr,
        Expr::Binary { op: BinaryOp::Add, lhs, rhs, .. }
            if matches!(lhs.as_ref(), Expr::Var { name, .. } if name == "Base")
                && matches!(rhs.as_ref(), Expr::Int { value: 1, .. })
    ));
    assert!(proc.ins.is_empty());
    assert!(proc.outs.is_empty());
    assert!(matches!(
        proc.ins_deferred_count.as_ref(),
        Some(Expr::Var { name, .. }) if name == "Count"
    ));
    assert!(matches!(
        proc.outs_deferred_count.as_ref(),
        Some(Expr::Var { name, .. }) if name == "Count"
    ));
    assert!(
        stmt_contains_var_with_suffix(&proc.sample[2], "Count"),
        "instantiated proc body should retain proc-local const symbols for semantics"
    );
    assert!(
        stmt_contains_var_with_suffix(&proc.sample[3], "Base"),
        "instantiated proc body should retain namespace const symbols for semantics"
    );
}

#[test]
fn parses_proc_block_wrapping_sample() {
    let src = r#"
proc Wrapped {
  ins { in1 }
  outs { out1 }
  block {
    acc = 1.0
    sample { out1 = in1 }
    acc = acc + 1.0
  }
}
sample { out1 = 0.0 }
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");
    assert!(proc.has_block_block);
    assert!(proc.has_sample_block);
    assert_eq!(proc.block_pre.len(), 1);
    assert_eq!(proc.sample.len(), 1);
    assert_eq!(proc.block_post.len(), 1);
}

#[test]
fn parses_proc_block_without_nested_sample_for_semantic_validation() {
    let src = r#"
proc Wrapped {
  outs { out1 }
  block {
    x = 1.0
  }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");

    assert!(proc.has_block_block);
    assert!(!proc.has_sample_block);
    assert_eq!(proc.block_pre.len(), 1);
    assert!(proc.sample.is_empty());
    assert!(proc.block_post.is_empty());
}

#[test]
fn parses_proc_block_wrapping_sample_with_indentation_syntax() {
    let src = r#"
proc Wrapped:
  ins:
    in1
  outs:
    out1
  block:
    acc = 1.0
    sample:
      out1 = in1 * acc
    acc = acc + 1.0
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");
    assert!(proc.has_block_block);
    assert_eq!(proc.block_pre.len(), 1);
    assert_eq!(proc.sample.len(), 1);
    assert_eq!(proc.block_post.len(), 1);
}

#[test]
fn parses_typed_top_level_ports_and_params() {
    let src = r#"
ins { in1: i32, in2 }
outs { out1: f64, out2 }
params { gain: f64 = 2.5, mode: i32 = 2, gate: bool = 1 }
sample { out1 = in1 * gain; out2 = mode + gate }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let ins = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Ins(v) => Some(v),
            _ => None,
        })
        .expect("ins block");
    assert_eq!(ins[0].name, "in1");
    assert_eq!(ins[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
    assert_eq!(ins[1].name, "in2");
    assert_eq!(ins[1].ty, None);

    let outs = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Outs(v) => Some(v),
            _ => None,
        })
        .expect("outs block");
    assert_eq!(outs[0].name, "out1");
    assert_eq!(outs[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
    assert_eq!(outs[1].name, "out2");
    assert_eq!(outs[1].ty, None);

    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");
    assert_eq!(params[0].name, "gain");
    assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
    assert_eq!(params[1].name, "mode");
    assert_eq!(params[1].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
    assert_eq!(params[2].name, "gate");
    assert_eq!(params[2].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
}

#[test]
fn parses_ranges_and_count_prefix_with_explicit_lists() {
    let src = r#"
ins 2:
  in1 = 440 {22000}
  in2 = 440 {0.01, 22000}
outs 1
params 2:
  freq: i32 = 500 {8000}
  mix = 0.5 {0.0, 1.0}
sample:
  out1 = in1 + in2 + freq + mix
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let ins = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Ins(v) => Some(v),
            _ => None,
        })
        .expect("ins block");
    assert_eq!(ins.len(), 2);
    assert_eq!(ins[0].name, "in1");
    assert!(matches!(
        ins[0].default,
        Some(Expr::Number { .. }) | Some(Expr::Int { .. })
    ));
    let in1_range = ins[0].range.as_ref().expect("in1 range should be parsed");
    assert!(in1_range.min.is_none());
    assert!(matches!(in1_range.max, Expr::Int { value: 22000, .. }));
    let in2_range = ins[1].range.as_ref().expect("in2 range should be parsed");
    assert!(in2_range.min.is_some());
    assert!(matches!(
        in2_range.max,
        Expr::Number { .. } | Expr::Int { value: 22000, .. }
    ));

    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "freq");
    assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
    assert!(matches!(
        params[0].default,
        Some(Expr::Int { value: 500, .. })
    ));
    let freq_range = params[0]
        .range
        .as_ref()
        .expect("freq range should be parsed");
    assert!(freq_range.min.is_none());
    assert!(matches!(freq_range.max, Expr::Int { value: 8000, .. }));
    assert_eq!(params[1].name, "mix");
    let mix_range = params[1]
        .range
        .as_ref()
        .expect("mix range should be parsed");
    assert!(mix_range.min.is_some());
    assert!(matches!(
        mix_range.max,
        Expr::Number { .. } | Expr::Int { value: 1, .. }
    ));
}

#[test]
fn parses_top_level_parameter_domains() {
    let src = r#"
params {
  cutoff = 440.0 {20, 20000, log, "Hz"}
  voices: i32 = 4 {0, 10, step = 2, unit = "voices"}
  mix = 0.5 {unit = "%", curve = -4, scale = linear, max = 1, min = 0}
  ceiling = 1.0 {max = 2}
}
sample {
  out1 = cutoff + voices + mix + ceiling
}
"#;
    let program = parse_program(src).expect("parameter domains should parse");
    let params = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => Some(params),
            _ => None,
        })
        .expect("params block");

    assert_eq!(params[0].control.scale, ParamScale::Log);
    assert_eq!(params[0].control.unit.as_deref(), Some("Hz"));
    assert!(params[0].control.step.is_none());
    assert!(params[0].range.as_ref().unwrap().min.is_some());

    assert_eq!(params[1].control.scale, ParamScale::Linear);
    assert_eq!(params[1].control.unit.as_deref(), Some("voices"));
    assert!(matches!(
        params[1].control.step,
        Some(Expr::Int { value: 2, .. })
    ));

    assert_eq!(params[2].control.scale, ParamScale::Linear);
    assert!(matches!(
        params[2].control.curve,
        Some(Expr::Int { value: -4, .. })
    ));
    assert_eq!(params[2].control.unit.as_deref(), Some("%"));
    assert!(params[3].range.as_ref().unwrap().min.is_none());
}

#[test]
fn rejects_invalid_parameter_domain_shapes() {
    for src in [
        "params { p = 1 {0, max = 2, 3} }\n",
        "params { p = 1 {0, 2, scale = log, scale = linear} }\n",
        "params { p = 1 {0, 2, curve = 1, curve = -1} }\n",
        "params { p = 1 {min = 0} }\n",
        "params { p = 1 {0, 2, unit = Hz} }\n",
        "params { p = 1 {0, 2, mystery = 1} }\n",
    ] {
        assert!(parse_program(src).is_err(), "source should fail: {src}");
    }
}

#[test]
fn parameter_scale_words_remain_valid_range_expressions() {
    let program =
        parse_program("const log = 20\nparams { cutoff = 440 {log, 20000, scale = log} }\n")
            .expect("scale words in range positions should remain expressions");
    let params = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => Some(params),
            _ => None,
        })
        .expect("params block");
    assert!(matches!(
        params[0].range.as_ref().unwrap().min,
        Some(Expr::Var { ref name, .. }) if name == "log"
    ));
    assert_eq!(params[0].control.scale, ParamScale::Log);
}

#[test]
fn rejects_count_prefix_mismatch_with_explicit_list() {
    let src = r#"
ins 2:
  in1
outs 1
sample:
  out1 = in1
"#;
    let program = parse_program(src).expect("parse should preserve count prefix for semantics");
    let ins = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Ins(v) => Some(v),
            _ => None,
        })
        .expect("ins block");
    assert_eq!(ins.len(), 1);
    assert_deferred_int_count(&ins.deferred_count, 2);
}

#[test]
fn rejects_out_defaults_or_ranges() {
    let src = r#"
outs:
  out1 = 0.0 {0.0, 1.0}
sample:
  out1 = 0.0
"#;
    let result = parse_program(src);
    assert!(result.is_err(), "expected outs default/range rejection");
}

#[test]
fn parses_top_level_ins_outs_params_count_shorthand() {
    let src = r#"
ins 3
outs 2
params 4
sample { out1 = in1 + in2 + in3 + param1 + param2 + param3 + param4; out2 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");

    let ins = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Ins(v) => Some(v),
            _ => None,
        })
        .expect("ins block");
    assert!(ins.is_empty());
    assert_deferred_int_count(&ins.deferred_count, 3);

    let outs = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Outs(v) => Some(v),
            _ => None,
        })
        .expect("outs block");
    assert!(outs.is_empty());
    assert_deferred_int_count(&outs.deferred_count, 2);

    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");
    assert!(params.is_empty());
    assert_deferred_int_count(&params.deferred_count, 4);
}

#[test]
fn parses_kouts_count_shorthand_with_section_default_type() {
    let src = r#"
kouts<f32> 2
block { kout1 = 0.0; kout2 = 1.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let kouts = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::KOuts(v) => Some(v),
            _ => None,
        })
        .expect("kouts block");
    assert_eq!(kouts.output_timing, OutputTiming::Block);
    assert_eq!(kouts.deferred_prefix, "kout");
    assert_deferred_int_count(&kouts.deferred_count, 2);
    assert_eq!(
        kouts.deferred_default_ty,
        Some(DeclType::Scalar(PrimitiveType::F32))
    );
}

#[test]
fn parses_indented_kouts_declarations() {
    let src = r#"
kouts:
  meter: f32
  peak
block:
  meter = 1.0
  peak = 0.5
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let kouts = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::KOuts(v) => Some(v),
            _ => None,
        })
        .expect("kouts block");

    assert_eq!(kouts.output_timing, OutputTiming::Block);
    assert_eq!(kouts.decls.len(), 2);
    assert_eq!(kouts[0].name, "meter");
    assert_eq!(kouts[0].output_timing, None);
    assert_eq!(kouts[0].ty, Some(DeclType::Scalar(PrimitiveType::F32)));
    assert_eq!(kouts[1].name, "peak");
    assert_eq!(kouts[1].output_timing, None);
}

#[test]
fn parses_top_level_kins_alias_as_params() {
    let src = r#"
kins<f64>:
  freq = 440.0
  mix: f32 = 0.25
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");

    assert_eq!(params.decls.len(), 2);
    assert_eq!(params.deferred_prefix, "kin");
    assert_eq!(params[0].name, "freq");
    assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
    assert_eq!(params[1].name, "mix");
    assert_eq!(params[1].ty, Some(DeclType::Scalar(PrimitiveType::F32)));
}

#[test]
fn rejects_proc_kins_alias() {
    let errors = parse_program(
        r#"
proc P {
  kins {
    freq = 1.0
  }
  outs 1
  sample { out1 = freq }
}
"#,
    )
    .expect_err("kins should be top-level only");
    assert!(!errors.is_empty());
}

#[test]
fn rejects_empty_proc_output_block_followed_by_another_output_block() {
    let errors = parse_program(
        r#"
proc P {
  outs {}
  kouts { meter }
  block { meter = 1.0 }
}
"#,
    )
    .expect_err("proc cannot declare two output blocks");

    assert!(errors
        .iter()
        .any(|diag| diag.message.contains("duplicate proc output block")));
}

#[test]
fn rejects_legacy_outs_rate_syntax() {
    let section_word_errors = parse_program(
        r#"
outs block {
  meter
}
block { meter = 1.0 }
"#,
    )
    .expect_err("outs block should be rejected");
    assert!(section_word_errors
        .iter()
        .any(|diag| diag.message.contains("use kouts for control-rate outputs")));

    let marker_errors = parse_program(
        r#"
outs {
  @block meter
}
block { meter = 1.0 }
"#,
    )
    .expect_err("per-output timing marker should be rejected");
    assert!(!marker_errors.is_empty());
}

#[test]
fn parses_top_level_count_shorthand_with_section_default_types() {
    let src = r#"
ins<f64> 2
outs<i32> 1
params<bool> 3
buffers<f32> 2
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");

    let ins = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Ins(v) => Some(v),
            _ => None,
        })
        .expect("ins block");
    assert!(ins.is_empty());
    assert_deferred_int_count(&ins.deferred_count, 2);
    assert_eq!(
        ins.deferred_default_ty,
        Some(DeclType::Scalar(PrimitiveType::F64))
    );

    let outs = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Outs(v) => Some(v),
            _ => None,
        })
        .expect("outs block");
    assert!(outs.is_empty());
    assert_deferred_int_count(&outs.deferred_count, 1);
    assert_eq!(
        outs.deferred_default_ty,
        Some(DeclType::Scalar(PrimitiveType::I32))
    );

    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");
    assert!(params.is_empty());
    assert_deferred_int_count(&params.deferred_count, 3);
    assert_eq!(
        params.deferred_default_ty,
        Some(DeclType::Scalar(PrimitiveType::Bool))
    );

    let buffers = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Buffers(v) => Some(v),
            _ => None,
        })
        .expect("buffers block");
    assert!(buffers.is_empty());
    assert_deferred_int_count(&buffers.deferred_count, 2);
    assert!(matches!(
        buffers
            .deferred_default_ty
            .as_ref()
            .map(|t| (&t.elem, &t.channels)),
        Some((
            BufferElemType::Primitive(PrimitiveType::F32),
            crate::ast::BufferChannels::Mono
        ))
    ));
}

#[test]
fn parses_proc_ins_outs_params_count_shorthand() {
    let src = r#"
proc Gain {
  ins 2
  params 1
  outs 1
  sample { out1 = in1 + in2 + param1 }
}
outs { out1 }
init { p = Gain() }
sample { out1 = p(0.5, 0.25) }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");
    assert!(proc.ins.is_empty());
    assert!(proc.outs.is_empty());
    assert!(proc.params.is_empty());
    assert_deferred_int_count(&proc.ins_deferred_count, 2);
    assert_deferred_int_count(&proc.outs_deferred_count, 1);
    assert_deferred_int_count(&proc.params_deferred_count, 1);
}

#[test]
fn parses_top_level_buffers_block_and_count_shorthand() {
    let src_explicit = r#"
buffers {
  buf1
  buf2: buffer<f64>
  buf3: buffer<f32[2]>
  buf4: buffer<f32[]>
  buf5: f32
  buf6: f64[2]
  buf7: f32 {4}
  buf8: f32[2] {count = 3}
}
sample { out1 = 0.0 }
"#;
    let program_explicit = parse_program(src_explicit).expect("parse_program should succeed");
    let buffers = program_explicit
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Buffers(v) => Some(v),
            _ => None,
        })
        .expect("buffers block");
    assert_eq!(buffers.len(), 8);
    assert_eq!(buffers[0].name, "buf1");
    assert!(buffers[0].ty.is_none());
    assert_eq!(buffers[1].name, "buf2");
    assert!(matches!(
        buffers[1].ty.as_ref().map(|t| &t.elem),
        Some(BufferElemType::Primitive(crate::ast::PrimitiveType::F64))
    ));
    assert!(matches!(
        buffers[2].ty.as_ref().map(|t| &t.channels),
        Some(crate::ast::BufferChannels::Static(_))
    ));
    assert!(matches!(
        buffers[3].ty.as_ref().map(|t| &t.channels),
        Some(crate::ast::BufferChannels::Dynamic)
    ));
    assert!(matches!(
        buffers[4].ty.as_ref().map(|t| &t.elem),
        Some(BufferElemType::Primitive(crate::ast::PrimitiveType::F32))
    ));
    assert!(matches!(
        buffers[4].ty.as_ref().map(|t| &t.channels),
        Some(crate::ast::BufferChannels::Mono)
    ));
    assert!(matches!(
        buffers[5].ty.as_ref().map(|t| &t.elem),
        Some(BufferElemType::Primitive(crate::ast::PrimitiveType::F64))
    ));
    assert!(matches!(
        buffers[5].ty.as_ref().map(|t| &t.channels),
        Some(crate::ast::BufferChannels::Static(_))
    ));
    assert_deferred_int_count(&buffers[6].array_size, 4);
    assert_deferred_int_count(&buffers[7].array_size, 3);
    assert!(matches!(
        buffers[7].ty.as_ref().map(|t| &t.channels),
        Some(crate::ast::BufferChannels::Static(_))
    ));

    let src_count = r#"
buffers 3
sample { out1 = 0.0 }
"#;
    let program_count = parse_program(src_count).expect("parse_program should succeed");
    let buffers_count = program_count
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Buffers(v) => Some(v),
            _ => None,
        })
        .expect("buffers count block");
    assert!(buffers_count.is_empty());
    assert_deferred_int_count(&buffers_count.deferred_count, 3);
}

#[test]
fn parses_top_level_buffers_count_shorthand_from_const() {
    let src = r#"
const N = 3
buffers N
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let buffers = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Buffers(v) => Some(v),
            _ => None,
        })
        .expect("buffers block");
    assert!(buffers.is_empty());
    assert!(matches!(
        buffers.deferred_count.as_ref(),
        Some(Expr::Var { name, .. }) if name == "N"
    ));
}

#[test]
fn parses_proc_buffers_count_shorthand_from_namespace_param() {
    let src = r#"
namespace DSP<N = 2>:
  proc Delay:
    buffers<f32> N
    outs 1
    sample:
      out1 = 0.0

outs 1
init:
  d = DSP<3>::Delay()
sample:
  out1 = d()
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "DSP" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Proc(p) if p.name == "Delay" => Some(p),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("proc block");
    assert!(proc.buffers.is_empty());
    assert!(matches!(
        proc.buffers_deferred_count.as_ref(),
        Some(Expr::Var { name, .. }) if name == "N"
    ));
    assert!(matches!(
        proc.buffers_deferred_default_ty
            .as_ref()
            .map(|t| (&t.elem, &t.channels)),
        Some((
            BufferElemType::Primitive(PrimitiveType::F32),
            crate::ast::BufferChannels::Mono
        ))
    ));
}

#[test]
fn parses_proc_buffers_block() {
    let src = r#"
proc Delay {
  buffers {
    line: buffer<f32[2]>
  }
  outs { out1 }
  sample { out1 = 0.0 }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.buffers.len(), 1);
    assert_eq!(proc.buffers[0].name, "line");
}

#[test]
fn parses_multichannel_buffer_indexing_as_internal_calls() {
    let src = r#"
buffers { buf1: buffer<f32[2]> }
sample {
  out1 = buf1[0, 3]
  buf1[1, 2] = 0.5
}
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(v) => Some(v),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 2);
    match &sample[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, args, .. } => {
                assert_eq!(name, "__onda_buffer_read_channel");
                assert_eq!(args.len(), 3);
            }
            _ => panic!("expected channel-read user call"),
        },
        _ => panic!("expected assignment statement"),
    }
    match &sample[1] {
        Stmt::Expr { expr, .. } => match expr {
            Expr::UserCall { name, args, .. } => {
                assert_eq!(name, "__onda_buffer_write_channel");
                assert_eq!(args.len(), 4);
            }
            _ => panic!("expected channel-write user call"),
        },
        _ => panic!("expected expression statement"),
    }
}

#[test]
fn parses_def_buffer_typed_params() {
    let src = r#"
def read_mono(b: buffer<f32>) {
  return 0.0
}
def read_stereo(b: buffer<f32[2]>) {
  return 0.0
}
def read_dyn(b: buffer<f32[]>) {
  return 0.0
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(d) => Some(d),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(defs.len(), 3);
    assert!(matches!(
        defs[0].params[0].ty,
        Some(crate::ast::FnParamType::Buffer(_))
    ));
    assert!(matches!(
        defs[1].params[0].ty,
        Some(crate::ast::FnParamType::Buffer(_))
    ));
    assert!(matches!(
        defs[2].params[0].ty,
        Some(crate::ast::FnParamType::Buffer(_))
    ));
}

#[test]
fn parses_generic_proc_type_params_and_decl_types() {
    let src = r#"
proc Gain<T> {
  ins { in1: T, in2: T[2] }
  outs { out1: T }
  params { g: T = 1.0, coeffs: T[2] = [1.0, 0.5] }
  buffers { b: buffer<T>, m: buffer<T[2]>, d: buffer<T[]> }
  sample { out1 = in1 * g }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc_def.name, "Gain");
    assert_eq!(proc_def.type_params, vec!["T".to_owned()]);
    assert!(matches!(
        proc_def.ins[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc_def.ins[1].ty,
        Some(DeclType::ArrayGeneric { ref elem, .. }) if elem == "T"
    ));
    assert!(matches!(
        proc_def.outs[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc_def.params[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc_def.params[1].ty,
        Some(DeclType::ArrayGeneric { ref elem, .. }) if elem == "T"
    ));
    assert!(matches!(
        proc_def.buffers[0].ty.as_ref().map(|t| &t.elem),
        Some(BufferElemType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc_def.buffers[1].ty.as_ref().map(|t| &t.elem),
        Some(BufferElemType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc_def.buffers[2].ty.as_ref().map(|t| &t.elem),
        Some(BufferElemType::Generic(ref n)) if n == "T"
    ));
}

#[test]
fn parses_generic_proc_section_default_types_with_overrides() {
    let src = r#"
proc Fx<T> {
  ins<T> { in1, trig: bool }
  outs<T> { out1, meter: f32 }
  params<T> { gain = 1.0, mode: i32 = 0 }
  buffers<T> { line, flags: i32 }
  sample { out1 = in1 * gain; meter = f32(mode) }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc_def.name, "Fx");
    assert_eq!(proc_def.type_params, vec!["T".to_owned()]);

    assert!(matches!(
        proc_def.ins[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert_eq!(
        proc_def.ins[1].ty,
        Some(DeclType::Scalar(PrimitiveType::Bool))
    );

    assert!(matches!(
        proc_def.outs[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert_eq!(
        proc_def.outs[1].ty,
        Some(DeclType::Scalar(PrimitiveType::F32))
    );

    assert!(matches!(
        proc_def.params[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert_eq!(
        proc_def.params[1].ty,
        Some(DeclType::Scalar(PrimitiveType::I32))
    );

    assert!(matches!(
        proc_def.buffers[0].ty.as_ref().map(|t| (&t.elem, &t.channels)),
        Some((BufferElemType::Generic(ref n), crate::ast::BufferChannels::Mono)) if n == "T"
    ));
    assert!(matches!(
        proc_def.buffers[1]
            .ty
            .as_ref()
            .map(|t| (&t.elem, &t.channels)),
        Some((
            BufferElemType::Primitive(PrimitiveType::I32),
            crate::ast::BufferChannels::Mono
        ))
    ));
}

#[test]
fn parses_generic_proc_ctor_with_explicit_type_args() {
    let src = r#"
proc Gain<T> {
  outs { out1: T }
  sample { out1 = 0.0 }
}
init { p = Gain<f64>() }
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            assert_eq!(name, "Gain");
            assert_eq!(
                type_args.as_slice(),
                &[CallTypeArg::Primitive(PrimitiveType::F64)]
            );
            assert!(args.is_empty());
        }
        _ => panic!("expected user call"),
    }
}

#[test]
fn parses_user_call_with_explicit_generic_type_args() {
    let src = r#"
sample {
  out1 = id<f64>(1.0)
}
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(v) => Some(v),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            assert_eq!(name, "id");
            assert_eq!(
                type_args.as_slice(),
                &[CallTypeArg::Primitive(PrimitiveType::F64)]
            );
            assert_eq!(args.len(), 1);
        }
        _ => panic!("expected user call"),
    }
}

#[test]
fn parses_user_call_with_generic_type_param_arg() {
    let src = r#"
proc Wrap<T> {
  sample {
    out1 = id<T>(1.0)
  }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    let Stmt::Assign { expr, .. } = &proc.sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            assert_eq!(name, "id");
            assert_eq!(
                type_args.as_slice(),
                &[CallTypeArg::Generic("T".to_owned())]
            );
            assert_eq!(args.len(), 1);
        }
        _ => panic!("expected user call"),
    }
}

#[test]
fn parses_generic_struct_type_params_and_fields() {
    let src = r#"
struct Pair<T> { a: T, b: T }
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");
    assert_eq!(st.name, "Pair");
    assert_eq!(st.type_params, vec!["T".to_owned()]);
    assert!(matches!(st.fields[0].ty, FieldType::Generic(ref n) if n == "T"));
    assert!(matches!(st.fields[1].ty, FieldType::Generic(ref n) if n == "T"));
}

#[test]
fn parses_generic_struct_array_field_type() {
    let src = r#"
struct Bank<T> { taps: T[4] }
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");
    match &st.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(matches!(spec.elem, ArrayElemType::Struct(ref n) if n == "T"));
        }
        _ => panic!("expected array field type"),
    }
}

#[test]
fn parses_generic_struct_ctor_with_explicit_type_args() {
    let src = r#"
struct Pair<T> { a: T, b: T }
init {
  p = Pair<f64>(f64(1.0), f64(2.0))
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            assert_eq!(name, "Pair");
            assert_eq!(
                type_args.as_slice(),
                &[CallTypeArg::Primitive(PrimitiveType::F64)]
            );
            assert_eq!(args.len(), 2);
        }
        _ => panic!("expected user call"),
    }
}

#[test]
fn parses_typed_proc_ports_and_params() {
    let src = r#"
proc Typed {
  ins { in1: i32, in2: f64 }
  outs { out1: i64 }
  params { gain: f64 = 2.0, mode: i32 = 1 }
  sample { out1 = i64(in1) + i64(mode) }
}
outs { out1 }
init { p = Typed() }
sample { out1 = p(1, 2.0) }
"#;
    let program = parse_program(src).expect("parse_program should succeed");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("expected a proc block");
    assert_eq!(proc.ins[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
    assert_eq!(proc.ins[1].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
    assert_eq!(proc.outs[0].ty, Some(DeclType::Scalar(PrimitiveType::I64)));
    assert_eq!(
        proc.params[0].ty,
        Some(DeclType::Scalar(PrimitiveType::F64))
    );
    assert_eq!(
        proc.params[1].ty,
        Some(DeclType::Scalar(PrimitiveType::I32))
    );
}

#[test]
fn parses_proc_field_call_expression() {
    let src = r#"
sample {
  out1 = p(0.25).out2
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert!(
                name.starts_with(PROC_FIELD_SENTINEL_PREFIX),
                "expected proc field sentinel call"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded field argument"
            );
        }
        _ => panic!("expected encoded proc field call expression"),
    }
}

#[test]
fn rejects_proc_indexed_call_expression() {
    let src = r#"
sample {
  out1 = p(0.25)[1]
}
"#;
    assert!(
        parse_program(src).is_err(),
        "proc indexed call syntax should be rejected"
    );
}

#[test]
fn parses_indentation_style_blocks() {
    let src = r#"
outs:
  out1

def add2(a, b):
  return a + b

sample:
  if (1.0 > 0.0):
    out1 = add2(0.25, 0.5)
  else:
    out1 = 0.0
"#;
    let program = parse_program(src).expect("indentation-style program should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 1);
    match &sample[0] {
        Stmt::If { .. } => {}
        _ => panic!("expected if statement in sample block"),
    }
}

#[test]
fn parses_indentation_if_elif_else() {
    let src = r#"
outs:
  out1
sample:
  if (x > 0.0):
    out1 = 1.0
  elif (x > -1.0):
    out1 = 0.5
  else:
    out1 = 0.0
"#;
    let program = parse_program(src).expect("indentation elif should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::If { else_branch, .. } = &sample[0] else {
        panic!("expected top-level if");
    };
    let Stmt::If {
        else_branch: nested_else,
        ..
    } = &else_branch[0]
    else {
        panic!("expected nested if for elif");
    };
    assert_eq!(nested_else.len(), 1);
}

#[test]
fn parses_indentation_if_elif_else_without_parentheses() {
    let src = r#"
outs:
  out1
sample:
  if x > 0.0:
    out1 = 1.0
  elif x > -1.0:
    out1 = 0.5
  else:
    out1 = 0.0
"#;
    let program = parse_program(src).expect("indentation elif should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::If { else_branch, .. } = &sample[0] else {
        panic!("expected top-level if");
    };
    let Stmt::If {
        else_branch: nested_else,
        ..
    } = &else_branch[0]
    else {
        panic!("expected nested if for elif");
    };
    assert_eq!(nested_else.len(), 1);
}

#[test]
fn parses_indentation_section_default_types() {
    let src = r#"
proc Gain<T>:
  ins<T>:
    in1
  outs<T>:
    out1
  params<T>:
    g = 1.0
  buffers<T>:
    line
  sample:
    out1 = in1 * g
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("indentation section defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert!(matches!(
        proc.ins[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc.outs[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc.params[0].ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    assert!(matches!(
        proc.buffers[0].ty.as_ref().map(|t| (&t.elem, &t.channels)),
        Some((BufferElemType::Generic(ref n), crate::ast::BufferChannels::Mono)) if n == "T"
    ));
}

#[test]
fn parses_init_section_default_types() {
    let src = r#"
proc Voice<T>:
  init<T>:
    x = 0.0
  sample:
    out1 = f32(x)
init<f64>:
  acc = 0.0
sample:
  out1 = f32(acc)
"#;
    let program = parse_program(src).expect("init section defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert!(matches!(
        proc.init.default_ty,
        Some(DeclType::Generic(ref n)) if n == "T"
    ));
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("top-level init block");
    assert!(matches!(
        init.default_ty,
        Some(DeclType::Scalar(crate::ast::PrimitiveType::F64))
    ));
}

#[test]
fn rejects_non_scalar_init_section_default_type() {
    let src = r#"
init<f32[4]>:
  x = 0.0
sample:
  out1 = x
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "init section default array type should be rejected"
    );
}

#[test]
fn parses_mixed_indentation_and_braces() {
    let src = r#"
outs:
  out1
sample {
  if (1.0 > 0.0):
    out1 = 1.0
  else { out1 = 0.0 }
}
"#;
    let program = parse_program(src).expect("mixed syntax program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_tab_indentation() {
    let src = "outs:
	out1
sample:
	out1 = 1.0
";
    let program = parse_program(src).expect("tab-indented program should parse");
    assert_eq!(program.blocks.len(), 2);
}

#[test]
fn parses_namespace_blocks_and_preserves_namespace_items() {
    let src = r#"
namespace A:
  struct S:
    x: f32
  def make():
    return 1.0
  namespace B:
    def run():
      return make()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("namespace source should parse");
    let ns = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "A" => Some(ns),
            _ => None,
        })
        .expect("A namespace");
    assert!(ns
        .items
        .iter()
        .any(|item| matches!(item, NamespaceItem::Struct(s) if s.name == "S")));
    assert!(ns
        .items
        .iter()
        .any(|item| matches!(item, NamespaceItem::Def(d) if d.name == "make")));
    assert!(ns.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(nested) if nested.name == "B"
            && nested.items.iter().any(|nested_item| matches!(nested_item, NamespaceItem::Def(d) if d.name == "run")))
    }));
}

#[test]
fn parses_namespace_path_form() {
    let src = r#"
namespace Top::Inner:
  def run(x):
    return x
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("namespace path form should parse");
    let ns = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Top::Inner" => Some(ns),
            _ => None,
        })
        .expect("namespace");
    assert!(ns
        .items
        .iter()
        .any(|item| matches!(item, NamespaceItem::Def(d) if d.name == "run")));
}

#[test]
fn parses_namespace_template_inline_instantiation_and_dedups() {
    let src = r#"
namespace Data<S = SR, C = 1>:
  struct Data<T>:
    storage: T[S * C]

init:
  d1 = Data<SR, 1>::Data<f64>()
  d2 = Data<S = SR, C = 1>::Data<f64>()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("templated namespace source should parse");

    let ns = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Data" => Some(ns),
            _ => None,
        })
        .expect("Data namespace");
    assert_eq!(ns.params.len(), 2);
    assert!(ns
        .items
        .iter()
        .any(|item| matches!(item, NamespaceItem::Struct(s) if s.name == "Data")));

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 2);

    let first_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected first init expr to be constructor call"),
        },
        _ => panic!("expected first init statement to be assignment"),
    };
    let second_name = match &init[1] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected second init expr to be constructor call"),
        },
        _ => panic!("expected second init statement to be assignment"),
    };
    assert_eq!(first_name, "Data<SR, 1>::Data");
    assert_eq!(second_name, "Data<S = SR, C = 1>::Data");
    assert!(first_name.ends_with("::Data"));
}

#[test]
fn parses_namespace_alias_target_single_segment_template_call() {
    let src = r#"
namespace Data<S = SR, C = 1>:
  struct Data<T>:
    storage: T[S * C]

namespace D = Data<SR, 1>

init:
  d = D::Data<f32>()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("namespace alias should parse");
    let alias = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::NamespaceAlias(alias) if alias.name == "D" => Some(alias),
            _ => None,
        })
        .expect("namespace alias");
    assert_eq!(alias.target[0].name, "Data");
    assert_eq!(alias.target[0].args.as_ref().map(Vec::len), Some(2));
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert_eq!(call_name, "D::Data");
    assert!(call_name.ends_with("::Data"));
}

#[test]
fn parses_top_level_use_declarations() {
    let src = r#"
use std::math
use std::fft<512> as fft512
use std::fft<1024>::FFT as FFT1024
pub use std::random::Rng as Random

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("use declarations should parse");
    let uses = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Use(use_decl) => Some(use_decl),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(uses.len(), 4);
    assert_eq!(uses[0].target[0].name, "std");
    assert_eq!(uses[0].target[1].name, "math");
    assert_eq!(uses[0].alias, None);
    assert!(!uses[0].public);
    assert_eq!(uses[1].target[1].name, "fft");
    assert_eq!(uses[1].target[1].args.as_ref().map(Vec::len), Some(1));
    assert_eq!(uses[1].alias.as_deref(), Some("fft512"));
    assert!(!uses[1].public);
    assert_eq!(uses[2].target[2].name, "FFT");
    assert_eq!(uses[2].alias.as_deref(), Some("FFT1024"));
    assert!(!uses[2].public);
    assert_eq!(uses[3].target[2].name, "Rng");
    assert_eq!(uses[3].alias.as_deref(), Some("Random"));
    assert!(uses[3].public);
}

#[test]
fn parses_namespace_local_use_declarations() {
    let src = r#"
namespace DSP:
  use std::math
  def run(x):
    return clamp(x, 0.0, 1.0)

sample:
  out1 = DSP::run(0.5)
"#;
    let program = parse_program(src).expect("namespace-local use declaration should parse");
    let ns = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Namespace(ns) if ns.name == "DSP" => Some(ns),
            _ => None,
        })
        .expect("DSP namespace");
    assert!(ns.items.iter().any(|item| matches!(
        item,
        NamespaceItem::Use(use_decl)
            if use_decl.target.len() == 2
                && use_decl.target[0].name == "std"
                && use_decl.target[1].name == "math"
    )));
}

#[test]
fn parses_namespace_template_implicit_default_instantiation() {
    let src = r#"
namespace Data<S = SR, C = 1>:
  struct Data<T>:
    storage: T[S * C]

init:
  d = Data::Data<f32>()
sample:
  out1 = 0.0
"#;
    let program =
        parse_program(src).expect("implicit default namespace template args should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert_eq!(call_name, "Data::Data");
    assert!(call_name.ends_with("::Data"));
}

#[test]
fn parses_namespace_local_alias_with_relative_template_target() {
    let src = r#"
namespace A:
  namespace Data<S = SR>:
    struct X:
      storage: f32[S]
  namespace D = Data<SR>
  def make():
    x = D::X()
    return 0.0

sample:
  out1 = A::make()
"#;
    let program = parse_program(src).expect("local namespace alias should parse");
    let make_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "A" => ns.items.iter().find_map(|item| match item {
                NamespaceItem::Def(d) if d.name == "make" => Some(d),
                _ => None,
            }),
            _ => None,
        })
        .expect("make def");
    let call_name = match &make_def.body[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert_eq!(call_name, "D::X");
}

#[test]
fn parses_namespace_local_alias_to_relative_template_generic_struct_ctor() {
    let src = r#"
namespace A:
  namespace Data<S = SR>:
    struct Store<T>:
      storage: T[S]
  namespace D = Data<SR>
  def make():
    s = D::Store<f64>()
    return 0.0

sample:
  out1 = A::make()
"#;
    let program = parse_program(src).expect("local namespace alias generic ctor should parse");
    let make_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "A" => ns.items.iter().find_map(|item| match item {
                NamespaceItem::Def(d) if d.name == "make" => Some(d),
                _ => None,
            }),
            _ => None,
        })
        .expect("make def");
    let (call_name, type_args) = match &make_def.body[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall {
                name, type_args, ..
            } => (name.clone(), type_args.clone()),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert_eq!(call_name, "D::Store");
    assert_eq!(type_args, vec![CallTypeArg::Primitive(PrimitiveType::F64)]);
}

#[test]
fn rejects_inconsistent_indentation() {
    let src = "outs:
  out1
sample:
  if (1.0 > 0.0):
    out1 = 1.0
 out1 = 0.0
";
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "parser should reject inconsistent indentation"
    );
}

#[test]
fn parses_block_wrapped_sample_section() {
    let src = r#"
outs { out1 }
init { x = 0.0 }
block {
  x = x + 1.0
  sample {
    out1 = x
  }
  x = x + 2.0
}
"#;
    let program = parse_program(src).expect("program with wrapped block sample should parse");
    let block_exec = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Block(exec) => Some(exec),
            _ => None,
        })
        .expect("block section");
    assert_eq!(block_exec.pre.len(), 1);
    assert_eq!(block_exec.sample.as_ref().map(|s| s.len()), Some(1));
    assert_eq!(block_exec.post.len(), 1);
}

#[test]
fn parses_proc_indexed_call_expression() {
    let src = r#"
sample {
  out1 = voices[1](0.25)
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(name, PROC_INDEX_CALL_SENTINEL);
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_BASE_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index base argument"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_EXPR_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index expression argument"
            );
        }
        _ => panic!("expected encoded proc indexed call expression"),
    }
}

#[test]
fn parses_proc_indexed_field_call_expression() {
    let src = r#"
sample {
  out1 = voices[1](0.25).out2
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert!(
                name.starts_with(PROC_FIELD_SENTINEL_PREFIX),
                "expected proc field sentinel call"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_BASE_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index base argument"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_EXPR_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index expression argument"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded field argument"
            );
        }
        _ => panic!("expected encoded proc indexed field call expression"),
    }
}

#[test]
fn parses_proc_indexed_param_field_expression() {
    let src = r#"
sample {
  out1 = voices[1].gain
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert!(
                name.starts_with(PROC_FIELD_SENTINEL_PREFIX),
                "expected proc field sentinel call"
            );
            assert!(args.iter().any(|a| {
                a.name.as_deref() == Some(PROC_INDEX_BASE_ARG)
                    && matches!(a.expr, Expr::Var { name: ref base, .. } if base == "voices")
            }));
            assert!(args.iter().any(|a| {
                a.name.as_deref() == Some(PROC_INDEX_EXPR_ARG)
                    && matches!(a.expr, Expr::Int { value: 1, .. })
            }));
            assert!(args.iter().any(|a| {
                a.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG)
                    && matches!(a.expr, Expr::Var { name: ref field, .. } if field == "gain")
            }));
        }
        other => panic!("expected encoded proc indexed param field expression, got {other:?}"),
    }
}

#[test]
fn parses_proc_indexed_event_call_expression() {
    let src = r#"
sample {
  voices[idx].note_on(0.25)
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Expr { expr, .. } = &sample[0] else {
        panic!("expected expression statement");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(name, "note_on");
            assert!(matches!(
                args.first(),
                Some(CallArg {
                    name: Some(receiver_marker),
                    expr: Expr::Index { base, .. },
                }) if receiver_marker == METHOD_RECEIVER_ARG && base == "voices"
            ));
        }
        _ => panic!("expected receiver-desugared indexed method call"),
    }
}

#[test]
fn indexed_method_syntax_preserves_a_neutral_receiver_argument() {
    let src = r#"
sample:
  out1 = sources[slot].readCW(0, position)
"#;
    let program = parse_program(src).expect("indexed receiver method should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign {
        expr: Expr::UserCall { name, args, .. },
        ..
    } = &sample[0]
    else {
        panic!("expected receiver-marked call");
    };
    assert_eq!(name, "readCW");
    assert!(matches!(
        args.first(),
        Some(CallArg {
            name: Some(receiver_marker),
            expr: Expr::Index { base, .. },
        }) if receiver_marker == METHOD_RECEIVER_ARG && base == "sources"
    ));
    assert!(args.iter().all(|arg| {
        !matches!(
            arg.name.as_deref(),
            Some(PROC_INDEX_BASE_ARG | PROC_INDEX_EXPR_ARG)
        )
    }));
}

#[test]
fn parses_sample_oversample_factor_brace_form() {
    let src = r#"
outs { out1 }
sample 4 {
  out1 = in1
}
"#;
    let program = parse_program(src).expect("sample oversample factor should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.oversample_factor, Some(Expr::int(4)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_sample_oversample_factor_indentation_form() {
    let src = r#"
outs:
  out1
sample 8:
  out1 = in1
"#;
    let program = parse_program(src).expect("indented sample oversample factor should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.oversample_factor, Some(Expr::int(8)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_proc_sample_oversample_factor() {
    let src = r#"
proc OS {
  outs { out1 }
  sample 2 { out1 = in1 }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("proc sample oversample factor should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.sample_oversample_factor, Some(Expr::int(2)));
    assert_eq!(proc.sample.len(), 1);
}

#[test]
fn parses_block_wrapped_sample_oversample_factor() {
    let src = r#"
outs { out1 }
block {
  sample 16 {
    out1 = in1
  }
}
"#;
    let program = parse_program(src).expect("wrapped sample oversample factor should parse");
    let block_exec = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Block(exec) => Some(exec),
            _ => None,
        })
        .expect("block section");
    let sample = block_exec.sample.as_ref().expect("nested sample");
    assert_eq!(sample.oversample_factor, Some(Expr::int(16)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_array_capacity_expression() {
    let src = r#"
outs { out1 }
struct Delay { buf: f32[SR * 2] }
init {
  d = Delay()
  b: f32[BLOCK_SIZE + 4]
}
sample {
  out1 = d.buf[0] + b[0]
}
"#;
    let program = parse_program(src).expect("program with array capacity expressions should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
}

#[test]
fn parses_typed_array_syntax_and_f32_alias() {
    let src = r#"
outs { out1 }
struct Delay {
  wide: f64[SR * 2]
  mono: f32[64]
}
init {
  a: i32[BLOCK_SIZE + 1]
  b: f32[8]
}
sample {
  out1 = 0.0
}
"#;
    let program = parse_program(src).expect("typed array syntax should parse");

    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");

    match &st.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(matches!(
                spec.elem,
                ArrayElemType::Primitive(crate::ast::PrimitiveType::F64)
            ));
        }
        _ => panic!("expected array field type"),
    }
    match &st.fields[1].ty {
        FieldType::Array(spec) => {
            assert!(matches!(
                spec.elem,
                ArrayElemType::Primitive(crate::ast::PrimitiveType::F32)
            ));
        }
        _ => panic!("expected array field type"),
    }

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");

    match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::ArrayCtor { spec, .. } => {
                assert!(matches!(
                    spec.elem,
                    ArrayElemType::Primitive(crate::ast::PrimitiveType::I32)
                ));
            }
            _ => panic!("expected array constructor"),
        },
        _ => panic!("expected assignment"),
    }
    match &init[1] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::ArrayCtor { spec, .. } => {
                assert!(matches!(
                    spec.elem,
                    ArrayElemType::Primitive(crate::ast::PrimitiveType::F32)
                ));
            }
            _ => panic!("expected array constructor"),
        },
        _ => panic!("expected assignment"),
    }
}

#[test]
fn parses_struct_array_typed_field_in_indentation_and_brace_forms() {
    let src_indent = r#"
outs:
  out1

struct Tap:
  x: f32

struct Voice:
  taps: Tap[3]

sample:
  out1 = 0.0
"#;
    let program_indent = parse_program(src_indent).expect("indentation Struct[N] should parse");
    assert!(
        program_indent
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Struct(_))),
        "expected struct blocks in indentation source"
    );

    let src_brace = r#"
outs { out1 }
struct Tap { x: f32 }
struct Voice { taps: Tap[3] }
sample { out1 = 0.0 }
"#;
    let program_brace = parse_program(src_brace).expect("brace Struct[N] should parse");
    assert!(
        program_brace
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Struct(_))),
        "expected struct blocks in brace source"
    );
}

#[test]
fn parses_array_type_sugar_in_struct_fields_and_init_typed_decls() {
    let src = r#"
outs { out1 }
struct Voice { x: f32 }
struct Bank {
  taps: f32[4]
  voices: Voice[2]
}
init {
  a: f32[8]
  b: Voice[2]
}
sample {
  out1 = a[0]
}
"#;
    let program = parse_program(src).expect("array type sugar should parse");

    let bank = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Bank" => Some(s),
            _ => None,
        })
        .expect("Bank struct");
    assert_eq!(bank.fields.len(), 2);
    match &bank.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(matches!(
                spec.elem,
                ArrayElemType::Primitive(crate::ast::PrimitiveType::F32)
            ));
        }
        _ => panic!("expected array field from f32[4] sugar"),
    }
    match &bank.fields[1].ty {
        FieldType::Array(spec) => {
            assert!(matches!(spec.elem, ArrayElemType::Struct(ref s) if s == "Voice"));
        }
        _ => panic!("expected array field from Voice[2] sugar"),
    }

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 2);
    for stmt in init {
        match stmt {
            Stmt::Assign { decl_ty, expr, .. } => {
                assert!(decl_ty.is_none(), "array sugar should lower to array ctor");
                assert!(
                    matches!(expr, Expr::ArrayCtor { .. }),
                    "array sugar should emit array constructor"
                );
            }
            _ => panic!("expected assignment in init"),
        }
    }
}

#[test]
fn rejects_non_array_literal_typed_array_initializer_expression() {
    let src = r#"
outs { out1 }
init {
  a: f32[4] = 1.0
}
sample {
  out1 = 0.0
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "typed array declaration with non-array initializer should be rejected"
    );
}

#[test]
fn parses_typed_array_initializer_expression() {
    let src = r#"
outs { out1 }
init {
  a: f32[2] = [1.0, 2.0]
}
sample {
  out1 = a[0]
}
"#;
    let program = parse_program(src).expect("typed array initializer should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::ArrayCtor {
            init: Some(values), ..
        } => assert_eq!(values.len(), 2),
        _ => panic!("expected ArrayCtor with array initializer"),
    }
}

#[test]
fn identifiers_starting_with_const_are_not_parsed_as_const_declarations() {
    let src = r#"
outs:
  out1
init:
  constructed: f32[1] = [1.0]
sample:
  out1 = constructed[0]
"#;
    let program = parse_program(src).expect("const-prefixed identifier should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    assert!(matches!(
        init.as_slice(),
        [Stmt::Assign {
            target: AssignTarget::Var(name),
            ..
        }] if name == "constructed"
    ));
}

#[test]
fn parses_untyped_array_literal_assignment_expression() {
    let src = r#"
outs { out1 }
sample {
  b = [1, 2, 3]
  out1 = b[1]
}
"#;
    let program = parse_program(src).expect("untyped array literal assignment should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sb) => Some(&sb.body),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign {
        target,
        decl_ty,
        expr,
        is_typed_decl,
        ..
    } = &sample[0]
    else {
        panic!("expected assignment");
    };
    assert!(matches!(target, AssignTarget::Var(name) if name == "b"));
    assert!(decl_ty.is_none());
    assert!(!is_typed_decl);
    match expr {
        Expr::ArrayLiteral { values, .. } => assert_eq!(values.len(), 3),
        _ => panic!("expected untyped array literal expression"),
    }
}

#[test]
fn parses_inferred_integer_binding_ranges_with_default_and_explicit_modes() {
    let src = r#"
params:
  test = 0

sample:
  clamped = 0 {1000}
  wrapped = 0 {1000, wrap}
  exclusive = 10 {10..1000}
  inclusive = 10 {10..=1000}
  named_count = 0 {count = 1000, mode = clamp}
  named_exclusive = 10 {range = 10..1000, mode = wrap}
  named_inclusive = 10 {range = 10..=1000}
  from_binding = test {0..1000}
"#;
    let program = parse_program(src).expect("inferred integer binding ranges should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(block) => Some(&block.body),
            _ => None,
        })
        .expect("sample block");
    let expected = [
        (BuiltinFn::BindingCountClamp, 0, 1000),
        (BuiltinFn::BindingCountWrap, 0, 1000),
        (BuiltinFn::BindingRangeClamp, 10, 1000),
        (BuiltinFn::BindingRangeInclusiveClamp, 10, 1000),
        (BuiltinFn::BindingCountClamp, 0, 1000),
        (BuiltinFn::BindingRangeWrap, 10, 1000),
        (BuiltinFn::BindingRangeInclusiveClamp, 10, 1000),
        (BuiltinFn::BindingRangeClamp, 0, 1000),
    ];
    for (statement, (expected_func, expected_begin, expected_end)) in sample.iter().zip(expected) {
        let Stmt::Assign {
            decl_ty,
            is_typed_decl,
            expr: Expr::Call { func, args, .. },
            ..
        } = statement
        else {
            panic!("expected an inferred ranged declaration");
        };
        assert_eq!(*decl_ty, Some(PrimitiveType::I32));
        assert!(*is_typed_decl);
        assert_eq!(*func, expected_func);
        assert_eq!(args.len(), 3);
        assert!(matches!(args[1], Expr::Int { value, .. } if value == expected_begin));
        assert!(matches!(args[2], Expr::Int { value, .. } if value == expected_end));
    }
}

#[test]
fn parses_pinned_init_state_independently_from_integer_ranges() {
    let program = parse_program(
        r#"
init:
  pin kernel: f32[8]
  pin cursor: i32 = 0 {8, wrap}
  pin gain = 1.0

sample:
  out1 = gain
"#,
    )
    .expect("pinned init state should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.pinned_roots, ["kernel", "cursor", "gain"]);
    assert_eq!(init.body.len(), 3);
    assert!(matches!(
        &init.body[1],
        Stmt::Assign {
            expr: Expr::Call {
                func: BuiltinFn::BindingCountWrap,
                ..
            },
            ..
        }
    ));
}

#[test]
fn rejects_pin_outside_direct_init_bindings() {
    for source in [
        "sample:\n  pin value = 1\n",
        "init:\n  if true:\n    pin value = 1\n",
        "init:\n  value = 1\n  pin value = 2\n",
        "init:\n  pin value += 1\n",
        "proc Voice:\n  params:\n    pin gain = 1.0\n",
    ] {
        assert!(
            parse_program(source).is_err(),
            "pin should be restricted to direct init bindings: {source}"
        );
    }
}

#[test]
fn pin_freshness_accounts_for_tuple_bindings() {
    let diagnostics = parse_program(
        "init:\n  (value, other) = (1.0, 2.0)\n  pin value = 3.0\nsample:\n  out1 = value\n",
    )
    .expect_err("a tuple-bound root must not be redeclared as pinned state");
    assert!(diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("'pin' requires a fresh state binding; 'value' was already assigned")));
}

#[test]
fn parameter_domains_remain_distinct_from_integer_binding_ranges() {
    let src = r#"
params:
  top: i32 = 6 {0, 6, step = 1}

proc Voice:
  params:
    selector: i32 = 6 {0, 6}
  outs:
    out1
  sample:
    index = selector {count = 7}
    out1 = f32(index)
"#;
    let program = parse_program(src).expect("parameter and binding domains should parse");

    let top_param = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => params.first(),
            _ => None,
        })
        .expect("top-level parameter");
    let top_range = top_param.range.as_ref().expect("top-level parameter range");
    assert!(matches!(top_range.min, Some(Expr::Int { value: 0, .. })));
    assert!(matches!(top_range.max, Expr::Int { value: 6, .. }));
    assert!(matches!(
        top_param.control.step,
        Some(Expr::Int { value: 1, .. })
    ));

    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("processor");
    let proc_param = proc.params.first().expect("processor parameter");
    let proc_range = proc_param
        .range
        .as_ref()
        .expect("processor parameter range");
    assert!(matches!(proc_range.min, Some(Expr::Int { value: 0, .. })));
    assert!(matches!(proc_range.max, Expr::Int { value: 6, .. }));
    assert!(proc_param.control.step.is_none());

    assert!(matches!(
        proc.sample.first(),
        Some(Stmt::Assign {
            expr: Expr::Call {
                func: BuiltinFn::BindingCountClamp,
                ..
            },
            ..
        })
    ));
}

#[test]
fn rejects_incomplete_and_duplicate_named_binding_ranges() {
    for (range, expected) in [
        (
            "{count = 8, range = 0..8}",
            "count and range domains are mutually exclusive",
        ),
        (
            "{0..8, count = 8}",
            "count and range domains are mutually exclusive",
        ),
        ("{0, 8}", "count and range domains are mutually exclusive"),
        ("{wrap, 8}", "positional binding range domain must precede"),
    ] {
        let source = format!("sample:\n  value = 0 {range}\n");
        let errors = parse_program(&source).expect_err("invalid binding range should fail");
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{errors:?}"
        );
    }
}

#[test]
fn parses_indexed_member_assignment_target_as_flat_index_target() {
    let src = r#"
outs { out1 }
sample {
  voices[i].freq = hz
}
"#;
    let program = parse_program(src).expect("indexed member assignment should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sb) => Some(&sb.body),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { target, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match target {
        AssignTarget::Index { base, index } => {
            assert_eq!(base, "voices.freq");
            assert!(matches!(index, Expr::Var { name, .. } if name == "i"));
        }
        _ => panic!("expected indexed assignment target"),
    }
}

#[test]
fn parses_struct_typed_array_single_ctor_initializer_expression() {
    let src = r#"
proc Voice {
  outs { out1 }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
sample {
  out1 = 0.0
}
"#;
    let program =
        parse_program(src).expect("struct typed array single ctor initializer should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::ArrayCtor {
            init: Some(values), ..
        } => {
            assert_eq!(values.len(), 1);
            assert!(matches!(values[0], Expr::UserCall { .. }));
        }
        _ => panic!("expected ArrayCtor with single ctor initializer"),
    }
}

#[test]
fn parse_program_file_resolves_include_and_import() {
    let dir = mk_temp_dir("include_import");
    let main = dir.join("main.onda");
    let filter = dir.join("filter.onda");
    let shared = dir.join("shared.onda");

    write_file(
        &shared,
        r#"
def shared_gain(x) {
  return x * 0.5
}
"#,
    );
    write_file(
        &filter,
        r#"
include "./shared.onda"
namespace DSP:
  struct OnePole:
    z: f32
  def process(x):
    return shared_gain(x)
"#,
    );
    write_file(
        &main,
        r#"
import filter
outs { out1 }
sample {
  s = DSP::OnePole()
  out1 = DSP::process(2.0) + s.z
}
"#,
    );

    let program = parse_program_file(&main).expect("parse_program_file should succeed");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "DSP"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Struct(_))))),
        "expected imported struct to be present"
    );
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "DSP"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Def(_)))))
            && program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected imported defs to be present"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_with_path_uses_entry_overlay_for_relative_imports() {
    let dir = mk_temp_dir("entry_overlay_import");
    let main = dir.join("main.onda");
    let filter = dir.join("filter.onda");
    let shared = dir.join("shared.onda");

    write_file(&main, "outs { out1 }\nsample { out1 = 0.0 }\n");
    write_file(
        &shared,
        r#"
def shared_gain(x) {
  return x * 0.5
}
"#,
    );
    write_file(
        &filter,
        r#"
include "./shared.onda"
namespace DSP:
  struct OnePole:
    z: f32
  def process(x):
    return shared_gain(x)
"#,
    );

    let overlay = r#"
import filter
outs { out1 }
sample {
  s = DSP::OnePole()
  out1 = DSP::process(2.0) + s.z
}
"#;

    let program = parse_program_with_path(overlay, &main).expect("overlay parse should succeed");
    assert!(program
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "DSP"
        && ns.items.iter().any(|item| matches!(item, NamespaceItem::Struct(_))))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_with_path_accepts_unsaved_entry_overlay() {
    let dir = mk_temp_dir("unsaved_entry_overlay");
    let main = dir.join("new.onda");

    let program = parse_program_with_path("outs 1\nsample:\n  out1 = 0.0\n", &main)
        .expect("an unsaved entry overlay should not require an on-disk file");

    assert!(program
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Sample(_))));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_with_overlays_accepts_unsaved_import() {
    let dir = mk_temp_dir("unsaved_import_overlay");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");
    let mut overlays = std::collections::HashMap::new();
    overlays.insert(
        main.clone(),
        "import lib\nouts 1\nsample:\n  out1 = Lib::value()\n".to_owned(),
    );
    overlays.insert(
        lib.parent().expect("lib parent").join(".").join("lib.onda"),
        "namespace Lib:\n  def value():\n    return 0.0\n".to_owned(),
    );

    let program = parse_program_file_with_overlays(&main, &overlays)
        .expect("an unsaved imported overlay should resolve before disk lookup");

    assert!(program
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Namespace(namespace) if namespace.name == "Lib")));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_with_overlays_uses_dependency_overlay_contents() {
    let dir = mk_temp_dir("dependency_overlay_import");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &main,
        r#"
import lib
outs { out1 }
sample { out1 = twice(SCALE) }
"#,
    );
    write_file(
        &lib,
        r#"
const SCALE = invalid
"#,
    );

    let mut overlays = std::collections::HashMap::new();
    overlays.insert(
        dir.join(".").join("lib.onda"),
        r#"
const SCALE = 0.25
def twice(x) { return x + x }
"#
        .to_owned(),
    );

    let program =
        parse_program_file_with_overlays(&main, &overlays).expect("overlay parse should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    let Expr::UserCall { args, .. } = expr else {
        panic!("expected rewritten user call");
    };
    assert!(matches!(
        args[0].expr,
        Expr::Var { ref name, .. } if name == "SCALE"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_rejects_import_module_with_runtime_blocks() {
    let dir = mk_temp_dir("import_runtime_reject");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &lib,
        r#"
outs { out1 }
sample { out1 = 0.0 }
"#,
    );
    write_file(
        &main,
        r#"
import lib
outs { out1 }
sample { out1 = 0.0 }
"#,
    );

    let result = parse_program_file(&main);
    assert!(
        result.is_err(),
        "imported module with runtime blocks should be rejected"
    );
    let errors = result.expect_err("expected parse error");
    assert!(!errors.is_empty(), "expected at least one diagnostic");
    let first = &errors[0];
    let canonical_lib = fs::canonicalize(&lib).expect("canonical lib path");
    let expected_file = canonical_lib
        .to_string_lossy()
        .to_string()
        .trim_start_matches(r"\\?\")
        .to_owned();
    assert_eq!(
        first.file.as_deref(),
        Some(expected_file.as_str()),
        "expected leaf diagnostic file to point at imported module"
    );
    assert_eq!((first.line, first.column), (2, 1));
    assert_eq!(first.end_line, 3);
    assert!(
        first.trace.iter().any(|t| t.contains("import 'lib'")),
        "expected trace to include import site"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_imports_top_level_consts() {
    let dir = mk_temp_dir("import_top_level_consts");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &lib,
        r#"
const SCALE = 0.25
def twice(x) { return x + x }
"#,
    );
    write_file(
        &main,
        r#"
import lib
outs { out1 }
sample { out1 = twice(SCALE) }
"#,
    );

    let program = parse_program_file(&main).expect("parse_program_file should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Const(decl) if decl.name == "SCALE")),
        "top-level consts should be retained for semantics"
    );
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    let Expr::UserCall { args, .. } = expr else {
        panic!("expected rewritten user call");
    };
    assert!(matches!(
        args[0].expr,
        Expr::Var { ref name, .. } if name == "SCALE"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn pest_parse_errors_map_back_to_original_indentation_lines() {
    let src = r#"proc Mix:
  ins:
    dry
    fb

  sample:
    out1 = (dry + fb) * 0.5

proc Saturate:
  sample:
    x = in1
    out1 = x - (x * x * x) * 0.1

init:
  mix = ()
  sat = Saturate()

graph:
  in1 >> mix.dry
  sat.out1 >>[1] mix.fb
  mix.out1 >> sat.in1
  mix.out1 >> out1
"#;

    let errors = parse_program(src).expect_err("expected parse error");
    let diag = errors
        .iter()
        .find(|diag| diag.message.contains("expected expr"))
        .expect("missing expected expr diagnostic");

    assert_eq!((diag.line, diag.column), (15, 10));
    assert!(
        !diag.message.contains("-->"),
        "diagnostic message should not include preprocessed-source location: {}",
        diag.message
    );
    assert!(
        !diag.message.contains("\n15 |"),
        "diagnostic message should not include a source snippet: {}",
        diag.message
    );
}

#[test]
fn parse_program_file_includes_top_level_consts() {
    let dir = mk_temp_dir("include_top_level_consts");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &lib,
        r#"
const SCALE = 0.25
"#,
    );
    write_file(
        &main,
        r#"
include "./lib.onda"
outs { out1 }
sample { out1 = SCALE }
"#,
    );

    let program = parse_program_file(&main).expect("parse_program_file should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Const(decl) if decl.name == "SCALE")),
        "included top-level consts should be retained for semantics"
    );
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    assert!(matches!(
        expr,
        Expr::Var { name, .. } if name == "SCALE"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_rejects_import_include_same_file_mix() {
    let dir = mk_temp_dir("import_include_mix");
    let main = dir.join("main.onda");
    let dep = dir.join("dep.onda");

    write_file(
        &dep,
        r#"
def f(x) { return x }
"#,
    );
    write_file(
        &main,
        r#"
import dep
include "./dep.onda"
outs { out1 }
sample { out1 = f(1.0) }
"#,
    );

    let result = parse_program_file(&main);
    assert!(
        result.is_err(),
        "same file cannot be both imported and included"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_in_memory_supports_builtin_std_imports() {
    let src = r#"
import std/math
outs { out1 }
sample { out1 = clamp(2.0, 0.0, 1.0) }
"#;
    let program = parse_program(src).expect("in-memory std import should parse");
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected std module declarations to be imported"
    );
}

#[test]
fn parse_program_in_memory_supports_std_prelude_module() {
    let src = r#"
import std/prelude
outs { out1 }
sample { out1 = clamp(2.0, 0.0, 1.0) + read([1.0, 2.0], 1) }
"#;
    let program = parse_program(src).expect("in-memory std/prelude import should parse");
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected std/prelude declarations to be imported"
    );
}

#[test]
fn parse_program_in_memory_supports_std_data_module() {
    let src = r#"
import std/data
outs { out1 }
init {
  d = std::data::Data<f32>()
}
sample {
  out1 = d.readL(0.5)
}
"#;
    let program = parse_program(src).expect("in-memory std/data import should parse");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "std::data"
                && ns.items.iter().any(|item| matches!(item, NamespaceItem::Struct(s) if s.name == "Data")))),
        "expected std/data declarations to be imported"
    );
}

#[test]
fn parses_typed_init_struct_decl_with_explicit_type_args_and_ctor() {
    let src = r#"
import std/data
init {
  line: std::data::Data<f32> = std::data::Data()
}
"#;
    let program = parse_program(src).expect("typed init struct declaration should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        !is_typed_decl,
        "typed struct decl should desugar to constructor-typed assignment"
    );
    let Expr::UserCall {
        name, type_args, ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert_eq!(
        type_args.as_slice(),
        &[CallTypeArg::Primitive(PrimitiveType::F32)]
    );
}

#[test]
fn parses_typed_init_struct_decl_with_explicit_type_args_and_default_ctor() {
    let src = r#"
import std/data
init {
  line: std::data::Data<f32>
}
"#;
    let program = parse_program(src).expect("typed init struct declaration should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        !is_typed_decl,
        "typed struct decl should desugar to constructor-typed assignment"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert_eq!(
        type_args.as_slice(),
        &[CallTypeArg::Primitive(PrimitiveType::F32)]
    );
}

#[test]
fn rejects_old_bracket_generic_type_args_in_typed_decl() {
    let src = r#"
import std/data
init {
  line: std::data::Data[f32]
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "old bracket generic type-arg syntax should be rejected"
    );
}

#[test]
fn rejects_old_bracket_namespace_and_generic_instantiation_syntax() {
    let src = r#"
import std/fft
init {
  fft: std::fft[8]::FFT[f32]
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "old bracket namespace/generic instantiation syntax should be rejected"
    );
}

#[test]
fn parses_typed_init_struct_decl_without_type_args_and_default_ctor() {
    let src = r#"
import std/data
init {
  line: std::data::Data
}
"#;
    let program = parse_program(src).expect("typed init struct declaration should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        *is_typed_decl,
        "typed struct decl without explicit type args should remain a typed declaration"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert!(type_args.is_empty(), "type args should be inferred later");
}

#[test]
fn parses_typed_init_namespace_instantiated_struct_decl_without_type_args() {
    let src = r#"
import std/data
init {
  line: std::data<SR, 1>::Data
}
"#;
    let program = parse_program(src).expect("typed init namespace-instantiated decl should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        *is_typed_decl,
        "typed struct decl without explicit type args should remain a typed declaration"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert!(type_args.is_empty(), "type args should be inferred later");
}

#[test]
fn parses_typed_init_namespaced_generic_struct_array_decl() {
    let src = r#"
import std/complex
init {
  bins: std::complex::Complex<f32>[4]
}
"#;
    let program =
        parse_program(src).expect("typed init namespaced generic struct array should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment in init");
    };
    match expr {
        Expr::ArrayCtor { spec, init, .. } => {
            assert!(
                matches!(spec.elem, ArrayElemType::Struct(ref s) if s == "std::complex::Complex<f32>")
            );
            assert!(matches!(spec.size.as_ref(), Expr::Int { value: 4, .. }));
            assert!(
                init.is_none(),
                "default ctor array decl should have no explicit initializer"
            );
        }
        other => panic!("expected array ctor, got {other:?}"),
    }
}

#[test]
fn parses_typed_init_namespace_instantiated_struct_decl_with_explicit_type_args() {
    let src = r#"
import std/fft
init {
  fft: std::fft<8>::FFT<f32>
}
"#;
    let program = parse_program(src)
        .expect("typed init namespace-instantiated struct with type args should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        !is_typed_decl,
        "typed struct decl with explicit type args should desugar to constructor-typed assignment"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::FFT"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert_eq!(
        type_args.as_slice(),
        &[CallTypeArg::Primitive(PrimitiveType::F32)]
    );
}

#[test]
fn parse_program_in_memory_supports_std_lookup_module() {
    let src = r#"
import std/lookup
buffers { b: buffer<f32[2]> }
outs { out1 }
sample {
  out1 = std::lookup::read(b, 0, 1) + std::lookup::readL(b, 1, 0.5)
}
"#;
    let program = parse_program(src).expect("in-memory std/lookup import should parse");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "std::lookup"
                && ns.items.iter().any(|item| matches!(item, NamespaceItem::Def(d) if d.name == "read")))),
        "expected std/lookup declarations to be imported"
    );
}

#[test]
fn parse_program_in_memory_rejects_non_std_imports() {
    let src = r#"
import my_lib
outs { out1 }
sample { out1 = 0.0 }
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "in-memory parser should reject non-std imports without file context"
    );
}

#[test]
fn parses_top_level_events_block_with_scalar_and_array_params() {
    let src = r#"
outs { out1 }
events {
  note_on(note: i32, vel: i32) {
    gate = vel
  }
  set_curve(values: f32[8]) {
    gate = values[0]
  }
}
init { gate = 0.0 }
sample { out1 = gate }
"#;

    let program = parse_program(src).expect("events block should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "note_on");
    assert_eq!(events[0].params.len(), 2);
    assert_eq!(events[1].name, "set_curve");
    assert_eq!(events[1].params.len(), 1);
    match &events[1].params[0].ty {
        EventParamType::Array { elem, .. } => assert_eq!(*elem, PrimitiveType::F32),
        _ => panic!("expected fixed-size primitive array param"),
    }
}

#[test]
fn parses_multiline_event_param_list_in_indentation_syntax() {
    let src = r#"
events:
  test(
    a: f32,
    b: f32,
  ):
    value = a + b
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("multiline event params should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "test");
    assert_eq!(events[0].params.len(), 2);
    assert_eq!(events[0].params[0].name, "a");
    assert_eq!(events[0].params[1].name, "b");
}

#[test]
fn parses_top_level_individual_event_syntax_and_merges_with_events_block() {
    let src = r#"
outs { out1 }
event note_on(note: i32) {
  gate = f32(note)
}
events:
  note_off():
    gate = 0.0
init:
  gate = 0.0
sample:
  out1 = gate
"#;

    let program = parse_program(src).expect("individual event syntax should parse");
    let events = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1, "expected merged top-level events block");
    assert_eq!(events[0].len(), 2);
    assert_eq!(events[0][0].name, "note_on");
    assert_eq!(events[0][1].name, "note_off");
}

#[test]
fn parses_proc_events_block_indentation_syntax() {
    let src = r#"
proc Voice:
  outs 1
  events:
    note_on(note: i32):
      gate = note
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc events indentation syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].name, "note_on");
    assert_eq!(proc.events[0].params.len(), 1);
}

#[test]
fn parses_proc_individual_event_syntax_and_merges_with_events_block() {
    let src = r#"
proc Voice:
  outs 1
  event note_on(note: i32):
    gate = f32(note)
  events:
    note_off():
      gate = 0.0
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("individual proc event syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 2);
    assert_eq!(proc.events[0].name, "note_on");
    assert_eq!(proc.events[1].name, "note_off");
}

#[test]
fn parses_proc_event_slice_param_syntax() {
    let src = r#"
proc Loader:
  events:
    set_ir(values: f32[]):
      last = values[0]
  init:
    last = 0.0
  sample:
    out1 = last
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc event slice syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].params.len(), 1);
    match &proc.events[0].params[0].ty {
        EventParamType::Slice { elem } => assert_eq!(*elem, PrimitiveType::F32),
        other => panic!("expected proc event slice param, got {other:?}"),
    }
}

#[test]
fn parses_generic_proc_event_slice_param_syntax() {
    let src = r#"
proc Loader<T>:
  init:
    last = 0.0
  events:
    set_ir(values: T[]):
      last = f32(values[0])
  sample:
    out1 = last
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event slice syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.type_params, vec!["T".to_owned()]);
    match &proc.events[0].params[0].ty {
        EventParamType::GenericSlice { elem } => assert_eq!(elem, "T"),
        other => panic!("expected generic proc event slice param, got {other:?}"),
    }
}

#[test]
fn parses_generic_proc_event_scalar_param_defaults() {
    let src = r#"
proc Filter<T>:
  events:
    set(freqv: T = 1200.0, rqv: T = 1.0):
      freq = freqv
      rq = rqv
  init:
    freq = 0.0
    rq = 0.0
  sample:
    out1 = f32(freq + rq)
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event scalar defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.type_params, vec!["T".to_owned()]);
    assert_eq!(proc.events[0].params.len(), 2);
    match &proc.events[0].params[0].ty {
        EventParamType::GenericScalar { name } => assert_eq!(name, "T"),
        other => panic!("expected generic scalar proc event param, got {other:?}"),
    }
    assert!(matches!(
        proc.events[0].params[0].default,
        Some(Expr::Number { value, .. }) if (value - 1200.0).abs() < f64::EPSILON
    ));
    match &proc.events[0].params[1].ty {
        EventParamType::GenericScalar { name } => assert_eq!(name, "T"),
        other => panic!("expected generic scalar proc event param, got {other:?}"),
    }
    assert!(matches!(
        proc.events[0].params[1].default,
        Some(Expr::Number { value, .. }) if (value - 1.0).abs() < f64::EPSILON
    ));
}

#[test]
fn parses_generic_proc_event_fixed_array_param_defaults() {
    let src = r#"
proc Loader<T>:
  events:
    load(values: T[2] = [1.0, 2.0]):
      last = f32(values[0] + values[1])
  init:
    last = 0.0
  sample:
    out1 = last
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event fixed-array defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    match &proc.events[0].params[0].ty {
        EventParamType::GenericArray { elem, size } => {
            assert_eq!(elem, "T");
            assert!(matches!(size, Expr::Int { value: 2, .. }));
        }
        other => panic!("expected generic fixed-array proc event param, got {other:?}"),
    }
    assert!(matches!(
        proc.events[0].params[0].default.as_ref(),
        Some(Expr::ArrayLiteral { values, .. }) if values.len() == 2
    ));
}

#[test]
fn parses_generic_proc_event_slice_with_scalar_params() {
    let src = r#"
proc Loader<T>:
  events:
    set_ir(values: T[], start: i32, limit: i32):
      x = start + limit
  sample:
    out1 = 0.0
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event mixed param syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].params.len(), 3);
    match &proc.events[0].params[0].ty {
        EventParamType::GenericSlice { elem } => assert_eq!(elem, "T"),
        other => panic!("expected generic proc event slice param, got {other:?}"),
    }
    match proc.events[0].params[1].ty {
        EventParamType::Scalar(PrimitiveType::I32) => {}
        ref other => panic!("expected i32 event param, got {other:?}"),
    }
    match proc.events[0].params[2].ty {
        EventParamType::Scalar(PrimitiveType::I32) => {}
        ref other => panic!("expected i32 event param, got {other:?}"),
    }
}

#[test]
fn defaults_untyped_event_params_to_f32() {
    let src = r#"
outs { out1 }
events {
  note_on(note, vel: i32, curve: f32[2]) {
    gate = note + f32(vel) + curve[0]
  }
}
init { gate = 0.0 }
sample { out1 = gate }
"#;

    let program = parse_program(src).expect("events block should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].params.len(), 3);
    match events[0].params[0].ty {
        EventParamType::Scalar(PrimitiveType::F32) => {}
        ref other => panic!("expected default f32 event param, got {other:?}"),
    }
    match events[0].params[1].ty {
        EventParamType::Scalar(PrimitiveType::I32) => {}
        ref other => panic!("expected explicit i32 event param, got {other:?}"),
    }
    match &events[0].params[2].ty {
        EventParamType::Array { elem, .. } => assert_eq!(*elem, PrimitiveType::F32),
        _ => panic!("expected fixed-size primitive array param"),
    }
}

#[test]
fn parses_event_param_defaults() {
    let src = r#"
events:
  note_on(freq_hz: f32 = 440.0, offsets: i32[2] = [1, 2], accent: bool = true):
    gate = freq_hz
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("event defaults should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events[0].params.len(), 3);
    assert!(matches!(
        events[0].params[0].default,
        Some(Expr::Number { value, .. }) if (value - 440.0).abs() < f64::EPSILON
    ));
    assert!(matches!(
        events[0].params[1].default.as_ref(),
        Some(Expr::ArrayLiteral { values, .. }) if values.len() == 2
    ));
    assert!(matches!(
        events[0].params[2].default,
        Some(Expr::Bool { value: true, .. })
    ));
}

#[test]
fn defaults_untyped_proc_event_params_to_f32() {
    let src = r#"
proc Voice:
  outs 1
  events:
    note_on(note):
      gate = note
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc events indentation syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].params.len(), 1);
    match proc.events[0].params[0].ty {
        EventParamType::Scalar(PrimitiveType::F32) => {}
        ref other => panic!("expected default f32 proc event param, got {other:?}"),
    }
}

#[test]
fn parses_proc_event_param_defaults() {
    let src = r#"
proc Voice:
  outs 1
  events:
    note_on(note: f32 = 440.0, accent: bool = false):
      gate = note
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc event defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert!(matches!(
        proc.events[0].params[0].default,
        Some(Expr::Number { value, .. }) if (value - 440.0).abs() < f64::EPSILON
    ));
    assert!(matches!(
        proc.events[0].params[1].default,
        Some(Expr::Bool { value: false, .. })
    ));
}

// ---- Phase 0: Namespace Const Fixes — Parser-level tests ----

#[test]
fn parses_2_level_nested_namespace_template() {
    let src = r#"
namespace Outer<A = 1>:
  namespace Inner<B = 2>:
    struct S:
      x: f32

init:
  s = Outer<10>::Inner<20>::S()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("2-level nested namespace template should parse");

    let outer = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Outer" => Some(ns),
            _ => None,
        })
        .expect("Outer namespace");
    assert!(outer.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(inner) if inner.name == "Inner"
            && inner.items.iter().any(|nested| matches!(nested, NamespaceItem::Struct(s) if s.name == "S")))
    }));

    // The init block should have a constructor call referencing the flattened struct
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 1);
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "Outer<10>::Inner<20>::S");
}

#[test]
fn parses_3_level_nested_namespace_template() {
    let src = r#"
namespace L1<A = 1>:
  namespace L2<B = 2>:
    namespace L3<C = 3>:
      struct S:
        x: f32

init:
  s = L1<10>::L2<20>::L3<30>::S()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("3-level nested namespace template should parse");

    let l1 = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "L1" => Some(ns),
            _ => None,
        })
        .expect("L1 namespace");
    assert!(l1.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(l2) if l2.name == "L2"
            && l2.items.iter().any(|nested| {
                matches!(nested, NamespaceItem::Namespace(l3) if l3.name == "L3"
                    && l3.items.iter().any(|leaf| matches!(leaf, NamespaceItem::Struct(s) if s.name == "S")))
            }))
    }));

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "L1<10>::L2<20>::L3<30>::S");
}

#[test]
fn parses_nested_template_inner_uses_outer_const_as_default() {
    let src = r#"
namespace Outer<S = SR>:
  namespace Inner<N = S>:
    struct Buf:
      data: f32

init:
  b = Outer<48000>::Inner::Buf()
sample:
  out1 = 0.0
"#;
    let program =
        parse_program(src).expect("inner template using outer const as default should parse");

    let outer = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Outer" => Some(ns),
            _ => None,
        })
        .expect("Outer namespace");
    assert!(outer.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(inner) if inner.name == "Inner"
            && matches!(inner.params[0].default, Expr::Var { ref name, .. } if name == "S"))
    }));

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "Outer<48000>::Inner::Buf");
}

#[test]
fn deduplicates_nested_namespace_template_instantiations() {
    let src = r#"
namespace Outer<S = SR>:
  namespace Inner<N = 1>:
    struct S:
      x: f32

init:
  a = Outer<SR>::Inner<1>::S()
  b = Outer<SR>::Inner<1>::S()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("nested namespace dedup should parse");

    assert!(program
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "Outer")));

    // Both constructor calls should reference the same struct name
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 2);
    let name_a = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    let name_b = match &init[1] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(
        name_a, name_b,
        "both calls should preserve the same namespace reference"
    );
}

// ---- Phase 0: Generics × Namespace Const Interaction — Parser-level tests ----

#[test]
fn parses_generic_struct_inside_namespace_template_t_s_pattern() {
    let src = r#"
namespace Data<S = SR>:
  struct Store<T>:
    buf: T[S]

init:
  s = Data<1024>::Store<f32>()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("T[S] pattern should parse");

    let store = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Data" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Struct(s) if s.name == "Store" => Some(s),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("Store struct");

    // Generic type params should be preserved
    assert_eq!(store.type_params, vec!["T".to_owned()]);

    // The field `buf` should be an array with generic element type
    assert_eq!(store.fields.len(), 1);
    assert_eq!(store.fields[0].name, "buf");
    match &store.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(
                matches!(spec.elem, ArrayElemType::Struct(ref n) if n == "T"),
                "expected ArrayElemType::Struct(\"T\"), got {:?}",
                spec.elem
            );
        }
        other => panic!("expected FieldType::Array, got {other:?}"),
    }

    // Constructor call should reference the instantiated namespace
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let (call_name, call_type_args) = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall {
                name, type_args, ..
            } => (name.clone(), type_args.clone()),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "Data<1024>::Store");
    assert_eq!(call_type_args.len(), 1);
    assert!(matches!(
        call_type_args[0],
        CallTypeArg::Primitive(PrimitiveType::F32)
    ));
}

#[test]
fn parses_generic_proc_inside_namespace_template() {
    let src = r#"
namespace FX<S = SR>:
  proc Delay<T>:
    ins<T> 1
    outs<T> 1
    init:
      buf: T = T(0.0)
    sample:
      out1 = in1

init:
  d = FX<48000>::Delay<f32>()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("generic proc inside ns template should parse");

    let proc_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "FX" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Proc(p) if p.name == "Delay" => Some(p),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("Delay proc");

    assert_eq!(proc_def.type_params, vec!["T".to_owned()]);
}

#[test]
fn parses_generic_struct_method_array_param_type() {
    let src = r#"
struct Store<T>:
  buf: T[2]
  def load(self, input: T[]):
    self.buf[0] = input[0]
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("generic struct method array param should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Store" => Some(s),
            _ => None,
        })
        .expect("Store struct");
    assert_eq!(st.type_params, vec!["T".to_owned()]);
    assert!(matches!(
        st.methods[0].params[1].ty,
        Some(FnParamType::ArrayGeneric(ref n)) if n == "T"
    ));
}

#[test]
fn parses_def_return_type_annotations() {
    let src = r#"
def scalar(x: f32) -> f64:
  return x

def pair<T>(x: T, y: i32) -> (T, i32):
  return (x, y)

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("def return annotations should parse");
    let defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(def) => Some(def),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        defs[0].return_ty,
        Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(
            PrimitiveType::F64
        )))
    ));
    assert!(matches!(
        defs[1].return_ty,
        Some(FnReturnType::Tuple(ref elems))
            if elems.as_slice()
                == [
                    FnReturnScalarType::Named("T".to_owned()),
                    FnReturnScalarType::Primitive(PrimitiveType::I32),
                ]
    ));
}

#[test]
fn parses_multiline_delimiters_across_core_constructs() {
    let src = r#"
namespace Math<
  N = (
    1 + 1
  ),
>:
  const Size = N

def mix<
  T,
>(
  a: T,
  b: T,
) -> (
  T,
  T,
):
  pair = (
    a,
    b,
  )
  return pair

params:
  freq = 440.0 {
    20.0,
    20000.0,
  }

init:
  arr: f32[
    Math<
      N = 2,
    >::Size
  ] = [
    0.0,
    1.0,
  ]
  x = arr[
    (
      0
    )
  ]

sample:
  out1 = mix<
    f32,
  >(
    arr[
      0
    ],
    x,
  )
"#;

    parse_program(src).expect("multiline delimiter forms should parse");
}

#[test]
fn parses_multiline_section_defaults_and_buffer_delimiters() {
    let src = r#"
outs<
  f32
>:
  out1

buffers<
  f32
>:
  line: buffer<
    f32[
      2
    ]
  >
  taps: f32[
    4
  ]

sample:
  out1 = 0.0
"#;

    parse_program(src).expect("multiline section defaults and buffer delimiters should parse");
}

#[test]
fn parses_multiline_graph_delay_and_endpoint_sets() {
    let src = r#"
outs:
  out1
  out2

graph:
  0.5 >>[
    2
  ] out1
  0.25 >> {
    out1,
    out2,
  }
  {
    out1,
    out2,
  } << 0.125
"#;

    parse_program(src).expect("multiline graph bracket forms should parse");
}

#[test]
fn parses_multiline_method_params_slices_and_indexes() {
    let src = r#"
struct Store:
  value: f32

  def set(
    self,
    value: f32,
  ):
    self.value = value

init:
  data: f32[
    4
  ] = [
    0.0,
    1.0,
    2.0,
    3.0,
  ]
  dst: f32[
    2
  ]
  s = Store()

sample:
  dst[
    0:
    2
  ] = data[
    1:
    3
  ]
  s.set(
    dst[
      0
    ],
  )
  out1 = data[
    0
  ]
"#;

    parse_program(src).expect("multiline method, slice, and index delimiters should parse");
}

#[test]
fn parses_struct_method_return_type_annotation() {
    let src = r#"
struct Pair<T>:
  a: T
  b: T

  def swap(self) -> (T, T):
    return (self.b, self.a)

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("method return annotation should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Pair" => Some(s),
            _ => None,
        })
        .expect("Pair struct");

    assert!(matches!(
        st.methods[0].return_ty,
        Some(FnReturnType::Tuple(ref elems))
            if elems.as_slice()
                == [
                    FnReturnScalarType::Named("T".to_owned()),
                    FnReturnScalarType::Named("T".to_owned()),
                ]
    ));
}

#[test]
fn parses_proc_local_def_return_type_annotation() {
    let src = r#"
proc Voice:
  outs:
    out1

  def pair(x: f32) -> (f32, i32):
    return (x, 1)

  sample:
    out1 = 0.0

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("proc-local return annotation should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) if p.name == "Voice" => Some(p),
            _ => None,
        })
        .expect("Voice proc");

    assert!(matches!(
        proc.local_defs[0].return_ty,
        Some(FnReturnType::Tuple(ref elems))
            if elems.as_slice()
                == [
                    FnReturnScalarType::Primitive(PrimitiveType::F32),
                    FnReturnScalarType::Primitive(PrimitiveType::I32),
                ]
    ));
}

#[test]
fn parses_namespaced_def_return_type_annotation() {
    let src = r#"
namespace dsp:
  struct Pair:
    x

def borrow(pair: dsp::Pair) -> dsp::Pair:
  return pair

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("namespaced return annotation should parse");
    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Def(def) if def.name == "borrow" => Some(def),
            _ => None,
        })
        .expect("borrow def");

    assert!(matches!(
        def.return_ty,
        Some(FnReturnType::Scalar(FnReturnScalarType::Named(ref name)))
            if name == "dsp::Pair"
    ));
}

#[test]
fn parses_nested_generic_struct_field_type() {
    let src = r#"
struct Inner<T>:
  data: T[2]

struct Outer<T>:
  inner: Inner<T>
  banks: Inner<f32>[2]

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("nested generic struct fields should parse");
    let outer = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Outer" => Some(s),
            _ => None,
        })
        .expect("Outer struct");
    assert_eq!(outer.fields[0].name, "inner");
    assert!(matches!(
        outer.fields[1].ty,
        FieldType::Array(ref spec)
            if matches!(spec.elem, ArrayElemType::Struct(ref n) if n == "Inner<f32>")
    ));
}

#[test]
fn parses_namespace_qualified_generic_type_in_call_type_args() {
    let src = r#"
namespace NS:
  struct Pair<T>:
    a: T
    b: T

proc Container<T>:
  outs 1
  init:
    p = NS::Pair<T>(T(1.0), T(2.0))
  sample:
    out1 = f32(p.a)

sample:
  out1 = 0.0
"#;
    let program =
        parse_program(src).expect("ns-qualified generic type in call type args should parse");

    let container = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) if p.name == "Container" => Some(p),
            _ => None,
        })
        .expect("Container proc");
    assert_eq!(container.type_params, vec!["T".to_owned()]);

    let pair = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "NS" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Struct(s) if s.name == "Pair" => Some(s),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("Pair struct");
    assert_eq!(pair.type_params, vec!["T".to_owned()]);
}

#[test]
fn parses_and_rewrites_const_decls_in_top_level_namespace_and_local_scopes() {
    let src = r#"
const N = 4

namespace NS:
  const M = N
  def value():
    return f32(M)

outs { out1 }
events {
  load(values: f32[N]) {}
}
sample {
  const X = N
  out1 = f32(X) + NS::value()
}
"#;
    let program = parse_program(src).expect("const rewriting should succeed");

    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Const(decl) if decl.name == "N")),
        "top-level scalar const declarations should be retained for semantics"
    );
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Namespace(ns) if ns.name == "NS"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Const(decl) if decl.name == "M")))),
        "namespace scalar const declarations should be retained for semantics"
    );

    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("top-level events");
    assert!(matches!(
        events[0].params[0].ty,
        EventParamType::Array {
            elem: PrimitiveType::F32,
            size: Expr::Var { ref name, .. },
        }
            if name == "N"
    ));

    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "NS" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Def(d) if d.name == "value" => Some(d),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("value def");
    assert!(matches!(
        def.body[0],
        Stmt::Return {
            expr: Expr::Cast {
                to: PrimitiveType::F32,
                ref expr,
                ..
            },
            ..
        } if matches!(expr.as_ref(), Expr::Var { name, .. } if name == "M")
    ));

    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(
        sample.body.len(),
        2,
        "local const should be retained for semantics"
    );
    assert!(matches!(
        &sample.body[0],
        Stmt::Const {
            decl: ConstDecl {
                name,
                expr: Expr::Var { name: expr_name, .. },
                ..
            },
            ..
        } if name == "X" && expr_name == "N"
    ));
    assert!(matches!(
        &sample.body[1],
        Stmt::Assign {
            expr:
                Expr::Binary {
                    lhs,
                    ..
                },
            ..
        } if expr_contains_var_with_suffix(lhs, "X")
    ));
}

#[test]
fn parses_explicit_top_level_config_constants() {
    let program = parse_program(
        "config const Size: i32 = 4\nconfig const Values: f32[Size] = [0.0, 1.0, 2.0, 3.0]\n",
    )
    .expect("configuration constants should parse");
    let declarations = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) => Some(decl),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(declarations.len(), 2);
    assert!(declarations.iter().all(|decl| decl.configurable));
    assert_eq!(declarations[0].name, "Size");
    assert!(matches!(
        declarations[0].ty,
        Some(ConstType::Scalar(PrimitiveType::I32))
    ));
    assert!(matches!(
        declarations[1].ty,
        Some(ConstType::Array {
            elem: PrimitiveType::F32,
            ..
        })
    ));
}

#[test]
fn config_constants_are_top_level_only() {
    for source in [
        "namespace NS:\n  config const Value: i32 = 1\n",
        "sample:\n  config const Value: i32 = 1\n",
        "proc P:\n  config const Value: i32 = 1\n  sample:\n    out1 = 0.0\n",
    ] {
        assert!(
            parse_program(source).is_err(),
            "nested configuration constant unexpectedly parsed: {source}"
        );
    }
}

#[test]
fn includes_allow_config_constants_but_imports_reject_them() {
    let dir = mk_temp_dir("config_const_source_modes");
    let main = dir.join("main.onda");
    let shared = dir.join("shared.onda");
    let module = dir.join("module.onda");
    write_file(&shared, "config const Shared: i32 = 1\n");
    write_file(&module, "config const Imported: i32 = 2\n");

    write_file(&main, "include \"./shared.onda\"\n");
    let program = load_program_file(&main).expect("includes should share configuration constants");
    assert!(program.program.blocks.iter().any(
        |block| matches!(block, Block::Const(decl) if decl.name == "Shared" && decl.configurable)
    ));

    write_file(&main, "import module\n");
    let error = load_program_file(&main)
        .expect_err("declaration-only imports must not expose configuration constants");
    assert!(error.diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("configuration constants are not allowed")));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn preserves_top_level_const_arrays_after_scalar_const_rewrite() {
    let src = r#"
const N = 3
const Table: f32[N] = [0.25, 0.5, 1.0]
"#;

    let program = parse_program(src).expect("top-level const array should parse");
    assert!(program
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Const(decl) if decl.name == "N")));
    let table = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Const(decl) if decl.name == "Table" => Some(decl),
            _ => None,
        })
        .expect("const array declaration should be retained");
    match &table.ty {
        Some(ConstType::Array { elem, size }) => {
            assert_eq!(*elem, PrimitiveType::F32);
            assert!(matches!(size, Expr::Var { name, .. } if name == "N"));
        }
        other => panic!("expected typed const array, got {other:?}"),
    }
    assert!(matches!(
        &table.expr,
        Expr::ArrayLiteral { values, .. } if values.len() == 3
    ));
}

#[test]
fn parses_const_array_slice_type_annotations() {
    let src = r#"
const Table: f32[] = [0.25, 0.5, 1.0]

namespace NS:
  const Flags: bool[] = [true, false]
"#;

    let program = parse_program(src).expect("const array slice annotations should parse");
    let table = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Const(decl) if decl.name == "Table" => Some(decl),
            _ => None,
        })
        .expect("top-level const array");
    assert!(matches!(
        &table.ty,
        Some(ConstType::Slice { elem }) if *elem == PrimitiveType::F32
    ));

    let flags = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Namespace(ns) if ns.name == "NS" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Const(decl) if decl.name == "Flags" => Some(decl),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("namespace const array");
    assert!(matches!(
        &flags.ty,
        Some(ConstType::Slice { elem }) if *elem == PrimitiveType::Bool
    ));
}

#[test]
fn parses_top_level_const_def() {
    let src = r#"
const def twice(x: f32) -> f32:
  return x * 2.0

const Table: f32[2] = [twice(0.5), twice(1.0)]
"#;

    let program = parse_program(src).expect("const def should parse");
    let def = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Def(def) if def.name == "twice" => Some(def),
            _ => None,
        })
        .expect("const def should be retained for semantics");
    assert!(def.is_const);
    assert_eq!(def.params.len(), 1);
    assert!(matches!(
        def.return_ty,
        Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(
            PrimitiveType::F32
        )))
    ));
}

#[test]
fn parses_const_def_array_return_type() {
    let src = r#"
const def table() -> f32[4]:
  return [0.0, 0.25, 0.5, 0.75]
"#;

    let program = parse_program(src).expect("const def array return should parse");
    let def = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Def(def) if def.name == "table" => Some(def),
            _ => None,
        })
        .expect("const def should be retained for semantics");
    assert!(def.is_const);
    assert!(matches!(
        def.return_ty,
        Some(FnReturnType::Array {
            elem: PrimitiveType::F32,
            size: Expr::Int { value: 4, .. },
        })
    ));
}

#[test]
fn qualifies_namespace_const_array_references() {
    let src = r#"
namespace LUT:
  const Table = [1, 2, 3]

outs:
  out1

sample:
  out1 = LUT::Table[0]
"#;

    let program = parse_program(src).expect("namespace const array should parse");
    assert!(program.blocks.iter().any(|block| {
        matches!(block, Block::Namespace(ns) if ns.name == "LUT"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Const(decl) if decl.name == "Table")))
    }));
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    match &sample.body[0] {
        Stmt::Assign { expr, .. } => {
            assert!(matches!(expr, Expr::Index { base, .. } if base == "LUT::Table"));
        }
        other => panic!("expected assignment, got {other:?}"),
    }
}

#[test]
fn parses_index_and_slice_on_namespace_template_const_references() {
    let src = r#"
namespace LUT<N = 2>:
  const Table: f32[N] = [0.5, 1.0]

outs:
  out1

sample:
  out1 = LUT<2>::Table[1]
  dst[:] = LUT<2>::Table[:]
"#;

    let program = parse_program(src).expect("namespace-template const array refs should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    match &sample.body[0] {
        Stmt::Assign { expr, .. } => {
            assert!(matches!(expr, Expr::Index { base, .. } if base == "LUT<2>::Table"));
        }
        other => panic!("expected index assignment, got {other:?}"),
    }
    match &sample.body[1] {
        Stmt::Assign { target, expr, .. } => {
            assert!(
                matches!(target, AssignTarget::Slice { base, .. } if base == "dst"),
                "expected slice assignment target, got {target:?}"
            );
            assert!(matches!(expr, Expr::Slice { base, .. } if base == "LUT<2>::Table"));
        }
        other => panic!("expected slice assignment, got {other:?}"),
    }
}

#[test]
fn preserves_proc_local_const_arrays_for_semantic_rejection() {
    let src = r#"
proc Voice:
  const Table = [1, 2]
  outs:
    out1
  sample:
    out1 = 0.0
"#;

    let program = parse_program(src).expect("proc-local const arrays should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Voice" => Some(proc),
            _ => None,
        })
        .expect("Voice proc");
    assert!(matches!(
        proc.consts.as_slice(),
        [ConstDecl {
            name,
            expr: Expr::ArrayLiteral { values, .. },
            ..
        }] if name == "Table" && values.len() == 2
    ));
}

#[test]
fn rejects_proc_local_const_defs() {
    let src = r#"
proc Voice:
  const def gain() -> f32:
    return 1.0
  outs:
    out1
  sample:
    out1 = 0.0
"#;

    let errors = parse_program(src).expect_err("proc-local const defs should be rejected");
    assert!(errors.iter().any(|diag| diag
        .message
        .contains("const defs are only supported at top-level and namespace scope")));
}

#[test]
fn qualifies_qualified_namespace_const_paths_for_semantics() {
    let src = r#"
import std/convolution

outs {
  out1
  out2
}
sample {
  out1 = f32(std::convolution<8, 8>::HopSize)
  out2 = f32(std::convolution::HopSize)
}
"#;
    let program =
        parse_program(src).expect("qualified namespace const access should rewrite successfully");

    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    for stmt in &sample.body {
        assert!(
            stmt_contains_var_with_suffix(stmt, "::HopSize"),
            "expected HopSize namespace const paths to be retained, got {stmt:?}"
        );
    }
}

#[test]
fn qualifies_relative_nested_namespace_const_paths_from_current_namespace() {
    let src = r#"
namespace Outer:
  namespace Inner:
    const VALUE = 3

  def read():
    return f32(Inner::VALUE)

outs {
  out1
}

sample {
  out1 = Outer::read()
}
"#;
    let program =
        parse_program(src).expect("relative nested namespace const access should rewrite");

    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Outer" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Def(def) if def.name == "read" => Some(def),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("read def");

    for stmt in &def.body {
        assert!(
            stmt_contains_var_with_suffix(stmt, "Inner::VALUE"),
            "expected relative nested namespace const path to be retained, got {stmt:?}"
        );
    }
}

fn stmt_contains_var_with_suffix(stmt: &Stmt, suffix: &str) -> bool {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            expr_contains_var_with_suffix(expr, suffix)
        }
        Stmt::Print { values, .. } => values
            .iter()
            .any(|expr| expr_contains_var_with_suffix(expr, suffix)),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_contains_var_with_suffix(cond, suffix)
                || then_branch
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
                || else_branch
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            expr_contains_var_with_suffix(start, suffix)
                || expr_contains_var_with_suffix(end, suffix)
                || step
                    .as_ref()
                    .map(|expr| expr_contains_var_with_suffix(expr, suffix))
                    .unwrap_or(false)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
        }
        Stmt::While { cond, body, .. } => {
            expr_contains_var_with_suffix(cond, suffix)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
        }
    }
}

fn expr_contains_var_with_suffix(expr: &Expr, suffix: &str) -> bool {
    match expr {
        Expr::Var { name, .. } => name.ends_with(suffix),
        Expr::Index { base, index, .. } => {
            base.ends_with(suffix) || expr_contains_var_with_suffix(index, suffix)
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            base.ends_with(suffix)
                || start
                    .as_ref()
                    .map(|expr| expr_contains_var_with_suffix(expr, suffix))
                    .unwrap_or(false)
                || end
                    .as_ref()
                    .map(|expr| expr_contains_var_with_suffix(expr, suffix))
                    .unwrap_or(false)
        }
        Expr::ArrayCtor { spec, init, .. } => {
            expr_contains_var_with_suffix(&spec.size, suffix)
                || init
                    .as_ref()
                    .map(|values| {
                        values
                            .iter()
                            .any(|expr| expr_contains_var_with_suffix(expr, suffix))
                    })
                    .unwrap_or(false)
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            expr_contains_var_with_suffix(lhs, suffix) || expr_contains_var_with_suffix(rhs, suffix)
        }
        Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => args
            .iter()
            .any(|expr| expr_contains_var_with_suffix(expr, suffix)),
        Expr::UserCall { name, args, .. } => {
            name.ends_with(suffix)
                || args
                    .iter()
                    .any(|arg| expr_contains_var_with_suffix(&arg.expr, suffix))
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            expr_contains_var_with_suffix(expr, suffix)
        }
        Expr::Tuple { values, .. } => values
            .iter()
            .any(|v| expr_contains_var_with_suffix(v, suffix)),
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => false,
    }
}

#[test]
fn instantiates_std_convolution_with_user_consts_and_rewrites_nested_proc_calls() {
    let src = r#"
import std/convolution

const FFT_SIZE = 1024
const MAX_IR = 100000

proc Engine:
  init:
    conv = std::convolution<FFT_SIZE, MAX_IR>::ZeroLatencyConvolver<f32>()
  sample:
    out1 = 0.0

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("std::convolution const instantiation should parse");

    let zero_latency = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "std::convolution" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Proc(p) if p.name == "ZeroLatencyConvolver" => Some(p),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("ZeroLatencyConvolver proc");

    let set_impulse = zero_latency
        .events
        .iter()
        .find(|e| e.name == "set_impulse")
        .expect("set_impulse event");
    let event_calls: Vec<_> = set_impulse
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } => Some(name.clone()),
            _ => None,
        })
        .collect();

    assert!(
        event_calls.iter().any(|name| name == "td.set_impulse"),
        "expected td event call to remain receiver-based, got {event_calls:?}"
    );
    assert!(
        event_calls.iter().any(|name| name == "final.set_impulse"),
        "expected final-stage event call to remain receiver-based, got {event_calls:?}"
    );
}

#[test]
fn parses_graph_block_with_rates_and_delay() {
    let src = r#"
outs { out1 }
params { mix = 0.25 }
proc OnePole {
  ins { in1 }
  params { cutoff = 1000.0 }
  outs { out1 }
  sample { out1 = in1 }
}
init {
  lp = OnePole()
}
graph {
  @sample mix >> lp.cutoff
  lp.out1 >>[1] out1
}
"#;

    let program = parse_program(src).expect("graph program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 2);
    assert_eq!(graph.edges[0].rate, Some(GraphRate::Sample));
    assert!(graph.edges[0].delay.is_none());
    assert_eq!(graph.edges[0].dests.len(), 1);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::ProcField { ref proc, ref field, .. }
        if proc == "lp" && field == "cutoff"
    ));
    assert_eq!(graph.edges[1].delay, Some(Expr::int(1)));
    assert_eq!(graph.edges[1].dests.len(), 1);
    assert!(matches!(
        graph.edges[1].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
}

#[test]
fn parses_graph_proc_array_slot_endpoints() {
    let src = r#"
outs { out1 }
proc Voice {
  outs { out1 }
  sample { out1 = 0.0 }
}
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].out1 >> out1
}
"#;

    let program = parse_program(src).expect("graph proc-array program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert!(matches!(
        graph.edges[0].source,
        Expr::UserCall { ref name, .. }
        if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}")
    ));
}

#[test]
fn parses_graph_proc_array_param_slot_sources() {
    let src = r#"
proc Voice {
  params { gain = 0.0 }
  outs { out1 }
  sample { out1 = gain }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].gain >> out1
}
"#;

    let program = parse_program(src).expect("graph proc-array param source program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    match &graph.edges[0].source {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(
                name,
                &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}")
            );
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref base, .. } if base == "voices")
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG)
                    && matches!(arg.expr, Expr::Int { value: 1, .. })
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref field, .. } if field == "gain")
            }));
        }
        other => panic!("expected graph proc-array param source sentinel call, got {other:?}"),
    }
}

#[test]
fn parses_graph_proc_array_output_array_slot_sources() {
    let src = r#"
proc Voice {
  outs:
    pair: f32[2]
  sample {
    pair[0] = 0.0
    pair[1] = 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].pair[0] >> out1
}
"#;

    let program = parse_program(src).expect("graph proc-array output array program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    match &graph.edges[0].source {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(name, GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL);
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref base, .. } if base == "voices")
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG)
                    && matches!(arg.expr, Expr::Int { value: 1, .. })
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref field, .. } if field == "pair")
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(GRAPH_PROC_FIELD_INDEX_EXPR_ARG)
                    && matches!(arg.expr, Expr::Int { value: 0, .. })
            }));
        }
        other => panic!("expected graph source sentinel call, got {other:?}"),
    }
}

#[test]
fn rejects_multi_dot_inline_indexed_member_chain() {
    let src = r#"
outs { out1 }
struct Node { value: f32 }
struct Wrapper { nodes: Node[2] }
init {
  data: Wrapper[2]
}
sample {
  out1 = data[0].nodes[0].value
}
"#;

    assert!(
        parse_program(src).is_err(),
        "multiple inline dot levels after indexed member access should be rejected"
    );
}

#[test]
fn parses_graph_receiver_edges_with_delay() {
    let src = r#"
outs:
  out1

graph:
  @sample out1 <<[2] 0.5
"#;
    let program = parse_program(src).expect("graph receiver program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].rate, Some(GraphRate::Sample));
    assert_eq!(graph.edges[0].delay, Some(Expr::int(2)));
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 1);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
}

#[test]
fn parses_graph_delay_expressions_with_consts_and_namespace_generics() {
    let src = r#"
const TAP = 2

namespace DelayCfg<Base = 1>:
  const LEN = Base + TAP

outs:
  out1

graph:
  0.5 >>[DelayCfg<1>::LEN + 1] out1
"#;
    let program = parse_program(src).expect("graph delay expression program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert!(
        matches!(
            graph.edges[0].delay.as_ref(),
            Some(Expr::Binary { op: BinaryOp::Add, lhs, rhs, .. })
                if matches!(rhs.as_ref(), Expr::Int { value: 1, .. })
                    && matches!(lhs.as_ref(), Expr::Var { name, .. } if name == "DelayCfg<1>::LEN")
        ),
        "delay expr: {:?}",
        graph.edges[0].delay
    );
}

#[test]
fn parses_graph_array_literal_and_slice_sources() {
    let src = r#"
ins:
  in1
  in2
  in_bus: f32[4]
outs:
  out_st: f32[2]

graph:
  [in1, in2] >> out_st
  in_bus[1:3] >> out_st
"#;
    let program = parse_program(src).expect("graph array literal/slice program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    match &graph.edges[0].source {
        Expr::ArrayLiteral { values, .. } => {
            assert_eq!(values.len(), 2);
            assert!(matches!(values[0], Expr::Var { ref name, .. } if name == "in1"));
            assert!(matches!(values[1], Expr::Var { ref name, .. } if name == "in2"));
        }
        other => panic!("expected graph array literal source, got {other:?}"),
    }

    match &graph.edges[1].source {
        Expr::Slice {
            base, start, end, ..
        } => {
            assert_eq!(base, "in_bus");
            assert!(matches!(start.as_deref(), Some(Expr::Int { value: 1, .. })));
            assert!(matches!(end.as_deref(), Some(Expr::Int { value: 3, .. })));
        }
        other => panic!("expected graph slice source, got {other:?}"),
    }
}

#[test]
fn parses_graph_receiver_proc_array_destinations() {
    let src = r#"
proc Voice {
  params { gain = 0.0 }
  outs { out1 }
  sample { out1 = gain }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].gain << 0.5
  out1 << voices[1].out1
}
"#;

    let program = parse_program(src).expect("graph receiver proc-array program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 2);
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 1);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::ProcIndexedField {
            ref proc,
            index: Expr::Int { value: 1, .. },
            ref field,
            ..
        } if proc == "voices" && field == "gain"
    ));
}

#[test]
fn parses_graph_fanout_destinations() {
    let src = r#"
outs:
  out1
  out2

graph:
  0.5 >> { out1, out2 }
"#;
    let program = parse_program(src).expect("graph fanout program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 2);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
    assert!(matches!(
        graph.edges[0].dests[1],
        GraphEndpoint::Symbol { ref name, .. } if name == "out2"
    ));
}

#[test]
fn parses_graph_receiver_fanout_destinations() {
    let src = r#"
outs:
  out1
  out2

graph:
  { out1, out2 } << 0.5
"#;
    let program = parse_program(src).expect("graph receiver fanout program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].source, Expr::number(0.5));
    assert_eq!(graph.edges[0].dests.len(), 2);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
    assert!(matches!(
        graph.edges[0].dests[1],
        GraphEndpoint::Symbol { ref name, .. } if name == "out2"
    ));
}

#[test]
fn stmt_locations_capture_single_line_end_positions() {
    let src = "sample:\n  out1 = in1 + 1.0\n";
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    let loc = sample.body[0].loc();
    assert_eq!(loc.file().as_deref(), Some("<memory>"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);
    assert_eq!(loc.end_line, 2);
    assert_eq!(loc.end_column, 19);
}

#[test]
fn stmt_locations_capture_multiline_end_positions() {
    let src = "sample {\n  clamp(\n    in1,\n    0.0,\n    1.0\n  )\n}\n";
    let program = parse_program(src).expect("program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    let loc = sample.body[0].loc();
    assert_eq!(loc.file().as_deref(), Some("<memory>"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);
    assert_eq!(loc.end_line, 6);
    assert_eq!(loc.end_column, 4);
}

#[test]
fn declaration_locations_capture_param_ranges() {
    let src = "params:\n  gain = 1.0\nsample:\n  out1 = gain\n";
    let program = parse_program(src).expect("program should parse");
    let params = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => Some(params),
            _ => None,
        })
        .expect("params block");

    let loc = params[0].loc.as_ref().expect("param location");
    assert_eq!(loc.file().as_deref(), Some("<memory>"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);
    assert_eq!(loc.end_line(), 2);
    assert_eq!(loc.end_column, 13);
}

#[test]
fn parses_proc_param_bind_hook() {
    let src = r#"
proc Voice:
  params:
    freq = 440.0 {20.0, 20000.0} => update_freq
  outs:
    out1
  sample:
    out1 = freq

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("processor param bind should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Voice" => Some(proc),
            _ => None,
        })
        .expect("Voice proc");

    assert_eq!(proc.params[0].name, "freq");
    assert_eq!(proc.params[0].bind.as_deref(), Some("update_freq"));
}

#[test]
fn parses_proc_private_params() {
    let src = r#"
proc Voice:
  params:
    private cutoff = 1000.0
    private coeffs: f32[2] = [0.5, 0.25]
  outs:
    out1
  sample:
    out1 = cutoff + coeffs[0]

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("private processor params should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Voice" => Some(proc),
            _ => None,
        })
        .expect("Voice proc");

    assert_eq!(proc.params[0].name, "cutoff");
    assert!(proc.params[0].private);
    assert_eq!(proc.params[1].name, "coeffs");
    assert!(proc.params[1].private);
    assert_eq!(proc.params.len(), 2);
}

#[test]
fn graph_locations_capture_edge_and_endpoint_ranges() {
    let src = "outs:\n  out1\ngraph:\n  0.5 >> out1\n";
    let program = parse_program(src).expect("program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    let edge_loc = graph.edges[0].loc();
    assert_eq!(edge_loc.file().as_deref(), Some("<memory>"));
    assert_eq!(edge_loc.line, 4);
    assert_eq!(edge_loc.column, 3);
    assert_eq!(edge_loc.end_line, 4);
    assert_eq!(edge_loc.end_column, 14);

    let dest_loc = graph.edges[0].dests[0].loc();
    assert_eq!(dest_loc.file().as_deref(), Some("<memory>"));
    assert_eq!(dest_loc.line, 4);
    assert_eq!(dest_loc.column, 10);
    assert_eq!(dest_loc.end_line, 4);
    assert_eq!(dest_loc.end_column, 14);
}

#[test]
fn parses_count_shorthand_span_for_semantic_diagnostics() {
    let src = "outs 0\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("parse should preserve invalid count for semantics");
    let outs = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Outs(outs) => Some(outs),
            _ => None,
        })
        .expect("outs block");
    let loc = outs.deferred_count.as_ref().expect("deferred count").loc();

    assert_eq!((loc.line, loc.column), (1, 6));
    assert_eq!(loc.end_line, 1);
    assert_eq!(loc.end_column, 7);
}

#[test]
fn preserves_invalid_scalar_const_expr_for_semantics() {
    let src = "const X = foo\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("parser should preserve invalid const for semantics");
    let decl = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Const(decl) if decl.name == "X" => Some(decl),
            _ => None,
        })
        .expect("const declaration should be retained");
    let loc = decl.expr.loc();

    assert_eq!((loc.line, loc.column), (1, 11));
    assert_eq!(loc.end_line, 1);
    assert_eq!(loc.end_column, 14);
}

#[test]
fn duplicate_namespace_template_diagnostics_report_namespace_span() {
    let src = "namespace Config<T = 1>:\n  struct A:\n    x: f32\nnamespace Config<T = 1>:\n  struct B:\n    x: f32\n";
    let program = parse_program(src).expect("duplicate namespace templates are semantic errors");
    assert_eq!(
        program
            .blocks
            .iter()
            .filter(|block| matches!(block, Block::Namespace(ns) if ns.name == "Config"))
            .count(),
        2
    );
}

#[test]
fn duplicate_namespace_alias_diagnostics_report_alias_span() {
    let src = "namespace Alias = std::math\nnamespace Alias = std::math\n";
    let program = parse_program(src).expect("duplicate namespace aliases are semantic errors");
    assert_eq!(
        program
            .blocks
            .iter()
            .filter(|block| matches!(block, Block::NamespaceAlias(alias) if alias.name == "Alias"))
            .count(),
        2
    );
}

#[test]
fn unknown_namespace_template_diagnostics_report_use_site_span() {
    let src = "outs:\n  out1\nsample:\n  out1 = Missing<1>::X\n";
    let program = parse_program(src).expect("unknown namespace templates are semantic errors");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample");
    assert!(
        matches!(&sample.body[0], Stmt::Assign { expr: Expr::Var { name, .. }, .. } if name == "Missing<1>::X")
    );
}

#[test]
fn namespace_template_argument_count_diagnostics_report_extra_arg_span() {
    let src = "namespace Data<S = SR, C = 1>:\n  const X = 0.0\nouts:\n  out1\nsample:\n  out1 = Data<1, 2, 3>::X\n";
    let program = parse_program(src).expect("argument count is validated semantically");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample");
    assert!(
        matches!(&sample.body[0], Stmt::Assign { expr: Expr::Var { name, .. }, .. } if name == "Data<1, 2, 3>::X")
    );
}

#[test]
fn typed_decl_namespace_template_diagnostics_report_type_span() {
    let src = "params:\n  gain: Missing<1>::X = 0.0\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("unknown namespace templates are semantic errors");
    let params = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => Some(params),
            _ => None,
        })
        .expect("params");
    assert!(
        matches!(params.decls[0].ty, Some(DeclType::Generic(ref name)) if name == "Missing<1>::X")
    );
}

#[test]
fn local_typed_decl_namespace_template_diagnostics_report_type_span() {
    let src = "outs:\n  out1\ninit:\n  x: Missing<1>::X = 0.0\nsample:\n  out1 = 0.0\n";
    let program = parse_program(src).expect("unknown namespace templates are semantic errors");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init");
    assert!(
        matches!(&init.body[0], Stmt::Assign { generic_decl_ty: Some(name), .. } if name.trim() == "Missing<1>::X")
    );
}

#[test]
fn parses_decimal_literals_with_f64_precision() {
    let src = "outs:\n  out1\nsample:\n  out1 = 0.12345678901234566\n";
    let program = parse_program(src).expect("decimal literal program should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(&sample.body),
            _ => None,
        })
        .expect("sample block");
    let expr = match &sample[0] {
        Stmt::Assign { expr, .. } => expr,
        other => panic!("expected sample assignment, got {other:?}"),
    };

    match expr {
        Expr::Number { value, .. } => {
            let expected = 0.12345678901234566_f64;
            let widened_f32 = expected as f32 as f64;
            assert!(
                (value - expected).abs() < 1e-18,
                "expected parser to preserve f64 precision, got {value:?}"
            );
            assert!(
                (value - widened_f32).abs() > 1e-9,
                "expected parser value to differ from widened f32 literal, got {value:?}"
            );
        }
        other => panic!("expected number literal, got {other:?}"),
    }
}

#[test]
fn parses_delegate_forms_and_static_subscription_targets_in_source_order() {
    let source = r#"delegate first(value)
delegates:
  second(values: f32[])
  third(flag: bool = true)

when first(value):
  result = value
when child.second(values):
  result = values[0]
when children[2].third(flag):
  result = f32(flag)
when children.first(index, value):
  result = f32(index + value)

proc Child:
  delegate local(value: i32)
  delegates:
    other(values: f64[2])
  when local(value):
    state = value
  sample:
    out1 = 0.0

sample:
  out1 = 0.0
"#;
    let program = parse_program(source).expect("delegate syntax should parse");
    let delegates = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Delegates(delegates) => Some(&delegates.delegates),
            _ => None,
        })
        .expect("merged top-level delegates");
    assert_eq!(
        delegates
            .iter()
            .map(|delegate| delegate.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
    assert!(matches!(
        delegates[0].params[0].ty,
        EventParamType::Scalar(PrimitiveType::F32)
    ));
    assert!(matches!(
        delegates[1].params[0].ty,
        EventParamType::Slice {
            elem: PrimitiveType::F32
        }
    ));
    assert!(delegates[2].params[0].default.is_some());

    let whens = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::When(when) => Some(when),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(whens.len(), 4);
    assert!(whens[0].target.receiver.is_empty());
    assert_eq!(whens[1].target.receiver, ["child"]);
    assert_eq!(whens[2].target.receiver, ["children"]);
    assert!(whens[2].target.index.is_some());
    assert_eq!(whens[3].bindings[0].name, "index");

    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("processor");
    assert_eq!(
        proc.delegates
            .iter()
            .map(|delegate| delegate.name.as_str())
            .collect::<Vec<_>>(),
        ["local", "other"]
    );
    assert_eq!(proc.whens.len(), 1);
}

#[test]
fn rejects_duplicate_delegates_across_singular_and_plural_forms() {
    let errors = parse_program("delegate done()\ndelegates:\n  done(value: i32)\n")
        .expect_err("duplicate delegates should fail during parsing");
    assert!(errors.iter().any(|error| error
        .message
        .contains("duplicate delegate declaration 'done'")));
}
