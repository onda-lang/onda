use std::collections::HashSet;
use std::path::Path;

use onda_frontend::{
    parse_program, parse_program_with_path, Block, Expr, NamespaceItem, ProcessorDef, Program, Span,
};

mod ast_index;
mod source_fallback;

use ast_index::{
    build_semantic_scope_index, collect_all_symbols, normalize_file_key,
    normalize_file_key_for_path,
};
use source_fallback::{
    build_source_scope_index, identifier_is_in_import_path, identifier_is_in_use_namespace_name,
    scan_identifiers,
};

pub(super) const SEMANTIC_TOKEN_TYPE_ENUM_MEMBER: u32 = 0;
pub(super) const SEMANTIC_TOKEN_TYPE_VARIABLE: u32 = 1;
pub(super) const SEMANTIC_TOKEN_TYPE_PORT: u32 = 2;
pub(super) const SEMANTIC_TOKEN_TYPE_PARAMETER: u32 = 3;
pub(super) const SEMANTIC_TOKEN_TYPE_FUNCTION: u32 = 4;
pub(super) const SEMANTIC_TOKEN_TYPE_TYPE: u32 = 5;
pub(super) const SEMANTIC_TOKEN_TYPE_NAMESPACE: u32 = 6;
pub(super) const SEMANTIC_TOKEN_TYPE_STATE: u32 = 7;
pub(super) const SEMANTIC_TOKEN_TYPE_KEYWORD: u32 = 8;
pub(super) const SEMANTIC_TOKEN_TYPE_NUMBER: u32 = 9;

const SEMANTIC_TOKEN_LEGEND: &[&str] = &[
    "enumMember",
    "variable",
    "port",
    "parameter",
    "function",
    "type",
    "namespace",
    "state",
    "keyword",
    "number",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SemanticToken {
    pub(super) line: u32,
    pub(super) start: u32,
    pub(super) length: u32,
    pub(super) token_type: u32,
    pub(super) token_modifiers: u32,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SemanticScope {
    namespaces: HashSet<String>,
    consts: HashSet<String>,
    types: HashSet<String>,
    functions: HashSet<String>,
    state_variables: HashSet<String>,
    variables: HashSet<String>,
    ports: HashSet<String>,
    parameters: HashSet<String>,
}

impl SemanticScope {
    pub(super) fn token_type_for(&self, name: &str) -> Option<u32> {
        if self.namespaces.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_NAMESPACE)
        } else if self.consts.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
        } else if self.types.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_TYPE)
        } else if self.parameters.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_PARAMETER)
        } else if self.ports.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_PORT)
        } else if self.functions.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_FUNCTION)
        } else if self.state_variables.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_STATE)
        } else if self.variables.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_VARIABLE)
        } else {
            None
        }
    }

    pub(super) fn token_type_for_source_fallback(&self, name: &str) -> Option<u32> {
        self.token_type_for(name).or_else(|| {
            if is_implicit_port_name(name) {
                Some(SEMANTIC_TOKEN_TYPE_PORT)
            } else {
                None
            }
        })
    }

    pub(super) fn insert_variable(&mut self, name: String) {
        if !self.consts.contains(&name)
            && !self.ports.contains(&name)
            && !self.parameters.contains(&name)
            && !self.types.contains(&name)
        {
            self.variables.insert(name);
        }
    }

    pub(super) fn insert_state_variable(&mut self, name: String) {
        if !self.consts.contains(&name)
            && !self.ports.contains(&name)
            && !self.parameters.contains(&name)
            && !self.types.contains(&name)
        {
            self.state_variables.insert(name);
        }
    }

    pub(super) fn imported_token_type_for(&self, name: &str) -> Option<u32> {
        if self.namespaces.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_NAMESPACE)
        } else if self.consts.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
        } else if self.types.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_TYPE)
        } else if self.functions.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_FUNCTION)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct SemanticScopeIndex {
    document_scope: SemanticScope,
    scopes: Vec<ScopedSemanticScope>,
    scopes_by_line: Vec<Vec<usize>>,
}

