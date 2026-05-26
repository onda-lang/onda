use std::collections::HashSet;
use std::path::Path;

use onda_frontend::{parse_program, parse_program_with_path};

mod ast_index;
mod source_fallback;

use ast_index::{build_semantic_scope_index, collect_all_symbols, normalize_file_key_for_path};
use source_fallback::{build_source_scope_index, identifier_is_in_import_path, scan_identifiers};

pub(super) const SEMANTIC_TOKEN_TYPE_ENUM_MEMBER: u32 = 0;
pub(super) const SEMANTIC_TOKEN_TYPE_VARIABLE: u32 = 1;
pub(super) const SEMANTIC_TOKEN_TYPE_PORT: u32 = 2;
pub(super) const SEMANTIC_TOKEN_TYPE_PARAMETER: u32 = 3;
pub(super) const SEMANTIC_TOKEN_TYPE_FUNCTION: u32 = 4;
pub(super) const SEMANTIC_TOKEN_TYPE_TYPE: u32 = 5;
pub(super) const SEMANTIC_TOKEN_TYPE_NAMESPACE: u32 = 6;
pub(super) const SEMANTIC_TOKEN_TYPE_STATE: u32 = 7;
pub(super) const SEMANTIC_TOKEN_TYPE_KEYWORD: u32 = 8;

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
];

const BUILTIN_CONSTS: &[&str] = &[
    "PI",
    "pi",
    "TWO_PI",
    "TWOPI",
    "two_pi",
    "twopi",
    "SAMPLE_RATE",
    "SAMPLERATE",
    "SR",
    "sample_rate",
    "samplerate",
    "BLOCK_SIZE",
    "BLOCKSIZE",
    "BS",
    "block_size",
    "blocksize",
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
        if self.consts.contains(name) {
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
        if self.consts.contains(name) {
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

    fn innermost_scope_index(&self, line: u32, column: u32) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (idx, scope) in self.scopes.iter().enumerate() {
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

pub(super) fn semantic_tokens_for_document(
    source: &str,
    path: Option<&Path>,
) -> Vec<SemanticToken> {
    let parsed_program = match path {
        Some(path) => parse_program_with_path(source, path).ok(),
        None => parse_program(source).ok(),
    };
    let imported_scope = parsed_program
        .as_ref()
        .map(collect_all_symbols)
        .unwrap_or_default();
    let current_file_key = match path {
        Some(path) => normalize_file_key_for_path(path),
        None => Some(ast_index::normalize_file_key("<memory>")),
    };
    let scope_index = parsed_program
        .as_ref()
        .map(|program| build_semantic_scope_index(program, current_file_key.as_deref()));
    let source_scope_index = build_source_scope_index(source);
    let mut source_decl_scope = SemanticScope::default();
    let source_lines = source.lines().collect::<Vec<_>>();

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
            if is_named_arg_label {
                return;
            }
            if name == "self" {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: SEMANTIC_TOKEN_TYPE_PARAMETER,
                    token_modifiers: 0,
                });
                return;
            }
            if after_dot {
                tokens.push(SemanticToken {
                    line,
                    start,
                    length,
                    token_type: if is_call {
                        SEMANTIC_TOKEN_TYPE_FUNCTION
                    } else {
                        SEMANTIC_TOKEN_TYPE_VARIABLE
                    },
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

fn is_semantic_keyword(name: &str, source_lines: &[&str], line: u32, start: u32) -> bool {
    if name == "pin" {
        return true;
    }
    if !matches!(
        name,
        "if" | "elif"
            | "else"
            | "for"
            | "while"
            | "loop"
            | "break"
            | "continue"
            | "return"
            | "assert"
    ) {
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
