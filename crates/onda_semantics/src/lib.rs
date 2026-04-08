use std::collections::{HashMap, HashSet};

use onda_frontend::{
    inject_auto_std_math, ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec,
    BlockKind, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg,
    CallTypeArg, CmpOp, DeclRange, DeclType, DiagCtx, Diagnostic, EventDef, EventParamType, Expr,
    FieldType, FnParamType, FunctionDef, GraphBlock, GraphEdge, GraphEndpoint, GraphRate,
    InitBlock, ParamDecl, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc,
    Stmt, StructDef, StructField,
};

mod array_structs;
mod builtins;
mod decl_symbols;
mod declaration_coercion;
mod def_semantics;
mod expr_analysis;
mod expr_typing;
mod expr_validation;
mod generic_specialization;
mod io_state_helpers;
mod namespacing;
mod pipeline;
mod port_coercion;
mod proc_call_rewrite;
mod proc_call_support;
mod proc_state_rewrite;
mod processor_lowering;
mod stmt_analysis;
use array_structs::*;
use builtins::*;
use decl_symbols::*;
use declaration_coercion::*;
use def_semantics::*;
use expr_analysis::{build_expr_env, build_scope_expr_env, ExprEnv, FnSignature, ScopeExprInputs};
use expr_typing::*;
use expr_validation::*;
use generic_specialization::*;
use io_state_helpers::*;
use namespacing::*;
pub use pipeline::{analyze, analyze_with_options, lower_graphs_for_inspection_with_options};
use port_coercion::*;
use proc_call_rewrite::*;
use proc_call_support::{
    rewrite_proc_alias_calls_for_validation, rewrite_proc_alias_calls_in_expr, split_dot_path,
    ProcArrayAliasInfo,
};
use proc_state_rewrite::*;
use stmt_analysis::*;

pub mod internal_names {
    pub const PROC_INDEX_BUFFER_SELECT_SENTINEL: &str =
        crate::proc_state_rewrite::PROC_INDEX_BUFFER_SELECT_SENTINEL;
    pub const PROC_INDEX_BASE_ARG: &str = crate::proc_state_rewrite::PROC_INDEX_BASE_ARG;
    pub const PROC_INDEX_EXPR_ARG: &str = crate::proc_state_rewrite::PROC_INDEX_EXPR_ARG;
}

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

    use onda_frontend::parse_program;

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
