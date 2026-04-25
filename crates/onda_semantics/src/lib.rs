use std::collections::{HashMap, HashSet};

use onda_frontend::{
    inject_auto_std_math, ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec,
    BlockKind, BufferBlock, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn,
    CallArg, CallTypeArg, CmpOp, ConstDecl, ConstType, DeclRange, DeclType, DiagCtx, Diagnostic,
    EventDef, EventParamType, Expr, FieldType, FnParamType, FnReturnScalarType, FnReturnType,
    FunctionDef, GraphBlock, GraphEdge, GraphEndpoint, GraphRate, InitBlock, NamespaceAliasDecl,
    NamespaceCallArg, NamespaceDecl, NamespaceItem, NamespaceRefSegment, ParamBlock, ParamDecl,
    PortBlock, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc, Stmt,
    StructDef, StructField,
};

mod array_structs;
mod builtins;
mod decl_symbols;
mod declaration_coercion;
mod def_semantics;
mod diag_utils;
mod executable_owner_analysis;
mod expr_analysis;
mod expr_typing;
mod expr_validation;
mod generic_specialization;
pub mod internal_names;
mod io_state_helpers;
mod namespacing;
mod pipeline;
mod port_coercion;
mod proc_call_rewrite;
mod proc_call_support;
mod proc_resolution;
mod proc_state_rewrite;
mod processor_lowering;
mod stmt_analysis;
use array_structs::*;
use builtins::*;
use decl_symbols::*;
use declaration_coercion::*;
use def_semantics::*;
pub(crate) use diag_utils::push_semantic;
use executable_owner_analysis::*;
use expr_analysis::{build_expr_env, build_scope_expr_env, ExprEnv, FnSignature, ScopeExprInputs};
use expr_typing::*;
use expr_validation::*;
use generic_specialization::*;
pub(crate) use internal_names::runtime_proc_array_active_symbol;
use io_state_helpers::*;
use namespacing::*;
pub use pipeline::{analyze, analyze_with_options, lower_graphs_for_inspection_with_options};
use port_coercion::*;
use proc_call_rewrite::*;
use proc_call_support::{
    rewrite_proc_alias_calls_for_validation, rewrite_proc_alias_calls_in_expr, split_dot_path,
    ProcArrayAliasInfo,
};
use proc_resolution::*;
use proc_state_rewrite::*;
use stmt_analysis::*;

#[derive(Debug, Clone)]
pub struct TypedProgram {
    pub ins: Vec<String>,
    pub outs: Vec<String>,
    pub in_types: HashMap<String, PrimitiveType>,
    pub out_types: HashMap<String, PrimitiveType>,
    pub param_types: HashMap<String, PrimitiveType>,
    pub in_defaults: HashMap<String, TypedConstValue>,
    pub in_ranges: HashMap<String, TypedValueRange>,
    pub in_arrays: HashMap<String, TypedArrayInfo>,
    pub out_arrays: HashMap<String, TypedArrayInfo>,
    pub param_arrays: HashMap<String, TypedArrayInfo>,
    pub const_arrays: Vec<TypedConstArray>,
    pub params: Vec<TypedParam>,
    pub buffers: Vec<TypedBufferDecl>,
    pub structs: Vec<TypedStruct>,
    pub defs: Vec<TypedFunction>,
    pub events: Vec<TypedEvent>,
    pub def_sample_oversample_factors: HashMap<String, usize>,
    pub proc_step_oversample_meta: HashMap<String, ProcStepOversampleMeta>,
    pub init: Vec<Stmt>,
    pub block_pre: Vec<Stmt>,
    pub sample_oversample_factor: usize,
    pub sample: Vec<Stmt>,
    pub block_post: Vec<Stmt>,
    pub state_vars: Vec<String>,
    pub state_types: Vec<PrimitiveType>,
    pub state_tuples: HashMap<String, Vec<PrimitiveType>>,
    pub array_vars: Vec<TypedArrayVar>,
    pub array_struct_roots: Vec<TypedArrayStructRoot>,
    pub ins_explicit: bool,
    pub outs_explicit: bool,
    pub params_explicit: bool,
}

