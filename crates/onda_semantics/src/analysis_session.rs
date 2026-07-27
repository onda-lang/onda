use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use onda_frontend::{load_program_file_with_overlays, Diagnostic, Program, SourceManifest};

use crate::{analyze_with_options, AnalysisOptions, TypedProgram};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DocumentVersion(pub i32);

#[derive(Debug, Clone)]
pub struct OpenDocument {
    pub version: DocumentVersion,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct AnalysisSnapshot {
    pub path: PathBuf,
    pub version: Option<DocumentVersion>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: SourceManifest,
    pub parsed: Option<Program>,
    pub typed: Option<TypedProgram>,
}

impl AnalysisSnapshot {
    pub fn succeeded(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Browser-safe source overlay and analysis session shared by the LSP and daemon.
#[derive(Debug, Default)]
pub struct AnalysisSession {
    open_documents: HashMap<PathBuf, OpenDocument>,
}

impl AnalysisSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_document(
        &mut self,
        path: impl AsRef<Path>,
        version: DocumentVersion,
        text: impl Into<String>,
    ) -> PathBuf {
        let path = normalize_session_path(path.as_ref());
        self.open_documents.insert(
            path.clone(),
            OpenDocument {
                version,
                text: text.into(),
            },
        );
        path
    }

    pub fn update_document(
        &mut self,
        path: impl AsRef<Path>,
        version: DocumentVersion,
        text: impl Into<String>,
    ) -> PathBuf {
        self.open_document(path, version, text)
    }

    pub fn close_document(&mut self, path: impl AsRef<Path>) -> Option<OpenDocument> {
        let path = normalize_session_path(path.as_ref());
        self.open_documents.remove(&path)
    }

    pub fn document(&self, path: impl AsRef<Path>) -> Option<&OpenDocument> {
        let path = normalize_session_path(path.as_ref());
        self.open_documents.get(&path)
    }

    pub fn open_documents(&self) -> &HashMap<PathBuf, OpenDocument> {
        &self.open_documents
    }

    pub fn analyze_document(
        &self,
        path: impl AsRef<Path>,
        options: AnalysisOptions,
    ) -> AnalysisSnapshot {
        let path = normalize_session_path(path.as_ref());
        let version = self
            .open_documents
            .get(&path)
            .map(|document| document.version);
        let overlays = self.overlay_map();
        let loaded = match load_program_file_with_overlays(&path, &overlays) {
            Ok(loaded) => loaded,
            Err(error) => {
                return AnalysisSnapshot {
                    path,
                    version,
                    diagnostics: error.diagnostics,
                    sources: error.sources,
                    parsed: None,
                    typed: None,
                };
            }
        };
        let sources = loaded.sources;
        let parsed = loaded.program;

        match analyze_with_options(parsed.clone(), options) {
            Ok(typed) => AnalysisSnapshot {
                path,
                version,
                diagnostics: Vec::new(),
                sources,
                parsed: Some(parsed),
                typed: Some(typed),
            },
            Err(diagnostics) => AnalysisSnapshot {
                path,
                version,
                diagnostics,
                sources,
                parsed: Some(parsed),
                typed: None,
            },
        }
    }

    pub fn overlay_map(&self) -> HashMap<PathBuf, String> {
        self.open_documents
            .iter()
            .map(|(path, document)| (path.clone(), document.text.clone()))
            .collect()
    }
}

pub fn normalize_session_path(path: &Path) -> PathBuf {
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
