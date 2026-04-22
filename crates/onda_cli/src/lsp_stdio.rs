use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use onda_daemon::{DaemonSession, DocumentVersion};
use onda_frontend::Diagnostic;
use serde::Deserialize;
use serde_json::{json, Value};

mod diagnostics;
mod path_utils;
mod semantic_tokens;

use diagnostics::{diagnostic_to_lsp, diagnostic_uri};
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
        },
        "serverInfo": {
            "name": "onda",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
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
        latest_full_text, lsp_document_path, path_to_file_uri, LspServer,
        TextDocumentContentChangeEvent,
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
}
