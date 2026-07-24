use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pest::error::{ErrorVariant, LineColLocation};
use pest::iterators::Pair;
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;

use crate::ast::{
    ArrayElemType, ArrayTypeSpec, AssertDecl, AssignTarget, BinaryOp, Block, BlockExec,
    BufferBlock, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg,
    CallTypeArg, CmpOp, ConstDecl, ConstType, DeclRange, DeclType, EventBlock, EventDef,
    EventParamDecl, EventParamType, Expr, FieldType, FnParamDecl, FnParamType, FunctionDef,
    GraphBlock, GraphEdge, GraphEndpoint, GraphRate, InitBlock, LogicalOp, OutputTiming,
    ParamBlock, ParamControl, ParamDecl, ParamScale, PortBlock, PortDecl, PrimitiveType,
    ProcessorDef, Program, SampleBlock, SourceLoc, Span, Stmt, StructDef, StructField,
    INTERNAL_BUFFER_READ2_FN, INTERNAL_BUFFER_WRITE2_FN, PARAM_SCALES,
};
use crate::diagnostics::Diagnostic;

const PROC_FIELD_SENTINEL_PREFIX: &str = "__onda_proc_field__";
const PROC_FIELD_SENTINEL_ARG: &str = "__proc_field";
const PROC_INDEX_CALL_SENTINEL: &str = "__onda_proc_index_call";
const PROC_INDEX_BASE_ARG: &str = "__proc_index_base";
const PROC_INDEX_EXPR_ARG: &str = "__proc_index_expr";
const GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL: &str = "__onda_graph_proc_array_field_index";
const GRAPH_PROC_FIELD_INDEX_EXPR_ARG: &str = "__proc_field_index_expr";
const STRUCT_ARRAY_FIELD_INDEX_SENTINEL: &str = "__onda_struct_array_field_index";
const SAFI_BASE_ARG: &str = "__safi_base";
const SAFI_IDX_ARG: &str = "__safi_idx";
const SAFI_FIELD_ARG: &str = "__safi_field";
const SAFI_FIELD_IDX_ARG: &str = "__safi_field_idx";
const STDLIB_AUTO_IMPORT_MODULES: &[&str] = &["std/prelude"];
const STDLIB_MODULE_PREFIX: &str = "std/";

pub const LANGUAGE_KEYWORDS: &[&str] = &[
    "import",
    "include",
    "ins",
    "inputs",
    "outs",
    "outputs",
    "kouts",
    "params",
    "kins",
    "events",
    "event",
    "buffers",
    "init",
    "block",
    "sample",
    "graph",
    "const",
    "def",
    "struct",
    "proc",
    "processor",
    "namespace",
    "use",
    "pub",
    "as",
    "pin",
    "if",
    "elif",
    "else",
    "for",
    "in",
    "while",
    "loop",
    "break",
    "continue",
    "return",
    "assert",
    "true",
    "false",
];

pub const RESERVED_IDENTIFIER_WORDS: &[&str] = &["while", "break", "continue", "pin", "as", "pub"];

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct OndaParser;

mod loading_support;
mod module_loading;
mod preprocess;

use module_loading::{parse_loc_from_raw, stmt_loc_from_pair};

mod type_helpers;
pub use module_loading::{
    inject_auto_std_math, inject_auto_std_prelude, parse_namespace_ref_text_ast, parse_program,
    parse_program_file, parse_program_file_from_virtual_sources, parse_program_file_with_overlays,
    parse_program_with_path, parse_stdlib_module,
};
use type_helpers::*;

pub fn stdlib_module_source(module: &str) -> Option<&'static str> {
    loading_support::builtin_std_module_source(module)
}

pub fn stdlib_module_names() -> impl Iterator<Item = &'static str> {
    loading_support::builtin_std_module_names()
}

pub fn is_language_keyword(name: &str) -> bool {
    LANGUAGE_KEYWORDS.contains(&name)
}

pub fn language_type_names() -> impl Iterator<Item = &'static str> {
    PrimitiveType::ALL
        .into_iter()
        .map(PrimitiveType::name)
        .chain(std::iter::once("buffer"))
}

pub fn is_language_type_name(name: &str) -> bool {
    PrimitiveType::is_name(name) || name == "buffer"
}

pub fn is_reserved_identifier(name: &str) -> bool {
    RESERVED_IDENTIFIER_WORDS.contains(&name)
}

pub fn is_reserved_word(name: &str) -> bool {
    is_language_keyword(name) || is_language_type_name(name)
}

mod expr_stmt;
use expr_stmt::*;

mod block_parsing;
use block_parsing::*;

fn diag_from_pest_error(err: pest::error::Error<Rule>) -> Diagnostic {
    let (line, column, end_line, end_column) = match err.line_col {
        LineColLocation::Pos((line, col)) => (line, col, line, col.saturating_add(1)),
        LineColLocation::Span((line, col), (end_line, end_col)) => (line, col, end_line, end_col),
    };
    let loc = parse_loc_from_raw(line, column, end_line, end_column);
    Diagnostic::syntax_at(format_pest_error_message(&err), &loc)
}

fn format_pest_error_message(err: &pest::error::Error<Rule>) -> String {
    match &err.variant {
        ErrorVariant::ParsingError {
            positives,
            negatives,
        } => {
            let mut parts = Vec::new();
            if !negatives.is_empty() {
                parts.push(format!("unexpected {}", format_rule_list(negatives)));
            }
            if !positives.is_empty() {
                parts.push(format!("expected {}", format_rule_list(positives)));
            }
            if parts.is_empty() {
                "parse error".to_owned()
            } else {
                parts.join("; ")
            }
        }
        ErrorVariant::CustomError { message } => message.clone(),
    }
}

fn format_rule_list(rules: &[Rule]) -> String {
    match rules {
        [] => String::new(),
        [rule] => format!("{rule:?}"),
        [first, second] => format!("{first:?} or {second:?}"),
        [head @ .., last] => {
            let mut text = head
                .iter()
                .map(|rule| format!("{rule:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            text.push_str(&format!(", or {last:?}"));
            text
        }
    }
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

pub(super) fn pair_symbol_text(pair: &Pair<'_, Rule>) -> String {
    pair.as_str().trim().to_owned()
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
