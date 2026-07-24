// Semantic passes intentionally keep their data dependencies explicit. Most of
// these functions are private traversal helpers, where bundling unrelated
// symbol tables into broad context objects would hide borrowing and mutation
// boundaries without reducing complexity.
#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet};

use onda_frontend::{
    inject_auto_std_math, ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec,
    BlockKind, BufferBlock, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn,
    CallArg, CallTypeArg, CmpOp, ConstDecl, ConstType, DeclRange, DeclType, DiagCtx, Diagnostic,
    EventDef, EventParamType, Expr, FieldType, FnParamType, FnReturnScalarType, FnReturnType,
    FunctionDef, GraphBlock, GraphEdge, GraphEndpoint, GraphRate, InitBlock, NamespaceAliasDecl,
    NamespaceCallArg, NamespaceDecl, NamespaceItem, NamespaceRefSegment, OutputTiming, ParamBlock,
    ParamDecl, ParamScale, PortBlock, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock,
    SourceLoc, Stmt, StructDef, StructField, UseDecl, INTERNAL_BUFFER_READ2_FN,
    INTERNAL_BUFFER_WRITE2_FN,
};

pub mod aggregate_layout;
mod analysis_session;
mod array_structs;
pub mod builtins;
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
mod mir_lowering;
mod namespacing;
mod pipeline;
mod port_coercion;
mod proc_call_rewrite;
mod proc_call_support;
mod proc_resolution;
mod proc_state_rewrite;
mod processor_lowering;
mod stmt_analysis;
pub use aggregate_layout::{
    AggregateLayout, AggregateLayoutArithmeticError, AggregateLayoutError, AggregateLayoutId,
    AggregateLayoutTable, AggregateLeafId, AggregateLeafLayout, AggregatePathComponent,
    AggregateTensorLayout,
};
pub use analysis_session::{
    normalize_session_path, AnalysisSession, AnalysisSnapshot, DocumentVersion, OpenDocument,
};
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
pub use mir_lowering::{lower_program_to_optimized_mir, MirLoweringError};
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
    /// The compile-time host configuration used during semantic analysis.
    /// MIR construction consumes this exact configuration so contextual
    /// constants and loop bounds cannot drift between compiler stages.
    pub analysis_options: AnalysisOptions,
    pub ins: Vec<String>,
    pub outs: Vec<String>,
    pub control_outs: Vec<String>,
    pub in_types: HashMap<String, PrimitiveType>,
    pub out_types: HashMap<String, PrimitiveType>,
    pub control_out_types: HashMap<String, PrimitiveType>,
    pub param_types: HashMap<String, PrimitiveType>,
    pub in_defaults: HashMap<String, TypedConstValue>,
    pub in_ranges: HashMap<String, TypedValueRange>,
    pub in_arrays: HashMap<String, TypedArrayInfo>,
    pub out_arrays: HashMap<String, TypedArrayInfo>,
    pub control_out_arrays: HashMap<String, TypedArrayInfo>,
    pub param_arrays: HashMap<String, TypedArrayInfo>,
    /// Fully resolved dynamic interface-group views (`ins[i]`, `outs[i]`,
    /// `kouts[i]`, and `params[i]`). Each slot names one concrete scalar ABI
    /// location, including an explicit element for declared array ports.
    pub interface_views: ResolvedInterfaceViews,
    pub const_arrays: Vec<TypedConstArray>,
    pub params: Vec<TypedParam>,
    pub buffers: Vec<TypedBufferDecl>,
    pub structs: Vec<TypedStruct>,
    /// Canonical recursive aggregate layouts produced by semantic analysis.
    /// Backends consume these stable IDs and resolved primitive leaves instead
    /// of rediscovering struct flattening and checked array strides.
    pub aggregate_layouts: AggregateLayoutTable,
    pub defs: Vec<TypedFunction>,
    pub events: Vec<TypedEvent>,
    pub def_sample_oversample_factors: HashMap<String, usize>,
    pub proc_step_oversample_meta: HashMap<String, ProcStepOversampleMeta>,
    pub proc_instance_oversample_factors: HashMap<String, usize>,
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
    /// Canonical processor-array members retained on their owning processor
    /// struct after processor desugaring has flattened the physical state
    /// fields. MIR uses this semantic map to resolve nested indexed processor
    /// references without interpreting generated storage names.
    pub nested_proc_arrays: Vec<TypedNestedProcArray>,
    pub ins_explicit: bool,
    pub audio_outs_explicit: bool,
    pub control_outs_explicit: bool,
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
    /// Primitive array parameters that semantic analysis proved are not
    /// mutated directly or through calls. MIR uses this to choose the slice
    /// access contract instead of rediscovering mutability from source AST.
    pub readonly_array_params: HashSet<String>,
    pub return_ty: ReturnType,
    /// Whether calls to this function produce the value described by
    /// `return_ty`. Functions with no `return` are represented as no-result
    /// functions; `return_ty` is meaningful only when this flag is true.
    pub returns_value: bool,
    /// Scalar local types resolved by semantic analysis. The table is keyed by
    /// source spelling, so MIR uses it for the first unique binding and retains
    /// the current assignment context when the same spelling denotes distinct
    /// nested or later bindings.
    pub local_scalar_types: HashMap<String, PrimitiveType>,
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
    pub control: TypedParamControl,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedParamControl {
    pub scale: ParamScale,
    pub unit: Option<String>,
    pub step: Option<TypedConstValue>,
    /// Number of equal intervals between the inclusive range endpoints.
    pub step_count: Option<u32>,
}

impl Default for TypedParamControl {
    fn default() -> Self {
        Self {
            scale: ParamScale::Linear,
            unit: None,
            step: None,
            step_count: None,
        }
    }
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

/// Stable identity within one resolved dynamic interface view.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct InterfaceSlotId(u32);

impl InterfaceSlotId {
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One scalar ABI location selected by a dynamic interface-group index.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedInterfaceSlot {
    pub id: InterfaceSlotId,
    /// Logical scalar port/parameter name, or the root name of a declared
    /// fixed array port/parameter.
    pub root: String,
    /// Element within `root` when it is a declared fixed array.
    pub element: Option<usize>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedInterfaceView {
    pub element_type: PrimitiveType,
    pub slots: Vec<ResolvedInterfaceSlot>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ResolvedInterfaceViews {
    pub inputs: Option<ResolvedInterfaceView>,
    pub audio_outputs: Option<ResolvedInterfaceView>,
    pub control_outputs: Option<ResolvedInterfaceView>,
    pub params: Option<ResolvedInterfaceView>,
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedNestedProcArray {
    pub owner_struct: String,
    pub field_name: String,
    pub proc_name: String,
    pub slots: Vec<String>,
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

    fn assert_analyze_error_contains(src: &str, expected: &str) {
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("analysis should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(expected)),
            "expected diagnostic containing '{expected}', got {errors:?}"
        );
    }

    #[test]
    fn validates_top_level_parameter_control_domains() {
        let program = parse_program(
            r#"
params {
  cutoff = 440.0 {20, 20000, log, "Hz"}
  mode: i32 = 4 {0, 10, step = 2}
}
outs { out1 }
sample { out1 = cutoff + mode }
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("valid parameter domains should analyze");

        assert_eq!(typed.params[0].control.scale, ParamScale::Log);
        assert_eq!(typed.params[0].control.unit.as_deref(), Some("Hz"));
        assert_eq!(typed.params[0].control.step, None);
        assert_eq!(typed.params[1].control.step, Some(TypedConstValue::I32(2)));
        assert_eq!(typed.params[1].control.step_count, Some(5));

        let mir =
            lower_program_to_optimized_mir(&typed).expect("parameter domains should lower to MIR");
        let params = &mir.as_program().interface.params;
        assert_eq!(params[0].control.scale, onda_mir::ParamScale::Log);
        assert_eq!(params[0].control.unit.as_deref(), Some("Hz"));
        assert_eq!(params[1].control.step, Some(onda_mir::ScalarValue::I32(2)));
        assert_eq!(params[1].control.step_count, Some(5));
    }

    #[test]
    fn integer_parameter_ranges_have_an_implicit_unit_step() {
        let program = parse_program(
            r#"
params { mode: i32 = 4 {0, 10} }
outs { out1 }
sample { out1 = mode }
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("integer domain should analyze");
        assert_eq!(typed.params[0].control.step, Some(TypedConstValue::I32(1)));
        assert_eq!(typed.params[0].control.step_count, Some(10));
    }

    #[test]
    fn rejects_invalid_top_level_parameter_control_domains() {
        for (domain, expected) in [
            ("{-20, 20000, log}", "0 < min < max"),
            ("{20, 20000, log, step = 10}", "cannot combine"),
            ("{0, 10, step = 3}", "divide the range exactly"),
            ("{0, 10, step = 2}", "default must lie on the step grid"),
        ] {
            assert_analyze_error_contains(
                &format!("params {{ p = 3.0 {domain} }}\nouts {{ out1 }}\nsample {{ out1 = p }}\n"),
                expected,
            );
        }
        assert_analyze_error_contains(
            "params { p: i32 = 1 {0, 10, log} }\nouts { out1 }\nsample { out1 = p }\n",
            "logarithmic scale requires f32 or f64",
        );
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
    fn bound_proc_param_hooks_lower_after_assignments_but_not_constructor_setup() {
        let src = r#"
proc Voice:
  params:
    gain = 1.0 {0.0, 1.0} => update
  init:
    cached = 0.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 2.0)
  v.gain = 0.25
sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("bound param hook program should analyze");
        let hook_name = "Voice.__proc_local__update";

        let top_level_hook_count = typed
            .init
            .iter()
            .filter(|stmt| {
                matches!(
                    stmt,
                    Stmt::Expr {
                        expr: Expr::UserCall { name, .. },
                        ..
                    } if name == hook_name
                )
            })
            .count();
        assert_eq!(
            top_level_hook_count, 1,
            "constructor setup stores should not inject top-level hooks: {:?}",
            typed.init
        );

        let user_assign_idx = typed
            .init
            .iter()
            .rposition(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } if name == "v.gain"
                )
            })
            .expect("missing user param assignment");
        assert!(matches!(
            typed.init.get(user_assign_idx + 1),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == hook_name
        ));

        let init_def = typed
            .defs
            .iter()
            .find(|def| def.name == "Voice.__proc_init")
            .expect("missing generated proc init def");
        assert!(matches!(
            init_def.body.last(),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == hook_name
        ));
    }

    #[test]
    fn bound_proc_param_hooks_inject_child_param_cascade_calls() {
        let src = r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = 0.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    child = Child()
  def update():
    child.gain = gain
  outs:
    out1
  sample:
    out1 = child()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("child bind propagation should analyze");
        let parent_hook = typed
            .defs
            .iter()
            .find(|def| def.name == "Parent.__proc_local__update")
            .expect("missing parent bind hook def");
        let assign_idx = parent_hook
            .body
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } if name == "self.child__gain"
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing child param assignment in parent hook: {:?}",
                    parent_hook.body
                )
            });
        assert!(matches!(
            parent_hook.body.get(assign_idx + 1),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == "Parent.__proc_local__nested__child__update"
        ));
    }

