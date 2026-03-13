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
    ArrayElemType, ArrayTypeSpec, AssertDecl, AssignTarget, BinaryOp, Block, BlockExec,
    BufferBlock, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg,
    CallTypeArg, CmpOp, ConstDecl, DeclRange, DeclType, EventBlock, EventDef, EventParamDecl,
    EventParamType, Expr, FieldType, FnParamDecl, FnParamType, FunctionDef, GraphBlock, GraphEdge,
    GraphEndpoint, GraphRate, InitBlock, LogicalOp, ParamBlock, ParamDecl, PortBlock, PortDecl,
    PrimitiveType, ProcessorDef, Program, SampleBlock, SourceLoc, Span, Stmt, StructDef,
    StructField,
};
use crate::diagnostics::Diagnostic;

const PROC_FIELD_SENTINEL_PREFIX: &str = "__omni_proc_field__";
const PROC_FIELD_SENTINEL_ARG: &str = "__proc_field";
const PROC_INDEX_CALL_SENTINEL: &str = "__omni_proc_index_call";
const PROC_INDEX_BASE_ARG: &str = "__proc_index_base";
const PROC_INDEX_EXPR_ARG: &str = "__proc_index_expr";
const GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL: &str = "__omni_graph_proc_array_field_index";
const GRAPH_PROC_FIELD_INDEX_EXPR_ARG: &str = "__proc_field_index_expr";
const BUFFER_READ2_INTERNAL_FN: &str = "__omni_buffer_read2";
const BUFFER_WRITE2_INTERNAL_FN: &str = "__omni_buffer_write2";
const STDLIB_AUTO_IMPORT_MODULES: &[&str] = &["std/prelude"];
const STDLIB_MODULE_PREFIX: &str = "std/";

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct OmniParser;

mod loading_support;
mod module_loading;
mod preprocess;

use module_loading::stmt_loc_from_pair;

mod type_helpers;
pub use module_loading::{
    inject_auto_std_math, inject_auto_std_prelude, parse_program, parse_program_file,
    parse_program_file_with_overlays, parse_program_with_path,
};
use type_helpers::*;

mod expr_stmt;
use expr_stmt::*;

mod block_parsing;
use block_parsing::*;

fn diag_from_pest_error(err: pest::error::Error<Rule>) -> Diagnostic {
    let (line, column, end_line, end_column) = match err.line_col {
        LineColLocation::Pos((line, col)) => (line, col, line, col.saturating_add(1)),
        LineColLocation::Span((line, col), (end_line, end_col)) => (line, col, end_line, end_col),
    };
    let mut diag = Diagnostic::syntax(err.to_string(), line, column);
    diag.end_line = end_line;
    diag.end_column = if end_line == line {
        end_column.max(column.saturating_add(1))
    } else {
        end_column.max(1)
    };
    diag
}

pub(super) fn syntax_at_pair(pair: &Pair<'_, Rule>, message: impl Into<String>) -> Diagnostic {
    let message = message.into();
    let loc: SourceLoc = stmt_loc_from_pair(pair).into();
    if loc.is_zero() {
        Diagnostic::syntax(message, 0, 0)
    } else {
        Diagnostic::syntax_at(message, &loc)
    }
}

pub(super) fn syntax_at_loc(loc: impl Into<SourceLoc>, message: impl Into<String>) -> Diagnostic {
    let message = message.into();
    let loc = loc.into();
    if loc.is_zero() {
        Diagnostic::syntax(message, 0, 0)
    } else {
        Diagnostic::syntax_at(message, &loc)
    }
}

#[cfg(test)]
mod tests;
