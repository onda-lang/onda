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
pub enum Block {
    Ins(Vec<PortDecl>),
    Outs(Vec<PortDecl>),
    Params(Vec<ParamDecl>),
    Const(ConstDecl),
    Events(Vec<EventDef>),
    Buffers(Vec<BufferDecl>),
    Assert(AssertDecl),
    Proc(ProcessorDef),
    Struct(StructDef),
    Def(FunctionDef),
    Init(InitBlock),
    Block(BlockExec),
    Sample(SampleBlock),
}

impl Block {
    pub fn kind(&self) -> BlockKind {
        match self {
            Self::Ins(_) => BlockKind::Ins,
            Self::Outs(_) => BlockKind::Outs,
            Self::Params(_) => BlockKind::Params,
            Self::Const(_) => BlockKind::Const,
            Self::Events(_) => BlockKind::Events,
            Self::Buffers(_) => BlockKind::Buffers,
            Self::Assert(_) => BlockKind::Assert,
            Self::Proc(_) => BlockKind::Proc,
            Self::Struct(_) => BlockKind::Struct,
            Self::Def(_) => BlockKind::Def,
            Self::Init(_) => BlockKind::Init,
            Self::Block(_) => BlockKind::Block,
            Self::Sample(_) => BlockKind::Sample,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum BlockKind {
    Ins,
    Outs,
    Params,
    Const,
    Events,
    Buffers,
    Assert,
    Proc,
    Struct,
    Def,
    Init,
    Block,
    Sample,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockExec {
    pub pre: Vec<Stmt>,
    pub sample: Option<SampleBlock>,
    pub post: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SampleBlock {
    pub oversample_factor: Option<Expr>,
    pub body: Vec<Stmt>,
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
    pub default_ty: Option<DeclType>,
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
    pub name: String,
    pub ty: Option<DeclType>,
    pub default: Option<Expr>,
    pub range: Option<DeclRange>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    pub name: String,
    pub ty: Option<DeclType>,
    pub default: Option<Expr>,
    pub range: Option<DeclRange>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclRange {
    pub min: Option<Expr>,
    pub max: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BufferDecl {
    pub name: String,
    pub ty: Option<BufferType>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertDecl {
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<PrimitiveType>,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub ins: Vec<PortDecl>,
    pub outs: Vec<PortDecl>,
    pub params: Vec<ParamDecl>,
    pub events: Vec<EventDef>,
    pub buffers: Vec<BufferDecl>,
    pub has_init_block: bool,
    pub has_block_block: bool,
    pub has_sample_block: bool,
    pub sample_oversample_factor: Option<Expr>,
    pub init: InitBlock,
    pub block_pre: Vec<Stmt>,
    pub sample: Vec<Stmt>,
    pub block_post: Vec<Stmt>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum PrimitiveType {
    F32,
    F64,
    I32,
    I64,
    Bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclType {
    Scalar(PrimitiveType),
    Generic(String),
    ArrayGeneric { elem: String, size: Expr },
    Array { elem: PrimitiveType, size: Expr },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FnParamType {
    Primitive(PrimitiveType),
    Struct(String),
    Buffer(BufferType),
    Array(Option<PrimitiveType>),
    ArrayGeneric(String),
    BareBuffer,
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
    pub name: String,
    pub ty: Option<FnParamType>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventDef {
    pub name: String,
    pub params: Vec<EventParamDecl>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventParamDecl {
    pub name: String,
    pub ty: EventParamType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventParamType {
    Scalar(PrimitiveType),
    Array { elem: PrimitiveType, size: Expr },
    Slice { elem: PrimitiveType },
    GenericSlice { elem: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub params: Vec<FnParamDecl>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<StructField>,
    pub methods: Vec<FunctionDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub ty: FieldType,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Scalar(PrimitiveType),
    Generic(String),
    Array(ArrayTypeSpec),
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
        start: Option<Expr>,
        end: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Const {
        loc: Option<SourceLoc>,
        decl: ConstDecl,
    },
    Assign {
        loc: Option<SourceLoc>,
        target: AssignTarget,
        decl_ty: Option<PrimitiveType>,
        generic_decl_ty: Option<String>,
        is_typed_decl: bool,
        expr: Expr,
    },
    Expr {
        loc: Option<SourceLoc>,
        expr: Expr,
    },
    Return {
        loc: Option<SourceLoc>,
        expr: Expr,
    },
    If {
        loc: Option<SourceLoc>,
        cond: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Vec<Stmt>,
    },
    For {
        loc: Option<SourceLoc>,
        var: String,
        step: Option<Expr>,
        start: Expr,
        end: Expr,
        end_inclusive: bool,
        body: Vec<Stmt>,
    },
    While {
        loc: Option<SourceLoc>,
        cond: Expr,
        body: Vec<Stmt>,
    },
    Break {
        loc: Option<SourceLoc>,
    },
    Continue {
        loc: Option<SourceLoc>,
    },
}

impl Stmt {
    pub fn loc(&self) -> Option<&SourceLoc> {
        match self {
            Self::Const { loc, .. } => loc.as_ref(),
            Self::Assign { loc, .. } => loc.as_ref(),
            Self::Expr { loc, .. } => loc.as_ref(),
            Self::Return { loc, .. } => loc.as_ref(),
            Self::If { loc, .. } => loc.as_ref(),
            Self::For { loc, .. } => loc.as_ref(),
            Self::While { loc, .. } => loc.as_ref(),
            Self::Break { loc, .. } => loc.as_ref(),
            Self::Continue { loc, .. } => loc.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(f32),
    Int(i64),
    Bool(bool),
    ArrayLiteral(Vec<Expr>),
    Var(String),
    Index {
        base: String,
        index: Box<Expr>,
    },
    Slice {
        base: String,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
    },
    ArrayCtor {
        spec: ArrayTypeSpec,
        init: Option<Vec<Expr>>,
    },
    Compare {
        op: CmpOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        func: BuiltinFn,
        args: Vec<Expr>,
    },
    UserCall {
        name: String,
        type_args: Vec<CallTypeArg>,
        args: Vec<CallArg>,
    },
    Cast {
        to: PrimitiveType,
        expr: Box<Expr>,
    },
    UnaryNot {
        expr: Box<Expr>,
    },
    UnaryBitNot {
        expr: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
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
