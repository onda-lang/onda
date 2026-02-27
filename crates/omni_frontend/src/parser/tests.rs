use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::{
    AssignTarget, BinaryOp, Block, BufferElemType, BuiltinFn, CallTypeArg, DataElemType, DeclType,
    EventParamType, Expr, FieldType, PrimitiveType, Stmt,
};

use super::{
    parse_program, parse_program_file, PROC_FIELD_SENTINEL_ARG, PROC_FIELD_SENTINEL_PREFIX,
    PROC_INDEX_BASE_ARG, PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG,
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
        } => assert_eq!(args.len(), 3),
        _ => panic!("top-level should be an fma builtin call"),
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
            assert!(matches!(start, Expr::Int(0)));
            assert!(matches!(end, Expr::Var(v) if v == "n"));
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
            assert!(matches!(start, Expr::Int(10)));
            assert!(matches!(end, Expr::Int(0)));
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
            assert!(matches!(start, Expr::Int(0)));
            assert!(matches!(end, Expr::Int(4)));
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
  def id[T](self, x: T) {
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
fn parses_data_ctor_and_index_access() {
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
        Some(Expr::Number(_)) | Some(Expr::Int(_))
    ));
    let in1_range = ins[0].range.as_ref().expect("in1 range should be parsed");
    assert!(in1_range.min.is_none());
    assert!(matches!(in1_range.max, Expr::Int(22000)));
    let in2_range = ins[1].range.as_ref().expect("in2 range should be parsed");
    assert!(in2_range.min.is_some());
    assert!(matches!(in2_range.max, Expr::Number(_) | Expr::Int(22000)));

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
    assert!(matches!(params[0].default, Some(Expr::Int(500))));
    let freq_range = params[0]
        .range
        .as_ref()
        .expect("freq range should be parsed");
    assert!(freq_range.min.is_none());
    assert!(matches!(freq_range.max, Expr::Int(8000)));
    assert_eq!(params[1].name, "mix");
    let mix_range = params[1]
        .range
        .as_ref()
        .expect("mix range should be parsed");
    assert!(mix_range.min.is_some());
    assert!(matches!(mix_range.max, Expr::Number(_) | Expr::Int(1)));
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
ins[f64] 2
outs[i32] 1
params[bool] 3
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
def pair[T, U](a: T, b: U) {
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
proc Gain[T] {
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
proc Fx[T] {
  ins[T] { in1, trig: bool }
  outs[T] { out1, meter: f32 }
  params[T] { gain = 1.0, mode: i32 = 0 }
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
proc Gain[T] {
  outs { out1: T }
  sample { out1 = 0.0 }
}
init { p = Gain[f64]() }
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
  out1 = id[f64](1.0)
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
proc Wrap[T] {
  sample {
    out1 = id[T](1.0)
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
struct Pair[T] { a: T, b: T }
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
struct Bank[T] { taps: T[4] }
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
        FieldType::Data(spec) => {
            assert!(matches!(spec.elem, DataElemType::Struct(ref n) if n == "T"));
        }
        _ => panic!("expected Data field type"),
    }
}

#[test]
fn parses_generic_struct_ctor_with_explicit_type_args() {
    let src = r#"
struct Pair[T] { a: T, b: T }
init {
  p = Pair[f64](f64(1.0), f64(2.0))
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
proc Gain[T]:
  ins[T]:
    in1
  outs[T]:
    out1
  params[T]:
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
proc Voice[T]:
  init[T]:
    x = 0.0
  sample:
    out1 = f32(x)
init[f64]:
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
init[f32[4]]:
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
fn rejects_legacy_data_syntax() {
    let src = r#"
outs { out1 }
init { a = Data[4] }
sample { out1 = 0.0 }
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "legacy Data[...] syntax should be rejected"
    );
    let diags = result.err().expect("parse should return diagnostics");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("legacy Data[...] syntax")),
        "expected legacy Data syntax diagnostic"
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
    assert_eq!(sample.oversample_factor, Some(Expr::Int(4)));
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
    assert_eq!(sample.oversample_factor, Some(Expr::Int(8)));
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
    assert_eq!(proc.sample_oversample_factor, Some(Expr::Int(2)));
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
    assert_eq!(sample.oversample_factor, Some(Expr::Int(16)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_data_capacity_expression() {
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
    let program = parse_program(src).expect("program with Data capacity expressions should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
}

#[test]
fn parses_typed_data_syntax_and_f32_alias() {
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
    let program = parse_program(src).expect("typed Data syntax should parse");

    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");

    match &st.fields[0].ty {
        FieldType::Data(spec) => {
            assert!(matches!(
                spec.elem,
                DataElemType::Primitive(crate::ast::PrimitiveType::F64)
            ));
        }
        _ => panic!("expected Data field type"),
    }
    match &st.fields[1].ty {
        FieldType::Data(spec) => {
            assert!(matches!(
                spec.elem,
                DataElemType::Primitive(crate::ast::PrimitiveType::F32)
            ));
        }
        _ => panic!("expected Data field type"),
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
            Expr::DataCtor { spec, .. } => {
                assert!(matches!(
                    spec.elem,
                    DataElemType::Primitive(crate::ast::PrimitiveType::I32)
                ));
            }
            _ => panic!("expected Data constructor"),
        },
        _ => panic!("expected assignment"),
    }
    match &init[1] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::DataCtor { spec, .. } => {
                assert!(matches!(
                    spec.elem,
                    DataElemType::Primitive(crate::ast::PrimitiveType::F32)
                ));
            }
            _ => panic!("expected Data constructor"),
        },
        _ => panic!("expected assignment"),
    }
}

#[test]
fn parses_struct_data_typed_field_in_indentation_and_brace_forms() {
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
        FieldType::Data(spec) => {
            assert!(matches!(
                spec.elem,
                DataElemType::Primitive(crate::ast::PrimitiveType::F32)
            ));
        }
        _ => panic!("expected Data field from f32[4] sugar"),
    }
    match &bank.fields[1].ty {
        FieldType::Data(spec) => {
            assert!(matches!(spec.elem, DataElemType::Struct(ref s) if s == "Voice"));
        }
        _ => panic!("expected Data field from Voice[2] sugar"),
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
                assert!(decl_ty.is_none(), "array sugar should lower to Data ctor");
                assert!(
                    matches!(expr, Expr::DataCtor { .. }),
                    "array sugar should emit Data constructor"
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
        Expr::DataCtor {
            init: Some(values), ..
        } => assert_eq!(values.len(), 2),
        _ => panic!("expected DataCtor with array initializer"),
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
        Expr::ArrayLiteral(values) => assert_eq!(values.len(), 3),
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
        Expr::DataCtor {
            init: Some(values), ..
        } => {
            assert_eq!(values.len(), 1);
            assert!(matches!(values[0], Expr::UserCall { .. }));
        }
        _ => panic!("expected DataCtor with single ctor initializer"),
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
    assert!(
        first.trace.iter().any(|t| t.contains("import 'lib'")),
        "expected trace to include import site"
    );
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
