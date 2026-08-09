use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use onda_frontend::{load_program_file_with_overlays, Diagnostic, Program, SourceManifest};

use crate::{
    analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions, MirLoweringError,
    TypedProgram,
};

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
    pub mir: Option<onda_mir::OptimizedProgram>,
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
                    mir: None,
                };
            }
        };
        let sources = loaded.sources;
        let parsed = loaded.program;

        match analyze_with_options(parsed.clone(), options) {
            Ok(typed) => match lower_program_to_optimized_mir(&typed) {
                Ok(mir) => AnalysisSnapshot {
                    path,
                    version,
                    diagnostics: Vec::new(),
                    sources,
                    parsed: Some(parsed),
                    typed: Some(typed),
                    mir: Some(mir),
                },
                Err(errors) => AnalysisSnapshot {
                    path,
                    version,
                    diagnostics: errors.into_iter().map(mir_lowering_diagnostic).collect(),
                    sources,
                    parsed: Some(parsed),
                    typed: None,
                    mir: None,
                },
            },
            Err(diagnostics) => AnalysisSnapshot {
                path,
                version,
                diagnostics: diagnostics
                    .into_iter()
                    .map(normalize_editor_diagnostic)
                    .collect(),
                sources,
                parsed: Some(parsed),
                typed: None,
                mir: None,
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

fn normalize_editor_diagnostic(mut diagnostic: Diagnostic) -> Diagnostic {
    if diagnostic
        .file
        .as_deref()
        .is_some_and(|file| file.starts_with('<'))
    {
        diagnostic.line = 0;
        diagnostic.column = 0;
        diagnostic.end_line = 0;
        diagnostic.end_column = 0;
        diagnostic.file = None;
    }
    diagnostic
}

fn mir_lowering_diagnostic(error: MirLoweringError) -> Diagnostic {
    let message = format!("MIR lowering failed: {}", error.message);
    if error.location.is_zero()
        || error
            .location
            .file()
            .is_some_and(|file| file.starts_with('<'))
    {
        Diagnostic::internal(message)
    } else {
        Diagnostic::internal_at(message, &error.location)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mir_diagnostics_preserve_user_source_locations() {
        let location = onda_frontend::SourceLoc::new(
            Some("/tmp/main.onda".to_owned()),
            7,
            11,
            7,
            15,
            Vec::new(),
        );
        let diagnostic = mir_lowering_diagnostic(MirLoweringError {
            message: "invalid generated call".to_owned(),
            location,
        });

        assert_eq!(diagnostic.code, onda_frontend::DiagCode::Internal);
        assert_eq!(diagnostic.line, 7);
        assert_eq!(diagnostic.column, 11);
        assert_eq!(diagnostic.file.as_deref(), Some("/tmp/main.onda"));
        assert!(diagnostic.message.contains("MIR lowering failed"));
    }

    #[test]
    fn mir_diagnostics_do_not_point_into_embedded_stdlib_sources() {
        let location = onda_frontend::SourceLoc::new(
            Some("<std/lookup.onda>".to_owned()),
            35,
            23,
            35,
            26,
            Vec::new(),
        );
        let diagnostic = mir_lowering_diagnostic(MirLoweringError {
            message: "invalid generated call".to_owned(),
            location,
        });

        assert_eq!(diagnostic.line, 0);
        assert_eq!(diagnostic.column, 0);
        assert_eq!(diagnostic.file, None);
    }

    #[test]
    fn semantic_diagnostics_do_not_point_into_embedded_stdlib_sources() {
        let diagnostic = normalize_editor_diagnostic(Diagnostic::semantic_ctx(
            "invalid specialization",
            17,
            17,
            onda_frontend::DiagCtx::new(onda_frontend::SourceLoc::new(
                Some("<std/lookup.onda>".to_owned()),
                17,
                17,
                17,
                27,
                Vec::new(),
            )),
        ));

        assert_eq!(diagnostic.line, 0);
        assert_eq!(diagnostic.column, 0);
        assert_eq!(diagnostic.file, None);
    }
}