#[derive(Debug, Clone)]
struct ScopedSemanticScope {
    scope: SemanticScope,
    parent: Option<usize>,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    depth: usize,
    allows_implicit_ports: bool,
}

impl SemanticScopeIndex {
    pub(super) fn token_type_for(&self, name: &str, line: u32, column: u32) -> (Option<u32>, bool) {
        let containing_scope = self.innermost_scope_index(line, column);
        let token_type = containing_scope
            .and_then(|idx| self.resolve_scope_chain(idx, name))
            .or_else(|| self.document_scope.token_type_for(name));
        (token_type, containing_scope.is_some())
    }

    fn push_scope(&mut self, parent: Option<usize>, span: onda_frontend::Span) -> Option<usize> {
        if span.is_zero() {
            return None;
        }
        let depth = parent.map(|idx| self.scopes[idx].depth + 1).unwrap_or(0);
        let allows_implicit_ports = parent
            .map(|idx| self.scopes[idx].allows_implicit_ports)
            .unwrap_or(false);
        self.scopes.push(ScopedSemanticScope::from_span(
            span,
            parent,
            depth,
            allows_implicit_ports,
        ));
        Some(self.scopes.len() - 1)
    }

    pub(super) fn rebuild_scope_line_index(&mut self) {
        self.scopes_by_line.clear();
        let Some(max_line) = self.scopes.iter().map(|scope| scope.end_line).max() else {
            return;
        };
        self.scopes_by_line
            .resize_with(max_line.saturating_add(1) as usize, Vec::new);
        for (idx, scope) in self.scopes.iter().enumerate() {
            for line in scope.start_line..=scope.end_line {
                if let Some(line_scopes) = self.scopes_by_line.get_mut(line as usize) {
                    line_scopes.push(idx);
                }
            }
        }
    }

    fn innermost_scope_index(&self, line: u32, column: u32) -> Option<usize> {
        let scopes_on_line = self.scopes_by_line.get(line as usize)?;
        let mut best: Option<usize> = None;
        for &idx in scopes_on_line {
            let scope = &self.scopes[idx];
            if !scope.contains(line, column) {
                continue;
            }
            let replace = match best {
                None => true,
                Some(best_idx) => {
                    scope.depth > self.scopes[best_idx].depth
                        || (scope.depth == self.scopes[best_idx].depth
                            && scope.is_narrower_than(&self.scopes[best_idx]))
                }
            };
            if replace {
                best = Some(idx);
            }
        }
        best
    }

    fn resolve_scope_chain(&self, idx: usize, name: &str) -> Option<u32> {
        let mut current = Some(idx);
        let mut allows_implicit_ports = false;
        while let Some(scope_idx) = current {
            let scope = &self.scopes[scope_idx];
            allows_implicit_ports |= scope.allows_implicit_ports;
            if let Some(token_type) = scope.scope.token_type_for(name) {
                return Some(token_type);
            }
            current = scope.parent;
        }
        if allows_implicit_ports && is_implicit_port_name(name) {
            return Some(SEMANTIC_TOKEN_TYPE_PORT);
        }
        None
    }
}

impl ScopedSemanticScope {
    fn from_span(
        span: onda_frontend::Span,
        parent: Option<usize>,
        depth: usize,
        allows_implicit_ports: bool,
    ) -> Self {
        Self {
            scope: SemanticScope::default(),
            parent,
            start_line: span.line.saturating_sub(1),
            start_column: u32::from(span.column.saturating_sub(1)),
            end_line: span.end_line().saturating_sub(1),
            end_column: u32::from(span.end_column.saturating_sub(1)),
            depth,
            allows_implicit_ports,
        }
    }

    fn contains(&self, line: u32, column: u32) -> bool {
        if line < self.start_line || line > self.end_line {
            return false;
        }
        if line == self.start_line && column < self.start_column {
            return false;
        }
        if line == self.end_line && column > self.end_column {
            return false;
        }
        true
    }

