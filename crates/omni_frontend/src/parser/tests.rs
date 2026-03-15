use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::{
    ArrayElemType, AssignTarget, BinaryOp, Block, BufferElemType, BuiltinFn, CallTypeArg, DeclType,
    EventParamType, Expr, FieldType, FnParamType, GraphEndpoint, GraphRate, PrimitiveType, Stmt,
};

use super::{
    parse_program, parse_program_file, parse_program_file_with_overlays, parse_program_with_path,
    GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL, GRAPH_PROC_FIELD_INDEX_EXPR_ARG,
    PROC_FIELD_SENTINEL_ARG, PROC_FIELD_SENTINEL_PREFIX, PROC_INDEX_BASE_ARG,
    PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG,
};

fn mk_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("omni_frontend_{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, text).expect("write test file");
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
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Assert(_))));
    assert!(
        program.blocks.iter().any(
            |b| matches!(b, Block::Struct(s) if s.name.contains("FFT") && s.name.ends_with("::Tag"))
        ),
        "expected instantiated namespaced struct"
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
            assert!(matches!(step, Some(Expr::Binary { .. })));
            assert!(matches!(start, Expr::Int { value: 10, .. }));
            assert!(matches!(end, Expr::Int { value: 0, .. }));
            assert!(!end_inclusive);
        }
        _ => panic!("expected for statement"),
    }
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
fn rejects_reserved_loop_control_keywords_as_identifiers() {
    let src = r#"
outs { out1 }
sample {
  while = 1.0
  break = 2.0
  continue = 3.0
  out1 = while + break + continue
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "reserved keywords should not parse as identifiers"
    );
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
fn rejects_generic_struct_method_type_params() {
    let src = r#"
outs { out1 }
struct Voice {
  phase: f32
  def id<T>(self, x: T) {
    return x
  }
}
sample {
  out1 = 0.0
}
"#;
    assert!(
        parse_program(src).is_err(),
        "generic method type params should be rejected"
    );
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
                AssignTarget::Slice { base, start, end } => {
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
fn rejects_proc_block_without_nested_sample() {
    let src = r#"
proc Wrapped {
  outs { out1 }
  block {
    x = 1.0
  }
}
sample { out1 = 0.0 }
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "proc block without nested sample should error"
    );
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
fn rejects_count_prefix_mismatch_with_explicit_list() {
    let src = r#"
ins 2:
  in1
outs 1
sample:
  out1 = in1
"#;
    let result = parse_program(src);
    assert!(result.is_err(), "expected count prefix mismatch error");
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
    assert_eq!(ins.len(), 3);
    assert_eq!(ins[0].name, "in1");
    assert_eq!(ins[1].name, "in2");
    assert_eq!(ins[2].name, "in3");
    assert!(ins.iter().all(|d| d.ty.is_none()));

    let outs = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Outs(v) => Some(v),
            _ => None,
        })
        .expect("outs block");
    assert_eq!(outs.len(), 2);
    assert_eq!(outs[0].name, "out1");
    assert_eq!(outs[1].name, "out2");
    assert!(outs.iter().all(|d| d.ty.is_none()));

    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");
    assert_eq!(params.len(), 4);
    assert_eq!(params[0].name, "param1");
    assert_eq!(params[1].name, "param2");
    assert_eq!(params[2].name, "param3");
    assert_eq!(params[3].name, "param4");
    assert!(params.iter().all(|d| d.ty.is_none() && d.default.is_none()));
}

#[test]
fn parses_top_level_count_shorthand_with_section_default_types() {
    let src = r#"
ins<f64> 2
outs<i32> 1
params<bool> 3
buffers[f32] 2
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
    assert_eq!(ins.len(), 2);
    assert_eq!(ins[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
    assert_eq!(ins[1].ty, Some(DeclType::Scalar(PrimitiveType::F64)));

    let outs = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Outs(v) => Some(v),
            _ => None,
        })
        .expect("outs block");
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));

    let params = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Params(v) => Some(v),
            _ => None,
        })
        .expect("params block");
    assert_eq!(params.len(), 3);
    assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
    assert_eq!(params[1].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
    assert_eq!(params[2].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
    assert!(params.iter().all(|d| d.default.is_none()));

    let buffers = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Buffers(v) => Some(v),
            _ => None,
        })
        .expect("buffers block");
    assert_eq!(buffers.len(), 2);
    assert!(buffers.iter().all(|b| matches!(
        b.ty.as_ref().map(|t| (&t.elem, &t.channels)),
        Some((
            BufferElemType::Primitive(PrimitiveType::F32),
            crate::ast::BufferChannels::Mono
        ))
    )));
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
    assert_eq!(proc.ins.len(), 2);
    assert_eq!(proc.ins[0].name, "in1");
    assert_eq!(proc.ins[1].name, "in2");
    assert_eq!(proc.outs.len(), 1);
    assert_eq!(proc.outs[0].name, "out1");
    assert_eq!(proc.params.len(), 1);
    assert_eq!(proc.params[0].name, "param1");
    assert_eq!(proc.params[0].ty, None);
    assert_eq!(proc.params[0].default, None);
}

