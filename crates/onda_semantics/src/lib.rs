// Semantic passes intentionally keep their data dependencies explicit. Most of
// these functions are private traversal helpers, where bundling unrelated
// symbol tables into broad context objects would hide borrowing and mutation
// boundaries without reducing complexity.
#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, HashMap, HashSet};

use onda_frontend::{
    inject_auto_std_math, ArrayElemType, ArrayTypeSpec, AssignTarget, BinaryOp, Block, BlockExec,
    BlockKind, BufferBlock, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn,
    CallArg, CallTypeArg, CmpOp, ConstDecl, ConstType, DeclRange, DeclType, DelegateDef, DiagCtx,
    Diagnostic, EventDef, EventParamDecl, EventParamType, Expr, FieldType, FnParamDecl,
    FnParamType, FnReturnScalarType, FnReturnType, FunctionDef, GraphBlock, GraphEdge,
    GraphEndpoint, GraphRate, InitBlock, NamespaceAliasDecl, NamespaceCallArg, NamespaceDecl,
    NamespaceItem, NamespaceRefSegment, OutputTiming, ParamBlock, ParamDecl, ParamScale, PortBlock,
    PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc, Stmt, StructDef,
    StructField, UseDecl, WhenDef, INTERNAL_BARE_RETURN_FN, INTERNAL_BUFFER_READ2_FN,
    INTERNAL_BUFFER_READ3_FN, INTERNAL_BUFFER_READ_CHANNEL_FN, INTERNAL_BUFFER_WRITE2_FN,
    INTERNAL_BUFFER_WRITE3_FN, INTERNAL_BUFFER_WRITE_CHANNEL_FN, READ_UNSAFE_FN, WRITE_UNSAFE_FN,
};

pub(crate) fn path_or_ancestor_is_declared(path: &str, roots: &HashSet<String>) -> bool {
    let mut candidate = path;
    loop {
        if roots.contains(candidate) {
            return true;
        }
        let Some((parent, _)) = candidate.rsplit_once('.') else {
            return false;
        };
        candidate = parent;
    }
}

fn event_param_as_fn_param(param: &EventParamDecl) -> FnParamDecl {
    let ty = match &param.ty {
        EventParamType::Scalar(ty) => FnParamType::Primitive(*ty),
        EventParamType::Array { elem, size } => FnParamType::SizedArray {
            elem: Some(*elem),
            generic_name: None,
            size: size.clone(),
        },
        EventParamType::Slice { elem } => FnParamType::Array(Some(*elem)),
        EventParamType::GenericScalar { name } => FnParamType::Struct(name.clone()),
        EventParamType::GenericArray { elem, size } => FnParamType::SizedArray {
            elem: None,
            generic_name: Some(elem.clone()),
            size: size.clone(),
        },
        EventParamType::GenericSlice { elem } => FnParamType::ArrayGeneric(elem.clone()),
    };
    FnParamDecl {
        loc: param.loc,
        name: param.name.clone(),
        ty: Some(ty),
        ty_loc: param.ty_loc,
        default: param.default.clone(),
    }
}

pub mod aggregate_layout;
mod analysis_session;
mod array_structs;
pub mod builtins;
mod callable_validation;
mod decl_symbols;
mod declaration_coercion;
mod def_semantics;
mod diag_utils;
mod executable_owner_analysis;
mod expr_analysis;
mod expr_typing;
mod expr_validation;
mod generic_specialization;
mod index_access;
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
mod task_lowering;
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
use index_access::*;
pub(crate) use internal_names::runtime_proc_array_active_symbol;
use io_state_helpers::*;
pub use mir_lowering::{lower_program_to_optimized_mir, MirLoweringError};
use namespacing::*;
pub use pipeline::{
    analyze, analyze_with_options, analyze_with_options_and_inputs, inspect_compile_constants,
    lower_graphs_for_inspection_with_options, lower_graphs_for_inspection_with_options_and_inputs,
};
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
    pub(crate) state_integer_ranges: HashMap<String, TypedIntegerRange>,
    pub(crate) pinned_state_roots: HashSet<String>,
    pub(crate) compiler_owned_state_roots: HashSet<String>,
    pub(crate) compiler_scratch_state_roots: HashSet<String>,
    pub in_defaults: HashMap<String, TypedConstValue>,
    pub in_ranges: HashMap<String, TypedValueRange>,
    pub(crate) dynamic_input_range_aliases: HashMap<String, String>,
    pub(crate) dynamic_param_range_aliases: HashMap<String, String>,
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
    pub delegates: Vec<TypedDelegate>,
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

