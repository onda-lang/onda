use std::collections::{HashMap, HashSet};

use omni_frontend::{
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedEventParam {
    pub name: String,
    pub ty: TypedEventParamType,
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

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TypedFieldType {
    Scalar(PrimitiveType),
    Struct,
    Array(usize),
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

    use omni_frontend::parse_program;

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
        let src = "outs:\n  out1\ndef foo():\n  a = [0.0]\n  a[0] = false\n  return 0.0\nsample:\n  out1 = 0.0\n";
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
}
