use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use omni_daemon::{DaemonSession, DocumentVersion};
use omni_frontend::{
    parse_program, parse_program_with_path, AssignTarget, Block, Diagnostic, Expr, FunctionDef,
    GraphBlock, GraphEndpoint, ParamDecl, PortDecl, ProcessorDef, Program, SourceLoc, Span, Stmt,
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
                eprintln!("omni lsp: failed to decode lsp message: {err}");
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
                    eprintln!("omni lsp: {err}");
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
                    "tokenTypes": ["enumMember", "variable", "port", "parameter"],
                    "tokenModifiers": [],
                }
            },
        },
        "serverInfo": {
            "name": "omni",
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
        "source": "omni",
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
    let parsed = match path {
        Some(path) => parse_program_with_path(source, path).or_else(|_| parse_program(source)),
        None => parse_program(source),
    };

    match parsed {
        Ok(program) => {
            let mut tokens = semantic_tokens_from_program(source, path, &program);
            tokens.extend(fallback_const_tokens(source));
            tokens.sort_by_key(|token| {
                (
                    token.line,
                    token.start,
                    token.length,
                    token.token_type,
                    token.token_modifiers,
                )
            });
            tokens.dedup_by(|lhs, rhs| {
                lhs.line == rhs.line
                    && lhs.start == rhs.start
                    && lhs.length == rhs.length
                    && lhs.token_type == rhs.token_type
                    && lhs.token_modifiers == rhs.token_modifiers
            });
            tokens
        }
        Err(_) => fallback_const_tokens(source),
    }
}

fn semantic_tokens_from_program(
    source: &str,
    path: Option<&Path>,
    program: &Program,
) -> Vec<SemanticToken> {
    let current_path = path.map(normalize_path);
    let mut tokens = Vec::new();
    let top_level_scope = collect_top_level_scope(program, current_path.as_deref(), &mut tokens);

    let mut runtime_scope = top_level_scope.clone();
    for block in &program.blocks {
        if !loc_matches_path(block.loc(), current_path.as_deref()) {
            continue;
        }
        match block {
            Block::Init(init) => {
                collect_stmt_tokens(
                    &init.body,
                    &mut runtime_scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                    true,
                );
            }
            Block::Block(exec) => {
                let mut pre_scope = runtime_scope.clone();
                collect_stmt_tokens(
                    &exec.pre,
                    &mut pre_scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                    false,
                );
                if let Some(sample) = &exec.sample {
                    let mut sample_scope = runtime_scope.clone();
                    collect_stmt_tokens(
                        &sample.body,
                        &mut sample_scope,
                        current_path.as_deref(),
                        source,
                        &mut tokens,
                        false,
                    );
                }
                let mut post_scope = runtime_scope.clone();
                collect_stmt_tokens(
                    &exec.post,
                    &mut post_scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                    false,
                );
            }
            Block::Sample(sample) => {
                let mut scope = runtime_scope.clone();
                collect_stmt_tokens(
                    &sample.body,
                    &mut scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                    false,
                );
            }
            Block::Graph(graph) => {
                collect_graph_tokens(
                    graph,
                    &runtime_scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                );
            }
            Block::Def(def) => {
                collect_function_tokens(
                    def,
                    &runtime_scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                );
            }
            Block::Proc(proc_def) => {
                collect_proc_tokens(
                    proc_def,
                    &runtime_scope,
                    current_path.as_deref(),
                    source,
                    &mut tokens,
                );
            }
            _ => {}
        }
    }

    tokens.sort_by_key(|token| {
        (
            token.line,
            token.start,
            token.length,
            token.token_type,
            token.token_modifiers,
        )
    });
    tokens.dedup_by(|lhs, rhs| {
        lhs.line == rhs.line
            && lhs.start == rhs.start
            && lhs.length == rhs.length
            && lhs.token_type == rhs.token_type
            && lhs.token_modifiers == rhs.token_modifiers
    });
    tokens
}

