pub mod ast;
pub mod diagnostics;
pub mod parser;

pub use ast::{
    ArrayElemType, ArrayTypeSpec, AssertDecl, AssignTarget, BinaryOp, Block, BlockExec, BlockKind,
    BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp,
    ConstDecl, DeclRange, DeclType, EventDef, EventParamDecl, EventParamType, Expr, FieldType,
    FnParamDecl, FnParamType, FunctionDef, GraphBlock, GraphEdge, GraphEndpoint, GraphRate,
    InitBlock, LogicalOp, ParamDecl, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock,
    SourceLoc, Span, Stmt, StructDef, StructField,
};
pub use diagnostics::{DiagCode, DiagCtx, Diagnostic};
pub use parser::{
    inject_auto_std_math, inject_auto_std_prelude, parse_program, parse_program_file,
    parse_program_file_with_overlays, parse_program_with_path,
};
