use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use onda_daemon::{DaemonSession, DocumentVersion};
use onda_frontend::{
    parse_program, parse_program_with_path, AssignTarget, Block, BlockExec, Diagnostic, EventDef,
    FunctionDef, ProcessorDef, Program, Span, Stmt,
};
use serde::Deserialize;
use serde_json::{json, Value};

const JSONRPC_VERSION: &str = "2.0";
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;
const SEMANTIC_TOKEN_TYPE_ENUM_MEMBER: u32 = 0;
const SEMANTIC_TOKEN_TYPE_VARIABLE: u32 = 1;
const SEMANTIC_TOKEN_TYPE_PORT: u32 = 2;
const SEMANTIC_TOKEN_TYPE_PARAMETER: u32 = 3;
const SEMANTIC_TOKEN_TYPE_FUNCTION: u32 = 4;
const SEMANTIC_TOKEN_TYPE_TYPE: u32 = 5;
const SEMANTIC_TOKEN_TYPE_NAMESPACE: u32 = 6;
const SEMANTIC_TOKEN_TYPE_STATE: u32 = 7;

pub fn run_stdio_loop() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut server = LspServer::default();

    loop {
        let Some(message) = read_lsp_message(&mut reader)? else {
            return Ok(());
        };

        match server.handle_message(message, &mut writer)? {
            LoopControl::Continue => {}
            LoopControl::ExitSuccess => return Ok(()),
            LoopControl::ExitFailure => {
                return Err("lsp exit received before shutdown".to_owned());
            }
        }
    }
}

enum LoopControl {
    Continue,
    ExitSuccess,
    ExitFailure,
}

#[derive(Default)]
struct LspServer {
    session: DaemonSession,
    shutdown_requested: bool,
    document_uris: HashMap<PathBuf, String>,
    published_by_entry: HashMap<PathBuf, HashSet<String>>,
}

impl LspServer {
    fn handle_message(
        &mut self,
        message: Value,
        writer: &mut impl Write,
    ) -> Result<LoopControl, String> {
        let envelope: ClientMessage = match serde_json::from_value(message) {
            Ok(envelope) => envelope,
            Err(err) => {
                eprintln!("onda lsp: failed to decode lsp message: {err}");
                return Ok(LoopControl::Continue);
            }
        };
        let request_id = envelope.id.clone();
        match self.handle_envelope(envelope, writer) {
            Ok(control) => Ok(control),
            Err(err) => {
                if let Some(id) = request_id {
                    let code = if err.starts_with("invalid params:") {
                        INVALID_PARAMS
                    } else {
                        INTERNAL_ERROR
                    };
                    write_error(writer, id, code, err)?;
                } else {
                    eprintln!("onda lsp: {err}");
                }
                Ok(LoopControl::Continue)
            }
        }
    }

    fn handle_envelope(
        &mut self,
        envelope: ClientMessage,
        writer: &mut impl Write,
    ) -> Result<LoopControl, String> {
        if envelope.jsonrpc.as_deref() != Some(JSONRPC_VERSION) {
            if let Some(id) = envelope.id {
                write_error(
                    writer,
                    id,
                    INTERNAL_ERROR,
                    "unsupported jsonrpc version".to_owned(),
                )?;
            }
            return Ok(LoopControl::Continue);
        }

        let Some(method) = envelope.method.as_deref() else {
            return Ok(LoopControl::Continue);
        };

        match method {
            "initialize" => {
                let params = parse_params::<InitializeParams>(envelope.params)?;
                let result = initialize_result(params.process_id);
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "initialized" | "$/cancelRequest" | "workspace/didChangeConfiguration" => {}
            "shutdown" => {
                self.shutdown_requested = true;
                write_result(writer, envelope.id.unwrap_or(Value::Null), Value::Null)?;
            }
            "exit" => {
                return Ok(if self.shutdown_requested {
                    LoopControl::ExitSuccess
                } else {
                    LoopControl::ExitFailure
                });
            }
            "textDocument/didOpen" => {
                let params = parse_params::<DidOpenTextDocumentParams>(envelope.params)?;
                let Some(path) =
                    lsp_document_path(&params.text_document.uri).map_err(invalid_params)?
                else {
                    return Ok(LoopControl::Continue);
                };
                let normalized = self.session.open_document(
                    &path,
                    DocumentVersion(params.text_document.version),
                    params.text_document.text,
                );
                self.document_uris
                    .insert(normalized.clone(), path_to_file_uri(&normalized));
                self.publish_diagnostics_for_entry(&normalized, writer)?;
            }
            "textDocument/didChange" => {
                let params = parse_params::<DidChangeTextDocumentParams>(envelope.params)?;
                let Some(text) = latest_full_text(&params.content_changes) else {
                    return Ok(LoopControl::Continue);
                };
                let Some(path) =
                    lsp_document_path(&params.text_document.uri).map_err(invalid_params)?
                else {
                    return Ok(LoopControl::Continue);
                };
                let normalized = self.session.update_document(
                    &path,
                    DocumentVersion(params.text_document.version),
                    text,
                );
                self.document_uris
                    .insert(normalized.clone(), path_to_file_uri(&normalized));
            }
            "textDocument/didSave" => {
                let params = parse_params::<DidSaveTextDocumentParams>(envelope.params)?;
                let Some(path) =
                    lsp_document_path(&params.text_document.uri).map_err(invalid_params)?
                else {
                    return Ok(LoopControl::Continue);
                };
                if let Some(text) = params.text {
                    let version = self
                        .session
                        .analysis()
                        .document(&path)
                        .map(|doc| doc.version)
                        .unwrap_or(DocumentVersion(0));
                    let normalized = self.session.update_document(&path, version, text);
                    self.document_uris
                        .insert(normalized.clone(), path_to_file_uri(&normalized));
                }
                self.publish_diagnostics_for_entry(&path, writer)?;
            }
            "textDocument/didClose" => {
                let params = parse_params::<DidCloseTextDocumentParams>(envelope.params)?;
                let Some(path) =
                    lsp_document_path(&params.text_document.uri).map_err(invalid_params)?
                else {
                    return Ok(LoopControl::Continue);
                };
                self.session.close_document(&path);
                self.document_uris.remove(&path);
                self.clear_entry_diagnostics(&path, writer)?;
            }
            "textDocument/semanticTokens/full" => {
                let params = parse_params::<SemanticTokensParams>(envelope.params)?;
                let result = self.semantic_tokens_for_uri(&params.text_document.uri)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            _ => {
                if let Some(id) = envelope.id {
                    write_error(
                        writer,
                        id,
                        METHOD_NOT_FOUND,
                        format!("unknown method '{method}'"),
                    )?;
                }
            }
        }

        Ok(LoopControl::Continue)
    }

    fn semantic_tokens_for_uri(&self, uri: &str) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(json!({ "data": [] }));
        };
        let source = self.source_text_for_path(&path)?;
        let tokens = semantic_tokens_for_document(&source, Some(&path));
        Ok(json!({
            "data": encode_semantic_tokens(&tokens),
        }))
    }

    fn source_text_for_path(&self, path: &Path) -> Result<String, String> {
        if let Some(document) = self.session.analysis().document(path) {
            return Ok(document.text.clone());
        }
        fs::read_to_string(path)
            .map_err(|err| format!("failed to read source '{}': {err}", path.display()))
    }

    fn publish_diagnostics_for_entry(
        &mut self,
        entry_path: &Path,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let snapshot = self.session.analyze_document(entry_path);
        let entry_path = normalize_path(entry_path);
        let default_uri = self
            .document_uris
            .get(&entry_path)
            .cloned()
            .unwrap_or_else(|| path_to_file_uri(&entry_path));
        let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();

        for diagnostic in &snapshot.diagnostics {
            let uri = diagnostic_uri(diagnostic, &default_uri)?;
            grouped
                .entry(uri)
                .or_default()
                .push(diagnostic_to_lsp(diagnostic));
        }

        let previous = self
            .published_by_entry
            .remove(&entry_path)
            .unwrap_or_default();
        let mut current = HashSet::new();

        for (uri, diagnostics) in grouped {
            write_notification(
                writer,
                "textDocument/publishDiagnostics",
                json!({
                    "uri": uri,
                    "diagnostics": diagnostics,
                }),
            )?;
            current.insert(uri);
        }

        if !current.contains(&default_uri) {
            write_notification(
                writer,
                "textDocument/publishDiagnostics",
                json!({
                    "uri": default_uri,
                    "diagnostics": [],
                }),
            )?;
            current.insert(default_uri);
        }

        for uri in previous.difference(&current) {
            write_notification(
                writer,
                "textDocument/publishDiagnostics",
                json!({
                    "uri": uri,
                    "diagnostics": [],
                }),
            )?;
        }

        self.published_by_entry.insert(entry_path, current);
        Ok(())
    }