    #[test]
    fn bound_proc_param_hooks_inject_dynamic_child_array_cascade_calls() {
        let src = r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = 0.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    children: Child[2] = Child()
  def update():
    for i in 0..2:
      children[i].gain = gain + f32(i)
  outs:
    out1
  sample:
    out1 = children[0]() + children[1]()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic child array bind propagation should analyze");
        let helper = typed
            .defs
            .iter()
            .find(|def| {
                def.name.starts_with("Parent.__arr_write_clamp_")
                    && def.body.iter().any(|stmt| {
                        stmt_contains_user_call_name(
                            stmt,
                            "Parent.__proc_local__nested__children_0___update",
                        )
                    })
                    && def.body.iter().any(|stmt| {
                        stmt_contains_user_call_name(
                            stmt,
                            "Parent.__proc_local__nested__children_1___update",
                        )
                    })
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing dynamic child array write helper with cascade hooks: {:?}",
                    typed
                        .defs
                        .iter()
                        .filter(|def| def.name.starts_with("Parent.__arr_write_clamp_"))
                        .map(|def| (&def.name, &def.body))
                        .collect::<Vec<_>>()
                )
            });

        assert!(
            helper.body.iter().any(|stmt| {
                stmt_contains_user_call_name(
                    stmt,
                    "Parent.__proc_local__nested__children_0___update",
                )
            }),
            "missing slot 0 cascade hook in {:?}",
            helper.body
        );
        assert!(
            helper.body.iter().any(|stmt| {
                stmt_contains_user_call_name(
                    stmt,
                    "Parent.__proc_local__nested__children_1___update",
                )
            }),
            "missing slot 1 cascade hook in {:?}",
            helper.body
        );
    }

    #[test]
    fn bound_proc_param_hooks_share_dynamic_proc_array_index_temps() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
  idx = 0
  voices[idx].gain = 0.5
sample:
  out1 = voices[0]()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic proc-array hook should analyze");
        let assign_idx = typed
            .init
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index {
                            base,
                            index: Expr::Var { name, .. },
                        },
                        ..
                    } if base == "voices.gain" && name.starts_with("__onda_bound_hook_index_tmp_")
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing indexed proc-array param assignment using a hook index temp: {:?}",
                    typed.init
                )
            });
        assert!(
            assign_idx >= 2,
            "hook temp prelude should precede assignment: {:?}",
            typed.init
        );

        let index_tmp = match &typed.init[assign_idx] {
            Stmt::Assign {
                target:
                    AssignTarget::Index {
                        index: Expr::Var { name, .. },
                        ..
                    },
                ..
            } => name.clone(),
            other => panic!("unexpected assignment shape: {other:?}"),
        };
        assert!(
            matches!(
                &typed.init[assign_idx - 2],
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } if name.starts_with("__onda_bound_hook_value_tmp_")
            ),
            "missing value temp before indexed assignment: {:?}",
            typed.init
        );
        assert!(
            matches!(
                &typed.init[assign_idx - 1],
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Var { name: idx_name, .. },
                    ..
                } if name == &index_tmp && idx_name == "idx"
            ),
            "missing index temp before indexed assignment: {:?}",
            typed.init
        );
        assert!(
            matches!(
                typed.init.get(assign_idx + 1),
                Some(Stmt::Expr {
                    expr:
                        Expr::UserCall {
                            name,
                            args,
                            ..
                        },
                    ..
                }) if name == "Voice.__proc_local__update"
                    && matches!(
                        args.first(),
                        Some(CallArg {
                            expr:
                                Expr::Index {
                                    base,
                                    index,
                                    ..
                                },
                            ..
                        }) if base == "voices"
                            && matches!(
                                index.as_ref(),
                                Expr::Var { name, .. } if name == &index_tmp
                            )
                    )
            ),
            "hook call should reuse the assignment index temp: {:?}",
            typed.init
        );
    }

    #[test]
    fn bound_proc_param_hook_rules_are_validated() {
        let cases = [
            (
                "top-level bind",
                "params:\n  gain = 1.0 => update\nouts:\n  out1\nsample:\n  out1 = gain\n",
                "binds are only supported on processor params",
            ),
            (
                "array bind",
                "proc Voice:\n  params:\n    gains: f32[2] = [0.0, 1.0] => update\n  outs:\n    out1\n  def update():\n    cached = gains[0]\n  sample:\n    out1 = gains[0]\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "binds are only supported on primitive scalar params",
            ),
            (
                "missing target",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "param bind target 'update' is missing",
            ),
            (
                "target params",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update(x):\n    cached = x\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "bind target 'update' must take zero parameters",
            ),
            (
                "target return type",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update() -> f32:\n    cached = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "bind target 'update' must not declare a return type",
            ),
            (
                "target return",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    return gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "bind target 'update' must not contain return",
            ),
            (
                "owner param write",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    gain = gain + 1.0\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot assign owner param 'gain'",
            ),
            (
                "input write",
                "proc Voice:\n  ins:\n    in1\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    in1 = gain\n  sample:\n    out1 = in1\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v(0.0)\n",
                "cannot write input 'in1'",
            ),
            (
                "output write",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    out1 = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write output 'out1'",
            ),
            (
                "child input write",
                "proc Child:\n  ins:\n    in1\n  outs:\n    out1\n  sample:\n    out1 = in1\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.in1 = gain\n  sample:\n    out1 = child(0.0)\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc input 'child.in1'",
            ),
            (
                "child output write",
                "proc Child:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.out1 = gain\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc output 'child.out1'",
            ),
            (
                "child internal state write",
                "proc Child:\n  init:\n    cached = 0.0\n  outs:\n    out1\n  sample:\n    out1 = cached\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.cached = gain\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'child.cached' is child proc state",
            ),
            (
                "child dynamic params write",
                "proc Child:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.params[0] = gain\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot assign child proc dynamic params 'child.params'",
            ),
            (
                "child proc array input write",
                "proc Child:\n  ins:\n    in1\n  outs:\n    out1\n  sample:\n    out1 = in1\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    for i in 0..2:\n      children[i].in1 = gain\n  sample:\n    out1 = children[0](0.0)\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc input 'children.in1'",
            ),
            (
                "child proc array output write",
                "proc Child:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    for i in 0..2:\n      children[i].out1 = gain\n  sample:\n    out1 = children[0]()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc output 'children.out1'",
            ),
            (
                "child proc array internal state write",
                "proc Child:\n  init:\n    cached = 0.0\n  outs:\n    out1\n  sample:\n    out1 = cached\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    for i in 0..2:\n      children[i].cached = gain\n  sample:\n    out1 = children[0]()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'children.cached' is child proc state",
            ),
            (
                "child proc array dynamic params write",
                "proc Child:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    children.params[0] = gain\n  sample:\n    out1 = children[0]()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot assign child proc dynamic params 'children.params'",
            ),
            (
                "child receiver call",
                "proc Child:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child()\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot call child proc receiver 'child(...)'",
            ),
            (
                "dynamic params assignment",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    params[0] = 1.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "dynamic params assignment in event",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  events:\n    set():\n      params[0] = 1.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "dynamic params assignment in def",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    cached = gain\n  def unsafe_set():\n    params[0] = 1.0\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "dynamic params assignment in sample",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    params[0] = 1.0\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "top-level child dynamic params assignment to bound proc",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\n  v.params[0] = 1.0\nsample:\n  out1 = v()\n",
                "dynamic 'v.params[...]' is not supported",
            ),
            (
                "owner child dynamic params assignment to bound proc",
                "proc Child:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = cached\nproc Parent:\n  init:\n    child = Child()\n  outs:\n    out1\n  sample:\n    child.params[0] = 1.0\n    out1 = child()\nouts:\n  out1\ninit:\n  p = Parent()\nsample:\n  out1 = p()\n",
                "dynamic 'child.params[...]' is not supported",
            ),
            (
                "top-level helper cannot receive owner params view",
                "def poke(ps: f32[]):\n  ps[0] = 1.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n    trim = 0.0\n  outs:\n    out1\n  def update():\n    poke(params)\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot use dynamic param array 'params'",
            ),
            (
                "proc-local helper cannot receive owner params view",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n    trim = 0.0\n  outs:\n    out1\n  def poke(ps: f32[]):\n    ps[0] = 1.0\n  def update():\n    poke(params)\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot use dynamic param array 'params'",
            ),
            (
                "top-level helper cannot receive child params view",
                "def poke(ps: f32[]):\n  ps[0] = 1.0\nproc Child:\n  params:\n    a = 0.0\n    b = 0.0\n  outs:\n    out1\n  sample:\n    out1 = a\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    poke(child.params)\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot use dynamic param array 'child.params'",
            ),
            (
                "top-level helper cannot mutate child state array",
                "def poke(xs: f32[]):\n  xs[0] = 1.0\nproc Child:\n  init:\n    table: f32[2] = [0.0, 0.0]\n  outs:\n    out1\n  sample:\n    out1 = table[0]\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    poke(child.table)\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'child.table' is child proc state",
            ),
            (
                "transitive helper cannot mutate child state array",
                "def poke(xs: f32[]):\n  xs[0] = 1.0\ndef forward(xs: f32[]):\n  poke(xs)\nproc Child:\n  init:\n    table: f32[2] = [0.0, 0.0]\n  outs:\n    out1\n  sample:\n    out1 = table[0]\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    forward(child.table)\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'child.table' is child proc state",
            ),
        ];

        for (_label, src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }

        let scalar_helper = "def coeff(x) -> f32:\n  return x * 2.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    cached = coeff(gain)\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n";
        let program = parse_program(scalar_helper).expect("parse should succeed");
        analyze(program).expect("scalar helper in bind hook should analyze");

        let untyped_scalar_local = "def shadow(x):\n  x = x + 1.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    shadow(gain)\n    cached = gain\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n";
        let program = parse_program(untyped_scalar_local).expect("parse should succeed");
        analyze(program).expect("untyped scalar helper local should analyze");
    }

    #[test]
    fn oversampled_bind_hooks_accept_sr_dependent_consts() {
        let cases = [
            "const INV_SR = 1.0 / SR\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    cached = gain * INV_SR\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
            "proc Voice:\n  const INV_SR = 1.0 / SR\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    cached = gain * INV_SR\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
            "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    const INV_SR = 1.0 / SR\n    cached = gain * INV_SR\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
            "const def inv_sr() -> f32:\n  return 1.0 / SR\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    cached = gain * inv_sr()\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
        ];

        for src in cases {
            let program = parse_program(src).expect("parse should succeed");
            analyze(program).expect("SR-dependent constants in bind hooks should analyze");
        }
    }

    #[test]
    fn oversampled_proc_state_shapes_use_runtime_sr() {
        let src = r#"
proc Voice:
  const Len = SR
  init:
    table: f32[Len] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
  outs:
    out1
  sample 2:
    out1 = table[7]

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("proc-local SR should size proc state arrays with effective runtime SR");
    }

    #[test]
    fn oversampled_proc_declared_arrays_and_proc_arrays_use_runtime_sr() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.0