#[derive(Debug, Clone)]
pub struct TypedEvent {
    pub name: String,
    pub params: Vec<TypedEventParam>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedEventParamDefault {
    Scalar(TypedConstValue),
    Array(Vec<TypedConstValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedEventParam {
    pub name: String,
    pub ty: TypedEventParamType,
    pub default: Option<TypedEventParamDefault>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedEventParamType {
    Scalar(PrimitiveType),
    Array { elem: PrimitiveType, len: usize },
    Slice { elem: PrimitiveType },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedConstArray {
    pub name: String,
    pub elem_ty: PrimitiveType,
    pub len: usize,
    pub values: Vec<TypedConstValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Scalar(TypedConstValue),
    Array {
        elem_ty: PrimitiveType,
        len: usize,
        values: Vec<TypedConstValue>,
    },
}

#[derive(Debug, Clone)]
pub struct ProcStepOversampleMeta {
    pub input_state_fields: HashMap<String, ProcInputOversampleStateFields>,
    pub output_state_fields: HashMap<String, ProcOutputOversampleStateFields>,
}

#[derive(Debug, Clone)]
pub struct ProcInputOversampleStateFields {
    pub up_stages: Vec<ProcSincStageStateFields>,
}

#[derive(Debug, Clone)]
pub struct ProcOutputOversampleStateFields {
    pub down_stages: Vec<ProcSincStageStateFields>,
}

#[derive(Debug, Clone)]
pub struct ProcSincStageStateFields {
    pub a0: String,
    pub a1: String,
    pub a2: String,
    pub a3: String,
    pub b0: String,
    pub b1: String,
    pub b2: String,
    pub b3: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum TypedBufferChannels {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedBufferDecl {
    pub name: String,
    pub elem_ty: PrimitiveType,
    pub channels: TypedBufferChannels,
}

#[derive(Debug, Clone)]
pub struct TypedStruct {
    pub name: String,
    pub fields: Vec<TypedStructField>,
}

#[derive(Debug, Clone)]
pub struct TypedStructField {
    pub name: String,
    pub ty: TypedFieldType,
    pub default: Option<Expr>,
    pub struct_name: Option<String>,
    pub array_elem_ty: Option<PrimitiveType>,
    pub array_elem_struct: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedFieldType {
    Scalar(PrimitiveType),
    Struct,
    Array(usize),
    Tuple(Vec<PrimitiveType>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReturnType {
    Scalar(PrimitiveType),
    Tuple(Vec<PrimitiveType>),
}

impl ReturnType {
    /// Returns the scalar type, panicking if this is a tuple.
    pub fn as_scalar(&self) -> PrimitiveType {
        match self {
            ReturnType::Scalar(ty) => *ty,
            ReturnType::Tuple(_) => panic!("expected scalar return type, got tuple"),
        }
    }

    /// Returns the scalar type if this is a scalar return.
    pub fn scalar(&self) -> Option<PrimitiveType> {
        match self {
            ReturnType::Scalar(ty) => Some(*ty),
            ReturnType::Tuple(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedFunction {
    pub name: String,
    pub method_of: Option<String>,
    pub type_params: Vec<String>,
    pub params: Vec<String>,
    pub param_defaults: Vec<Option<Expr>>,
    pub param_kinds: Vec<TypedFnParam>,
    pub return_ty: ReturnType,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedFnParam {
    Scalar {
        ty: Option<PrimitiveType>,
    },
    Struct {
        struct_name: String,
    },
    ProcArray {
        proc_name: String,
        len: usize,
    },
    StructArray {
        struct_name: String,
    },
    Array {
        elem_ty: PrimitiveType,
    },
    Buffer {
        elem_ty: PrimitiveType,
        channels: TypedBufferChannels,
    },
    Tuple {
        elem_tys: Vec<PrimitiveType>,
    },
}

#[derive(Debug, Clone)]
pub struct TypedParam {
    pub name: String,
    pub ty: PrimitiveType,
    pub default: TypedConstValue,
    pub range: Option<TypedValueRange>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypedValueRange {
    pub min: TypedConstValue,
    pub max: TypedConstValue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypedConstValue {
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    Bool(bool),
}

impl TypedConstValue {
    pub fn to_f32(self) -> f32 {
        match self {
            Self::F32(v) => v,
            Self::F64(v) => v as f32,
            Self::I32(v) => v as f32,
            Self::I64(v) => v as f32,
            Self::Bool(v) => {
                if v {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn to_f64(self) -> f64 {
        match self {
            Self::F32(v) => v as f64,
            Self::F64(v) => v,
            Self::I32(v) => v as f64,
            Self::I64(v) => v as f64,
            Self::Bool(v) => {
                if v {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TypedArrayInfo {
    pub elem_ty: PrimitiveType,
    pub len: usize,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct TypedArrayVar {
    pub name: String,
    pub len: usize,
    pub elem_ty: PrimitiveType,
}

#[derive(Debug, Clone)]
pub struct TypedArrayStructRoot {
    pub name: String,
    pub struct_name: String,
    pub len: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ArrayStructRootInfo {
    pub(crate) struct_name: String,
    pub(crate) len: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalArrayAliasInfo {
    pub(crate) len: usize,
    pub(crate) elem_ty: PrimitiveType,
    pub(crate) elem_struct: Option<String>,
    pub(crate) writable: bool,
}

pub(crate) type LocalAliasTypes = HashMap<String, PrimitiveType>;

#[derive(Debug, Clone, Copy)]
pub struct AnalysisOptions {
    pub sample_rate: f32,
    pub block_size: usize,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            block_size: 512,
        }
    }
}

impl TypedProgram {
    pub fn param_default(&self, name: &str) -> Option<f32> {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.default.to_f32())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Init,
    Block,
    Sample,
    Def,
}

fn with_stmt_diag_context<T>(stmt: &Stmt, f: impl FnOnce(DiagCtx) -> T) -> T {
    let diag = DiagCtx::new(stmt.loc());
    f(diag)
}

fn with_expr_diag_context<T>(expr: &Expr, f: impl FnOnce(DiagCtx) -> T) -> T {
    let diag = DiagCtx::new(expr.loc());
    f(diag)
}

fn with_expr_diag_context_mut<T>(expr: &mut Expr, f: impl FnOnce(DiagCtx, &mut Expr) -> T) -> T {
    let loc = expr.loc();
    let diag = DiagCtx::new(loc);
    f(diag, expr)
}

fn with_loc_diag_context<T>(loc: impl Into<SourceLoc>, f: impl FnOnce(DiagCtx) -> T) -> T {
    let loc = loc.into();
    let diag = DiagCtx::new(loc);
    f(diag)
}

fn with_graph_edge_diag_context<T>(edge: &GraphEdge, f: impl FnOnce(DiagCtx) -> T) -> T {
    let diag = DiagCtx::new(edge.loc());
    f(diag)
}

fn with_stmt_diag_context_mut<T>(stmt: &mut Stmt, f: impl FnOnce(DiagCtx, &mut Stmt) -> T) -> T {
    let loc = stmt.loc();
    let diag = DiagCtx::new(loc);
    f(diag, stmt)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use onda_frontend::{parse_program, parse_program_file};

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onda_semantics_{prefix}_{nanos}"));
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
    fn expression_diagnostics_use_identifier_spans() {
        let src = "outs:\n  out1\nsample:\n  out1 = missing + 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unknown symbol should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("unknown symbol 'missing'"))
            .expect("missing unresolved symbol diagnostic");

        assert_eq!((diag.line, diag.column), (4, 10));
        assert_eq!(diag.end_line, 4);
        assert_eq!(diag.end_column, 17);
    }

    #[test]
    fn declaration_diagnostics_use_param_spans() {
        let src = "outs:\n  out1\nparams:\n  gain = 0.5\n  gain = 1.0\nsample:\n  out1 = gain\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("duplicate param should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("duplicate param 'gain'"))
            .expect("missing duplicate param diagnostic");

        assert_eq!((diag.line, diag.column), (5, 3));
    }

    #[test]
    fn top_level_const_array_reads_analyze() {
        let src = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

outs:
  out1

sample:
  out1 = Table[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const array read should analyze");
        assert_eq!(typed.const_arrays.len(), 1);
        let table = &typed.const_arrays[0];
        assert_eq!(table.name, "Table");
        assert_eq!(table.elem_ty, PrimitiveType::F32);
        assert_eq!(table.len, 3);
        assert_eq!(table.values[1], TypedConstValue::F32(0.5));
    }

    #[test]
    fn count_shorthand_expands_in_semantic_preprocessing_from_const_defs() {
        let src = r#"
const def count() -> i32:
  return 3

const N = count()

ins N
outs N
params N
buffers N

sample:
  outs[0] = ins[0] + param1
  outs[1] = ins[1] + param2
  outs[2] = ins[2] + param3
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("semantic count expansion should analyze");

        assert_eq!(typed.ins, vec!["in1", "in2", "in3"]);
        assert_eq!(typed.outs, vec!["out1", "out2", "out3"]);
        assert_eq!(typed.params.len(), 3);
        assert_eq!(typed.buffers.len(), 3);
    }

    #[test]
    fn count_shorthand_accepts_direct_const_def_calls() {
        let src = r#"
const def count() -> i32:
  return 2

ins (count())
outs (count())

sample:
  outs[0] = ins[0]
  outs[1] = ins[1]
"#;
        let program = parse_program(src).expect("parse should preserve direct const def count");
        let typed = analyze(program).expect("direct const def count should analyze");

        assert_eq!(typed.ins, vec!["in1", "in2"]);
        assert_eq!(typed.outs, vec!["out1", "out2"]);
    }

    #[test]
    fn proc_local_scalar_consts_expand_counts_in_semantics() {
        let src = r#"
const def count() -> i32:
  return 2

proc Voice:
  const N = count()
  ins N
  outs N
  sample:
    out1 = in1
    out2 = in2

outs 2
init:
  v = Voice()
sample:
  v(0.25, 0.5)
  outs[0] = v.out1
  outs[1] = v.out2
"#;
        let program = parse_program(src).expect("parse should preserve proc local consts");
        let typed = analyze(program).expect("proc local const counts should analyze");

        assert_eq!(typed.outs, vec!["out1", "out2"]);
    }

    #[test]
    fn statement_local_scalar_consts_call_const_defs_in_semantics() {
        let src = r#"
const def gain() -> f32:
  return 0.5

outs:
  out1

sample:
  const G = gain()
  out1 = G
"#;
        let program = parse_program(src).expect("parse should preserve local consts");
        let typed = analyze(program).expect("statement local const should analyze");

        assert_eq!(typed.sample.len(), 1);
        assert!(!matches!(typed.sample[0], Stmt::Const { .. }));
    }

    #[test]
    fn assignment_to_statement_local_const_is_rejected_in_semantics() {
        let src = r#"
outs:
  out1

sample:
  const X = 1
  X = 2
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve local consts");
        let errors = analyze(program).expect_err("assignment to local const should fail");
        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("cannot assign to constant 'X'")));
    }

    #[test]
    fn proc_local_const_arrays_are_rejected_in_semantics() {
        let src = r#"
proc Voice:
  const Table = [1, 2]
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v.out1
"#;
        let program = parse_program(src).expect("parse should preserve proc local const array");
        let errors = analyze(program).expect_err("proc local const array should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const arrays are only supported at top-level and namespace scope")));
    }

    #[test]
    fn count_shorthand_rejects_forward_const_def_calls() {
        let src = r#"
ins N
outs 1

const def count() -> i32:
  return 2

const N = count()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward count const def should fail");
        assert!(errors.iter().any(|diag| {
            diag.message
                .contains("ins count expression uses non-constant symbol 'N'")
        }));
    }

    #[test]
    fn count_prefix_mismatch_is_reported_in_semantics() {
        let src = r#"
const def count() -> i32:
  return 2

const N = count()

ins N:
  in1

outs 1
sample:
  out1 = in1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("count/list mismatch should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("ins block count (2) does not match explicit declaration count (1)")));
    }

    #[test]
    fn count_shorthand_zero_diagnostic_uses_count_span() {
        let src = "outs 0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("zero count should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("outs count expression must be greater than zero")
            })
            .expect("missing count diagnostic");

        assert_eq!((diag.line, diag.column), (1, 6));
        assert_eq!(diag.end_line, 1);
        assert_eq!(diag.end_column, 7);
    }

    #[test]
    fn scalar_const_validation_diagnostics_use_expr_span() {
        let src = "const X = foo\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should preserve invalid const");
        let errors = analyze(program).expect_err("invalid const should fail in semantics");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("const scalar 'X' uses non-constant symbol 'foo'")
            })
            .expect("missing const validation diagnostic");

        assert_eq!((diag.line, diag.column), (1, 11));
        assert_eq!(diag.end_line, 1);
        assert_eq!(diag.end_column, 14);
    }

    #[test]
    fn direct_const_def_calls_fold_in_array_sizes_defaults_and_oversampling() {
        let src = r#"
const def count() -> i32:
  return 2

const def values() -> f32[2]:
  return [0.25, 0.75]

params:
  taps: f32[count()] = values()

outs:
  out1

init:
  state: f32[count()]

sample count():
  out1 = taps[0] + taps[1] + state[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("direct const def preprocessing should analyze");

        assert_eq!(typed.sample_oversample_factor, 2);
        assert_eq!(typed.param_arrays.get("taps").map(|info| info.len), Some(2));
        let tap_defaults = typed
            .params
            .iter()
            .map(|param| (param.name.as_str(), param.default))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            tap_defaults.get("taps[0]"),
            Some(&TypedConstValue::F32(0.25))
        );
        assert_eq!(
            tap_defaults.get("taps[1]"),
            Some(&TypedConstValue::F32(0.75))
        );
        let state = typed
            .array_vars
            .iter()
            .find(|array| array.name == "state")
            .expect("typed state array");
        assert_eq!(state.len, 2);
    }

    #[test]
    fn direct_const_def_calls_fold_in_asserts_and_graph_delays() {
        let src = r#"
namespace Check:
  const def ok() -> bool:
    return true

  assert(ok())

const def delay() -> i32:
  return 2

ins 1
outs 1

graph:
  in1 >>[delay()] out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("direct const def graph delay should analyze");
    }

    #[test]
    fn const_def_signature_sizes_can_call_earlier_const_defs() {
        let src = r#"
const def count() -> i32:
  return 3

const def values(xs: f32[count()]) -> f32[count()]:
  return xs

const Table: f32[count()] = values([0.25, 0.5, 0.75])

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def signature sizes should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const table");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::F32(0.75));
    }

    #[test]
    fn namespace_template_args_accept_semantic_scalar_consts() {
        let src = r#"
const def count() -> i32:
  return 4

const Size = count()

namespace LUT<N = 2>:
  const Value = N
  const Table: i32[N] = [0, 1, 2, 3]

outs:
  out1

sample:
  out1 = f32(LUT<Size>::Value)
"#;
        let program = parse_program(src).expect("parse should preserve semantic template arg");
        let typed = analyze(program).expect("semantic namespace template arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 4);
        assert_eq!(table.values[3], TypedConstValue::I32(3));
    }

    #[test]
    fn namespace_template_args_can_shadow_semantic_scalar_const_names() {
        let src = r#"
const def count() -> i32:
  return 5

const N = count()

namespace LUT<N = 2>:
  const Value = N

outs:
  out1

sample:
  out1 = f32(LUT<N>::Value)
"#;
        let program = parse_program(src).expect("parse should preserve shadowing template arg");
        analyze(program).expect("shadowing semantic namespace template arg should analyze");
    }

    #[test]
    fn namespace_alias_args_accept_semantic_scalar_consts() {
        let src = r#"
const def count() -> i32:
  return 3

const Size = count()

namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30]

namespace Picked = LUT<Size>

outs:
  out1

sample:
  out1 = f32(Picked::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve semantic alias arg");
        let typed = analyze(program).expect("semantic namespace alias arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(30));
    }

    #[test]
    fn namespace_template_args_accept_direct_const_def_calls() {
        let src = r#"
const def count() -> i32:
  return 3

namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30]

namespace Picked = LUT<count()>

outs:
  out1

sample:
  out1 = f32(Picked::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve direct const def arg");
        let typed = analyze(program).expect("direct const def namespace arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(30));
    }

    #[test]
    fn namespace_template_defaults_accept_direct_const_def_calls() {
        let src = r#"
namespace Outer:
  const def count() -> i32:
    return 3

  namespace Inner<N = count()>:
    const Table: i32[N] = [0, 2, 4]

outs:
  out1

sample:
  out1 = f32(Outer::Inner::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve direct const def default");
        let typed = analyze(program).expect("direct const def namespace default should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(4));
    }

    #[test]
    fn nested_namespace_template_defaults_accept_semantic_scalar_consts() {
        let src = r#"
namespace Outer:
  const def count() -> i32:
    return 3

  const Size = count()

  namespace Inner<N = Size>:
    const Table: i32[N] = [0, 2, 4]

outs:
  out1

sample:
  out1 = f32(Outer::Inner::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve semantic nested default");
        let typed = analyze(program).expect("semantic namespace default should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(4));
    }

    #[test]
    fn nested_namespace_const_arrays_and_const_defs_are_usable_from_code() {
        let src = r#"
namespace Outer<A = 2>:
  namespace Inner<B = 3>:
    const def ramp() -> f32[B]:
      values: f32[B]
      for i in 0..B:
        values[i] = f32(A + i)
      return values

    const Table: f32[B] = ramp()

    namespace Leaf<C = A + B>:
      const Value = Table[1] + f32(C)

outs:
  out1

sample:
  out1 = Outer<2>::Inner<3>::Leaf::Value + Outer<2>::Inner<3>::Table[2]
"#;
        let program = parse_program(src).expect("parse should preserve nested namespace use");
        let typed = analyze(program).expect("nested namespace consts should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("nested namespace const table");
        assert_eq!(table.len, 3);
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(2.0),
                TypedConstValue::F32(3.0),
                TypedConstValue::F32(4.0)
            ]
        );
    }

    #[test]
    fn namespace_template_instantiations_dedup_by_evaluated_const_values() {
        let src = r#"
const def count() -> i32:
  return 3

namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30]

outs:
  out1

sample:
  out1 = f32(LUT<3>::Table[0] + LUT<count()>::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve namespace instantiations");
        let typed = analyze(program).expect("deduped namespace instantiations should analyze");
        let tables = typed
            .const_arrays
            .iter()
            .filter(|array| array.name.contains("LUT__nsinst") && array.name.ends_with("::Table"))
            .collect::<Vec<_>>();

        assert_eq!(tables.len(), 1, "const arrays: {:?}", typed.const_arrays);
        assert_eq!(tables[0].values[2], TypedConstValue::I32(30));
    }

    #[test]
    fn imported_namespaces_can_provide_const_arrays_and_const_defs() {
        let dir = mk_temp_dir("imported_namespaced_consts");
        let main = dir.join("main.onda");
        let lib = dir.join("lib.onda");

        write_file(
            &lib,
            r#"
namespace Imported:
  const def offset() -> i32:
    return 2

  namespace Tables<N = offset()>:
    const def ramp() -> f32[N]:
      values: f32[N]
      for i in 0..N:
        values[i] = f32(i + offset())
      return values

    const Table: f32[N] = ramp()
"#,
        );
        write_file(
            &main,
            r#"
import lib

outs:
  out1

sample:
  out1 = Imported::Tables::Table[1]
"#,
        );

        let program =
            parse_program_file(&main).expect("program with namespace import should parse");
        let typed = analyze(program).expect("imported namespace consts should analyze");
        fs::remove_dir_all(&dir).ok();

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("imported namespace const table");
        assert_eq!(table.len, 2);
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(2.0), TypedConstValue::F32(3.0)]
        );
    }

    #[test]
    fn namespace_template_args_with_runtime_symbols_fail_in_semantics() {
        let src = r#"
namespace LUT<N = 2>:
  const Value = N

outs:
  out1

sample:
  idx = 3
  out1 = f32(LUT<idx>::Value)
"#;
        let program = parse_program(src).expect("parse should preserve template arg for semantics");
        let errors = analyze(program).expect_err("runtime namespace template arg should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("namespace template")
                    && diag.message.contains("LUT")
                    && diag.message.contains("uses non-constant symbol 'idx'")
            }),
            "diagnostics: {errors:?}"
        );
    }

    #[test]
    fn duplicate_namespace_templates_and_aliases_fail_in_semantics() {
        let src = r#"
namespace Config<N = 1>:
  const Value = N

namespace Config<N = 1>:
  const Value = N

namespace Picked = Config<1>
namespace Picked = Config<1>

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("duplicates should parse for semantic diagnostics");
        let errors = analyze(program).expect_err("duplicate namespaces should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("duplicate namespace template 'Config'")));
        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("duplicate namespace alias 'Picked'")));
    }

    #[test]
    fn namespace_template_argument_errors_are_semantic_diagnostics() {
        let cases = [
            (
                "too many positional arguments",
                r#"
namespace Data<S = 1, C = 1>:
  const Value = S + C

outs:
  out1

sample:
  out1 = f32(Data<1, 2, 3>::Value)
"#,
                "namespace template 'Data' received too many positional arguments",
            ),
            (
                "unknown named argument",
                r#"
namespace Data<S = 1, C = 1>:
  const Value = S + C

outs:
  out1

sample:
  out1 = f32(Data<Rows = 4>::Value)
"#,
                "namespace template 'Data' received unknown named arguments: Rows",
            ),
            (
                "duplicate named argument",
                r#"
namespace Data<S = 1, C = 1>:
  const Value = S + C

outs:
  out1

sample:
  out1 = f32(Data<C = 2, C = 3>::Value)
"#,
                "namespace template 'Data' argument 'C' specified more than once",
            ),
            (
                "unknown namespace template",
                r#"
outs:
  out1

sample:
  out1 = f32(Missing<1>::Value)
"#,
                "unknown namespace template 'Missing'",
            ),
        ];

        for (label, src, expected) in cases {
            let program = parse_program(src).unwrap_or_else(|err| panic!("{label}: {err:?}"));
            let errors = analyze(program).expect_err(label);
            assert!(
                errors.iter().any(|diag| diag.message.contains(expected)),
                "{label}: expected {expected:?}, got {errors:?}"
            );
        }
    }

    #[test]
    fn const_array_size_with_runtime_symbol_fails_in_semantics() {
        let src = r#"
const Table: f32[BadSize] = [1.0]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve const array size");
        let errors = analyze(program).expect_err("invalid const array size should fail");
        assert!(
            errors.iter().any(|diag| {
                let message = &diag.message;
                message.contains("const array 'Table' size uses non-constant symbol 'BadSize'")
                    || message.contains(
                        "const array 'Table' size must be a compile-time integer constant expression",
                    )
            }),
            "diagnostics: {errors:?}"
        );
    }

    #[test]
    fn const_array_writes_are_rejected() {
        let src = r#"
const Table = [1, 2, 3]

outs:
  out1

sample:
  Table[0] = 4
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array write should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("cannot assign to immutable array alias 'Table'")));
    }

    #[test]
    fn namespaced_const_array_alias_writes_are_rejected() {
        let src = r#"
namespace LUT:
  const Table: f32[2] = [0.25, 0.5]

namespace Picked = LUT

outs:
  out1

sample:
  Picked::Table[0] = 0.0
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("namespaced const array write should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("cannot assign to immutable array alias 'LUT::Table'")),
            "diagnostics: {errors:?}"
        );
    }

    #[test]
    fn const_array_unsafe_write_is_rejected() {
        let src = r#"
const Table = [1, 2, 3]

outs:
  out1

sample:
  unsafe_write(Table, 0, 4)
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array unsafe_write should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("builtin 'unsafe_write' cannot write immutable array alias 'Table'")));
    }

    #[test]
    fn const_array_method_unsafe_write_is_rejected() {
        let src = r#"
const Table = [1, 2, 3]

outs:
  out1

sample:
  Table.unsafe_write(0, 4)
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array method unsafe_write should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("builtin 'unsafe_write' cannot write immutable array alias 'Table'")));
    }

    #[test]
    fn const_arrays_can_be_passed_to_readonly_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def sum_first_last(arr: f32[]):
  return arr[0] + arr[arr.len() - 1]

outs:
  out1

sample:
  out1 = sum_first_last(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array readonly def arg should analyze");
    }

    #[test]
    fn const_array_slices_can_be_passed_to_readonly_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def first(arr: f32[]):
  return arr[0]

outs:
  out1

sample:
  out1 = first(Table[:])
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array slice readonly def arg should analyze");
    }

    #[test]
    fn const_arrays_cannot_be_passed_to_mutating_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_first(arr: f32[]):
  arr[0] = 0.0
  return arr[0]

outs:
  out1

sample:
  out1 = write_first(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array mutable def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn namespaced_const_arrays_cannot_be_passed_to_mutating_array_params() {
        let src = r#"
namespace LUT:
  const Table: f32[3] = [1.0, 2.0, 3.0]

namespace Picked = LUT

def write_first(arr: f32[]):
  arr[0] = 0.0
  return arr[0]

outs:
  out1

sample:
  out1 = write_first(Picked::Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("namespaced const array mutable arg should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "cannot pass immutable array alias 'LUT::Table' to mutable array parameter 'arr'"
            )),
            "diagnostics: {errors:?}"
        );
    }

    #[test]
    fn const_arrays_cannot_be_passed_to_unsafe_write_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_first(arr: f32[]):
  unsafe_write(arr, 0, 0.0)
  return arr[0]

outs:
  out1

sample:
  out1 = write_first(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array unsafe_write def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn const_arrays_cannot_be_passed_to_method_unsafe_write_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_first(arr: f32[]):
  arr.unsafe_write(0, 0.0)
  return arr[0]

outs:
  out1

sample:
  out1 = write_first(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("const array method unsafe_write def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn const_arrays_cannot_be_passed_through_mutating_array_alias_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_alias(arr: f32[]):
  view = arr[:]
  view[0] = 0.0
  return view[0]

outs:
  out1

sample:
  out1 = write_alias(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array alias mutable def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn const_arrays_can_be_forwarded_through_readonly_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def first(arr: f32[]):
  return arr[0]

def wrap(arr: f32[]):
  return first(arr)

outs:
  out1

sample:
  out1 = wrap(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array readonly forwarded def arg should analyze");
    }

    #[test]
    fn const_arrays_cannot_be_forwarded_to_mutating_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_first(arr: f32[]):
  arr[0] = 0.0
  return arr[0]

def wrap(arr: f32[]):
  return write_first(arr)

outs:
  out1

sample:
  out1 = wrap(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("const array forwarded mutable def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn def_bodies_can_read_const_arrays() {
        let src = r#"
const Table = [1.0, 2.0, 3.0]

def pick(i: i32):
  return Table[i]

outs:
  out1

sample:
  out1 = pick(1)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("def const array read should analyze");
    }

    #[test]
    fn const_array_len_and_static_index_are_compile_time_evaluable() {
        let src = r#"
const Table: i32[3] = [2, 4, 8]
const Picked = Table[2]

namespace Check:
  assert(Table.len() == 3)
  assert(Picked == 8)

outs:
  out1

sample:
  out1 = f32(Table[1])
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array compile-time reads should analyze");
    }

    #[test]
    fn const_array_static_index_oob_is_semantic_error() {
        let src = r#"
const Table = [1, 2]

namespace Check:
  assert(Table[2] == 0)

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("oob const array index should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const array 'Table' index 2 is out of bounds for length 2")));
    }

    #[test]
    fn const_array_values_can_initialize_fixed_array_defaults() {
        let src = r#"
const Spread: f32[2] = [0.2, 0.8]

ins:
  freqs: f32[2] = Spread

params:
  pan: f32[2] = Spread

outs:
  out1

sample:
  out1 = freqs[0] + pan[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const array defaults should analyze");

        assert_eq!(
            typed.in_defaults.get("freqs[0]"),
            Some(&TypedConstValue::F32(0.2))
        );
        assert_eq!(
            typed.in_defaults.get("freqs[1]"),
            Some(&TypedConstValue::F32(0.8))
        );

        let pan_defaults = typed
            .params
            .iter()
            .map(|param| (param.name.as_str(), param.default))
            .collect::<HashMap<_, _>>();
        assert_eq!(pan_defaults.get("pan[0]"), Some(&TypedConstValue::F32(0.2)));
        assert_eq!(pan_defaults.get("pan[1]"), Some(&TypedConstValue::F32(0.8)));
    }

    #[test]
    fn const_array_fixed_array_defaults_require_matching_length() {
        let src = r#"
const Spread: f32[3] = [0.2, 0.5, 0.8]

ins:
  freqs: f32[2] = Spread

outs:
  out1

sample:
  out1 = freqs[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-length const array default should fail");

        assert!(errors.iter().any(|diag| diag.message.contains(
            "input 'freqs' default const array 'Spread' has type f32[3], expected f32[2]"
        )));
    }

    #[test]
    fn const_array_fixed_array_defaults_require_matching_element_type() {
        let src = r#"
const Spread: f32[2] = [0.2, 0.8]

params:
  pan: f64[2] = Spread

outs:
  out1

sample:
  out1 = f32(pan[0])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-type const array default should fail");

        assert!(errors.iter().any(|diag| diag.message.contains(
            "param '<top-level>.pan' default const array 'Spread' has type f32[2], expected f64[2]"
        )));
    }

    #[test]
    fn const_array_event_defaults_require_matching_element_type() {
        let src = r#"
const Curve: f32[2] = [0.25, 0.75]

init:
  value = 0.0

event set_curve(curve: f64[2] = Curve):
  value = f32(curve[0])

outs:
  out1

sample:
  out1 = value
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("wrong-type event const array default should fail");

        assert!(errors.iter().any(|diag| diag.message.contains(
            "event 'set_curve.curve' default const array 'Curve' has type f32[2], expected f64[2]"
        )));
    }

    #[test]
    fn scalar_const_defs_can_initialize_const_array_elements() {
        let src = r#"
const def twice(x: f32) -> f32:
  return x * 2.0

const Table: f32[2] = [twice(0.5), twice(1.0)]

outs:
  out1

sample:
  out1 = Table[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def call in const array should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(1.0), TypedConstValue::F32(2.0)]
        );
        assert!(
            typed.defs.iter().all(|def| def.name != "twice"),
            "const defs should not be emitted as runtime defs"
        );
    }

    #[test]
    fn scalar_const_defs_can_call_earlier_const_defs() {
        let src = r#"
const def base() -> i32:
  return 21

const def doubled() -> i32:
  return base() * 2

const Table = [doubled()]

outs:
  out1

sample:
  out1 = f32(Table[0])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def calling earlier const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::I32);
        assert_eq!(table.values, vec![TypedConstValue::I32(42)]);
    }

    #[test]
    fn const_def_return_types_are_required_and_enforced() {
        let src = r#"
const def missing():
  return 1.0

const Table: f32[1] = [missing()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("missing return type should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'missing' must declare an explicit return type")));

        let src = r#"
const def bad_scalar() -> i32:
  return 0.5

const Table: i32[1] = [bad_scalar()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong scalar return type should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'bad_scalar' return must be an integer constant")));

        let src = r#"
const def bad_array() -> f32[2]:
  return [0.25]

const Table: f32[2] = bad_array()

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong array return shape should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'bad_array' return: expected array length 2, got 1")),
            "expected const def return shape diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn scalar_const_defs_can_initialize_scalar_consts() {
        let src = r#"
const def curve_gain(x: f64) -> f64:
  return x * x + 0.12345678901234568

const Gain = curve_gain(0.5)
const Table: f64[1] = [Gain]

outs:
  out1

sample:
  out1 = f32(Gain)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def scalar const should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::F64);
        assert_eq!(
            table.values,
            vec![TypedConstValue::F64(
                0.5_f64 * 0.5_f64 + 0.12345678901234568
            )]
        );
    }

    #[test]
    fn scalar_consts_from_const_defs_preserve_i64_precision() {
        let src = r#"
const def big() -> i64:
  return 9007199254740993

const Big = big()
const Table: i64[1] = [Big]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("i64 const def scalar const should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::I64);
        assert_eq!(table.values, vec![TypedConstValue::I64(9007199254740993)]);
    }

    #[test]
    fn scalar_consts_can_depend_on_semantic_scalar_consts() {
        let src = r#"
const def base() -> f64:
  return 0.25

const A = base()
const B: f64 = A + 0.125
const Table: f64[1] = [B]

outs:
  out1

sample:
  out1 = f32(B)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dependent semantic scalar consts should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::F64(0.375)]);
    }

    #[test]
    fn namespace_scalar_consts_from_const_defs_initialize_const_arrays() {
        let src = r#"
namespace LUT:
  const def gain() -> f32:
    return 0.25

  const Gain = gain()
  const Table: f32[1] = [Gain]

outs:
  out1

sample:
  out1 = LUT::Table[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("namespaced semantic scalar const should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::F32(0.25)]);
    }

    #[test]
    fn const_defs_reject_forward_references_from_bodies() {
        let src = r#"
const def earlier() -> f32:
  return later()

const def later() -> f32:
  return 1.0

const Table: f32[1] = [earlier()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const def call should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'later' is not visible from const def 'earlier'")));
    }

    #[test]
    fn const_defs_reject_forward_references_from_param_defaults() {
        let src = r#"
const def earlier(x: f32 = later()) -> f32:
  return x

const def later() -> f32:
  return 1.0

const Table: f32[1] = [earlier()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const def default should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'later' is not visible from const def 'earlier'")));
    }

    #[test]
    fn const_defs_reject_direct_recursion() {
        let src = r#"
const def recurse() -> f32:
  return recurse()

const Table: f32[1] = [recurse()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("recursive const def should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("recursive const def call involving 'recurse'")));
    }

    #[test]
    fn const_defs_reject_mutual_recursion() {
        let src = r#"
const def a() -> f32:
  return b()

const def b() -> f32:
  return a()

const Table: f32[1] = [b()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mutually recursive const defs should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("recursive const def call involving")));
    }

    #[test]
    fn const_defs_reject_runtime_symbol_access() {
        let src = r#"
const def read_input() -> f32:
  return in1

ins:
  in1

const Table: f32[1] = [read_input()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("runtime symbol const def should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("non-constant symbol 'in1'")));
    }

    #[test]
    fn const_defs_reject_ordinary_def_calls() {
        let src = r#"
def runtime_helper() -> f32:
  return 1.0

const def build() -> f32:
  return runtime_helper()

const Table: f32[1] = [build()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("ordinary def const call should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("unknown const def 'runtime_helper'")));
    }

    #[test]
    fn const_defs_report_loop_iteration_cap() {
        let src = r#"
const def runaway() -> i32:
  loop 1000001:
    x = _
  return 0

const Table: i32[1] = [runaway()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const def loop cap should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("loop exceeded 1000000 iterations")));
    }

    #[test]
    fn const_defs_can_read_fixed_array_params() {
        let src = r#"
const Source: f32[3] = [0.25, 0.5, 1.0]

const def mix(xs: f32[3]) -> f32:
  return xs[0] + xs[2]

const Table: f32[2] = [mix(Source), mix([1.0, 2.0, 4.0])]

outs:
  out1

sample:
  out1 = Table[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("fixed-array const def params should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(1.25), TypedConstValue::F32(5.0)]
        );
    }

    #[test]
    fn const_defs_can_return_arrays_derived_from_fixed_array_params() {
        let src = r#"
namespace LUT<N = 3>:
  const Base: f32[N] = [1.0, 2.0, 3.0]

  const def scale(xs: f32[N], gain: f32) -> f32[N]:
    values: f32[N]
    for i in 0..N:
      values[i] = xs[i] * gain
    return values

  const Table: f32[N] = scale(Base, 0.5)

outs:
  out1

sample:
  out1 = LUT::Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array-param const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(0.5),
                TypedConstValue::F32(1.0),
                TypedConstValue::F32(1.5)
            ]
        );
    }

    #[test]
    fn const_defs_can_pass_local_arrays_to_fixed_array_params() {
        let src = r#"
const def copy(xs: f32[2]) -> f32[2]:
  return xs

const def swapped(xs: f32[2]) -> f32[2]:
  values: f32[2]
  values[0] = xs[1]
  values[1] = xs[0]
  return copy(values)

const Table: f32[2] = copy(swapped([1.0, 2.0]))

outs:
  out1

sample:
  out1 = Table[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("local array const-def arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(2.0), TypedConstValue::F32(1.0)]
        );
    }

    #[test]
    fn const_def_fixed_array_params_require_matching_shape() {
        let src = r#"
const Source: f32[2] = [0.25, 0.5]

const def first(xs: f32[3]) -> f32:
  return xs[0]

const Table: f32[1] = [first(Source)]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-shape const def array arg should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'first' argument 'xs': expected f32[3], got f32[2]")));
    }

    #[test]
    fn namespaced_scalar_const_defs_can_initialize_const_arrays() {
        let src = r#"
namespace LUT:
  const def gain() -> f32:
    return 0.25

  const Table: f32[1] = [gain()]

outs:
  out1

sample:
  out1 = LUT::Table[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("namespace const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::F32(0.25)]);
    }

    #[test]
    fn const_defs_can_return_array_literals_for_const_array_initializers() {
        let src = r#"
const def table() -> f32[3]:
  return [0.25, 0.5, 1.0]

const Table: f32[3] = table()

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array-returning const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(0.25),
                TypedConstValue::F32(0.5),
                TypedConstValue::F32(1.0)
            ]
        );
        assert!(
            typed.defs.iter().all(|def| def.name != "table"),
            "const defs should not be emitted as runtime defs"
        );
    }

    #[test]
    fn const_defs_can_fill_local_arrays_with_compile_time_loops() {
        let src = r#"
const def ramp() -> f32[4]:
  values: f32[4]
  for i in 0..4:
    values[i] = f32(i) + 0.5
  return values

const Table: f32[4] = ramp()

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("loop-filled const def array should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(0.5),
                TypedConstValue::F32(1.5),
                TypedConstValue::F32(2.5),
                TypedConstValue::F32(3.5)
            ]
        );
    }

    #[test]
    fn namespace_const_defs_can_return_arrays_using_namespace_sizes() {
        let src = r#"
namespace LUT<N = 3>:
  const def ramp() -> i32[N]:
    values: i32[N]
    loop N:
      values[_] = _ * 2
    return values

  const Table: i32[N] = ramp()

outs:
  out1

sample:
  out1 = f32(LUT::Table[2])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("namespaced array-returning const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::I32);
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::I32(0),
                TypedConstValue::I32(2),
                TypedConstValue::I32(4)
            ]
        );
    }

    #[test]
    fn const_defs_can_build_window_tables_with_builtin_math() {
        let src = r#"
namespace Windows<N = 4>:
  const def hann() -> f32[N]:
    values: f32[N]
    for i in 0..N:
      phase = TWO_PI * f32(i) / f32(N - 1)
      values[i] = 0.5 - 0.5 * cos(phase)
    return values

  const Hann: f32[N] = hann()

outs:
  out1

sample:
  out1 = Windows::Hann[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def hann table should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Hann"))
            .expect("typed const array");
        let values = table
            .values
            .iter()
            .map(|value| match value {
                TypedConstValue::F32(value) => *value,
                other => panic!("expected f32 value, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert!((values[0] - 0.0).abs() < 1e-6);
        assert!((values[1] - 0.75).abs() < 1e-6);
        assert!((values[2] - 0.75).abs() < 1e-6);
        assert!((values[3] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn non_const_defs_reject_array_return_annotations() {
        let src = r#"
def table() -> f32[2]:
  return [0.0, 1.0]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("ordinary def array return should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("function 'table' array return types are only supported for const defs")));
    }

    #[test]
    fn overload_diagnostics_use_call_spans() {
        let src = "outs:\n  out1\ndef foo(x: f32):\n  return x\ndef foo(x: f64):\n  return f32(x)\nsample:\n  out1 = foo(1)\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("ambiguous overload should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("ambiguous overload for function 'foo'")
            })
            .expect("missing ambiguous overload diagnostic");

        assert_eq!((diag.line, diag.column), (8, 10));
        assert_eq!(diag.end_line, 8);
    }

    #[test]
    fn def_body_assignment_diagnostics_use_rhs_spans() {
        let src = "outs:\n  out1\ndef foo():\n  a = [0.0]\n  a[0] = false\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("def body type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("array/buffer write type mismatch"))
            .expect("missing def body assignment diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn def_body_assignment_diagnostics_use_target_spans() {
        let src = "outs:\n  out1\ndef foo():\n  PI = 1.0\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("builtin constant assignment should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("cannot assign to builtin constant 'PI'")
            })
            .expect("missing builtin constant assignment diagnostic");

        assert_eq!((diag.line, diag.column), (4, 3));
        assert_eq!(diag.end_line, 4);
        assert_eq!(diag.end_column, 5);
    }

    #[test]
    fn init_assignment_diagnostics_use_target_spans() {
        let src = "outs:\n  out1\ninit:\n  PI = 1.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("init builtin constant assignment should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("cannot assign to builtin constant 'PI'")
            })
            .expect("missing init builtin constant assignment diagnostic");

        assert_eq!((diag.line, diag.column), (4, 3));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn runtime_assignment_diagnostics_use_target_spans() {
        let src = "outs:\n  out1\nsample:\n  PI = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("sample builtin constant assignment should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("cannot assign to builtin constant 'PI'")
            })
            .expect("missing sample builtin constant assignment diagnostic");

        assert_eq!((diag.line, diag.column), (4, 3));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn init_assignment_diagnostics_use_rhs_spans() {
        let src = "outs:\n  out1\ninit:\n  a = [0.0]\n  a[0] = false\nsample:\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("init array write type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("array/buffer write type mismatch"))
            .expect("missing init array write diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn runtime_assignment_diagnostics_use_rhs_spans() {
        let src = "outs:\n  out1\nsample:\n  a = [0.0]\n  a[0] = false\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("sample array write type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("array/buffer write type mismatch"))
            .expect("missing sample array write diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn runtime_slice_bound_diagnostics_use_bound_spans() {
        let src = "outs:\n  out1\nsample:\n  a = [0.0, 0.0]\n  a[false:] = 0.5\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("slice bound type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("slice start bound requires numeric type")
            })
            .expect("missing slice bound diagnostic");

        assert_eq!((diag.line, diag.column), (5, 5));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn runtime_slice_bound_diagnostics_use_const_use_site_spans() {
        let src = "const BAD = false\nouts:\n  out1\nsample:\n  a = [0.0, 0.0]\n  a[BAD:] = 0.5\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const-expanded slice bound should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("slice start bound requires numeric type")
            })
            .expect("missing slice bound diagnostic");

        assert_eq!((diag.line, diag.column), (6, 5));
        assert_eq!(diag.end_line, 6);
        assert_eq!(diag.end_column, 8);
    }

    #[test]
    fn init_array_literal_empty_diagnostics_use_expr_spans() {
        let src = "outs:\n  out1\ninit:\n  a = []\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("empty init array literal should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("array initializer for symbol 'a' cannot be empty")
            })
            .expect("missing empty array initializer diagnostic");

        assert_eq!((diag.line, diag.column), (4, 7));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn runtime_typed_array_size_diagnostics_use_size_spans() {
        let src = "outs:\n  out1\nsample:\n  a: f32[1.5] = [1.0]\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("non-integer typed array size should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("typed array declaration size for symbol 'a' in sample must evaluate to an integer value")
            })
            .expect("missing typed array size diagnostic");

        assert_eq!((diag.line, diag.column), (4, 10));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn def_array_literal_empty_diagnostics_use_expr_spans() {
        let src = "outs:\n  out1\ndef foo():\n  a = []\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("empty def array literal should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("array initializer for symbol 'a' cannot be empty")
            })
            .expect("missing empty def array initializer diagnostic");

        assert_eq!((diag.line, diag.column), (4, 7));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn def_typed_array_size_diagnostics_use_size_spans() {
        let src =
            "outs:\n  out1\ndef foo():\n  a: f32[1.5] = [1.0]\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("non-integer def typed array size should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("typed array declaration size for symbol 'a' in def must evaluate to an integer value")
            })
            .expect("missing def typed array size diagnostic");

        assert_eq!((diag.line, diag.column), (4, 10));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn proc_array_size_diagnostics_use_size_spans() {
        let src = "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nouts:\n  out1\ninit:\n  voices: Voice[1.5] = Voice()\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("non-integer proc array size should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message.contains(
                    "top-level processor array 'voices' size must evaluate to an integer value",
                )
            })
            .expect("missing proc array size diagnostic");

        assert_eq!((diag.line, diag.column), (9, 17));
        assert_eq!(diag.end_line, 9);
    }

    #[test]
    fn proc_array_initializer_entry_diagnostics_use_entry_spans() {
        let src = "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Other:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nouts:\n  out1\ninit:\n  voices: Voice[2] = [Other(), Voice()]\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mismatched proc array initializer should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message.contains(
                    "top-level processor array 'voices' initializer entry 0 uses constructor 'Other' but 'Voice' is required",
                )
            })
            .expect("missing proc array initializer entry diagnostic");

        assert_eq!((diag.line, diag.column), (14, 23));
        assert_eq!(diag.end_line, 14);
    }

    #[test]
    fn duplicate_block_diagnostics_use_block_spans() {
        let src =
            "outs:\n  out1\nparams:\n  gain = 0.5\nparams:\n  mix = 0.25\nsample:\n  out1 = gain\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("duplicate params block should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("duplicate block 'params'"))
            .expect("missing duplicate block diagnostic");

        assert_eq!((diag.line, diag.column), (5, 1));
    }

    #[test]
    fn missing_sample_diagnostic_uses_nearest_block_span() {
        let src = "outs:\n  out1\nparams:\n  gain = 0.5\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("missing sample block should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("missing required 'sample' block"))
            .expect("missing sample diagnostic");

        assert_eq!((diag.line, diag.column), (3, 1));
    }

    #[test]
    fn untyped_top_level_params_infer_type_from_const_defaults() {
        let src = r#"
outs:
  out1
params:
  bare
  float_default = 0.0
  int_default = 0
  int_expr = 1 + 2
  float_expr = PI * 2.0
  explicit_f64: f64 = 0.0
  explicit_i64: i64 = 0
sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("param defaults should infer");

        assert_eq!(typed.param_types.get("bare"), Some(&PrimitiveType::F32));
        assert_eq!(
            typed.param_types.get("float_default"),
            Some(&PrimitiveType::F32)
        );
        assert_eq!(
            typed.param_types.get("int_default"),
            Some(&PrimitiveType::I32)
        );
        assert_eq!(typed.param_types.get("int_expr"), Some(&PrimitiveType::I32));
        assert_eq!(
            typed.param_types.get("float_expr"),
            Some(&PrimitiveType::F32)
        );
        assert_eq!(
            typed.param_types.get("explicit_f64"),
            Some(&PrimitiveType::F64)
        );
        assert_eq!(
            typed.param_types.get("explicit_i64"),
            Some(&PrimitiveType::I64)
        );
    }

    #[test]
    fn untyped_proc_params_infer_type_from_const_defaults() {
        let src = r#"
proc Voice:
  params:
    bare
    float_default = 0.0
    int_default = 0
    int_expr = 1 + 2
    float_expr = PI * 2.0
    explicit_f64: f64 = 0.0
    explicit_i64: i64 = 0
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

init:
  voice = Voice()

sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc param defaults should infer");
        let voice = typed
            .structs
            .iter()
            .find(|s| s.name == "Voice")
            .expect("missing lowered Voice struct");

        let param_tys = voice
            .fields
            .iter()
            .filter_map(|field| {
                let ty = match field.ty {
                    TypedFieldType::Scalar(ty) => ty,
                    _ => return None,
                };
                match field.name.as_str() {
                    "bare" => Some(("bare", ty)),
                    "float_default" => Some(("float_default", ty)),
                    "int_default" => Some(("int_default", ty)),
                    "int_expr" => Some(("int_expr", ty)),
                    "float_expr" => Some(("float_expr", ty)),
                    "explicit_f64" => Some(("explicit_f64", ty)),
                    "explicit_i64" => Some(("explicit_i64", ty)),
                    _ => None,
                }
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(param_tys.get("bare"), Some(&PrimitiveType::F32));
        assert_eq!(param_tys.get("float_default"), Some(&PrimitiveType::F32));
        assert_eq!(param_tys.get("int_default"), Some(&PrimitiveType::I32));
        assert_eq!(param_tys.get("int_expr"), Some(&PrimitiveType::I32));
        assert_eq!(param_tys.get("float_expr"), Some(&PrimitiveType::F32));
        assert_eq!(param_tys.get("explicit_f64"), Some(&PrimitiveType::F64));
        assert_eq!(param_tys.get("explicit_i64"), Some(&PrimitiveType::I64));
    }

    #[test]
    fn top_level_input_array_defaults_are_typed_per_element() {
        let src = r#"
ins:
  freqs: f32[3] = [220, 440, 880]
outs:
  out1
sample:
  out1 = freqs[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("input array defaults should analyze");

        let freqs = typed
            .in_arrays
            .get("freqs")
            .expect("missing input array info");
        assert_eq!(freqs.elem_ty, PrimitiveType::F32);
        assert_eq!(freqs.len, 3);
        assert_eq!(freqs.offset, 0);
        assert_eq!(
            typed.in_defaults.get("freqs[0]"),
            Some(&TypedConstValue::F32(220.0))
        );
        assert_eq!(
            typed.in_defaults.get("freqs[1]"),
            Some(&TypedConstValue::F32(440.0))
        );
        assert_eq!(
            typed.in_defaults.get("freqs[2]"),
            Some(&TypedConstValue::F32(880.0))
        );
    }

    #[test]
    fn input_array_defaults_require_exact_length() {
        let src = r#"
ins:
  freqs: f32[3] = [220, 440]
outs:
  out1
sample:
  out1 = freqs[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-length input default should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("input 'freqs' default expects 3 elements, got 2")),
            "expected array-length diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn proc_input_and_param_array_defaults_work_for_generic_processors() {
        let src = r#"
proc Voice<T>:
  ins:
    freqs: T[3] = [220, 440, 880]
  params:
    amps: T[2] = [0.5, 0.25]
  outs:
    out1
  sample:
    out1 = freqs[2] * amps[1]

outs:
  out1

init:
  voice = Voice<f32>()

sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("generic proc array defaults should analyze");
    }

    #[test]
    fn declaration_only_library_file_does_not_require_sample_block() {
        let src = "proc Mix:\n  ins:\n    dry\n    fb\n  sample:\n    out1 = (dry + fb) * 0.5\n\ndef clip(x) {\n  return x\n}\nconst SCALE = 0.5\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declaration-only library file should analyze");
    }

    #[test]
    fn init_buffer_len_is_rejected_semantically() {
        let src = "buffers:\n  src: buffer[f32]\nouts:\n  out1\ninit:\n  n = src.len()\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("buffer len in init should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("buffer method 'src.len()'"))
            .expect("missing init buffer len diagnostic");

        assert!(diag.message.contains("not allowed in init"));
        assert_eq!((diag.line, diag.column), (6, 7));
        assert_eq!(diag.end_line, 6);
    }

    #[test]
    fn init_buffer_index_is_rejected_semantically() {
        let src = "buffers:\n  src: buffer[f32]\nouts:\n  out1\ninit:\n  first = src[0]\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("buffer indexing in init should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("buffer indexing 'src[...]'"))
            .expect("missing init buffer indexing diagnostic");

        assert!(diag.message.contains("not allowed in init"));
        assert_eq!((diag.line, diag.column), (6, 11));
        assert_eq!(diag.end_line, 6);
    }

    #[test]
    fn block_without_nested_sample_reports_only_block_specific_error() {
        let src = "outs { out1 }\nblock { x = 0.0 }\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block without nested sample should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("block section must include nested 'sample' block")),
            "missing block-specific diagnostic"
        );
        assert!(
            !errors
                .iter()
                .any(|diag| diag.message.contains("missing required 'sample' block")),
            "unexpected duplicate missing-sample diagnostic"
        );
    }

    #[test]
    fn def_returns_must_share_a_compatible_type() {
        let src = "outs:\n  out1\ndef test():\n  if true:\n    return 0\n  return (0, 1)\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mixed scalar/tuple returns should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("return in function 'test' type mismatch")
                    && diag.message.contains("cannot assign (i32, i32)")
            })
            .expect("missing incompatible return diagnostic");

        assert_eq!((diag.line, diag.column), (6, 10));
        assert_eq!(diag.end_line, 6);
    }

    #[test]
    fn explicit_def_return_type_allows_implicit_widening() {
        let src = "outs:\n  out1\ndef widen(x: i32) -> i64:\n  return x\nsample:\n  out1 = f32(widen(1))\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("explicit widening return annotation should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "widen")
            .expect("missing typed def");
        assert_eq!(def.return_ty, ReturnType::Scalar(PrimitiveType::I64));
    }

    #[test]
    fn explicit_def_return_type_rejects_implicit_narrowing() {
        let src =
            "outs:\n  out1\ndef narrow() -> i32:\n  return 3.5\nsample:\n  out1 = f32(narrow())\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("implicit narrowing return should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("return in function 'narrow'")
                    && diag.message.contains("cannot assign F32 to I32")
            }),
            "expected return mismatch diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn generic_def_return_annotation_specializes_through_monomorphization() {
        let src = "outs:\n  out1\ndef id<T>(x: T) -> T:\n  return x\nsample:\n  out1 = id(0.5)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("generic return annotation should analyze");
        assert!(
            typed.defs.iter().any(|def| {
                def.name.contains("id.__mono")
                    && def.return_ty == ReturnType::Scalar(PrimitiveType::F32)
            }),
            "expected monomorphized id def with f32 return, got {:#?}",
            typed.defs
        );
    }

    #[test]
    fn explicit_tuple_return_type_analyzes_and_sets_typed_return() {
        let src = "outs:\n  out1\ndef pair(x: f32) -> (f32, i32):\n  return (x, 1)\nsample:\n  vals = pair(0.5)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("explicit tuple return annotation should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "pair")
            .expect("missing typed def");
        assert_eq!(
            def.return_ty,
            ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        );
    }

    #[test]
    fn explicit_tuple_return_type_rejects_element_mismatch() {
        let src = "outs:\n  out1\ndef pair() -> (f32, i32):\n  return (1.0, 2.5)\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("tuple return element mismatch should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("return in function 'pair'")
                    && diag.message.contains("cannot assign F32 to I32")
            }),
            "expected tuple return mismatch diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn generic_tuple_return_annotation_specializes_through_monomorphization() {
        let src = "outs:\n  out1\ndef pair<T>(x: T, y: i32) -> (T, i32):\n  return (x, y)\nsample:\n  vals = pair(0.5, 2)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("generic tuple return annotation should analyze");
        assert!(
            typed.defs.iter().any(|def| {
                def.name.contains("pair.__mono")
                    && def.return_ty
                        == ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
            }),
            "expected monomorphized pair def with tuple return, got {:#?}",
            typed.defs
        );
    }

    #[test]
    fn unannotated_defs_still_infer_tuple_returns() {
        let src = "outs:\n  out1\ndef pair(x):\n  return (x, 1)\nsample:\n  vals = pair(0.5)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("unannotated tuple return should still infer");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.contains("pair"))
            .expect("missing typed def");
        assert_eq!(
            def.return_ty,
            ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        );
    }

    #[test]
    fn return_annotations_do_not_change_overload_resolution_behavior() {
        let src = "outs:\n  out1\ndef foo(x: f32) -> f32:\n  return x\ndef foo(x: f64) -> f32:\n  return f32(x)\nsample:\n  out1 = foo(1)\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("annotated ambiguous overload should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message
                    .contains("ambiguous overload for function 'foo'")
            }),
            "expected ambiguous overload diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn generic_struct_method_return_annotation_specializes_with_owner_generics() {
        let src = "struct Pair<T>:\n  a: T\n  b: T\n\n  def swap(self) -> (T, T):\n    return (self.b, self.a)\n\nouts:\n  out1\ninit:\n  p = Pair<f32>(1.0, 2.0)\nsample:\n  vals = p.swap()\n  out1 = vals[0] + vals[1]\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed =
            analyze(program).expect("generic struct method return annotation should analyze");
        assert!(
            typed.defs.iter().any(|def| {
                def.name.ends_with(".swap")
                    && def.method_of.is_some()
                    && def.return_ty
                        == ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::F32])
            }),
            "expected specialized swap method def with tuple return, got {:#?}",
            typed.defs
        );
    }

    #[test]
    fn proc_local_def_return_annotation_lowers_and_validates() {
        let src = "proc Voice:\n  outs:\n    out1\n\n  def pair(x: f32) -> (f32, i32):\n    return (x, 1)\n\n  sample:\n    vals = pair(0.5)\n    out1 = vals[0] + f32(vals[1])\n\ninit:\n  voice = Voice()\n\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-local return annotation should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.contains("Voice.__proc_local__pair"))
            .expect("missing lowered proc-local def");
        assert_eq!(
            def.return_ty,
            ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        );
        assert!(matches!(
            def.param_kinds.first(),
            Some(TypedFnParam::Struct { struct_name }) if struct_name == "Voice"
        ));
    }

    #[test]
    fn struct_return_annotation_is_rejected() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef borrow(pair: Pair) -> Pair:\n  return pair\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("struct return annotation should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message
                    .contains("function 'borrow' return type 'Pair' is not supported")
            }),
            "expected unsupported struct return diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn namespaced_struct_return_annotation_is_rejected_after_rewrite() {
        let src = "namespace dsp:\n  struct Pair:\n    x\nouts:\n  out1\ndef borrow(pair: dsp::Pair) -> dsp::Pair:\n  return pair\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("namespaced struct return annotation should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message
                    .contains("function 'borrow' return type 'dsp::Pair' is not supported")
            }),
            "expected unsupported namespaced return diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn sample_tuple_local_survives_if_merge() {
        let src = "outs:\n  out1\nsample:\n  if true:\n    vals = (0.5, 1)\n  else:\n    vals = (0.25, 2)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("tuple local should survive if merge");
    }

    #[test]
    fn sample_tuple_local_survives_loop_reassignment() {
        let src = "outs:\n  out1\nsample:\n  vals = (0.0, 0)\n  for i in 0..2:\n    vals = (f32(i), i)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("tuple local should survive loop reassignment");
    }

    #[test]
    fn def_tuple_local_survives_if_merge() {
        let src = "outs:\n  out1\ndef pick(flag: bool) -> f32:\n  if flag:\n    vals = (0.5, 1)\n  else:\n    vals = (0.25, 2)\n  return vals[0] + f32(vals[1])\nsample:\n  out1 = pick(true)\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("tuple local in def should survive if merge");
    }

    #[test]
    fn tuple_tracking_clears_after_scalar_reassignment() {
        let src = "outs:\n  out1\nsample:\n  vals = (0.5, 1)\n  vals = 0.5\n  out1 = vals[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("stale tuple tracking should be rejected");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("indexed expression 'vals[...]' is not a array/buffer symbol")),
            "expected stale tuple indexing diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn namespaced_proc_array_typed_declaration_analyzes() {
        let src = "import std/osc\nouts:\n  out1\ninit:\n  voices: std::osc::Sine[2] = std::osc::Sine()\nsample:\n  out1 = voices[0]()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("namespaced proc array typed declaration should analyze");
    }

    #[test]
    fn def_accepts_proc_array_param_for_indexed_init_events() {
        let src = "import std/osc\nouts:\n  out1\ndef init_voices(voices):\n  for i in 0..2:\n    voices[i].init(freq = 110.0)\ninit:\n  voices: std::osc::Sine[2]\n  init_voices(voices)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-array def parameter should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "init_voices")
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::ProcArray {
                    proc_name,
                    len: 2
                }] if proc_name.starts_with("std::osc::Sine")
            ),
            "expected proc-array param kind, got {:#?}",
            def.param_kinds
        );
        assert!(
            !def.body.iter().any(def_stmt_contains_proc_index_sentinel),
            "proc-array indexed event call should be rewritten before typed def lowering: {:#?}",
            def.body
        );
    }

    #[test]
    fn def_accepts_proc_array_param_for_indexed_field_assignments() {
        let src = "proc Voice:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nouts:\n  out1\ndef set_gains(voices, gain):\n  for i in 0..2:\n    voices[i].gain = gain + f32(i)\ninit:\n  voices: Voice[2] = Voice()\n  set_gains(voices, 1.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-array indexed field assignment should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "set_gains")
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::ProcArray { proc_name, len: 2 }, TypedFnParam::Scalar { .. }]
                    if proc_name == "Voice"
            ),
            "expected proc-array param kind, got {:#?}",
            def.param_kinds
        );
        assert!(
            !def.body.iter().any(def_stmt_contains_proc_index_sentinel),
            "proc-array indexed field assignment should be rewritten before typed def lowering: {:#?}",
            def.body
        );
    }

    #[test]
    fn def_accepts_proc_array_param_len_builtin() {
        let src = "proc Voice:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nouts:\n  out1\ndef set_and_sum(voices):\n  total = 0.0\n  for i in 0..(voices.len()):\n    voices[i].gain = f32(i + 1)\n    total = total + voices[i]()\n  return total + f32(voices.len())\ninit:\n  voices: Voice[3] = Voice()\nsample:\n  out1 = set_and_sum(voices)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-array len builtin should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "set_and_sum")
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::ProcArray { proc_name, len: 3 }] if proc_name == "Voice"
            ),
            "expected proc-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn unused_untyped_proc_array_def_is_ignored() {
        let src = "import std/osc\nouts:\n  out1\ndef init_voices(voices):\n  for i in 0..2:\n    voices[i].init(freq = 110.0)\ninit:\n  voices: std::osc::Sine[2]\n  for i in 0..2:\n    voices[i].init(freq = 110.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("unused untyped proc-array def should be ignored");
        assert!(
            !typed.defs.iter().any(|def| def.name == "init_voices"),
            "unused def unexpectedly survived into typed program: {:#?}",
            typed.defs
        );
    }

    #[test]
    fn unused_explicitly_typed_struct_def_still_reports_body_errors() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef broken(pair: Pair):\n  return pair.y\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("unused explicitly typed struct def should still analyze");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("struct parameter 'pair' (type 'Pair') has no field 'y'")),
            "expected unreachable explicit struct def diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn unused_explicitly_typed_proc_def_still_reports_body_errors() {
        let src = "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nouts:\n  out1\ndef broken(voice: Voice):\n  return voice.missing\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("unused explicitly typed proc def should still analyze");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("struct parameter 'voice' (type 'Voice') has no field 'missing'")),
            "expected unreachable explicit proc def diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn def_forwards_proc_array_params_across_calls() {
        let src = "import std/osc\nouts:\n  out1\ndef init_inner(voices, freq):\n  for i in 0..2:\n    voices[i].init(freq = freq * f32(i + 1))\ndef init_outer(voices, freq):\n  init_inner(voices, freq)\ninit:\n  voices: std::osc::Sine[2]\n  init_outer(voices, 110.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded proc-array params should analyze");
        for def_name in ["init_inner", "init_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::ProcArray { proc_name, len: 2 })
                        if proc_name.starts_with("std::osc::Sine")
                ),
                "expected proc-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_forwards_proc_array_params_across_multiple_layers() {
        let src = "import std/osc\nouts:\n  out1\ndef init_leaf(voices, freq):\n  for i in 0..2:\n    voice = voices[i]\n    voice.init(freq = freq * f32(i + 1))\ndef init_mid(voices, freq):\n  init_leaf(voices, freq)\ndef init_top(voices, freq):\n  init_mid(voices, freq)\ninit:\n  voices: std::osc::Sine[2]\n  init_top(voices, 110.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("multi-layer proc-array forwarding should analyze");
        for def_name in ["init_leaf", "init_mid", "init_top"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::ProcArray { proc_name, len: 2 })
                        if proc_name.starts_with("std::osc::Sine")
                ),
                "expected proc-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_infers_struct_array_params_from_call_sites() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_pairs(pairs):\n  total = 0.0\n  for i in 0..2:\n    total = total + pairs[i].x\n  return total\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = sum_pairs(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("struct-array def param should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "sum_pairs")
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::StructArray { struct_name }] if struct_name == "Pair"
            ),
            "expected struct-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn def_infers_struct_array_params_from_len_builtin() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef set_and_sum(pairs):\n  total = 0.0\n  for i in 0..(pairs.len()):\n    pairs[i].x = f32(i + 1)\n    total = total + pairs[i].x\n  return total + f32(pairs.len())\ninit:\n  pairs: Pair[3]\nsample:\n  out1 = set_and_sum(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("struct-array len builtin should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "set_and_sum")
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::StructArray { struct_name }] if struct_name == "Pair"
            ),
            "expected struct-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn def_infers_struct_array_params_from_indexed_field_assignments() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef seed_pairs(pairs):\n  for i in 0..2:\n    pairs[i].x = f32(i + 1)\ninit:\n  pairs: Pair[2]\n  seed_pairs(pairs)\nsample:\n  out1 = pairs[0].x + pairs[1].x\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("struct-array indexed field assignment should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "seed_pairs")
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::StructArray { struct_name }] if struct_name == "Pair"
            ),
            "expected struct-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn def_forwards_struct_array_params_across_calls() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_inner(pairs):\n  total = 0.0\n  for i in 0..2:\n    total = total + pairs[i].x\n  return total\ndef sum_outer(pairs):\n  return sum_inner(pairs)\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = sum_outer(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded struct-array params should analyze");
        for def_name in ["sum_inner", "sum_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Pair"
                ),
                "expected struct-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_infers_struct_array_params_across_multiple_layers_with_methods() {
        let src = "struct Tap:\n  gain: f32\n\n  def read(self):\n    return self.gain * 2.0\n\nstruct Voice:\n  tap: Tap\n  bias: f32\n\n  def value(self):\n    return self.tap.read() + self.bias\n\nouts:\n  out1\ndef read_leaf(voice: Voice):\n  return voice.value()\ndef read_mid(voices, idx: i32):\n  return read_leaf(voices[idx])\ndef read_top(voices, idx: i32):\n  return read_mid(voices, idx)\ninit:\n  voices: Voice[2]\n  v = voices[0]\n  v.tap.gain = 1.0\n  v.bias = 0.5\nsample:\n  out1 = read_top(voices, 0)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed =
            analyze(program).expect("multi-layer struct-array method forwarding should analyze");
        let read_leaf = typed
            .defs
            .iter()
            .find(|def| def.name == "read_leaf")
            .expect("missing typed def 'read_leaf'");
        assert!(
            matches!(
                read_leaf.param_kinds.first(),
                Some(TypedFnParam::Struct { struct_name }) if struct_name == "Voice"
            ),
            "expected Voice owner param for 'read_leaf', got {:#?}",
            read_leaf.param_kinds
        );
        for def_name in ["read_mid", "read_top"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Voice"
                ),
                "expected struct-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_forwards_struct_array_alias_params_across_calls() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_inner(pairs):\n  total = 0.0\n  for i in 0..2:\n    p = pairs[i]\n    total = total + p.x\n  return total\ndef sum_outer(pairs):\n  return sum_inner(pairs)\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = sum_outer(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded struct-array alias params should analyze");
        for def_name in ["sum_inner", "sum_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Pair"
                ),
                "expected struct-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn unused_untyped_struct_array_def_is_ignored() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_pairs(pairs):\n  total = 0.0\n  for i in 0..2:\n    total = total + pairs[i].x\n  return total\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = pairs[0].x + pairs[1].x\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("unused untyped struct-array def should be ignored");
        assert!(
            !typed.defs.iter().any(|def| def.name == "sum_pairs"),
            "unused def unexpectedly survived into typed program: {:#?}",
            typed.defs
        );
    }

    #[test]
    fn def_accepts_proc_array_alias_init_events() {
        let src = "import std/osc\nouts:\n  out1\ndef init_voices(voices):\n  for i in 0..2:\n    voice = voices[i]\n    voice.init(freq = 110.0 * f32(i + 1))\ninit:\n  voices: std::osc::Sine[2]\n  init_voices(voices)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc-array alias init def should analyze");
    }

    #[test]
    fn def_forwarding_proc_params_preserves_nested_proc_array_block_hooks() {
        let src = r#"
proc Voice:
  outs:
    out1
  block:
    sample:
      out1 = 0.25

proc Bank:
  outs:
    out1
  init:
    voices: Voice[2] = Voice()
  sample:
    out1 = 0.0

outs:
  out1

def inner(bank: Bank, idx: i32):
  return bank.voices[idx]()

def outer(bank: Bank):
  idx: i32 = 1
  return inner(bank, idx)

init:
  bank = Bank()

sample:
  out1 = outer(bank)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded proc param call should analyze");

        let bank_struct = typed
            .structs
            .iter()
            .find(|st| st.name == "Bank")
            .expect("missing lowered Bank struct");
        assert!(
            bank_struct.fields.iter().any(|field| {
                field.name == "__onda_proc_block_active_voices"
                    && matches!(field.ty, TypedFieldType::Array(2))
            }),
            "expected Bank struct to own nested proc-array active slots, got {:#?}",
            bank_struct.fields
        );

        let inner = typed
            .defs
            .iter()
            .find(|def| def.name == "inner")
            .expect("missing typed inner def");
        assert!(
            inner
                .body
                .iter()
                .any(|stmt| stmt_contains_assign_to_index_base(
                    stmt,
                    "bank.__onda_proc_block_active_voices"
                )),
            "expected inner def to mark nested proc-array slots active: {:#?}",
            inner.body
        );

        assert!(
            typed
                .block_pre
                .iter()
                .any(|stmt| stmt_contains_user_call_name(stmt, "Bank.__proc_block_pre")),
            "expected sample caller to inject Bank block_pre: {:#?}",
            typed.block_pre
        );
        assert!(
            typed
                .block_post
                .iter()
                .any(|stmt| stmt_contains_user_call_name(stmt, "Bank.__proc_block_post")),
            "expected sample caller to inject Bank block_post: {:#?}",
            typed.block_post
        );

        let bank_block_post = typed
            .defs
            .iter()
            .find(|def| def.name == "Bank.__proc_block_post")
            .expect("missing lowered Bank block_post def");
        assert!(
            bank_block_post
                .body
                .iter()
                .any(|stmt| stmt_contains_index_base(stmt, "self.__onda_proc_block_active_voices")),
            "expected Bank block_post to flush nested proc-array active slots: {:#?}",
            bank_block_post.body
        );
    }

    #[test]
    fn def_multi_layer_proc_array_forwarding_preserves_block_hooks() {
        let src = r#"
proc Voice:
  outs:
    out1
  block:
    sample:
      out1 = 0.25

outs:
  out1

def leaf(voices, idx: i32):
  return voices[idx]()

def mid(voices, idx: i32):
  return leaf(voices, idx)

def outer(voices, idx: i32):
  return mid(voices, idx)

init:
  voices: Voice[2] = Voice()
  idx: i32 = 1

sample:
  out1 = outer(voices, idx)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed =
            analyze(program).expect("multi-layer proc-array block-hook call should analyze");

        for def_name in ["leaf", "mid", "outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::ProcArray { proc_name, len: 2 }) if proc_name == "Voice"
                ),
                "expected proc-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }

        assert!(
            typed
                .block_pre
                .iter()
                .any(|stmt| stmt_contains_assign_to_index_base(
                    stmt,
                    "__onda_proc_block_active_voices"
                )),
            "expected sample caller to reset top-level proc-array active slots in block_pre: {:#?}",
            typed.block_pre
        );
        assert!(
            typed
                .block_post
                .iter()
                .any(|stmt| {
                    stmt_contains_user_call_name(stmt, "Voice.__proc_block_post")
                        || stmt_contains_index_base(stmt, "__onda_proc_block_active_voices")
                }),
            "expected sample caller to flush top-level proc-array active slots in block_post: {:#?}",
            typed.block_post
        );
    }

    #[test]
    fn def_forwards_owner_proc_params_across_multiple_layers() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

