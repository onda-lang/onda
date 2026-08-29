use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use onda_frontend::{
    load_program_file_from_snapshot, load_program_file_with_overlays, parse_stdlib_module,
    Diagnostic, Program, SourceManifest,
};
use onda_semantics::{AnalysisOptions, AnalysisSession, DocumentVersion};
use serde::Deserialize;
use serde_json::{json, Value};

mod completion;
mod diagnostics;
mod language_intrinsics;
mod namespace_resolution;
mod navigation;
mod param_domain;
mod path_utils;
mod position;
mod semantic_tokens;
mod unsafe_index;

use completion::{
    completion_items_for_document_with_index, completion_trigger_characters,
    CompletionIndexSnapshot, CompletionPosition,
};
use diagnostics::{diagnostic_to_lsp, diagnostic_uri};
use navigation::{
    definition_for_document_with_parsed, document_symbols_for_document_with_parsed,
    hover_for_document_with_parsed, signature_help_for_document_with_parsed,
    stdlib_virtual_document, stdlib_virtual_source, NavigationPosition,
};
use path_utils::{lsp_document_path, normalize_path, path_to_file_uri};
use semantic_tokens::{
    encode_semantic_tokens, semantic_token_legend, semantic_tokens_for_document_source_only,
    semantic_tokens_for_document_with_parsed,
};

const JSONRPC_VERSION: &str = "2.0";
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;
const CHANGE_DIAGNOSTIC_DEBOUNCE: Duration = Duration::from_millis(400);
const IMMEDIATE_DIAGNOSTICS: Duration = Duration::from_millis(0);

pub fn run_stdio_loop() -> Result<(), String> {
    let (core_tx, core_rx) = mpsc::channel::<CoreEvent>();
    spawn_lsp_reader(core_tx.clone());

    let (immediate_diagnostic_tx, immediate_diagnostic_rx) = mpsc::channel::<DiagnosticJob>();
    let (background_diagnostic_tx, background_diagnostic_rx) = mpsc::channel::<DiagnosticJob>();
    spawn_diagnostic_worker(immediate_diagnostic_rx, core_tx.clone());
    spawn_diagnostic_worker(background_diagnostic_rx, core_tx);

    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let mut core = LspCore::new(immediate_diagnostic_tx, background_diagnostic_tx);
    core.run(core_rx, &mut writer)
}

fn spawn_lsp_reader(core_tx: mpsc::Sender<CoreEvent>) {
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            match read_lsp_message(&mut reader) {
                Ok(Some(message)) => {
                    if core_tx.send(CoreEvent::ClientMessage(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = core_tx.send(CoreEvent::ReaderClosed);
                    break;
                }
                Err(err) => {
                    let _ = core_tx.send(CoreEvent::ReaderError(err));
                    break;
                }
            }
        }
    });
}

fn spawn_diagnostic_worker(
    diagnostic_rx: mpsc::Receiver<DiagnosticJob>,
    core_tx: mpsc::Sender<CoreEvent>,
) {
    thread::spawn(move || {
        while let Ok(job) = diagnostic_rx.recv() {
            let result = run_diagnostic_job(job);
            if core_tx
                .send(CoreEvent::DiagnosticsReady(Box::new(result)))
                .is_err()
            {
                break;
            }
        }
    });
}

enum LoopControl {
    Continue,
    ExitSuccess,
    ExitFailure,
}

enum CoreEvent {
    ClientMessage(Value),
    DiagnosticsReady(Box<DiagnosticJobResult>),
    ReaderClosed,
    ReaderError(String),
}

#[derive(Debug, Clone)]
struct DiagnosticJob {
    entry_path: PathBuf,
    generation: u64,
    open_documents: Vec<DiagnosticOpenDocument>,
}

#[derive(Debug, Clone)]
struct DiagnosticOpenDocument {
    path: PathBuf,
    version: DocumentVersion,
    text: String,
}

#[derive(Debug)]
struct DiagnosticJobResult {
    entry_path: PathBuf,
    generation: u64,
    diagnostics: Vec<Diagnostic>,
    sources: SourceManifest,
    parse_succeeded: bool,
    parse_fingerprint: Option<DocumentFingerprint>,
    completion_index_snapshot: Option<CompletionIndexSnapshot>,
}

#[derive(Debug, Clone, Copy)]
enum DiagnosticDelay {
    Immediate,
    Debounced,
}

impl DiagnosticDelay {
    fn duration(self) -> Duration {
        match self {
            DiagnosticDelay::Immediate => IMMEDIATE_DIAGNOSTICS,
            DiagnosticDelay::Debounced => CHANGE_DIAGNOSTIC_DEBOUNCE,
        }
    }
}

#[derive(Debug)]
struct DiagnosticScheduleRequest {
    entries: Vec<PathBuf>,
    delay: DiagnosticDelay,
}

#[derive(Debug, Clone)]
struct ScheduledDiagnostic {
    generation: u64,
    due_at: Instant,
    delay: DiagnosticDelay,
}

struct LspCore {
    server: LspServer,
    immediate_diagnostic_tx: mpsc::Sender<DiagnosticJob>,
    background_diagnostic_tx: mpsc::Sender<DiagnosticJob>,
    pending_diagnostics: HashMap<PathBuf, ScheduledDiagnostic>,
    diagnostic_generations: HashMap<PathBuf, u64>,
}

impl LspCore {
    fn new(
        immediate_diagnostic_tx: mpsc::Sender<DiagnosticJob>,
        background_diagnostic_tx: mpsc::Sender<DiagnosticJob>,
    ) -> Self {
        let server = LspServer {
            defer_diagnostics: true,
            ..LspServer::default()
        };
        Self {
            server,
            immediate_diagnostic_tx,
            background_diagnostic_tx,
            pending_diagnostics: HashMap::new(),
            diagnostic_generations: HashMap::new(),
        }
    }

    fn run(
        &mut self,
        core_rx: mpsc::Receiver<CoreEvent>,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        loop {
            self.dispatch_due_diagnostics();
            let event = match self.next_diagnostic_timeout() {
                Some(timeout) => match core_rx.recv_timeout(timeout) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                },
                None => match core_rx.recv() {
                    Ok(event) => event,
                    Err(_) => return Ok(()),
                },
            };

            match event {
                CoreEvent::ClientMessage(message) => {
                    match self.server.handle_message(message, writer)? {
                        LoopControl::Continue => {}
                        LoopControl::ExitSuccess => return Ok(()),
                        LoopControl::ExitFailure => {
                            return Err("lsp exit received before shutdown".to_owned());
                        }
                    }
                    self.drain_diagnostic_requests();
                    self.dispatch_due_diagnostics();
                }
                CoreEvent::DiagnosticsReady(result) => {
                    self.publish_diagnostic_result(*result, writer)?;
                }
                CoreEvent::ReaderClosed => return Ok(()),
                CoreEvent::ReaderError(err) => return Err(err),
            }
        }
    }

    fn next_diagnostic_timeout(&self) -> Option<Duration> {
        let due_at = self
            .pending_diagnostics
            .values()
            .map(|scheduled| scheduled.due_at)
            .min()?;
        Some(due_at.saturating_duration_since(Instant::now()))
    }

    fn drain_diagnostic_requests(&mut self) {
        for DiagnosticScheduleRequest { entries, delay } in self.server.take_diagnostic_requests() {
            self.schedule_diagnostic_entries(entries, delay);
        }
    }

    fn schedule_diagnostic_entries(&mut self, entries: Vec<PathBuf>, delay: DiagnosticDelay) {
        let due_at = Instant::now() + delay.duration();
        for entry in entries {
            let entry = normalize_path(&entry);
            let generation = self
                .diagnostic_generations
                .entry(entry.clone())
                .and_modify(|generation| *generation += 1)
                .or_insert(1);
            self.pending_diagnostics.insert(
                entry,
                ScheduledDiagnostic {
                    generation: *generation,
                    due_at,
                    delay,
                },
            );
        }
    }

    fn dispatch_due_diagnostics(&mut self) {
        let now = Instant::now();
        let due_entries = self
            .pending_diagnostics
            .iter()
            .filter(|&(_entry, scheduled)| scheduled.due_at <= now)
            .map(|(entry, scheduled)| (entry.clone(), scheduled.generation))
            .collect::<Vec<_>>();
        for (entry, generation) in due_entries {
            let Some(scheduled) = self.pending_diagnostics.remove(&entry) else {
                continue;
            };
            let job = self.server.diagnostic_job_for_entry(&entry, generation);
            let tx = match scheduled.delay {
                DiagnosticDelay::Immediate => &self.immediate_diagnostic_tx,
                DiagnosticDelay::Debounced => &self.background_diagnostic_tx,
            };
            if tx.send(job).is_err() {
                break;
            }
        }
    }

    fn publish_diagnostic_result(
        &mut self,
        result: DiagnosticJobResult,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let current_generation = self
            .diagnostic_generations
            .get(&result.entry_path)
            .copied()
            .unwrap_or_default();
        if current_generation != result.generation {
            return Ok(());
        }
        if self.server.session.document(&result.entry_path).is_none() {
            return Ok(());
        }
        self.server.publish_diagnostic_result(result, writer)
    }
}

#[derive(Default)]
struct LspServer {
    session: AnalysisSession,
    analysis_options: AnalysisOptions,
    shutdown_requested: bool,
    defer_diagnostics: bool,
    diagnostic_requests: Vec<DiagnosticScheduleRequest>,
    completion_snippets: bool,
    semantic_tokens_refresh: bool,
    next_server_request_id: u64,
    document_uris: HashMap<PathBuf, String>,
    published_by_entry: HashMap<PathBuf, HashSet<String>>,
    parse_cache: HashMap<PathBuf, CachedParsedDocument>,
    completion_index_cache: HashMap<PathBuf, CachedCompletionIndex>,
    semantic_token_cache: HashMap<PathBuf, CachedSemanticTokens>,
    dependency_fingerprint_cache: DependencyFingerprintCache,
}

/// Transport-neutral Onda LSP session.
///
/// Native `onda lsp` feeds this session from stdio. Browser hosts feed the
/// same JSON-RPC messages from a Web Worker and receive every response or
/// notification produced while handling that message.
#[derive(Default)]
pub struct LspSession {
    server: LspServer,
}

impl LspSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_analysis_options(&mut self, options: AnalysisOptions) {
        self.server.analysis_options = options;
        self.server.note_watched_files_changed(&[]);
    }

    pub fn handle_message(&mut self, message: Value) -> Result<Vec<Value>, String> {
        let mut output = Vec::new();
        self.server.handle_message(message, &mut output)?;
        decode_lsp_messages(&output)
    }

    pub fn handle_message_json(&mut self, message: &str) -> Result<String, String> {
        let message = serde_json::from_str(message)
            .map_err(|error| format!("invalid lsp json message: {error}"))?;
        serde_json::to_string(&self.handle_message(message)?)
            .map_err(|error| format!("failed to encode lsp responses: {error}"))
    }
}

struct CachedParsedDocument {
    fingerprint: DocumentFingerprint,
    sources: SourceManifest,
    parsed: Option<Arc<Program>>,
}

struct CachedSemanticTokens {
    fingerprint: DocumentFingerprint,
    encoded: Vec<u32>,
}

struct CachedCompletionIndex {
    fingerprint: DocumentFingerprint,
    snapshot: CompletionIndexSnapshot,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct DocumentFingerprint {
    source_hash: u64,
    dependency_hash: u64,
}

#[derive(Default)]
struct DependencyFingerprintCache {
    disk_files: HashMap<PathBuf, CachedDependencyFile>,
}

#[derive(Debug, Clone)]
struct CachedDependencyFile {
    stamp: DependencyFileStamp,
    source_hash: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DependencyFileStamp {
    len: u64,
    modified: Option<SystemTime>,
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
                self.semantic_tokens_refresh =
                    client_supports_semantic_tokens_refresh(params.capabilities.as_ref());
                let result = initialize_result(params.process_id);
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "initialized" | "$/cancelRequest" | "workspace/didChangeConfiguration" => {}
            "workspace/didChangeWatchedFiles" => {
                let params = parse_params::<DidChangeWatchedFilesParams>(envelope.params)?;
                let affected = self.note_watched_files_changed(&params.changes);
                self.publish_or_schedule_diagnostics_for_entries(
                    affected,
                    DiagnosticDelay::Immediate,
                    writer,
                )?;
            }
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
                let affected = self.note_document_changed(&normalized);
                self.document_uris
                    .insert(normalized.clone(), path_to_file_uri(&normalized));
                self.publish_diagnostics_for_entries(affected, writer)?;
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
                let affected = self.note_document_changed(&normalized);
                self.document_uris
                    .insert(normalized.clone(), path_to_file_uri(&normalized));
                self.publish_or_schedule_diagnostics_for_entries(
                    affected,
                    DiagnosticDelay::Debounced,
                    writer,
                )?;
            }
            "textDocument/didSave" => {
                let params = parse_params::<DidSaveTextDocumentParams>(envelope.params)?;
                let Some(path) =
                    lsp_document_path(&params.text_document.uri).map_err(invalid_params)?
                else {
                    return Ok(LoopControl::Continue);
                };
                let affected = if let Some(text) = params.text {
                    let version = self
                        .session
                        .document(&path)
                        .map(|doc| doc.version)
                        .unwrap_or(DocumentVersion(0));
                    let normalized = self.session.update_document(&path, version, text);
                    let affected = self.note_document_changed(&normalized);
                    self.document_uris
                        .insert(normalized.clone(), path_to_file_uri(&normalized));
                    affected
                } else {
                    self.note_document_changed(&path)
                };
                self.publish_or_schedule_diagnostics_for_entries(
                    affected,
                    DiagnosticDelay::Immediate,
                    writer,
                )?;
            }
            "textDocument/didClose" => {
                let params = parse_params::<DidCloseTextDocumentParams>(envelope.params)?;
                let Some(path) =
                    lsp_document_path(&params.text_document.uri).map_err(invalid_params)?
                else {
                    return Ok(LoopControl::Continue);
                };
                self.session.close_document(&path);
                let normalized = normalize_path(&path);
                let affected = self.note_document_closed(&normalized);
                self.document_uris.remove(&normalized);
                self.clear_entry_diagnostics(&normalized, writer)?;
                self.publish_or_schedule_diagnostics_for_entries(
                    affected,
                    DiagnosticDelay::Immediate,
                    writer,
                )?;
            }
            "textDocument/semanticTokens/full" => {
                let params = parse_params::<SemanticTokensParams>(envelope.params)?;
                let result = self.semantic_tokens_for_uri(&params.text_document.uri)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/completion" => {
                let params = parse_params::<CompletionParams>(envelope.params)?;
                let result = self.completions_for_uri(
                    &params.text_document.uri,
                    params.position,
                    params.context.as_ref(),
                )?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/hover" => {
                let params = parse_params::<HoverParams>(envelope.params)?;
                let result = self.hover_for_uri(&params.text_document.uri, params.position)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/signatureHelp" => {
                let params = parse_params::<SignatureHelpParams>(envelope.params)?;
                let result =
                    self.signature_help_for_uri(&params.text_document.uri, params.position)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "textDocument/definition" => {
                let params = parse_params::<DefinitionParams>(envelope.params)?;
                let result = self.definition_for_uri(&params.text_document.uri, params.position)?;
                write_result(writer, envelope.id.unwrap_or(Value::Null), result)?;
            }
            "onda/virtualDocument" => {
                let params = parse_params::<VirtualDocumentParams>(envelope.params)?;
                let result = stdlib_virtual_document(&params.uri).unwrap_or(Value::Null);
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

    fn semantic_tokens_for_uri(&mut self, uri: &str) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(json!({ "data": [] }));
        };
        let source = self.source_text_for_path(&path)?;
        let normalized = normalize_path(&path);
        if self.session.document(&normalized).is_some() {
            return self.semantic_tokens_for_open_document(&normalized, &source);
        }

        let overlays = self.session.overlay_map();
        let fingerprint = self.document_fingerprint_for_path(&normalized, &source, &overlays);
        if let Some(cached) = self.semantic_token_cache.get(&normalized) {
            if cached.fingerprint == fingerprint {
                return Ok(json!({
                    "data": cached.encoded.clone(),
                }));
            }
        }

        let parsed = self.parsed_program_for_path(&normalized, &overlays, fingerprint);
        let tokens =
            semantic_tokens_for_document_with_parsed(&source, Some(&normalized), parsed.as_deref());
        let encoded = encode_semantic_tokens(&tokens);
        self.semantic_token_cache.insert(
            normalized,
            CachedSemanticTokens {
                fingerprint,
                encoded: encoded.clone(),
            },
        );
        Ok(json!({
            "data": encoded,
        }))
    }

