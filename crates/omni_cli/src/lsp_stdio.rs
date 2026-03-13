use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use omni_daemon::{DaemonSession, DocumentVersion};
use omni_frontend::Diagnostic;
use serde::Deserialize;
use serde_json::{json, Value};

const JSONRPC_VERSION: &str = "2.0";
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
        let envelope: ClientMessage = serde_json::from_value(message)
            .map_err(|err| format!("failed to decode lsp message: {err}"))?;
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
                let path = file_uri_to_path(&params.text_document.uri)
                    .map(|path| normalize_path(&path))
                    .map_err(invalid_params)?;
                let normalized = self.session.open_document(
                    &path,
                    DocumentVersion(params.text_document.version),
                    params.text_document.text,
                );
                self.document_uris
                    .insert(normalized, params.text_document.uri);
            }
            "textDocument/didChange" => {
                let params = parse_params::<DidChangeTextDocumentParams>(envelope.params)?;
                let Some(text) = latest_full_text(&params.content_changes) else {
                    return Ok(LoopControl::Continue);
                };
                let path = file_uri_to_path(&params.text_document.uri)
                    .map(|path| normalize_path(&path))
                    .map_err(invalid_params)?;
                let normalized = self.session.update_document(
                    &path,
                    DocumentVersion(params.text_document.version),
                    text,
                );
                self.document_uris
                    .insert(normalized, params.text_document.uri);
            }
            "textDocument/didSave" => {
                let params = parse_params::<DidSaveTextDocumentParams>(envelope.params)?;
                let path = file_uri_to_path(&params.text_document.uri)
                    .map(|path| normalize_path(&path))
                    .map_err(invalid_params)?;
                if let Some(text) = params.text {
                    let version = self
                        .session
                        .analysis()
                        .document(&path)
                        .map(|doc| doc.version)
                        .unwrap_or(DocumentVersion(0));
                    let normalized = self.session.update_document(&path, version, text);
                    self.document_uris
                        .insert(normalized, params.text_document.uri.clone());
                }
                self.publish_diagnostics_for_entry(&path, &params.text_document.uri, writer)?;
            }
            "textDocument/didClose" => {
                let params = parse_params::<DidCloseTextDocumentParams>(envelope.params)?;
                let path = file_uri_to_path(&params.text_document.uri)
                    .map(|path| normalize_path(&path))
                    .map_err(invalid_params)?;
                self.session.close_document(&path);
                self.document_uris.remove(&path);
                self.clear_entry_diagnostics(&path, &params.text_document.uri, writer)?;
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

    fn publish_diagnostics_for_entry(
        &mut self,
        entry_path: &Path,
        entry_uri: &str,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let snapshot = self.session.analyze_document(entry_path);
        let entry_path = normalize_path(entry_path);
        let default_uri = self
            .document_uris
            .get(&entry_path)
            .cloned()
            .unwrap_or_else(|| entry_uri.to_owned());
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
        entry_uri: &str,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let entry_path = normalize_path(entry_path);
        let mut cleared = self
            .published_by_entry
            .remove(&entry_path)
            .unwrap_or_default();
        cleared.insert(entry_uri.to_owned());
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
            }
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
        diagnostic_message, file_uri_to_path, latest_full_text, path_to_file_uri,
        TextDocumentContentChangeEvent,
    };
    use omni_frontend::{DiagCode, Diagnostic};
    use serde_json::json;

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
}