fn fallback_const_tokens(source: &str) -> Vec<SemanticToken> {
    let const_names = collect_const_names(source);
    if const_names.is_empty() {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    scan_identifiers(source, |name, line, start, length| {
        if const_names.contains(name) {
            tokens.push(SemanticToken {
                line,
                start,
                length,
                token_type: SEMANTIC_TOKEN_TYPE_ENUM_MEMBER,
                token_modifiers: 0,
            });
        }
    });
    tokens
}

fn collect_const_names(source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut expect_name = false;
    scan_identifiers(source, |name, _, _, _| {
        if expect_name {
            names.insert(name.to_owned());
            expect_name = false;
        } else if name == "const" {
            expect_name = true;
        }
    });
    names
}

#[derive(Debug, Clone, Default)]
struct SemanticScope {
    consts: HashSet<String>,
    variables: HashSet<String>,
    ports: HashSet<String>,
    parameters: HashSet<String>,
}

impl SemanticScope {
    fn token_type_for(&self, name: &str) -> Option<u32> {
        if self.consts.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
        } else if self.parameters.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_PARAMETER)
        } else if self.ports.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_PORT)
        } else if is_implicit_port_name(name) {
            Some(SEMANTIC_TOKEN_TYPE_PORT)
        } else if self.variables.contains(name) {
            Some(SEMANTIC_TOKEN_TYPE_VARIABLE)
        } else {
            None
        }
    }
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

fn collect_top_level_scope(
    program: &Program,
    current_path: Option<&Path>,
    tokens: &mut Vec<SemanticToken>,
) -> SemanticScope {
    let mut scope = SemanticScope::default();
    for block in &program.blocks {
        if !loc_matches_path(block.loc(), current_path) {
            continue;
        }
        match block {
            Block::Const(decl) => {
                scope.consts.insert(decl.name.clone());
                push_name_token(
                    tokens,
                    decl.loc.into(),
                    &decl.name,
                    SEMANTIC_TOKEN_TYPE_ENUM_MEMBER,
                );
            }
            Block::Ins(ports) | Block::Outs(ports) => {
                for decl in &ports.decls {
                    scope.ports.insert(decl.name.clone());
                    push_name_token(
                        tokens,
                        decl.loc.into(),
                        &decl.name,
                        SEMANTIC_TOKEN_TYPE_PORT,
                    );
                }
            }
            Block::Params(params) => {
                for decl in &params.decls {
                    scope.parameters.insert(decl.name.clone());
                    push_name_token(
                        tokens,
                        decl.loc.into(),
                        &decl.name,
                        SEMANTIC_TOKEN_TYPE_PARAMETER,
                    );
                }
            }
            Block::Buffers(buffers) => {
                for decl in &buffers.decls {
                    scope.ports.insert(decl.name.clone());
                    push_name_token(
                        tokens,
                        decl.loc.into(),
                        &decl.name,
                        SEMANTIC_TOKEN_TYPE_PORT,
                    );
                }
            }
            _ => {}
        }
    }
    scope
}

fn collect_proc_tokens(
    proc_def: &ProcessorDef,
    parent_scope: &SemanticScope,
    current_path: Option<&Path>,
    source: &str,
    tokens: &mut Vec<SemanticToken>,
) {
    if !loc_matches_path(proc_def.loc.into(), current_path) {
        return;
    }

    let mut proc_scope = parent_scope.clone();
    declare_port_tokens(&proc_def.ins, &mut proc_scope, tokens);
    declare_port_tokens(&proc_def.outs, &mut proc_scope, tokens);
    declare_param_tokens(&proc_def.params, &mut proc_scope, tokens);
    for buffer in &proc_def.buffers {
        proc_scope.ports.insert(buffer.name.clone());
        push_name_token(
            tokens,
            buffer.loc.into(),
            &buffer.name,
            SEMANTIC_TOKEN_TYPE_PORT,
        );
    }

    let mut runtime_scope = proc_scope.clone();
    collect_stmt_tokens(
        &proc_def.init.body,
        &mut runtime_scope,
        current_path,
        source,
        tokens,
        true,
    );

    let mut sample_scope = runtime_scope.clone();
    collect_stmt_tokens(
        &proc_def.sample,
        &mut sample_scope,
        current_path,
        source,
        tokens,
        false,
    );

    let mut pre_scope = runtime_scope.clone();
    collect_stmt_tokens(
        &proc_def.block_pre,
        &mut pre_scope,
        current_path,
        source,
        tokens,
        false,
    );

    let mut post_scope = runtime_scope.clone();
    collect_stmt_tokens(
        &proc_def.block_post,
        &mut post_scope,
        current_path,
        source,
        tokens,
        false,
    );

    if let Some(graph) = &proc_def.graph {
        collect_graph_tokens(graph, &runtime_scope, current_path, source, tokens);
    }

    for def in &proc_def.local_defs {
        collect_function_tokens(def, &runtime_scope, current_path, source, tokens);
    }
}