    fn is_narrower_than(&self, other: &Self) -> bool {
        let self_line_span = self.end_line.saturating_sub(self.start_line);
        let other_line_span = other.end_line.saturating_sub(other.start_line);
        if self_line_span != other_line_span {
            return self_line_span < other_line_span;
        }

        let self_col_span = self.end_column.saturating_sub(self.start_column);
        let other_col_span = other.end_column.saturating_sub(other.start_column);
        self_col_span < other_col_span
    }
}

pub(super) fn semantic_token_legend() -> &'static [&'static str] {
    SEMANTIC_TOKEN_LEGEND
}

#[cfg(test)]
pub(super) fn semantic_tokens_for_document(
    source: &str,
    path: Option<&Path>,
) -> Vec<SemanticToken> {
    semantic_tokens_for_document_with_optional_parse(source, path, None, true)
}

pub(super) fn semantic_tokens_for_document_with_parsed(
    source: &str,
    path: Option<&Path>,
    parsed_program: Option<&onda_frontend::Program>,
) -> Vec<SemanticToken> {
    semantic_tokens_for_document_with_optional_parse(source, path, parsed_program, true)
}

pub(super) fn semantic_tokens_for_document_source_only(
    source: &str,
    path: Option<&Path>,
) -> Vec<SemanticToken> {
    semantic_tokens_for_document_with_optional_parse(source, path, None, false)
}

