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
    Buffers(Vec<BufferDecl>),
    Proc(ProcessorDef),
    Struct(StructDef),
    Def(FunctionDef),
    Init(Vec<Stmt>),
    Block(BlockExec),
    Sample(Vec<Stmt>),
}

impl Block {
    pub fn kind(&self) -> BlockKind {
        match self {
            Self::Ins(_) => BlockKind::Ins,
            Self::Outs(_) => BlockKind::Outs,
            Self::Params(_) => BlockKind::Params,
            Self::Buffers(_) => BlockKind::Buffers,
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
    Buffers,
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
    pub sample: Option<Vec<Stmt>>,
    pub post: Vec<Stmt>,
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
pub struct ProcessorDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub ins: Vec<PortDecl>,
    pub outs: Vec<PortDecl>,
    pub params: Vec<ParamDecl>,
    pub buffers: Vec<BufferDecl>,
    pub has_init_block: bool,
    pub has_block_block: bool,
    pub has_sample_block: bool,
    pub init: Vec<Stmt>,
    pub block_pre: Vec<Stmt>,
    pub sample: Vec<Stmt>,
    pub block_post: Vec<Stmt>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
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
    Data(DataTypeSpec),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataElemType {
    Primitive(PrimitiveType),
    Struct(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataTypeSpec {
    pub elem: DataElemType,
    pub size: Box<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    Var(String),
    Index { base: String, index: Expr },
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
        start: Expr,
        end: Expr,
        body: Vec<Stmt>,
    },
}

impl Stmt {
    pub fn loc(&self) -> Option<&SourceLoc> {
        match self {
            Self::Assign { loc, .. } => loc.as_ref(),
            Self::Expr { loc, .. } => loc.as_ref(),
            Self::Return { loc, .. } => loc.as_ref(),
            Self::If { loc, .. } => loc.as_ref(),
            Self::For { loc, .. } => loc.as_ref(),
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
    DataCtor {
        spec: DataTypeSpec,
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