    fn clear_entry_diagnostics(
        &mut self,
        entry_path: &Path,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let entry_path = normalize_path(entry_path);
        let mut cleared = self
            .published_by_entry
            .remove(&entry_path)
            .unwrap_or_default();
        cleared.insert(path_to_file_uri(&entry_path));
        for uri in cleared {
            write_notification(
                writer,
                "textDocument/publishDiagnostics",
                json!({
                    "uri": uri,
                    "diagnostics": [],
                }),
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ClientMessage {
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    #[serde(default)]
    process_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidOpenTextDocumentParams {
    text_document: VersionedTextDocumentItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionedTextDocumentItem {
    uri: String,
    version: i32,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidChangeTextDocumentParams {
    text_document: VersionedTextDocumentIdentifier,
    content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Deserialize)]
struct VersionedTextDocumentIdentifier {
    uri: String,
    version: i32,
}

#[derive(Debug, Deserialize)]
struct TextDocumentContentChangeEvent {
    #[serde(default)]
    range: Option<Value>,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidSaveTextDocumentParams {
    text_document: TextDocumentIdentifier,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidCloseTextDocumentParams {
    text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
struct TextDocumentIdentifier {
    uri: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticTokensParams {
    text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SemanticToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    token_modifiers: u32,
}

fn latest_full_text(changes: &[TextDocumentContentChangeEvent]) -> Option<String> {
    changes
        .iter()
        .rev()
        .find(|change| change.range.is_none())
        .map(|change| change.text.clone())
}

fn parse_params<T>(params: Option<Value>) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let value = params.unwrap_or(Value::Null);
    serde_json::from_value(value).map_err(|err| format!("invalid params: {err}"))
}

fn initialize_result(process_id: Option<u32>) -> Value {
    let _ = process_id;
    json!({
        "capabilities": {
            "textDocumentSync": {
                "openClose": true,
                "change": 1,
                "save": {
                    "includeText": false,
                },
            },
            "semanticTokensProvider": {
                "full": true,
                "legend": {
                    "tokenTypes": ["enumMember", "variable", "port", "parameter", "function", "type", "namespace", "state"],
                    "tokenModifiers": [],
                }
            },
        },
        "serverInfo": {
            "name": "onda",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn diagnostic_uri(diagnostic: &Diagnostic, default_uri: &str) -> Result<String, String> {
    let Some(file) = diagnostic.file.as_deref() else {
        return Ok(default_uri.to_owned());
    };
    if file.starts_with('<') {
        return Ok(default_uri.to_owned());
    }
    Ok(path_to_file_uri(&normalize_path(Path::new(file))))
}

fn diagnostic_to_lsp(diagnostic: &Diagnostic) -> Value {
    json!({
        "range": diagnostic_range(diagnostic),
        "severity": 1,
        "source": "onda",
        "code": diagnostic.code as i32,
        "message": diagnostic_message(diagnostic),
    })
}

fn diagnostic_range(diagnostic: &Diagnostic) -> Value {
    let start_line = diagnostic.line.saturating_sub(1);
    let start_character = diagnostic.column.saturating_sub(1);
    let mut end_line = diagnostic.end_line.saturating_sub(1);
    let mut end_character = diagnostic.end_column.saturating_sub(1);

    if diagnostic.end_line == 0 {
        end_line = start_line;
    }
    if end_line == start_line && end_character <= start_character {
        end_character = start_character.saturating_add(1);
    }

    json!({
        "start": {
            "line": start_line,
            "character": start_character,
        },
        "end": {
            "line": end_line,
            "character": end_character,
        }
    })
}

fn diagnostic_message(diagnostic: &Diagnostic) -> String {
    if diagnostic.trace.is_empty() {
        return diagnostic.message.clone();
    }
    format!(
        "{}\ntrace:\n{}",
        diagnostic.message,
        diagnostic
            .trace
            .iter()
            .rev()
            .map(|trace| format!("- {trace}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn semantic_tokens_for_document(source: &str, path: Option<&Path>) -> Vec<SemanticToken> {
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
        None => Some(normalize_file_key("<memory>")),
    };
    let scope_index = parsed_program
        .as_ref()
        .map(|program| build_semantic_scope_index(program, current_file_key.as_deref()));
    let mut source_scope = SemanticScope::default();

    collect_symbols_from_source(source, &mut source_scope);

    let mut tokens = Vec::new();
    scan_identifiers(
        source,
        |name, line, start, length, after_dot, is_call, in_ns_path| {
            // self → parameter color (same as other arguments)
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
                if is_call {
                    tokens.push(SemanticToken {
                        line,
                        start,
                        length,
                        token_type: SEMANTIC_TOKEN_TYPE_FUNCTION,
                        token_modifiers: 0,
                    });
                } else {
                    tokens.push(SemanticToken {
                        line,
                        start,
                        length,
                        token_type: SEMANTIC_TOKEN_TYPE_VARIABLE,
                        token_modifiers: 0,
                    });
                }
                return;
            }
            // Namespace path segments: std::fft, std::complex::Complex, etc.
            if in_ns_path {
                let token_type = source_scope
                    .token_type_for_source_fallback(name)
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
            let resolved_token_type = match scope_resolution {
                Some((token_type, true)) => token_type
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .or_else(|| source_scope.imported_token_type_for(name)),
                Some((token_type, false)) => token_type
                    .or_else(|| imported_scope.imported_token_type_for(name))
                    .or_else(|| source_scope.imported_token_type_for(name))
                    .or_else(|| source_scope.token_type_for_source_fallback(name)),
                None => source_scope
                    .token_type_for_source_fallback(name)
                    .or_else(|| imported_scope.imported_token_type_for(name)),
            };
            if let Some(mut token_type) = resolved_token_type {
                // Variables called with () are callable proc instances — color as function
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
            } else if is_call && !is_reserved_word(name) {
                // Unknown identifier followed by () → intrinsic/function call
                // (skip keywords like if/while/for and type casts like i32/f32)
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

fn is_reserved_word(name: &str) -> bool {
    matches!(
        name,
        "if" | "elif"
            | "else"
            | "for"
            | "in"
            | "while"
            | "loop"
            | "break"
            | "continue"
            | "return"
            | "assert"
            | "true"
            | "false"
            | "f32"
            | "f64"
            | "i32"
            | "i64"
            | "bool"
            | "buffer"
            | "proc"
            | "processor"
            | "struct"
            | "def"
            | "const"
            | "namespace"
            | "import"
            | "include"
            | "ins"
            | "outs"
            | "params"
            | "buffers"
            | "init"
            | "event"
            | "events"
            | "sample"
            | "block"
            | "graph"
    )
}

#[derive(Debug, Clone, Default)]
struct SemanticScope {
    consts: HashSet<String>,
    types: HashSet<String>,
    functions: HashSet<String>,
    state_variables: HashSet<String>,
    variables: HashSet<String>,
    ports: HashSet<String>,
    parameters: HashSet<String>,
}

impl SemanticScope {
    fn token_type_for(&self, name: &str) -> Option<u32> {
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

    fn token_type_for_source_fallback(&self, name: &str) -> Option<u32> {
        self.token_type_for(name).or_else(|| {
            if is_implicit_port_name(name) {
                Some(SEMANTIC_TOKEN_TYPE_PORT)
            } else {
                None
            }
        })
    }

    fn insert_variable(&mut self, name: String) {
        if !self.consts.contains(&name)
            && !self.ports.contains(&name)
            && !self.parameters.contains(&name)
            && !self.types.contains(&name)
        {
            self.variables.insert(name);
        }
    }

    fn insert_state_variable(&mut self, name: String) {
        if !self.consts.contains(&name)
            && !self.ports.contains(&name)
            && !self.parameters.contains(&name)
            && !self.types.contains(&name)
        {
            self.state_variables.insert(name);
        }
    }

    fn imported_token_type_for(&self, name: &str) -> Option<u32> {
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
struct SemanticScopeIndex {
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
    fn token_type_for(&self, name: &str, line: u32, column: u32) -> (Option<u32>, bool) {
        let containing_scope = self.innermost_scope_index(line, column);
        let token_type = containing_scope
            .and_then(|idx| self.resolve_scope_chain(idx, name))
            .or_else(|| self.document_scope.token_type_for(name));
        (token_type, containing_scope.is_some())
    }

    fn push_scope(&mut self, parent: Option<usize>, span: Span) -> Option<usize> {
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
        span: Span,
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

fn build_semantic_scope_index(
    program: &Program,
    current_file_key: Option<&str>,
) -> SemanticScopeIndex {
    let mut index = SemanticScopeIndex::default();
    for &name in BUILTIN_CONSTS {
        index.document_scope.consts.insert(name.to_owned());
    }

    let current_blocks = program
        .blocks
        .iter()
        .filter(|block| block_belongs_to_current_file(block, current_file_key))
        .collect::<Vec<_>>();

    for block in &current_blocks {
        collect_document_scope_symbols(block, &mut index.document_scope);
    }

    build_top_level_runtime_scope(&mut index, &current_blocks);

    for block in current_blocks {
        match block {
            Block::Def(def) => build_function_scope(&mut index, None, def),
            Block::Proc(proc_def) => build_proc_scope(&mut index, proc_def),
            Block::Struct(struct_def) => {
                let Some(owner_idx) = index.push_scope(None, struct_def.loc) else {
                    continue;
                };
                for tp in &struct_def.type_params {
                    index.scopes[owner_idx].scope.types.insert(tp.clone());
                }
                for method in &struct_def.methods {
                    build_function_scope(&mut index, Some(owner_idx), method);
                }
            }
            _ => {}
        }
    }

    index
}

fn normalize_file_key_for_path(path: &Path) -> Option<String> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Some(normalize_file_key(&canonical.to_string_lossy()))
}

fn normalize_file_key(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized
        .strip_prefix("//?/")
        .or_else(|| normalized.strip_prefix("\\\\?\\"))
        .unwrap_or(&normalized);
    normalized.to_ascii_lowercase()
}

fn block_belongs_to_current_file(block: &Block, current_file_key: Option<&str>) -> bool {
    span_belongs_to_current_file(block.loc().span(), current_file_key)
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

fn collect_document_scope_symbols(block: &Block, scope: &mut SemanticScope) {
    match block {
        Block::Const(decl) => {
            scope.consts.insert(decl.name.clone());
        }
        Block::Def(def) => {
            scope.functions.insert(def.name.clone());
        }
        Block::Proc(proc_def) => {
            scope.types.insert(proc_def.name.clone());
        }
        Block::Struct(struct_def) => {
            scope.types.insert(struct_def.name.clone());
            for method in &struct_def.methods {
                scope.functions.insert(method.name.clone());
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
struct RuntimeStmtRegion<'a> {
    span: Span,
    body: &'a [Stmt],
    collect_state_symbols: bool,
}

#[derive(Default)]
struct TopLevelRuntimeSections<'a> {
    span: Option<Span>,
    ports: Vec<String>,
    parameters: Vec<String>,
    stmt_regions: Vec<RuntimeStmtRegion<'a>>,
    events: Vec<&'a EventDef>,
}

fn build_top_level_runtime_scope(index: &mut SemanticScopeIndex, blocks: &[&Block]) -> Option<usize> {
    let sections = collect_top_level_runtime_sections(blocks);
    let span = sections.span?;
    let owner_idx = create_runtime_owner_scope(index, None, span, true)?;
    {
        let owner = &mut index.scopes[owner_idx].scope;
        for name in sections.ports {
            owner.ports.insert(name);
        }
        for name in sections.parameters {
            owner.parameters.insert(name);
        }
    }
    build_runtime_stmt_regions(index, owner_idx, &sections.stmt_regions);
    for event in sections.events {
        build_event_scope(index, owner_idx, event);
    }
    Some(owner_idx)
}

fn collect_top_level_runtime_sections<'a>(blocks: &[&'a Block]) -> TopLevelRuntimeSections<'a> {
    let mut sections = TopLevelRuntimeSections::default();

    for block in blocks {
        match block {
            Block::Ins(ports) | Block::Outs(ports) => {
                extend_runtime_owner_span(&mut sections.span, block.loc().span());
                sections
                    .ports
                    .extend(ports.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Params(params) => {
                extend_runtime_owner_span(&mut sections.span, block.loc().span());
                sections
                    .parameters
                    .extend(params.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Buffers(buffers) => {
                extend_runtime_owner_span(&mut sections.span, block.loc().span());
                sections
                    .ports
                    .extend(buffers.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Init(init) => {
                extend_runtime_owner_span(&mut sections.span, init.loc);
                sections.stmt_regions.push(RuntimeStmtRegion {
                    span: init.loc,
                    body: &init.body,
                    collect_state_symbols: true,
                });
            }
            Block::Block(exec) => {
                extend_runtime_owner_span(&mut sections.span, exec.loc);
                sections.stmt_regions.extend(runtime_regions_for_block_exec(exec));
            }
            Block::Sample(sample) => {
                extend_runtime_owner_span(&mut sections.span, sample.loc);
                sections.stmt_regions.push(RuntimeStmtRegion {
                    span: sample.loc,
                    body: &sample.body,
                    collect_state_symbols: false,
                });
            }
            Block::Events(events) => {
                extend_runtime_owner_span(&mut sections.span, events.loc);
                sections.events.extend(events.events.iter());
            }
            Block::Graph(graph) => {
                extend_runtime_owner_span(&mut sections.span, graph.loc);
            }
            _ => {}
        }
    }

    sections
}

fn create_runtime_owner_scope(
    index: &mut SemanticScopeIndex,
    parent: Option<usize>,
    span: Span,
    allows_implicit_ports: bool,
) -> Option<usize> {
    let scope_idx = index.push_scope(parent, span)?;
    index.scopes[scope_idx].allows_implicit_ports = allows_implicit_ports;
    Some(scope_idx)
}

fn extend_runtime_owner_span(span: &mut Option<Span>, next: Span) {
    *span = Some(match *span {
        Some(current) => Span::spanning(current, next),
        None => next,
    });
}

fn runtime_regions_for_block_exec<'a>(exec: &'a BlockExec) -> Vec<RuntimeStmtRegion<'a>> {
    let mut regions = Vec::new();
    if let Some(span) = span_for_stmt_body(&exec.pre) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &exec.pre,
            collect_state_symbols: true,
        });
    }
    if let Some(sample) = &exec.sample {
        regions.push(RuntimeStmtRegion {
            span: sample.loc,
            body: &sample.body,
            collect_state_symbols: false,
        });
    }
    if let Some(span) = span_for_stmt_body(&exec.post) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &exec.post,
            collect_state_symbols: true,
        });
    }
    regions
}

fn build_proc_scope(index: &mut SemanticScopeIndex, proc_def: &ProcessorDef) {
    let Some(owner_idx) = create_runtime_owner_scope(index, None, span_for_proc_scope(proc_def), true)
    else {
        return;
    };
    {
        let owner = &mut index.scopes[owner_idx].scope;
        for tp in &proc_def.type_params {
            owner.types.insert(tp.clone());
        }
        for decl in &proc_def.ins {
            owner.ports.insert(decl.name.clone());
        }
        for decl in &proc_def.outs {
            owner.ports.insert(decl.name.clone());
        }
        for decl in &proc_def.params {
            owner.parameters.insert(decl.name.clone());
        }
        for buffer in &proc_def.buffers {
            owner.ports.insert(buffer.name.clone());
        }
        for def in &proc_def.local_defs {
            owner.functions.insert(def.name.clone());
        }
    }
    let stmt_regions = proc_runtime_stmt_regions(proc_def);
    build_runtime_stmt_regions(index, owner_idx, &stmt_regions);

    for event in &proc_def.events {
        build_event_scope(index, owner_idx, event);
    }
    for def in &proc_def.local_defs {
        build_function_scope(index, Some(owner_idx), def);
    }
}

fn proc_runtime_stmt_regions<'a>(proc_def: &'a ProcessorDef) -> Vec<RuntimeStmtRegion<'a>> {
    let mut regions = Vec::new();
    regions.push(RuntimeStmtRegion {
        span: proc_def.init.loc,
        body: &proc_def.init.body,
        collect_state_symbols: true,
    });
    if let Some(span) = span_for_stmt_body(&proc_def.block_pre) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &proc_def.block_pre,
            collect_state_symbols: true,
        });
    }
    if let Some(span) = span_for_stmt_body(&proc_def.sample) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &proc_def.sample,
            collect_state_symbols: false,
        });
    }
    if let Some(span) = span_for_stmt_body(&proc_def.block_post) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &proc_def.block_post,
            collect_state_symbols: true,
        });
    }
    regions
}

fn build_function_scope(index: &mut SemanticScopeIndex, parent: Option<usize>, def: &FunctionDef) {
    let Some(scope_idx) = index.push_scope(parent, span_for_function_scope(def)) else {
        return;
    };
    let reserved = parent
        .map(|idx| index.scopes[idx].scope.clone())
        .unwrap_or_default();
    let allows_implicit_ports = index.scopes[scope_idx].allows_implicit_ports;
    {
        let scope = &mut index.scopes[scope_idx].scope;
        scope.functions.insert(def.name.clone());
        for tp in &def.type_params {
            scope.types.insert(tp.clone());
        }
        for param in &def.params {
            scope.parameters.insert(param.name.clone());
        }
        collect_stmt_symbols(&def.body, scope);
        prune_shadowed_variables(scope, &reserved, allows_implicit_ports);
    }
}

fn build_event_scope(
    index: &mut SemanticScopeIndex,
    parent: usize,
    event: &onda_frontend::EventDef,
) {
    let Some(scope_idx) = index.push_scope(Some(parent), span_for_event_scope(event)) else {
        return;
    };
    let reserved = index.scopes[parent].scope.clone();
    let allows_implicit_ports = index.scopes[scope_idx].allows_implicit_ports;
    {
        let scope = &mut index.scopes[scope_idx].scope;
        scope.functions.insert(event.name.clone());
        for param in &event.params {
            scope.parameters.insert(param.name.clone());
        }
        collect_stmt_symbols(&event.body, scope);
        prune_shadowed_variables(scope, &reserved, allows_implicit_ports);
    }
}

fn build_stmt_scope(index: &mut SemanticScopeIndex, parent: usize, span: Span, stmts: &[Stmt]) {
    let Some(scope_idx) = index.push_scope(Some(parent), span) else {
        return;
    };
    let reserved = index.scopes[parent].scope.clone();
    let allows_implicit_ports = index.scopes[scope_idx].allows_implicit_ports;
    {
        let scope = &mut index.scopes[scope_idx].scope;
        collect_stmt_symbols(stmts, scope);
        prune_shadowed_variables(scope, &reserved, allows_implicit_ports);
    }
}

fn build_runtime_stmt_regions(
    index: &mut SemanticScopeIndex,
    owner_idx: usize,
    stmt_regions: &[RuntimeStmtRegion<'_>],
) {
    for region in stmt_regions {
        if region.collect_state_symbols {
            collect_runtime_state_symbols(region.body, &mut index.scopes[owner_idx].scope);
        }
        if !region.body.is_empty() {
            build_stmt_scope(index, owner_idx, region.span, region.body);
        }
    }
}

fn span_for_stmt(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::If {
            loc,
            then_branch,
            else_branch,
            ..
        } => {
            let mut span = *loc;
            if let Some(branch_span) = span_for_stmt_body(then_branch) {
                span = Span::spanning(span, branch_span);
            }
            if let Some(branch_span) = span_for_stmt_body(else_branch) {
                span = Span::spanning(span, branch_span);
            }
            span
        }
        Stmt::For { loc, body, .. } | Stmt::While { loc, body, .. } => {
            let mut span = *loc;
            if let Some(body_span) = span_for_stmt_body(body) {
                span = Span::spanning(span, body_span);
            }
            span
        }
        _ => stmt.loc().span(),
    }
}

fn span_for_stmt_body(stmts: &[Stmt]) -> Option<Span> {
    let mut iter = stmts.iter();
    let first = span_for_stmt(iter.next()?);
    Some(iter.fold(first, |span, stmt| Span::spanning(span, span_for_stmt(stmt))))
}

fn span_for_function_scope(def: &FunctionDef) -> Span {
    span_for_stmt_body(&def.body)
        .map(|body_span| Span::spanning(def.loc, body_span))
        .unwrap_or(def.loc)
}

fn span_for_event_scope(event: &onda_frontend::EventDef) -> Span {
    span_for_stmt_body(&event.body)
        .map(|body_span| Span::spanning(event.loc, body_span))
        .unwrap_or(event.loc)
}

fn span_for_proc_scope(proc_def: &onda_frontend::ProcessorDef) -> Span {
    let mut span = proc_def.loc;

    for decl in &proc_def.consts {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.ins {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.outs {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.params {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.buffers {
        span = Span::spanning(span, decl.loc);
    }

    span = Span::spanning(span, proc_def.init.loc);
    if let Some(body_span) = span_for_stmt_body(&proc_def.block_pre) {
        span = Span::spanning(span, body_span);
    }
    if let Some(body_span) = span_for_stmt_body(&proc_def.sample) {
        span = Span::spanning(span, body_span);
    }
    if let Some(body_span) = span_for_stmt_body(&proc_def.block_post) {
        span = Span::spanning(span, body_span);
    }
    if let Some(graph) = &proc_def.graph {
        span = Span::spanning(span, graph.loc);
    }
    for event in &proc_def.events {
        span = Span::spanning(span, span_for_event_scope(event));
    }
    for def in &proc_def.local_defs {
        span = Span::spanning(span, span_for_function_scope(def));
    }

    span
}

fn collect_runtime_state_symbols(stmts: &[Stmt], scope: &mut SemanticScope) {
    let mut collected = SemanticScope::default();
    collect_state_stmt_symbols(stmts, &mut collected, 0);
    scope.state_variables.extend(collected.state_variables);
}

fn prune_shadowed_variables(
    scope: &mut SemanticScope,
    reserved: &SemanticScope,
    allows_implicit_ports: bool,
) {
    scope.variables.retain(|name| {
        reserved.token_type_for(name).is_none()
            && (!allows_implicit_ports || !is_implicit_port_name(name))
    });
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

fn collect_all_symbols(program: &Program) -> SemanticScope {
    let mut scope = SemanticScope::default();
    for block in &program.blocks {
        collect_block_symbols(block, &mut scope);
    }
    scope
}

fn collect_block_symbols(block: &Block, scope: &mut SemanticScope) {
    match block {
        Block::Const(decl) => {
            scope.consts.insert(decl.name.clone());
        }
        Block::Ins(ports) | Block::Outs(ports) => {
            for decl in &ports.decls {
                scope.ports.insert(decl.name.clone());
            }
        }
        Block::Params(params) => {
            for decl in &params.decls {
                scope.parameters.insert(decl.name.clone());
            }
        }
        Block::Buffers(buffers) => {
            for decl in &buffers.decls {
                scope.ports.insert(decl.name.clone());
            }
        }
        Block::Events(events) => {
            for event in &events.events {
                scope.functions.insert(event.name.clone());
                for param in &event.params {
                    scope.parameters.insert(param.name.clone());
                }
                collect_stmt_symbols(&event.body, scope);
            }
        }
        Block::Init(init) => {
            collect_state_stmt_symbols(&init.body, scope, 0);
        }
        Block::Block(exec) => {
            collect_stmt_symbols(&exec.pre, scope);
            if let Some(sample) = &exec.sample {
                collect_stmt_symbols(&sample.body, scope);
            }
            collect_stmt_symbols(&exec.post, scope);
        }
        Block::Sample(sample) => {
            collect_stmt_symbols(&sample.body, scope);
        }
        Block::Def(def) => {
            scope.functions.insert(def.name.clone());
            collect_def_symbols(def, scope);
        }
        Block::Proc(proc_def) => {
            scope.types.insert(proc_def.name.clone());
            collect_proc_symbols(proc_def, scope);
        }
        Block::Struct(struct_def) => {
            scope.types.insert(struct_def.name.clone());
            for tp in &struct_def.type_params {
                scope.types.insert(tp.clone());
            }
            for method in &struct_def.methods {
                scope.functions.insert(method.name.clone());
                collect_def_symbols(method, scope);
            }
        }
        _ => {}
    }
}

fn collect_proc_symbols(proc_def: &onda_frontend::ProcessorDef, scope: &mut SemanticScope) {
    for tp in &proc_def.type_params {
        scope.types.insert(tp.clone());
    }
    for decl in &proc_def.ins {
        scope.ports.insert(decl.name.clone());
    }
    for decl in &proc_def.outs {
        scope.ports.insert(decl.name.clone());
    }
    for decl in &proc_def.params {
        scope.parameters.insert(decl.name.clone());
    }
    for buffer in &proc_def.buffers {
        scope.ports.insert(buffer.name.clone());
    }
    collect_state_stmt_symbols(&proc_def.init.body, scope, 0);
    collect_stmt_symbols(&proc_def.block_pre, scope);
    collect_stmt_symbols(&proc_def.sample, scope);
    collect_stmt_symbols(&proc_def.block_post, scope);
    for event in &proc_def.events {
        scope.functions.insert(event.name.clone());
        for param in &event.params {
            scope.parameters.insert(param.name.clone());
        }
        collect_stmt_symbols(&event.body, scope);
    }
    for def in &proc_def.local_defs {
        scope.functions.insert(def.name.clone());
        collect_def_symbols(def, scope);
    }
}

fn collect_def_symbols(def: &FunctionDef, scope: &mut SemanticScope) {
    for tp in &def.type_params {
        scope.types.insert(tp.clone());
    }
    for param in &def.params {
        scope.parameters.insert(param.name.clone());
    }
    collect_stmt_symbols(&def.body, scope);
}

fn collect_stmt_symbols(stmts: &[Stmt], scope: &mut SemanticScope) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => {
                scope.consts.insert(decl.name.clone());
            }
            Stmt::Assign { target, .. } => {
                collect_target_symbols(target, scope);
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_stmt_symbols(then_branch, scope);
                collect_stmt_symbols(else_branch, scope);
            }
            Stmt::For { var, body, .. } => {
                scope.variables.insert(var.clone());
                collect_stmt_symbols(body, scope);
            }
            Stmt::While { body, .. } => {
                collect_stmt_symbols(body, scope);
            }
            _ => {}
        }
    }
}

fn collect_state_stmt_symbols(stmts: &[Stmt], scope: &mut SemanticScope, scope_depth: usize) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => {
                scope.consts.insert(decl.name.clone());
            }
            Stmt::Assign { target, .. } => {
                if scope_depth == 0 {
                    collect_state_target_symbols(target, scope);
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_state_stmt_symbols(then_branch, scope, scope_depth + 1);
                collect_state_stmt_symbols(else_branch, scope, scope_depth + 1);
            }
            Stmt::For { body, .. } => {
                collect_state_stmt_symbols(body, scope, scope_depth + 1);
            }
            Stmt::While { body, .. } => {
                collect_state_stmt_symbols(body, scope, scope_depth + 1);
            }
            _ => {}
        }
    }
}

fn collect_target_symbols(target: &AssignTarget, scope: &mut SemanticScope) {
    match target {
        AssignTarget::Var(name) => {
            scope.insert_variable(name.clone());
        }
        AssignTarget::Tuple(names) => {
            for name in names {
                scope.insert_variable(name.clone());
            }
        }
        _ => {}
    }
}

fn collect_state_target_symbols(target: &AssignTarget, scope: &mut SemanticScope) {
    match target {
        AssignTarget::Var(name) => {
            scope.insert_state_variable(name.clone());
        }
        AssignTarget::Tuple(names) => {
            for name in names {
                scope.insert_state_variable(name.clone());
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SourceSection {
    None,
    Ins,
    Outs,
    Params,
    Buffers,
    Init,
    Block,
    Sample,
    Events,
    Def,
    EventHandler,
}

/// Collect symbols directly from source text. This handles namespaced files where
/// the parser lowers procs into structs+defs, losing the original structure.
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

fn collect_symbols_from_source(source: &str, scope: &mut SemanticScope) {
    for &name in BUILTIN_CONSTS {
        scope.consts.insert(name.to_owned());
    }

    let mut section = SourceSection::None;
    let mut section_indent: usize = 0;
    // Track the events section so we can restore it when leaving an event handler
    let mut events_indent: Option<usize> = None;

    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // If we've dedented back to or past the section start, leave it
        if section != SourceSection::None && indent <= section_indent {
            // When leaving an EventHandler, check if we're still within the events section
            if section == SourceSection::EventHandler {
                if let Some(ev_indent) = events_indent {
                    if indent > ev_indent {
                        section = SourceSection::Events;
                        section_indent = ev_indent;
                    } else {
                        section = SourceSection::None;
                        events_indent = None;
                    }
                } else {
                    section = SourceSection::None;
                }
            } else {
                if section == SourceSection::Events {
                    events_indent = None;
                }
                section = SourceSection::None;
            }
        }

        // Namespace generic params: namespace name<Param = val, ...>
        if trimmed.starts_with("namespace ") {
            let rest = &trimmed["namespace ".len()..];
            // Register each segment of the namespace path (before < or :)
            let path_end = rest
                .find(|c: char| c == '<' || c == ':' || c == '=')
                .unwrap_or(rest.len());
            let path = rest[..path_end].trim();
            for segment in path.split("::") {
                let segment = segment.trim();
                if let Some(name) = extract_leading_ident(segment) {
                    scope.types.insert(name.to_owned());
                }
            }
            extract_namespace_generic_consts(trimmed, scope);
            section = SourceSection::None;
            section_indent = indent;
            continue;
        }

        // const Name = ...
        if let Some(rest) = trimmed.strip_prefix("const ") {
            if let Some(name) = extract_leading_ident(rest) {
                scope.consts.insert(name.to_owned());
            }
            continue;
        }

        // proc/struct Name<T, ...>:
        if trimmed.starts_with("proc ")
            || trimmed.starts_with("processor ")
            || trimmed.starts_with("struct ")
        {
            // Extract the name after the keyword
            let after_kw = if trimmed.starts_with("processor ") {
                &trimmed["processor ".len()..]
            } else if trimmed.starts_with("proc ") {
                &trimmed["proc ".len()..]
            } else {
                &trimmed["struct ".len()..]
            };
            if let Some(name) = extract_leading_ident(after_kw.trim_start()) {
                scope.types.insert(name.to_owned());
            }
            extract_type_params(trimmed, scope);
            section = SourceSection::None;
            section_indent = indent;
            continue;
        }

        // def Name<T>(params):
        if trimmed.starts_with("def ") {
            extract_def_symbols(trimmed, scope);
            section = SourceSection::Def;
            section_indent = indent;
            continue;
        }

        // event Name(params):
        if trimmed.starts_with("event ") {
            extract_event_symbols(trimmed, scope);
            section = SourceSection::EventHandler;
            section_indent = indent;
            continue;
        }

        // Section headers: ins/outs/params/buffers/init/events/sample/block/graph
        if let Some(new_section) = detect_section_header(trimmed) {
            section = new_section;
            section_indent = indent;
            if section == SourceSection::Events {
                events_indent = Some(indent);
            }
            // Handle ins<T> N / outs<T> N — extract type params
            if matches!(section, SourceSection::Ins | SourceSection::Outs) {
                extract_type_params(trimmed, scope);
            }
            continue;
        }

        // Event handler: name(params): — inside events section
        if section == SourceSection::Events && indent > section_indent {
            if let Some(paren) = trimmed.find('(') {
                let name = &trimmed[..paren];
                if !name.is_empty()
                    && is_ident_start(name.as_bytes()[0])
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    scope.functions.insert(name.to_owned());
                    extract_params_from_parens(&trimmed[paren..], scope);
                    section = SourceSection::EventHandler;
                    section_indent = indent;
                    continue;
                }
            }
        }

        // Within port/param/buffer sections: each line declares a name
        match section {
            SourceSection::Ins | SourceSection::Outs | SourceSection::Buffers => {
                if indent > section_indent {
                    if let Some(name) = extract_leading_ident(trimmed) {
                        scope.ports.insert(name.to_owned());
                    }
                }
            }
            SourceSection::Params => {
                if indent > section_indent {
                    if let Some(name) = extract_leading_ident(trimmed) {
                        scope.parameters.insert(name.to_owned());
                    }
                }
            }
            _ => {}
        }

        // for NAME in
        if let Some(rest) = trimmed.strip_prefix("for ") {
            if let Some(in_pos) = rest.find(" in ") {
                let name = rest[..in_pos].trim();
                if !name.is_empty() && is_ident_start(name.as_bytes()[0]) {
                    scope.insert_variable(name.to_owned());
                }
            }
            continue;
        }

        // Typed declaration: NAME: TYPE or NAME: TYPE = VALUE
        // Assignment: NAME = VALUE (not ==, !=, <=, >=)
        // Indexed/slice assignment: NAME[...] = ...
        if let Some(name) = extract_leading_ident(trimmed) {
            let rest = trimmed[name.len()..].trim_start();
            if rest.starts_with(':') && !rest.starts_with("::") {
                if section == SourceSection::Init && indent > section_indent {
                    scope.insert_state_variable(name.to_owned());
                } else {
                    scope.insert_variable(name.to_owned());
                }
            } else if rest.starts_with('=') && !rest.starts_with("==") {
                if section == SourceSection::Init && indent > section_indent {
                    scope.insert_state_variable(name.to_owned());
                } else {
                    scope.insert_variable(name.to_owned());
                }
            } else if rest.starts_with('[') {
                if section == SourceSection::Init && indent > section_indent {
                    scope.insert_state_variable(name.to_owned());
                } else {
                    scope.insert_variable(name.to_owned());
                }
            }
        }
    }
}

fn detect_section_header(trimmed: &str) -> Option<SourceSection> {
    let pairs: &[(&str, SourceSection)] = &[
        ("ins", SourceSection::Ins),
        ("outs", SourceSection::Outs),
        ("params", SourceSection::Params),
        ("buffers", SourceSection::Buffers),
        ("init", SourceSection::Init),
        ("events", SourceSection::Events),
        ("sample", SourceSection::Sample),
        ("block", SourceSection::Block),
    ];
    for &(kw, sec) in pairs {
        if trimmed.starts_with(kw) {
            let rest = &trimmed[kw.len()..];
            if rest.is_empty()
                || rest.starts_with(':')
                || rest.starts_with('<')
                || rest.starts_with(' ')
                || rest.starts_with('{')
            {
                return Some(sec);
            }
        }
    }
    None
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Extract the first identifier from `text`, e.g. `"foo_bar: i32"` → `"foo_bar"`.
fn extract_leading_ident(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || !is_ident_start(bytes[0]) {
        return None;
    }
    let end = bytes
        .iter()
        .position(|&b| !is_ident_continue(b))
        .unwrap_or(bytes.len());
    Some(&text[..end])
}

/// Extract const names from namespace generic defaults: `namespace name<A = 1, B = 2>:`.
fn extract_namespace_generic_consts(trimmed: &str, scope: &mut SemanticScope) {
    if let Some(open) = trimmed.find('<') {
        if let Some(close) = trimmed[open..].find('>') {
            let inner = &trimmed[open + 1..open + close];
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(name) = extract_leading_ident(part) {
                    scope.consts.insert(name.to_owned());
                }
            }
        }
    }
}

/// Extract type params from `<T, U>` in proc/struct/def/ins/outs lines.
fn extract_type_params(trimmed: &str, scope: &mut SemanticScope) {
    if let Some(open) = trimmed.find('<') {
        if let Some(close) = trimmed[open..].find('>') {
            let inner = &trimmed[open + 1..open + close];
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(name) = extract_leading_ident(part) {
                    // Skip numeric-only params (e.g. `<256>`) and defaults (e.g. `FFTSize = 256`)
                    if name.as_bytes()[0].is_ascii_alphabetic() && !part.contains('=') {
                        scope.types.insert(name.to_owned());
                    }
                }
            }
        }
    }
}

/// Extract function name and params from `def name(param: Type, ...)` or `def name<T>(...)`.
fn extract_def_symbols(trimmed: &str, scope: &mut SemanticScope) {
    let rest = trimmed.strip_prefix("def ").unwrap_or(trimmed).trim_start();
    if let Some(name) = extract_leading_ident(rest) {
        scope.functions.insert(name.to_owned());
        let after_name = &rest[name.len()..];
        // Skip optional type params
        let after_generics = if let Some(open) = after_name.find('<') {
            if let Some(close) = after_name[open..].find('>') {
                extract_type_params(after_name, scope);
                &after_name[open + close + 1..]
            } else {
                after_name
            }
        } else {
            after_name
        };
        // Extract params from parens
        if let Some(paren_start) = after_generics.find('(') {
            extract_params_from_parens(&after_generics[paren_start..], scope);
        }
    }
}

/// Extract function name and params from `event name(param: Type, ...)`.
fn extract_event_symbols(trimmed: &str, scope: &mut SemanticScope) {
    let rest = trimmed
        .strip_prefix("event ")
        .unwrap_or(trimmed)
        .trim_start();
    if let Some(name) = extract_leading_ident(rest) {
        scope.functions.insert(name.to_owned());
        let after_name = &rest[name.len()..];
        if let Some(paren_start) = after_name.find('(') {
            extract_params_from_parens(&after_name[paren_start..], scope);
        }
    }
}

/// Extract parameter names from `(name: Type, name2: Type)`.
fn extract_params_from_parens(text: &str, scope: &mut SemanticScope) {
    // Find matching parens
    let inner = if let Some(open) = text.find('(') {
        if let Some(close) = text[open..].find(')') {
            &text[open + 1..open + close]
        } else {
            return;
        }
    } else {
        return;
    };
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(name) = extract_leading_ident(part) {
            scope.parameters.insert(name.to_owned());
        }
    }
}

/// Collect `const NAME` declarations from source text (for tests and backward compat).
#[cfg(test)]
fn collect_const_names(source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("const ") {
            if let Some(name) = extract_leading_ident(rest.trim_start()) {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

fn scan_identifiers(source: &str, mut f: impl FnMut(&str, u32, u32, u32, bool, bool, bool)) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut line = 0_u32;
    let mut column = 0_u32;
    let mut prev_char = '\0';
    let mut prev_prev_char = '\0';

    while index < chars.len() {
        let ch = chars[index];
        if ch == '#' {
            while index < chars.len() && chars[index] != '\n' {
                prev_prev_char = prev_char;
                prev_char = chars[index];
                advance_position(chars[index], &mut line, &mut column);
                index += 1;
            }
            continue;
        }
        if ch == '"' {
            advance_position(ch, &mut line, &mut column);
            index += 1;
            let mut escaped = false;
            while index < chars.len() {
                let ch = chars[index];
                advance_position(ch, &mut line, &mut column);
                index += 1;
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    break;
                }
            }
            prev_prev_char = prev_char;
            prev_char = '"';
            continue;
        }
        if is_identifier_start(ch) {
            let after_dot = prev_char == '.' && prev_prev_char != '.';
            let start_line = line;
            let start_column = column;
            let mut name = String::new();
            while index < chars.len() && is_identifier_continue(chars[index]) {
                let ch = chars[index];
                name.push(ch);
                advance_position(ch, &mut line, &mut column);
                index += 1;
            }
            let length = name.encode_utf16().count() as u32;
            // Check if preceded by ::
            let after_colons = prev_char == ':' && prev_prev_char == ':';
            prev_prev_char = prev_char;
            prev_char = name.chars().last().unwrap_or('\0');
            // Peek ahead past optional <...> to see what follows
            let mut peek = index;
            while peek < chars.len() && chars[peek].is_whitespace() && chars[peek] != '\n' {
                peek += 1;
            }
            let mut is_call = peek < chars.len() && chars[peek] == '(';
            let mut followed_by_colons =
                peek + 1 < chars.len() && chars[peek] == ':' && chars[peek + 1] == ':';
            if !is_call && !followed_by_colons && peek < chars.len() && chars[peek] == '<' {
                // Skip over <...> generics then check for ( or ::
                let mut depth = 1;
                peek += 1;
                while peek < chars.len() && depth > 0 {
                    if chars[peek] == '<' {
                        depth += 1;
                    } else if chars[peek] == '>' {
                        depth -= 1;
                    } else if chars[peek] == '\n' {
                        break;
                    }
                    peek += 1;
                }
                if depth == 0 {
                    while peek < chars.len() && chars[peek].is_whitespace() && chars[peek] != '\n' {
                        peek += 1;
                    }
                    is_call = peek < chars.len() && chars[peek] == '(';
                    followed_by_colons =
                        peek + 1 < chars.len() && chars[peek] == ':' && chars[peek + 1] == ':';
                }
            }
            let in_ns_path = after_colons || followed_by_colons;
            f(
                &name,
                start_line,
                start_column,
                length,
                after_dot,
                is_call,
                in_ns_path,
            );
            continue;
        }

        if !ch.is_whitespace() {
            prev_prev_char = prev_char;
            prev_char = ch;
        }
        advance_position(ch, &mut line, &mut column);
        index += 1;
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn advance_position(ch: char, line: &mut u32, column: &mut u32) {
    match ch {
        '\n' => {
            *line += 1;
            *column = 0;
        }
        '\r' => {}
        _ => {
            *column += ch.len_utf16() as u32;
        }
    }
}

fn encode_semantic_tokens(tokens: &[SemanticToken]) -> Vec<u32> {
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

fn read_lsp_message(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|err| format!("failed to read lsp header: {err}"))?;
        if read == 0 {
            return if content_length.is_none() {
                Ok(None)
            } else {
                Err("unexpected EOF while reading lsp headers".to_owned())
            };
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                let parsed = value.trim().parse::<usize>().map_err(|err| {
                    format!("invalid Content-Length header '{}': {err}", value.trim())
                })?;
                content_length = Some(parsed);
            }
        }
    }

    let content_length =
        content_length.ok_or_else(|| "missing Content-Length header".to_owned())?;
    let mut payload = vec![0_u8; content_length];
    reader
        .read_exact(&mut payload)
        .map_err(|err| format!("failed to read lsp payload: {err}"))?;
    let text = String::from_utf8(payload)
        .map_err(|err| format!("lsp payload was not valid utf-8: {err}"))?;
    serde_json::from_str(&text).map_err(|err| format!("invalid lsp json payload: {err}"))
}

fn write_result(writer: &mut impl Write, id: Value, result: Value) -> Result<(), String> {
    write_lsp_message(
        writer,
        &json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "result": result,
        }),
    )
}

fn write_error(
    writer: &mut impl Write,
    id: Value,
    code: i64,
    message: String,
) -> Result<(), String> {
    write_lsp_message(
        writer,
        &json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "error": {
                "code": code,
                "message": message,
            }
        }),
    )
}

fn write_notification(writer: &mut impl Write, method: &str, params: Value) -> Result<(), String> {
    write_lsp_message(
        writer,
        &json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
            "params": params,
        }),
    )
}

fn write_lsp_message(writer: &mut impl Write, message: &Value) -> Result<(), String> {
    let payload = serde_json::to_vec(message)
        .map_err(|err| format!("failed to encode lsp message: {err}"))?;
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())
        .map_err(|err| format!("failed to write lsp header: {err}"))?;
    writer
        .write_all(&payload)
        .map_err(|err| format!("failed to write lsp payload: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("failed to flush lsp message: {err}"))?;
    Ok(())
}

fn lsp_document_path(uri: &str) -> Result<Option<PathBuf>, String> {
    match uri.split_once(':') {
        Some(("file", _)) => file_uri_to_path(uri).map(|path| Some(normalize_path(&path))),
        Some(("untitled", _)) => Ok(None),
        Some((scheme, _)) => Err(format!("unsupported uri scheme '{scheme}'")),
        None => Err(format!("invalid uri '{uri}'")),
    }
}

fn file_uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("unsupported uri '{uri}', expected file:// uri"))?;
    let decoded = percent_decode(rest)?;
    #[cfg(windows)]
    {
        if decoded.starts_with("//") {
            return Ok(PathBuf::from(decoded.replace('/', "\\")));
        }
        let decoded =
            if decoded.len() >= 3 && decoded.starts_with('/') && decoded.as_bytes()[2] == b':' {
                decoded[1..].to_owned()
            } else {
                decoded
            };
        return Ok(PathBuf::from(decoded.replace('/', "\\")));
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(decoded))
    }
}

fn path_to_file_uri(path: &Path) -> String {
    let normalized = normalize_path(path);
    let raw = normalized.to_string_lossy();
    #[cfg(windows)]
    let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    #[cfg(windows)]
    let raw = if raw.starts_with('\\') {
        raw.replace('\\', "/")
    } else {
        format!("/{}", raw.replace('\\', "/"))
    };
    #[cfg(not(windows))]
    let raw = raw.to_string();

    format!("file://{}", percent_encode_path(&raw))
}

fn percent_decode(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(format!("invalid percent escape in '{input}'"));
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| format!("invalid percent escape in '{input}'"))?;
            let value = u8::from_str_radix(hex, 16)
                .map_err(|_| format!("invalid percent escape in '{input}'"))?;
            out.push(value);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|err| format!("invalid utf-8 in uri: {err}"))
}

fn percent_encode_path(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let ch = byte as char;
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '/' | ':');
        if keep {
            out.push(ch);
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => unreachable!("hex digit out of range"),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

fn invalid_params(message: String) -> String {
    format!("invalid params: {message}")
}

#[cfg(test)]
mod tests {
    use super::{
        collect_const_names, collect_symbols_from_source, diagnostic_message, file_uri_to_path,
        is_reserved_word, latest_full_text, lsp_document_path, path_to_file_uri,
        semantic_tokens_for_document, LspServer, SemanticScope, TextDocumentContentChangeEvent,
        SEMANTIC_TOKEN_TYPE_ENUM_MEMBER, SEMANTIC_TOKEN_TYPE_FUNCTION,
        SEMANTIC_TOKEN_TYPE_PARAMETER, SEMANTIC_TOKEN_TYPE_PORT, SEMANTIC_TOKEN_TYPE_STATE,
        SEMANTIC_TOKEN_TYPE_VARIABLE,
    };
    use onda_frontend::{DiagCode, Diagnostic};
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onda_lsp_{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, text).expect("write test file");
    }

    fn decode_lsp_messages(bytes: Vec<u8>) -> Vec<serde_json::Value> {
        let mut cursor = Cursor::new(bytes);
        let mut messages = Vec::new();
        while let Some(message) = super::read_lsp_message(&mut cursor).expect("decode lsp message")
        {
            messages.push(message);
        }
        messages
    }

    #[test]
    fn latest_full_text_prefers_last_full_document_change() {
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: Some(json!({ "start": 0 })),
                text: "partial".to_owned(),
            },
            TextDocumentContentChangeEvent {
                range: None,
                text: "full".to_owned(),
            },
        ];
        assert_eq!(latest_full_text(&changes), Some("full".to_owned()));
    }

    #[test]
    fn reserved_words_include_singular_event_keyword() {
        assert!(is_reserved_word("event"));
        assert!(is_reserved_word("events"));
    }

    #[test]
    fn source_fallback_collects_singular_event_symbols_like_defs() {
        let source = concat!(
            "proc Env:\n",
            "  init:\n",
            "    phase = 0.0\n",
            "\n",
            "  event note_on(freq = 440.0):\n",
            "    local = freq\n",
            "    phase = local\n",
        );
        let mut scope = SemanticScope::default();
        collect_symbols_from_source(source, &mut scope);

        assert_eq!(
            scope.token_type_for_source_fallback("note_on"),
            Some(SEMANTIC_TOKEN_TYPE_FUNCTION)
        );
        assert_eq!(
            scope.token_type_for_source_fallback("freq"),
            Some(SEMANTIC_TOKEN_TYPE_PARAMETER)
        );
        assert_eq!(
            scope.token_type_for_source_fallback("local"),
            Some(SEMANTIC_TOKEN_TYPE_VARIABLE)
        );
        assert_eq!(
            scope.token_type_for_source_fallback("phase"),
            Some(SEMANTIC_TOKEN_TYPE_STATE)
        );
    }

    #[test]
    fn diagnostic_message_appends_trace() {
        let diagnostic = Diagnostic {
            code: DiagCode::Semantic,
            message: "root error".to_owned(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
            file: None,
            trace: vec!["deep".to_owned(), "higher".to_owned()],
        };
        let message = diagnostic_message(&diagnostic);
        assert!(message.contains("root error"));
        assert!(message.contains("trace:"));
        assert!(message.contains("- higher"));
        assert!(message.contains("- deep"));
    }

    #[test]
    fn file_uri_round_trips_common_paths() {
        let path = if cfg!(windows) {
            std::path::PathBuf::from(r"C:\Users\franc\Sources\onda llvm\file.onda")
        } else {
            std::path::PathBuf::from("/tmp/onda llvm/file.onda")
        };
        let uri = path_to_file_uri(&path);
        let decoded = file_uri_to_path(&uri).expect("file uri should decode");
        assert_eq!(decoded, path);
    }

    #[test]
    fn untitled_uri_is_accepted_without_disk_path() {
        assert_eq!(
            lsp_document_path("untitled:Scratch-1").expect("untitled uri should be accepted"),
            None
        );
    }

    #[test]
    fn collect_const_names_finds_declarations() {
        let source = "const GAIN = 0.5\nsample:\n  const MIX = GAIN\n";
        let names = collect_const_names(source);
        assert!(names.contains("GAIN"));
        assert!(names.contains("MIX"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn semantic_tokens_mark_const_declarations_and_uses() {
        let source = "const GAIN = 0.5\nsample:\n  out1 = GAIN\n";
        let tokens = semantic_tokens_for_document(source, None);
        assert!(
            tokens
                .iter()
                .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
            "tokens: {tokens:?}"
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_PORT),
            "tokens: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_proc_ports_and_local_variables() {
        let source = "proc Mix:\n  ins:\n    dry\n    fb\n\n  sample:\n    out1 = (dry + fb) * 0.5\n\ninit:\n  mix = Mix()\n\ngraph:\n  in1 >> mix.dry\n";
        let tokens = semantic_tokens_for_document(source, None);

        assert!(tokens.iter().any(|token| {
            token.line == 2
                && token.start == 4
                && token.length == 3
                && token.token_type == SEMANTIC_TOKEN_TYPE_PORT
        }));
        assert!(tokens.iter().any(|token| {
            token.line == 3
                && token.start == 4
                && token.length == 2
                && token.token_type == SEMANTIC_TOKEN_TYPE_PORT
        }));
        assert!(tokens.iter().any(|token| {
            token.line == 6
                && token.start == 12
                && token.length == 3
                && token.token_type == SEMANTIC_TOKEN_TYPE_PORT
        }));
        assert!(tokens.iter().any(|token| {
            token.line == 6
                && token.start == 18
                && token.length == 2
                && token.token_type == SEMANTIC_TOKEN_TYPE_PORT
        }));
        assert!(tokens.iter().any(|token| {
            token.line == 9
                && token.start == 2
                && token.length == 3
                && token.token_type == SEMANTIC_TOKEN_TYPE_STATE
        }));
        assert!(
            tokens.iter().any(|token| {
                token.line == 12
                    && token.start == 9
                    && token.length == 3
                    && token.token_type == SEMANTIC_TOKEN_TYPE_STATE
            }),
            "tokens: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_local_variable_uses_in_sample_blocks() {
        let source = "proc Saturate:\n  sample:\n    x = in1\n    out1 = x - (x * x * x) * 0.1\n";
        let tokens = semantic_tokens_for_document(source, None);

        assert!(
            tokens.iter().any(|token| {
                token.length == 1 && token.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE
            }),
            "tokens: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_onda_params_as_parameters() {
        let source = "params:\n  gain = 0.5\nsample:\n  out1 = gain\n";
        let tokens = semantic_tokens_for_document(source, None);

        assert!(
            tokens.iter().any(|token| {
                token.line == 1
                    && token.start == 2
                    && token.length == 4
                    && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "tokens: {tokens:?}"
        );
        assert!(
            tokens.iter().any(|token| {
                token.line == 3
                    && token.start == 9
                    && token.length == 4
                    && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "tokens: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_do_not_leak_event_locals_into_sample_blocks() {
        let source = concat!(
            "import std/osc\n",
            "\n",
            "outs:\n",
            "  out1\n",
            "\n",
            "init:\n",
            "  freq_state = 220.0\n",
            "  amp_state = 0.0\n",
            "  gate = false\n",
            "  osc = std::osc::Sine(freq = 220.0)\n",
            "\n",
            "events:\n",
            "  note_on(freq_hz = 440.0, amp = 1.0):\n",
            "    freq_state = freq_hz\n",
            "    amp_state = amp\n",
            "    gate = true\n",
            "\n",
            "  note_off():\n",
            "    gate = false\n",
            "\n",
            "sample:\n",
            "  osc.freq = freq_state\n",
            "\n",
            "  out1 = 0.0\n",
            "  if (gate):\n",
            "    out1 = osc() * amp\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let amp_tokens = tokens
            .iter()
            .filter(|token| {
                let line = token.line as usize;
                let start = token.start as usize;
                let end = start + token.length as usize;
                source
                    .lines()
                    .nth(line)
                    .and_then(|text| text.get(start..end))
                    == Some("amp")
            })
            .collect::<Vec<_>>();

        assert!(
            amp_tokens.iter().any(|token| {
                token.line == 12 && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "event param declaration should still be highlighted: {amp_tokens:?}"
        );
        assert!(
            amp_tokens.iter().any(|token| {
                token.line == 14 && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "event param use should still be highlighted: {amp_tokens:?}"
        );
        assert!(
            !amp_tokens.iter().any(|token| token.line == 25),
            "sample block should not highlight event-local amp: {amp_tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_do_not_leak_event_locals_into_sample_blocks_for_preview_events_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("examples")
            .join("preview_events.onda");
        let source = std::fs::read_to_string(&path).expect("example source should be readable");
        let tokens = semantic_tokens_for_document(&source, Some(&path));

        let amp_tokens = tokens
            .iter()
            .filter(|token| {
                let line = token.line as usize;
                let start = token.start as usize;
                let end = start + token.length as usize;
                source
                    .lines()
                    .nth(line)
                    .and_then(|text| text.get(start..end))
                    == Some("amp")
            })
            .collect::<Vec<_>>();

        assert!(
            amp_tokens.iter().any(|token| {
                token.line == 12 && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "event param declaration should still be highlighted: {amp_tokens:?}"
        );
        assert!(
            amp_tokens.iter().any(|token| {
                token.line == 14 && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "event param use should still be highlighted: {amp_tokens:?}"
        );
        assert!(
            !amp_tokens.iter().any(|token| token.line == 24),
            "sample block should not highlight event-local amp: {amp_tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_do_not_expose_top_level_runtime_scope_inside_top_level_defs() {
        let source = concat!(
            "outs:\n",
            "  out1\n",
            "params:\n",
            "  gain = 1.0\n",
            "buffers:\n",
            "  buf: f32[]\n",
            "init:\n",
            "  state = 0.0\n",
            "\n",
            "def leak(x):\n",
            "  y = x + in1 + out1 + gain + state + buf[0]\n",
            "  return y\n",
            "\n",
            "sample:\n",
            "  out1 = state + gain\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        assert!(
            find_tokens("x")
                .iter()
                .any(|t| t.line == 9 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "top-level def param x should be highlighted inside def body"
        );
        assert!(
            find_tokens("y")
                .iter()
                .any(|t| t.line == 10 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
            "top-level def local y should be highlighted inside def body"
        );
        assert!(
            !find_tokens("in1").iter().any(|t| t.line == 10),
            "implicit top-level input should not be highlighted inside top-level def"
        );
        for name in ["out1", "gain", "state", "buf"] {
            assert!(
                !find_tokens(name).iter().any(|t| t.line == 10),
                "{name} should not be highlighted inside top-level def"
            );
        }
        assert!(
            find_tokens("state")
                .iter()
                .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "top-level state should still be highlighted in sample"
        );
        assert!(
            find_tokens("gain")
                .iter()
                .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "top-level param should still be highlighted in sample"
        );
    }

    #[test]
    fn semantic_tokens_do_not_bleed_between_top_level_defs() {
        let source = concat!(
            "def first(x):\n",
            "  local = x\n",
            "  return local\n",
            "\n",
            "def second():\n",
            "  return x + local\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        assert!(
            find_tokens("x")
                .iter()
                .any(|t| t.line == 1 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "first def param x should be highlighted in first body"
        );
        assert!(
            find_tokens("local")
                .iter()
                .any(|t| t.line == 2 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
            "first def local should be highlighted in first body"
        );
        assert!(
            !find_tokens("x").iter().any(|t| t.line == 5),
            "first def param should not bleed into second def"
        );
        assert!(
            !find_tokens("local").iter().any(|t| t.line == 5),
            "first def local should not bleed into second def"
        );
    }

    #[test]
    fn semantic_tokens_do_not_bleed_between_struct_methods() {
        let source = concat!(
            "struct Box:\n",
            "  value: f32 = 0.0\n",
            "\n",
            "  def set(self, x):\n",
            "    tmp = x\n",
            "    self.value = tmp\n",
            "\n",
            "  def get(self):\n",
            "    return x + tmp + self.value\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        assert!(
            find_tokens("x")
                .iter()
                .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "method param x should be highlighted in set body"
        );
        assert!(
            find_tokens("tmp")
                .iter()
                .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
            "method local tmp should be highlighted in set body"
        );
        assert!(
            !find_tokens("x").iter().any(|t| t.line == 8),
            "set param x should not bleed into get"
        );
        assert!(
            !find_tokens("tmp").iter().any(|t| t.line == 8),
            "set local tmp should not bleed into get"
        );
    }

    #[test]
    fn semantic_tokens_keep_proc_runtime_scope_but_not_proc_local_def_locals() {
        let source = concat!(
            "proc Reader:\n",
            "  ins:\n",
            "    in1\n",
            "  outs:\n",
            "    out1\n",
            "  params:\n",
            "    gain = 1.0\n",
            "  buffers:\n",
            "    line: buffer[f32]\n",
            "  init:\n",
            "    state = 0.0\n",
            "\n",
            "  def helper(x):\n",
            "    tmp = x + in1 + gain + state + line[0]\n",
            "    return tmp\n",
            "\n",
            "  sample:\n",
            "    out1 = helper(0.0)\n",
            "    out1 = tmp\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        assert!(
            find_tokens("x")
                .iter()
                .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "proc-local def param should be highlighted"
        );
        assert!(
            find_tokens("tmp")
                .iter()
                .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
            "proc-local def local should be highlighted"
        );
        assert!(
            find_tokens("in1")
                .iter()
                .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
            "proc input should be highlighted inside proc-local def"
        );
        assert!(
            find_tokens("gain")
                .iter()
                .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "proc param should be highlighted inside proc-local def"
        );
        assert!(
            find_tokens("state")
                .iter()
                .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "proc state should be highlighted inside proc-local def"
        );
        assert!(
            find_tokens("line")
                .iter()
                .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
            "proc buffer should be highlighted inside proc-local def"
        );
        assert!(
            !find_tokens("tmp").iter().any(|t| t.line == 18),
            "proc-local def local should not bleed into sample"
        );
    }

    #[test]
    fn publish_diagnostics_does_not_immediately_clear_entry_uri() {
        let dir = mk_temp_dir("publish_diagnostics");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "proc Saturate:\n  sample:\n    x = in1\n    out1 = x\n\ninit:\n  sat = Saturat()\n",
        );

        let mut server = LspServer::default();
        let normalized = server.session.open_document(
            &main,
            onda_daemon::DocumentVersion(1),
            fs::read_to_string(&main).expect("read test file"),
        );
        let uri = path_to_file_uri(&normalized);
        server.document_uris.insert(normalized, uri.clone());

        let mut writer = Vec::new();
        server
            .publish_diagnostics_for_entry(&main, &mut writer)
            .expect("publish diagnostics");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        let entry_notifications = notifications
            .iter()
            .filter(|message| {
                message["params"]["uri"]
                    .as_str()
                    .map(|value| value == uri)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        assert_eq!(
            entry_notifications.len(),
            1,
            "unexpected publish sequence: {notifications:?}"
        );
        assert!(
            entry_notifications[0]["params"]["diagnostics"]
                .as_array()
                .map(|diagnostics| !diagnostics.is_empty())
                .unwrap_or(false),
            "expected non-empty diagnostics for entry uri: {entry_notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_tokens_mark_init_vars_in_proc_defs_events_and_sample() {
        let source = concat!(
            "proc Conv:\n",
            "  init:\n",
            "    delay: f32[100]\n",
            "    write: i32 = 0\n",
            "\n",
            "  def clear():\n",
            "    delay[:] = 0.0\n",
            "    write = 0\n",
            "\n",
            "  events:\n",
            "    reset():\n",
            "      clear()\n",
            "      write = 0\n",
            "\n",
            "  sample:\n",
            "    delay[write] = in1\n",
            "    write = write + 1\n",
            "    out1 = delay[0]\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        // 'delay' in def clear (line 6)
        assert!(
            tokens
                .iter()
                .any(|t| t.line == 6 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
            "delay in def: {tokens:?}"
        );
        // 'write' in def clear (line 7)
        assert!(
            tokens
                .iter()
                .any(|t| t.line == 7 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
            "write in def: {tokens:?}"
        );
        // 'write' in events reset (line 12)
        assert!(
            tokens.iter().any(|t| t.line == 12
                && t.token_type == SEMANTIC_TOKEN_TYPE_STATE
                && t.length == 5),
            "write in events: {tokens:?}"
        );
        // 'delay' in sample (line 15)
        assert!(
            tokens.iter().any(|t| t.line == 15
                && t.token_type == SEMANTIC_TOKEN_TYPE_STATE
                && t.length == 5),
            "delay in sample: {tokens:?}"
        );
        // 'write' in sample (line 15, inside delay[write])
        assert!(
            tokens.iter().any(|t| t.line == 15
                && t.token_type == SEMANTIC_TOKEN_TYPE_STATE
                && t.length == 5),
            "write in sample delay[write]: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_work_inside_namespace() {
        let source = concat!(
            "namespace test::ns:\n",
            "  const SIZE = 10\n",
            "\n",
            "  proc Foo:\n",
            "    init:\n",
            "      buf: f32[SIZE]\n",
            "      pos: i32 = 0\n",
            "\n",
            "    def clear():\n",
            "      buf[:] = 0.0\n",
            "      pos = 0\n",
            "\n",
            "    sample:\n",
            "      buf[pos] = in1\n",
            "      pos = pos + 1\n",
            "      out1 = buf[0]\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        // 'buf' in def clear (line 9)
        assert!(
            tokens
                .iter()
                .any(|t| t.line == 9 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 3),
            "buf in def: {tokens:?}"
        );
        // 'pos' in sample (line 13)
        assert!(
            tokens.iter().any(|t| t.line == 13
                && t.token_type == SEMANTIC_TOKEN_TYPE_STATE
                && t.length == 3),
            "pos in sample: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_proc_section_declarations() {
        let source = concat!(
            "proc Loop:\n",
            "  ins:\n",
            "    input\n",
            "  outs:\n",
            "    wet\n",
            "  params:\n",
            "    rate = 1.0\n",
            "  buffers:\n",
            "    buf: f32[]\n",
            "  init:\n",
            "    pos = 0.0\n",
            "  sample:\n",
            "    wet = input\n",
            "    pos = pos + rate\n",
            "    out1 = buf.readL(0, pos)\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        assert!(
            tokens.iter().any(|t| {
                t.line == 2
                    && t.start == 4
                    && t.length == 5
                    && t.token_type == SEMANTIC_TOKEN_TYPE_PORT
            }),
            "input decl: {tokens:?}"
        );
        assert!(
            tokens.iter().any(|t| {
                t.line == 4
                    && t.start == 4
                    && t.length == 3
                    && t.token_type == SEMANTIC_TOKEN_TYPE_PORT
            }),
            "wet decl: {tokens:?}"
        );
        assert!(
            tokens.iter().any(|t| {
                t.line == 6
                    && t.start == 4
                    && t.length == 4
                    && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER
            }),
            "rate decl: {tokens:?}"
        );
        assert!(
            tokens.iter().any(|t| {
                t.line == 8
                    && t.start == 4
                    && t.length == 3
                    && t.token_type == SEMANTIC_TOKEN_TYPE_PORT
            }),
            "buf decl: {tokens:?}"
        );
        assert!(
            tokens.iter().any(|t| {
                t.line == 10
                    && t.start == 4
                    && t.length == 3
                    && t.token_type == SEMANTIC_TOKEN_TYPE_STATE
            }),
            "pos decl: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_proc_output_in_else_branch() {
        let source = concat!(
            "proc Svf:\n",
            "  params:\n",
            "    mode: i32 = 0\n",
            "\n",
            "  sample:\n",
            "    if (mode <= 0):\n",
            "      out1 = 0.0\n",
            "    elif (mode == 1):\n",
            "      out1 = 1.0\n",
            "    else:\n",
            "      out1 = 2.0\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        assert!(
            tokens.iter().any(|t| {
                t.line == 10
                    && t.start == 6
                    && t.length == 4
                    && t.token_type == SEMANTIC_TOKEN_TYPE_PORT
            }),
            "else-branch proc output should be highlighted as port: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_top_level_init_vars_in_buffer_looper_read() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("examples")
            .join("buffer_looper_read.onda");
        let source = std::fs::read_to_string(&path).expect("example source should be readable");
        let tokens = semantic_tokens_for_document(&source, Some(&path));

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        let pos_tokens = find_tokens("pos");
        assert!(!pos_tokens.is_empty(), "pos should be highlighted");
        assert!(
            pos_tokens
                .iter()
                .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "pos should be state everywhere: {pos_tokens:?}"
        );

        let rate_tokens = find_tokens("rate");
        assert!(!rate_tokens.is_empty(), "rate should be highlighted");
        assert!(
            rate_tokens
                .iter()
                .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
            "rate should be parameter everywhere: {rate_tokens:?}"
        );

        let src_tokens = find_tokens("src");
        assert!(!src_tokens.is_empty(), "src should be highlighted");
        assert!(
            src_tokens
                .iter()
                .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
            "src should be port everywhere: {src_tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_do_not_mark_nested_init_locals_as_state() {
        let source = concat!(
            "outs:\n",
            "  out1\n",
            "init:\n",
            "  voices = 0.0\n",
            "  for i in 0..4:\n",
            "    h = f32(i + 1)\n",
            "    voices = h\n",
            "sample:\n",
            "  out1 = voices\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        let h_tokens = find_tokens("h");
        assert!(
            h_tokens
                .iter()
                .any(|t| t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
            "nested init local should be highlighted as local variable: {h_tokens:?}"
        );
        assert!(
            h_tokens
                .iter()
                .all(|t| t.token_type != SEMANTIC_TOKEN_TYPE_STATE),
            "nested init local should not be highlighted as state: {h_tokens:?}"
        );

        let voices_tokens = find_tokens("voices");
        assert!(
            voices_tokens
                .iter()
                .any(|t| t.line == 3 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "top-level init assignment should still be state: {voices_tokens:?}"
        );
        assert!(
            voices_tokens
                .iter()
                .any(|t| t.line == 8 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "sample use should still see top-level init state: {voices_tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_block_pre_state_in_nested_sample() {
        let source = concat!(
            "outs:\n",
            "  out1\n",
            "block:\n",
            "  acc = 0.0\n",
            "  sample:\n",
            "    out1 = acc\n",
        );
        let tokens = semantic_tokens_for_document(source, None);

        let acc_tokens = tokens
            .iter()
            .filter(|t| {
                let line = t.line as usize;
                let col = t.start as usize;
                let len = t.length as usize;
                source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some("acc")
            })
            .collect::<Vec<_>>();

        assert!(
            acc_tokens
                .iter()
                .any(|t| t.line == 3 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "block-pre declaration should be highlighted as state: {acc_tokens:?}"
        );
        assert!(
            acc_tokens
                .iter()
                .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "nested sample should see block-pre carried state: {acc_tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_highlight_for_loop_references() {
        let source = concat!(
            "namespace test::ns:\n",       // 0
            "  const SIZE = 10\n",         // 1
            "\n",                          // 2
            "  proc Foo:\n",               // 3
            "    init:\n",                 // 4
            "      count: i32 = 0\n",      // 5
            "\n",                          // 6
            "    sample:\n",               // 7
            "      for i in 0..SIZE:\n",   // 8
            "        count = count + 1\n", // 9
        );
        let tokens = semantic_tokens_for_document(source, None);

        // SIZE in for loop (line 8) should be const
        assert!(
            tokens.iter().any(|t| t.line == 8
                && t.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER
                && t.length == 4),
            "SIZE in for loop: {tokens:?}"
        );
        // count in for body (line 9) should be variable
        assert!(
            tokens
                .iter()
                .any(|t| t.line == 9 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
            "count in for body: {tokens:?}"
        );
        // i should be variable (loop var)
        assert!(
            tokens.iter().any(|t| t.line == 8
                && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE
                && t.length == 1),
            "i in for loop: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_work_for_convolution_onda() {
        let source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std/convolution.onda"),
        )
        .expect("read convolution.onda");
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std/convolution.onda");
        let tokens = semantic_tokens_for_document(&source, Some(&path));

        let find_tokens = |name: &str| -> Vec<&super::SemanticToken> {
            tokens
                .iter()
                .filter(|t| {
                    let line = t.line as usize;
                    let col = t.start as usize;
                    let len = t.length as usize;
                    source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
                })
                .collect()
        };

        // Init variables highlighted in defs/events/sample
        assert!(
            !find_tokens("delay").is_empty(),
            "delay should be highlighted"
        );
        assert!(
            !find_tokens("write").is_empty(),
            "write should be highlighted"
        );
        assert!(
            !find_tokens("active_taps").is_empty(),
            "active_taps should be highlighted"
        );

        // Namespace generic consts
        let fft_tokens = find_tokens("FFTSize");
        assert!(!fft_tokens.is_empty(), "FFTSize should be highlighted");
        assert!(
            fft_tokens
                .iter()
                .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
            "FFTSize should be const: {fft_tokens:?}"
        );

        // Type params
        let t_tokens = find_tokens("T");
        assert!(!t_tokens.is_empty(), "T should be highlighted");
        assert!(
            t_tokens
                .iter()
                .all(|t| t.token_type == super::SEMANTIC_TOKEN_TYPE_TYPE),
            "T should be type: {t_tokens:?}"
        );

        // Functions (def methods and event handlers)
        let clear_state = find_tokens("clear_state");
        assert!(!clear_state.is_empty(), "clear_state should be highlighted");
        assert!(
            clear_state
                .iter()
                .any(|t| t.token_type == super::SEMANTIC_TOKEN_TYPE_FUNCTION),
            "clear_state should be function: {clear_state:?}"
        );

        // Proc instance variables (td, tail in ZeroLatencyConvolver)
        assert!(!find_tokens("td").is_empty(), "td should be highlighted");
        assert!(
            !find_tokens("tail").is_empty(),
            "tail should be highlighted"
        );

        // Event handler names (set_impulse, reset)
        let set_impulse = find_tokens("set_impulse");
        assert!(!set_impulse.is_empty(), "set_impulse should be highlighted");
        let reset = find_tokens("reset");
        assert!(!reset.is_empty(), "reset should be highlighted");
    }

    #[test]
    fn did_open_publishes_diagnostics_immediately() {
        let dir = mk_temp_dir("did_open_publish");
        let main = dir.join("main.onda");
        let source = "init:\n  sat = Saturat()\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let uri = path_to_file_uri(&main);
        let mut writer = Vec::new();
        let message = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "version": 1,
                    "text": source,
                }
            }
        });

        server
            .handle_message(message, &mut writer)
            .expect("didOpen should succeed");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        assert!(
            notifications.iter().any(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .map(|diagnostics| !diagnostics.is_empty())
                    .unwrap_or(false)
            }),
            "expected didOpen to publish diagnostics: {notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
