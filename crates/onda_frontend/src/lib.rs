pub mod ast;
pub mod diagnostics;
pub mod parser;

pub use ast::{
    ArrayElemType, ArrayTypeSpec, AssertDecl, AssignTarget, BinaryOp, Block, BlockExec, BlockKind,
    BufferBlock, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg,
    CallTypeArg, CmpOp, ConstDecl, ConstType, DeclRange, DeclType, EventBlock, EventDef,
    EventParamDecl, EventParamType, Expr, FieldType, FnParamDecl, FnParamType, FnReturnScalarType,
    FnReturnType, FunctionDef, GraphBlock, GraphEdge, GraphEndpoint, GraphRate, InitBlock,
    LogicalOp, NamespaceAliasDecl, NamespaceCallArg, NamespaceDecl, NamespaceItem,
    NamespaceRefSegment, NamespaceTemplateParam, ParamBlock, ParamDecl, PortBlock, PortDecl,
    PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc, Span, Stmt, StructDef,
    StructField,
};
pub use diagnostics::{DiagCode, DiagCtx, Diagnostic};
pub use parser::{
    inject_auto_std_math, inject_auto_std_prelude, parse_namespace_ref_text_ast, parse_program,
    parse_program_file, parse_program_file_with_overlays, parse_program_with_path,
};
