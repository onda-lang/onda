mod analysis_session;
mod preview_session;

pub use analysis_session::{AnalysisSession, AnalysisSnapshot, DocumentVersion, OpenDocument};
pub use preview_session::{PreviewBuildError, PreviewOptions, PreviewParamInfo, PreviewSession};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use omni_frontend::Diagnostic;
use omni_semantics::AnalysisOptions;

use crate::analysis_session::normalize_session_path;

#[derive(Debug, Clone, Copy)]
pub struct DaemonConfig {
    pub analysis: AnalysisOptions,
    pub preview: PreviewOptions,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            analysis: AnalysisOptions::default(),
            preview: PreviewOptions::default(),
        }
    }
}

#[derive(Debug, Default)]
pub struct DaemonSession {
    config: DaemonConfig,
    analysis: AnalysisSession,
    previews: HashMap<PathBuf, PreviewSession>,
}

impl DaemonSession {
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            analysis: AnalysisSession::new(),
            previews: HashMap::new(),
        }
    }

    pub fn config(&self) -> DaemonConfig {
        self.config
    }

    pub fn set_config(&mut self, config: DaemonConfig) {
        self.config = config;
        self.previews.clear();
    }

    pub fn set_analysis_options(&mut self, options: AnalysisOptions) {
        self.config.analysis = options;
    }

    pub fn set_preview_options(&mut self, options: PreviewOptions) {
        self.config.preview = options;
        self.previews.clear();
    }

    pub fn analysis(&self) -> &AnalysisSession {
        &self.analysis
    }

    pub fn analysis_mut(&mut self) -> &mut AnalysisSession {
        &mut self.analysis
    }

    pub fn open_document(
        &mut self,
        path: impl AsRef<Path>,
        version: DocumentVersion,
        text: impl Into<String>,
    ) -> PathBuf {
        self.analysis.open_document(path, version, text)
    }

    pub fn update_document(
        &mut self,
        path: impl AsRef<Path>,
        version: DocumentVersion,
        text: impl Into<String>,
    ) -> PathBuf {
        self.analysis.update_document(path, version, text)
    }

    pub fn close_document(&mut self, path: impl AsRef<Path>) -> Option<OpenDocument> {
        let normalized = normalize_session_path(path.as_ref());
        self.previews.remove(&normalized);
        self.analysis.close_document(normalized)
    }

    pub fn analyze_document(&self, path: impl AsRef<Path>) -> AnalysisSnapshot {
        self.analysis.analyze_document(path, self.config.analysis)
    }

    pub fn start_preview(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<&PreviewSession, PreviewBuildError> {
        self.start_preview_with_options(path, self.config.preview)
    }

    pub fn start_preview_with_options(
        &mut self,
        path: impl AsRef<Path>,
        options: PreviewOptions,
    ) -> Result<&PreviewSession, PreviewBuildError> {
        let normalized = normalize_session_path(path.as_ref());
        let preview = PreviewSession::build(&self.analysis, &normalized, options)?;
        self.previews.insert(normalized.clone(), preview);
        Ok(self
            .previews
            .get(&normalized)
            .expect("preview inserted into session"))
    }

    pub fn rebuild_preview(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<&PreviewSession, PreviewBuildError> {
        let normalized = normalize_session_path(path.as_ref());
        let options = self
            .previews
            .get(&normalized)
            .map(|preview| preview.options())
            .unwrap_or(self.config.preview);
        self.start_preview_with_options(normalized, options)
    }

    pub fn preview(&self, path: impl AsRef<Path>) -> Option<&PreviewSession> {
        let normalized = normalize_session_path(path.as_ref());
        self.previews.get(&normalized)
    }

    pub fn preview_mut(&mut self, path: impl AsRef<Path>) -> Option<&mut PreviewSession> {
        let normalized = normalize_session_path(path.as_ref());
        self.previews.get_mut(&normalized)
    }

    pub fn stop_preview(&mut self, path: impl AsRef<Path>) -> Option<PreviewSession> {
        let normalized = normalize_session_path(path.as_ref());
        self.previews.remove(&normalized)
    }

    pub fn render_preview_block(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Vec<Vec<f32>>, Diagnostic> {
        let normalized = normalize_session_path(path.as_ref());
        let preview = self.previews.get_mut(&normalized).ok_or_else(|| {
            Diagnostic::runtime(
                format!("preview is not active for '{}'", normalized.display()),
                0,
                0,
            )
        })?;
        preview.render_block()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("omni_daemon_{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, text).expect("write test file");
    }

    #[test]
    fn analyze_document_uses_entry_overlay_contents() {
        let dir = mk_temp_dir("entry_overlay");
        let main = dir.join("main.omni");
        let lib = dir.join("lib.omni");

        write_file(&main, "outs { out1 }\nsample { out1 = 0.0 }\n");
        write_file(&lib, "def gain(x) { return x * 0.5 }\n");

        let mut session = DaemonSession::default();
        session.open_document(
            &main,
            DocumentVersion(7),
            "import lib\nouts { out1 }\nsample { out1 = gain(1.0) }\n",
        );

        let snapshot = session.analyze_document(&main);
        assert!(snapshot.succeeded(), "expected success, got {:?}", snapshot);
        assert_eq!(snapshot.version, Some(DocumentVersion(7)));
        assert!(snapshot.typed.is_some());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn analyze_document_uses_overlay_dependency_contents() {
        let dir = mk_temp_dir("dependency_overlay");
        let main = dir.join("main.omni");
        let lib = dir.join("lib.omni");

        write_file(
            &main,
            "import lib\nouts { out1 }\nsample { out1 = gain(SCALE) }\n",
        );
        write_file(&lib, "const SCALE = invalid\n");

        let mut session = DaemonSession::default();
        session.open_document(
            &lib,
            DocumentVersion(3),
            "const SCALE = 0.5\ndef gain(x) { return x }\n",
        );

        let snapshot = session.analyze_document(&main);
        assert!(snapshot.succeeded(), "expected success, got {:?}", snapshot);
        assert!(snapshot.typed.is_some());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn close_document_reverts_to_disk_contents() {
        let dir = mk_temp_dir("close_reverts_to_disk");
        let main = dir.join("main.omni");

        write_file(&main, "outs { out1 }\nsample { out1 = 0.0 }\n");

        let mut session = DaemonSession::default();
        session.open_document(
            &main,
            DocumentVersion(1),
            "outs { out1 }\nsample { out1 = missing }\n",
        );

        let with_overlay = session.analyze_document(&main);
        assert!(with_overlay
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("unknown symbol 'missing'")));

        session.close_document(&main);
        let from_disk = session.analyze_document(&main);
        assert!(from_disk.succeeded(), "expected disk analysis success");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_renders_and_updates_scalar_param() {
        let dir = mk_temp_dir("preview_render");
        let main = dir.join("main.omni");

        write_file(
            &main,
            "outs:\n  out1\nparams:\n  gain = 0.25 {0.0, 1.0}\nsample:\n  out1 = gain\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_preview(&main)
            .expect("preview should compile and start");

        let param_info = session.preview(&main).expect("active preview").param_info();
        assert_eq!(param_info.len(), 1);
        assert_eq!(param_info[0].name, "gain");
        assert_eq!(param_info[0].default, Some(0.25));

        let first = session
            .render_preview_block(&main)
            .expect("first preview render should succeed");
        assert_eq!(first.len(), 1);
        assert!(first[0].iter().all(|sample| (*sample - 0.25).abs() < 1e-6));

        session
            .preview_mut(&main)
            .expect("active preview")
            .set_param_f64("gain", 0.5)
            .expect("param update should succeed");
        let second = session
            .render_preview_block(&main)
            .expect("second preview render should succeed");
        assert!(second[0].iter().all(|sample| (*sample - 0.5).abs() < 1e-6));

        fs::remove_dir_all(&dir).ok();
    }
}
