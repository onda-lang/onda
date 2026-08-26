use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub blocks: Vec<Block>,
}

impl Program {
    pub fn block(&self, kind: BlockKind) -> Option<&Block> {
        self.blocks.iter().find(|b| b.kind() == kind)
    }
}

#[derive(Debug, Clone, PartialEq)]
// This public, mutation-heavy AST intentionally keeps declaration payloads inline. Boxing only the
// processor variant would add allocation and dereference churn throughout parser/semantic passes
// and would be a breaking representation change; executable size is normalized at the MIR boundary.
#[allow(clippy::large_enum_variant)]
pub enum Block {
    Ins(PortBlock),
    Outs(PortBlock),
    KOuts(PortBlock),
    Params(ParamBlock),
    Const(ConstDecl),
    Events(EventBlock),
    Delegates(DelegateBlock),
    When(WhenDef),
    Tasks(TaskBlock),
    Buffers(BufferBlock),
    Assert(AssertDecl),
    Namespace(NamespaceDecl),
    NamespaceAlias(NamespaceAliasDecl),
    Use(UseDecl),
    Proc(ProcessorDef),
    Struct(StructDef),
    Def(FunctionDef),
    Init(InitBlock),
    Block(BlockExec),
    Sample(SampleBlock),
    Graph(GraphBlock),
}

impl Block {
    pub fn kind(&self) -> BlockKind {
        match self {
            Self::Ins(_) => BlockKind::Ins,
            Self::Outs(_) => BlockKind::Outs,
            Self::KOuts(_) => BlockKind::KOuts,
            Self::Params(_) => BlockKind::Params,
            Self::Const(_) => BlockKind::Const,
            Self::Events(_) => BlockKind::Events,
            Self::Delegates(_) => BlockKind::Delegates,
            Self::When(_) => BlockKind::When,
            Self::Tasks(_) => BlockKind::Tasks,
            Self::Buffers(_) => BlockKind::Buffers,
            Self::Assert(_) => BlockKind::Assert,
            Self::Namespace(_) => BlockKind::Namespace,
            Self::NamespaceAlias(_) => BlockKind::NamespaceAlias,
            Self::Use(_) => BlockKind::Use,
            Self::Proc(_) => BlockKind::Proc,
            Self::Struct(_) => BlockKind::Struct,
            Self::Def(_) => BlockKind::Def,
            Self::Init(_) => BlockKind::Init,
            Self::Block(_) => BlockKind::Block,
            Self::Sample(_) => BlockKind::Sample,
            Self::Graph(_) => BlockKind::Graph,
        }
    }

    pub fn loc(&self) -> SourceLoc {
        match self {
            Self::Ins(ports) | Self::Outs(ports) | Self::KOuts(ports) => ports.loc.into(),
            Self::Params(params) => params.loc.into(),
            Self::Const(decl) => decl.loc.into(),
            Self::Events(events) => events.loc.into(),
            Self::Delegates(delegates) => delegates.loc.into(),
            Self::When(when) => when.loc.into(),
            Self::Tasks(tasks) => tasks.loc.into(),
            Self::Buffers(buffers) => buffers.loc.into(),
            Self::Assert(assert_decl) => assert_decl.loc.into(),
            Self::Namespace(namespace) => namespace.loc.into(),
            Self::NamespaceAlias(alias) => alias.loc.into(),
            Self::Use(use_decl) => use_decl.loc.into(),
            Self::Proc(proc_def) => proc_def.loc.into(),
            Self::Struct(struct_def) => struct_def.loc.into(),
            Self::Def(def) => def.loc.into(),
            Self::Init(init) => init.loc.into(),
            Self::Block(exec) => exec.loc.into(),
            Self::Sample(sample) => sample.loc.into(),
            Self::Graph(graph) => graph.loc.into(),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum BlockKind {
    Ins,
    Outs,
    KOuts,
    Params,
    Const,
    Events,
    Delegates,
    When,
    Tasks,
    Buffers,
    Assert,
    Namespace,
    NamespaceAlias,
    Use,
    Proc,
    Struct,
    Def,
    Init,
    Block,
    Sample,
    Graph,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortBlock {
    pub loc: Span,
    pub decls: Vec<PortDecl>,
    pub deferred_count: Option<Expr>,
    pub deferred_default_ty: Option<DeclType>,
    pub deferred_prefix: String,
    pub output_timing: OutputTiming,
    pub output_timing_loc: Span,
}

impl Deref for PortBlock {
    type Target = Vec<PortDecl>;

    fn deref(&self) -> &Self::Target {
        &self.decls
    }
}

impl DerefMut for PortBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.decls
    }
}

impl<'a> IntoIterator for &'a PortBlock {
    type Item = &'a PortDecl;
    type IntoIter = std::slice::Iter<'a, PortDecl>;

    fn into_iter(self) -> Self::IntoIter {
        self.decls.iter()
    }
}

impl<'a> IntoIterator for &'a mut PortBlock {
    type Item = &'a mut PortDecl;
    type IntoIter = std::slice::IterMut<'a, PortDecl>;

    fn into_iter(self) -> Self::IntoIter {
        self.decls.iter_mut()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamBlock {
    pub loc: Span,
    pub decls: Vec<ParamDecl>,
    pub deferred_count: Option<Expr>,
    pub deferred_default_ty: Option<DeclType>,
    pub deferred_prefix: String,
}

impl Deref for ParamBlock {
    type Target = Vec<ParamDecl>;

    fn deref(&self) -> &Self::Target {
        &self.decls
    }
}

impl DerefMut for ParamBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.decls
    }
}

impl<'a> IntoIterator for &'a ParamBlock {
    type Item = &'a ParamDecl;
    type IntoIter = std::slice::Iter<'a, ParamDecl>;

    fn into_iter(self) -> Self::IntoIter {
        self.decls.iter()
    }
}

impl<'a> IntoIterator for &'a mut ParamBlock {
    type Item = &'a mut ParamDecl;
    type IntoIter = std::slice::IterMut<'a, ParamDecl>;

    fn into_iter(self) -> Self::IntoIter {
        self.decls.iter_mut()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventBlock {
    pub loc: Span,
    pub events: Vec<EventDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DelegateBlock {
    pub loc: Span,
    pub delegates: Vec<DelegateDef>,
}

impl Deref for DelegateBlock {
    type Target = Vec<DelegateDef>;

    fn deref(&self) -> &Self::Target {
        &self.delegates
    }
}

impl DerefMut for DelegateBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.delegates
    }
}

impl<'a> IntoIterator for &'a DelegateBlock {
    type Item = &'a DelegateDef;
    type IntoIter = std::slice::Iter<'a, DelegateDef>;

    fn into_iter(self) -> Self::IntoIter {
        self.delegates.iter()
    }
}

impl Deref for EventBlock {
    type Target = Vec<EventDef>;

    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl DerefMut for EventBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.events
    }
}

impl<'a> IntoIterator for &'a EventBlock {
    type Item = &'a EventDef;
    type IntoIter = std::slice::Iter<'a, EventDef>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}

impl<'a> IntoIterator for &'a mut EventBlock {
    type Item = &'a mut EventDef;
    type IntoIter = std::slice::IterMut<'a, EventDef>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter_mut()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BufferBlock {
    pub loc: Span,
    pub decls: Vec<BufferDecl>,
    pub deferred_count: Option<Expr>,
    pub deferred_default_ty: Option<BufferType>,
}

impl Deref for BufferBlock {
    type Target = Vec<BufferDecl>;

    fn deref(&self) -> &Self::Target {
        &self.decls
    }
}

impl DerefMut for BufferBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.decls
    }
}

impl<'a> IntoIterator for &'a BufferBlock {
    type Item = &'a BufferDecl;
    type IntoIter = std::slice::Iter<'a, BufferDecl>;

