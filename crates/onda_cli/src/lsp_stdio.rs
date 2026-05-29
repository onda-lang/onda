use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use onda_daemon::{DaemonSession, DocumentVersion};
use onda_frontend::Diagnostic;
use serde::Deserialize;
use serde_json::{json, Value};

mod completion;
mod diagnostics;
mod navigation;
mod path_utils;
mod position;
mod semantic_tokens;

use completion::{
    completion_items_for_document, completion_trigger_characters, CompletionPosition,
};
use diagnostics::{diagnostic_to_lsp, diagnostic_uri};
use navigation::{
    definition_for_document, document_symbols_for_document, hover_for_document, NavigationPosition,
};
use path_utils::{lsp_document_path, normalize_path, path_to_file_uri};
use semantic_tokens::{
    encode_semantic_tokens, semantic_token_legend, semantic_tokens_for_document,
};

const JSONRPC_VERSION: &str = "2.0";
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

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
    completion_snippets: bool,
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
                self.completion_snippets =
                    client_supports_completion_snippets(params.capabilities.as_ref());
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
            "textDocument/completion" => {
                let params = parse_params::<CompletionParams>(envelope.params)?;
                let result =
                    self.completions_for_uri(&params.text_document.uri, params.position)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/hover" => {
                let params = parse_params::<HoverParams>(envelope.params)?;
                let result = self.hover_for_uri(&params.text_document.uri, params.position)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/definition" => {
                let params = parse_params::<DefinitionParams>(envelope.params)?;
                let result = self.definition_for_uri(&params.text_document.uri, params.position)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/documentSymbol" => {
                let params = parse_params::<DocumentSymbolParams>(envelope.params)?;
                let result = self.document_symbols_for_uri(&params.text_document.uri)?;
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

    fn completions_for_uri(&self, uri: &str, position: Position) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(json!({
                "isIncomplete": false,
                "items": [],
            }));
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.analysis().overlay_map();
        let items = completion_items_for_document(
            &source,
            Some(&path),
            &overlays,
            CompletionPosition {
                line: position.line,
                character: position.character,
            },
            self.completion_snippets,
        );
        Ok(json!({
            "isIncomplete": false,
            "items": items,
        }))
    }

    fn hover_for_uri(&self, uri: &str, position: Position) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(Value::Null);
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.analysis().overlay_map();
        Ok(hover_for_document(
            &source,
            Some(&path),
            &overlays,
            NavigationPosition {
                line: position.line,
                character: position.character,
            },
        )
        .unwrap_or(Value::Null))
    }

    fn definition_for_uri(&self, uri: &str, position: Position) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(Value::Null);
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.analysis().overlay_map();
        Ok(definition_for_document(
            &source,
            Some(&path),
            &overlays,
            NavigationPosition {
                line: position.line,
                character: position.character,
            },
        )
        .unwrap_or(Value::Null))
    }

    fn document_symbols_for_uri(&self, uri: &str) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(json!([]));
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.analysis().overlay_map();
        Ok(json!(document_symbols_for_document(
            &source,
            Some(&path),
            &overlays,
        )))
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
    #[serde(default)]
    capabilities: Option<Value>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionParams {
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HoverParams {
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefinitionParams {
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentSymbolParams {
    text_document: TextDocumentIdentifier,
}

#[derive(Debug, Copy, Clone, Deserialize)]
struct Position {
    line: u32,
    character: u32,
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
                    "tokenTypes": semantic_token_legend(),
                    "tokenModifiers": [],
                }
            },
            "completionProvider": {
                "resolveProvider": false,
                "triggerCharacters": completion_trigger_characters(),
            },
            "hoverProvider": true,
            "definitionProvider": true,
            "documentSymbolProvider": true,
        },
        "serverInfo": {
            "name": "onda",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn client_supports_completion_snippets(capabilities: Option<&Value>) -> bool {
    capabilities
        .and_then(|value| {
            value
                .pointer("/textDocument/completion/completionItem/snippetSupport")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
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

fn invalid_params(message: String) -> String {
    format!("invalid params: {message}")
}

#[cfg(test)]
mod tests {
    use super::diagnostics::diagnostic_message;
    use super::path_utils::file_uri_to_path;
    use super::{
        initialize_result, latest_full_text, lsp_document_path, path_to_file_uri, LspServer,
        Position, TextDocumentContentChangeEvent,
    };
    use onda_frontend::{DiagCode, Diagnostic};
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
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

    fn stdlib_cache_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().expect("stdlib cache env lock")
    }

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set_path(name: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn clear_readonly_recursive(path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    clear_readonly_recursive(&entry.path());
                }
            }
        }
        let mut permissions = metadata.permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions).ok();
        }
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

    fn position_after(source: &str, needle: &str) -> Position {
        let end = source
            .rfind(needle)
            .map(|idx| idx + needle.len())
            .expect("needle should exist in source");
        let before = &source[..end];
        let line = before.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = before
            .rsplit_once('\n')
            .map(|(_, tail)| tail.encode_utf16().count())
            .unwrap_or_else(|| before.encode_utf16().count()) as u32;
        Position { line, character }
    }

    fn completion_labels_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> Vec<String> {
        completion_items_for(server, path, source, needle)
            .iter()
            .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
            .collect()
    }

    fn completion_items_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> Vec<serde_json::Value> {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let position = position_after(source, needle);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "textDocument/completion",
                    "params": {
                        "textDocument": { "uri": uri },
                        "position": {
                            "line": position.line,
                            "character": position.character,
                        }
                    }
                }),
                &mut writer,
            )
            .expect("completion should succeed");

        let messages = decode_lsp_messages(writer);
        messages
            .iter()
            .find(|message| message["id"] == json!(1))
            .and_then(|message| message["result"]["items"].as_array())
            .expect("completion response items")
            .clone()
    }

    fn request_with_position(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
        method: &str,
    ) -> serde_json::Value {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let position = position_after(source, needle);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method,
                    "params": {
                        "textDocument": { "uri": uri },
                        "position": {
                            "line": position.line,
                            "character": position.character,
                        }
                    }
                }),
                &mut writer,
            )
            .expect("lsp request should succeed");

        decode_lsp_messages(writer)
            .into_iter()
            .find(|message| message["id"] == json!(1))
            .and_then(|message| message.get("result").cloned())
            .expect("lsp result")
    }

    fn hover_markdown_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> Option<String> {
        request_with_position(server, path, source, needle, "textDocument/hover")
            .pointer("/contents/value")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
    }

    fn definition_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> serde_json::Value {
        request_with_position(server, path, source, needle, "textDocument/definition")
    }

    fn document_symbols_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
    ) -> Vec<serde_json::Value> {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "textDocument/documentSymbol",
                    "params": {
                        "textDocument": { "uri": uri }
                    }
                }),
                &mut writer,
            )
            .expect("document symbol request should succeed");

        decode_lsp_messages(writer)
            .into_iter()
            .find(|message| message["id"] == json!(1))
            .and_then(|message| message["result"].as_array().cloned())
            .expect("document symbol response")
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
    fn completion_triggers_include_call_argument_contexts() {
        let result = initialize_result(None);
        let triggers = result["capabilities"]["completionProvider"]["triggerCharacters"]
            .as_array()
            .expect("completion trigger characters")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();

        assert!(triggers.contains(&"("), "triggers: {triggers:?}");
        assert!(triggers.contains(&","), "triggers: {triggers:?}");
    }

    #[test]
    fn completion_expands_count_shorthand_proc_call_args() {
        let dir = mk_temp_dir("completion_count_shorthand_proc_args");
        let main = dir.join("main.onda");
        let source = r#"
proc Counted:
  ins 2
  params 2
  outs 1

  sample:
    out1 = in1

init:
  counted = Counted()

sample:
  out1 = counted(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "counted(");

        for expected in ["in1", "in2", "param1", "param2"] {
            assert!(
                labels.contains(&expected.to_owned()),
                "expected {expected} in labels: {labels:?}"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_expands_imported_std_count_shorthand_proc_call_args() {
        let dir = mk_temp_dir("completion_std_count_shorthand_proc_args");
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

init:
  conv = std::convolution<64, 1024>::ZeroLatencyConvolver<f32>()

sample:
  out1 = conv(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "conv(");

        assert!(
            labels.contains(&"in1".to_owned()),
            "expected std convolver input in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_symbol_alias_for_proc_instance_call_args() {
        let dir = mk_temp_dir("completion_use_symbol_alias_proc_call_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace Fx:
  proc Convolver:
    ins:
      input
    outs:
      out1
    params:
      gain = 1.0

    sample:
      out1 = input * gain

use Fx::Convolver as Conv

outs:
  out1

init:
  conv = Conv()

sample:
  out1 = conv(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "conv(");

        assert!(
            labels.contains(&"input".to_owned()),
            "expected aliased proc input in labels: {labels:?}"
        );
        assert!(
            labels.contains(&"gain".to_owned()),
            "expected aliased proc param in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_symbol_alias_for_function_call_args() {
        let dir = mk_temp_dir("completion_use_symbol_alias_function_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace Fx:
  def mix(input, gain):
    return input * gain

use Fx::mix as mx

outs:
  out1

sample:
  out1 = mx(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "mx(");

        assert!(
            labels.contains(&"input".to_owned()),
            "expected aliased def arg in labels: {labels:?}"
        );
        assert!(
            labels.contains(&"gain".to_owned()),
            "expected aliased def arg in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_namespace_alias_for_constructor_call_args() {
        let dir = mk_temp_dir("completion_use_namespace_alias_constructor_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace Fx:
  proc Convolver:
    params:
      gain = 1.0

    sample:
      out1 = gain

use Fx as fx

init:
  conv = fx::Convolver(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "fx::Convolver(");

        assert!(
            labels.contains(&"gain".to_owned()),
            "expected aliased constructor param in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_switches_to_expression_items_after_named_arg_equals() {
        let dir = mk_temp_dir("completion_named_arg_value");
        let main = dir.join("main.onda");
        let source = concat!(
            "proc Counted:\n",
            "  ins 1\n",
            "  outs 1\n",
            "\n",
            "  sample:\n",
            "    out1 = in1\n",
            "\n",
            "params:\n",
            "  gain = 1.0\n",
            "\n",
            "init:\n",
            "  counted = Counted()\n",
            "\n",
            "sample:\n",
            "  out1 = counted(in1 = "
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "in1 = ");

        assert!(
            labels.contains(&"gain".to_owned()),
            "named arg value should complete expression symbols: {labels:?}"
        );
        assert!(
            !labels.contains(&"in1".to_owned()),
            "named arg value should not repeat the named arg itself: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_returns_to_named_args_after_named_arg_value_comma() {
        let dir = mk_temp_dir("completion_named_arg_after_comma");
        let main = dir.join("main.onda");
        let source = concat!(
            "proc Counted:\n",
            "  ins 2\n",
            "  outs 1\n",
            "\n",
            "  sample:\n",
            "    out1 = in1\n",
            "\n",
            "params:\n",
            "  gain = 1.0\n",
            "\n",
            "init:\n",
            "  counted = Counted()\n",
            "\n",
            "sample:\n",
            "  out1 = counted(in1 = gain, "
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "gain, ");

        assert!(
            labels.contains(&"in1".to_owned()),
            "after a comma, completion should offer named args again: {labels:?}"
        );
        assert!(
            labels.contains(&"in2".to_owned()),
            "after a comma, completion should offer remaining proc inputs: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
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

    #[test]
    fn completion_hides_private_imported_use_symbols() {
        let dir = mk_temp_dir("completion_private_use");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = sh
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sh");

        assert!(labels.contains(&"shaped".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"shape".to_owned()),
            "private imported use should not be completed in importer: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_reexports_pub_use_symbols() {
        let dir = mk_temp_dir("completion_pub_use");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = sh
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sh");

        assert!(
            labels.contains(&"shape".to_owned()),
            "pub use should be completed in importer: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_filters_pinned_proc_params_after_receiver_dot() {
        let dir = mk_temp_dir("completion_proc_members");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    pin cutoff = 1000.0
    gain = 1.0

  outs:
    out1

  events:
    note_on(v):
      gain = v

  sample:
    out1 = gain

init:
  voice = Voice()

sample:
  voice.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "voice.");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"out1".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"note_on".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"init".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"cutoff".to_owned()),
            "pinned param should not be exposed after receiver dot: {labels:?}"
        );
        assert!(
            !labels.contains(&"params".to_owned()),
            "dynamic params should be hidden when a proc has pinned params: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_hides_dynamic_params_for_procs_without_params() {
        let dir = mk_temp_dir("completion_proc_no_params");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  outs:
    out1

  sample:
    out1 = 0.0

init:
  voice = Voice()

sample:
  voice.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "voice.");

        assert!(labels.contains(&"out1".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"init".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"params".to_owned()),
            "dynamic params should be hidden when a proc declares no params: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_leak_instances_from_other_scopes() {
        let dir = mk_temp_dir("completion_instance_scope_leak");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

def build():
  v = Voice()
  return 0.0

outs:
  out1

sample:
  v.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "v.");

        assert!(
            !labels.contains(&"gain".to_owned()),
            "function-local instance should not leak into sample member completion: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_filters_pinned_proc_params_in_live_call_args() {
        let dir = mk_temp_dir("completion_proc_call_args");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    pin cutoff = 1000.0
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

init:
  voice = Voice()

sample:
  out1 = voice(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "voice(");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"cutoff".to_owned()),
            "pinned param should not be exposed as a live call arg: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_top_level_runtime_symbols_in_sample() {
        let dir = mk_temp_dir("completion_top_level_runtime");
        let main = dir.join("main.onda");
        let source = r#"
params:
  gain = 1.0

outs:
  out1

init:
  phase = 0.0

sample:
  ga
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "ga");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_proc_scope_symbols_in_local_defs() {
        let dir = mk_temp_dir("completion_proc_scope_symbols");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  ins:
    dry

  params:
    gain = 1.0

  outs:
    out1

  init:
    cached = 0.0

  def update(delta):
    ga

  sample:
    out1 = gain
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "ga");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_leak_other_function_locals() {
        let dir = mk_temp_dir("completion_scope_leak");
        let main = dir.join("main.onda");
        let source = r#"
def other():
  secret = 1
  return secret

def current():
  return se
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "se");

        assert!(
            !labels.contains(&"secret".to_owned()),
            "locals from sibling defs should not be completed: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_leak_control_flow_locals() {
        let dir = mk_temp_dir("completion_control_flow_scope");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  if true:
    tmp = 1.0
  for i in 0..2:
    loop_tmp = f32(i)
  out1 = t
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "out1 = t");

        assert!(
            !labels.contains(&"tmp".to_owned()),
            "branch local should not complete after branch: {labels:?}"
        );
        assert!(
            !labels.contains(&"loop_tmp".to_owned()) && !labels.contains(&"i".to_owned()),
            "loop locals should not complete after loop: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_offer_future_locals() {
        let dir = mk_temp_dir("completion_future_local");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  earlier = 1.0
  out1 =
  later = earlier
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "out1 =");

        assert!(
            !labels.contains(&"later".to_owned()),
            "future local should not complete before declaration: {labels:?}"
        );
        assert!(
            labels.contains(&"earlier".to_owned()),
            "earlier local should still complete: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_hides_generated_onda_symbols() {
        let dir = mk_temp_dir("completion_generated_onda_symbols");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  visible = 1.0
  __onda_internal = 2.0
  out1 =
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "out1 =");

        assert!(
            labels.contains(&"visible".to_owned()),
            "ordinary local should complete: {labels:?}"
        );
        assert!(
            labels.iter().all(|label| !label.starts_with("__onda")),
            "generated/internal symbols should not complete: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_duplicate_init_state_as_local() {
        let dir = mk_temp_dir("completion_init_state_dedup");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

init:
  phase = 0.0
  ph

sample:
  out1 = phase
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "  ph");
        let phase_details = items
            .iter()
            .filter(|item| item["label"] == json!("phase"))
            .map(|item| item["detail"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            phase_details,
            vec!["state".to_owned()],
            "init state should complete once as state: {items:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_namespace_members_on_incomplete_line() {
        let dir = mk_temp_dir("completion_namespace_members");
        let main = dir.join("main.onda");
        let source = r#"
import std/osc

outs:
  out1

sample:
  out1 = std::osc::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "std::osc::");

        assert!(labels.contains(&"Sine".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Saw".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_members_after_std_generic_namespace() {
        let dir = mk_temp_dir("completion_std_generic_namespace_members");
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  proc Use<T>:
    init:
      conv = std::convolution<FFTSize, MaxImpulseLen>::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(
            &mut server,
            &main,
            source,
            "std::convolution<FFTSize, MaxImpulseLen>::",
        );

        assert!(
            labels.contains(&"BlockConvolver".to_owned()),
            "labels: {labels:?}"
        );
        assert!(
            labels.contains(&"TimeDomainConvolver".to_owned()),
            "labels: {labels:?}"
        );
        assert!(
            labels.contains(&"ZeroLatencyConvolver".to_owned()),
            "labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_relative_generic_namespace_members_from_current_scope() {
        let dir = mk_temp_dir("completion_relative_generic_namespace_members");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace Convolution2<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        outs<T> 1
        sample:
          out1 = 0.0

  namespace Convolution3<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        init:
          c = Convolution2<FFTSize, MaxKernel>::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(
            &mut server,
            &main,
            source,
            "Convolution2<FFTSize, MaxKernel>::",
        );

        assert!(labels.contains(&"Mono".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_current_namespace_symbols_unqualified() {
        let dir = mk_temp_dir("completion_current_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  struct Shape:
    x: f32

  proc Voice:
    outs:
      out1
    sample:
      out1 = 0.0

  namespace Inner:
    const X = 1

  def shape(x):
    return x

  def run(x):
    return (
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "return (");

        assert!(labels.contains(&"Bias".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Shape".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Voice".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Inner".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"shape".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_orders_namespace_members_by_declaration_kind() {
        let dir = mk_temp_dir("completion_namespace_member_order");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  namespace Zoo:
    const X = 1

  namespace Alpha:
    const X = 1

  struct ZStruct:
    value: f32

  struct AStruct:
    value: f32

  proc ZProc:
    outs:
      out1
    sample:
      out1 = 0.0

  proc AProc:
    outs:
      out1
    sample:
      out1 = 0.0

  def zdef():
    return 0.0

  def adef():
    return 0.0

  const ZConst = 1
  const AConst = 1

outs:
  out1

sample:
  out1 = DSP::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "DSP::");
        let expected = vec![
            "Alpha", "Zoo", "AStruct", "ZStruct", "AProc", "ZProc", "adef", "zdef", "AConst",
            "ZConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("Alpha", "10_Alpha"),
            ("AStruct", "11_AStruct"),
            ("AProc", "12_AProc"),
            ("adef", "13_adef"),
            ("AConst", "15_AConst"),
        ] {
            let item = items
                .iter()
                .find(|item| item["label"] == json!(label))
                .unwrap_or_else(|| panic!("missing {label} in {items:?}"));
            assert_eq!(item["sortText"], json!(sort_text), "item: {item:?}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_orders_scope_symbols_before_declarations_and_events_after_defs() {
        let dir = mk_temp_dir("completion_scope_symbol_order");
        let main = dir.join("main.onda");
        let source = r#"
namespace gNs:
  const X = 1

struct hStruct:
  value: f32

proc iProc:
  outs:
    out1
  sample:
    out1 = 0.0

def j_def():
  return 0.0

const zConst = 1

proc Voice:
  ins:
    c_in

  buffers:
    b_buf: f32

  params:
    e_param = 0.0

  outs:
    d_out

  events:
    k_event():
      e_param = 1.0

  def f_proc_def():
    return e_param

  sample:
    a_local = 0.0
    d_out =
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "d_out =");
        let expected = vec![
            "a_local",
            "b_buf",
            "c_in",
            "d_out",
            "e_param",
            "f_proc_def",
            "gNs",
            "hStruct",
            "iProc",
            "j_def",
            "k_event",
            "zConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("a_local", "00_a_local"),
            ("b_buf", "01_b_buf"),
            ("f_proc_def", "02_f_proc_def"),
            ("gNs", "10_gNs"),
            ("j_def", "13_j_def"),
            ("k_event", "14_k_event"),
            ("zConst", "15_zConst"),
        ] {
            let item = items
                .iter()
                .find(|item| item["label"] == json!(label))
                .unwrap_or_else(|| panic!("missing {label} in {items:?}"));
            assert_eq!(item["sortText"], json!(sort_text), "item: {item:?}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_namespace_aliases() {
        let dir = mk_temp_dir("completion_use_namespace_alias");
        let main = dir.join("main.onda");
        let source = r#"
import std/fft
use std::fft<8> as fft8

outs:
  out1

sample:
  out1 = fft8::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "fft8::");

        assert!(labels.contains(&"FFT".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_shows_generics_for_generic_symbols() {
        let dir = mk_temp_dir("completion_generics");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP<N = 4>:
  struct Shape<T>:
    x: T

  proc Voice<T>:
    outs:
      out1
    sample:
      out1 = 0.0

  namespace Inner<M = 2>:
    const X = 1

  def run(x):
    return V
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        server.completion_snippets = true;
        let items = completion_items_for(&mut server, &main, source, "return V");
        let voice = items
            .iter()
            .find(|item| item["label"] == json!("Voice"))
            .expect("Voice completion item");

        assert_eq!(voice["labelDetails"]["detail"], json!("<T>()"));
        assert_eq!(voice["insertText"], json!("Voice<${1:T}>($2)"));
        assert_eq!(voice["insertTextFormat"], json!(2));
        assert!(
            voice["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Voice<T>"),
            "item: {voice:?}"
        );

        let items = completion_items_for(&mut server, &main, source, "return ");
        let shape = items
            .iter()
            .find(|item| item["label"] == json!("Shape"))
            .expect("Shape completion item");
        let inner = items
            .iter()
            .find(|item| item["label"] == json!("Inner"))
            .expect("Inner completion item");

        assert_eq!(shape["labelDetails"]["detail"], json!("<T>"));
        assert_eq!(shape["insertText"], json!("Shape<${1:T}>"));
        assert_eq!(shape["insertTextFormat"], json!(2));
        assert!(
            shape["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Shape<T>"),
            "item: {shape:?}"
        );
        assert_eq!(inner["labelDetails"]["detail"], json!("<M>"));
        assert_eq!(inner["insertText"], json!("Inner<${1:M}>"));
        assert_eq!(inner["insertTextFormat"], json!(2));
        assert!(
            inner["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Inner<M>"),
            "item: {inner:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_inserts_std_complex_generic_snippet() {
        let dir = mk_temp_dir("completion_std_complex_generic");
        let main = dir.join("main.onda");
        let source = r#"
import std/complex

outs:
  out1

init:
  z: std::complex::C

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        server.completion_snippets = true;
        let items = completion_items_for(&mut server, &main, source, "std::complex::C");
        let complex = items
            .iter()
            .find(|item| item["label"] == json!("Complex"))
            .expect("Complex completion item");

        assert_eq!(complex["labelDetails"]["detail"], json!("<T>"));
        assert_eq!(complex["insertText"], json!("Complex<${1:T}>"));
        assert_eq!(complex["insertTextFormat"], json!(2));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_keeps_generics_without_snippets_and_no_fake_constructor_namespace() {
        let dir = mk_temp_dir("completion_generic_plain_text");
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  struct Box<T>:
    value: T

  def id<T>(x: T):
    return x

  def make<T>(x: T):
    local: B
    return i

  proc Use<T>:
    init:
      conv = std::convolution<FFTSize, MaxImpulseLen>::BlockConv
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "BlockConv");
        let block_convolver_items = items
            .iter()
            .filter(|item| item["label"] == json!("BlockConvolver"))
            .collect::<Vec<_>>();
        assert_eq!(
            block_convolver_items.len(),
            1,
            "BlockConvolver should have one completion item: {items:?}"
        );
        let block_convolver = block_convolver_items[0];
        assert_eq!(block_convolver["kind"], json!(4));
        assert_eq!(block_convolver["labelDetails"]["detail"], json!("<T>()"));
        assert_eq!(block_convolver["insertText"], json!("BlockConvolver<T>("));
        assert!(
            !items
                .iter()
                .any(|item| item["label"] == json!("BlockConvolver") && item["kind"] == json!(9)),
            "constructor should not also appear as a namespace: {items:?}"
        );

        let items = completion_items_for(&mut server, &main, source, "local: B");
        let box_item = items
            .iter()
            .find(|item| item["label"] == json!("Box"))
            .expect("Box completion item");
        assert_eq!(box_item["insertText"], json!("Box<T>"));

        let items = completion_items_for(&mut server, &main, source, "return i");
        let id_item = items
            .iter()
            .find(|item| item["label"] == json!("id"))
            .expect("id completion item");
        assert_eq!(id_item["insertText"], json!("id<T>("));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hover_describes_user_defs() {
        let dir = mk_temp_dir("hover_user_defs");
        let main = dir.join("main.onda");
        let source = r#"
def scale(x):
  return x

outs:
  out1

sample:
  out1 = scale(0.5)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let hover = hover_markdown_for(&mut server, &main, source, "scale")
            .expect("hover should resolve user def");

        assert!(hover.contains("def scale(x)"), "hover: {hover}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hover_describes_builtin_calls() {
        let dir = mk_temp_dir("hover_builtin_calls");
        let main = dir.join("main.onda");
        let source = r#"
init:
  buf: f32[4]
  n = buf.len()

sample:
  x = unsafe_read(buf, 0)
  y = fabs(0.0 - 1.0)
  sr = HOST_SR
  out1 = x + y + sr
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let unsafe_read = hover_markdown_for(&mut server, &main, source, "unsafe_read")
            .expect("hover should resolve unsafe_read builtin");
        let len = hover_markdown_for(&mut server, &main, source, "len")
            .expect("hover should resolve len builtin");
        let fabs = hover_markdown_for(&mut server, &main, source, "fabs")
            .expect("hover should resolve fabs builtin alias");
        let host_sr = hover_markdown_for(&mut server, &main, source, "HOST_SR")
            .expect("hover should resolve HOST_SR builtin const");

        assert!(
            unsafe_read.contains("built-in call unsafe_read(...)"),
            "hover: {unsafe_read}"
        );
        assert!(len.contains("built-in call .len(...)"), "hover: {len}");
        assert!(fabs.contains("built-in call fabs(...)"), "hover: {fabs}");
        assert!(
            host_sr.contains("builtin const HOST_SR: f32"),
            "hover: {host_sr}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn callable_display_includes_argument_signatures() {
        let dir = mk_temp_dir("callable_display_argument_signatures");
        let main = dir.join("main.onda");
        let source = r#"
def scale<T>(x: T, amount: f32 = 1.0):
  return x

proc Voice<T>:
  ins<T>:
    input

  params:
    pin cutoff: f32 = 1000.0
    gain: f32 = 1.0

  buffers:
    table: buffer[f32]

  event set(v: f32 = 0.5):
    gain = v

  sample:
    out1 = input * gain

init:
  voice = Voice<f32>(cutoff = 800.0, gain = 0.25, table = table)

sample:
  voice.init(gain = 0.5)
  voice.set(v = 0.75)
  out1 = voice(0.1, gain = 0.5) + scale<f32>(0.25)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let def_hover = hover_markdown_for(&mut server, &main, source, "scale")
            .expect("hover should resolve generic def");
        let constructor_hover = hover_markdown_for(&mut server, &main, source, "Voice")
            .expect("hover should resolve constructor");
        let call_hover = hover_markdown_for(&mut server, &main, source, "voice")
            .expect("hover should resolve proc call");
        let init_hover = hover_markdown_for(&mut server, &main, source, "init")
            .expect("hover should resolve proc init");
        let event_hover = hover_markdown_for(&mut server, &main, source, "set")
            .expect("hover should resolve event");

        assert!(
            def_hover.contains("def scale<T>(x: T, amount: f32 = 1.0)"),
            "hover: {def_hover}"
        );
        assert!(
            constructor_hover.contains(
                "proc Voice<T>(pin cutoff: f32 = 1000.0, gain: f32 = 1.0, table: buffer[f32])"
            ),
            "hover: {constructor_hover}"
        );
        assert!(
            call_hover.contains("proc call voice(input: T, gain: f32 = 1.0)"),
            "hover: {call_hover}"
        );
        assert!(
            init_hover.contains("event init(pin cutoff: f32 = 1000.0, gain: f32 = 1.0)"),
            "hover: {init_hover}"
        );
        assert!(
            event_hover.contains("event set(v: f32 = 0.5)"),
            "hover: {event_hover}"
        );

        let def_items = completion_items_for(&mut server, &main, source, "scale");
        let scale_item = def_items
            .iter()
            .find(|item| item["label"] == json!("scale"))
            .expect("scale completion item");
        assert_eq!(
            scale_item["labelDetails"]["detail"],
            json!("<T>(x: T, amount: f32 = 1.0)")
        );
        assert_eq!(
            scale_item["detail"],
            json!("def scale<T>(x: T, amount: f32 = 1.0)")
        );

        let member_items = completion_items_for(&mut server, &main, source, "voice.");
        let init_item = member_items
            .iter()
            .find(|item| item["label"] == json!("init"))
            .expect("init completion item");
        let set_item = member_items
            .iter()
            .find(|item| item["label"] == json!("set"))
            .expect("set completion item");
        assert_eq!(
            init_item["detail"],
            json!("event init(pin cutoff: f32 = 1000.0, gain: f32 = 1.0)")
        );
        assert_eq!(set_item["detail"], json!("event set(v: f32 = 0.5)"));
        assert_eq!(
            init_item["labelDetails"]["detail"],
            json!("(pin cutoff: f32 = 1000.0, gain: f32 = 1.0)")
        );
        assert_eq!(set_item["labelDetails"]["detail"], json!("(v: f32 = 0.5)"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_relative_namespaced_proc_instance_call_args() {
        let dir = mk_temp_dir("completion_relative_namespaced_proc_instance_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace Convolution2<FFTSize = 2048, MaxKernel = 8192>:
    namespace Mono:
      proc ar<T>:
        ins<T>:
          in1
          trigger

        params:
          gain: f32 = 1.0

        buffers:
          kernel: buffer[T]

        sample:
          out1 = in1 * gain

  namespace Convolution3<FFTSize = 2048, MaxKernel = 8192>:
    namespace Mono:
      proc ar<T>:
        ins<T>:
          in1
          trigger

        buffers:
          kernel: buffer[T]

        init:
          conv = Convolution2<FFTSize, MaxKernel>::Mono::ar<T>(kernel = kernel)

        sample:
          conv.init(gain = 0.5)
          out1 = conv(in1, trigger)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let call_labels = completion_labels_for(&mut server, &main, source, "conv(");
        assert!(
            call_labels.contains(&"in1".to_owned()),
            "labels: {call_labels:?}"
        );
        assert!(
            call_labels.contains(&"trigger".to_owned()),
            "labels: {call_labels:?}"
        );
        assert!(
            call_labels.contains(&"gain".to_owned()),
            "labels: {call_labels:?}"
        );

        let init_labels = completion_labels_for(&mut server, &main, source, "conv.init(");
        assert!(
            init_labels.contains(&"gain".to_owned()),
            "labels: {init_labels:?}"
        );

        let ctor_labels = completion_labels_for(&mut server, &main, source, "Mono::ar<T>(kernel");
        assert!(
            ctor_labels.contains(&"kernel".to_owned()),
            "labels: {ctor_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_current_namespace_symbols() {
        let dir = mk_temp_dir("definition_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  def shape(x):
    return x + Bias
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Bias");

        assert_ne!(definition, json!(null), "definition should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(2));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_top_level_consts() {
        let dir = mk_temp_dir("definition_top_level_const");
        let main = dir.join("main.onda");
        let source = r#"
const Scale = 0.5

outs:
  out1

sample:
  out1 = Scale
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Scale");

        assert_ne!(definition, json!(null), "top-level const should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_runtime_scope_consts() {
        let dir = mk_temp_dir("definition_runtime_const");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  const Bias = 0.5
  out1 = Bias
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Bias");

        assert_ne!(definition, json!(null), "runtime const should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(5));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_returns_null_for_explicit_use_ambiguity() {
        let dir = mk_temp_dir("definition_use_ambiguity");
        let main = dir.join("main.onda");
        let source = r#"
import std/math
use std::math

outs:
  out1

def clamp(x, lo, hi):
  return x

sample:
  out1 = clamp(2.0, 0.0, 1.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "out1 = clamp");

        assert_eq!(
            definition,
            json!(null),
            "ambiguous use collision should not jump to one arbitrary target"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_honors_imported_use_privacy() {
        let dir = mk_temp_dir("definition_use_privacy");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = shaped(0.0)
  out1 = shape(0.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let shaped = definition_for(&mut server, &main, source, "shaped");
        let shape = definition_for(&mut server, &main, source, "shape");

        assert_ne!(shaped, json!(null), "imported public def should resolve");
        assert_eq!(
            shape,
            json!(null),
            "private imported use should not resolve in importer"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_reexports_pub_use_symbols() {
        let dir = mk_temp_dir("definition_pub_use");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = shape(0.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "shape");

        assert_ne!(definition, json!(null), "pub use should resolve");
        assert!(definition["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_namespace_local_relative_use() {
        let dir = mk_temp_dir("definition_namespace_local_relative_use");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  namespace helpers:
    def shape(x):
      return x

  use helpers

  def run(x):
    return shape(x)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "return shape");

        assert_ne!(
            definition,
            json!(null),
            "namespace-local relative use should resolve"
        );
        assert_eq!(definition["range"]["start"]["line"], json!(3));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_namespace_local_use_does_not_leak() {
        let dir = mk_temp_dir("definition_namespace_local_use_leak");
        let main = dir.join("main.onda");
        let source = r#"
namespace A:
  namespace helpers:
    def hidden(x):
      return x

  use helpers

  def run(x):
    return hidden(x)

namespace B:
  def run(x):
    return hidden(x)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "return hidden");

        assert_eq!(
            definition,
            json!(null),
            "namespace-local use should not resolve outside its namespace"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_imported_defs() {
        let dir = mk_temp_dir("definition_imported_def");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
def shaped(x, amount = 1.0):
  return x * amount
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = shaped(0.0, amount = 0.5)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "shaped");
        let arg_definition = definition_for(&mut server, &main, source, "amount");

        assert_ne!(definition, json!(null), "imported def should resolve");
        assert!(definition["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(definition["range"]["start"]["line"], json!(1));
        assert_ne!(
            arg_definition,
            json!(null),
            "imported def named argument should resolve"
        );
        assert!(arg_definition["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(arg_definition["range"]["start"]["line"], json!(1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_imported_proc_constructors_and_events() {
        let dir = mk_temp_dir("definition_imported_proc_event");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  event set(v):
    gain = v

  sample:
    out1 = gain
"#,
        );
        let source = r#"
import lib

outs:
  out1

init:
  voice = Voice()

sample:
  voice.set(v = 0.5)
  out1 = voice()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let proc_def = definition_for(&mut server, &main, source, "Voice");
        let event_def = definition_for(&mut server, &main, source, "set");
        let event_arg = definition_for(&mut server, &main, source, "set(v");

        assert_ne!(proc_def, json!(null), "imported proc should resolve");
        assert!(proc_def["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(proc_def["range"]["start"]["line"], json!(1));

        assert_ne!(event_def, json!(null), "imported proc event should resolve");
        assert!(event_def["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(event_def["range"]["start"]["line"], json!(8));
        assert_ne!(
            event_arg,
            json!(null),
            "imported proc-event named argument should resolve"
        );
        assert!(event_arg["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(event_arg["range"]["start"]["line"], json!(8));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_materializes_stdlib_goto_targets_readonly() {
        let dir = mk_temp_dir("definition_stdlib_materialized");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/math

outs:
  out1

sample:
  out1 = std::math::clamp(0.5, 0.0, 1.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let import_target = definition_for(&mut server, &main, source, "std/math");
        let clamp_target = definition_for(&mut server, &main, source, "clamp");

        for target in [import_target, clamp_target] {
            assert_ne!(target, json!(null), "stdlib goto should resolve");
            let uri = target["uri"].as_str().expect("stdlib goto uri");
            let path = file_uri_to_path(uri).expect("stdlib target should be a file uri");
            assert!(
                path.starts_with(&cache),
                "stdlib target should be inside cache: {}",
                path.display()
            );
            assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some("math.onda")
            );
            assert!(path.exists(), "materialized stdlib file should exist");
            assert!(
                fs::metadata(&path)
                    .expect("materialized stdlib metadata")
                    .permissions()
                    .readonly(),
                "materialized stdlib file should be read-only"
            );
            assert_eq!(
                fs::read_to_string(&path).expect("read materialized stdlib"),
                onda_frontend::stdlib_module_source("std/math").expect("embedded std/math")
            );
        }

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_stdlib_proc_events_from_namespaced_generic_instances() {
        let dir = mk_temp_dir("definition_stdlib_proc_event_namespace");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

proc Plain<T>:
  outs<T> 1
  sample:
    out1 = 0.0

init:
  conv = Plain<f32>()

namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  proc Use<T>:
    ins<T> 1
    outs<T> 1

    init:
      conv = std::convolution<FFTSize, MaxImpulseLen>::BlockConvolver<T>()
      ir: T[MaxImpulseLen]

    sample:
      conv.set_impulse(ir)
      out1 = conv(in1)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "set_impulse");

        assert_ne!(
            definition,
            json!(null),
            "stdlib proc event should resolve from a namespaced proc instance"
        );
        let uri = definition["uri"].as_str().expect("stdlib event uri");
        let path = file_uri_to_path(uri).expect("stdlib event should be a file uri");
        assert!(
            path.starts_with(&cache),
            "stdlib event should materialize inside cache: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("convolution.onda")
        );
        let line = definition["range"]["start"]["line"]
            .as_u64()
            .expect("event line") as usize;
        let materialized = fs::read_to_string(&path).expect("read materialized stdlib");
        let target_line = materialized
            .lines()
            .nth(line)
            .expect("definition line should exist");
        assert!(
            target_line.contains("set_impulse"),
            "definition should target set_impulse, got line: {target_line}"
        );

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_relative_qualified_paths_from_namespace_scope() {
        let dir = mk_temp_dir("definition_relative_qualified_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace Convolution2<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        outs<T> 1
        sample:
          out1 = 0.0

  namespace Convolution3<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        init:
          conv = Convolution2<FFTSize, MaxKernel>::Mono::ar<T>()
        sample:
          out1 = conv()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let mono = definition_for(
            &mut server,
            &main,
            source,
            "Convolution2<FFTSize, MaxKernel>::Mono",
        );
        let ar = definition_for(
            &mut server,
            &main,
            source,
            "Convolution2<FFTSize, MaxKernel>::Mono::ar",
        );

        assert_ne!(mono, json!(null), "qualified namespace should resolve");
        assert_eq!(
            mono["range"]["start"]["line"],
            json!(3),
            "Mono should resolve to Convolution2::Mono, not Convolution3::Mono"
        );
        assert_ne!(ar, json!(null), "qualified proc should resolve");
        assert_eq!(
            ar["range"]["start"]["line"],
            json!(4),
            "ar should resolve to Convolution2::Mono::ar, not Convolution3::Mono::ar"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_namespace_template_parameters_as_consts() {
        let dir = mk_temp_dir("definition_namespace_template_params");
        let main = dir.join("main.onda");
        let source = r#"
namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  proc Use<T>:
    ins<T> 1
    outs<T> 1

    init:
      ir: T[MaxImpulseLen]

    sample:
      for i in 0..FFTSize:
        out1 = in1
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let max_impulse = definition_for(&mut server, &main, source, "T[MaxImpulseLen");
        let fft_size = definition_for(&mut server, &main, source, "0..FFTSize");

        for definition in [max_impulse, fft_size] {
            assert_ne!(
                definition,
                json!(null),
                "namespace template parameter should resolve"
            );
            assert_eq!(definition["range"]["start"]["line"], json!(1));
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_receiver_members_and_hides_pinned_params() {
        let dir = mk_temp_dir("definition_receiver_members");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    pin cutoff = 1000.0
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

init:
  voice = Voice()

sample:
  out1 = voice.gain
  out1 = voice.cutoff
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let gain = definition_for(&mut server, &main, source, "gain");
        let cutoff = definition_for(&mut server, &main, source, "cutoff");

        assert_ne!(gain, json!(null), "public proc param should resolve");
        assert_eq!(gain["range"]["start"]["line"], json!(4));
        assert_eq!(
            cutoff,
            json!(null),
            "pinned proc param should not resolve through receiver access"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_does_not_leak_instances_from_other_scopes() {
        let dir = mk_temp_dir("definition_instance_scope_leak");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

def build():
  v = Voice()
  return 0.0

outs:
  out1

sample:
  out1 = v.gain
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "v.gain");

        assert_eq!(
            definition,
            json!(null),
            "function-local instance should not leak into sample member definition"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_does_not_leak_control_flow_locals() {
        let dir = mk_temp_dir("definition_control_flow_scope");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  if true:
    tmp = 1.0
  for i in 0..2:
    loop_tmp = f32(i)
  out1 = tmp + loop_tmp + f32(i)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let branch_local = definition_for(&mut server, &main, source, "out1 = tmp");
        let loop_local = definition_for(&mut server, &main, source, "tmp + loop_tmp");
        let loop_var = definition_for(&mut server, &main, source, "f32(i");

        assert_eq!(
            branch_local,
            json!(null),
            "branch local should not resolve after branch"
        );
        assert_eq!(
            loop_local,
            json!(null),
            "loop body local should not resolve after loop"
        );
        assert_eq!(
            loop_var,
            json!(null),
            "loop variable should not resolve after loop"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_does_not_resolve_future_locals() {
        let dir = mk_temp_dir("definition_future_local");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  earlier = 1.0
  out1 = earlier + later
  later = 2.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let earlier = definition_for(&mut server, &main, source, "out1 = earlier");
        let later = definition_for(&mut server, &main, source, "+ later");

        assert_ne!(earlier, json!(null), "earlier local should resolve");
        assert_eq!(
            later,
            json!(null),
            "future local should not resolve before declaration"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_top_level_params_and_init_state() {
        let dir = mk_temp_dir("definition_top_level_runtime_scope");
        let main = dir.join("main.onda");
        let source = r#"
params:
  gain = 1.0

outs:
  out1

init:
  phase = 0.0

sample:
  out1 = gain + phase
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let gain = definition_for(&mut server, &main, source, "gain");
        let phase = definition_for(&mut server, &main, source, "phase");

        assert_ne!(gain, json!(null), "top-level param should resolve");
        assert_eq!(gain["range"]["start"]["line"], json!(2));
        assert_ne!(phase, json!(null), "top-level init state should resolve");
        assert_eq!(phase["range"]["start"]["line"], json!(8));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_proc_params_and_init_state() {
        let dir = mk_temp_dir("definition_proc_runtime_scope");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  init:
    cached = 0.0

  sample:
    out1 = gain + cached
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let gain = definition_for(&mut server, &main, source, "gain");
        let cached = definition_for(&mut server, &main, source, "cached");

        assert_ne!(gain, json!(null), "proc param should resolve");
        assert_eq!(gain["range"]["start"]["line"], json!(3));
        assert_ne!(cached, json!(null), "proc init state should resolve");
        assert_eq!(cached["range"]["start"]["line"], json!(9));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn document_symbols_include_nested_namespace_items() {
        let dir = mk_temp_dir("document_symbols_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  struct Shape:
    x: f32

  def shape(x):
    return x
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let symbols = document_symbols_for(&mut server, &main, source);
        let dsp = symbols
            .iter()
            .find(|symbol| symbol["name"] == json!("DSP"))
            .expect("DSP namespace symbol");
        let child_names = dsp["children"]
            .as_array()
            .expect("namespace children")
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();

        assert!(child_names.contains(&"Bias"), "children: {child_names:?}");
        assert!(child_names.contains(&"Shape"), "children: {child_names:?}");
        assert!(child_names.contains(&"shape"), "children: {child_names:?}");

        fs::remove_dir_all(&dir).ok();
    }
}