proc Voice:
  const Len = SR
  params:
    gains: f32[Len] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
  init:
    children: Child[Len] = Child()
  outs:
    out1
  sample 2:
    out1 = gains[7] + children[7]()

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("proc param arrays and child proc arrays should use effective runtime SR");
    }

    #[test]
    fn external_sr_consts_keep_host_values_inside_oversampled_procs() {
        let src = r#"
namespace Host:
  const Len = SR

proc Voice:
  init:
    table: f32[Host::Len] = [0.0, 0.0, 0.0, 1.0]
  outs:
    out1
  sample 2:
    out1 = table[3]

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("namespace SR constants should keep the host-rate value where they are defined");
    }

    #[test]
    fn host_sr_builtin_stays_host_in_oversampled_proc_contexts() {
        let src = r#"
proc Voice:
  const HostLen = HOST_SR
  const HostLenFromSampleRate = HOST_SAMPLE_RATE
  const HostLenFromSamplerate = HOST_SAMPLERATE
  const HostLenLowerSampleRate = host_sample_rate
  const HostLenLowerSamplerate = host_samplerate
  const RuntimeLen = SR
  params:
    gains: f32[HostLen] = [0.0, 0.0, 0.0, 1.0]
    more: f32[HostLenFromSampleRate] = [0.0, 0.0, 0.0, 1.0]
  init:
    table: f32[RuntimeLen] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
    table_host_samplerate: f32[HostLenFromSamplerate] = [0.0, 0.0, 0.0, 1.0]
  outs:
    out1
  sample 2:
    const LocalHostLen = host_sample_rate
    const LocalHostLen2 = host_samplerate
    out1 = gains[LocalHostLen - 1] + more[HostLenLowerSampleRate - 1] + table_host_samplerate[LocalHostLen2 - 1] + table[RuntimeLen - 1] + f32(HostLenLowerSamplerate)

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("HOST_SR should keep the host sample rate inside oversampled proc contexts");
    }

    #[test]
    fn proc_call_named_param_arg_errors_are_validated() {
        let cases = [
            (
                r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(0.25)
"#,
                "too many positional arguments",
            ),
            (
                r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(freq = 440.0)
"#,
                "unknown named argument 'freq'",
            ),
            (
                r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(gain = 0.25, gain = 0.5)
"#,
                "duplicate named argument 'gain'",
            ),
        ];

        for (src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }
    }

    #[test]
    fn proc_call_named_param_args_preserve_expression_call_order() {
        let src = r#"
proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v() + v(gain = 2.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("analysis should succeed");
        assert_eq!(typed.sample.len(), 7, "{:#?}", typed.sample);
        assert!(matches!(
            &typed.sample[2],
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::UserCall { name: call_name, .. },
                ..
            } if name == "__onda_proc_call_result_tmp_0"
                && call_name == "Voice.__proc_call_out0"
        ));
        assert!(matches!(
            &typed.sample[3],
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::Number { value, .. },
                ..
            } if name == "v.gain" && (*value - 2.0).abs() <= f64::EPSILON
        ));
        assert!(matches!(
            &typed.sample[4],
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } if name == "Voice.__proc_local__update"
        ));
        assert!(matches!(
            &typed.sample[5],
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::UserCall { name: call_name, .. },
                ..
            } if name == "__onda_proc_call_result_tmp_1"
                && call_name == "Voice.__proc_call_out0"
        ));
    }

    #[test]
    fn proc_call_named_param_args_reject_control_flow_unsafe_contexts() {
        assert_analyze_error_contains(
            r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
params:
  ready: bool = 0
init:
  v = Voice()
sample:
  if ready && v(gain = 0.25) > 0.0:
    out1 = 1.0
  else:
    out1 = 0.0
"#,
            "named param arguments are not supported in logical expressions",
        );

        assert_analyze_error_contains(
            r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
init:
  v = Voice()
sample:
  while v(gain = 0.25) > 0.0:
    break
  out1 = 0.0
"#,
            "named param arguments are not supported in while conditions",
        );
    }

    #[test]
    fn pinned_proc_params_accept_constructor_and_builtin_init() {
        let src = r#"
proc Voice:
  params:
    pin cutoff = 1000.0
    pin coeffs: f32[2] = [0.5, 0.25]
    gain = 1.0
  init:
    cached = cutoff + coeffs[0] + coeffs[1] + gain
  event refresh(cutoff_v: f32, coeffs_v: f32[2]):
    cutoff = cutoff_v
    coeffs[0] = coeffs_v[0]
    coeffs[1] = coeffs_v[1]
    cached = cutoff + coeffs[0] + coeffs[1] + gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
events:
  reset():
    voice.init(cutoff = 1500.0, coeffs = [0.2, 0.3], gain = 0.75)
init:
  voice = Voice(cutoff = 1200.0, coeffs = [0.1, 0.2], gain = 0.5)
sample:
  out1 = voice(gain = 0.25)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("pinned constructor/init params should analyze");
    }

    #[test]
    fn pinned_proc_params_reject_external_access() {
        let cases = [
            (
                "field assignment",
                "proc Voice:\n  params:\n    pin cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  voice.cutoff = 1200.0\n  out1 = voice()\n",
                "param 'cutoff' is pinned and cannot be assigned",
            ),
            (
                "field read",
                "proc Voice:\n  params:\n    pin cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice.cutoff\n",
                "param 'cutoff' is pinned and cannot be read",
            ),
            (
                "array assignment",
                "proc Voice:\n  params:\n    pin coeffs: f32[2] = [0.5, 0.25]\n  outs:\n    out1\n  sample:\n    out1 = coeffs[0]\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  voice.coeffs[0] = 0.1\n  out1 = voice()\n",
                "param 'coeffs' is pinned and cannot be assigned",
            ),
            (
                "array read",
                "proc Voice:\n  params:\n    pin coeffs: f32[2] = [0.5, 0.25]\n  outs:\n    out1\n  sample:\n    out1 = coeffs[0]\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice.coeffs[0]\n",
                "param 'coeffs' is pinned and cannot be read",
            ),
            (
                "named call arg",
                "proc Voice:\n  params:\n    pin cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice(cutoff = 1200.0)\n",
                "named argument 'cutoff' is pinned",
            ),
            (
                "dynamic params read",
                "proc Voice:\n  params:\n    pin cutoff = 1000.0\n    gain = 1.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff + gain\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice.params[0]\n",
                "has pinned params, so dynamic param access",
            ),
            (
                "dynamic params assignment",
                "proc Voice:\n  params:\n    pin cutoff = 1000.0\n    gain = 1.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff + gain\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  voice.params[0] = 0.5\n  out1 = voice()\n",
                "has pinned params, so assignment through dynamic",
            ),
        ];

        for (label, src, expected) in cases {
            let program = parse_program(src).expect(label);
            let errors = analyze(program).expect_err(label);
            assert!(
                errors.iter().any(|diag| diag.message.contains(expected)),
                "case '{label}' expected diagnostic containing '{expected}', got {errors:?}"
            );
        }
    }

    #[test]
    fn proc_call_named_param_args_in_nested_wrappers_inject_hooks() {
        let src = r#"
proc Leaf:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Mid:
  params:
    gain = 0.0
  init:
    leaf = Leaf()
  outs:
    out1
  sample:
    out1 = leaf(gain = gain)

proc Parent:
  init:
    mid = Mid()
  outs:
    out1
  sample:
    mid.gain = 0.25
    out1 = mid()

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("analysis should succeed");
        let step = typed
            .defs
            .iter()
            .find(|def| def.name == "Parent.__proc_nested_mid_step")
            .expect("missing nested mid step");
        let assign_idx = step
            .body
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } if name == "self.mid__leaf__gain"
                )
            })
            .expect("missing lowered nested child param assignment");
        assert!(matches!(
            step.body.get(assign_idx + 1),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == "Parent.__proc_local__nested__mid__leaf__update"
        ));
    }

    #[test]
    fn oversampled_bound_proc_param_hooks_can_use_runtime_sample_rate_directly() {
        let src = r#"
const HostBS = BS

proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain / SR + f32(BS) + f32(HostBS)
  outs:
    out1
  sample 2:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("direct runtime SR in hook should analyze");
        assert_eq!(
            typed
                .def_sample_oversample_factors
                .get("Voice.__proc_local__update")
                .copied(),
            Some(2)
        );
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
    fn proc_bodies_can_read_const_arrays_with_runtime_index() {
        let src = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

proc Voice:
  params:
    idx: i32 = 1
  outs:
    out1
  sample:
    out1 = Table[idx]

outs:
  out1

init:
  voice = Voice()

sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc dynamic const array read should analyze");
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
    fn use_namespace_brings_members_into_unqualified_scope() {
        let src = r#"
import std/math
use std::math

outs:
  out1

sample:
  out1 = clamp(2.0, 0.0, 1.0) + lerp(0.0, 2.0, 0.25)
"#;
        let program = parse_program(src).expect("parse should preserve use namespace");
        analyze(program).expect("namespace use should analyze");
    }

    #[test]
    fn use_single_namespace_brings_child_namespaces_into_unqualified_scope() {
        let src = r#"
namespace sc:
  namespace SinOsc:
    def ar():
      return 0.25

use sc

outs:
  out1

sample:
  out1 = SinOsc::ar()
"#;
        let program = parse_program(src).expect("parse should preserve single namespace use");
        analyze(program).expect("single namespace use should analyze child namespace prefix");
    }

    #[test]
    fn use_single_namespace_resolves_child_namespace_templates() {
        let src = r#"
namespace dsp:
  namespace Table<N = 2>:
    const Size = N

use dsp

outs:
  out1

sample:
  out1 = f32(Table<4>::Size)
"#;
        let program =
            parse_program(src).expect("parse should preserve single namespace template use");
        analyze(program).expect("single namespace use should analyze child namespace template");
    }

    #[test]
    fn use_single_namespace_resolves_child_namespace_aliases() {
        let src = r#"
namespace ugens:
  namespace LocalOsc:
    def ar():
      return 0.5

  namespace Osc = LocalOsc

use ugens

outs:
  out1

sample:
  out1 = Osc::ar()
"#;
        let program =
            parse_program(src).expect("parse should preserve child namespace alias through use");
        analyze(program).expect("single namespace use should analyze child namespace alias");
    }

    #[test]
    fn use_single_namespace_child_collision_requires_qualified_namespace_root() {
        let src = r#"
namespace imported:
  namespace Osc:
    def ar():
      return 0.5

namespace Osc:
  def ar():
    return 0.25

use imported

outs:
  out1

sample:
  out1 = Osc::ar()
"#;
        assert_analyze_error_contains(src, "ambiguous unqualified namespace 'Osc'");
    }

    #[test]
    fn use_single_namespace_child_collision_allows_qualified_namespace_root() {
        let src = r#"
namespace imported:
  namespace Osc:
    def ar():
      return 0.5

namespace Osc:
  def ar():
    return 0.25

use imported

outs:
  out1

sample:
  out1 = imported::Osc::ar()
"#;
        let program = parse_program(src).expect("parse should preserve qualified namespace root");
        analyze(program).expect("qualified namespace root should avoid use ambiguity");
    }

    #[test]
    fn use_symbol_brings_one_member_into_unqualified_scope() {
        let src = r#"
import std/random
use std::random::Rng

outs:
  out1

init:
  rng = Rng<f32>(state = 123)

sample:
  out1 = rng.next()
"#;
        let program = parse_program(src).expect("parse should preserve use symbol");
        analyze(program).expect("symbol use should analyze");
    }

    #[test]
    fn use_const_assignment_targets_resolve_to_imported_const() {
        let src = r#"
namespace NS:
  const X = 1

use NS::X

outs:
  out1

init:
  X = 2

sample:
  out1 = f32(X)
"#;
        assert_analyze_error_contains(src, "cannot assign to constant 'NS::X'");
    }

    #[test]
    fn use_const_array_assignment_targets_resolve_to_imported_const_array() {
        let src = r#"
namespace NS:
  const Table: f32[2] = [0.25, 0.5]

use NS::Table

outs:
  out1

sample:
  Table[0] = 0.0
  out1 = Table[1]
"#;
        assert_analyze_error_contains(src, "cannot assign to immutable array alias 'NS::Table'");
    }

    #[test]
    fn use_namespace_aliases_can_coexist_for_template_instantiations() {
        let src = r#"
import std/fft
use std::fft<8> as fft8
use std::fft<16> as fft16

outs:
  out1

init:
  a = fft8::FFT<f32>()
  b = fft16::FFT<f32>()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve namespace use aliases");
        analyze(program).expect("namespace use aliases should analyze");
    }

    #[test]
    fn use_symbol_alias_can_name_template_member() {
        let src = r#"
import std/fft
use std::fft<8>::FFT as FFT8

outs:
  out1

init:
  fft = FFT8<f32>()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve symbol use alias");
        analyze(program).expect("symbol use alias should analyze");
    }

    #[test]
    fn explicit_use_collision_requires_qualified_name() {
        let src = r#"
import std/math
use std::math

outs:
  out1

def clamp(x, lo, hi):
  return x

sample:
  out1 = clamp(2.0, 0.0, 1.0)
"#;
        assert_analyze_error_contains(src, "ambiguous unqualified symbol 'clamp'");
    }

    #[test]
    fn explicit_use_collision_allows_qualified_name() {
        let src = r#"
import std/math
use std::math

outs:
  out1

def clamp(x, lo, hi):
  return x

sample:
  out1 = std::math::clamp(2.0, 0.0, 1.0)
"#;
        let program = parse_program(src).expect("parse should preserve qualified use collision");
        analyze(program).expect("qualified name should avoid explicit use ambiguity");
    }

    #[test]
    fn use_namespace_does_not_capture_function_parameter_reads() {
        let src = r#"
import std/math
use std::math

outs:
  out1

def id(clamp):
  return clamp

sample:
  out1 = id(0.5)
"#;
        let program = parse_program(src).expect("parse should preserve local shadowing");
        analyze(program).expect("function parameter should shadow explicit use member");
    }

    #[test]
    fn use_namespace_does_not_capture_local_variable_reads() {
        let src = r#"
import std/math
use std::math

outs:
  out1

sample:
  clamp = 0.5
  out1 = clamp
"#;
        let program = parse_program(src).expect("parse should preserve local assignment");
        analyze(program)
            .expect("local variable should shadow explicit use member after assignment");
    }

    #[test]
    fn imported_private_use_is_not_reexported() {
        let dir = mk_temp_dir("private_use_not_reexported");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        let errors = analyze(program).expect_err("private imported use should not reexport");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("unknown symbol 'shape'")
                    || diag.message.contains("unknown function 'shape'")
            }),
            "expected unknown private use symbol, got {errors:?}"
        );
    }

    #[test]
    fn imported_private_use_still_works_inside_imported_file() {
        let dir = mk_temp_dir("private_use_internal");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = shaped(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        analyze(program).expect("private imported use should work inside imported file");
    }

    #[test]
    fn imported_pub_use_is_reexported() {
        let dir = mk_temp_dir("pub_use_reexported");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        analyze(program).expect("pub use from imported file should reexport");
    }

    #[test]
    fn imported_pub_use_alias_is_reexported() {
        let dir = mk_temp_dir("pub_use_alias_reexported");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers as h
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = h::shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        analyze(program).expect("pub use alias from imported file should reexport");
    }

    #[test]
    fn imported_private_use_alias_is_file_scoped() {
        let dir = mk_temp_dir("private_use_alias_scoped");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers as h

def shaped(x):
  return h::shape(x)
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = h::shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        let errors = analyze(program).expect_err("private imported alias should not reexport");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("unknown namespace 'h'")
                    || diag.message.contains("unknown symbol 'h'")
            }),
            "expected unknown private alias, got {errors:?}"
        );
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
    fn namespace_template_defaults_use_definition_scope_when_instantiated_elsewhere() {
        let src = r#"
const Size = 3

namespace LUT<N = Size>:
  const Value = N

namespace Consumer:
  const Size = 5
  const Picked: i32[1] = [LUT::Value]

outs:
  out1

sample:
  out1 = f32(Consumer::Picked[0])
"#;
        let program = parse_program(src).expect("parse should preserve namespace default");
        let typed = analyze(program).expect("definition-scoped namespace default should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Consumer::Picked")
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::I32(3)]);
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
    fn namespace_template_bodies_use_definition_scope_for_consts() {
        let src = r#"
namespace LUT<N = 1>:
  const Table: f32[1] = [Gain * f32(N)]

const Gain = 0.5

outs:
  out1

sample:
  out1 = LUT<2>::Table[0]
"#;
        let program = parse_program(src).expect("parse should preserve template body");
        let errors = analyze(program).expect_err("template body should not see later scalar const");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "const array 'LUT__nsinst0::Table' element 0 uses non-constant symbol 'Gain'"
            )),
            "expected definition-scope diagnostic, got {errors:?}"
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
    fn const_def_names_conflict_with_runtime_defs() {
        let src = r#"
const def foo() -> f32:
  return 0.25

def foo(x: f32):
  return x * 2.0

outs:
  out1

sample:
  out1 = foo(0.5)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const def/runtime def name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def name 'foo' conflicts with existing symbol")),
            "expected const def name conflict, got {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|diag| !diag.message.contains("expects 0 argument")),
            "runtime def call should not be intercepted by const folding: {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_params() {
        let src = r#"
const gain = 1.0

params:
  gain = 0.5

outs:
  out1

sample:
  out1 = gain
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const/param name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant name 'gain' conflicts with existing symbol")),
            "expected const/param conflict, got {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_earlier_params_without_forward_ref_noise() {
        let src = r#"
params:
  gain = 0.5

outs:
  out1

sample:
  out1 = gain

const gain = 1.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("param/const name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant name 'gain' conflicts with existing symbol")),
            "expected param/const conflict, got {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|diag| !diag.message.contains("not visible before its declaration")),
            "runtime param read should not be reported as forward const use: {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_function_params() {
        let src = r#"