#[derive(Debug, Clone)]
pub struct TypedDelegate {
    pub name: String,
    pub params: Vec<TypedEventParam>,
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

/// Immutable host-selected values for one semantic analysis request.
///
/// Each key must name an executable-root `config const` declaration, and each
/// value must exactly match its declared scalar or array element type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompileInputs {
    pub constants: BTreeMap<String, ConstValue>,
}

/// Resolves host-readable Onda literals against the declared `config const`
/// types. Callers may merge the resulting maps in precedence order before
/// semantic analysis.
pub fn compile_inputs_from_literals(
    program: &Program,
    literals: impl IntoIterator<Item = (String, String)>,
    options: AnalysisOptions,
) -> Result<CompileInputs, Vec<Diagnostic>> {
    let mut inputs = CompileInputs::default();
    for (name, raw_value) in literals {
        let decl = program.blocks.iter().find_map(|block| match block {
            Block::Const(decl) if decl.name == name => Some(decl),
            _ => None,
        });
        let Some(decl) = decl else {
            return Err(vec![Diagnostic::semantic(
                format!("unknown configuration constant '{name}'"),
                0,
                0,
            )]);
        };
        if !decl.configurable {
            return Err(vec![Diagnostic::semantic_span(
                format!(
                    "constant '{name}' is not host-configurable; declare it with 'config const'"
                ),
                decl.loc.as_ref(),
            )]);
        }
        let Some(declared_ty) = decl.ty.as_ref() else {
            return Err(vec![Diagnostic::semantic_span(
                format!("configuration constant '{name}' requires an explicit type"),
                decl.loc.as_ref(),
            )]);
        };
        let value = compile_input_from_literal(&name, &raw_value, declared_ty, decl, options)?;
        inputs.constants.insert(name, value);
    }
    Ok(inputs)
}

fn compile_input_from_literal(
    name: &str,
    raw_value: &str,
    declared_ty: &ConstType,
    decl: &ConstDecl,
    options: AnalysisOptions,
) -> Result<ConstValue, Vec<Diagnostic>> {
    let source = format!("const OndaHostValue = {raw_value}\n");
    let literal_program = onda_frontend::parse_program(&source)?;
    let mut blocks = literal_program.blocks.into_iter();
    let expr = match (blocks.next(), blocks.next()) {
        (Some(Block::Const(value)), None) => value.expr,
        _ => {
            return Err(vec![Diagnostic::semantic_span(
                format!(
                    "invalid configuration value for '{name}': expected exactly one expression"
                ),
                decl.loc.as_ref(),
            )]);
        }
    };
    let ty = match declared_ty {
        ConstType::Array { elem, .. } => ConstType::Slice { elem: *elem },
        other => other.clone(),
    };
    let synthetic = Program {
        blocks: vec![Block::Const(ConstDecl {
            loc: decl.loc,
            name: name.to_owned(),
            ty: Some(ty),
            expr,
            configurable: true,
        })],
    };
    let mut descriptors = inspect_compile_constants(synthetic, options, &CompileInputs::default())?;
    descriptors
        .pop()
        .map(|descriptor| descriptor.value)
        .ok_or_else(|| {
            vec![Diagnostic::internal(format!(
                "failed to resolve configuration value for '{name}'"
            ))]
        })
}

/// The resolved source shape of one host-configurable constant.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CompileConstKind {
    Scalar,
    FixedArray,
    Array,
}

/// A configuration declaration resolved under one complete compile input map.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileConstDescriptor {
    pub name: String,
    pub kind: CompileConstKind,
    pub value: ConstValue,
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
    /// Number of individually bindable resources in this declaration.
    pub array_len: usize,
    /// Whether the declaration used the fixed resource-array form, including `[1]`.
    pub is_array: bool,
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
    pub(crate) integer_range: Option<TypedIntegerRange>,
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