    fn semantic_tokens_for_open_document(
        &mut self,
        normalized: &Path,
        source: &str,
    ) -> Result<Value, String> {
        let source_hash = hash_source(source);
        let fingerprint = DocumentFingerprint {
            source_hash,
            dependency_hash: 0,
        };
        if let Some(cached) = self.semantic_token_cache.get(normalized) {
            if cached.fingerprint == fingerprint {
                return Ok(json!({
                    "data": cached.encoded.clone(),
                }));
            }
        }

        let parsed = self.parsed_program_for_open_request(normalized, source);
        let tokens = if let Some(parsed) = parsed.as_deref() {
            semantic_tokens_for_document_with_parsed(source, Some(normalized), Some(parsed))
        } else {
            semantic_tokens_for_document_source_only(source, Some(normalized))
        };
        let encoded = encode_semantic_tokens(&tokens);
        self.semantic_token_cache.insert(
            normalized.to_path_buf(),
            CachedSemanticTokens {
                fingerprint,
                encoded: encoded.clone(),
            },
        );
        Ok(json!({
            "data": encoded,
        }))
    }

    fn completions_for_uri(
        &mut self,
        uri: &str,
        position: Position,
        context: Option<&CompletionRequestContext>,
    ) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(json!({
                "isIncomplete": false,
                "items": [],
            }));
        };
        let source = self.source_text_for_path(&path)?;
        if colon_trigger_is_single_colon(&source, position, context) {
            return Ok(json!({
                "isIncomplete": true,
                "items": [],
            }));
        }
        let completion_position = CompletionPosition {
            line: position.line,
            character: position.character,
        };
        let overlays = self.session.overlay_map();
        let snippets = self.completion_snippets;
        let normalized = normalize_path(&path);
        let parsed = if self.session.document(&normalized).is_some() {
            self.parsed_program_for_open_request(&normalized, &source)
        } else {
            let fingerprint = self.document_fingerprint_for_path(&normalized, &source, &overlays);
            self.parsed_program_for_path(&normalized, &overlays, fingerprint)
        };
        let index_snapshot = parsed
            .as_deref()
            .and_then(|program| self.completion_index_snapshot_for_path(&normalized, program));
        let completion = completion_items_for_document_with_index(
            &source,
            Some(&normalized),
            &overlays,
            parsed.as_deref(),
            index_snapshot,
            completion_position,
            snippets,
        );
        Ok(json!({
            "isIncomplete": completion.is_incomplete,
            "items": completion.items,
        }))
    }

    fn hover_for_uri(&mut self, uri: &str, position: Position) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(Value::Null);
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.overlay_map();
        let normalized = normalize_path(&path);
        let parsed = if self.session.document(&normalized).is_some() {
            self.parsed_program_for_open_navigation_request(&normalized, &source)
        } else {
            let fingerprint = self.document_fingerprint_for_path(&normalized, &source, &overlays);
            self.parsed_program_for_path(&normalized, &overlays, fingerprint)
        };
        Ok(hover_for_document_with_parsed(
            &source,
            Some(&normalized),
            &overlays,
            parsed.as_deref(),
            NavigationPosition {
                line: position.line,
                character: position.character,
            },
        )
        .unwrap_or(Value::Null))
    }

    fn definition_for_uri(&mut self, uri: &str, position: Position) -> Result<Value, String> {
        if let Some((module, path, source)) = stdlib_virtual_source(uri) {
            let overlays = self.session.overlay_map();
            let parsed = parse_stdlib_module(module).ok();
            return Ok(definition_for_document_with_parsed(
                source,
                Some(&path),
                &overlays,
                parsed.as_ref(),
                NavigationPosition {
                    line: position.line,
                    character: position.character,
                },
            )
            .unwrap_or(Value::Null));
        }
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(Value::Null);
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.overlay_map();
        let normalized = normalize_path(&path);
        let parsed = if self.session.document(&normalized).is_some() {
            self.parsed_program_for_open_navigation_request(&normalized, &source)
        } else {
            let fingerprint = self.document_fingerprint_for_path(&normalized, &source, &overlays);
            self.parsed_program_for_path(&normalized, &overlays, fingerprint)
        };
        Ok(definition_for_document_with_parsed(
            &source,
            Some(&normalized),
            &overlays,
            parsed.as_deref(),
            NavigationPosition {
                line: position.line,
                character: position.character,
            },
        )
        .unwrap_or(Value::Null))
    }

    fn signature_help_for_uri(&mut self, uri: &str, position: Position) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(Value::Null);
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.overlay_map();
        let normalized = normalize_path(&path);
        let parsed = if self.session.document(&normalized).is_some() {
            self.parsed_program_for_open_navigation_request(&normalized, &source)
        } else {
            let fingerprint = self.document_fingerprint_for_path(&normalized, &source, &overlays);
            self.parsed_program_for_path(&normalized, &overlays, fingerprint)
        };
        Ok(signature_help_for_document_with_parsed(
            &source,
            Some(&normalized),
            &overlays,
            parsed.as_deref(),
            NavigationPosition {
                line: position.line,
                character: position.character,
            },
        )
        .unwrap_or(Value::Null))
    }

    fn document_symbols_for_uri(&mut self, uri: &str) -> Result<Value, String> {
        let Some(path) = lsp_document_path(uri).map_err(invalid_params)? else {
            return Ok(json!([]));
        };
        let source = self.source_text_for_path(&path)?;
        let overlays = self.session.overlay_map();
        let normalized = normalize_path(&path);
        let parsed = if self.session.document(&normalized).is_some() {
            self.parsed_program_for_open_navigation_request(&normalized, &source)
        } else {
            let fingerprint = self.document_fingerprint_for_path(&normalized, &source, &overlays);
            self.parsed_program_for_path(&normalized, &overlays, fingerprint)
        };
        Ok(json!(document_symbols_for_document_with_parsed(
            &source,
            Some(&normalized),
            &overlays,
            parsed.as_deref(),
        )))
    }

    fn source_text_for_path(&self, path: &Path) -> Result<String, String> {
        if let Some(document) = self.session.document(path) {
            return Ok(document.text.clone());
        }
        fs::read_to_string(path)
            .map_err(|err| format!("failed to read source '{}': {err}", path.display()))
    }

    fn note_document_changed(&mut self, path: &Path) -> Vec<PathBuf> {
        let normalized = normalize_path(path);
        let affected = self.diagnostic_entries_affected_by_change(&normalized);
        self.dependency_fingerprint_cache.remove(&normalized);
        self.invalidate_analysis_entries(
            affected
                .iter()
                .map(PathBuf::as_path)
                .chain(std::iter::once(normalized.as_path())),
        );
        affected
    }

    fn note_document_closed(&mut self, path: &Path) -> Vec<PathBuf> {
        let affected = self.note_document_changed(path);
        let normalized = normalize_path(path);
        self.parse_cache.remove(&normalized);
        self.completion_index_cache.remove(&normalized);
        self.semantic_token_cache.remove(&normalized);
        affected
    }

    fn note_watched_files_changed(&mut self, changes: &[FileEvent]) -> Vec<PathBuf> {
        if changes.is_empty() {
            let affected = self.open_document_paths();
            self.clear_analysis_caches();
            return affected;
        }
        let mut changed_paths = Vec::with_capacity(changes.len());
        for change in changes {
            match lsp_document_path(&change.uri) {
                Ok(Some(path)) => changed_paths.push(normalize_path(&path)),
                _ => {
                    let affected = self.open_document_paths();
                    self.clear_analysis_caches();
                    return affected;
                }
            }
        }
        changed_paths.sort();
        changed_paths.dedup();

        let affected = self.diagnostic_entries_affected_by_changes(&changed_paths);
        for changed in &changed_paths {
            self.dependency_fingerprint_cache.remove(changed);
        }
        self.invalidate_analysis_entries(
            affected
                .iter()
                .map(PathBuf::as_path)
                .chain(changed_paths.iter().map(PathBuf::as_path)),
        );
        affected
    }

    fn open_document_paths(&self) -> Vec<PathBuf> {
        self.session.open_documents().keys().cloned().collect()
    }

    fn clear_analysis_caches(&mut self) {
        self.semantic_token_cache.clear();
        self.dependency_fingerprint_cache.clear();
        self.parse_cache.clear();
        self.completion_index_cache.clear();
    }

    fn invalidate_analysis_entries<'a>(&mut self, paths: impl IntoIterator<Item = &'a Path>) {
        for path in paths {
            let normalized = normalize_path(path);
            self.semantic_token_cache.remove(&normalized);
            self.parse_cache.remove(&normalized);
            self.completion_index_cache.remove(&normalized);
        }
    }

    fn parsed_program_for_path(
        &mut self,
        path: &Path,
        overlays: &HashMap<PathBuf, String>,
        fingerprint: DocumentFingerprint,
    ) -> Option<Arc<Program>> {
        let normalized = normalize_path(path);
        if let Some(parsed) = self
            .parse_cache
            .get(&normalized)
            .filter(|cached| cached.fingerprint == fingerprint)
            .map(|cached| cached.parsed.clone())
        {
            return parsed;
        }

        let loaded = load_program_file_with_overlays(&normalized, overlays);
        let (parsed, sources) = match loaded {
            Ok(loaded) => (Some(Arc::new(loaded.program)), loaded.sources),
            Err(error) => (None, error.sources),
        };
        self.parse_cache.insert(
            normalized,
            CachedParsedDocument {
                fingerprint,
                sources,
                parsed: parsed.clone(),
            },
        );
        parsed
    }

    fn parsed_program_for_open_request(
        &mut self,
        path: &Path,
        source: &str,
    ) -> Option<Arc<Program>> {
        let normalized = normalize_path(path);
        let source_hash = hash_source(source);
        if let Some(parsed) = self
            .parse_cache
            .get(&normalized)
            .filter(|cached| cached.fingerprint.source_hash == source_hash)
            .and_then(|cached| cached.parsed.clone())
        {
            return Some(parsed);
        }
        let overlays = self.session.overlay_map();
        let loaded = load_program_file_with_overlays(&normalized, &overlays);
        let (parsed, sources) = match loaded {
            Ok(loaded) => (Some(Arc::new(loaded.program)), loaded.sources),
            Err(error) => (None, error.sources),
        };
        self.parse_cache.insert(
            normalized,
            CachedParsedDocument {
                fingerprint: DocumentFingerprint {
                    source_hash,
                    dependency_hash: 0,
                },
                sources,
                parsed: parsed.clone(),
            },
        );
        parsed
    }

    fn parsed_program_for_open_navigation_request(
        &mut self,
        path: &Path,
        source: &str,
    ) -> Option<Arc<Program>> {
        let normalized = normalize_path(path);
        let overlays = self.session.overlay_map();
        let fingerprint = self.document_fingerprint_for_path(&normalized, source, &overlays);
        self.parse_cache
            .get(&normalized)
            .filter(|cached| cached.fingerprint == fingerprint)
            .and_then(|cached| cached.parsed.clone())
    }

    fn document_fingerprint_for_path(
        &mut self,
        path: &Path,
        source: &str,
        overlays: &HashMap<PathBuf, String>,
    ) -> DocumentFingerprint {
        let normalized = normalize_path(path);
        if let Some(cached) = self.parse_cache.get(&normalized) {
            let fingerprint = self
                .dependency_fingerprint_cache
                .document_fingerprint_from_manifest(&normalized, source, overlays, &cached.sources);
            if fingerprint == cached.fingerprint {
                return fingerprint;
            }
        }
        let previous_sources = self
            .parse_cache
            .get(&normalized)
            .map(|cached| cached.sources.clone())
            .unwrap_or_default();
        let loaded = load_program_file_with_overlays(&normalized, overlays);
        let (parsed, mut sources, succeeded) = match loaded {
            Ok(loaded) => (Some(Arc::new(loaded.program)), loaded.sources, true),
            Err(error) => (None, error.sources, false),
        };
        if !succeeded {
            for path in previous_sources.files {
                if !sources.files.contains(&path) && !sources.unresolved_files.contains(&path) {
                    sources.files.push(path);
                }
            }
            for path in previous_sources.unresolved_files {
                if !sources.files.contains(&path) && !sources.unresolved_files.contains(&path) {
                    sources.unresolved_files.push(path);
                }
            }
        }
        let fingerprint = self
            .dependency_fingerprint_cache
            .document_fingerprint_from_manifest(&normalized, source, overlays, &sources);
        self.parse_cache.insert(
            normalized,
            CachedParsedDocument {
                fingerprint,
                sources,
                parsed,
            },
        );
        fingerprint
    }

    #[cfg(test)]
    fn cache_parsed_program_for_path(
        &mut self,
        path: &Path,
        fingerprint: DocumentFingerprint,
        parsed: Option<Program>,
    ) {
        if let Some(parsed) = parsed {
            self.store_parsed_program_for_path(path, fingerprint, Some(Arc::new(parsed)));
        }
    }

    fn cache_parsed_program_with_sources(
        &mut self,
        path: &Path,
        fingerprint: DocumentFingerprint,
        sources: SourceManifest,
        parsed: Option<Program>,
    ) {
        let normalized = normalize_path(path);
        self.parse_cache.insert(
            normalized.clone(),
            CachedParsedDocument {
                fingerprint,
                sources,
                parsed: parsed.map(Arc::new),
            },
        );
        self.completion_index_cache.remove(&normalized);
        self.semantic_token_cache.remove(&normalized);
    }

    #[cfg(test)]
    fn store_parsed_program_for_path(
        &mut self,
        path: &Path,
        fingerprint: DocumentFingerprint,
        parsed: Option<Arc<Program>>,
    ) {
        let normalized = normalize_path(path);
        let sources = self
            .parse_cache
            .get(&normalized)
            .map(|cached| cached.sources.clone())
            .unwrap_or_default();
        self.parse_cache.insert(
            normalized.clone(),
            CachedParsedDocument {
                fingerprint,
                sources,
                parsed,
            },
        );
        self.completion_index_cache.remove(&normalized);
        self.semantic_token_cache.remove(&normalized);
    }

    fn completion_index_snapshot_for_path(
        &mut self,
        path: &Path,
        program: &Program,
    ) -> Option<&CompletionIndexSnapshot> {
        let normalized = normalize_path(path);
        let fingerprint = self.parse_cache.get(&normalized)?.fingerprint;
        let stale = self
            .completion_index_cache
            .get(&normalized)
            .map(|cached| cached.fingerprint != fingerprint)
            .unwrap_or(true);
        if stale {
            self.completion_index_cache.insert(
                normalized.clone(),
                CachedCompletionIndex {
                    fingerprint,
                    snapshot: CompletionIndexSnapshot::build(program, Some(&normalized)),
                },
            );
        }
        self.completion_index_cache
            .get(&normalized)
            .map(|cached| &cached.snapshot)
    }

    fn publish_or_schedule_diagnostics_for_entries(
        &mut self,
        entries: Vec<PathBuf>,
        delay: DiagnosticDelay,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        if self.defer_diagnostics {
            self.diagnostic_requests
                .push(DiagnosticScheduleRequest { entries, delay });
            return Ok(());
        }
        for entry_path in entries {
            self.publish_diagnostics_for_entry(&entry_path, writer)?;
        }
        Ok(())
    }

    fn take_diagnostic_requests(&mut self) -> Vec<DiagnosticScheduleRequest> {
        std::mem::take(&mut self.diagnostic_requests)
    }

    fn diagnostic_job_for_entry(&mut self, entry_path: &Path, generation: u64) -> DiagnosticJob {
        let entry_path = normalize_path(entry_path);
        let open_documents = self
            .session
            .open_documents()
            .iter()
            .map(|(path, document)| DiagnosticOpenDocument {
                path: path.clone(),
                version: document.version,
                text: document.text.clone(),
            })
            .collect();
        DiagnosticJob {
            entry_path,
            generation,
            open_documents,
        }
    }

    fn publish_diagnostic_result(
        &mut self,
        result: DiagnosticJobResult,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let entry_path = result.entry_path;
        let should_refresh_semantic_tokens = self.semantic_tokens_refresh && result.parse_succeeded;
        if let Some(fingerprint) = result.parse_fingerprint {
            // Span file and trace IDs belong to the thread that parsed them.
            // Replay the worker's exact source graph here before caching its AST.
            let parsed = result
                .parse_succeeded
                .then(|| replay_parsed_program(&entry_path, &result.sources))
                .flatten();
            self.cache_parsed_program_with_sources(
                &entry_path,
                fingerprint,
                result.sources,
                parsed,
            );
            if let Some(snapshot) = result.completion_index_snapshot {
                self.completion_index_cache.insert(
                    normalize_path(&entry_path),
                    CachedCompletionIndex {
                        fingerprint,
                        snapshot,
                    },
                );
            }
        }
        let default_uri = self
            .document_uris
            .get(&entry_path)
            .cloned()
            .unwrap_or_else(|| path_to_file_uri(&entry_path));
        self.publish_diagnostics_for_entry_data(
            &entry_path,
            &default_uri,
            result.diagnostics.iter(),
            writer,
        )?;
        if should_refresh_semantic_tokens {
            self.request_semantic_tokens_refresh(writer)?;
        }
        Ok(())
    }

    fn publish_diagnostics_for_entry(
        &mut self,
        entry_path: &Path,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let entry_path = normalize_path(entry_path);

        let source = self.source_text_for_path(&entry_path).ok();
        let overlays = self.session.overlay_map();
        let snapshot = self
            .session
            .analyze_document(&entry_path, self.analysis_options);
        let parse_fingerprint = source.as_deref().map(|source| {
            self.dependency_fingerprint_cache
                .document_fingerprint_from_manifest(
                    &entry_path,
                    source,
                    &overlays,
                    &snapshot.sources,
                )
        });
        let should_refresh_semantic_tokens =
            self.semantic_tokens_refresh && snapshot.parsed.is_some();
        if let Some(fingerprint) = parse_fingerprint {
            self.cache_parsed_program_with_sources(
                &entry_path,
                fingerprint,
                snapshot.sources.clone(),
                snapshot.parsed.clone(),
            );
        }
        let default_uri = self
            .document_uris
            .get(&entry_path)
            .cloned()
            .unwrap_or_else(|| path_to_file_uri(&entry_path));
        self.publish_diagnostics_for_entry_data(
            &entry_path,
            &default_uri,
            snapshot.diagnostics.iter(),
            writer,
        )?;
        if should_refresh_semantic_tokens {
            self.request_semantic_tokens_refresh(writer)?;
        }
        Ok(())
    }

    fn publish_diagnostics_for_entry_data<'a>(
        &mut self,
        entry_path: &Path,
        default_uri: &str,
        diagnostics: impl Iterator<Item = &'a Diagnostic>,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();

        for diagnostic in diagnostics {
            if !diagnostic.editor_visible {
                continue;
            }
            let uri = diagnostic_uri(diagnostic, default_uri)?;
            grouped
                .entry(uri)
                .or_default()
                .push(diagnostic_to_lsp(diagnostic));
        }

        let previous = self
            .published_by_entry
            .remove(entry_path)
            .unwrap_or_default();
        let (notifications, current) =
            diagnostic_publish_notifications(grouped, default_uri.to_owned(), previous);
        for params in notifications {
            write_notification(writer, "textDocument/publishDiagnostics", params)?;
        }
        self.published_by_entry
            .insert(normalize_path(entry_path), current);
        Ok(())
    }

    fn publish_diagnostics_for_entries(
        &mut self,
        entries: Vec<PathBuf>,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        for entry_path in entries {
            self.publish_diagnostics_for_entry(&entry_path, writer)?;
        }
        Ok(())
    }

    fn diagnostic_entries_affected_by_change(&self, changed_path: &Path) -> Vec<PathBuf> {
        self.diagnostic_entries_affected_by_changes(&[normalize_path(changed_path)])
    }

    fn diagnostic_entries_affected_by_changes(&self, changed_paths: &[PathBuf]) -> Vec<PathBuf> {
        let changed_paths = changed_paths
            .iter()
            .map(|path| normalize_path(path))
            .collect::<HashSet<_>>();
        let mut out = Vec::<PathBuf>::new();
        let mut seen = HashSet::<PathBuf>::new();

        for entry_path in self.session.open_documents().keys() {
            let entry_path = normalize_path(entry_path);
            let previous_sources = self
                .parse_cache
                .get(&entry_path)
                .map(|cached| &cached.sources);
            let affected = changed_paths.contains(&entry_path)
                || previous_sources.is_none_or(|sources| {
                    changed_paths
                        .iter()
                        .any(|changed_path| source_manifest_contains_path(sources, changed_path))
                });
            if affected && seen.insert(entry_path.clone()) {
                out.push(entry_path);
            }
        }

        out
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

    fn request_semantic_tokens_refresh(&mut self, writer: &mut impl Write) -> Result<(), String> {
        self.next_server_request_id = self.next_server_request_id.saturating_add(1);
        write_request(
            writer,
            json!(format!(
                "onda-semantic-tokens-refresh-{}",
                self.next_server_request_id
            )),
            "workspace/semanticTokens/refresh",
        )
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
#[serde(rename_all = "camelCase")]
struct DidChangeWatchedFilesParams {
    changes: Vec<FileEvent>,
}

#[derive(Debug, Deserialize)]
struct FileEvent {
    uri: String,
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
    #[serde(default)]
    context: Option<CompletionRequestContext>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionRequestContext {
    #[serde(default)]
    trigger_character: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HoverParams {
    text_document: TextDocumentIdentifier,
    position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureHelpParams {
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
struct VirtualDocumentParams {
    uri: String,
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

fn colon_trigger_is_single_colon(
    source: &str,
    position: Position,
    context: Option<&CompletionRequestContext>,
) -> bool {
    if context.and_then(|ctx| ctx.trigger_character.as_deref()) != Some(":") {
        return false;
    }
    let offset = position::byte_offset_for_lsp_position(
        source,
        position::LspPosition::new(position.line, position.character),
    );
    !source[..offset.min(source.len())].ends_with("::")
}

fn diagnostic_publish_notifications(
    grouped: HashMap<String, Vec<Value>>,
    default_uri: String,
    previous: HashSet<String>,
) -> (Vec<Value>, HashSet<String>) {
    let mut notifications = Vec::new();
    let mut current = HashSet::new();

    for (uri, diagnostics) in grouped {
        notifications.push(json!({
            "uri": uri,
            "diagnostics": diagnostics,
        }));
        current.insert(uri);
    }

    if !current.contains(&default_uri) {
        notifications.push(json!({
            "uri": default_uri,
            "diagnostics": [],
        }));
        current.insert(default_uri);
    }

    for uri in previous.difference(&current) {
        notifications.push(json!({
            "uri": uri,
            "diagnostics": [],
        }));
    }

    (notifications, current)
}

fn run_diagnostic_job(job: DiagnosticJob) -> DiagnosticJobResult {
    let mut session = AnalysisSession::new();
    for document in job.open_documents {
        session.open_document(document.path, document.version, document.text);
    }
    let overlays = session.overlay_map();
    let source = overlays
        .get(&job.entry_path)
        .cloned()
        .or_else(|| fs::read_to_string(&job.entry_path).ok());
    let snapshot = session.analyze_document(&job.entry_path, AnalysisOptions::default());
    let parse_fingerprint = source.as_deref().map(|source| {
        DependencyFingerprintCache::default().document_fingerprint_from_manifest(
            &job.entry_path,
            source,
            &overlays,
            &snapshot.sources,
        )
    });
    let completion_index_snapshot = snapshot
        .parsed
        .as_ref()
        .map(|program| CompletionIndexSnapshot::build(program, Some(&job.entry_path)));
    DiagnosticJobResult {
        entry_path: job.entry_path,
        generation: job.generation,
        diagnostics: snapshot.diagnostics,
        sources: snapshot.sources,
        parse_succeeded: snapshot.parsed.is_some(),
        parse_fingerprint,
        completion_index_snapshot,
    }
}

fn replay_parsed_program(entry_path: &Path, sources: &SourceManifest) -> Option<Program> {
    let documents = sources
        .documents
        .iter()
        .map(|document| (document.path.clone(), document.contents.clone()))
        .collect::<HashMap<_, _>>();
    load_program_file_from_snapshot(entry_path, &documents, &sources.resolutions)
        .ok()
        .map(|loaded| loaded.program)
}

fn hash_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
fn source_fingerprint(source: &str) -> DocumentFingerprint {
    DocumentFingerprint {
        source_hash: hash_source(source),
        dependency_hash: 0,
    }
}

impl DependencyFingerprintCache {
    fn document_fingerprint_from_manifest(
        &mut self,
        path: &Path,
        source: &str,
        overlays: &HashMap<PathBuf, String>,
        manifest: &SourceManifest,
    ) -> DocumentFingerprint {
        let mut dependency_hasher = DefaultHasher::new();
        let entry = normalize_path(path);
        for (kind, dependencies) in [
            ("resolved", manifest.files.as_slice()),
            ("unresolved", manifest.unresolved_files.as_slice()),
        ] {
            kind.hash(&mut dependency_hasher);
            for dependency in dependencies {
                let dependency = dependency_cache_path(dependency);
                if dependency == entry {
                    continue;
                }
                dependency.hash(&mut dependency_hasher);
                if let Some(overlay_source) = overlay_source_for_path(&dependency, overlays) {
                    "overlay".hash(&mut dependency_hasher);
                    overlay_source.hash(&mut dependency_hasher);
                    continue;
                }
                match self.disk_file_summary(&dependency) {
                    Ok(summary) => {
                        "disk".hash(&mut dependency_hasher);
                        summary.source_hash.hash(&mut dependency_hasher);
                    }
                    Err(kind) => {
                        "missing".hash(&mut dependency_hasher);
                        kind.hash(&mut dependency_hasher);
                    }
                }
            }
        }
        DocumentFingerprint {
            source_hash: hash_source(source),
            dependency_hash: dependency_hasher.finish(),
        }
    }

    fn remove(&mut self, path: &Path) {
        self.disk_files.remove(&dependency_cache_path(path));
    }

    fn clear(&mut self) {
        self.disk_files.clear();
    }

    #[cfg(test)]
    fn source_depends_on_path(
        &mut self,
        path: &Path,
        _source: &str,
        target: &Path,
        overlays: &HashMap<PathBuf, String>,
        previous_sources: Option<&SourceManifest>,
    ) -> bool {
        let target = normalize_path(target);
        if let Some(previous_sources) = previous_sources {
            return source_manifest_contains_path(previous_sources, &target);
        }
        let loaded = load_program_file_with_overlays(path, overlays);
        let sources = match loaded {
            Ok(loaded) => loaded.sources,
            Err(error) => error.sources,
        };
        source_manifest_contains_path(&sources, &target)
    }

    fn disk_file_summary(&mut self, path: &Path) -> Result<CachedDependencyFile, io::ErrorKind> {
        onda_frontend::ensure_no_symlink_components(path).map_err(|error| error.kind())?;
        let normalized = dependency_cache_path(path);
        let metadata = match fs::metadata(&normalized) {
            Ok(metadata) => metadata,
            Err(err) => {
                self.disk_files.remove(&normalized);
                return Err(err.kind());
            }
        };
        let stamp = DependencyFileStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if let Some(cached) = self.disk_files.get(&normalized) {
            if cached.stamp == stamp {
                return Ok(cached.clone());
            }
        }

        let source = match fs::read_to_string(&normalized) {
            Ok(source) => source,
            Err(err) => {
                self.disk_files.remove(&normalized);
                return Err(err.kind());
            }
        };
        let summary = CachedDependencyFile {
            stamp,
            source_hash: hash_source(&source),
        };
        self.disk_files.insert(normalized, summary.clone());
        Ok(summary)
    }
}

fn dependency_cache_path(path: &Path) -> PathBuf {
    onda_frontend::absolute_lexical_path(path).unwrap_or_else(|_| path.to_path_buf())
}

fn source_manifest_contains_path(sources: &SourceManifest, target: &Path) -> bool {
    sources
        .files
        .iter()
        .chain(&sources.unresolved_files)
        .any(|source| normalize_path(source) == target)
}

fn overlay_source_for_path<'a>(
    path: &Path,
    overlays: &'a HashMap<PathBuf, String>,
) -> Option<&'a str> {
    overlays
        .get(path)
        .or_else(|| overlays.get(&normalize_path(path)))
        .map(String::as_str)
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
            "signatureHelpProvider": {
                "triggerCharacters": ["(", ","],
            },
            "definitionProvider": true,
            "documentSymbolProvider": true,
            "experimental": {
                "ondaVirtualDocuments": true,
            },
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

fn client_supports_semantic_tokens_refresh(capabilities: Option<&Value>) -> bool {
    capabilities
        .and_then(|value| {
            value
                .pointer("/workspace/semanticTokens/refreshSupport")
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

fn decode_lsp_messages(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let mut reader = BufReader::new(bytes);
    let mut messages = Vec::new();
    while let Some(message) = read_lsp_message(&mut reader)? {
        messages.push(message);
    }
    Ok(messages)
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

fn write_request(writer: &mut impl Write, id: Value, method: &str) -> Result<(), String> {
    write_lsp_message(
        writer,
        &json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "method": method,
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
    use super::diagnostics::{diagnostic_message, diagnostic_to_lsp};
    use super::path_utils::file_uri_to_path;
    use super::{
        initialize_result, latest_full_text, lsp_document_path, path_to_file_uri,
        DependencyFingerprintCache, DiagnosticDelay, DiagnosticJobResult,
        DiagnosticScheduleRequest, LspCore, LspServer, LspSession, Position,
        TextDocumentContentChangeEvent,
    };
    use onda_frontend::{DiagCode, Diagnostic, SourceManifest};
    use onda_semantics as onda_daemon;
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Mutex, MutexGuard};
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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                permissions.set_mode(permissions.mode() | 0o200);
            }
            #[cfg(not(unix))]
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
        completion_items_for_with_context(server, path, source, needle, None)
    }

    fn completion_items_for_with_context(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
        context: Option<serde_json::Value>,
    ) -> Vec<serde_json::Value> {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let position = position_after(source, needle);
        let mut params = json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": position.line,
                "character": position.character,
            }
        });
        if let Some(context) = context {
            params["context"] = context;
        }
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "textDocument/completion",
                    "params": params
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

    fn semantic_token_data_for(server: &mut LspServer, path: &Path) -> Vec<u32> {
        let uri = path_to_file_uri(path);
        server
            .semantic_tokens_for_uri(&uri)
            .expect("semantic tokens should succeed")["data"]
            .as_array()
            .expect("semantic token data")
            .iter()
            .map(|value| value.as_u64().expect("semantic token integer") as u32)
            .collect()
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
    fn parsed_document_cache_reparses_after_document_change() {
        let dir = mk_temp_dir("parse_cache_reparse");
        let main = dir.join("main.onda");
        let old_source = r#"
namespace Old:
  const X = 1
"#;
        let new_source = r#"
namespace New:
  const X = 1
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_symbols = document_symbols_for(&mut server, &main, old_source);
        let new_symbols = document_symbols_for(&mut server, &main, new_source);
        let old_names = old_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();
        let new_names = new_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();

        assert!(old_names.contains(&"Old"), "old symbols: {old_symbols:?}");
        assert!(new_names.contains(&"New"), "new symbols: {new_symbols:?}");
        assert!(
            !new_names.contains(&"Old"),
            "new symbols should not come from stale parse cache: {new_symbols:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parsed_open_document_cache_refreshes_importing_document_after_diagnostics() {
        let dir = mk_temp_dir("parse_cache_open_importing_reparse");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let old_source = r#"
include "lib.onda"

namespace Old:
  const X = 1
"#;
        let new_source = r#"
include "lib.onda"

namespace New:
  const X = 1
"#;
        write_file(&lib, "namespace Imported:\n  const X = 1\n");
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_symbols = document_symbols_for(&mut server, &main, old_source);
        let normalized = server.session.update_document(
            &main,
            onda_daemon::DocumentVersion(2),
            new_source.to_owned(),
        );
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("diagnostics should refresh parsed snapshot");
        let new_symbols = document_symbols_for(&mut server, &main, new_source);
        let old_names = old_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();
        let new_names = new_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();

        assert!(old_names.contains(&"Old"), "old symbols: {old_symbols:?}");
        assert!(new_names.contains(&"New"), "new symbols: {new_symbols:?}");
        assert!(
            !new_names.contains(&"Old"),
            "changed importing source should refresh after diagnostics: {new_symbols:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_reparses_after_valid_document_change() {
        let dir = mk_temp_dir("completion_cache_reparse");
        let main = dir.join("main.onda");
        let old_source = r#"
namespace Old:
  const X = 1

sample:
  out1 = O
"#;
        let new_source = r#"
namespace New:
  const X = 1

sample:
  out1 = N
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_labels = completion_labels_for(&mut server, &main, old_source, "O");
        let new_labels = completion_labels_for(&mut server, &main, new_source, "N");

        assert!(
            old_labels.contains(&"Old".to_owned()),
            "old completion should see old AST: {old_labels:?}"
        );
        assert!(
            new_labels.contains(&"New".to_owned()),
            "completion should reparse valid changed text: {new_labels:?}"
        );
        assert!(
            !new_labels.contains(&"Old".to_owned()),
            "completion should not use stale parse when changed text parses: {new_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unresolved_dependency_creation_invalidates_the_parse_cache() {
        let dir = mk_temp_dir("parse_cache_unresolved_dependency");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = "import lib\nsample:\n  out1 = target()\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let overlays = server.session.overlay_map();
        let missing_fingerprint = server.document_fingerprint_for_path(&main, source, &overlays);
        assert!(
            server
                .parse_cache
                .get(&super::normalize_path(&main))
                .is_some_and(|cached| cached.parsed.is_none()),
            "the unresolved import should initially fail to parse"
        );
        assert!(
            server
                .dependency_fingerprint_cache
                .source_depends_on_path(&main, source, &lib, &overlays, None,),
            "an unresolved candidate must still count as a dependency"
        );

        write_file(&lib, "def target():\n  return 0.0\n");
        let resolved_fingerprint = server.document_fingerprint_for_path(&main, source, &overlays);

        assert_ne!(
            resolved_fingerprint, missing_fingerprint,
            "creating an unresolved dependency must invalidate the cached fingerprint"
        );
        assert!(
            server
                .parse_cache
                .get(&super::normalize_path(&main))
                .is_some_and(|cached| cached.parsed.is_some()),
            "the importer should be reparsed after its dependency appears"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parsed_document_cache_refreshes_after_dependency_diagnostics() {
        let dir = mk_temp_dir("parse_cache_dependency_reparse");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = Lib::target()
"#;
        write_file(
            &lib,
            r#"
namespace Lib:
  def target():
    return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_definition = definition_for(&mut server, &main, source, "Lib::target");

        write_file(
            &lib,
            r#"
namespace Lib:
  const Spacer = 1

  def target():
    return 0.0
	"#,
        );
        server
            .publish_diagnostics_for_entry(&main, &mut Vec::new())
            .expect("diagnostics should refresh imported definition snapshot");
        let new_definition = definition_for(&mut server, &main, source, "Lib::target");

        assert_ne!(
            old_definition["range"]["start"]["line"],
            new_definition["range"]["start"]["line"],
            "definition should reflect changed imported file after diagnostics, old={old_definition:?}, new={new_definition:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_for_open_importing_document_parses_before_diagnostics() {
        let dir = mk_temp_dir("definition_open_importing_before_diagnostics");
        let main = dir.join("main.onda");
        let source = r#"
import std/delay

proc CyberCell<T>:
  ins<T>:
    src
    fb

  outs<T> 1

  params<T>:
    drive = 1.0
    bias = 0.0

  sample 32:
    x = (src + fb + bias) * drive
    out1 = x

init:
  smear = std::delay::Delay<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        for (needle, expected_line) in [("src", 5), ("fb", 6), ("bias", 12), ("drive", 11)] {
            let definition = definition_for(&mut server, &main, source, needle);
            assert_ne!(
                definition,
                json!(null),
                "{needle} should resolve before diagnostics populate the parse cache"
            );
            assert_eq!(
                definition["range"]["start"]["line"],
                json!(expected_line),
                "{needle} should resolve to its proc-local declaration: {definition:?}"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_for_open_importing_document_resolves_stdlib_proc_before_diagnostics() {
        let dir = mk_temp_dir("definition_open_importing_stdlib_before_diagnostics");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/delay

init:
  smear = std::delay::Delay<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Delay");

        assert_ne!(
            definition,
            json!(null),
            "stdlib proc should resolve before diagnostics populate the parse cache"
        );
        let uri = definition["uri"].as_str().expect("stdlib proc uri");
        let path = file_uri_to_path(uri).expect("stdlib proc should be a file uri");
        assert!(
            path.starts_with(&cache),
            "stdlib proc should materialize inside cache: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("delay.onda")
        );
        let line = definition["range"]["start"]["line"]
            .as_u64()
            .expect("proc line") as usize;
        let materialized = fs::read_to_string(&path).expect("read materialized stdlib");
        let target_line = materialized
            .lines()
            .nth(line)
            .expect("definition line should exist");
        assert!(
            target_line.contains("proc Delay"),
            "definition should target std::delay::Delay, got line: {target_line}"
        );

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parsed_document_cache_tracks_on_import_dependencies_after_diagnostics() {
        let dir = mk_temp_dir("parse_cache_on_dependency_reparse");
        let lib = dir.join("lib.on");
        let main = dir.join("main.onda");
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = Lib::target()
"#;
        write_file(
            &lib,
            r#"
namespace Lib:
  def target():
    return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_definition = definition_for(&mut server, &main, source, "Lib::target");

        write_file(
            &lib,
            r#"
namespace Lib:
  const Spacer = 1

  def target():
    return 0.0
	"#,
        );
        server
            .publish_diagnostics_for_entry(&main, &mut Vec::new())
            .expect("diagnostics should refresh .on imported definition snapshot");
        let new_definition = definition_for(&mut server, &main, source, "Lib::target");

        assert_ne!(
            old_definition["range"]["start"]["line"],
            new_definition["range"]["start"]["line"],
            "definition should reflect changed .on imported file after diagnostics, old={old_definition:?}, new={new_definition:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_token_cache_reparses_after_dependency_change() {
        let dir = mk_temp_dir("semantic_token_cache_dependency_reparse");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
import lib

sample:
  out1 = target()
"#;
        write_file(
            &lib,
            r#"
def target():
  return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_data = semantic_token_data_for(&mut server, &main);

        write_file(
            &lib,
            r#"
const target = 0.0
"#,
        );
        let new_data = semantic_token_data_for(&mut server, &main);

        assert_ne!(
            old_data, new_data,
            "semantic token cache should account for imported file changes"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostics_refresh_requests_semantic_tokens_after_parsed_importer_update() {
        let dir = mk_temp_dir("semantic_token_cache_diagnostic_refresh");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = r#"
import lib
use sc

sample:
  out1 = 0.0
"#;
        let old_lib_source = r#"
def sc():
  return 0.0
"#;
        let new_lib_source = r#"
namespace sc:
  namespace SinOsc:
    const A = 1
"#;
        write_file(&lib, old_lib_source);
        write_file(&main, main_source);

        let mut server = LspServer {
            semantic_tokens_refresh: true,
            ..LspServer::default()
        };
        let main_path =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path =
            server
                .session
                .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib_source);

        server
            .publish_diagnostics_for_entry(&main_path, &mut Vec::new())
            .expect("initial diagnostics should publish");
        let old_data = semantic_token_data_for(&mut server, &main_path);
        assert!(
            !old_data.is_empty(),
            "initial semantic token data should be non-empty"
        );

        server
            .session
            .update_document(&lib_path, onda_daemon::DocumentVersion(2), new_lib_source);
        server.note_document_changed(&lib_path);
        let mut writer = Vec::new();
        server
            .publish_diagnostics_for_entry(&main_path, &mut writer)
            .expect("refreshed diagnostics should publish");
        let messages = decode_lsp_messages(writer);

        assert!(
            messages.iter().any(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "workspace/semanticTokens/refresh")
                    .unwrap_or(false)
            }),
            "parsed diagnostic refresh should request semantic-token refresh: {messages:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_tokens_for_open_document_do_not_scan_import_dependencies() {
        let dir = mk_temp_dir("semantic_tokens_open_no_dependency_scan");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = target()
"#;
        write_file(
            &lib,
            r#"
def target():
  return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let data = semantic_token_data_for(&mut server, &normalized);

        assert!(
            !data.is_empty(),
            "open document should produce semantic tokens"
        );
        assert!(
            server.dependency_fingerprint_cache.disk_files.is_empty(),
            "open-document semantic tokens should not walk imported files"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_edit_refreshes_open_importer_semantic_tokens_immediately() {
        let dir = mk_temp_dir("semantic_tokens_open_dependency_edit");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\nsample:\n  out1 = target()\n";
        let old_lib = "def target():\n  return 0.0\n";
        let new_lib = "const target = 0.0\n";
        write_file(&lib, old_lib);
        write_file(&main, main_source);

        let mut server = LspServer::default();
        let main_path =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path = server
            .session
            .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib);
        let old_data = semantic_token_data_for(&mut server, &main_path);

        let changed =
            server
                .session
                .update_document(&lib_path, onda_daemon::DocumentVersion(2), new_lib);
        server.note_document_changed(&changed);
        let new_data = semantic_token_data_for(&mut server, &main_path);

        assert_ne!(
            old_data, new_data,
            "semantic tokens should use the edited overlay"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_uses_cached_parse_after_unsaved_edit() {
        let dir = mk_temp_dir("completion_cached_parse_after_edit");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0

use sc

init:
  b = Sin

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let fingerprint = super::source_fingerprint(source);
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &normalized,
            &server.session.overlay_map(),
        )
        .expect("source should parse");
        server.cache_parsed_program_for_path(&normalized, fingerprint, Some(parsed));
        server.note_document_changed(&normalized);

        let labels = completion_labels_for(&mut server, &main, source, "Sin");

        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "completion should reuse cached namespace index after edit: {labels:?}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_for_open_document_reuses_cached_imported_symbols_without_dependency_scan() {
        let dir = mk_temp_dir("completion_open_reparse_no_dependency_scan");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let initial_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()

sample:
  out1 = a()
"#;
        let edited_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()
  d = S

sample:
  out1 = a()
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0
"#,
        );
        write_file(&main, initial_source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), initial_source);
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &normalized,
            &server.session.overlay_map(),
        )
        .expect("initial source should parse");
        server.cache_parsed_program_for_path(
            &normalized,
            super::source_fingerprint(initial_source),
            Some(parsed),
        );
        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), edited_source);
        server.dependency_fingerprint_cache.clear();

        let labels = completion_labels_for(&mut server, &main, edited_source, "d = S");

        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "completion should keep imported namespace symbols from the cached parse: {labels:?}"
        );
        assert!(
            server.dependency_fingerprint_cache.disk_files.is_empty(),
            "open-document completion should not walk imported files"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostics_do_not_retain_stale_parse_after_syntax_error() {
        let dir = mk_temp_dir("diagnostic_error_drops_stale_parse_cache");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let initial_source = r#"
include "lib.onda"

use sc

init:
  t = 0.0

sample:
  out1 = 0.0
"#;
        let invalid_source = r#"
include "lib.onda"

use sc

init:
  t =

sample:
  out1 = 0.0
"#;
        let completion_source = r#"
include "lib.onda"

use sc

init:
  t = Si

sample:
  out1 = 0.0
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    const A = 1
"#,
        );
        write_file(&main, initial_source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), initial_source);
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("initial diagnostics should cache parsed snapshot");
        assert!(
            server
                .parse_cache
                .get(&normalized)
                .and_then(|cached| cached.parsed.as_ref())
                .is_some(),
            "initial successful diagnostics should cache a parsed snapshot"
        );

        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), invalid_source);
        server.note_document_changed(&main);
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("syntax diagnostics should publish");
        assert!(
            server
                .parse_cache
                .get(&normalized)
                .and_then(|cached| cached.parsed.as_ref())
                .is_none(),
            "failed diagnostics should not keep a stale parsed snapshot"
        );

        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(3), completion_source);
        server.note_document_changed(&main);
        let labels = completion_labels_for(&mut server, &main, completion_source, "Si");
        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "completion should reparse current source after an error: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_rebuilds_index_when_parsed_dependencies_change() {
        let dir = mk_temp_dir("completion_rebuilds_changed_dependency_index");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
include "lib.onda"

use sc

init:
  d = S

sample:
  out1 = 0.0
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    const A = 1
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_parsed = onda_frontend::parse_program_file_with_overlays(
            &super::normalize_path(&main),
            &server.session.overlay_map(),
        )
        .expect("initial source should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(source),
            Some(old_parsed),
        );
        let labels = completion_labels_for(&mut server, &main, source, "d = S");
        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "initial completion should build ready index: {labels:?}"
        );

        write_file(
            &lib,
            r#"
namespace sc:
  namespace SawOsc:
    const A = 1
"#,
        );
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &super::normalize_path(&main),
            &server.session.overlay_map(),
        )
        .expect("changed dependency should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(source),
            Some(parsed),
        );

        let labels = completion_labels_for(&mut server, &main, source, "d = S");
        assert!(
            labels.contains(&"SawOsc".to_owned()),
            "completion should use the replacement parsed dependency: {labels:?}"
        );
        assert!(
            !labels.contains(&"SinOsc".to_owned()),
            "completion should not retain stale dependency symbols: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn namespace_completion_for_open_document_reuses_cached_imported_symbols_without_dependency_scan(
    ) {
        let dir = mk_temp_dir("completion_open_namespace_reparse_no_dependency_scan");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let initial_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()

sample:
  out1 = a()
"#;
        let edited_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()
  d = SinOsc::

sample:
  out1 = a()
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0
"#,
        );
        write_file(&main, initial_source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), initial_source);
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &normalized,
            &server.session.overlay_map(),
        )
        .expect("initial source should parse");
        server.cache_parsed_program_for_path(
            &normalized,
            super::source_fingerprint(initial_source),
            Some(parsed),
        );
        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), edited_source);
        server.dependency_fingerprint_cache.clear();

        let labels = completion_labels_for(&mut server, &main, edited_source, "SinOsc::");

        assert!(
            labels.contains(&"ar".to_owned()),
            "namespace completion should keep imported namespace members from the cached parse: {labels:?}"
        );
        assert!(
            server.dependency_fingerprint_cache.disk_files.is_empty(),
            "open-document namespace completion should not walk imported files"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostic_entries_include_open_importers_after_dependency_change() {
        let dir = mk_temp_dir("diagnostic_dependency_importers");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\nouts:\n  out1\nsample:\n  out1 = Lib::target()\n";
        write_file(&lib, "namespace Lib:\n  def target():\n    return 0.0\n");
        write_file(&main, main_source);

        let mut server = LspServer::default();
        let main_path =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path = server.session.open_document(
            &lib,
            onda_daemon::DocumentVersion(2),
            "namespace Lib:\n  def target():\n    return invalid\n",
        );

        let affected = server.diagnostic_entries_affected_by_change(&lib_path);
        assert!(
            affected.contains(&main_path),
            "importing entry should be re-diagnosed after dependency change: {affected:?}"
        );
        assert!(
            affected.contains(&lib_path),
            "changed open document should be diagnosed: {affected:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_change_keeps_unrelated_entry_caches() {
        let dir = mk_temp_dir("targeted_dependency_invalidation");
        let first_lib = dir.join("first_lib.onda");
        let second_lib = dir.join("second_lib.onda");
        let first_main = dir.join("first_main.onda");
        let second_main = dir.join("second_main.onda");
        let first_lib_source = "const first = 1.0\n";
        let second_lib_source = "const second = 2.0\n";
        let first_main_source = "import first_lib\nsample:\n  out1 = first\n";
        let second_main_source = "import second_lib\nsample:\n  out1 = second\n";
        write_file(&first_lib, first_lib_source);
        write_file(&second_lib, second_lib_source);
        write_file(&first_main, first_main_source);
        write_file(&second_main, second_main_source);

        let mut server = LspServer::default();
        let first_main = server.session.open_document(
            &first_main,
            onda_daemon::DocumentVersion(1),
            first_main_source,
        );
        let second_main = server.session.open_document(
            &second_main,
            onda_daemon::DocumentVersion(1),
            second_main_source,
        );
        let first_lib = server.session.open_document(
            &first_lib,
            onda_daemon::DocumentVersion(1),
            first_lib_source,
        );
        server
            .publish_diagnostics_for_entry(&first_main, &mut Vec::new())
            .expect("cache first entry");
        server
            .publish_diagnostics_for_entry(&second_main, &mut Vec::new())
            .expect("cache second entry");
        assert!(server.parse_cache.contains_key(&first_main));
        assert!(server.parse_cache.contains_key(&second_main));

        server.session.update_document(
            &first_lib,
            onda_daemon::DocumentVersion(2),
            "const first = 3.0\n",
        );
        let affected = server.note_document_changed(&first_lib);

        assert!(affected.contains(&first_main));
        assert!(affected.contains(&first_lib));
        assert!(!affected.contains(&second_main));
        assert!(!server.parse_cache.contains_key(&first_main));
        assert!(
            server.parse_cache.contains_key(&second_main),
            "an unrelated source graph should retain its parse cache"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_edit_invalidates_importer_completion_immediately() {
        let dir = mk_temp_dir("dependency_edit_completion_invalidation");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\ninit:\n  value = Lib::Old\n";
        let old_lib = "namespace Lib:\n  const Old = 1\n";
        let new_lib = "namespace Lib:\n  const New = 1\n";
        write_file(&lib, old_lib);
        write_file(&main, main_source);

        let mut server = LspServer::default();
        server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        server
            .session
            .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib);
        let old_labels = completion_labels_for(&mut server, &main, main_source, "Lib::");
        assert!(
            old_labels.contains(&"Old".to_owned()),
            "labels: {old_labels:?}"
        );

        let changed =
            server
                .session
                .update_document(&lib, onda_daemon::DocumentVersion(2), new_lib);
        server.note_document_changed(&changed);
        let new_labels = completion_labels_for(&mut server, &main, main_source, "Lib::");

        assert!(
            new_labels.contains(&"New".to_owned()),
            "labels: {new_labels:?}"
        );
        assert!(
            !new_labels.contains(&"Old".to_owned()),
            "stale imported symbol should be gone: {new_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_edit_refreshes_importer_navigation_immediately() {
        let dir = mk_temp_dir("dependency_edit_navigation_invalidation");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\nsample:\n  out1 = target()\n";
        let old_lib = "def target():\n  return 0.0\n";
        let new_lib = "\n\ndef target():\n  return 1.0\n";
        write_file(&lib, old_lib);
        write_file(&main, main_source);

        let mut server = LspServer::default();
        server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path = server
            .session
            .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib);
        let old_definition = definition_for(&mut server, &main, main_source, "target");

        let changed =
            server
                .session
                .update_document(&lib_path, onda_daemon::DocumentVersion(2), new_lib);
        server.note_document_changed(&changed);
        let new_definition = definition_for(&mut server, &main, main_source, "target");

        assert_ne!(
            old_definition["range"]["start"]["line"], new_definition["range"]["start"]["line"],
            "navigation should use the edited dependency overlay"
        );

        fs::remove_dir_all(&dir).ok();
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
        assert!(triggers.contains(&"{"), "triggers: {triggers:?}");
    }

    #[test]
    fn initialize_advertises_signature_help_for_calls() {
        let result = initialize_result(None);
        assert_eq!(
            result["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );
    }

    #[test]
    fn completion_offers_unused_parameter_domain_fields() {
        let dir = mk_temp_dir("completion_param_domain_fields");
        let main = dir.join("main.onda");
        let source = "params:\n  cutoff = 440.0 {min = 20, ";
        write_file(&main, source);

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
        let items = completion_items_for(&mut server, &main, source, source);
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();

        assert!(!labels.contains(&"min"), "items: {items:?}");
        for expected in ["max", "scale", "curve", "unit", "step"] {
            assert!(labels.contains(&expected), "missing {expected}: {items:?}");
        }
        let scale = items
            .iter()
            .find(|item| item["label"] == json!("scale"))
            .expect("scale field completion");
        assert_eq!(scale["kind"], json!(10));
        assert_eq!(scale["detail"], json!("parameter domain field"));
        assert_eq!(scale["insertText"], json!("scale = ${1|linear,log|}"));
        assert_eq!(scale["insertTextFormat"], json!(2));
        let curve = items
            .iter()
            .find(|item| item["label"] == json!("curve"))
            .expect("curve field completion");
        assert_eq!(curve["insertText"], json!("curve = $1"));

        let shorthand_source = "params:\n  cutoff = 440.0 {20000, scale = linear, ";
        write_file(&main, shorthand_source);
        let items = completion_items_for(&mut server, &main, shorthand_source, shorthand_source);
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"min"), "items: {items:?}");
        assert!(labels.contains(&"max"), "items: {items:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_only_offers_count_for_buffer_annotations() {
        let dir = mk_temp_dir("completion_buffer_count_field");
        let main = dir.join("main.onda");
        let source = "buffers:\n  bank: f32 {";
        write_file(&main, source);

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
        let items = completion_items_for(&mut server, &main, source, source);
        assert_eq!(items.len(), 1, "items: {items:?}");
        assert_eq!(items[0]["label"], json!("count"));
        assert_eq!(items[0]["kind"], json!(10));
        assert_eq!(items[0]["detail"], json!("buffer count field"));
        assert_eq!(items[0]["insertText"], json!("count = $1"));
        assert_eq!(items[0]["insertTextFormat"], json!(2));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_treats_log_as_a_contextual_parameter_scale() {
        let dir = mk_temp_dir("completion_param_domain_log");
        let main = dir.join("main.onda");
        let named_source = "params:\n  cutoff = 440.0 {min = 20, max = 20000, scale = lo";
        write_file(&main, named_source);

        let mut server = LspServer::default();
        let named_items = completion_items_for(&mut server, &main, named_source, "scale = lo");
        assert_eq!(named_items.len(), 1, "items: {named_items:?}");
        assert_eq!(named_items[0]["label"], json!("log"));
        assert_eq!(named_items[0]["kind"], json!(20));
        assert_eq!(named_items[0]["detail"], json!("parameter scale"));

        let positional_source = "params:\n  cutoff = 440.0 {20, 20000, lo";
        let positional_items =
            completion_items_for(&mut server, &main, positional_source, positional_source);
        assert_eq!(positional_items.len(), 1, "items: {positional_items:?}");
        assert_eq!(positional_items[0]["label"], json!("log"));
        assert_eq!(positional_items[0]["kind"], json!(20));

        let expression_source = "params:\n  cutoff = 440.0 {lo";
        let expression_items =
            completion_items_for(&mut server, &main, expression_source, expression_source);
        let expression_log = expression_items
            .iter()
            .find(|item| item["label"] == json!("log"))
            .expect("stdlib log expression completion");
        assert_eq!(expression_log["kind"], json!(3));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_colon_trigger_ignores_single_colon() {
        let dir = mk_temp_dir("completion_single_colon_trigger");
        let main = dir.join("main.onda");
        let source = "namespace Foo:\n  const A = 1\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for_with_context(
            &mut server,
            &main,
            source,
            "Foo:",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        );

        assert!(
            items.is_empty(),
            "single colon trigger should not produce completions: {items:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_colon_trigger_allows_double_colon() {
        let dir = mk_temp_dir("completion_double_colon_trigger");
        let main = dir.join("main.onda");
        let source = "namespace Foo:\n  const A = 1\n\nsample:\n  out1 = Foo::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_items_for_with_context(
            &mut server,
            &main,
            source,
            "Foo::",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        )
        .iter()
        .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();

        assert!(
            labels.contains(&"A".to_owned()),
            "double colon trigger should produce namespace completions: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
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
            editor_visible: true,
        };
        let message = diagnostic_message(&diagnostic);
        assert!(message.contains("root error"));
        assert!(message.contains("trace:"));
        assert!(message.contains("- higher"));
        assert!(message.contains("- deep"));
    }

    #[test]
    fn diagnostic_to_lsp_uses_descriptive_code() {
        let diagnostic = Diagnostic {
            code: DiagCode::Syntax,
            message: "expected graph arrow".to_owned(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
            file: None,
            trace: Vec::new(),
            editor_visible: true,
        };

        let lsp = diagnostic_to_lsp(&diagnostic);
        assert_eq!(lsp["code"], json!("syntax"));
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

    #[cfg(unix)]
    #[test]
    fn lsp_document_paths_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = mk_temp_dir("symlink_document");
        let target = dir.join("main.onda");
        let alias = dir.join("linked.onda");
        write_file(&target, "sample:\n  out1 = 0.0\n");
        symlink(&target, &alias).expect("create document symlink");

        let error = lsp_document_path(&format!("file://{}", alias.display()))
            .expect_err("LSP documents must reject symlinks");
        assert!(error.contains("symlink component"));

        let cached = dir.join("cached.onda");
        write_file(&cached, "const value = 1.0\n");
        let mut cache = DependencyFingerprintCache::default();
        cache
            .disk_file_summary(&cached)
            .expect("cache regular dependency");
        fs::remove_file(&cached).expect("remove regular dependency");
        symlink(&target, &cached).expect("replace dependency with symlink");
        assert_eq!(
            cache
                .disk_file_summary(&cached)
                .expect_err("cached dependencies must reject symlink replacements"),
            std::io::ErrorKind::InvalidInput
        );

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn publish_diagnostics_does_not_immediately_clear_entry_uri() {
        let dir = mk_temp_dir("publish_diagnostics");
        let main = dir.join("main.onda");
        write_file(&main, "sample:\n  out1 = missing\n");

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
        let source = "sample:\n  out1 = missing\n";
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
    fn did_open_reports_unselected_buffer_collection_metadata() {
        let dir = mk_temp_dir("did_open_buffer_collection");
        let main = dir.join("main.onda");
        let source = "buffers:\n  bank: f32[] {2}\nblock:\n  channels = bank.chans()\n  sample:\n    out1 = 0.0\n";

        let mut server = LspServer::default();
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_file_uri(&main),
                            "languageId": "onda",
                            "version": 1,
                            "text": source
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        let diagnostics = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .flat_map(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("select a slot"))),
            "expected collection diagnostic from didOpen: {diagnostics:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_reports_invalid_integer_lookup_specializations() {
        let dir = mk_temp_dir("did_open_integer_lookup_position");
        let main = dir.join("main.onda");
        let source = "params:\n  layer: i32\n  position: i32\nbuffers:\n  layers: f32[] {4}\nblock:\n  source = layers[layer]\n  sample:\n    out1 = source.readL(0, position)\n";

        let mut server = LspServer::default();
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_file_uri(&main),
                            "languageId": "onda",
                            "version": 1,
                            "text": source
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        let diagnostics = decode_lsp_messages(writer)
            .into_iter()
            .find(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .and_then(|message| message["params"]["diagnostics"].as_array().cloned())
            .unwrap_or_default();
        assert_eq!(
            diagnostics.len(),
            1,
            "unexpected diagnostics: {diagnostics:?}"
        );
        let diagnostic = &diagnostics[0];
        let message = diagnostic["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("while checking specialization of 'std::lookup::_split_position'")
                && message.contains("got I32"),
            "unexpected diagnostic message: {message}"
        );
        assert_eq!(diagnostic["code"], json!("semantic"));
        assert_eq!(diagnostic["range"]["start"]["line"], json!(8));
        assert_eq!(diagnostic["range"]["start"]["character"], json!(11));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_empty_document_publishes_no_errors() {
        let dir = mk_temp_dir("did_open_empty_document");
        let main = dir.join("empty.onda");

        let mut server = LspServer::default();
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_file_uri(&main),
                            "languageId": "onda",
                            "version": 1,
                            "text": ""
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        let notifications = decode_lsp_messages(writer);
        let diagnostics = notifications
            .iter()
            .filter(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .flat_map(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .collect::<Vec<_>>();
        assert!(diagnostics.is_empty(), "diagnostics: {diagnostics:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lsp_hides_missing_sample_diagnostic() {
        let dir = mk_temp_dir("hide_missing_sample_diagnostic");
        let main = dir.join("main.onda");
        let source = "outs:\n  out1\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let mut writer = Vec::new();
        server
            .publish_diagnostics_for_entry(&normalized, &mut writer)
            .expect("diagnostics should publish");

        let notifications = decode_lsp_messages(writer);
        let messages = notifications
            .iter()
            .filter(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .flat_map(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .filter_map(|diagnostic| diagnostic["message"].as_str())
            .collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .all(|message| !message.contains("missing required 'sample' block")),
            "messages: {messages:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_runs_diagnostics_even_when_diagnostics_are_deferred() {
        let dir = mk_temp_dir("did_open_deferred_runs_now");
        let main = dir.join("main.onda");
        let source = r#"
import std/delay

proc CyberCell<T>:
  ins<T>:
    src
    fb

  outs<T> 1

  params<T>:
    drive = 1.0
    bias = 0.0

  sample:
    out1 = (src + fb + bias) * drive

init:
  smear = std::delay::Delay<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer {
            defer_diagnostics: true,
            ..LspServer::default()
        };
        let uri = path_to_file_uri(&main);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": source,
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        assert!(
            server.diagnostic_requests.is_empty(),
            "didOpen should run diagnostics instead of scheduling them"
        );
        let normalized = server
            .session
            .open_documents()
            .keys()
            .next()
            .expect("opened document path")
            .clone();
        assert!(
            server
                .parse_cache
                .get(&normalized)
                .and_then(|cached| cached.parsed.as_ref())
                .is_some(),
            "didOpen diagnostics should populate the parsed snapshot"
        );
        let definition = definition_for(&mut server, &main, source, "Delay");
        assert_ne!(
            definition,
            json!(null),
            "definition should use the didOpen-populated parsed snapshot"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_save_publishes_diagnostics_immediately() {
        let dir = mk_temp_dir("did_save_publish");
        let main = dir.join("main.onda");
        let valid_source = "sample:\n  out1 = 0.0\n";
        let invalid_source = "sample:\n  out1 = missing\n";
        write_file(&main, valid_source);

        let mut server = LspServer::default();
        let uri = path_to_file_uri(&main);
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": valid_source,
                        }
                    }
                }),
                &mut Vec::new(),
            )
            .expect("didOpen should succeed");

        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didSave",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                        },
                        "text": invalid_source,
                    }
                }),
                &mut writer,
            )
            .expect("didSave should succeed");

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
            "expected didSave to publish diagnostics: {notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_change_publishes_diagnostics_in_synchronous_mode() {
        let dir = mk_temp_dir("did_change_publish_sync");
        let main = dir.join("main.onda");
        let valid_source = "sample:\n  out1 = 0.0\n";
        let invalid_source = "sample:\n  out1 = missing\n";
        write_file(&main, valid_source);

        let mut server = LspServer::default();
        let uri = path_to_file_uri(&main);
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": valid_source,
                        }
                    }
                }),
                &mut Vec::new(),
            )
            .expect("didOpen should succeed");

        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 2,
                        },
                        "contentChanges": [
                            {
                                "text": invalid_source,
                            }
                        ],
                    }
                }),
                &mut writer,
            )
            .expect("didChange should succeed");

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
            "didChange should publish diagnostics in synchronous mode: {notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_change_defers_diagnostics_in_deferred_mode() {
        let dir = mk_temp_dir("did_change_deferred");
        let main = dir.join("main.onda");
        let valid_source = "sample:\n  out1 = 0.0\n";
        let invalid_source = "sample:\n  out1 = missing\n";
        write_file(&main, valid_source);

        let mut server = LspServer {
            defer_diagnostics: true,
            ..LspServer::default()
        };
        let uri = path_to_file_uri(&main);
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": valid_source,
                        }
                    }
                }),
                &mut Vec::new(),
            )
            .expect("didOpen should succeed");
        server.take_diagnostic_requests();

        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 2,
                        },
                        "contentChanges": [
                            {
                                "text": invalid_source,
                            }
                        ],
                    }
                }),
                &mut writer,
            )
            .expect("didChange should succeed");

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
            notifications.is_empty(),
            "deferred didChange should not write diagnostics on the request path: {notifications:?}"
        );

        let requests = server.take_diagnostic_requests();
        assert!(
            matches!(
                requests.as_slice(),
                [DiagnosticScheduleRequest {
                    delay: DiagnosticDelay::Debounced,
                    ..
                }]
            ),
            "deferred didChange should queue debounced affected-entry diagnostics: {requests:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deferred_diagnostics_drop_stale_generations() {
        let dir = mk_temp_dir("diagnostic_stale_generation");
        let main = dir.join("main.onda");
        write_file(&main, "sample:\n  out1 = 0.0\n");
        let main = super::normalize_path(&main);

        let (immediate_tx, _immediate_rx) = mpsc::channel();
        let (background_tx, _background_rx) = mpsc::channel();
        let mut core = LspCore::new(immediate_tx, background_tx);
        core.diagnostic_generations.insert(main.clone(), 2);

        let mut writer = Vec::new();
        core.publish_diagnostic_result(
            DiagnosticJobResult {
                entry_path: main,
                generation: 1,
                diagnostics: Vec::new(),
                sources: SourceManifest::default(),
                parse_succeeded: false,
                parse_fingerprint: None,
                completion_index_snapshot: None,
            },
            &mut writer,
        )
        .expect("stale diagnostics should be accepted and ignored");

        assert!(
            decode_lsp_messages(writer).is_empty(),
            "stale diagnostics should not publish notifications"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deferred_diagnostics_drop_closed_entries() {
        let dir = mk_temp_dir("diagnostic_closed_entry");
        let main = dir.join("main.onda");
        write_file(&main, "sample:\n  out1 = 0.0\n");
        let main = super::normalize_path(&main);

        let (immediate_tx, _immediate_rx) = mpsc::channel();
        let (background_tx, _background_rx) = mpsc::channel();
        let mut core = LspCore::new(immediate_tx, background_tx);
        core.diagnostic_generations.insert(main.clone(), 1);

        let mut writer = Vec::new();
        core.publish_diagnostic_result(
            DiagnosticJobResult {
                entry_path: main,
                generation: 1,
                diagnostics: Vec::new(),
                sources: SourceManifest::default(),
                parse_succeeded: false,
                parse_fingerprint: None,
                completion_index_snapshot: None,
            },
            &mut writer,
        )
        .expect("closed-entry diagnostics should be accepted and ignored");

        assert!(
            decode_lsp_messages(writer).is_empty(),
            "closed-entry diagnostics should not publish notifications"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deferred_diagnostic_parse_keeps_source_files_for_navigation() {
        let dir = mk_temp_dir("diagnostic_parse_source_files");
        let main = super::normalize_path(&dir.join("main.onda"));
        let lib = super::normalize_path(&dir.join("lib.onda"));
        let main_source = "import lib\n\nsample:\n  out1 = Target\n";
        let lib_source = "const Target = 1.0\n";
        write_file(&main, main_source);
        write_file(&lib, lib_source);

        // Source locations use compact thread-local IDs. Occupy the IDs that
        // the diagnostic worker will independently assign to this source graph
        // so a worker-owned Program cannot accidentally appear valid here.
        let _ = onda_frontend::SourceLoc::new(
            Some(dir.join("unrelated_one.onda").display().to_string()),
            1,
            1,
            1,
            2,
            Vec::new(),
        );
        let _ = onda_frontend::SourceLoc::new(
            Some(dir.join("unrelated_two.onda").display().to_string()),
            1,
            1,
            1,
            2,
            Vec::new(),
        );

        let worker_main = main.clone();
        let worker_source = main_source.to_owned();
        let result = std::thread::spawn(move || {
            super::run_diagnostic_job(super::DiagnosticJob {
                entry_path: worker_main.clone(),
                generation: 1,
                open_documents: vec![super::DiagnosticOpenDocument {
                    path: worker_main,
                    version: onda_daemon::DocumentVersion(1),
                    text: worker_source,
                }],
            })
        })
        .join()
        .expect("diagnostic worker should finish");
        assert!(result.parse_succeeded, "worker source should parse");

        let mut server = LspServer::default();
        server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        server
            .publish_diagnostic_result(result, &mut Vec::new())
            .expect("diagnostic result should publish");

        let definition = definition_for(&mut server, &main, main_source, "Target");
        let hover = hover_markdown_for(&mut server, &main, main_source, "Target")
            .expect("imported definition should have hover information");
        assert_eq!(
            definition["uri"],
            json!(path_to_file_uri(&lib)),
            "imported definition should retain the worker's source file: {definition:?}"
        );
        assert!(
            hover.contains(&lib.display().to_string()),
            "hover should name the imported definition's source file: {hover}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn immediate_diagnostics_use_immediate_worker_lane() {
        let dir = mk_temp_dir("diagnostic_immediate_lane");
        let main = super::normalize_path(&dir.join("main.onda"));
        write_file(&main, "sample:\n  out1 = 0.0\n");

        let (immediate_tx, immediate_rx) = mpsc::channel();
        let (background_tx, background_rx) = mpsc::channel();
        let mut core = LspCore::new(immediate_tx, background_tx);
        core.schedule_diagnostic_entries(vec![main.clone()], DiagnosticDelay::Immediate);
        core.dispatch_due_diagnostics();

        let job = immediate_rx
            .try_recv()
            .expect("immediate diagnostics should dispatch to immediate lane");
        assert_eq!(job.entry_path, main);
        assert!(
            background_rx.try_recv().is_err(),
            "immediate diagnostics should not wait in the background lane"
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
    fn completion_filters_private_proc_params_after_receiver_dot() {
        let dir = mk_temp_dir("completion_proc_members");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    private cutoff = 1000.0
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
            "private param should not be exposed after receiver dot: {labels:?}"
        );
        assert!(
            !labels.contains(&"params".to_owned()),
            "dynamic params should be hidden when a proc has private params: {labels:?}"
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
    fn completion_filters_private_proc_params_in_live_call_args() {
        let dir = mk_temp_dir("completion_proc_call_args");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    private cutoff = 1000.0
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
            "private param should not be exposed as a live call arg: {labels:?}"
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
    fn completion_discovers_std_namespaces_without_prior_import() {
        let dir = mk_temp_dir("completion_std_namespace_discovery");
        let main = dir.join("main.onda");
        let source = "init:\n  a = std::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std::");
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();

        for expected in ["osc", "filter", "env", "delay", "dynamics", "sample"] {
            assert!(labels.contains(&expected), "missing {expected}: {items:?}");
        }
        for unrelated in ["init", "sin", "PI"] {
            assert!(
                !labels.contains(&unrelated),
                "unexpected {unrelated}: {items:?}"
            );
        }
        assert!(
            items.iter().all(|item| item["kind"] == json!(9)),
            "std:: should contain only namespaces: {items:?}"
        );
        assert!(
            items.iter().all(|item| {
                item["sortText"]
                    .as_str()
                    .is_some_and(|sort_text| sort_text.starts_with("00_00_"))
            }),
            "namespace ranks: {items:?}"
        );

        let osc = items
            .iter()
            .find(|item| item["label"] == json!("osc"))
            .expect("osc namespace completion");
        assert_eq!(
            osc["additionalTextEdits"][0]["newText"],
            json!("import std/osc\n"),
            "item: {osc:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_filters_std_namespace_prefix_without_prior_import() {
        let dir = mk_temp_dir("completion_std_namespace_prefix");
        let main = dir.join("main.onda");
        let source = "init:\n  a = std::o\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "std::o");

        assert_eq!(labels, vec!["osc".to_owned()], "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_discovers_std_module_symbols_without_prior_import() {
        let dir = mk_temp_dir("completion_std_module_symbol_discovery");
        let main = dir.join("main.onda");
        let source = "init:\n  a = std::osc::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std::osc::");
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();

        for expected in ["Phasor", "Sine", "Saw", "Pulse"] {
            assert!(labels.contains(&expected), "missing {expected}: {items:?}");
        }
        for unrelated in ["init", "sample", "sin", "PI", "Complex"] {
            assert!(
                !labels.contains(&unrelated),
                "unexpected {unrelated}: {items:?}"
            );
        }

        assert_eq!(
            &labels[..8],
            ["KSine", "Phasor", "Pulse", "Saw", "SawDown", "Sine", "Square", "Triangle"],
            "constructors should lead the qualified list: {items:?}"
        );
        assert!(
            items[..8].iter().all(|item| {
                item["sortText"]
                    .as_str()
                    .is_some_and(|sort_text| sort_text.starts_with("00_01_"))
            }),
            "constructor ranks: {items:?}"
        );

        let sine = items
            .iter()
            .find(|item| item["label"] == json!("Sine"))
            .expect("Sine completion");
        assert_eq!(sine["kind"], json!(4), "item: {sine:?}");
        assert_eq!(
            sine["additionalTextEdits"][0]["newText"],
            json!("import std/osc\n"),
            "item: {sine:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_duplicate_existing_std_import_edit() {
        let dir = mk_temp_dir("completion_existing_std_import");
        let main = dir.join("main.onda");
        let source = "import std/osc\ninit:\n  a = std::osc::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std::osc::");
        let sine = items
            .iter()
            .find(|item| item["label"] == json!("Sine"))
            .expect("Sine completion");

        assert!(
            sine.get("additionalTextEdits").is_none(),
            "existing import should not be duplicated: {sine:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_replaces_general_session_across_std_namespace_triggers() {
        let dir = mk_temp_dir("completion_std_trigger_sequence");
        let main = dir.join("main.onda");
        let mut server = LspServer::default();

        let root_source = "init:\n  a = std::\n";
        write_file(&main, root_source);
        let root_labels = completion_items_for_with_context(
            &mut server,
            &main,
            root_source,
            "std::",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        )
        .iter()
        .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
        assert!(
            root_labels.contains(&"osc".to_owned()),
            "labels: {root_labels:?}"
        );
        assert!(
            !root_labels.contains(&"Sine".to_owned()),
            "labels: {root_labels:?}"
        );

        let module_source = "init:\n  a = std::osc::\n";
        let module_labels = completion_items_for_with_context(
            &mut server,
            &main,
            module_source,
            "std::osc::",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        )
        .iter()
        .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
        assert!(
            module_labels.contains(&"Sine".to_owned()),
            "labels: {module_labels:?}"
        );
        assert!(
            !module_labels.contains(&"sample".to_owned()),
            "labels: {module_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_marks_empty_qualified_prefix_incomplete_for_requery() {
        let dir = mk_temp_dir("completion_incomplete_requery");
        let main = dir.join("main.onda");
        let source = "import std/osc\ninit:\n  sine = std::osc::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let result = server
            .completions_for_uri(
                &path_to_file_uri(&normalized),
                position_after(source, "std::osc::"),
                None,
            )
            .expect("completion should succeed");

        assert_eq!(result["isIncomplete"], json!(true), "result: {result:?}");
        assert!(
            result["items"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["label"] == json!("Sine"))),
            "result: {result:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_only_std_osc_members_on_first_qualified_request() {
        let dir = mk_temp_dir("completion_std_osc_first_qualified_request");
        let main = dir.join("main.onda");
        write_file(&main, "");

        let mut server = LspServer::default();
        let normalized = server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), "");
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("empty document diagnostics should populate the initial parse cache");

        let source = r#"import std/osc

init:
  sine = std::osc::Si
"#;
        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), source);
        server.note_document_changed(&main);

        let items = completion_items_for(&mut server, &main, source, "std::osc::Si");

        assert_eq!(items.len(), 1, "items: {items:?}");
        assert_eq!(items[0]["label"], json!("Sine"), "item: {:?}", items[0]);
        assert_eq!(items[0]["kind"], json!(4), "item: {:?}", items[0]);
        assert!(
            items[0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.starts_with("proc std::osc::Sine")),
            "item: {:?}",
            items[0]
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_for_bare_std_osc_namespace_excludes_unrelated_symbols() {
        let dir = mk_temp_dir("completion_std_osc_namespace_only");
        let main = dir.join("main.onda");
        let source = r#"import std/osc

init:
  sine = std::osc::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "std::osc::");

        for expected in ["Phasor", "Sine", "Saw", "poly_blep"] {
            assert!(
                labels.contains(&expected.to_owned()),
                "missing {expected}: {labels:?}"
            );
        }
        for unrelated in ["sin", "PI", "sample", "Complex"] {
            assert!(
                !labels.contains(&unrelated.to_owned()),
                "unexpected {unrelated}: {labels:?}"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_after_import_slash_inserts_only_the_remaining_segment() {
        let dir = mk_temp_dir("completion_import_path_segment");
        let main = dir.join("main.onda");
        let source = "import std/";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std/");
        let osc = items
            .iter()
            .find(|item| item["label"] == json!("osc"))
            .unwrap_or_else(|| panic!("missing osc module completion: {items:?}"));

        assert_eq!(osc["insertText"], json!("osc"), "item: {osc:?}");
        assert!(
            items
                .iter()
                .all(|item| item["insertText"] != json!("std/osc")),
            "completion must not duplicate the existing std/ prefix: {items:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_resolve_unqualified_imported_namespace_types() {
        let dir = mk_temp_dir("completion_unqualified_imported_namespace_type");
        let main = dir.join("main.onda");
        let source = r#"import std/osc

init:
  sine = Sine()
  sine.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sine.");

        assert!(labels.is_empty(), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_expose_std_members_through_use_without_import() {
        let dir = mk_temp_dir("completion_std_use_without_import");
        let main = dir.join("main.onda");
        let source = r#"use std::osc

init:
  sine = Si
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "Si");

        assert!(
            !labels.contains(&"Sine".to_owned()),
            "use must not expose members of an unimported module: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_does_not_resolve_members_of_unqualified_imported_namespace_types() {
        let dir = mk_temp_dir("navigation_unqualified_imported_namespace_type");
        let main = dir.join("main.onda");
        let source = r#"import std/osc

init:
  sine = Sine()
  value = sine.freq
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "sine.freq");

        assert_eq!(definition, json!(null), "definition: {definition:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_unqualified_namespace_types_after_use() {
        let dir = mk_temp_dir("completion_used_namespace_type");
        let main = dir.join("main.onda");
        let source = r#"import std/osc
use std::osc

init:
  sine = Sine()
  sine.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sine.");

        assert!(labels.contains(&"freq".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"amp".to_owned()), "labels: {labels:?}");

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
    fn completion_lists_child_namespaces_from_single_namespace_use() {
        let dir = mk_temp_dir("completion_single_namespace_use_children");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0

use sc

init:
  a = SinO

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "SinO");

        assert!(labels.contains(&"SinOsc".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_walks_child_namespace_from_single_namespace_use() {
        let dir = mk_temp_dir("completion_single_namespace_use_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0
    proc kr:
      kouts:
        kout1
      block:
        kout1 = 0.0

use sc

init:
  a = SinOsc::a

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let all_labels = completion_labels_for(&mut server, &main, source, "SinOsc::");
        let ar_labels = completion_labels_for(&mut server, &main, source, "SinOsc::a");

        assert!(
            all_labels.contains(&"ar".to_owned()),
            "labels: {all_labels:?}"
        );
        assert!(
            all_labels.contains(&"kr".to_owned()),
            "labels: {all_labels:?}"
        );
        assert!(
            ar_labels.contains(&"ar".to_owned()),
            "labels: {ar_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_walks_child_namespace_alias_from_single_namespace_use() {
        let dir = mk_temp_dir("completion_single_namespace_use_alias_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace ugens:
  namespace LocalOsc:
    def ar():
      return 0.0
    def kr():
      return 0.0

  namespace Osc = LocalOsc

use ugens

init:
  a = Osc::a

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "Osc::a");

        assert!(labels.contains(&"ar".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_walks_child_namespace_from_single_namespace_use() {
        let dir = mk_temp_dir("definition_single_namespace_use_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    def ar():
      return 0.0

use sc

sample:
  out1 = SinOsc::ar()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "SinOsc::ar");

        assert_ne!(
            definition,
            json!(null),
            "definition should resolve through single namespace use"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_walks_child_namespace_alias_from_single_namespace_use() {
        let dir = mk_temp_dir("definition_single_namespace_use_alias_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace ugens:
  namespace LocalOsc:
    def ar():
      return 0.0

  namespace Osc = LocalOsc

use ugens

sample:
  out1 = Osc::ar()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Osc::ar");

        assert_ne!(
            definition,
            json!(null),
            "definition should resolve through namespace alias from single namespace use"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_struct_fields_through_lsp_server_path() {
        let dir = mk_temp_dir("definition_struct_fields_lsp");
        let main = dir.join("main.onda");
        let source = r#"import std/math

struct Box:
  value: f32 = 0.0

  def set(self, target):
    self.value = 0.0

  def get(self):
    return self.value
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let field_definition = definition_for(&mut server, &main, source, "  value");
        let self_definition = definition_for(&mut server, &main, source, "self.value");

        assert_eq!(
            field_definition["range"]["start"]["line"],
            json!(3),
            "field declaration should resolve through server path: {field_definition:?}"
        );
        assert_eq!(
            self_definition["range"]["start"]["line"],
            json!(3),
            "self.field should resolve through server path: {self_definition:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hover_resolves_struct_fields_through_lsp_server_path() {
        let dir = mk_temp_dir("hover_struct_fields_lsp");
        let main = dir.join("main.onda");
        let source = r#"import std/math

struct Box:
  value: f32 = 0.0

  def set(self, target):
    self.value = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let field_hover =
            hover_markdown_for(&mut server, &main, source, "  value").unwrap_or_default();
        let self_hover =
            hover_markdown_for(&mut server, &main, source, "self.value").unwrap_or_default();

        assert!(
            field_hover.contains("field value"),
            "field declaration hover should resolve through server path: {field_hover:?}"
        );
        assert!(
            self_hover.contains("field value"),
            "self.field hover should resolve through server path: {self_hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_current_struct_fields_with_stale_parse_cache() {
        let dir = mk_temp_dir("navigation_struct_fields_stale_parse");
        let main = super::normalize_path(&dir.join("main.onda"));
        let old_source = r#"import std/math

struct Box:
  old_value: f32 = 0.0

  def get(self):
    return self.old_value
"#;
        let current_source = r#"import std/math

struct Box:
  value: f32 = 0.0

  def get(self):
    return self.value
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_parsed =
            onda_frontend::parse_program_file_with_overlays(&main, &server.session.overlay_map())
                .expect("old source should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(old_source),
            Some(old_parsed),
        );
        write_file(&main, current_source);

        let field_definition = definition_for(&mut server, &main, current_source, "  value");
        let self_definition = definition_for(&mut server, &main, current_source, "self.value");
        let self_hover = hover_markdown_for(&mut server, &main, current_source, "self.value")
            .unwrap_or_default();

        assert_eq!(
            field_definition["range"]["start"]["line"],
            json!(3),
            "current field declaration should resolve despite stale parse: {field_definition:?}"
        );
        assert_eq!(
            self_definition["range"]["start"]["line"],
            json!(3),
            "current self.field should resolve despite stale parse: {self_definition:?}"
        );
        assert!(
            self_hover.contains("field value"),
            "current self.field hover should resolve despite stale parse: {self_hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_struct_methods_and_typed_param_members_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_struct_methods_lsp");
        let main = dir.join("main.onda");
        let source = r#"
struct Box:
  value: f32 = 0.0

  def set(self, target):
    self.value = 0.0

  def get(self):
    return self.value

  def bump(self, amount):
    self.set(target = amount)

def read(item: Box):
  return item.value + item.get()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let self_method = definition_for(&mut server, &main, source, "self.set");
        let self_method_arg = definition_for(&mut server, &main, source, "target");
        let param_field = definition_for(&mut server, &main, source, "item.value");
        let param_method = definition_for(&mut server, &main, source, "item.get");

        assert_eq!(self_method["range"]["start"]["line"], json!(4));
        assert_eq!(self_method_arg["range"]["start"]["line"], json!(4));
        assert_eq!(param_field["range"]["start"]["line"], json!(2));
        assert_eq!(param_method["range"]["start"]["line"], json!(7));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_struct_constructor_field_args_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_struct_ctor_args_lsp");
        let main = dir.join("main.onda");
        let source = r#"
struct Pair:
  left: f32 = 0.0
  right: f32 = 0.0

init:
  p = Pair(left = 1.0, right = 2.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let left = definition_for(&mut server, &main, source, "left");
        let right = definition_for(&mut server, &main, source, "right");

        assert_eq!(left["range"]["start"]["line"], json!(2));
        assert_eq!(right["range"]["start"]["line"], json!(3));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_namespace_local_const_use_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_namespace_local_const_use_lsp");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  def shape(x):
    return x + Bias
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "return x + Bias");
        let hover =
            hover_markdown_for(&mut server, &main, source, "return x + Bias").unwrap_or_default();

        assert_eq!(definition["range"]["start"]["line"], json!(2));
        assert!(
            hover.contains("const Bias"),
            "namespace-local const hover should resolve at use site: {hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_namespace_local_const_in_generic_def_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_namespace_generic_const_use_lsp");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  const TEST = 10
  
  def sampleDuration<T>():
    return T(1.0) / T(SR)

  def blockDuration<T>():
    return T(BS) / T(SR) * TEST
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "* TEST");
        let hover = hover_markdown_for(&mut server, &main, source, "* TEST").unwrap_or_default();

        assert_eq!(
            definition["range"]["start"]["line"],
            json!(2),
            "namespace-local const in generic def should resolve through server path: {definition:?}"
        );
        assert!(
            hover.contains("const TEST"),
            "namespace-local const hover should resolve in generic def: {hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_current_namespace_const_with_stale_parse_cache() {
        let dir = mk_temp_dir("navigation_namespace_const_stale_parse");
        let main = super::normalize_path(&dir.join("main.onda"));
        let old_source = r#"
namespace sc:
  def blockDuration<T>():
    return T(BS) / T(SR)
"#;
        let current_source = r#"
namespace sc:
  const TEST = 10
  
  def sampleDuration<T>():
    return T(1.0) / T(SR)

  def blockDuration<T>():
    return T(BS) / T(SR) * TEST
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_parsed =
            onda_frontend::parse_program_file_with_overlays(&main, &server.session.overlay_map())
                .expect("old source should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(old_source),
            Some(old_parsed),
        );
        write_file(&main, current_source);

        let definition = definition_for(&mut server, &main, current_source, "* TEST");
        let hover =
            hover_markdown_for(&mut server, &main, current_source, "* TEST").unwrap_or_default();

        assert_eq!(
            definition["range"]["start"]["line"],
            json!(2),
            "current namespace const should resolve despite stale parse: {definition:?}"
        );
        assert!(
            hover.contains("const TEST"),
            "current namespace const hover should resolve despite stale parse: {hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_struct_self_members_typed_param_members_and_constructor_args() {
        let dir = mk_temp_dir("completion_struct_members_args");
        let main = dir.join("main.onda");
        let self_source = r#"
struct Box:
  value: f32 = 0.0

  def set(self, x):
    self.value = x

  def get(self):
    return self.value

  def complete_self(self):
    self.
"#;
        let self_arg_source = r#"
struct Box:
  value: f32 = 0.0

  def set(self, x):
    self.value = x

  def complete_self_args(self):
    self.set(
"#;
        let typed_param_source = r#"
struct Box:
  value: f32 = 0.0

  def get(self):
    return self.value

def read(item: Box):
  return item.
"#;
        let ctor_source = r#"
struct Box:
  value: f32 = 0.0

init:
  b = Box(
"#;
        write_file(&main, self_source);

        let mut server = LspServer::default();
        let self_labels = completion_labels_for(&mut server, &main, self_source, "self.");
        assert!(
            self_labels.contains(&"value".to_owned()),
            "labels: {self_labels:?}"
        );
        assert!(
            self_labels.contains(&"set".to_owned()),
            "labels: {self_labels:?}"
        );

        let arg_labels = completion_labels_for(&mut server, &main, self_arg_source, "self.set(");
        assert!(
            arg_labels.contains(&"x".to_owned()),
            "labels: {arg_labels:?}"
        );

        let param_labels = completion_labels_for(&mut server, &main, typed_param_source, "item.");
        assert!(
            param_labels.contains(&"value".to_owned()),
            "labels: {param_labels:?}"
        );
        assert!(
            param_labels.contains(&"get".to_owned()),
            "labels: {param_labels:?}"
        );

        let ctor_labels = completion_labels_for(&mut server, &main, ctor_source, "Box(");
        assert!(
            ctor_labels.contains(&"value".to_owned()),
            "labels: {ctor_labels:?}"
        );

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
            "Alpha", "Zoo", "AProc", "ZProc", "AStruct", "ZStruct", "adef", "zdef", "AConst",
            "ZConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("Alpha", "00_00_Alpha"),
            ("AProc", "00_01_AProc"),
            ("AStruct", "00_02_AStruct"),
            ("adef", "00_10_adef"),
            ("AConst", "00_11_AConst"),
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
    fn completion_prefers_matching_case_then_declaration_kind() {
        let dir = mk_temp_dir("completion_case_aware_order");
        let main = dir.join("main.onda");
        let source = r#"
namespace Cases:
  namespace SNamespace:
    const X = 1

  namespace sNamespace:
    const X = 1

  proc SProc:
    outs:
      out1
    sample:
      out1 = 0.0

  proc sProc:
    outs:
      out1
    sample:
      out1 = 0.0

  struct SStruct:
    value: f32

  struct sStruct:
    value: f32

  def SDef():
    return 0.0

  def sDef():
    return 0.0

  const SConst = 1
  const sConst = 1

outs:
  out1

sample:
  out1 = Cases::S
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "Cases::S");
        let expected = vec![
            "SNamespace",
            "SProc",
            "SStruct",
            "SDef",
            "SConst",
            "sNamespace",
            "sProc",
            "sStruct",
            "sDef",
            "sConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("SNamespace", "0_00_00_SNamespace"),
            ("SProc", "0_00_01_SProc"),
            ("sNamespace", "1_00_00_sNamespace"),
            ("sProc", "1_00_01_sProc"),
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
    fn completion_requeries_after_empty_general_prefix_before_case_ranking() {
        let dir = mk_temp_dir("completion_case_aware_requery");
        let main = dir.join("main.onda");
        let empty_prefix_source = r#"import std/osc
use std::osc

init:
  osc = Sine()
  tosc = 

sample:
  out1 = osc()
"#;
        write_file(&main, empty_prefix_source);

        let mut server = LspServer::default();
        let normalized = server.session.open_document(
            &main,
            onda_daemon::DocumentVersion(1),
            empty_prefix_source,
        );
        let initial = server
            .completions_for_uri(
                &path_to_file_uri(&normalized),
                position_after(empty_prefix_source, "tosc = "),
                None,
            )
            .expect("initial completion should succeed");
        assert_eq!(
            initial["isIncomplete"],
            json!(true),
            "the client must requery after the first identifier character: {initial:?}"
        );

        let typed_source = empty_prefix_source.replacen("tosc = ", "tosc = S", 1);
        let labels = completion_labels_for(&mut server, &main, &typed_source, "tosc = S");
        assert_eq!(
            labels.first().map(String::as_str),
            Some("Saw"),
            "{labels:?}"
        );
        let std_position = labels
            .iter()
            .position(|label| label == "std")
            .expect("lowercase std fallback");
        assert!(
            std_position > 0,
            "uppercase matches must precede std: {labels:?}"
        );

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

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
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

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
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
  x = buf[0]
  y = fabs(0.0 - 1.0)
  sr = HOST_SR
  out1 = x + y + sr
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let len = hover_markdown_for(&mut server, &main, source, "len")
            .expect("hover should resolve len builtin");
        let fabs = hover_markdown_for(&mut server, &main, source, "fabs")
            .expect("hover should resolve fabs builtin alias");
        let host_sr = hover_markdown_for(&mut server, &main, source, "HOST_SR")
            .expect("hover should resolve HOST_SR builtin const");

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
    private cutoff: f32 = 1000.0
    gain: f32 = 1.0

  buffers:
    table: buffer<f32>

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
                "proc Voice<T>(private cutoff: f32 = 1000.0, gain: f32 = 1.0, table: buffer<f32>)"
            ),
            "hover: {constructor_hover}"
        );
        assert!(
            call_hover.contains("proc call voice(input: T, gain: f32 = 1.0)"),
            "hover: {call_hover}"
        );
        assert!(
            init_hover.contains("event init(private cutoff: f32 = 1000.0, gain: f32 = 1.0)"),
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
            json!("event init(private cutoff: f32 = 1000.0, gain: f32 = 1.0)")
        );
        assert_eq!(set_item["detail"], json!("event set(v: f32 = 0.5)"));
        assert_eq!(
            init_item["labelDetails"]["detail"],
            json!("(private cutoff: f32 = 1000.0, gain: f32 = 1.0)")
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
          kernel: buffer<T>

        sample:
          out1 = in1 * gain

  namespace Convolution3<FFTSize = 2048, MaxKernel = 8192>:
    namespace Mono:
      proc ar<T>:
        ins<T>:
          in1
          trigger

        buffers:
          kernel: buffer<T>

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
    fn navigation_treats_first_qualified_segment_as_namespace() {
        let dir = mk_temp_dir("navigation_qualified_namespace_segment");
        let main = dir.join("main.onda");
        let source = concat!(
            "namespace mode:\n",
            "  const LOW = 0\n",
            "\n",
            "proc Filter:\n",
            "  params:\n",
            "    mode: i32 = mode::LOW\n",
            "\n",
            "  outs:\n",
            "    out1\n",
            "    out2\n",
            "\n",
            "  sample:\n",
            "    out1 = mode::LOW\n",
            "    out2 = mode\n",
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let qualified_definition = definition_for(&mut server, &main, source, "out1 = mode");
        let bare_definition = definition_for(&mut server, &main, source, "out2 = mode");
        let qualified_hover =
            hover_markdown_for(&mut server, &main, source, "out1 = mode").unwrap_or_default();
        let bare_hover =
            hover_markdown_for(&mut server, &main, source, "out2 = mode").unwrap_or_default();

        assert_ne!(
            qualified_definition,
            json!(null),
            "qualified namespace segment should resolve"
        );
        assert_eq!(qualified_definition["range"]["start"]["line"], json!(0));
        assert_ne!(
            bare_definition,
            json!(null),
            "bare param reference should resolve"
        );
        assert_eq!(bare_definition["range"]["start"]["line"], json!(5));
        assert!(
            qualified_hover.contains("namespace mode"),
            "hover: {qualified_hover}"
        );
        assert!(
            bare_hover.contains("proc param mode"),
            "hover: {bare_hover}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_qualified_namespace_segment_ignores_value_symbols() {
        let dir = mk_temp_dir("navigation_qualified_namespace_segment_ignores_values");
        let main = dir.join("main.onda");
        let source = concat!(
            "namespace mode:\n",
            "  const LOW = 0\n",
            "\n",
            "namespace outer:\n",
            "  const mode = 1\n",
            "\n",
            "  proc Filter:\n",
            "    params:\n",
            "      mode: i32 = 0\n",
            "\n",
            "    outs:\n",
            "      out1\n",
            "      out2\n",
            "\n",
            "    sample:\n",
            "      out1 = mode::LOW\n",
            "      out2 = mode\n",
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let qualified_definition = definition_for(&mut server, &main, source, "out1 = mode");
        let bare_definition = definition_for(&mut server, &main, source, "out2 = mode");
        let qualified_hover =
            hover_markdown_for(&mut server, &main, source, "out1 = mode").unwrap_or_default();

        assert_ne!(
            qualified_definition,
            json!(null),
            "qualified namespace segment should resolve"
        );
        assert_eq!(
            qualified_definition["range"]["start"]["line"],
            json!(0),
            "qualified segment should resolve to the namespace, not outer::mode const"
        );
        assert_ne!(
            bare_definition,
            json!(null),
            "bare param reference should resolve"
        );
        assert_eq!(bare_definition["range"]["start"]["line"], json!(8));
        assert!(
            qualified_hover.contains("namespace mode"),
            "hover: {qualified_hover}"
        );

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
    fn prelude_method_calls_support_hover_signature_help_and_definition() {
        let dir = mk_temp_dir("prelude_method_navigation");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = "buffers:\n  source: f32[]\nsample:\n  out1 = source.readL(0, 0.5)\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let hover = hover_markdown_for(&mut server, &main, source, "readL")
            .expect("prelude method hover should resolve");
        assert!(hover.contains("def readL("), "unexpected hover: {hover}");

        let signatures = request_with_position(
            &mut server,
            &main,
            source,
            "readL(0,",
            "textDocument/signatureHelp",
        );
        let labels = signatures["signatures"]
            .as_array()
            .expect("signature list")
            .iter()
            .filter_map(|signature| signature["label"].as_str())
            .collect::<Vec<_>>();
        assert!(
            labels.contains(&"def readL(buf, pos)"),
            "signatures: {labels:?}"
        );
        assert!(
            labels.contains(&"def readL(buf, ch: i32, pos)"),
            "signatures: {labels:?}"
        );
        assert_eq!(signatures["activeParameter"], json!(2));

        let definition = definition_for(&mut server, &main, source, "readL");
        assert_ne!(
            definition,
            json!(null),
            "prelude method goto should resolve"
        );
        let uri = definition["uri"].as_str().expect("stdlib definition uri");
        let path = file_uri_to_path(uri).expect("stdlib definition should be a file URI");
        assert!(
            path.starts_with(&cache),
            "unexpected target: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("lookup.onda")
        );

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
    fn definition_resolves_materialized_stdlib_proc_state_inside_local_def() {
        let dir = mk_temp_dir("definition_materialized_stdlib_proc_state_local_def");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/smoothing

init:
  lag = std::smoothing::Lag<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Lag");
        let uri = definition["uri"].as_str().expect("stdlib proc uri");
        let smoothing_path = file_uri_to_path(uri).expect("stdlib proc should be a file uri");
        let smoothing_source =
            fs::read_to_string(&smoothing_path).expect("read materialized smoothing");

        let needle = "\n      coef";
        let state_definition =
            definition_for(&mut server, &smoothing_path, &smoothing_source, needle);
        assert_ne!(
            state_definition,
            json!(null),
            "{needle:?} should resolve inside materialized stdlib local def"
        );
        assert_eq!(
            state_definition["range"]["start"]["line"],
            json!(15),
            "{needle:?} should goto the init declaration: {state_definition:?}"
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
    fn definition_resolves_receiver_members_and_hides_private_params() {
        let dir = mk_temp_dir("definition_receiver_members");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    private cutoff = 1000.0
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
            "private proc param should not resolve through receiver access"
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

    #[test]
    fn protocol_completion_lists_virtual_project_imports() {
        let mut session = LspSession::new();
        session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "processId": null, "capabilities": {} },
            }))
            .expect("initialize should succeed");
        for (uri, text) in [
            (
                "file:///onda-project/lib.onda",
                "namespace Lib:\n  const value = 1.0\n",
            ),
            ("file:///onda-project/main.onda", "import l\n"),
        ] {
            session
                .handle_message(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": "onda",
                            "version": 1,
                            "text": text,
                        }
                    },
                }))
                .expect("virtual document should open");
        }

        let messages = session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": "file:///onda-project/main.onda" },
                    "position": { "line": 0, "character": 8 },
                },
            }))
            .expect("completion should succeed");
        let items = messages[0]["result"]["items"]
            .as_array()
            .expect("completion items");
        assert!(
            items.iter().any(|item| item["label"] == json!("lib")),
            "virtual project module should be offered: {items:?}"
        );
    }

    #[test]
    fn protocol_serves_read_only_stdlib_virtual_documents() {
        let mut session = LspSession::new();
        let messages = session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "onda/virtualDocument",
                "params": { "uri": "onda-stdlib:///std/osc.onda" },
            }))
            .expect("virtual document request should succeed");
        let document = &messages[0]["result"];
        assert_eq!(document["path"], json!("std/osc.onda"));
        assert_eq!(document["languageId"], json!("onda"));
        assert_eq!(document["readOnly"], json!(true));
        assert!(
            document["text"]
                .as_str()
                .is_some_and(|source| source.contains("proc Saw")),
            "virtual document should contain the embedded std/osc source"
        );

        let source = document["text"].as_str().expect("stdlib source").to_owned();
        let position = position_after(&source, "Phasor");
        let messages = session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": "onda-stdlib:///std/osc.onda" },
                    "position": {
                        "line": position.line,
                        "character": position.character,
                    }
                },
            }))
            .expect("virtual stdlib definition request should succeed");
        let definition = &messages[0]["result"];
        assert_eq!(definition["uri"], json!("onda-stdlib:///std/osc.onda"));
        assert_eq!(definition["range"]["start"]["line"], json!(1));
    }
}