#[test]
fn parses_top_level_buffers_block_and_count_shorthand() {
    let src_explicit = r#"
buffers {
  buf1
  buf2: buffer[f64]
  buf3: buffer[f32[2]]
  buf4: buffer[f32[]]
  buf5: f32
  buf6: f64[2]
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
    assert_eq!(buffers.len(), 6);
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
    assert_eq!(buffers_count.len(), 3);
    assert_eq!(buffers_count[0].name, "buf1");
    assert_eq!(buffers_count[1].name, "buf2");
    assert_eq!(buffers_count[2].name, "buf3");
}

#[test]
fn parses_proc_buffers_block() {
    let src = r#"
proc Delay {
  buffers {
    line: buffer[f32[2]]
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
fn parses_two_dim_buffer_indexing_as_internal_calls() {
    let src = r#"
buffers { buf1: buffer[f32[2]] }
sample {
  out1 = buf1[0][3]
  buf1[1][2] = 0.5
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
                assert_eq!(name, "__omni_buffer_read2");
                assert_eq!(args.len(), 3);
            }
            _ => panic!("expected read2 user call"),
        },
        _ => panic!("expected assignment statement"),
    }
    match &sample[1] {
        Stmt::Expr { expr, .. } => match expr {
            Expr::UserCall { name, args, .. } => {
                assert_eq!(name, "__omni_buffer_write2");
                assert_eq!(args.len(), 4);
            }
            _ => panic!("expected write2 user call"),
        },
        _ => panic!("expected expression statement"),
    }
}

#[test]
fn parses_def_buffer_typed_params() {
    let src = r#"
def read_mono(b: buffer[f32]) {
  return 0.0
}
def read_stereo(b: buffer[f32[2]]) {
  return 0.0
}
def read_dyn(b: buffer[f32[]]) {
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
fn rejects_generic_def_type_params() {
    let src = r#"
def pair<T, U>(a: T, b: U) {
  return a
}
sample { out1 = 0.0 }
"#;
    assert!(
        parse_program(src).is_err(),
        "generic def type params should be rejected"
    );
}

#[test]
fn parses_generic_proc_type_params_and_decl_types() {
    let src = r#"
proc Gain<T> {
  ins { in1: T, in2: T[2] }
  outs { out1: T }
  params { g: T = 1.0, coeffs: T[2] = [1.0, 0.5] }
  buffers { b: buffer[T], m: buffer[T[2]], d: buffer[T[]] }
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
  buffers[T] { line, flags: i32 }
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
  buffers[T]:
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
fn parses_namespace_blocks_and_flattens_symbol_names() {
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
    let mut struct_names = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    struct_names.sort();
    assert_eq!(struct_names, vec!["A::S".to_owned()]);

    let mut def_names = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(d) => Some(d.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    def_names.sort();
    assert_eq!(
        def_names,
        vec!["A::B::run".to_owned(), "A::make".to_owned()]
    );
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
    let def_name = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Def(d) => Some(d.name.clone()),
            _ => None,
        })
        .expect("def");
    assert_eq!(def_name, "Top::Inner::run");
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

    let struct_names = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(struct_names.len(), 1);
    assert!(struct_names[0].contains("__nsinst"));

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
    assert_eq!(first_name, second_name);
    assert!(first_name.contains("__nsinst"));
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
    assert!(call_name.contains("__nsinst"));
    assert!(call_name.ends_with("::Data"));
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
    assert!(call_name.contains("__nsinst"));
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
            Block::Def(d) if d.name == "A::make" => Some(d),
            _ => None,
        })
        .expect("A::make");
    let call_name = match &make_def.body[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert!(call_name.starts_with("A::Data__nsinst"));
    assert!(call_name.ends_with("::X"));
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
            Block::Def(d) if d.name == "A::make" => Some(d),
            _ => None,
        })
        .expect("A::make");
    let (call_name, type_args) = match &make_def.body[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall {
                name, type_args, ..
            } => (name.clone(), type_args.clone()),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert!(call_name.starts_with("A::Data__nsinst"));
    assert!(call_name.ends_with("::Store"));
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
            assert_eq!(name, &format!("{PROC_INDEX_CALL_SENTINEL}.note_on"));
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
        _ => panic!("expected encoded proc indexed event call expression"),
    }
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
    let main = dir.join("main.omni");
    let filter = dir.join("filter.omni");
    let shared = dir.join("shared.omni");

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
include "./shared.omni"
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
        program.blocks.iter().any(|b| matches!(b, Block::Struct(_))),
        "expected imported struct to be present"
    );
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected imported defs to be present"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_with_path_uses_entry_overlay_for_relative_imports() {
    let dir = mk_temp_dir("entry_overlay_import");
    let main = dir.join("main.omni");
    let filter = dir.join("filter.omni");
    let shared = dir.join("shared.omni");

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
include "./shared.omni"
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
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_with_overlays_uses_dependency_overlay_contents() {
    let dir = mk_temp_dir("dependency_overlay_import");
    let main = dir.join("main.omni");
    let lib = dir.join("lib.omni");

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
        dir.join(".").join("lib.omni"),
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
        Expr::Number { value: n, .. } if (n - 0.25).abs() < 1e-9
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_rejects_import_module_with_runtime_blocks() {
    let dir = mk_temp_dir("import_runtime_reject");
    let main = dir.join("main.omni");
    let lib = dir.join("lib.omni");

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
    let main = dir.join("main.omni");
    let lib = dir.join("lib.omni");

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
        !program.blocks.iter().any(|b| matches!(b, Block::Const(_))),
        "top-level consts should be folded away after rewrite"
    );
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    let Expr::UserCall { args, .. } = expr else {
        panic!("expected rewritten user call");
    };
    assert!(matches!(
        args[0].expr,
        Expr::Number { value: n, .. } if (n - 0.25).abs() < 1e-9
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
}

#[test]
fn parse_program_file_includes_top_level_consts() {
    let dir = mk_temp_dir("include_top_level_consts");
    let main = dir.join("main.omni");
    let lib = dir.join("lib.omni");

    write_file(
        &lib,
        r#"
const SCALE = 0.25
"#,
    );
    write_file(
        &main,
        r#"
include "./lib.omni"
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
        !program.blocks.iter().any(|b| matches!(b, Block::Const(_))),
        "included top-level consts should be folded away after rewrite"
    );
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    assert!(matches!(
        expr,
        Expr::Number { value: n, .. } if (*n - 0.25).abs() < 1e-9
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_rejects_import_include_same_file_mix() {
    let dir = mk_temp_dir("import_include_mix");
    let main = dir.join("main.omni");
    let dep = dir.join("dep.omni");

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
include "./dep.omni"
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
            .any(|b| matches!(b, Block::Struct(s) if s.name.contains("std::data") && s.name.ends_with("::Data"))),
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
buffers { b: buffer[f32[2]] }
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
            .any(|b| matches!(b, Block::Def(d) if d.name.contains("std::lookup::read"))),
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

    // There should be exactly one struct emitted (flattened from the nested namespace)
    let struct_names: Vec<_> = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        struct_names.len(),
        1,
        "expected 1 struct, got {struct_names:?}"
    );
    assert!(
        struct_names[0].contains("__nsinst"),
        "struct name should contain __nsinst: {}",
        struct_names[0]
    );

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
    assert!(
        call_name.contains("__nsinst"),
        "call name should contain __nsinst: {call_name}"
    );
    assert!(
        call_name.ends_with("::S"),
        "call name should end with ::S: {call_name}"
    );
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

    let struct_names: Vec<_> = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        struct_names.len(),
        1,
        "expected 1 struct, got {struct_names:?}"
    );
    assert!(struct_names[0].contains("__nsinst"));

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
    assert!(call_name.contains("__nsinst"));
    assert!(
        call_name.ends_with("::S"),
        "call name should end with ::S: {call_name}"
    );
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

    let struct_names: Vec<_> = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        struct_names.len(),
        1,
        "expected 1 struct, got {struct_names:?}"
    );
    assert!(struct_names[0].contains("__nsinst"));

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
    assert!(call_name.contains("__nsinst"));
    assert!(
        call_name.ends_with("::Buf"),
        "call name should end with ::Buf: {call_name}"
    );
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

    // Dedup: only one struct should be emitted for the same template args
    let struct_names: Vec<_> = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        struct_names.len(),
        1,
        "dedup should produce exactly 1 struct, got {struct_names:?}"
    );

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
        "both calls should reference the same deduped struct"
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
            Block::Struct(s) if s.name.ends_with("::Store") => Some(s),
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
    assert!(
        call_name.contains("__nsinst"),
        "call name should contain __nsinst: {call_name}"
    );
    assert!(
        call_name.ends_with("::Store"),
        "call name should end with ::Store: {call_name}"
    );
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
            Block::Proc(p) if p.name.ends_with("::Delay") => Some(p),
            _ => None,
        })
        .expect("Delay proc");

    assert_eq!(proc_def.type_params, vec!["T".to_owned()]);
    assert!(
        proc_def.name.contains("__nsinst"),
        "proc name should contain __nsinst: {}",
        proc_def.name
    );
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

    // Check that the Pair struct exists with the NS:: prefix
    let pair = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "NS::Pair" => Some(s),
            _ => None,
        })
        .expect("NS::Pair struct");
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
        !program.blocks.iter().any(|b| matches!(b, Block::Const(_))),
        "const declarations should be stripped from the rewritten program"
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
            size: Expr::Int { value: 4, .. },
        }
    ));

    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Def(d) if d.name == "NS::value" => Some(d),
            _ => None,
        })
        .expect("NS::value def");
    assert!(matches!(
        def.body[0],
        Stmt::Return {
            expr: Expr::Cast {
                to: PrimitiveType::F32,
                ..
            },
            ..
        }
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
        1,
        "local const should be stripped from the sample body"
    );
}

