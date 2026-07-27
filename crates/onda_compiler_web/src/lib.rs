#![deny(clippy::all)]

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use onda_frontend::{
    load_program_file_from_virtual_sources, parse_program, DiagCode, Diagnostic, Program,
    SourceManifest,
};
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};
use serde::Serialize;

pub const MIR_SCHEMA_VERSION: u32 = onda_mir::MIR_SCHEMA_VERSION;

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct CompilerDiagnostic {
    pub stage: &'static str,
    pub code: u16,
    pub message: String,
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct CompilerFailure {
    pub diagnostics: Vec<CompilerDiagnostic>,
    pub source_files: Vec<String>,
    pub unresolved_source_files: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CompilationOutput<T> {
    pub output: T,
    pub source_files: Vec<String>,
}

impl CompilerFailure {
    fn without_sources(diagnostics: Vec<CompilerDiagnostic>) -> Self {
        Self {
            diagnostics,
            source_files: Vec::new(),
            unresolved_source_files: Vec::new(),
        }
    }

    fn with_sources(diagnostics: Vec<CompilerDiagnostic>, source_files: Vec<String>) -> Self {
        Self {
            diagnostics,
            source_files,
            unresolved_source_files: Vec::new(),
        }
    }

    fn with_source_manifest(
        diagnostics: Vec<CompilerDiagnostic>,
        source_files: Vec<String>,
        unresolved_source_files: Vec<String>,
    ) -> Self {
        Self {
            diagnostics,
            source_files,
            unresolved_source_files,
        }
    }
}

impl CompilerDiagnostic {
    fn configuration(message: impl Into<String>) -> Self {
        Self {
            stage: "configuration",
            code: DiagCode::Semantic as u16,
            message: message.into(),
            file: None,
            line: 0,
            column: 0,
            end_line: 0,
            end_column: 0,
            trace: Vec::new(),
        }
    }

    fn source(stage: &'static str, diagnostic: Diagnostic) -> Self {
        Self {
            stage,
            code: diagnostic.code as u16,
            message: diagnostic.message,
            file: diagnostic.file,
            line: diagnostic.line,
            column: diagnostic.column,
            end_line: diagnostic.end_line,
            end_column: diagnostic.end_column,
            trace: diagnostic.trace,
        }
    }
}

/// Compiles one in-memory Onda source file to validated, versioned MIR JSON.
///
/// Built-in `std/...` modules are embedded by `onda_frontend`, so this path
/// does not require filesystem access and is suitable for a browser compiler.
pub fn compile_source_to_mir_json(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<String, Vec<CompilerDiagnostic>> {
    compile_source_to_mir_json_with_manifest(source, sample_rate, block_size)
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

/// Compiles source to the compact MessagePack MIR transport used by browser
/// backends. JSON remains available for diagnostics and external tooling.
pub fn compile_source_to_mir_messagepack(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<Vec<u8>, Vec<CompilerDiagnostic>> {
    compile_source_to_mir_messagepack_with_manifest(source, sample_rate, block_size)
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

/// Compiles an in-memory multi-file project without consulting the host
/// filesystem. Paths are project-relative and imports/includes resolve only
/// against `sources` or the embedded standard library.
pub fn compile_project_sources_to_mir_json(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<String, Vec<CompilerDiagnostic>> {
    compile_project_sources_to_mir_json_with_manifest(entry_path, sources, sample_rate, block_size)
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

pub fn compile_project_sources_to_mir_messagepack(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<Vec<u8>, Vec<CompilerDiagnostic>> {
    compile_project_sources_to_mir_messagepack_with_manifest(
        entry_path,
        sources,
        sample_rate,
        block_size,
    )
    .map(|compiled| compiled.output)
    .map_err(|failure| failure.diagnostics)
}

pub fn compile_source_to_mir_json_with_manifest(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<String>, CompilerFailure> {
    let compiled = lower_source_to_mir_with_manifest(source, sample_rate, block_size)?;
    encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized)
}

pub fn compile_source_to_mir_messagepack_with_manifest(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    let compiled = lower_source_to_mir_with_manifest(source, sample_rate, block_size)?;
    encode_mir_compilation(
        compiled,
        "mir-messagepack",
        onda_mir::to_messagepack_optimized,
    )
}

pub fn compile_project_sources_to_mir_json_with_manifest(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<String>, CompilerFailure> {
    let compiled =
        lower_project_sources_to_mir_with_manifest(entry_path, sources, sample_rate, block_size)?;
    encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized)
}

pub fn compile_project_sources_to_mir_messagepack_with_manifest(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    let compiled =
        lower_project_sources_to_mir_with_manifest(entry_path, sources, sample_rate, block_size)?;
    encode_mir_compilation(
        compiled,
        "mir-messagepack",
        onda_mir::to_messagepack_optimized,
    )
}

fn encode_mir_compilation<T, E>(
    compiled: CompilationOutput<onda_mir::OptimizedProgram>,
    stage: &'static str,
    encode: impl FnOnce(&onda_mir::OptimizedProgram) -> Result<T, E>,
) -> Result<CompilationOutput<T>, CompilerFailure>
where
    E: ToString,
{
    let output = encode(&compiled.output).map_err(|error| {
        CompilerFailure::with_sources(
            mir_encoding_error(stage, error),
            compiled.source_files.clone(),
        )
    })?;
    Ok(CompilationOutput {
        output,
        source_files: compiled.source_files,
    })
}

fn lower_source_to_mir_with_manifest(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let parsed = parse_program(source).map_err(|diagnostics| {
        CompilerFailure::without_sources(
            diagnostics
                .into_iter()
                .map(|diagnostic| CompilerDiagnostic::source("parse", diagnostic))
                .collect::<Vec<_>>(),
        )
    })?;
    let output = lower_parsed_program(parsed, config).map_err(CompilerFailure::without_sources)?;
    Ok(CompilationOutput {
        output,
        source_files: Vec::new(),
    })
}

fn lower_project_sources_to_mir_with_manifest(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    // This is a logical namespace rather than a host filesystem path. Keeping
    // it relative avoids target-specific `Path::is_absolute` behavior on
    // `wasm32-unknown-unknown` while the virtual loader still confines every
    // lookup beneath the namespace.
    let root = PathBuf::from("onda-project");
    let mut overlays = HashMap::with_capacity(sources.len());
    for (path, source) in sources {
        let path = checked_project_path(path).map_err(CompilerFailure::without_sources)?;
        let full_path = root.join(path);
        if overlays.insert(full_path, source.clone()).is_some() {
            return Err(CompilerFailure::without_sources(vec![
                CompilerDiagnostic::configuration(
                    "project contains duplicate normalized source paths",
                ),
            ]));
        }
    }
    let entry_path =
        root.join(checked_project_path(entry_path).map_err(CompilerFailure::without_sources)?);
    if !overlays.contains_key(&entry_path) {
        return Err(CompilerFailure::without_sources(vec![
            CompilerDiagnostic::configuration(format!(
                "project entry '{entry_path}' is not present in the source map",
                entry_path = entry_path.display()
            )),
        ]));
    }
    let loaded =
        load_program_file_from_virtual_sources(&root, &entry_path, &overlays).map_err(|error| {
            let source_files = virtual_source_files(&root, &error.sources);
            let unresolved_source_files = virtual_paths(&root, &error.sources.unresolved_files);
            let diagnostics = error
                .diagnostics
                .into_iter()
                .map(|diagnostic| CompilerDiagnostic::source("parse", diagnostic))
                .collect::<Vec<_>>();
            CompilerFailure::with_source_manifest(
                diagnostics,
                source_files,
                unresolved_source_files,
            )
        })?;
    let source_files = virtual_source_files(&root, &loaded.sources);
    let output = lower_parsed_program(loaded.program, config)
        .map_err(|diagnostics| CompilerFailure::with_sources(diagnostics, source_files.clone()))?;
    Ok(CompilationOutput {
        output,
        source_files,
    })
}

fn virtual_source_files(root: &Path, manifest: &SourceManifest) -> Vec<String> {
    virtual_paths(root, &manifest.files)
}

fn virtual_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.strip_prefix(root).unwrap_or(path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn checked_project_path(path: &str) -> Result<PathBuf, Vec<CompilerDiagnostic>> {
    let portable_absolute = path.starts_with('/')
        || path.starts_with('\\')
        || matches!(path.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    let path = Path::new(path);
    if path.as_os_str().is_empty() || portable_absolute || path.is_absolute() {
        return Err(vec![CompilerDiagnostic::configuration(
            "project source paths must be non-empty and relative",
        )]);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => normalized.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(vec![CompilerDiagnostic::configuration(format!(
                    "project source path '{}' escapes the virtual project root",
                    path.display()
                ))]);
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(vec![CompilerDiagnostic::configuration(
            "project source paths must identify a file",
        )]);
    }
    Ok(normalized)
}

fn lower_parsed_program(
    parsed: Program,
    config: onda_mir::CompileConfig,
) -> Result<onda_mir::OptimizedProgram, Vec<CompilerDiagnostic>> {
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: config.sample_rate,
            block_size: config.block_size as usize,
        },
    )
    .map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| CompilerDiagnostic::source("semantic", diagnostic))
            .collect::<Vec<_>>()
    })?;
    lower_program_to_optimized_mir(&typed).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| CompilerDiagnostic {
                stage: "mir",
                code: DiagCode::Internal as u16,
                message: error.message,
                file: error.location.file(),
                line: error.location.line,
                column: error.location.column,
                end_line: error.location.end_line,
                end_column: error.location.end_column,
                trace: error.location.trace(),
            })
            .collect::<Vec<_>>()
    })
}

fn compile_config(
    sample_rate: f32,
    block_size: u32,
) -> Result<onda_mir::CompileConfig, Vec<CompilerDiagnostic>> {
    onda_mir::CompileConfig::new(sample_rate, block_size)
        .map_err(|error| vec![CompilerDiagnostic::configuration(error.to_string())])
}

fn mir_encoding_error(stage: &'static str, error: impl ToString) -> Vec<CompilerDiagnostic> {
    vec![CompilerDiagnostic {
        stage,
        code: DiagCode::Internal as u16,
        message: error.to_string(),
        file: None,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        trace: Vec::new(),
    }]
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn mir_schema_version() -> u32 {
    MIR_SCHEMA_VERSION
}

/// Stateful `onda lsp` server for browser Worker transports.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct OndaLsp {
    session: onda_lsp::LspSession,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl OndaLsp {
    #[wasm_bindgen::prelude::wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            session: onda_lsp::LspSession::new(),
        }
    }

    pub fn set_analysis_options(
        &mut self,
        sample_rate: f32,
        block_size: u32,
    ) -> Result<(), wasm_bindgen::JsValue> {
        let config = onda_mir::CompileConfig::new(sample_rate, block_size)
            .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))?;
        self.session.set_analysis_options(AnalysisOptions {
            sample_rate: config.sample_rate,
            block_size: config.block_size as usize,
        });
        Ok(())
    }

    /// Accepts one JSON-RPC LSP message and returns a JSON array containing
    /// all responses and notifications emitted synchronously for it.
    pub fn handle_message(&mut self, message_json: &str) -> Result<String, wasm_bindgen::JsValue> {
        self.session
            .handle_message_json(message_json)
            .map_err(|error| wasm_bindgen::JsValue::from_str(&error))
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for OndaLsp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct FrontendMessagePackCompilation {
    mir: Vec<u8>,
    source_files_json: String,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl FrontendMessagePackCompilation {
    pub fn take_mir(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.mir)
    }

    pub fn source_files_json(&self) -> String {
        self.source_files_json.clone()
    }
}

#[cfg(target_arch = "wasm32")]
fn frontend_messagepack_compilation(
    compiled: CompilationOutput<Vec<u8>>,
) -> FrontendMessagePackCompilation {
    FrontendMessagePackCompilation {
        mir: compiled.output,
        source_files_json: encode_source_files(&compiled.source_files),
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct FrontendJsonCompilation {
    mir: String,
    source_files_json: String,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl FrontendJsonCompilation {
    pub fn take_mir(&mut self) -> String {
        std::mem::take(&mut self.mir)
    }

    pub fn source_files_json(&self) -> String {
        self.source_files_json.clone()
    }
}

#[cfg(target_arch = "wasm32")]
fn frontend_json_compilation(compiled: CompilationOutput<String>) -> FrontendJsonCompilation {
    FrontendJsonCompilation {
        mir: compiled.output,
        source_files_json: encode_source_files(&compiled.source_files),
    }
}

#[cfg(target_arch = "wasm32")]
fn encode_source_files(source_files: &[String]) -> String {
    serde_json::to_string(source_files).unwrap_or_else(|_| "[]".to_owned())
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_to_mir_json(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<FrontendJsonCompilation, wasm_bindgen::JsValue> {
    compile_source_to_mir_json_with_manifest(source, sample_rate, block_size)
        .map(frontend_json_compilation)
        .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_to_mir_messagepack(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<FrontendMessagePackCompilation, wasm_bindgen::JsValue> {
    compile_source_to_mir_messagepack_with_manifest(source, sample_rate, block_size)
        .map(frontend_messagepack_compilation)
        .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_project_to_mir_json(
    entry_path: &str,
    sources_json: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<FrontendJsonCompilation, wasm_bindgen::JsValue> {
    let sources = decode_project_sources_json(sources_json)?;
    compile_project_sources_to_mir_json_with_manifest(entry_path, &sources, sample_rate, block_size)
        .map(frontend_json_compilation)
        .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_project_to_mir_messagepack(
    entry_path: &str,
    sources_json: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<FrontendMessagePackCompilation, wasm_bindgen::JsValue> {
    let sources = decode_project_sources_json(sources_json)?;
    compile_project_sources_to_mir_messagepack_with_manifest(
        entry_path,
        &sources,
        sample_rate,
        block_size,
    )
    .map(frontend_messagepack_compilation)
    .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
fn decode_project_sources_json(
    sources_json: &str,
) -> Result<HashMap<String, String>, wasm_bindgen::JsValue> {
    serde_json::from_str(sources_json).map_err(|error| {
        compiler_failure_js(CompilerFailure::without_sources(vec![
            CompilerDiagnostic::configuration(format!("invalid project source map JSON: {error}")),
        ]))
    })
}

#[cfg(target_arch = "wasm32")]
fn compiler_failure_js(failure: CompilerFailure) -> wasm_bindgen::JsValue {
    let encoded = serde_json::to_string(&failure).unwrap_or_else(|_| {
        "{\"diagnostics\":[{\"stage\":\"internal\",\"message\":\"failed to encode compiler diagnostics\"}],\"source_files\":[],\"unresolved_source_files\":[]}".to_owned()
    });
    wasm_bindgen::JsValue::from_str(&encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onda_sources_below(root: &Path) -> Vec<PathBuf> {
        fn visit(directory: &Path, out: &mut Vec<PathBuf>) {
            let mut entries = std::fs::read_dir(directory)
                .expect("example directory should be readable")
                .map(|entry| entry.expect("example entry should be readable").path())
                .collect::<Vec<_>>();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    visit(&path, out);
                } else if path.extension().and_then(|value| value.to_str()) == Some("onda") {
                    out.push(path);
                }
            }
        }

        let mut out = Vec::new();
        visit(root, &mut out);
        out
    }

    #[test]
    fn compiles_in_memory_source_to_valid_deterministic_mir() {
        let source = r#"
params:
  gain = 0.25
outs:
  out1
init:
  phase = 0.0
sample:
  phase = phase + gain
  out1 = phase
"#;
        let first = compile_source_to_mir_json(source, 48_000.0, 128)
            .expect("source should compile to MIR JSON");
        let second = compile_source_to_mir_json(source, 48_000.0, 128)
            .expect("source should compile deterministically");
        assert_eq!(first, second);
        let mir = unsafe { onda_mir::from_json_with_producer_proofs(&first) }
            .expect("JSON should decode as trusted producer MIR");
        assert_eq!(mir.config.sample_rate, 48_000.0);
        assert_eq!(mir.config.block_size, 128);

        let packed = compile_source_to_mir_messagepack(source, 48_000.0, 128)
            .expect("source should compile to MessagePack MIR");
        assert!(packed.len() < first.len());
        let packed_mir = unsafe { onda_mir::from_messagepack_with_producer_proofs(&packed) }
            .expect("MessagePack should decode as trusted producer MIR");
        assert_eq!(packed_mir.as_program(), mir.as_program());
    }

    #[test]
    fn compiles_embedded_standard_library_modules_without_a_filesystem() {
        let source = r#"
import std/osc

outs:
  out1
init:
  oscillator = std::osc::Sine()
sample:
  out1 = oscillator()
"#;
        let json = compile_source_to_mir_json(source, 48_000.0, 128)
            .expect("embedded standard library source should compile");
        unsafe { onda_mir::from_json_with_producer_proofs(&json) }
            .expect("standard-library result should be valid producer MIR");
    }

    #[test]
    fn compiles_a_multi_file_virtual_project_without_disk_files() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "import dsp\nouts 1\nsample:\n  out1 = DSP::value()\n".to_owned(),
            ),
            (
                "dsp.onda".to_owned(),
                "namespace DSP:\n  def value() -> f32:\n    return 0.75\n".to_owned(),
            ),
        ]);
        let json = compile_project_sources_to_mir_json("main.onda", &sources, 48_000.0, 128)
            .expect("virtual import should compile without filesystem access");
        unsafe { onda_mir::from_json_with_producer_proofs(&json) }
            .expect("virtual project result should be valid producer MIR");
    }

    #[test]
    fn project_compilation_returns_only_contributing_virtual_sources() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "include \"./shared.onda\"\nimport dsp/filter\nimport std/math\nouts 1\nsample:\n  out1 = DSP::value()\n"
                    .to_owned(),
            ),
            (
                "shared.onda".to_owned(),
                "const shared = 0.25\n".to_owned(),
            ),
            (
                "dsp/filter.onda".to_owned(),
                "namespace DSP:\n  def value() -> f32:\n    return 0.75\n".to_owned(),
            ),
            (
                "unused.onda".to_owned(),
                "const unused = 1.0\n".to_owned(),
            ),
        ]);
        let compiled_messagepack = compile_project_sources_to_mir_messagepack_with_manifest(
            "main.onda",
            &sources,
            48_000.0,
            128,
        )
        .expect("virtual project should compile");
        assert_eq!(
            compiled_messagepack.source_files,
            vec!["main.onda", "shared.onda", "dsp/filter.onda"]
        );

        let compiled_json =
            compile_project_sources_to_mir_json_with_manifest("main.onda", &sources, 48_000.0, 128)
                .expect("virtual project should compile to JSON");
        assert_eq!(
            compiled_json.source_files,
            vec!["main.onda", "shared.onda", "dsp/filter.onda"]
        );
        unsafe { onda_mir::from_json_with_producer_proofs(&compiled_json.output) }
            .expect("manifest-bearing JSON result should be valid producer MIR");
    }

    #[test]
    fn failed_project_compilation_returns_partial_source_manifest() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "import dsp\nouts 1\nsample:\n  out1 = 0.0\n".to_owned(),
            ),
            ("dsp.onda".to_owned(), "this is not valid onda\n".to_owned()),
        ]);
        let failure = compile_project_sources_to_mir_messagepack_with_manifest(
            "main.onda",
            &sources,
            48_000.0,
            128,
        )
        .expect_err("dependency should fail to parse");
        assert_eq!(failure.source_files, vec!["main.onda", "dsp.onda"]);
        assert!(failure.unresolved_source_files.is_empty());
        assert!(!failure.diagnostics.is_empty());
    }

    #[test]
    fn failed_project_compilation_returns_unresolved_source_candidates() {
        let sources = HashMap::from([("main.onda".to_owned(), "import dsp/filter\n".to_owned())]);
        let failure = compile_project_sources_to_mir_messagepack_with_manifest(
            "main.onda",
            &sources,
            48_000.0,
            128,
        )
        .expect_err("missing dependency should fail");
        assert_eq!(failure.source_files, vec!["main.onda"]);
        assert_eq!(
            failure.unresolved_source_files,
            vec!["dsp/filter.onda", "dsp/filter.on"]
        );
    }

    #[test]
    fn rejects_project_paths_that_escape_the_virtual_root() {
        let sources = HashMap::from([("../main.onda".to_owned(), String::new())]);
        let errors = compile_project_sources_to_mir_json("../main.onda", &sources, 48_000.0, 128)
            .expect_err("escaping project paths should fail");
        assert_eq!(errors[0].stage, "configuration");
    }

    #[test]
    fn rejects_nested_imports_and_includes_that_escape_the_virtual_root() {
        for source in [
            "include \"../outside.onda\"\n",
            "include \"/tmp/outside.onda\"\n",
            "import ../outside\n",
        ] {
            let sources = HashMap::from([("main.onda".to_owned(), source.to_owned())]);
            let errors = compile_project_sources_to_mir_json("main.onda", &sources, 48_000.0, 128)
                .expect_err("nested virtual path escape should fail");
            assert_eq!(errors[0].stage, "parse");
            assert!(errors[0].message.contains("escapes project root"));
        }
    }

    #[test]
    fn returns_structured_source_diagnostics() {
        let errors = compile_source_to_mir_json("sample:\n  out1 = missing\n", 48_000.0, 128)
            .expect_err("invalid source should fail");
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|diagnostic| diagnostic.stage == "semantic"));
        assert!(errors.iter().any(|diagnostic| diagnostic.line > 0));
    }

    #[test]
    fn rejects_invalid_host_configuration() {
        let errors = compile_source_to_mir_json("", f32::NAN, 0)
            .expect_err("invalid configuration should fail before parsing");
        assert_eq!(errors[0].stage, "configuration");

        let errors = compile_source_to_mir_json(
            "this source must not be parsed",
            48_000.0,
            i32::MAX as u32 + 1,
        )
        .expect_err("oversized blocks should fail before parsing");
        assert_eq!(errors[0].stage, "configuration");
        assert!(errors[0].message.contains("2147483647"));
    }

    #[test]
    fn browser_front_half_compiles_the_checked_in_example_corpus() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let paths = onda_sources_below(&examples);
        assert_eq!(paths.len(), 46, "example corpus count changed");
        let sources = paths
            .iter()
            .map(|path| {
                let relative = path
                    .strip_prefix(&examples)
                    .expect("example should be below corpus root")
                    .to_string_lossy()
                    .into_owned();
                let source =
                    std::fs::read_to_string(path).expect("example source should be readable");
                (relative, source)
            })
            .collect::<HashMap<_, _>>();
        let mut failures = Vec::new();
        for path in paths {
            let entry = path
                .strip_prefix(&examples)
                .expect("example should be below corpus root")
                .to_string_lossy();
            if let Err(errors) =
                compile_project_sources_to_mir_json(&entry, &sources, 48_000.0, 512)
            {
                failures.push(format!("{}: {errors:?}", path.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "browser compiler corpus failures:\n{}",
            failures.join("\n")
        );
    }
}