fn declare_port_tokens(
    decls: &[PortDecl],
    scope: &mut SemanticScope,
    tokens: &mut Vec<SemanticToken>,
) {
    for decl in decls {
        scope.ports.insert(decl.name.clone());
        push_name_token(
            tokens,
            decl.loc.into(),
            &decl.name,
            SEMANTIC_TOKEN_TYPE_PORT,
        );
    }
}

fn declare_param_tokens(
    decls: &[ParamDecl],
    scope: &mut SemanticScope,
    tokens: &mut Vec<SemanticToken>,
) {
    for decl in decls {
        scope.parameters.insert(decl.name.clone());
        push_name_token(
            tokens,
            decl.loc.into(),
            &decl.name,
            SEMANTIC_TOKEN_TYPE_PARAMETER,
        );
    }
}

fn collect_function_tokens(
    def: &FunctionDef,
    parent_scope: &SemanticScope,
    current_path: Option<&Path>,
    source: &str,
    tokens: &mut Vec<SemanticToken>,
) {
    if !loc_matches_path(def.loc.into(), current_path) {
        return;
    }

    let mut scope = parent_scope.clone();
    for param in &def.params {
        scope.parameters.insert(param.name.clone());
        push_name_token(
            tokens,
            param.loc.into(),
            &param.name,
            SEMANTIC_TOKEN_TYPE_PARAMETER,
        );
    }
    collect_stmt_tokens(&def.body, &mut scope, current_path, source, tokens, false);
}