#[test]
fn rewrites_qualified_namespace_const_paths_to_compile_time_values() {
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
            !stmt_contains_var_with_suffix(stmt, "::HopSize"),
            "expected HopSize namespace const paths to fold away, got {stmt:?}"
        );
    }
}

fn stmt_contains_var_with_suffix(stmt: &Stmt, suffix: &str) -> bool {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            expr_contains_var_with_suffix(expr, suffix)
        }
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
            Block::Proc(p) if p.name.ends_with("::ZeroLatencyConvolver") => Some(p),
            _ => None,
        })
        .expect("instantiated ZeroLatencyConvolver proc");

    let init_calls: Vec<_> = zero_latency
        .init
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::UserCall {
                    name: call_name, ..
                },
                ..
            } => Some((name.clone(), call_name.clone())),
            _ => None,
        })
        .collect();

    assert!(
        init_calls
            .iter()
            .any(|(name, call_name)| name == "td" && call_name.contains("::TimeDomainConvolver")),
        "expected td ctor to be namespaced, got {init_calls:?}"
    );
    assert!(
        init_calls
            .iter()
            .any(|(name, call_name)| name == "tail" && call_name.contains("::BlockConvolver")),
        "expected tail ctor to be namespaced, got {init_calls:?}"
    );

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
        event_calls.iter().any(|name| name == "tail.set_impulse"),
        "expected tail event call to remain receiver-based, got {event_calls:?}"
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
fn syntax_diagnostics_report_count_shorthand_span() {
    let src = "outs 0\nsample:\n  out1 = 0.0\n";
    let errors = parse_program(src).expect_err("invalid outs count should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("outs count shorthand must be greater than zero")
        })
        .expect("missing outs count diagnostic");

    assert_eq!((diag.line, diag.column), (1, 6));
    assert_eq!(diag.end_line, 1);
    assert_eq!(diag.end_column, 7);
}