    fn into_iter(self) -> Self::IntoIter {
        self.decls.iter()
    }
}

impl<'a> IntoIterator for &'a mut BufferBlock {
    type Item = &'a mut BufferDecl;
    type IntoIter = std::slice::IterMut<'a, BufferDecl>;

    fn into_iter(self) -> Self::IntoIter {
        self.decls.iter_mut()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockExec {
    pub loc: Span,
    pub pre: Vec<Stmt>,
    pub sample: Option<SampleBlock>,
    pub post: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SampleBlock {
    pub loc: Span,
    pub oversample_factor: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphBlock {
    pub loc: Span,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub loc: Span,
    pub rate: Option<GraphRate>,
    pub source: Expr,
    pub delay: Option<Expr>,
    pub dests: Vec<GraphEndpoint>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum GraphRate {
    Block,
    Sample,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphEndpoint {
    Symbol {
        loc: Span,
        name: String,
    },
    ProcField {
        loc: Span,
        proc: String,
        field: String,
    },
    ProcIndexedField {
        loc: Span,
        proc: String,
        index: Expr,
        field: String,
    },
}

impl Deref for SampleBlock {
    type Target = Vec<Stmt>;

    fn deref(&self) -> &Self::Target {
        &self.body
    }
}

impl DerefMut for SampleBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitBlock {
    pub loc: Span,
    pub default_ty: Option<DeclType>,
    pub default_ty_loc: Span,
    pub pinned_roots: Vec<String>,
    /// Compiler-generated roots that are live only within one entry-point
    /// activation and must not participate in portable snapshots.
    pub compiler_scratch_roots: Vec<String>,
    pub body: Vec<Stmt>,
}

impl Deref for InitBlock {
    type Target = Vec<Stmt>;

    fn deref(&self) -> &Self::Target {
        &self.body
    }
}

impl DerefMut for InitBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.body
    }
}

impl IntoIterator for InitBlock {
    type Item = Stmt;
    type IntoIter = std::vec::IntoIter<Stmt>;

    fn into_iter(self) -> Self::IntoIter {
        self.body.into_iter()
    }
}

impl<'a> IntoIterator for &'a InitBlock {
    type Item = &'a Stmt;
    type IntoIter = std::slice::Iter<'a, Stmt>;

    fn into_iter(self) -> Self::IntoIter {
        self.body.iter()
    }
}

impl<'a> IntoIterator for &'a mut InitBlock {
    type Item = &'a mut Stmt;
    type IntoIter = std::slice::IterMut<'a, Stmt>;

    fn into_iter(self) -> Self::IntoIter {
        self.body.iter_mut()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortDecl {
    pub loc: Span,
    pub name: String,
    pub output_timing: Option<OutputTiming>,
    pub output_timing_loc: Span,
    pub ty: Option<DeclType>,
    pub ty_loc: Span,
    pub default: Option<Expr>,
    pub range: Option<DeclRange>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum OutputTiming {
    Sample,
    Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    pub loc: Span,
    pub name: String,
    pub private: bool,
    pub ty: Option<DeclType>,
    pub ty_loc: Span,
    pub default: Option<Expr>,
    pub range: Option<DeclRange>,
    pub control: ParamControl,
    pub bind: Option<String>,
}

#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, Hash)]
pub enum ParamScale {
    #[default]
    Linear,
    Log,
}

pub const PARAM_SCALES: &[(ParamScale, &str)] =
    &[(ParamScale::Linear, "linear"), (ParamScale::Log, "log")];
pub const PARAM_DOMAIN_FIELDS: &[&str] = &["min", "max", "scale", "curve", "unit", "step"];
pub const PARAM_DOMAIN_POSITIONAL_FIELDS: &[&str] = &["min", "max", "scale", "unit", "step"];

impl ParamScale {
    pub fn from_name(name: &str) -> Option<Self> {
        PARAM_SCALES
            .iter()
            .find_map(|(scale, scale_name)| (*scale_name == name).then_some(*scale))
    }

    pub fn name(self) -> &'static str {
        PARAM_SCALES
            .iter()
            .find_map(|(scale, name)| (*scale == self).then_some(*name))
            .expect("PARAM_SCALES must contain every ParamScale variant")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamControl {
    pub scale: ParamScale,
    pub curve: Option<Expr>,
    pub unit: Option<String>,
    pub step: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclRange {
    pub min: Option<Expr>,
    pub max: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BufferDecl {
    pub loc: Span,
    pub name: String,
    pub ty: Option<BufferType>,
    pub ty_loc: Span,
    /// Compile-time number of external buffer resources in this declaration.
    /// `None` denotes one ordinary buffer.
    pub array_size: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertDecl {
    pub loc: Span,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceDecl {
    pub loc: Span,
    pub name: String,
    pub params: Vec<NamespaceTemplateParam>,
    pub items: Vec<NamespaceItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceTemplateParam {
    pub name: String,
    pub default: Expr,
}

#[derive(Debug, Clone, PartialEq)]
// Keep namespace declarations representation-compatible with top-level `Block` declarations.
#[allow(clippy::large_enum_variant)]
pub enum NamespaceItem {
    Assert(AssertDecl),
    Const(ConstDecl),
    Struct(StructDef),
    Def(FunctionDef),
    Proc(ProcessorDef),
    Namespace(NamespaceDecl),
    Alias(NamespaceAliasDecl),
    Use(UseDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceAliasDecl {
    pub loc: Span,
    pub name: String,
    pub target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    pub loc: Span,
    pub target: Vec<NamespaceRefSegment>,
    pub alias: Option<String>,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceRefSegment {
    pub name: String,
    pub args: Option<Vec<NamespaceCallArg>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceCallArg {
    pub name: Option<String>,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub loc: Span,
    pub name: String,
    pub ty: Option<ConstType>,
    pub expr: Expr,
    /// True only for an executable-root declaration written as `config const`.
    pub configurable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorDef {
    pub loc: Span,
    pub name: String,
    pub type_params: Vec<String>,
    pub consts: Vec<ConstDecl>,
    pub ins: Vec<PortDecl>,
    pub ins_deferred_count: Option<Expr>,
    pub ins_deferred_default_ty: Option<DeclType>,
    pub outs: Vec<PortDecl>,
    pub outs_deferred_count: Option<Expr>,
    pub outs_deferred_default_ty: Option<DeclType>,
    pub outs_timing: OutputTiming,
    pub outs_timing_loc: Span,
    pub params: Vec<ParamDecl>,
    pub params_deferred_count: Option<Expr>,
    pub params_deferred_default_ty: Option<DeclType>,
    pub events: Vec<EventDef>,
    pub delegates: Vec<DelegateDef>,
    pub whens: Vec<WhenDef>,
    pub tasks: Vec<TaskDef>,
    pub buffers: Vec<BufferDecl>,
    pub buffers_deferred_count: Option<Expr>,
    pub buffers_deferred_default_ty: Option<BufferType>,
    pub has_init_block: bool,
    pub has_block_block: bool,
    pub has_sample_block: bool,
    pub has_graph_block: bool,
    pub sample_oversample_factor: Option<Expr>,
    pub init: InitBlock,
    pub block_pre: Vec<Stmt>,
    pub sample: Vec<Stmt>,
    pub block_post: Vec<Stmt>,
    pub graph: Option<GraphBlock>,
    pub local_defs: Vec<FunctionDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskDef {
    pub loc: Span,
    pub name: String,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskBlock {
    pub loc: Span,
    pub tasks: Vec<TaskDef>,
}

impl Deref for TaskBlock {
    type Target = Vec<TaskDef>;

    fn deref(&self) -> &Self::Target {
        &self.tasks
    }
}

impl DerefMut for TaskBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tasks
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum PrimitiveType {
    F32,
    F64,
    I32,
    I64,
    Bool,
}

impl PrimitiveType {
    pub const ALL: [Self; 5] = [Self::F32, Self::F64, Self::I32, Self::I64, Self::Bool];

    pub fn name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Bool => "bool",
        }
    }

    pub fn from_name(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ty| ty.name() == text)
    }

    pub fn is_name(text: &str) -> bool {
        Self::from_name(text).is_some()
    }

    pub fn is_numeric(self) -> bool {
        !matches!(self, Self::Bool)
    }
}

pub const INTERNAL_BUFFER_READ2_FN: &str = "__onda_buffer_read2";
pub const INTERNAL_BUFFER_WRITE2_FN: &str = "__onda_buffer_write2";
pub const INTERNAL_BUFFER_READ_CHANNEL_FN: &str = "__onda_buffer_read_channel";
pub const INTERNAL_BUFFER_WRITE_CHANNEL_FN: &str = "__onda_buffer_write_channel";
pub const INTERNAL_TASK_AWAIT_FN: &str = "__onda_task_await";
pub const INTERNAL_TASK_YIELD_FN: &str = "__onda_task_yield";
pub const INTERNAL_BARE_RETURN_FN: &str = "__onda_bare_return";
pub const INTERNAL_BUFFER_READ3_FN: &str = "__onda_buffer_read3";
pub const INTERNAL_BUFFER_WRITE3_FN: &str = "__onda_buffer_write3";
pub const READ_UNSAFE_FN: &str = "read_unsafe";
pub const WRITE_UNSAFE_FN: &str = "write_unsafe";
/// Preserves the receiver in `value.method(...)` until semantic resolution.
pub const METHOD_RECEIVER_ARG: &str = "__onda_method_receiver";

#[derive(Debug, Clone, PartialEq)]
pub enum ConstType {
    Scalar(PrimitiveType),
    Array { elem: PrimitiveType, size: Expr },
    Slice { elem: PrimitiveType },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclType {
    Scalar(PrimitiveType),
    Generic(String),
    ArrayGeneric { elem: String, size: Expr },
    Array { elem: PrimitiveType, size: Expr },
    Tuple(Vec<PrimitiveType>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FnParamType {
    Primitive(PrimitiveType),
    Struct(String),
    Buffer(BufferType),
    /// Compiler-generated fixed collection of buffer references. Source
    /// `buffers` declarations use `{count}`; this type carries that shape
    /// through generated processor functions without exposing an aggregate
    /// runtime value.
    BufferArray {
        buffer: BufferType,
        len: usize,
    },
    Array(Option<PrimitiveType>),
    ArrayGeneric(String),
    /// Sized array param: `f32[4]` → `SizedArray(Some(F32), expr)`,
    /// `T[4]` → `SizedArray(None, expr)` with generic_name set.
    SizedArray {
        elem: Option<PrimitiveType>,
        generic_name: Option<String>,
        size: Expr,
    },
    BareBuffer,
    Tuple(Vec<PrimitiveType>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FnReturnScalarType {
    Primitive(PrimitiveType),
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FnReturnType {
    Scalar(FnReturnScalarType),
    Array { elem: PrimitiveType, size: Expr },
    Tuple(Vec<FnReturnScalarType>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BufferElemType {
    Primitive(PrimitiveType),
    Generic(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BufferChannels {
    Mono,
    Static(Expr),
    Dynamic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BufferType {
    pub elem: BufferElemType,
    pub channels: BufferChannels,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnParamDecl {
    pub loc: Span,
    pub name: String,
    pub ty: Option<FnParamType>,
    pub ty_loc: Span,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventDef {
    pub loc: Span,
    pub name: String,
    pub params: Vec<EventParamDecl>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DelegateDef {
    pub loc: Span,
    pub name: String,
    pub params: Vec<EventParamDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhenDef {
    pub loc: Span,
    pub target: WhenTarget,
    pub bindings: Vec<WhenBinding>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhenTarget {
    pub loc: Span,
    /// Dot-separated receiver path preceding the delegate name. Empty for an
    /// owner-local delegate. Semantic analysis enforces the one-child boundary.
    pub receiver: Vec<String>,
    /// Present only for `child[constant].delegate`.
    pub index: Option<Expr>,
    pub delegate: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhenBinding {
    pub loc: Span,
    /// `_` is retained explicitly so diagnostics and formatting preserve the
    /// source binding position without introducing a symbol.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventParamDecl {
    pub loc: Span,
    pub name: String,
    pub ty: EventParamType,
    pub ty_loc: Span,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventParamType {
    Scalar(PrimitiveType),
    GenericScalar { name: String },
    Array { elem: PrimitiveType, size: Expr },
    GenericArray { elem: String, size: Expr },
    Slice { elem: PrimitiveType },
    GenericSlice { elem: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub loc: Span,
    pub name: String,
    pub is_const: bool,
    pub type_params: Vec<String>,
    pub params: Vec<FnParamDecl>,
    pub return_ty: Option<FnReturnType>,
    pub return_ty_loc: Span,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub loc: Span,
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<StructField>,
    pub methods: Vec<FunctionDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub loc: Span,
    pub name: String,
    pub ty: FieldType,
    pub ty_loc: Span,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Scalar(PrimitiveType),
    Generic(String),
    Array(ArrayTypeSpec),
    Tuple(Vec<PrimitiveType>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElemType {
    Primitive(PrimitiveType),
    Struct(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArrayTypeSpec {
    pub elem: ArrayElemType,
    pub size: Box<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    Var(String),
    Index {
        base: String,
        index: Expr,
    },
    Slice {
        base: String,
        selector: Option<Box<Expr>>,
        channel: Option<Box<Expr>>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
    },
    Tuple(Vec<String>),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub struct Span {
    pub line: u32,
    pub column: u16,
    pub end_line_delta: u16,
    pub end_column: u16,
    file_id: u16,
    trace_id: u16,
}

impl Span {
    pub const ZERO: Span = Span {
        line: 0,
        column: 0,
        end_line_delta: 0,
        end_column: 0,
        file_id: 0,
        trace_id: 0,
    };

    pub fn new(line: usize, column: usize, end_line: usize, end_column: usize) -> Self {
        Self::new_with_context(line, column, end_line, end_column, 0, 0)
    }

    fn new_with_context(
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
        file_id: u16,
        trace_id: u16,
    ) -> Self {
        let line32 = line.min(u32::MAX as usize) as u32;
        let delta = (end_line as u32).saturating_sub(line32);
        let column16 = column.min(u16::MAX as usize) as u16;
        let end_column16 = if line == 0 {
            0
        } else if end_line <= line {
            end_column
                .max(column.saturating_add(1))
                .min(u16::MAX as usize) as u16
        } else {
            end_column.max(1).min(u16::MAX as usize) as u16
        };
        Self {
            line: line32,
            column: column16,
            end_line_delta: delta.min(u16::MAX as u32) as u16,
            end_column: end_column16,
            file_id,
            trace_id,
        }
    }

    pub fn end_line(&self) -> u32 {
        self.line + self.end_line_delta as u32
    }

    pub fn file(&self) -> Option<String> {
        SourceLoc::from(*self).file()
    }

    pub fn trace(&self) -> Vec<String> {
        SourceLoc::from(*self).trace()
    }

    pub fn is_zero(&self) -> bool {
        self.line == 0
    }

    pub fn as_ref(&self) -> Option<&Span> {
        if self.is_zero() {
            None
        } else {
            Some(self)
        }
    }

    pub fn cloned(self) -> Option<Span> {
        if self.is_zero() {
            None
        } else {
            Some(self)
        }
    }

    pub fn expect(self, message: &str) -> Span {
        assert!(!self.is_zero(), "{message}");
        self
    }

    pub fn map<T>(self, f: impl FnOnce(&Span) -> T) -> Option<T> {
        self.as_ref().map(f)
    }

    pub fn or(self, fallback: impl Into<Span>) -> Span {
        if self.is_zero() {
            fallback.into()
        } else {
            self
        }
    }

    pub fn spanning(a: Span, b: Span) -> Span {
        if a.is_zero() {
            return b;
        }
        if b.is_zero() {
            return a;
        }
        let end = b.end_line().max(a.end_line());
        let end_column = if b.end_line() > a.end_line() {
            b.end_column
        } else if a.end_line() > b.end_line() {
            a.end_column
        } else {
            a.end_column.max(b.end_column)
        };
        Span::new_with_context(
            a.line as usize,
            a.column as usize,
            end as usize,
            end_column as usize,
            if a.file_id != 0 { a.file_id } else { b.file_id },
            if a.trace_id != 0 {
                a.trace_id
            } else {
                b.trace_id
            },
        )
    }
}

impl From<&SourceLoc> for Span {
    fn from(loc: &SourceLoc) -> Self {
        Span::new_with_context(
            loc.line,
            loc.column,
            loc.end_line,
            loc.end_column,
            loc.file_id,
            loc.trace_id,
        )
    }
}

impl From<SourceLoc> for Span {
    fn from(loc: SourceLoc) -> Self {
        loc.span()
    }
}

impl From<Option<&SourceLoc>> for Span {
    fn from(loc: Option<&SourceLoc>) -> Self {
        loc.map(Span::from).unwrap_or_default()
    }
}

impl From<Option<SourceLoc>> for Span {
    fn from(loc: Option<SourceLoc>) -> Self {
        loc.map(Span::from).unwrap_or_default()
    }
}

impl From<Option<&Span>> for Span {
    fn from(span: Option<&Span>) -> Self {
        span.copied().unwrap_or_default()
    }
}

impl From<Option<Span>> for Span {
    fn from(span: Option<Span>) -> Self {
        span.unwrap_or_default()
    }
}

impl From<Span> for SourceLoc {
    fn from(span: Span) -> Self {
        if span.is_zero() {
            return SourceLoc::ZERO;
        }
        SourceLoc {
            line: span.line as usize,
            column: span.column as usize,
            end_line: span.end_line() as usize,
            end_column: span.end_column as usize,
            file_id: span.file_id,
            trace_id: span.trace_id,
        }
    }
}

impl From<&Span> for SourceLoc {
    fn from(span: &Span) -> Self {
        (*span).into()
    }
}

impl From<Option<&Span>> for SourceLoc {
    fn from(span: Option<&Span>) -> Self {
        span.copied().map(SourceLoc::from).unwrap_or_default()
    }
}

impl From<Option<Span>> for SourceLoc {
    fn from(span: Option<Span>) -> Self {
        span.map(SourceLoc::from).unwrap_or_default()
    }
}

impl From<&SourceLoc> for SourceLoc {
    fn from(loc: &SourceLoc) -> Self {
        *loc
    }
}

impl From<Option<&SourceLoc>> for SourceLoc {
    fn from(loc: Option<&SourceLoc>) -> Self {
        loc.copied().unwrap_or_default()
    }
}

impl From<Option<SourceLoc>> for SourceLoc {
    fn from(loc: Option<SourceLoc>) -> Self {
        loc.unwrap_or_default()
    }
}

#[derive(Debug, Default)]
struct SourceContextInterner {
    files: Vec<String>,
    file_ids: HashMap<String, u16>,
    traces: Vec<Vec<String>>,
    trace_ids: HashMap<Vec<String>, u16>,
}

impl SourceContextInterner {
    fn intern_file(&mut self, file: Option<&str>) -> u16 {
        let Some(file) = file else {
            return 0;
        };
        if let Some(id) = self.file_ids.get(file) {
            return *id;
        }
        let Ok(id) = u16::try_from(self.files.len() + 1) else {
            return u16::MAX;
        };
        let owned = file.to_owned();
        self.files.push(owned.clone());
        self.file_ids.insert(owned, id);
        id
    }

    fn intern_trace(&mut self, trace: &[String]) -> u16 {
        if trace.is_empty() {
            return 0;
        }
        if let Some(id) = self.trace_ids.get(trace) {
            return *id;
        }
        let Ok(id) = u16::try_from(self.traces.len() + 1) else {
            return u16::MAX;
        };
        let owned = trace.to_vec();
        self.traces.push(owned.clone());
        self.trace_ids.insert(owned, id);
        id
    }

    fn file(&self, file_id: u16) -> Option<String> {
        if file_id == 0 {
            return None;
        }
        self.files.get(file_id as usize - 1).cloned()
    }

    fn trace(&self, trace_id: u16) -> Vec<String> {
        if trace_id == 0 {
            return Vec::new();
        }
        self.traces
            .get(trace_id as usize - 1)
            .cloned()
            .unwrap_or_default()
    }
}

thread_local! {
    static SOURCE_CONTEXT_INTERNER: RefCell<SourceContextInterner> =
        RefCell::new(SourceContextInterner::default());
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct SourceLoc {
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    file_id: u16,
    trace_id: u16,
}

impl SourceLoc {
    pub const ZERO: SourceLoc = SourceLoc {
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        file_id: 0,
        trace_id: 0,
    };

    pub fn new(
        file: Option<String>,
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
        trace: Vec<String>,
    ) -> Self {
        let (file_id, trace_id) = SOURCE_CONTEXT_INTERNER.with(|interner| {
            let mut interner = interner.borrow_mut();
            (
                interner.intern_file(file.as_deref()),
                interner.intern_trace(&trace),
            )
        });
        let end_column = if line == 0 {
            0
        } else if end_line <= line {
            end_column.max(column.saturating_add(1))
        } else {
            end_column.max(1)
        };
        Self {
            line,
            column,
            end_line,
            end_column,
            file_id,
            trace_id,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.line == 0
    }

    pub fn as_ref(&self) -> Option<&SourceLoc> {
        if self.is_zero() {
            None
        } else {
            Some(self)
        }
    }

    pub fn cloned(self) -> Option<SourceLoc> {
        if self.is_zero() {
            None
        } else {
            Some(self)
        }
    }

    pub fn expect(self, message: &str) -> SourceLoc {
        assert!(!self.is_zero(), "{message}");
        self
    }

    pub fn map<T>(self, f: impl FnOnce(&SourceLoc) -> T) -> Option<T> {
        self.as_ref().map(f)
    }

    pub fn or(self, fallback: impl Into<SourceLoc>) -> SourceLoc {
        if self.is_zero() {
            fallback.into()
        } else {
            self
        }
    }

    pub fn span(&self) -> Span {
        Span::new_with_context(
            self.line,
            self.column,
            self.end_line,
            self.end_column,
            self.file_id,
            self.trace_id,
        )
    }

    pub fn file(&self) -> Option<String> {
        SOURCE_CONTEXT_INTERNER.with(|interner| interner.borrow().file(self.file_id))
    }

    pub fn trace(&self) -> Vec<String> {
        SOURCE_CONTEXT_INTERNER.with(|interner| interner.borrow().trace(self.trace_id))
    }

    pub fn spanning(start: impl Into<SourceLoc>, end: impl Into<SourceLoc>) -> Self {
        let start = start.into();
        let end = end.into();
        if start.is_zero() {
            return end;
        }
        if end.is_zero() {
            return start;
        }
        Self {
            line: start.line,
            column: start.column,
            end_line: end.end_line,
            end_column: end.end_column,
            file_id: if start.file_id != 0 {
                start.file_id
            } else {
                end.file_id
            },
            trace_id: if start.trace_id != 0 {
                start.trace_id
            } else {
                end.trace_id
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Const {
        loc: Span,
        decl: ConstDecl,
    },
    Assign {
        loc: Span,
        target_loc: Span,
        target: AssignTarget,
        decl_ty: Option<PrimitiveType>,
        generic_decl_ty: Option<String>,
        is_typed_decl: bool,
        typed_decl_ty_loc: Span,
        expr: Expr,
    },
    Expr {
        loc: Span,
        expr: Expr,
    },
    Return {
        loc: Span,
        expr: Expr,
    },
    If {
        loc: Span,
        cond: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Vec<Stmt>,
    },
    For {
        loc: Span,
        var: String,
        var_ty: PrimitiveType,
        step: Option<Expr>,
        start: Expr,
        end: Expr,
        end_inclusive: bool,
        body: Vec<Stmt>,
    },
    While {
        loc: Span,
        cond: Expr,
        body: Vec<Stmt>,
    },
    Break {
        loc: Span,
    },
    Continue {
        loc: Span,
    },
}

impl Stmt {
    pub fn loc(&self) -> SourceLoc {
        match self {
            Self::Const { loc, .. } => (*loc).into(),
            Self::Assign { loc, .. } => (*loc).into(),
            Self::Expr { loc, .. } => (*loc).into(),
            Self::Return { loc, .. } => (*loc).into(),
            Self::If { loc, .. } => (*loc).into(),
            Self::For { loc, .. } => (*loc).into(),
            Self::While { loc, .. } => (*loc).into(),
            Self::Break { loc, .. } => (*loc).into(),
            Self::Continue { loc, .. } => (*loc).into(),
        }
    }

    pub fn assign_target_loc(&self) -> SourceLoc {
        match self {
            Self::Assign { target_loc, .. } => (*target_loc).into(),
            _ => SourceLoc::ZERO,
        }
    }

    pub fn assign_decl_type_loc(&self) -> SourceLoc {
        match self {
            Self::Assign {
                typed_decl_ty_loc, ..
            } => (*typed_decl_ty_loc).into(),
            _ => SourceLoc::ZERO,
        }
    }
}

impl GraphEdge {
    pub fn loc(&self) -> SourceLoc {
        self.loc.into()
    }
}

impl GraphEndpoint {
    pub fn loc(&self) -> SourceLoc {
        match self {
            Self::Symbol { loc, .. }
            | Self::ProcField { loc, .. }
            | Self::ProcIndexedField { loc, .. } => (*loc).into(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number {
        loc: Span,
        value: f64,
    },
    Int {
        loc: Span,
        value: i64,
    },
    Bool {
        loc: Span,
        value: bool,
    },
    ArrayLiteral {
        loc: Span,
        values: Vec<Expr>,
    },
    Var {
        loc: Span,
        name: String,
    },
    Index {
        loc: Span,
        base: String,
        index: Box<Expr>,
    },
    Slice {
        loc: Span,
        base: String,
        selector: Option<Box<Expr>>,
        channel: Option<Box<Expr>>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
    },
    ArrayCtor {
        loc: Span,
        spec: ArrayTypeSpec,
        init: Option<Vec<Expr>>,
        /// Whether evaluating this compiler-level constructor initializes the
        /// allocated storage. Source constructors always set this; lowering
        /// passes may clear it for declaration-only scratch storage.
        initialize: bool,
    },
    Compare {
        loc: Span,
        op: CmpOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        loc: Span,
        func: BuiltinFn,
        args: Vec<Expr>,
    },
    UserCall {
        loc: Span,
        name: String,
        type_args: Vec<CallTypeArg>,
        args: Vec<CallArg>,
    },
    Cast {
        loc: Span,
        to: PrimitiveType,
        expr: Box<Expr>,
    },
    UnaryNot {
        loc: Span,
        expr: Box<Expr>,
    },
    UnaryBitNot {
        loc: Span,
        expr: Box<Expr>,
    },
    Logical {
        loc: Span,
        op: LogicalOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Binary {
        loc: Span,
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Tuple {
        loc: Span,
        values: Vec<Expr>,
    },
}

impl Expr {
    pub fn loc(&self) -> SourceLoc {
        match self {
            Self::Number { loc, .. }
            | Self::Int { loc, .. }
            | Self::Bool { loc, .. }
            | Self::ArrayLiteral { loc, .. }
            | Self::Var { loc, .. }
            | Self::Index { loc, .. }
            | Self::Slice { loc, .. }
            | Self::ArrayCtor { loc, .. }
            | Self::Compare { loc, .. }
            | Self::Call { loc, .. }
            | Self::UserCall { loc, .. }
            | Self::Cast { loc, .. }
            | Self::UnaryNot { loc, .. }
            | Self::UnaryBitNot { loc, .. }
            | Self::Logical { loc, .. }
            | Self::Binary { loc, .. }
            | Self::Tuple { loc, .. } => (*loc).into(),
        }
    }

    pub fn set_loc(&mut self, new_loc: impl Into<SourceLoc>) {
        let new_loc: Span = Span::from(new_loc.into());
        match self {
            Self::Number { loc, .. }
            | Self::Int { loc, .. }
            | Self::Bool { loc, .. }
            | Self::ArrayLiteral { loc, .. }
            | Self::Var { loc, .. }
            | Self::Index { loc, .. }
            | Self::Slice { loc, .. }
            | Self::ArrayCtor { loc, .. }
            | Self::Compare { loc, .. }
            | Self::Call { loc, .. }
            | Self::UserCall { loc, .. }
            | Self::Cast { loc, .. }
            | Self::UnaryNot { loc, .. }
            | Self::UnaryBitNot { loc, .. }
            | Self::Logical { loc, .. }
            | Self::Binary { loc, .. }
            | Self::Tuple { loc, .. } => *loc = new_loc,
        }
    }

    pub fn with_loc(mut self, new_loc: impl Into<SourceLoc>) -> Self {
        self.set_loc(new_loc);
        self
    }

    pub fn with_loc_of(self, other: &Expr) -> Self {
        self.with_loc(other.loc())
    }

    pub fn number(value: f64) -> Self {
        Self::Number {
            loc: Span::ZERO,
            value,
        }
    }

    pub fn int(value: i64) -> Self {
        Self::Int {
            loc: Span::ZERO,
            value,
        }
    }

    pub fn bool(value: bool) -> Self {
        Self::Bool {
            loc: Span::ZERO,
            value,
        }
    }

    pub fn array_literal(values: Vec<Expr>) -> Self {
        Self::ArrayLiteral {
            loc: Span::ZERO,
            values,
        }
    }

    pub fn var(name: impl Into<String>) -> Self {
        Self::Var {
            loc: Span::ZERO,
            name: name.into(),
        }
    }
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number { value: lhs, .. }, Self::Number { value: rhs, .. }) => lhs == rhs,
            (Self::Int { value: lhs, .. }, Self::Int { value: rhs, .. }) => lhs == rhs,
            (Self::Bool { value: lhs, .. }, Self::Bool { value: rhs, .. }) => lhs == rhs,
            (Self::ArrayLiteral { values: lhs, .. }, Self::ArrayLiteral { values: rhs, .. }) => {
                lhs == rhs
            }
            (Self::Var { name: lhs, .. }, Self::Var { name: rhs, .. }) => lhs == rhs,
            (
                Self::Index {
                    base: lhs_base,
                    index: lhs_index,
                    ..
                },
                Self::Index {
                    base: rhs_base,
                    index: rhs_index,
                    ..
                },
            ) => lhs_base == rhs_base && lhs_index == rhs_index,
            (
                Self::Slice {
                    base: lhs_base,
                    start: lhs_start,
                    end: lhs_end,
                    ..
                },
                Self::Slice {
                    base: rhs_base,
                    start: rhs_start,
                    end: rhs_end,
                    ..
                },
            ) => lhs_base == rhs_base && lhs_start == rhs_start && lhs_end == rhs_end,
            (
                Self::ArrayCtor {
                    spec: lhs_spec,
                    init: lhs_init,
                    initialize: lhs_initialize,
                    ..
                },
                Self::ArrayCtor {
                    spec: rhs_spec,
                    init: rhs_init,
                    initialize: rhs_initialize,
                    ..
                },
            ) => lhs_spec == rhs_spec && lhs_init == rhs_init && lhs_initialize == rhs_initialize,
            (
                Self::Compare {
                    op: lhs_op,
                    lhs: lhs_expr,
                    rhs: lhs_rhs,
                    ..
                },
                Self::Compare {
                    op: rhs_op,
                    lhs: rhs_expr,
                    rhs: rhs_rhs,
                    ..
                },
            ) => lhs_op == rhs_op && lhs_expr == rhs_expr && lhs_rhs == rhs_rhs,
            (
                Self::Call {
                    func: lhs_func,
                    args: lhs_args,
                    ..
                },
                Self::Call {
                    func: rhs_func,
                    args: rhs_args,
                    ..
                },
            ) => lhs_func == rhs_func && lhs_args == rhs_args,
            (
                Self::UserCall {
                    name: lhs_name,
                    type_args: lhs_type_args,
                    args: lhs_args,
                    ..
                },
                Self::UserCall {
                    name: rhs_name,
                    type_args: rhs_type_args,
                    args: rhs_args,
                    ..
                },
            ) => lhs_name == rhs_name && lhs_type_args == rhs_type_args && lhs_args == rhs_args,
            (
                Self::Cast {
                    to: lhs_to,
                    expr: lhs_expr,
                    ..
                },
                Self::Cast {
                    to: rhs_to,
                    expr: rhs_expr,
                    ..
                },
            ) => lhs_to == rhs_to && lhs_expr == rhs_expr,
            (Self::UnaryNot { expr: lhs, .. }, Self::UnaryNot { expr: rhs, .. }) => lhs == rhs,
            (Self::UnaryBitNot { expr: lhs, .. }, Self::UnaryBitNot { expr: rhs, .. }) => {
                lhs == rhs
            }
            (
                Self::Logical {
                    op: lhs_op,
                    lhs: lhs_expr,
                    rhs: lhs_rhs,
                    ..
                },
                Self::Logical {
                    op: rhs_op,
                    lhs: rhs_expr,
                    rhs: rhs_rhs,
                    ..
                },
            ) => lhs_op == rhs_op && lhs_expr == rhs_expr && lhs_rhs == rhs_rhs,
            (
                Self::Binary {
                    op: lhs_op,
                    lhs: lhs_expr,
                    rhs: lhs_rhs,
                    ..
                },
                Self::Binary {
                    op: rhs_op,
                    lhs: rhs_expr,
                    rhs: rhs_rhs,
                    ..
                },
            ) => lhs_op == rhs_op && lhs_expr == rhs_expr && lhs_rhs == rhs_rhs,
            (Self::Tuple { values: lhs, .. }, Self::Tuple { values: rhs, .. }) => lhs == rhs,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallArg {
    pub name: Option<String>,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallTypeArg {
    Primitive(PrimitiveType),
    Generic(String),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum BuiltinFn {
    Sin,
    Cos,
    Tan,
    Tanh,
    Atan,
    Atan2,
    Exp,
    Log,
    Sqrt,
    Pow,
    Abs,
    Floor,
    Ceil,
    Round,
    Trunc,
    Min,
    Max,
    Fma,
    /// Compiler-generated range clamp. This is intentionally absent from
    /// `ALL` and `from_name`, so it is not part of the source-language
    /// builtin surface.
    RangeClamp,
    /// Parser-generated zero-based integer binding count clamp. Semantic
    /// analysis validates the positive count before canonicalizing it to an
    /// inclusive `RangeClamp` from zero.
    BindingCountClamp,
    /// Parser-generated half-open integer range clamp. Semantic analysis
    /// validates its exclusive upper bound before canonicalizing it to
    /// `RangeClamp` with inclusive constant bounds.
    BindingRangeClamp,
    /// Parser-generated inclusive integer range clamp. It remains distinct
    /// until semantic range validation so declaration rewrites retain the
    /// binding invariant.
    BindingRangeInclusiveClamp,
    /// Compiler-generated inclusive integer range wrap. This is intentionally
    /// absent from `ALL` and `from_name`.
    RangeWrap,
    /// Parser-generated zero-based integer binding count wrap. Like the
    /// binding range markers, it must not survive semantic validation.
    BindingCountWrap,
    /// Parser-generated half-open integer range wrap. Like
    /// `BindingRangeClamp`, it must not survive semantic range validation.
    BindingRangeWrap,
    /// Parser-generated inclusive integer range wrap. Like the other binding
    /// markers, it must not survive semantic range validation.
    BindingRangeInclusiveWrap,
}

impl BuiltinFn {
    pub const ALL: [Self; 18] = [
        Self::Sin,
        Self::Cos,
        Self::Tan,
        Self::Tanh,
        Self::Atan,
        Self::Atan2,
        Self::Exp,
        Self::Log,
        Self::Sqrt,
        Self::Pow,
        Self::Abs,
        Self::Floor,
        Self::Ceil,
        Self::Round,
        Self::Trunc,
        Self::Min,
        Self::Max,
        Self::Fma,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "sin" => Some(Self::Sin),
            "cos" => Some(Self::Cos),
            "tan" => Some(Self::Tan),
            "tanh" => Some(Self::Tanh),
            "atan" => Some(Self::Atan),
            "atan2" => Some(Self::Atan2),
            "exp" => Some(Self::Exp),
            "log" => Some(Self::Log),
            "sqrt" => Some(Self::Sqrt),
            "pow" => Some(Self::Pow),
            "abs" | "fabs" => Some(Self::Abs),
            "floor" => Some(Self::Floor),
            "ceil" => Some(Self::Ceil),
            "round" => Some(Self::Round),
            "trunc" => Some(Self::Trunc),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "fma" => Some(Self::Fma),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Tan => "tan",
            Self::Tanh => "tanh",
            Self::Atan => "atan",
            Self::Atan2 => "atan2",
            Self::Exp => "exp",
            Self::Log => "log",
            Self::Sqrt => "sqrt",
            Self::Pow => "pow",
            Self::Abs => "abs",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Round => "round",
            Self::Trunc => "trunc",
            Self::Min => "min",
            Self::Max => "max",
            Self::Fma => "fma",
            Self::RangeClamp => "<range-clamp>",
            Self::BindingCountClamp => "<binding-count-clamp>",
            Self::BindingRangeClamp => "<binding-range-clamp>",
            Self::BindingRangeInclusiveClamp => "<inclusive-binding-range-clamp>",
            Self::RangeWrap => "<range-wrap>",
            Self::BindingCountWrap => "<binding-count-wrap>",
            Self::BindingRangeWrap => "<binding-range-wrap>",
            Self::BindingRangeInclusiveWrap => "<inclusive-binding-range-wrap>",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::Sin
            | Self::Cos
            | Self::Tan
            | Self::Tanh
            | Self::Atan
            | Self::Exp
            | Self::Log
            | Self::Sqrt
            | Self::Abs
            | Self::Floor
            | Self::Ceil
            | Self::Round
            | Self::Trunc => 1,
            Self::Pow | Self::Atan2 | Self::Min | Self::Max => 2,
            Self::Fma => 3,
            Self::RangeClamp
            | Self::BindingCountClamp
            | Self::BindingRangeClamp
            | Self::BindingRangeInclusiveClamp
            | Self::RangeWrap
            | Self::BindingCountWrap
            | Self::BindingRangeWrap
            | Self::BindingRangeInclusiveWrap => 3,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