pub(crate) fn is_bare_return_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::UserCall { name, type_args, args, .. }
            if name == INTERNAL_BARE_RETURN_FN && type_args.is_empty() && args.is_empty()
    )
}

pub(crate) fn zero_expr(ty: PrimitiveType) -> Expr {
    match ty {
        PrimitiveType::F32 | PrimitiveType::F64 => Expr::number(0.0),
        PrimitiveType::I32 | PrimitiveType::I64 => Expr::int(0),
        PrimitiveType::Bool => Expr::bool(false),
    }
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
    /// Compiler-owned helpers that execute with access to the program's
    /// runtime state and interface rather than in the lexical `def` scope.
    pub(crate) runtime_context: bool,
    /// Authored lexical functions whose body directly publishes print output.
    pub(crate) publishes_print: bool,
    pub method_of: Option<String>,
    pub type_params: Vec<String>,
    pub params: Vec<String>,
    pub param_defaults: Vec<Option<Expr>>,
    pub param_kinds: Vec<TypedFnParam>,
    /// Primitive array parameters that semantic analysis proved are not
    /// mutated directly or through calls. MIR uses this to choose the slice
    /// access contract instead of rediscovering mutability from source AST.
    pub readonly_array_params: HashSet<String>,
    /// Integer range contracts for concrete flattened reference parameters.
    /// The function boundary supplies the binding identity that source names
    /// alone cannot provide.
    pub(crate) integer_range_params: HashMap<String, TypedIntegerRange>,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct TypedIntegerRange {
    pub(crate) ty: PrimitiveType,
    pub(crate) min: i64,
    pub(crate) max: i64,
    pub(crate) wrap: bool,
}

pub(crate) fn typed_integer_range_from_expr(
    expr: &Expr,
    declared_ty: Option<PrimitiveType>,
) -> Option<TypedIntegerRange> {
    let Expr::Call { func, args, .. } = expr else {
        return None;
    };
    let wrap = match func {
        BuiltinFn::RangeClamp => false,
        BuiltinFn::RangeWrap => true,
        _ => return None,
    };
    let [_, Expr::Int { value: min, .. }, Expr::Int { value: max, .. }] = args.as_slice() else {
        return None;
    };
    let ty @ (PrimitiveType::I32 | PrimitiveType::I64) = declared_ty? else {
        return None;
    };
    Some(TypedIntegerRange {
        ty,
        min: *min,
        max: *max,
        wrap,
    })
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
    BufferArray {
        elem_ty: PrimitiveType,
        channels: TypedBufferChannels,
        len: usize,
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
    pub curve: Option<f64>,
    pub unit: Option<String>,
    pub step: Option<TypedConstValue>,
    /// Number of equal intervals between the inclusive range endpoints.
    pub step_count: Option<u32>,
}

impl Default for TypedParamControl {
    fn default() -> Self {
        Self {
            scale: ParamScale::Linear,
            curve: None,
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
    pub const fn primitive_type(self) -> PrimitiveType {
        match self {
            Self::F32(_) => PrimitiveType::F32,
            Self::F64(_) => PrimitiveType::F64,
            Self::I32(_) => PrimitiveType::I32,
            Self::I64(_) => PrimitiveType::I64,
            Self::Bool(_) => PrimitiveType::Bool,
        }
    }

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
    /// Physical flattening extent used by aggregate lowering. Unsized
    /// function parameters use one element as their local layout unit.
    pub(crate) len: usize,
    /// Source-visible fixed length, when the root has one. This stays `None`
    /// for `Struct[]` parameters so call validation never mistakes the local
    /// layout unit for a declared one-element contract.
    pub(crate) static_len: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalArrayAliasInfo {
    /// Physical extent or conservative layout hint used by semantic lowering.
    /// This is not necessarily a source-level fixed-length guarantee: slices
    /// and unsized function parameters still need a non-zero local extent.
    pub(crate) len: usize,
    /// Source-visible fixed length, when the binding has one. Keeping this
    /// separate from `len` prevents lowering placeholders from satisfying a
    /// fixed-array call contract.
    pub(crate) static_len: Option<usize>,
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
#[path = "tests/mod.rs"]
mod tests;
