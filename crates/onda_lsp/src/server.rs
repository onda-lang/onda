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
    encode_semantic_tokens, semantic_token_legend, semantic_token_modifier_legend,
    semantic_tokens_for_document_source_only, semantic_tokens_for_document_with_parsed,
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
                    "tokenModifiers": semantic_token_modifier_legend(),
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
#[path = "server/tests/mod.rs"]
mod tests;