fn collect_stmt_tokens(
    stmts: &[Stmt],
    scope: &mut SemanticScope,
    current_path: Option<&Path>,
    source: &str,
    tokens: &mut Vec<SemanticToken>,
    allow_local_variables: bool,
) {
    for stmt in stmts {
        if !loc_matches_path(stmt.loc(), current_path) {
            continue;
        }
        match stmt {
            Stmt::Const { decl, .. } => {
                push_name_token(
                    tokens,
                    decl.loc.into(),
                    &decl.name,
                    SEMANTIC_TOKEN_TYPE_ENUM_MEMBER,
                );
                collect_expr_tokens(&decl.expr, scope, current_path, source, tokens);
                scope.consts.insert(decl.name.clone());
            }
            Stmt::Assign {
                target,
                target_loc,
                expr,
                ..
            } => {
                collect_assign_target_tokens(
                    target,
                    *target_loc,
                    scope,
                    current_path,
                    source,
                    tokens,
                    allow_local_variables,
                );
                collect_expr_tokens(expr, scope, current_path, source, tokens);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_expr_tokens(expr, scope, current_path, source, tokens);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_expr_tokens(cond, scope, current_path, source, tokens);
                let mut then_scope = scope.clone();
                collect_stmt_tokens(
                    then_branch,
                    &mut then_scope,
                    current_path,
                    source,
                    tokens,
                    allow_local_variables,
                );
                let mut else_scope = scope.clone();
                collect_stmt_tokens(
                    else_branch,
                    &mut else_scope,
                    current_path,
                    source,
                    tokens,
                    allow_local_variables,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                var,
                body,
                ..
            } => {
                collect_expr_tokens(start, scope, current_path, source, tokens);
                collect_expr_tokens(end, scope, current_path, source, tokens);
                if let Some(step) = step {
                    collect_expr_tokens(step, scope, current_path, source, tokens);
                }
                let mut loop_scope = scope.clone();
                if allow_local_variables {
                    loop_scope.variables.insert(var.clone());
                }
                collect_stmt_tokens(
                    body,
                    &mut loop_scope,
                    current_path,
                    source,
                    tokens,
                    allow_local_variables,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_expr_tokens(cond, scope, current_path, source, tokens);
                let mut loop_scope = scope.clone();
                collect_stmt_tokens(
                    body,
                    &mut loop_scope,
                    current_path,
                    source,
                    tokens,
                    allow_local_variables,
                );
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

fn collect_assign_target_tokens(
    target: &AssignTarget,
    target_loc: Span,
    scope: &mut SemanticScope,
    current_path: Option<&Path>,
    source: &str,
    tokens: &mut Vec<SemanticToken>,
    allow_local_variables: bool,
) {
    if !loc_matches_path(target_loc.into(), current_path) {
        return;
    }
    match target {
        AssignTarget::Var(name) => {
            let Some(token_type) = scope
                .token_type_for(name)
                .or_else(|| allow_local_variables.then_some(SEMANTIC_TOKEN_TYPE_VARIABLE))
            else {
                return;
            };
            push_name_token(tokens, target_loc.into(), name, token_type);
            if allow_local_variables && token_type == SEMANTIC_TOKEN_TYPE_VARIABLE {
                scope.variables.insert(name.clone());
            }
        }
        AssignTarget::Index { base, index } => {
            if let Some(token_type) = scope.token_type_for(base) {
                push_name_token(tokens, target_loc.into(), base, token_type);
            }
            collect_expr_tokens(index, scope, current_path, source, tokens);
        }
        AssignTarget::Slice { base, start, end } => {
            if let Some(token_type) = scope.token_type_for(base) {
                push_name_token(tokens, target_loc.into(), base, token_type);
            }
            if let Some(start) = start {
                collect_expr_tokens(start, scope, current_path, source, tokens);
            }
            if let Some(end) = end {
                collect_expr_tokens(end, scope, current_path, source, tokens);
            }
        }
        AssignTarget::Tuple(names) => {
            for name in names {
                if let Some(token_type) = scope
                    .token_type_for(name)
                    .or_else(|| allow_local_variables.then_some(SEMANTIC_TOKEN_TYPE_VARIABLE))
                {
                    push_name_token(tokens, target_loc.into(), name, token_type);
                    if allow_local_variables && token_type == SEMANTIC_TOKEN_TYPE_VARIABLE {
                        scope.variables.insert(name.clone());
                    }
                }
            }
        }
    }
}

fn collect_expr_tokens(
    expr: &Expr,
    scope: &SemanticScope,
    current_path: Option<&Path>,
    source: &str,
    tokens: &mut Vec<SemanticToken>,
) {
    if !loc_matches_path(expr.loc(), current_path) {
        return;
    }
    match expr {
        Expr::Var { loc, name } => {
            collect_named_ref_tokens(*loc, name, scope, source, tokens);
        }
        Expr::Index { loc, base, index } => {
            if let Some(token_type) = scope.token_type_for(base) {
                push_name_token(tokens, (*loc).into(), base, token_type);
            }
            collect_expr_tokens(index, scope, current_path, source, tokens);
        }
        Expr::Slice {
            loc,
            base,
            start,
            end,
        } => {
            if let Some(token_type) = scope.token_type_for(base) {
                push_name_token(tokens, (*loc).into(), base, token_type);
            }
            if let Some(start) = start {
                collect_expr_tokens(start, scope, current_path, source, tokens);
            }
            if let Some(end) = end {
                collect_expr_tokens(end, scope, current_path, source, tokens);
            }
        }
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                collect_expr_tokens(value, scope, current_path, source, tokens);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_expr_tokens(&spec.size, scope, current_path, source, tokens);
            if let Some(init) = init {
                for value in init {
                    collect_expr_tokens(value, scope, current_path, source, tokens);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_expr_tokens(lhs, scope, current_path, source, tokens);
            collect_expr_tokens(rhs, scope, current_path, source, tokens);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_tokens(arg, scope, current_path, source, tokens);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                collect_expr_tokens(&arg.expr, scope, current_path, source, tokens);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_expr_tokens(expr, scope, current_path, source, tokens);
        }
        Expr::Tuple { values, .. } => {
            for value in values {
                collect_expr_tokens(value, scope, current_path, source, tokens);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn collect_graph_tokens(
    graph: &GraphBlock,
    scope: &SemanticScope,
    current_path: Option<&Path>,
    source: &str,
    tokens: &mut Vec<SemanticToken>,
) {
    if !loc_matches_path(graph.loc.into(), current_path) {
        return;
    }
    for edge in &graph.edges {
        collect_expr_tokens(&edge.source, scope, current_path, source, tokens);
        if let Some(delay) = &edge.delay {
            collect_expr_tokens(delay, scope, current_path, source, tokens);
        }
        for dest in &edge.dests {
            match dest {
                GraphEndpoint::Symbol { loc, name } => {
                    collect_named_ref_tokens(*loc, name, scope, source, tokens);
                }
                GraphEndpoint::ProcField { loc, proc, field } => {
                    if let Some(token_type) = scope.token_type_for(proc) {
                        push_name_token(tokens, (*loc).into(), proc, token_type);
                    }
                    push_offset_token(
                        tokens,
                        (*loc).into(),
                        proc.encode_utf16().count() as u32 + 1,
                        field,
                        SEMANTIC_TOKEN_TYPE_PORT,
                    );
                }
                GraphEndpoint::ProcIndexedField {
                    loc, proc, index, ..
                } => {
                    if let Some(token_type) = scope.token_type_for(proc) {
                        push_name_token(tokens, (*loc).into(), proc, token_type);
                    }
                    collect_expr_tokens(index, scope, current_path, source, tokens);
                }
            }
        }
    }
}

fn collect_named_ref_tokens(
    loc: Span,
    name: &str,
    scope: &SemanticScope,
    _source: &str,
    tokens: &mut Vec<SemanticToken>,
) {
    if let Some((base, field)) = name.split_once('.') {
        if let Some(token_type) = scope.token_type_for(base) {
            push_name_token(tokens, loc.into(), base, token_type);
        }
        push_offset_token(
            tokens,
            loc.into(),
            base.encode_utf16().count() as u32 + 1,
            field,
            SEMANTIC_TOKEN_TYPE_PORT,
        );
        return;
    }

    if let Some(token_type) = scope.token_type_for(name) {
        push_name_token(tokens, loc.into(), name, token_type);
    }
}

fn push_name_token(tokens: &mut Vec<SemanticToken>, loc: SourceLoc, name: &str, token_type: u32) {
    if loc.line == 0 || name.is_empty() {
        return;
    }
    tokens.push(SemanticToken {
        line: (loc.line.saturating_sub(1)) as u32,
        start: (loc.column.saturating_sub(1)) as u32,
        length: name.encode_utf16().count() as u32,
        token_type,
        token_modifiers: 0,
    });
}

fn push_offset_token(
    tokens: &mut Vec<SemanticToken>,
    loc: SourceLoc,
    offset: u32,
    name: &str,
    token_type: u32,
) {
    if loc.line == 0 || name.is_empty() {
        return;
    }
    tokens.push(SemanticToken {
        line: (loc.line.saturating_sub(1)) as u32,
        start: (loc.column.saturating_sub(1)) as u32 + offset,
        length: name.encode_utf16().count() as u32,
        token_type,
        token_modifiers: 0,
    });
}

fn loc_matches_path(loc: SourceLoc, current_path: Option<&Path>) -> bool {
    match current_path {
        Some(current_path) => loc
            .file()
            .map(|file| normalize_path(Path::new(&file)) == current_path)
            .unwrap_or(false),
        None => true,
    }
}

fn scan_identifiers(source: &str, mut f: impl FnMut(&str, u32, u32, u32)) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut line = 0_u32;
    let mut column = 0_u32;

    while index < chars.len() {
        let ch = chars[index];
        if ch == '#' {
            while index < chars.len() && chars[index] != '\n' {
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
            continue;
        }
        if is_identifier_start(ch) {
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
            f(&name, start_line, start_column, length);
            continue;
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
        collect_const_names, diagnostic_message, file_uri_to_path, latest_full_text,
        lsp_document_path, path_to_file_uri, semantic_tokens_for_document, LspServer,
        TextDocumentContentChangeEvent, SEMANTIC_TOKEN_TYPE_ENUM_MEMBER,
        SEMANTIC_TOKEN_TYPE_PARAMETER, SEMANTIC_TOKEN_TYPE_PORT, SEMANTIC_TOKEN_TYPE_VARIABLE,
    };
    use omni_frontend::{DiagCode, Diagnostic};
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
        let dir = std::env::temp_dir().join(format!("omni_lsp_{prefix}_{nanos}"));
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
            std::path::PathBuf::from(r"C:\Users\franc\Sources\omni llvm\file.omni")
        } else {
            std::path::PathBuf::from("/tmp/omni llvm/file.omni")
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
                && token.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE
        }));
        assert!(
            tokens.iter().any(|token| {
                token.line == 12
                    && token.start == 9
                    && token.length == 3
                    && token.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE
            }),
            "tokens: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_do_not_mark_local_variable_uses_in_sample_blocks() {
        let source = "proc Saturate:\n  sample:\n    x = in1\n    out1 = x - (x * x * x) * 0.1\n";
        let tokens = semantic_tokens_for_document(source, None);

        assert!(
            !tokens.iter().any(|token| {
                token.length == 1 && token.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE
            }),
            "tokens: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_mark_omni_params_as_parameters() {
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
    fn publish_diagnostics_does_not_immediately_clear_entry_uri() {
        let dir = mk_temp_dir("publish_diagnostics");
        let main = dir.join("main.omni");
        write_file(
            &main,
            "proc Saturate:\n  sample:\n    x = in1\n    out1 = x\n\ninit:\n  sat = Saturat()\n",
        );

        let mut server = LspServer::default();
        let normalized = server.session.open_document(
            &main,
            omni_daemon::DocumentVersion(1),
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
    fn did_open_publishes_diagnostics_immediately() {
        let dir = mk_temp_dir("did_open_publish");
        let main = dir.join("main.omni");
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