const X = 1.0

outs:
  out1

def f(X: f32):
  return X

sample:
  out1 = f(2.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const/function parameter conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("function parameter 'X' conflicts with constant 'X'")),
            "expected function parameter/const conflict, got {errors:?}"
        );
    }

    #[test]
    fn const_array_names_conflict_with_array_function_params() {
        let src = r#"
const Table: f32[] = [1.0]

outs:
  out1

def first(Table: f32[]):
  return Table[0]

sample:
  out1 = first([2.0])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("const array/function parameter conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("function parameter 'Table' conflicts with constant 'Table'")),
            "expected function parameter/const array conflict, got {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_runtime_defs() {
        let src = r#"
const foo = 1.0

def foo(x: f32):
  return x * 2.0

outs:
  out1

sample:
  out1 = foo(0.5)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const/runtime def name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant name 'foo' conflicts with existing symbol")),
            "expected const/runtime def conflict, got {errors:?}"
        );
    }

    #[test]
    fn runtime_const_refs_reject_forward_scalar_consts() {
        let src = r#"
outs:
  out1

sample:
  out1 = Later

const Later = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward scalar const use should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Later' is not visible before its declaration")),
            "expected forward scalar const diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn runtime_const_refs_reject_forward_const_arrays() {
        let src = r#"
outs:
  out1

sample:
  idx = 0
  out1 = Table[idx]

const Table: f32[1] = [0.5]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const array use should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Table' is not visible before its declaration")),
            "expected forward const array diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn declaration_defaults_reject_forward_const_arrays() {
        let src = r#"
params:
  taps: f32[2] = Table

const Table: f32[2] = [0.25, 0.75]

outs:
  out1

sample:
  out1 = taps[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const array default should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Table' is not visible before its declaration")),
            "expected forward const array default diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn asserts_reject_forward_scalar_consts() {
        let src = r#"
namespace Check:
  assert(Later == 1)

const Later = 1

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const assert should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Later' is not visible before its declaration")),
            "expected forward const assert diagnostic, got {errors:?}"
        );
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
const def unused_missing():
  return 1.0

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unused missing return type should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'unused_missing' must declare an explicit return type")));

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
    fn unused_const_def_bodies_are_structurally_validated() {
        let src = r#"
const def unused_bad() -> f32:
  sin(0.0)
  return 1.0

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unused unsupported const def body should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'unused_bad' statement is not supported")),
            "expected unsupported const def statement diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn const_def_local_consts_are_immutable() {
        let src = r#"
const def bad() -> i32:
  const X = 1
  X = 2
  return X

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("local const reassignment should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'bad' cannot assign to local const 'X'")),
            "expected local const reassignment diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn unused_const_def_loop_vars_cannot_rebind_local_consts() {
        let src = r#"
const def bad() -> i32:
  const X = 1
  for X in 0..2:
    const Y = X
  return X

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("loop var local const reassignment should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'bad' cannot assign to local const 'X'")),
            "expected loop var local const diagnostic, got {errors:?}"
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
    fn const_defs_can_read_any_length_array_params() {
        let src = r#"
const A: f32[] = [0.25, 0.5, 1.0]
const B: f32[] = [2.0, 4.0]

const def sum(xs: f32[]) -> f32:
  total = 0.0
  for i in 0..(xs.len()):
    total = total + xs[i]
  return total

const Table: f32[] = [sum(A), sum(B), sum([10.0, 20.0, 30.0, 40.0])]

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("typed slice const def params should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(1.75),
                TypedConstValue::F32(6.0),
                TypedConstValue::F32(100.0)
            ]
        );
    }

    #[test]
    fn const_defs_can_read_untyped_any_primitive_array_params() {
        let src = r#"
const F: f32[] = [0.25, 0.5]
const I: i32[] = [10, 20, 30]

const def size(xs: []) -> i32:
  return xs.len()

const Sizes: i32[] = [size(F), size(I), size([true, false, true, false])]

outs:
  out1

sample:
  out1 = f32(Sizes[2])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("untyped slice const def params should analyze");

        let sizes = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Sizes")
            .expect("typed const array");
        assert_eq!(
            sizes.values,
            vec![
                TypedConstValue::I32(2),
                TypedConstValue::I32(3),
                TypedConstValue::I32(4)
            ]
        );
    }

    #[test]
    fn const_def_slice_params_are_read_only() {
        let src = r#"
const Source: f32[] = [0.25, 0.5]

const def bad(xs: f32[]) -> f32:
  xs[0] = 1.0
  return xs[0]

const Value = bad(Source)

outs:
  out1

sample:
  out1 = Value
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("read-only const def slice write should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'bad' cannot write read-only array parameter 'xs'")));
    }

    #[test]
    fn const_def_typed_slice_params_reject_wrong_element_type() {
        let src = r#"
const Source: i32[] = [1, 2]

const def first(xs: f32[]) -> f32:
  return xs[0]

const Value = first(Source)

outs:
  out1

sample:
  out1 = Value
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-type const def slice arg should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'first' argument 'xs': expected f32[], got i32[2]")));
    }

    #[test]
    fn const_array_slice_annotations_infer_initializer_length() {
        let src = r#"
const Full: f32[] = [0.0, 0.25, 0.5, 0.75]
const Mid: f32[] = Full[1:-1]

outs:
  out1

sample:
  out1 = Mid[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const slice annotation should infer length");

        let full = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Full")
            .expect("typed full array");
        assert_eq!(full.len, 4);

        let mid = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Mid")
            .expect("typed mid array");
        assert_eq!(mid.len, 2);
        assert_eq!(
            mid.values,
            vec![TypedConstValue::F32(0.25), TypedConstValue::F32(0.5)]
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
    fn untyped_const_can_infer_array_returning_const_def_initializer() {
        let src = r#"
const N = 3

const def harmonic_ratios() -> f32[N]:
  values: f32[N]
  for i in 0..N:
    values[i] = f32(i + 1)
  return values

const Ratios = harmonic_ratios()

outs:
  out1

sample:
  out1 = Ratios[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("untyped array const def initializer should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Ratios")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(1.0),
                TypedConstValue::F32(2.0),
                TypedConstValue::F32(3.0)
            ]
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
        assert!(!diag.editor_visible, "missing sample is compile-only");
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
    fn uninstantiated_generic_proc_rejects_unknown_forwarded_ctor_type_arg() {
        let src = r#"
proc Child<T>:
  outs<T> 1
  sample:
    out1 = 0.0

proc Wrapper<T>:
  init:
    child = Child<thisisnotvalid>()
  sample:
    child()
"#;

        assert_analyze_error_contains(src, "unknown generic type argument 'thisisnotvalid'");
    }

    #[test]
    fn uninstantiated_generic_proc_allows_declared_forwarded_ctor_type_arg() {
        let src = r#"
proc Child<T>:
  outs<T> 1
  sample:
    out1 = 0.0

proc Wrapper<T>:
  init:
    child = Child<T>()
  sample:
    child()
"#;

        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declared forwarded generic type arg should analyze");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_rejects_unknown_forwarded_ctor_type_arg() {
        let src = r#"
namespace dsp:
  proc Child<T>:
    outs<T> 1
    sample:
      out1 = 0.0

  namespace Wrap<N = 4>:
    namespace Mono:
      proc Parent<T>:
        init:
          child = Child<thisisnotvalid>()
        sample:
          child()
"#;

        assert_analyze_error_contains(src, "unknown generic type argument 'thisisnotvalid'");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_allows_declared_forwarded_ctor_type_arg() {
        let src = r#"
namespace dsp:
  proc Child<T>:
    outs<T> 1
    sample:
      out1 = 0.0

  namespace Wrap<N = 4>:
    namespace Mono:
      proc Parent<T>:
        init:
          child = Child<T>()
        sample:
          child()
"#;

        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declared forwarded namespace template type arg should analyze");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_rejects_unknown_template_member() {
        let src = r#"
import std/convolution

namespace Test<FFTSize = 64, MaxKernel = 1024>:
  proc Wrapper<T>:
    outs<T> 1
    init:
      t = std::convolution<FFTSize, MaxKernel>::nope
    sample:
      out1 = 0.0
"#;

        assert_analyze_error_contains(src, "unknown symbol 'nope' in namespace 'std::convolution'");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_rejects_unknown_static_array_size() {
        let src = r#"
namespace Test<MaxKernel = 1024>:
  proc Wrapper<T>:
    outs<T> 1
    init:
      current_ir: T[MaxKrnel]
    sample:
      out1 = 0.0
"#;

        assert_analyze_error_contains(src, "unknown constant 'MaxKrnel'");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_allows_declared_static_array_size() {
        let src = r#"
namespace Test<MaxKernel = 1024>:
  proc Wrapper<T>:
    outs<T> 1
    init:
      current_ir: T[MaxKernel]
    sample:
      out1 = 0.0
"#;

        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declared namespace template size should analyze");
    }

    #[test]
    fn proc_input_and_param_array_indices_preserve_declared_element_type() {
        let src = r#"
proc Voice:
  ins:
    vals: i64[2] = [5, 6]
  params:
    gains: i64[2] = [1, 2]
  outs:
    out1
  init:
    start: i64 = gains[0]
  sample:
    total: i64 = vals[0] + gains[1] + start
    out1 = f32(total)

outs:
  out1

init:
  voice = Voice()

sample:
  out1 = voice(vals = [7, 8])
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc array port and param indices should preserve i64 type");
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
    fn def_param_shadows_same_named_top_level_buffer_during_monomorphization() {
        let src = r#"
buffers:
  buf: buffer[f32]

outs:
  out1

def read_first(buf: buffer[f32], index: i32):
  return buf[index]

sample:
  out1 = read_first(buf, 0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("def parameter should shadow the top-level buffer binding");
    }

    #[test]
    fn block_without_nested_sample_reports_only_block_specific_error() {
        let src = "outs { out1 }\nblock { x = 0.0 }\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block without nested sample should fail");

        let diagnostic = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("sample-rate outputs must include nested 'sample' block")
            })
            .expect("missing block-specific diagnostic");
        assert!(!diagnostic.editor_visible, "missing sample is compile-only");
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
    fn value_returning_def_requires_both_if_branches_to_return() {
        let src = "outs:\n  out1\ndef choose(flag: bool) -> f32:\n  if flag:\n    return 1.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("partial return should fail");
        let diagnostic = errors
            .iter()
            .find(|diag| {
                diag.message.contains("function 'choose' returns a value")
                    && diag.message.contains("not all reachable paths")
            })
            .expect("missing partial-return diagnostic");
        assert_eq!((diagnostic.line, diagnostic.column), (3, 1));
    }

    #[test]
    fn value_returning_def_accepts_complete_if_else() {
        let src = "outs:\n  out1\ndef choose(flag: bool) -> f32:\n  if flag:\n    return 1.0\n  else:\n    return 2.0\nsample:\n  out1 = choose(true)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("complete branch returns should analyze");
        let choose = typed
            .defs
            .iter()
            .find(|def| def.name == "choose")
            .expect("missing choose def");
        assert!(choose.returns_value);
    }

    #[test]
    fn loop_nested_return_is_conservatively_not_total() {
        let src = "outs:\n  out1\ndef first(n: i32) -> i32:\n  for i in 0..n:\n    return i\nsample:\n  out1 = f32(first(1))\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("loop-only return should fail");
        assert!(errors.iter().any(|diag| {
            diag.message.contains("function 'first' returns a value")
                && diag.message.contains("not all reachable paths")
        }));
    }

    #[test]
    fn nested_branch_return_does_not_cover_outer_fallthrough() {
        let src = "outs:\n  out1\ndef nested(a: bool, b: bool):\n  if a:\n    if b:\n      return 1.0\n    else:\n      return 2.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("outer fallthrough should fail");
        assert!(errors.iter().any(|diag| {
            diag.message.contains("function 'nested' returns a value")
                && diag.message.contains("not all reachable paths")
        }));
    }

    #[test]
    fn return_after_conservative_loop_covers_fallthrough() {
        let src = "outs:\n  out1\ndef first_or_zero(n: i32) -> i32:\n  for i in 0..n:\n    if i > 0:\n      return i\n  return 0\nsample:\n  out1 = f32(first_or_zero(1))\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("post-loop return should cover loop fallthrough");
    }

    #[test]
    fn no_result_def_may_fall_through() {
        let src = "outs:\n  out1\ndef observe(flag: bool):\n  if flag:\n    value = 1.0\n  while flag:\n    break\nsample:\n  observe(false)\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("no-result def should be allowed to fall through");
        let observe = typed
            .defs
            .iter()
            .find(|def| def.name == "observe")
            .expect("missing observe def");
        assert!(!observe.returns_value);
    }

    #[test]
    fn proc_local_value_helper_requires_total_return() {
        let src = "proc Voice:\n  outs:\n    out1\n  def choose(flag: bool) -> f32:\n    if flag:\n      return 1.0\n  sample:\n    out1 = 0.0\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("partial proc-local helper should fail");
        assert!(errors.iter().any(|diag| {
            diag.message.contains("Voice")
                && diag.message.contains("returns a value")
                && diag.message.contains("not all reachable paths")
        }));
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
    fn typed_scalar_call_argument_rejects_implicit_narrowing() {
        let src = r#"
outs:
  out1

def take(x: f32) -> f32:
  return x

sample:
  wide: f64 = 1.25
  out1 = take(wide)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("typed f64 call argument must not narrow to f32");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains("function 'take'")
                    && diagnostic.message.contains("cannot assign F64 to F32")
            }),
            "expected call-argument narrowing diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn typed_scalar_call_argument_accepts_contextual_numeric_literal() {
        let src = r#"
outs:
  out1

def take(x: f32) -> f32:
  return x

sample:
  out1 = take(1.25)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("a numeric literal should adopt the f32 parameter context");
    }

    #[test]
    fn repeated_generic_scalar_constraints_choose_one_widened_type() {
        let src = r#"
outs:
  out1

def choose<T>(x: T, lo: T, hi: T) -> T:
  if x < lo:
    return lo
  if x > hi:
    return hi
  return x

sample:
  x: f32 = f32(16777216.0)
  lo: f64 = f64(16777217.0)
  hi: f64 = f64(16777218.0)
  out1 = f32(choose(x, lo, hi) - lo)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("mixed scalar constraints should widen T to f64");
        let choose = typed
            .defs
            .iter()
            .find(|function| function.name.starts_with("choose.__mono__g_f64"))
            .expect("missing widened f64 specialization");
        assert!(
            choose.param_kinds.iter().all(|kind| matches!(
                kind,
                TypedFnParam::Scalar {
                    ty: Some(PrimitiveType::F64)
                }
            )),
            "all repeated T parameters must use the same f64 specialization: {:?}",
            choose.param_kinds
        );
        assert_eq!(choose.return_ty, ReturnType::Scalar(PrimitiveType::F64));
    }

    #[test]
    fn explicit_generic_scalar_type_rejects_typed_narrowing_argument() {
        let src = r#"
outs:
  out1

def id<T>(x: T) -> T:
  return x

sample:
  wide: f64 = 1.25
  out1 = id<f32>(wide)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("explicit f32 generic argument must not narrow f64");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains("function 'id")
                    && diagnostic.message.contains("cannot assign F64 to F32")
            }),
            "expected explicit-generic narrowing diagnostic, got {errors:?}"
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
    fn duplicate_generated_generic_struct_specialization_is_deduped() {
        let src = "namespace sc:\n  struct CyclePhase<T>:\n    phase: T\n\n    def tick(self):\n      self.phase = self.phase + T(1.0)\n      return self.phase\n\n  namespace Sine:\n    proc ar<T>:\n      outs:\n        out1: T\n      init<T>:\n        core = sc::CyclePhase<T>()\n      sample:\n        out1 = core.tick()\n\nouts:\n  out1\ninit:\n  a = sc::Sine::ar()\n  z = sc::CyclePhase<f32>()\n\nsample:\n  out1 = a()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program)
            .expect("duplicate generated generic struct specialization should be deduped");
        let generated = typed
            .structs
            .iter()
            .filter(|s| s.name == "sc::CyclePhase.__gen__f32")
            .count();
        assert_eq!(generated, 1);
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
            .find(|def| def.name.starts_with("set_gains"))
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
                .find(|def| def.name.starts_with(def_name))
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
                .find(|def| def.name.starts_with(def_name))
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
                .contains("for sample-rate proc is only allowed in sample")),
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
                .contains("for sample-rate proc is only allowed in sample")),
            "expected sample-only proc operator diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn unqualified_proc_event_call_reports_receiver_only_guidance() {
        let src = r#"
proc Phasor:
  params:
    freq = 1.0

  event set_freq(val):
    freq = val

  init:
    set_freq(freq)

  outs:
    out1

  sample:
    out1 = freq

outs:
  out1

init:
  phasor = Phasor()

sample:
  out1 = phasor()
"#;

        assert_analyze_error_contains(src, "proc event 'set_freq' is receiver-only");
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
                    || step.as_ref().is_some_and(expr_contains_proc_index_sentinel)
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
    fn sample_scoped_local_name_can_be_reintroduced_with_a_different_type() {
        let src = r#"
outs:
  out1

sample:
  if in1 > 0.0:
    temp = 0.0
  for i in 0..1:
    temp = f32(i)
  temp = true
  if temp:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("branch- and loop-local bindings must not escape their scopes");
    }

    #[test]
    fn multi_argument_numeric_builtins_adapt_literals_to_a_concrete_peer() {
        let src = r#"
def bounded(x: f32) -> f32:
  return fma(max(x, 16777217.0), 1.0, 0.0)

outs:
  out1

sample:
  out1 = bounded(f32(16777216.0))
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("builtin literals should adopt the concrete f32 peer width");
    }

    #[test]
    fn comparison_literal_accepts_the_concrete_f32_peer_context() {
        let src = r#"
outs:
  out1

sample:
  x: f32 = f32(16777216.0)
  if x == 16777217.0:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("comparison literal should adopt its concrete f32 peer width");
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
    fn block_pre_cannot_read_sample_rate_inputs() {
        let src = "ins:\n  in1\nouts:\n  out1\nblock:\n  held = in1\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre input read should fail");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'in1'")),
            "missing unknown-symbol diagnostic for block pre input read: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_read_dynamic_inputs() {
        let src = "ins 1\nouts:\n  out1\nblock:\n  held = ins[0]\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre dynamic input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains("ins[i]")),
            "missing dynamic-input diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_post_cannot_read_dynamic_inputs() {
        let src = "ins 1\nouts:\n  out1\nblock:\n  sample:\n    out1 = in1\n  held = ins[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block post dynamic input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains("ins[i]")),
            "missing dynamic-input diagnostic for block post: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_write_dynamic_outputs() {
        let src = "outs 1\nblock:\n  outs[0] = 0.0\n  sample:\n    out1 = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre dynamic output write should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains("outs[i]")),
            "missing dynamic-output diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_read_input_arrays() {
        let src = "ins:\n  freqs: f32[2] = [220, 440]\nouts:\n  out1\nblock:\n  held = freqs[0]\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre input array read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains("freqs")),
            "missing input-array diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_write_output_arrays() {
        let src = "outs:\n  stereo: f32[2]\nblock:\n  stereo[0] = 0.0\n  sample:\n    stereo[0] = 1.0\n    stereo[1] = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre output array write should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains("stereo")),
            "missing output-array diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_can_read_dynamic_params() {
        let src = "params 2\nouts:\n  out1\nblock:\n  held = params[0] + params[1]\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("block pre dynamic param read should analyze");
    }

    #[test]
    fn dynamic_params_are_not_first_class_values() {
        let cases = [
            (
                "top-level local alias",
                "params 2\nouts:\n  out1\nblock:\n  ps = params\n  sample:\n    out1 = ps[0]\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level slice alias",
                "params 2\nouts:\n  out1\nblock:\n  ps = params[0:2]\n  sample:\n    out1 = ps[0]\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level def argument",
                "def sum(ps: f32[]) -> f32:\n  return ps[0]\nparams 2\nouts:\n  out1\nsample:\n  out1 = sum(params)\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level def indexed read",
                "def get() -> f32:\n  return params[0]\nparams 2\nouts:\n  out1\nsample:\n  out1 = get()\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "top-level init indexed read",
                "params 2\nouts:\n  out1\ninit:\n  held = params[0]\nsample:\n  out1 = held\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "top-level block surface assignment",
                "params 2\nouts:\n  out1\nblock:\n  params = 1.0\n  sample:\n    out1 = params[0]\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level sample kins surface assignment",
                "kins 2\nouts:\n  out1\nsample:\n  kins = 1.0\n  out1 = kins[0]\n",
                "dynamic param array 'kins' is not a first-class value",
            ),
            (
                "proc init indexed read",
                "proc P:\n  params 2\n  outs:\n    out1\n  init:\n    held = params[0]\n  sample:\n    out1 = param1\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "proc local def indexed read",
                "proc P:\n  params 2\n  outs:\n    out1\n  def get() -> f32:\n    return params[0]\n  sample:\n    out1 = get()\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "proc local def surface assignment",
                "proc P:\n  params 2\n  outs:\n    out1\n  def set():\n    params = 1.0\n  sample:\n    set()\n    out1 = param1\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level kins def indexed read",
                "def get() -> f32:\n  return kins[0]\nkins 2\nouts:\n  out1\nsample:\n  out1 = get()\n",
                "dynamic param indexing 'kins[...]' is only allowed in block or sample",
            ),
            (
                "top-level kins first-class argument",
                "def sum(ps: f32[]) -> f32:\n  return ps[0]\nkins 2\nouts:\n  out1\nsample:\n  out1 = sum(kins)\n",
                "dynamic param array 'kins' is not a first-class value",
            ),
            (
                "child param surface alias",
                "proc Child:\n  params:\n    a = 0.0\n    b = 0.0\n  outs:\n    out1\n  sample:\n    out1 = a\nproc Parent:\n  init:\n    child = Child()\n  outs:\n    out1\n  sample:\n    ps = child.params\n    out1 = ps[0]\nouts:\n  out1\ninit:\n  p = Parent()\nsample:\n  out1 = p()\n",
                "dynamic param array 'child.params' is not a first-class value",
            ),
            (
                "child param surface assignment",
                "proc Child:\n  params:\n    a = 0.0\n    b = 0.0\n  outs:\n    out1\n  sample:\n    out1 = a\nproc Parent:\n  init:\n    child = Child()\n  outs:\n    out1\n  sample:\n    child.params = 1.0\n    out1 = child()\nouts:\n  out1\ninit:\n  p = Parent()\nsample:\n  out1 = p()\n",
                "dynamic param array 'child.params' is not a first-class value",
            ),
        ];

        for (_label, src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }
    }

    #[test]
    fn io_surfaces_are_block_sample_bound_and_not_first_class_values() {
        let cases = [
            (
                "init scalar input read",
                "ins:\n  in1\nouts:\n  out1\ninit:\n  held = in1\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'in1' is only available in block or sample",
            ),
            (
                "init named input read",
                "ins:\n  audio\nouts:\n  out1\ninit:\n  held = audio\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
            (
                "def scalar input read",
                "def get() -> f32:\n  return in1\nins:\n  in1\nouts:\n  out1\nsample:\n  out1 = get()\n",
                "I/O symbol 'in1' is only available in block or sample",
            ),
            (
                "def named output read",
                "def get() -> f32:\n  return wet\nouts:\n  wet\nsample:\n  wet = 0.0\n",
                "I/O symbol 'wet' is only available in block or sample",
            ),
            (
                "event scalar output write",
                "outs:\n  out1\nevent ping():\n  out1 = 1.0\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'out1' is only available in block or sample",
            ),
            (
                "event named kout write",
                "kouts:\n  meter\nevent ping():\n  meter = 1.0\nblock:\n  meter = 0.0\n",
                "I/O symbol 'meter' is only available in block or sample",
            ),
            (
                "init dynamic input read",
                "ins 2\nouts:\n  out1\ninit:\n  held = ins[0]\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'ins' is only available in block or sample",
            ),
            (
                "def dynamic kouts read",
                "def get() -> f32:\n  return kouts[0]\nkouts 2\nblock:\n  kouts[0] = 0.0\n  kouts[1] = get()\n",
                "I/O symbol 'kouts' is only available in block or sample",
            ),
            (
                "sample synthetic input assignment",
                "ins 2\nouts:\n  out1\nsample:\n  ins = 0.0\n  out1 = ins[0]\n",
                "I/O array 'ins' is not a first-class value",
            ),
            (
                "sample synthetic output assignment",
                "outs 2\nsample:\n  outs = 0.0\n  outs[0] = 0.0\n  outs[1] = 0.0\n",
                "I/O array 'outs' is not a first-class value",
            ),
            (
                "block synthetic kouts assignment",
                "kouts 2\nblock:\n  kouts = 0.0\n  kouts[0] = 0.0\n",
                "I/O array 'kouts' is not a first-class value",
            ),
            (
                "sample input array argument",
                "def first(xs: f32[]) -> f32:\n  return xs[0]\nins:\n  freqs: f32[2] = [220, 440]\nouts:\n  out1\nsample:\n  out1 = first(freqs)\n",
                "I/O array 'freqs' is not a first-class value",
            ),
            (
                "sample input array slice alias",
                "ins:\n  freqs: f32[2] = [220, 440]\nouts:\n  out1\nsample:\n  fs = freqs[0:2]\n  out1 = fs[0]\n",
                "I/O array 'freqs' is not a first-class value",
            ),
            (
                "sample synthetic output argument",
                "def poke(xs: f32[]):\n  xs[0] = 1.0\nouts 2\nsample:\n  poke(outs)\n  outs[0] = 0.0\n  outs[1] = 0.0\n",
                "I/O array 'outs' is not a first-class value",
            ),
            (
                "top-level event named input read",
                "ins:\n  audio\nouts:\n  out1\nevent ping():\n  held = audio\nsample:\n  out1 = audio\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
            (
                "proc init named input read",
                "proc P:\n  ins:\n    audio\n  outs:\n    out1\n  init:\n    held = audio\n  sample:\n    out1 = audio\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p(0.0)\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
            (
                "proc event named output write",
                "proc P:\n  outs:\n    wet\n  event ping():\n    wet = 1.0\n  sample:\n    wet = 0.0\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "I/O symbol 'wet' is only available in block or sample",
            ),
            (
                "proc local def named input read",
                "proc P:\n  ins:\n    audio\n  outs:\n    out1\n  def get() -> f32:\n    return audio\n  sample:\n    out1 = get()\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p(0.0)\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
        ];

        for (_label, src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }
    }

    #[test]
    fn top_level_kins_alias_analyzes_as_params() {
        let src = r#"
kins:
  gain = 0.25

outs:
  out1

sample:
  out1 = gain
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("kins alias should analyze as params");

        assert_eq!(typed.params.len(), 1);
        assert_eq!(typed.params[0].name, "gain");
        assert_eq!(typed.param_types["gain"], PrimitiveType::F32);
    }

    #[test]
    fn top_level_infers_numbered_kins_as_params() {
        let src = r#"
sample:
  out1 = kin2
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("implicit kinN params should analyze");

        assert_eq!(
            typed
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["kin1", "kin2"]
        );
        assert_eq!(typed.outs, vec!["out1"]);
        assert!(!typed.params_explicit);
    }

    #[test]
    fn top_level_infers_numbered_params_as_params() {
        let src = r#"
sample:
  out1 = param2
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("implicit paramN params should analyze");

        assert_eq!(
            typed
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["param1", "param2"]
        );
        assert_eq!(typed.outs, vec!["out1"]);
    }

    #[test]
    fn top_level_dynamic_kins_indexes_explicit_kins() {
        let src = r#"
kins 2
sample:
  out1 = kins[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic kins should analyze");

        assert_eq!(
            typed
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["kin1", "kin2"]
        );
        assert!(typed.params_explicit);
    }

    #[test]
    fn top_level_infers_numbered_kouts() {
        let src = r#"
block:
  kout2 = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("implicit koutN outputs should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["kout1", "kout2"]);
    }

    #[test]
    fn top_level_dynamic_kouts_indexes_explicit_kouts() {
        let src = r#"
kouts 2
block:
  kouts[1] = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic kouts should analyze");

        assert_eq!(typed.control_outs, vec!["kout1", "kout2"]);
    }

    #[test]
    fn top_level_kouts_arrays_use_array_storage_not_scalar_slots() {
        let src = r#"
kouts:
  meter: f32[2]

block:
  meter[0] = 0.25
  meter[1] = 0.75
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("kouts array should analyze");

        assert_eq!(typed.control_outs, vec!["meter[0]", "meter[1]"]);
        assert!(typed
            .array_vars
            .iter()
            .any(|array| array.name == "meter" && array.len == 2));
        assert!(!typed.state_vars.iter().any(|name| name == "meter[0]"));
        assert!(!typed.state_vars.iter().any(|name| name == "meter[1]"));
    }

    #[test]
    fn top_level_rejects_both_params_and_kins() {
        assert_analyze_error_contains(
            r#"
params:
  gain = 0.25

kins:
  freq = 440.0

outs:
  out1

sample:
  out1 = gain
"#,
            "duplicate block 'params'",
        );
    }

    #[test]
    fn top_level_mixed_sample_and_block_outputs_are_split() {
        let src = r#"
outs:
  out1
kouts:
  meter

block:
  meter = 1.0
  sample:
    out1 = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("mixed output timing should analyze");

        assert_eq!(typed.outs, vec!["out1"]);
        assert_eq!(typed.control_outs, vec!["meter"]);
        assert_eq!(typed.out_types["out1"], PrimitiveType::F32);
        assert_eq!(typed.control_out_types["meter"], PrimitiveType::F32);
        assert!(typed.state_vars.iter().any(|name| name == "meter"));
    }

    #[test]
    fn top_level_rejects_output_and_control_output_name_conflict() {
        assert_analyze_error_contains(
            r#"
outs:
  myOut
kouts:
  myOut

block:
  myOut = 1.0
  sample:
    myOut = 0.5
"#,
            "control output 'myOut' conflicts with output 'myOut'",
        );
    }

    #[test]
    fn top_level_rejects_numbered_audio_names_in_kouts() {
        assert_analyze_error_contains(
            r#"
kouts:
  out1

block:
  out1 = 1.0
"#,
            "use 'koutN' for control outputs",
        );
    }

    #[test]
    fn top_level_control_only_block_output_does_not_require_sample() {
        let src = r#"
kouts:
  meter

block:
  meter = 1.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("control-only block should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["meter"]);
    }

    #[test]
    fn top_level_control_only_output_requires_block_entry() {
        assert_analyze_error_contains(
            r#"
kouts:
  meter
"#,
            "missing required 'block' section",
        );
    }

    #[test]
    fn legacy_outs_rate_aliases_are_rejected_by_parser() {
        let src = r#"
outs @block {
  meter
}

block {
  meter = 1.0
}
"#;
        let errors = parse_program(src).expect_err("outs @block should not parse");
        assert!(!errors.is_empty());
    }

    #[test]
    fn current_owner_outputs_are_write_only() {
        assert_analyze_error_contains(
            r#"
outs {
  out1
}

sample {
  out1 = out1
}
"#,
            "cannot read output symbol 'out1'",
        );
    }

    #[test]
    fn current_owner_output_arrays_are_write_only() {
        assert_analyze_error_contains(
            r#"
outs {
  stereo: f32[2]
}

sample {
  stereo[0] = 0.25
  stereo[1] = stereo[0]
}
"#,
            "cannot read output array symbol 'stereo[...]'",
        );
        assert_analyze_error_contains(
            r#"
kouts {
  meter: f32[2]
}

block {
  meter[0] = 1.0
  meter[1] = meter[0]
}
"#,
            "cannot read output array symbol 'meter[...]'",
        );
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter: f32[2]
}

block {
  sample {
    out1 = 0.0
    meter[0] = 1.0
  }
}
"#,
            "cannot assign to output array symbol 'meter' in sample",
        );
    }

    #[test]
    fn current_owner_output_arrays_allow_matching_phase_writes() {
        let program = parse_program(
            r#"
outs {
  stereo: f32[2]
}
kouts {
  meter: f32[2]
}

block {
  meter[0] = 1.0
  meter[1] = 2.0
  sample {
    stereo[0] = 0.25
    stereo[1] = 0.5
  }
}
"#,
        )
        .expect("parse should succeed");
        analyze(program).expect("matching-phase output array writes should analyze");
    }

    #[test]
    fn protected_proc_views_cannot_be_passed_as_array_pointers() {
        assert_analyze_error_contains(
            r#"
def poke(ps: f32[]):
  ps[0] = 1.0

params {
  gain = 0.0
  trim = 0.0
}
outs {
  out1
}

sample {
  poke(params)
  out1 = gain
}
"#,
            "dynamic param array 'params' is not a first-class value",
        );
    }

    #[test]
    fn mixed_timing_outputs_are_write_only_in_all_runtime_scopes() {
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter
}
block {
  meter = 1.0
  sample {
    out1 = meter
  }
}
"#,
            "cannot read output symbol 'meter'",
        );
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter
}
block {
  held = out1
  meter = 1.0
  sample {
    out1 = 0.0
  }
}
"#,
            "cannot read output symbol 'out1'",
        );
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter
}
block {
  meter = 1.0
  sample {
    out1 = kouts[0]
  }
}
"#,
            "cannot read output symbol 'kouts[i]'",
        );
    }

    #[test]
    fn mixed_timing_outputs_allow_matching_dynamic_indexing() {
        let program = parse_program(
            r#"
outs 2
kouts 2

block {
  kouts[1] = 1.0
  sample {
    outs[0] = 0.25
    outs[1] = 0.5
  }
}
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("mixed dynamic outputs should analyze");

        assert_eq!(typed.outs, vec!["out1", "out2"]);
        assert_eq!(typed.control_outs, vec!["kout1", "kout2"]);
    }

    #[test]
    fn proc_rejects_both_outs_and_kouts() {
        let errors = parse_program(
            r#"
proc P
{
  outs {
    out1
  }
  kouts {
    kout1
  }
  sample {
    out1 = 1.0
  }
}
"#,
        )
        .expect_err("proc cannot declare both outs and kouts");
        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("duplicate proc output block")));
    }

    #[test]
    fn block_rate_proc_operator_can_be_called_from_block() {
        let src = r#"
proc Meter
{
  kouts {
    kout1
  }
  block {
    kout1 = 2.0
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("block-rate proc operator should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["meter"]);
    }

    #[test]
    fn block_rate_proc_ordinal_alias_uses_kout_prefix() {
        let src = r#"
proc Meter
{
  kouts {
    level
  }
  block {
    level = 2.0
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m().kout1
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("kouts proc ordinal alias should use koutN");
    }

    #[test]
    fn block_rate_proc_rejects_audio_ordinal_alias() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    level
  }
  block {
    level = 2.0
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m().out1
}
"#,
            "or koutN",
        );
    }

    #[test]
    fn sample_rate_proc_rejects_control_ordinal_alias() {
        assert_analyze_error_contains(
            r#"
proc Voice
{
  outs {
    wet
  }
  sample {
    wet = 1.0
  }
}

outs {
  out1
}

init {
  v = Voice()
}

sample {
  out1 = v().kout1
}
"#,
            "or outN",
        );
    }

    #[test]
    fn block_rate_proc_kout_alias_conflicts_with_event_name() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    level
  }
  init {
    held = 0.0
  }
  event kout1():
    held = 1.0
  block {
    level = held
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m()
}
"#,
            "event name conflicts",
        );
    }

    #[test]
    fn block_rate_proc_operator_can_be_called_from_proc_block() {
        let src = r#"
proc Meter
{
  kouts<i32> {
    kout1
  }
  block {
    kout1 = 2
  }
}

proc Outer
{
  kouts<i32> {
    kout1
  }
  init {
    m = Meter()
  }
  block {
    kout1 = m()
  }
}

kouts {
  meter: i32
}

init {
  o = Outer()
}

block {
  meter = o()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("nested block-rate proc operator should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["meter"]);
    }

    #[test]
    fn block_rate_proc_operator_is_rejected_from_proc_sample() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    kout1
  }
  block {
    kout1 = 2.0
  }
}

proc Voice
{
  outs {
    out1
  }
  init {
    m = Meter()
  }
  sample {
    out1 = m()
  }
}

outs {
  out1
}

init {
  v = Voice()
}

sample {
  out1 = v()
}
"#,
            "for block-rate proc is only allowed in block",
        );
    }

    #[test]
    fn proc_infers_numbered_params() {
        let src = r#"
proc Voice
{
  sample {
    out1 = param2
  }
}

init {
  v = Voice(param2 = 0.25)
}

sample {
  out1 = v()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("implicit proc paramN should analyze");
    }

    #[test]
    fn proc_rejects_numbered_kins() {
        assert_analyze_error_contains(
            r#"
proc Voice
{
  sample {
    out1 = kin1
  }
}

sample {
  out1 = Voice()
}
"#,
            "unknown symbol 'kin1'",
        );
    }

    #[test]
    fn proc_rejects_dynamic_kins() {
        assert_analyze_error_contains(
            r#"
proc Voice
{
  sample {
    out1 = kins[0]
  }
}

sample {
  out1 = Voice()
}
"#,
            "'kins[i]' requires",
        );
    }

    #[test]
    fn proc_infers_numbered_kouts_and_block_timing() {
        let src = r#"
proc Meter
{
  block {
    kout1 = param1
  }
}

init {
  m = Meter(param1 = 0.5)
}

kouts {
  meter
}

block {
  meter = m()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("implicit proc koutN should analyze as block-rate proc");
    }

    #[test]
    fn nested_proc_surface_infers_child_kout_fields() {
        let src = r#"
proc Meter
{
  block {
    kout1 = 2.0
  }
}

proc Outer
{
  outs {
    out1
  }
  init {
    m = Meter()
  }
  sample {
    out1 = m.kout1
  }
}

outs {
  out1
}

init {
  o = Outer()
}

sample {
  out1 = o()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("implicit child kout field should analyze");
    }

    #[test]
    fn proc_rejects_explicit_outs_with_inferred_kouts() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  outs {
    out1
  }
  block {
    kout1 = 1.0
  }
}

sample {
  out1 = Meter()
}
"#,
            "cannot mix outs and inferred control koutN outputs",
        );
    }

    #[test]
    fn proc_dynamic_kouts_indexes_control_outputs() {
        let src = r#"
proc Meter
{
  kouts 2
  block {
    kouts[1] = param1
  }
}

init {
  m = Meter(param1 = 0.5)
}

kouts {
  meter
}

block {
  meter = m().kout2
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc dynamic kouts should analyze");
    }

    #[test]
    fn top_level_graph_rejects_block_timed_outputs() {
        assert_analyze_error_contains(
            r#"
kouts {
  meter
}

graph {
  1.0 >> meter
}
"#,
            "top-level graph block does not support kouts",
        );
    }

    #[test]
    fn graph_source_rejects_block_rate_proc_output() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    kout1
  }
  block {
    kout1 = 2.0
  }
}

