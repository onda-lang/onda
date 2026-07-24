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
    NamespaceRefSegment, NamespaceTemplateParam, OutputTiming, ParamBlock, ParamControl, ParamDecl,
    ParamScale, PortBlock, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc,
    Span, Stmt, StructDef, StructField, UseDecl, INTERNAL_BUFFER_READ2_FN,
    INTERNAL_BUFFER_WRITE2_FN, PARAM_SCALES,
};
pub use diagnostics::{DiagCode, DiagCtx, Diagnostic};
pub use parser::{
    inject_auto_std_math, inject_auto_std_prelude, is_language_keyword, is_language_type_name,
    is_reserved_identifier, is_reserved_word, language_type_names, parse_namespace_ref_text_ast,
    parse_program, parse_program_file, parse_program_file_from_virtual_sources,
    parse_program_file_with_overlays, parse_program_with_path, parse_stdlib_module,
    stdlib_module_names, stdlib_module_source, LANGUAGE_KEYWORDS, RESERVED_IDENTIFIER_WORDS,
};
