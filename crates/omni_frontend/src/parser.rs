use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pest::error::LineColLocation;
use pest::iterators::Pair;
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;

use crate::ast::{
    AssignTarget, BinaryOp, Block, BlockExec, BufferChannels, BufferDecl, BufferElemType,
    BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp, DataElemType, DataTypeSpec, DeclRange,
    DeclType, EventDef, EventParamDecl, EventParamType, Expr, FieldType, FnParamDecl, FnParamType,
    FunctionDef, LogicalOp, ParamDecl, PortDecl, PrimitiveType, ProcessorDef, Program, SampleBlock,
    SourceLoc, Stmt, StructDef, StructField,
};
use crate::diagnostics::Diagnostic;

const PROC_FIELD_SENTINEL_PREFIX: &str = "__omni_proc_field__";
const PROC_FIELD_SENTINEL_ARG: &str = "__proc_field";
const PROC_INDEX_CALL_SENTINEL: &str = "__omni_proc_index_call";
const PROC_INDEX_BASE_ARG: &str = "__proc_index_base";
const PROC_INDEX_EXPR_ARG: &str = "__proc_index_expr";
const BUFFER_READ2_INTERNAL_FN: &str = "__omni_buffer_read2";
const BUFFER_WRITE2_INTERNAL_FN: &str = "__omni_buffer_write2";
const STDLIB_AUTO_IMPORT_MODULE: &str = "std/math";
const STDLIB_MODULE_PREFIX: &str = "std/";

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct OmniParser;

mod module_loading;

use module_loading::stmt_loc_from_pair;

mod type_helpers;
pub use module_loading::{inject_auto_std_math, parse_program, parse_program_file};
use type_helpers::*;

mod expr_stmt;
use expr_stmt::*;

mod block_parsing;
use block_parsing::*;

#[derive(Clone, Copy)]
struct PendingIndentBlock {
    line: usize,
    indent: usize,
}

fn diag_from_pest_error(err: pest::error::Error<Rule>) -> Diagnostic {
    let (line, column) = match err.line_col {
        LineColLocation::Pos((line, col)) => (line, col),
        LineColLocation::Span((line, col), _) => (line, col),
    };
    Diagnostic::syntax(err.to_string(), line, column)
}

#[cfg(test)]
mod tests;