proc Bank:
  params:
    base = 0.0
  outs:
    out1
  init:
    voices: Voice[2] = Voice()
    voices[0].init(gain = base + 1.0)
    voices[1].init(gain = base + 2.0)
  sample:
    out1 = voices[1]()

proc Rack:
  outs:
    out1
  init:
    banks: Bank[2] = [Bank(base = 0.0), Bank(base = 10.0)]
  sample:
    out1 = 0.0

outs:
  out1

def read_leaf(rack: Rack, bank_idx: i32):
  return rack.banks[bank_idx]().out1

def read_mid(rack: Rack, bank_idx: i32):
  return read_leaf(rack, bank_idx)

def read_outer(rack: Rack):
  return read_mid(rack, 1)

init:
  rack = Rack()

sample:
  out1 = read_outer(rack)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("multi-layer owner-proc forwarding should analyze");

        for def_name in ["read_leaf", "read_mid", "read_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::Struct { struct_name }) if struct_name == "Rack"
                ),
                "expected Rack owner param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn top_level_event_proc_call_is_rejected_as_not_sample_only() {
        let src = r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

events:
  fire():
    x = voice()

init:
  voice = Voice()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program).expect_err("event proc operator call should fail");
        assert!(
            errs.iter().any(|diag| diag
                .message
                .contains("proc operator '()' is only allowed in sample")),
            "expected sample-only proc operator diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn top_level_event_def_proc_array_call_is_rejected_as_not_sample_only() {
        let src = r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

def run_selected(voices, idx: i32):
  return voices[idx]()

events:
  fire():
    x = run_selected(voices, idx)

init:
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 0

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program).expect_err("event def proc-array operator call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn top_level_event_def_owner_proc_call_is_rejected_as_not_sample_only() {
        let src = r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

proc Bank:
  outs:
    out1
  init:
    voices: Voice[2] = Voice()
  sample:
    out1 = 0.0

outs:
  out1

def run_selected(bank: Bank, idx: i32):
  return bank.voices[idx]()

events:
  fire():
    x = run_selected(bank, idx)

init:
  bank = Bank()
  idx: i32 = 0

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program).expect_err("event owner-proc operator call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn proc_local_def_called_from_proc_event_cannot_call_nested_proc_operator() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.75

proc Parent:
  outs:
    out1
  init:
    child = Child()

  def run_child():
    return child()

  events:
    ping():
      x = run_child()

  sample:
    out1 = 0.0

init:
  p = Parent()

sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs =
            analyze(program).expect_err("proc-event proc-local def operator call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn proc_local_def_called_from_proc_event_cannot_call_nested_proc_array_alias() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.75

proc Parent:
  outs:
    out1
  init:
    children: Child[2] = Child()
    idx: i32 = 0

  def run_child():
    v = children[idx]
    return v()

  events:
    ping():
      x = run_child()
      idx = 1 - idx

  sample:
    out1 = 0.0

init:
  p = Parent()

sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs =
            analyze(program).expect_err("proc-event proc-local proc-array alias call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn proc_event_proc_operator_is_rejected_even_when_called_from_sample() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.25

proc Parent:
  outs:
    out1
  init:
    child = Child()

  events:
    ping():
      x = child()

  sample:
    out1 = 0.0

outs:
  out1

init:
  parent = Parent()

sample:
  parent.ping()
  out1 = parent()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program)
            .expect_err("proc event body proc operator should fail even from sample caller");
        assert!(
            errs.iter().any(|diag| diag
                .message
                .contains("proc operator '()' is only allowed in sample")),
            "expected sample-only proc operator diagnostic, got {errs:?}"
        );
    }

    fn def_stmt_contains_proc_index_sentinel(stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                expr_contains_proc_index_sentinel(expr)
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                expr_contains_proc_index_sentinel(cond)
                    || then_branch
                        .iter()
                        .any(def_stmt_contains_proc_index_sentinel)
                    || else_branch
                        .iter()
                        .any(def_stmt_contains_proc_index_sentinel)
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                expr_contains_proc_index_sentinel(start)
                    || expr_contains_proc_index_sentinel(end)
                    || step
                        .as_ref()
                        .is_some_and(|expr| expr_contains_proc_index_sentinel(expr))
                    || body.iter().any(def_stmt_contains_proc_index_sentinel)
            }
            Stmt::While { cond, body, .. } => {
                expr_contains_proc_index_sentinel(cond)
                    || body.iter().any(def_stmt_contains_proc_index_sentinel)
            }
        }
    }

    fn stmt_contains_user_call_name(stmt: &Stmt, expected_name: &str) -> bool {
        match stmt {
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } => name == expected_name,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                then_branch
                    .iter()
                    .any(|stmt| stmt_contains_user_call_name(stmt, expected_name))
                    || else_branch
                        .iter()
                        .any(|stmt| stmt_contains_user_call_name(stmt, expected_name))
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => body
                .iter()
                .any(|stmt| stmt_contains_user_call_name(stmt, expected_name)),
            _ => false,
        }
    }

    fn stmt_contains_assign_to_index_base(stmt: &Stmt, expected_base: &str) -> bool {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Index { base, .. },
                ..
            } => base == expected_base,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                then_branch
                    .iter()
                    .any(|stmt| stmt_contains_assign_to_index_base(stmt, expected_base))
                    || else_branch
                        .iter()
                        .any(|stmt| stmt_contains_assign_to_index_base(stmt, expected_base))
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => body
                .iter()
                .any(|stmt| stmt_contains_assign_to_index_base(stmt, expected_base)),
            _ => false,
        }
    }

    fn stmt_contains_index_base(stmt: &Stmt, expected_base: &str) -> bool {
        match stmt {
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                expr_contains_index_base(expr, expected_base)
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                expr_contains_index_base(cond, expected_base)
                    || then_branch
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
                    || else_branch
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                expr_contains_index_base(start, expected_base)
                    || expr_contains_index_base(end, expected_base)
                    || step
                        .as_ref()
                        .is_some_and(|expr| expr_contains_index_base(expr, expected_base))
                    || body
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
            }
            Stmt::While { cond, body, .. } => {
                expr_contains_index_base(cond, expected_base)
                    || body
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
            }
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
        }
    }

    fn expr_contains_index_base(expr: &Expr, expected_base: &str) -> bool {
        match expr {
            Expr::Index { base, index, .. } => {
                base == expected_base || expr_contains_index_base(index, expected_base)
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => values
                .iter()
                .any(|expr| expr_contains_index_base(expr, expected_base)),
            Expr::Slice { start, end, .. } => {
                start
                    .as_ref()
                    .is_some_and(|expr| expr_contains_index_base(expr, expected_base))
                    || end
                        .as_ref()
                        .is_some_and(|expr| expr_contains_index_base(expr, expected_base))
            }
            Expr::ArrayCtor { spec, init, .. } => {
                expr_contains_index_base(&spec.size, expected_base)
                    || init.as_ref().is_some_and(|values| {
                        values
                            .iter()
                            .any(|expr| expr_contains_index_base(expr, expected_base))
                    })
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                expr_contains_index_base(lhs, expected_base)
                    || expr_contains_index_base(rhs, expected_base)
            }
            Expr::Call { args, .. } => args
                .iter()
                .any(|expr| expr_contains_index_base(expr, expected_base)),
            Expr::UserCall { args, .. } => args
                .iter()
                .any(|arg| expr_contains_index_base(&arg.expr, expected_base)),
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner, .. }
            | Expr::UnaryBitNot { expr: inner, .. } => {
                expr_contains_index_base(inner, expected_base)
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => false,
        }
    }

    fn expr_contains_proc_index_sentinel(expr: &Expr) -> bool {
        match expr {
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => false,
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                values.iter().any(expr_contains_proc_index_sentinel)
            }
            Expr::Index { index, .. } => expr_contains_proc_index_sentinel(index),
            Expr::Slice { start, end, .. } => {
                start
                    .as_ref()
                    .is_some_and(|expr| expr_contains_proc_index_sentinel(expr))
                    || end
                        .as_ref()
                        .is_some_and(|expr| expr_contains_proc_index_sentinel(expr))
            }
            Expr::ArrayCtor { spec, init, .. } => {
                expr_contains_proc_index_sentinel(&spec.size)
                    || init
                        .as_ref()
                        .is_some_and(|values| values.iter().any(expr_contains_proc_index_sentinel))
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                expr_contains_proc_index_sentinel(lhs) || expr_contains_proc_index_sentinel(rhs)
            }
            Expr::Call { args, .. } => args.iter().any(expr_contains_proc_index_sentinel),
            Expr::UserCall { name, args, .. } => {
                name.starts_with("__onda_proc_index_call")
                    || args
                        .iter()
                        .any(|arg| expr_contains_proc_index_sentinel(&arg.expr))
            }
            Expr::Cast { expr, .. }
            | Expr::UnaryNot { expr, .. }
            | Expr::UnaryBitNot { expr, .. } => expr_contains_proc_index_sentinel(expr),
        }
    }

    #[test]
    fn namespace_errors_on_namespaced_calls_use_call_spans() {
        let src = "import std/osc\nouts:\n  out1\nsample:\n  out1 = std::osc::Missing()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unknown namespaced call should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("unknown symbol 'Missing' in namespace 'std::osc'")
            })
            .expect("missing unknown namespaced call diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn init_branch_local_can_feed_top_level_state_but_not_escape_to_sample() {
        let src = "outs:\n  out1\ninit:\n  if true:\n    tmp = 1.0\n  else:\n    tmp = 2.0\n  carried = tmp\nsample:\n  out1 = carried\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("branch-local init value should feed later top-level init state");
    }

    #[test]
    fn init_loop_local_does_not_escape_loop() {
        let src = "outs:\n  out1\ninit:\n  for i in 0..2:\n    tmp = f32(i)\n  carried = tmp\nsample:\n  out1 = carried\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("loop-local init symbol should not escape");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'tmp'")),
            "missing unknown-symbol diagnostic for escaped init loop local: {errors:#?}"
        );
    }

    #[test]
    fn sample_typed_declaration_stays_local() {
        let src = "outs:\n  out1\nsample:\n  tmp: f32 = 1.0\n  out1 = tmp\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("sample-local typed declaration should analyze");
        assert!(
            !typed.state_vars.iter().any(|name| name == "tmp"),
            "sample local typed declaration unexpectedly became state"
        );
    }

    #[test]
    fn block_pre_top_level_state_is_visible_in_sample_and_post() {
        let src = "outs:\n  out1\nblock:\n  pre_root = 1.0\n  sample:\n    mix = pre_root\n    out1 = mix\n  post_seen = pre_root\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("top-level block pre vars should be visible in sample and post");
    }

    #[test]
    fn nested_block_local_is_not_visible_in_sample() {
        let src =
            "outs:\n  out1\nblock:\n  if true:\n    nested = 1.0\n  sample:\n    out1 = nested\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("nested block local should not escape into sample");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'nested'")),
            "missing unknown-symbol diagnostic for nested block local: {errors:#?}"
        );
    }

    #[test]
    fn proc_init_branch_local_can_feed_top_level_proc_state() {
        let src = "proc Voice:\n  outs:\n    out1\n  init:\n    if true:\n      tmp = 1.0\n    else:\n      tmp = 2.0\n    carried = tmp\n  sample:\n    out1 = carried\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc init branch-local value should feed later proc state");
    }

    #[test]
    fn proc_block_pre_top_level_state_is_visible_in_sample_and_post() {
        let src = "proc Voice:\n  outs:\n    out1\n  block:\n    pre_root = 1.0\n    sample:\n      out1 = pre_root\n    post_seen = pre_root\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc block pre vars should be visible in sample and post");
    }

    #[test]
    fn proc_nested_block_local_is_not_visible_in_sample() {
        let src = "proc Voice:\n  outs:\n    out1\n  block:\n    if true:\n      nested = 1.0\n    sample:\n      out1 = nested\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("nested proc block local should not escape into sample");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'nested'")),
            "missing unknown-symbol diagnostic for nested proc block local: {errors:#?}"
        );
    }

    #[test]
    fn event_branch_local_can_feed_later_event_state_write() {
        let src = "outs:\n  out1\ninit:\n  phase = 0.0\nevents:\n  ping():\n    if true:\n      tmp = 1.0\n    else:\n      tmp = 2.0\n    phase = tmp\nsample:\n  out1 = phase\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("event branch-local should feed later event state write");
    }

    #[test]
    fn individual_event_syntax_merges_with_events_block_during_analysis() {
        let src = "outs:\n  out1\nevent ping(x: i32):\n  phase = f32(x)\nevents:\n  reset():\n    phase = 0.0\ninit:\n  phase = 0.0\nsample:\n  out1 = phase\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("merged event syntax should analyze");
        assert_eq!(typed.events.len(), 2);
        assert_eq!(typed.events[0].name, "ping");
        assert_eq!(typed.events[1].name, "reset");
    }

    #[test]
    fn event_loop_local_does_not_escape_loop() {
        let src = "outs:\n  out1\ninit:\n  phase = 0.0\nevents:\n  ping():\n    for i in 0..2:\n      tmp = f32(i)\n    phase = tmp\nsample:\n  out1 = phase\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("event loop-local symbol should not escape loop");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'tmp'")),
            "missing unknown-symbol diagnostic for escaped event loop local: {errors:#?}"
        );
    }

    #[test]
    fn proc_event_branch_local_can_feed_later_proc_state_write() {
        let src = "proc Voice:\n  outs:\n    out1\n  init:\n    phase = 0.0\n  events:\n    ping():\n      if true:\n        tmp = 1.0\n      else:\n        tmp = 2.0\n      phase = tmp\n  sample:\n    out1 = phase\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc event branch-local should feed later proc state write");
    }

    #[test]
    fn individual_proc_event_syntax_merges_with_proc_events_block_during_analysis() {
        let src = "proc Voice:\n  outs:\n    out1\n  event ping(x: i32):\n    phase = f32(x)\n  events:\n    reset():\n      phase = 0.0\n  init:\n    phase = 0.0\n  sample:\n    out1 = phase\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("merged proc event syntax should analyze");
    }
}
