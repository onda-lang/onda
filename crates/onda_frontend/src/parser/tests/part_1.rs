use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::{
    ArrayElemType, AssignTarget, BinaryOp, Block, BufferElemType, BuiltinFn, CallArg, CallTypeArg,
    ConstDecl, ConstType, DeclType, EventParamType, Expr, FieldType, FnParamType,
    FnReturnScalarType, FnReturnType, GraphEndpoint, GraphRate, LogicalOp, NamespaceItem,
    OutputTiming, ParamScale, PrimitiveType, Stmt, TupleAssignTarget, INTERNAL_BARE_RETURN_FN,
    INTERNAL_TASK_AWAIT_FN, INTERNAL_TASK_YIELD_FN,
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
    2.0
  )
  out1 = max(
    p.a,
    p.b
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
    trigger = 1.0
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
fn parses_for_statement_with_unparenthesized_expression_bounds() {
    let src = r#"
outs { out1 }
sample {
  values = [1.0, 2.0, 3.0, 4.0]
  for i in 1 + 1..values.len() { out1 = out1 + values[i] }
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
            assert!(matches!(start, Expr::Binary { .. }));
            assert!(matches!(end, Expr::UserCall { .. }));
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
fn parses_underscore_as_an_explicit_for_loop_variable() {
    let program = parse_program("sample:\n  for _ in 0..4:\n    out1 = 1.0\n")
        .expect("underscore loop variable should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(&sample.body),
            _ => None,
        })
        .expect("sample block");

    assert!(matches!(&sample[0], Stmt::For { var, .. } if var == "_"));
}

#[test]
fn parses_loop_statement_with_unparenthesized_expression_count() {
    let src = r#"
outs { out1 }
sample {
  values = [1.0, 2.0, 3.0, 4.0]
  loop values.len() { out1 = out1 + 1.0 }
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
        Stmt::For { end, .. } => assert!(matches!(end, Expr::UserCall { .. })),
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