fn semantic_tokens_for_document_with_optional_parse(
    source: &str,
    path: Option<&Path>,
    parsed_program: Option<&onda_frontend::Program>,
    parse_if_missing: bool,
) -> Vec<SemanticToken> {
    let parsed_owned;
    let parsed_program = if let Some(program) = parsed_program {
        Some(program)
    } else if !parse_if_missing {
        None
    } else {
        parsed_owned = match path {
            Some(path) => parse_program_with_path(source, path).ok(),
            None => parse_program(source).ok(),
        };
        parsed_owned.as_ref()
    };
    let imported_scope = parsed_program.map(collect_all_symbols).unwrap_or_default();
    let current_file_key = match path {
        Some(path) => normalize_file_key_for_path(path),
        None => Some(ast_index::normalize_file_key("<memory>")),
    };
    let scope_index = parsed_program
        .map(|program| build_semantic_scope_index(program, current_file_key.as_deref()));
    let source_scope_index = build_source_scope_index(source);
    let mut source_decl_scope = SemanticScope::default();
    let source_lines = source.lines().collect::<Vec<_>>();
    let graph_delay_ranges = parsed_program
        .map(|program| collect_graph_delay_ranges(program, current_file_key.as_deref()))
        .unwrap_or_default();

    source_fallback::collect_source_declaration_symbols(source, &mut source_decl_scope);

    let mut tokens = Vec::new();
    scan_identifiers(
        source,
        |name, line, start, length, after_dot, is_call, in_ns_path, is_named_arg_label| {
            if identifier_is_in_import_path(&source_lines, line, start) {
                let token_type = source_decl_scope
                    .imported_token_type_for(name)
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .unwrap_or(SEMANTIC_TOKEN_TYPE_NAMESPACE);
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type,
                    token_modifiers: 0,
                });
                return;
            }
            if name != "as" && identifier_is_in_use_namespace_name(&source_lines, line, start) {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_NAMESPACE,
                    token_modifiers: 0,
                });
                return;
            }
            if is_semantic_keyword(name, &source_lines, line, start) {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_KEYWORD,
                    token_modifiers: 0,
                });
                return;
            }
            if onda_frontend::is_language_type_name(name) {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_TYPE,
                    token_modifiers: 0,
                });
                return;
            }
            if is_named_arg_label {
                let token_type = named_arg_label_token_type(
                    source,
                    &source_lines,
                    line,
                    start,
                    &source_decl_scope,
                    &imported_scope,
                );
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type,
                    token_modifiers: 0,
                });
                return;
            }
            if identifier_is_struct_field_declaration(&source_lines, line, start) {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_STATE,
                    token_modifiers: 0,
                });
                return;
            }
            if name == "self" {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_PORT,
                    token_modifiers: 0,
                });
                return;
            }
            if after_dot {
                if identifier_is_after_self_dot(&source_lines, line, start) {
                    tokens.push(SemanticToken {
                        line,
                        start,
                        length,
                        token_type: SEMANTIC_TOKEN_TYPE_STATE,
                        token_modifiers: 0,
                    });
                    return;
                }
                let token_type = if is_call {
                    SEMANTIC_TOKEN_TYPE_FUNCTION
                } else {
                    SEMANTIC_TOKEN_TYPE_PORT
                };
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type,
                    token_modifiers: 0,
                });
                return;
            }
            if in_ns_path {
                let token_type = source_decl_scope
                    .imported_token_type_for(name)
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .unwrap_or(SEMANTIC_TOKEN_TYPE_NAMESPACE);
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type,
                    token_modifiers: 0,
                });
                return;
            }
            let scope_resolution = scope_index
                .as_ref()
                .map(|index| index.token_type_for(name, line, start));
            let source_scope_resolution = source_scope_index.token_type_for(name, line, start);
            let resolved_token_type = match scope_resolution {
                Some((token_type, true)) => token_type
                    .or(source_scope_resolution)
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .or_else(|| source_decl_scope.imported_token_type_for(name)),
                Some((Some(SEMANTIC_TOKEN_TYPE_NAMESPACE), false)) => source_scope_resolution
                    .or(Some(SEMANTIC_TOKEN_TYPE_NAMESPACE))
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .or_else(|| source_decl_scope.imported_token_type_for(name))
                    .or_else(|| source_decl_scope.token_type_for_source_fallback(name)),
                Some((token_type, false)) => token_type
                    .or(source_scope_resolution)
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .or_else(|| source_decl_scope.imported_token_type_for(name))
                    .or_else(|| source_decl_scope.token_type_for_source_fallback(name)),
                None => source_scope_resolution
                    .or_else(|| source_decl_scope.token_type_for_source_fallback(name))
                    .or_else(|| imported_scope.imported_token_type_for(name)),
            };
            if let Some(mut token_type) = resolved_token_type {
                if is_call && token_type == SEMANTIC_TOKEN_TYPE_VARIABLE {
                    token_type = SEMANTIC_TOKEN_TYPE_FUNCTION;
                }
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type,
                    token_modifiers: 0,
                });
            } else if is_call && !source_fallback::is_reserved_word(name) {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_FUNCTION,
                    token_modifiers: 0,
                });
            }
        },
    );
    add_graph_delay_number_literals(source, &graph_delay_ranges, &mut tokens);

    tokens.sort_by_key(|t| (t.line, t.start, t.length, t.token_type, t.token_modifiers));
    tokens.dedup_by(|a, b| {
        a.line == b.line
            && a.start == b.start
            && a.length == b.length
            && a.token_type == b.token_type
            && a.token_modifiers == b.token_modifiers
    });
    tokens
}

#[derive(Clone, Copy)]
struct SourceRange {
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

impl SourceRange {
    fn from_span(span: Span) -> Option<Self> {
        if span.is_zero() {
            return None;
        }
        Some(Self {
            start_line: span.line.saturating_sub(1),
            start_column: u32::from(span.column.saturating_sub(1)),
            end_line: span.end_line().saturating_sub(1),
            end_column: u32::from(span.end_column.saturating_sub(1)),
        })
    }