outs {
  out1
}

init {
  m = Meter()
}

graph {
  m.kout1 >> out1
}
"#,
            "graph source cannot read block-rate processor output",
        );
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
    fn proc_block_pre_cannot_read_dynamic_inputs() {
        let src = "proc Voice:\n  ins 1\n  outs:\n    out1\n  block:\n    held = ins[0]\n    sample:\n      out1 = held\nins:\n  in1\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice(in1)\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("proc block pre dynamic input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains("ins[i]")),
            "missing proc dynamic-input diagnostic for block pre: {errors:#?}"
        );
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
    fn generic_proc_event_scalar_params_specialize_with_defaults() {
        let src = r#"
proc Filter<T>:
  outs:
    out1
  init<T>:
    freq = 0.0
    rq = 0.0
  events:
    set(freqv: T = 1200.0, rqv: T = 1.0):
      freq = freqv
      rq = rqv
  sample:
    out1 = f32(freq + rq)

outs:
  out1
init:
  a = Filter<f32>()
  b = Filter<f64>()
  a.set()
  b.set(rqv = 0.5)
sample:
  out1 = a() + b()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("generic scalar proc event params should specialize");
    }

    #[test]
    fn generic_proc_event_fixed_array_params_specialize_with_defaults() {
        let src = r#"
proc Loader<T>:
  outs:
    out1
  init<T>:
    sum = 0.0
  events:
    load(values: T[2] = [1.0, 2.0]):
      sum = values[0] + values[1]
  sample:
    out1 = f32(sum)

outs:
  out1
init:
  loader = Loader<f64>()
  loader.load()
sample:
  out1 = loader()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("generic fixed-array proc event params should specialize");
    }

    #[test]
    fn individual_proc_event_syntax_merges_with_proc_events_block_during_analysis() {
        let src = "proc Voice:\n  outs:\n    out1\n  event ping(x: i32):\n    phase = f32(x)\n  events:\n    reset():\n      phase = 0.0\n  init:\n    phase = 0.0\n  sample:\n    out1 = phase\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("merged proc event syntax should analyze");
    }

    #[test]
    fn runtime_defs_reject_direct_recursion_as_unbounded_realtime_work() {
        let src = r#"
def recurse(x: f32) -> f32:
  return recurse(x)

sample:
  out1 = recurse(0.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("recursive runtime def should fail");
        assert!(errors.iter().any(|diagnostic| diagnostic
            .message
            .contains("recursive runtime def cycle is not realtime-safe: recurse -> recurse")));
    }

    #[test]
    fn runtime_defs_reject_mutual_recursion_as_unbounded_realtime_work() {
        let src = r#"
def first(x: f32) -> f32:
  return second(x)

def second(x: f32) -> f32:
  return first(x)

sample:
  out1 = first(0.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mutually recursive runtime defs should fail");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic.message.contains(
                "recursive runtime def cycle is not realtime-safe: first -> second -> first",
            )
        }));
    }

    #[test]
    fn typed_program_resolves_dynamic_interface_views_to_concrete_slots() {
        let src = r#"
ins:
  dry: f32 = 0.0
  bands: f32[2] = [0.0, 0.0]

outs:
  main: f32
  pair: f32[2]

kouts:
  meter: f32
  leds: f32[2]

params:
  gain: f32 = 0.5
  controls: f32[2] = [0.25, 0.75]

block:
  kouts[0] = params[0]
  sample:
    outs[0] = ins[0]
    outs[1] = ins[1] * params[1]
    outs[2] = ins[2] * params[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("uniform interface views should resolve");

        let inputs = typed.interface_views.inputs.as_ref().expect("input view");
        assert_eq!(inputs.element_type, PrimitiveType::F32);
        assert_eq!(
            inputs
                .slots
                .iter()
                .map(|slot| (slot.id.raw(), slot.root.as_str(), slot.element))
                .collect::<Vec<_>>(),
            vec![
                (0, "dry", None),
                (1, "bands", Some(0)),
                (2, "bands", Some(1))
            ]
        );
        let outputs = typed
            .interface_views
            .audio_outputs
            .as_ref()
            .expect("audio output view");
        assert_eq!(outputs.slots[0].root, "main");
        assert_eq!(outputs.slots[1].root, "pair");
        assert_eq!(outputs.slots[1].element, Some(0));
        assert_eq!(outputs.slots[2].element, Some(1));
        assert_eq!(
            typed
                .interface_views
                .control_outputs
                .as_ref()
                .expect("control output view")
                .slots
                .len(),
            3
        );
        assert_eq!(
            typed
                .interface_views
                .params
                .as_ref()
                .expect("parameter view")
                .slots
                .len(),
            3
        );
    }
}