#[test]
fn const_validation_diagnostics_report_expr_span() {
    let src = "const X = foo\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
    let errors = parse_program(src).expect_err("invalid const should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("const 'X': expression references non-compile-time symbol 'foo'")
        })
        .expect("missing const validation diagnostic");

    assert_eq!((diag.line, diag.column), (1, 11));
    assert_eq!(diag.end_line, 1);
    assert_eq!(diag.end_column, 14);
}

#[test]
fn duplicate_namespace_template_diagnostics_report_namespace_span() {
    let src = "namespace Config<T = 1>:\n  struct A:\n    x: f32\nnamespace Config<T = 1>:\n  struct B:\n    x: f32\n";
    let errors = parse_program(src).expect_err("duplicate namespace template should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("duplicate namespace template 'Config'")
        })
        .expect("missing duplicate namespace template diagnostic");

    assert_eq!((diag.line, diag.column), (4, 11));
    assert_eq!(diag.end_line, 4);
}

#[test]
fn duplicate_namespace_alias_diagnostics_report_alias_span() {
    let src = "namespace Alias = std::math\nnamespace Alias = std::math\n";
    let errors = parse_program(src).expect_err("duplicate namespace alias should fail");
    let diag = errors
        .iter()
        .find(|diag| diag.message.contains("duplicate namespace alias 'Alias'"))
        .expect("missing duplicate namespace alias diagnostic");

    assert_eq!((diag.line, diag.column), (2, 11));
    assert_eq!(diag.end_line, 2);
}

