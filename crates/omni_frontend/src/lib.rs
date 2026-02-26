pub mod ast;
pub mod diagnostics;
pub mod parser;

pub use ast::{
    AssignTarget, BinaryOp, Block, BlockExec, BlockKind, BufferChannels, BufferDecl,
    BufferElemType, BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp, DataElemType, DataTypeSpec,
    DeclRange, DeclType, Expr, FieldType, FnParamDecl, FnParamType, FunctionDef, LogicalOp,
    ParamDecl, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc, Stmt,
    StructDef, StructField,
};
pub use diagnostics::{with_diagnostic_location, DiagCode, Diagnostic};
pub use parser::{inject_auto_std_math, parse_program, parse_program_file};