    fn line_bounds(&self, line: u32, line_len: u32) -> Option<(u32, u32)> {
        if line < self.start_line || line > self.end_line {
            return None;
        }
        let start = if line == self.start_line {
            self.start_column
        } else {
            0
        };
        let end = if line == self.end_line {
            self.end_column
        } else {
            line_len
        };
        (start < end).then_some((start, end))
    }
}

fn collect_graph_delay_ranges(
    program: &Program,
    current_file_key: Option<&str>,
) -> Vec<SourceRange> {
    let mut ranges = Vec::new();
    for block in &program.blocks {
        collect_block_graph_delay_ranges(block, current_file_key, &mut ranges);
    }
    ranges
}

fn collect_block_graph_delay_ranges(
    block: &Block,
    current_file_key: Option<&str>,
    ranges: &mut Vec<SourceRange>,
) {
    match block {
        Block::Graph(graph) => {
            for edge in &graph.edges {
                collect_graph_delay_expr_range(edge.delay.as_ref(), current_file_key, ranges);
            }
        }
        Block::Proc(proc_def) => {
            collect_proc_graph_delay_ranges(proc_def, current_file_key, ranges)
        }
        Block::Namespace(ns) => {
            for item in &ns.items {
                collect_namespace_item_graph_delay_ranges(item, current_file_key, ranges);
            }
        }
        _ => {}
    }
}

fn collect_namespace_item_graph_delay_ranges(
    item: &NamespaceItem,
    current_file_key: Option<&str>,
    ranges: &mut Vec<SourceRange>,
) {
    match item {
        NamespaceItem::Proc(proc_def) => {
            collect_proc_graph_delay_ranges(proc_def, current_file_key, ranges);
        }
        NamespaceItem::Namespace(ns) => {
            for item in &ns.items {
                collect_namespace_item_graph_delay_ranges(item, current_file_key, ranges);
            }
        }
        _ => {}
    }
}

fn collect_proc_graph_delay_ranges(
    proc_def: &ProcessorDef,
    current_file_key: Option<&str>,
    ranges: &mut Vec<SourceRange>,
) {
    let Some(graph) = &proc_def.graph else {
        return;
    };
    for edge in &graph.edges {
        collect_graph_delay_expr_range(edge.delay.as_ref(), current_file_key, ranges);
    }
}

fn collect_graph_delay_expr_range(
    delay: Option<&Expr>,
    current_file_key: Option<&str>,
    ranges: &mut Vec<SourceRange>,
) {
    let Some(delay) = delay else {
        return;
    };
    let span = Span::from(delay.loc());
    if !span_belongs_to_current_file(span, current_file_key) {
        return;
    }
    if let Some(range) = SourceRange::from_span(span) {
        ranges.push(range);
    }
}

fn span_belongs_to_current_file(span: Span, current_file_key: Option<&str>) -> bool {
    match current_file_key {
        Some(expected) => span
            .file()
            .map(|file| normalize_file_key(&file) == expected)
            .unwrap_or(false),
        None => true,
    }
}

fn add_graph_delay_number_literals(
    source: &str,
    ranges: &[SourceRange],
    tokens: &mut Vec<SemanticToken>,
) {
    if ranges.is_empty() {
        return;
    }
    for (line_no, line_text) in source.lines().enumerate() {
        let line_no = line_no as u32;
        let line_len = line_text
            .chars()
            .map(|ch| ch.len_utf16() as u32)
            .sum::<u32>();
        for range in ranges {
            let Some((start, end)) = range.line_bounds(line_no, line_len) else {
                continue;
            };
            scan_number_literals_on_line(line_no, line_text, start, end, tokens);
        }
    }
}

fn scan_number_literals_on_line(
    line_no: u32,
    line_text: &str,
    start: u32,
    end: u32,
    tokens: &mut Vec<SemanticToken>,
) {
    let chars = line_text.chars().collect::<Vec<_>>();
    let mut idx = start as usize;
    let end = end as usize;
    while idx < chars.len() && idx < end {
        let ch = chars[idx];
        let starts_dot_number =
            ch == '.' && idx + 1 < chars.len() && chars[idx + 1].is_ascii_digit();
        if !ch.is_ascii_digit() && !starts_dot_number {
            idx += 1;
            continue;
        }
        if idx > 0 && is_identifier_continue_char(chars[idx - 1]) {
            idx += 1;
            continue;
        }

        let literal_start = idx;
        if ch == '.' {
            idx += 1;
            while idx < chars.len() && idx < end && chars[idx].is_ascii_digit() {
                idx += 1;
            }
        } else {
            while idx < chars.len() && idx < end && chars[idx].is_ascii_digit() {
                idx += 1;
            }
            if idx < chars.len() && idx < end && chars[idx] == '.' {
                idx += 1;
                while idx < chars.len() && idx < end && chars[idx].is_ascii_digit() {
                    idx += 1;
                }
            }
        }

        if idx < chars.len() && is_identifier_continue_char(chars[idx]) {
            continue;
        }
        tokens.push(SemanticToken {
            line: line_no,
            start: literal_start as u32,
            length: (idx - literal_start) as u32,
            token_type: SEMANTIC_TOKEN_TYPE_NUMBER,
            token_modifiers: 0,
        });
    }
}

fn is_semantic_keyword(name: &str, source_lines: &[&str], line: u32, start: u32) -> bool {
    if name == "as" {
        return true;
    }
    if name == "pin" {
        return true;
    }
    if !onda_frontend::is_language_keyword(name) {
        return false;
    }

    let Some(line_text) = source_lines.get(line as usize) else {
        return false;
    };
    let start = start as usize;
    if start > line_text.len() {
        return false;
    }
    let before = &line_text[..start];
    let before_trimmed = before.trim_end();
    before_trimmed.is_empty()
        || before_trimmed.ends_with('{')
        || before_trimmed.ends_with('}')
        || before_trimmed.ends_with(';')
        || (name == "use" && before_trimmed == "pub")
}

fn identifier_is_after_self_dot(source_lines: &[&str], line: u32, start: u32) -> bool {
    let Some(line_text) = source_lines.get(line as usize) else {
        return false;
    };
    let start = start as usize;
    let Some(before_dot) = line_text
        .get(..start)
        .and_then(|before| before.strip_suffix('.'))
    else {
        return false;
    };
    let receiver_end = before_dot.trim_end().len();
    let receiver = &before_dot[..receiver_end];
    let receiver_start = receiver
        .rfind(|ch: char| !is_identifier_continue_char(ch))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    receiver.get(receiver_start..) == Some("self")
}

fn identifier_is_struct_field_declaration(source_lines: &[&str], line: u32, start: u32) -> bool {
    let Some(line_text) = source_lines.get(line as usize) else {
        return false;
    };
    let start = start as usize;
    if start != leading_indent_len(line_text) {
        return false;
    }
    let Some(rest) = line_text.get(start..) else {
        return false;
    };
    let Some(name_end) = leading_identifier_len(rest) else {
        return false;
    };
    let after_name = rest[name_end..].trim_start();
    if !after_name.starts_with(':') || after_name.starts_with("::") {
        return false;
    }
    let field_indent = start;
    if field_indent == 0 {
        return false;
    }

    let mut idx = line as usize;
    while idx > 0 {
        idx -= 1;
        let Some(prev_line) = source_lines.get(idx) else {
            break;
        };
        let trimmed = prev_line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let prev_indent = leading_indent_len(prev_line);
        if prev_indent >= field_indent {
            continue;
        }
        return trimmed.starts_with("struct ");
    }
    false
}

fn named_arg_label_token_type(
    source: &str,
    source_lines: &[&str],
    line: u32,
    start: u32,
    source_decl_scope: &SemanticScope,
    imported_scope: &SemanticScope,
) -> u32 {
    let Some(callee) = named_arg_enclosing_callee(source, source_lines, line, start) else {
        return SEMANTIC_TOKEN_TYPE_STATE;
    };
    if callee_is_proc_endpoint_target(&callee, source_decl_scope, imported_scope) {
        SEMANTIC_TOKEN_TYPE_PORT
    } else {
        SEMANTIC_TOKEN_TYPE_STATE
    }
}

fn named_arg_enclosing_callee(
    source: &str,
    source_lines: &[&str],
    line: u32,
    start: u32,
) -> Option<String> {
    let label_offset = source_offset_for_position(source_lines, line, start)?;
    let open_paren = enclosing_open_paren(source, label_offset)?;
    let callee_end = source[..open_paren].trim_end().len();
    let callee_start = source[..callee_end]
        .rfind(|ch: char| !is_callee_path_char(ch))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let callee = source[callee_start..callee_end].trim();
    (!callee.is_empty()).then(|| callee.to_owned())
}

fn source_offset_for_position(source_lines: &[&str], line: u32, start: u32) -> Option<usize> {
    let line_idx = line as usize;
    let start = start as usize;
    let line_text = source_lines.get(line_idx)?;
    if start > line_text.len() {
        return None;
    }
    let preceding_len = source_lines
        .iter()
        .take(line_idx)
        .map(|line| line.len() + 1)
        .sum::<usize>();
    Some(preceding_len + start)
}

fn enclosing_open_paren(source: &str, before_offset: usize) -> Option<usize> {
    let mut depth = 0_u32;
    for (idx, ch) in source[..before_offset].char_indices().rev() {
        match ch {
            ')' => depth = depth.saturating_add(1),
            '(' if depth == 0 => return Some(idx),
            '(' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn is_callee_path_char(ch: char) -> bool {
    ch == '_'
        || ch == ':'
        || ch == '.'
        || ch == '<'
        || ch == '>'
        || ch == '['
        || ch == ']'
        || ch.is_ascii_alphanumeric()
}

fn callee_is_proc_endpoint_target(
    callee: &str,
    source_decl_scope: &SemanticScope,
    imported_scope: &SemanticScope,
) -> bool {
    let callee = strip_type_args_from_callee(callee);
    if callee.ends_with(".init") || callee.ends_with("::init") {
        return true;
    }
    let leaf = callee
        .rsplit([':', '.'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(callee)
        .trim();
    source_decl_scope.types.contains(leaf)
        || imported_scope.types.contains(leaf)
        || leaf.chars().next().is_some_and(char::is_uppercase)
}

fn strip_type_args_from_callee(callee: &str) -> &str {
    callee.find('<').map(|idx| &callee[..idx]).unwrap_or(callee)
}

fn leading_indent_len(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

fn leading_identifier_len(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    let mut end = first.len_utf8();
    for (idx, ch) in chars {
        if is_identifier_continue_char(ch) {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    Some(end)
}

fn is_identifier_continue_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

pub(super) fn encode_semantic_tokens(tokens: &[SemanticToken]) -> Vec<u32> {
    let mut data = Vec::with_capacity(tokens.len() * 5);
    let mut prev_line = 0_u32;
    let mut prev_start = 0_u32;

    for token in tokens {
        let delta_line = token.line.saturating_sub(prev_line);
        let delta_start = if delta_line == 0 {
            token.start.saturating_sub(prev_start)
        } else {
            token.start
        };
        data.push(delta_line);
        data.push(delta_start);
        data.push(token.length);
        data.push(token.token_type);
        data.push(token.token_modifiers);
        prev_line = token.line;
        prev_start = token.start;
    }

    data
}

fn is_implicit_port_name(name: &str) -> bool {
    implicit_port_index(name, "in").is_some() || implicit_port_index(name, "out").is_some()
}

fn implicit_port_index(name: &str, prefix: &str) -> Option<u32> {
    let suffix = name.strip_prefix(prefix)?;
    let value = suffix.parse::<u32>().ok()?;
    if (1..=64).contains(&value) {
        Some(value)
    } else {
        None
    }
}

#[cfg(test)]
pub(super) fn collect_source_declaration_symbols(source: &str, scope: &mut SemanticScope) {
    source_fallback::collect_source_declaration_symbols(source, scope);
}

#[cfg(test)]
pub(super) fn is_reserved_word(name: &str) -> bool {
    source_fallback::is_reserved_word(name)
}

#[cfg(test)]
pub(super) fn collect_const_names(source: &str) -> HashSet<String> {
    source_fallback::collect_const_names(source)
}

#[cfg(test)]
mod tests;