#[test]
fn unknown_namespace_template_diagnostics_report_use_site_span() {
    let src = "outs:\n  out1\nsample:\n  out1 = Missing<1>::X\n";
    let errors = parse_program(src).expect_err("unknown namespace template should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("unknown namespace template 'Missing'")
        })
        .expect("missing unknown namespace template diagnostic");

    assert_eq!(diag.file.as_deref(), Some("<memory>"));
    assert_eq!((diag.line, diag.column), (4, 10));
    assert_eq!(diag.end_line, 4);
}

#[test]
fn namespace_template_argument_count_diagnostics_report_extra_arg_span() {
    let src = "namespace Data<S = SR, C = 1>:\n  const X = 0.0\nouts:\n  out1\nsample:\n  out1 = Data<1, 2, 3>::X\n";
    let errors = parse_program(src).expect_err("too many namespace template args should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("namespace template 'Data' received too many positional arguments")
        })
        .expect("missing namespace template argument count diagnostic");

    assert_eq!(diag.file.as_deref(), Some("<memory>"));
    assert_eq!((diag.line, diag.column), (6, 21));
    assert_eq!(diag.end_line, 6);
}

#[test]
fn typed_decl_namespace_template_diagnostics_report_type_span() {
    let src = "params:\n  gain: Missing<1>::X = 0.0\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
    let errors = parse_program(src)
        .expect_err("unknown namespace template in typed parameter declaration should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("unknown namespace template 'Missing'")
        })
        .expect("missing typed parameter namespace template diagnostic");

    assert_eq!(diag.file.as_deref(), Some("<memory>"));
    assert_eq!((diag.line, diag.column), (2, 9));
    assert_eq!(diag.end_line, 2);
}

#[test]
fn local_typed_decl_namespace_template_diagnostics_report_type_span() {
    let src = "outs:\n  out1\ninit:\n  x: Missing<1>::X = 0.0\nsample:\n  out1 = 0.0\n";
    let errors =
        parse_program(src).expect_err("unknown namespace template in local typed decl should fail");
    let diag = errors
        .iter()
        .find(|diag| {
            diag.message
                .contains("unknown namespace template 'Missing'")
        })
        .expect("missing local typed decl namespace template diagnostic");

    assert_eq!(diag.file.as_deref(), Some("<memory>"));
    assert_eq!((diag.line, diag.column), (4, 6));
    assert_eq!(diag.end_line, 4);
}
