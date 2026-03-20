mod analysis_session;
mod preview_session;

pub use analysis_session::{AnalysisSession, AnalysisSnapshot, DocumentVersion, OpenDocument};
pub use preview_session::{
    PreviewBufferChannels, PreviewBufferInfo, PreviewBuildError, PreviewEventInfo,
    PreviewEventParamInfo, PreviewEventValue, PreviewOptions, PreviewParamInfo, PreviewSession,
};

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

    fn write_wav(path: &Path, channels: u16, sample_rate: u32, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for sample in samples {
            writer.write_sample(*sample).expect("write wav sample");
        }
        writer.finalize().expect("finalize wav");
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
            .start_preview_with_options(
                &main,
                PreviewOptions {
                    float_param_smoothing_ms: 0.0,
                    ..PreviewOptions::default()
                },
            )
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

    #[test]
    fn preview_binds_wav_file_to_f32_buffer() {
        let dir = mk_temp_dir("preview_buffer");
        let main = dir.join("main.omni");
        let wav = dir.join("input.wav");
        let wav_alt = dir.join("input_alt.wav");

        write_file(
            &main,
            "buffers:\n  src: buffer[f32]\nouts:\n  out1\ninit:\n  idx = 0\nsample:\n  out1 = src[idx]\n  idx = idx + 1\n",
        );
        write_wav(&wav, 1, 48_000, &[0.1, 0.2, 0.3, 0.4]);
        write_wav(&wav_alt, 1, 48_000, &[0.9, 0.8, 0.7, 0.6]);

        let mut session = DaemonSession::default();
        session
            .start_preview(&main)
            .expect("preview should compile and start");

        let buffer_info = session
            .preview(&main)
            .expect("active preview")
            .buffer_info();
        assert_eq!(buffer_info.len(), 1);
        assert_eq!(buffer_info[0].name, "src");

        session
            .preview_mut(&main)
            .expect("active preview")
            .bind_buffer_wav_path("src", &wav)
            .expect("wav buffer bind should succeed");

        let rendered = session
            .render_preview_block(&main)
            .expect("preview render with bound wav should succeed");
        assert!((rendered[0][0] - 0.1).abs() < 1e-6);
        assert!((rendered[0][1] - 0.2).abs() < 1e-6);
        assert!((rendered[0][2] - 0.3).abs() < 1e-6);
        assert!((rendered[0][3] - 0.4).abs() < 1e-6);

        session
            .preview_mut(&main)
            .expect("active preview")
            .bind_buffer_wav_path("src", &wav_alt)
            .expect("second wav buffer bind should succeed");
        let rebound = session
            .render_preview_block(&main)
            .expect("preview render with rebound wav should succeed");
        assert!((rebound[0][0] - 0.9).abs() < 1e-6);
        assert!((rebound[0][1] - 0.8).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_binds_multichannel_wav_file_to_f32_buffer() {
        let dir = mk_temp_dir("preview_buffer_stereo");
        let main = dir.join("main.omni");
        let wav = dir.join("stereo.wav");

        write_file(
            &main,
            "buffers:\n  src: buffer[f32[2]]\nouts:\n  out1\n  out2\ninit:\n  idx = 0\nsample:\n  out1 = src[0][idx]\n  out2 = src[1][idx]\n  idx = idx + 1\n",
        );
        write_wav(&wav, 2, 48_000, &[0.1, 0.5, 0.2, 0.6, 0.3, 0.7, 0.4, 0.8]);

        let mut session = DaemonSession::default();
        session
            .start_preview(&main)
            .expect("preview should compile and start");

        session
            .preview_mut(&main)
            .expect("active preview")
            .bind_buffer_wav_path("src", &wav)
            .expect("stereo wav buffer bind should succeed");

        let rendered = session
            .render_preview_block(&main)
            .expect("preview render with bound stereo wav should succeed");
        assert!((rendered[0][0] - 0.1).abs() < 1e-6);
        assert!((rendered[1][0] - 0.5).abs() < 1e-6);
        assert!((rendered[0][1] - 0.2).abs() < 1e-6);
        assert!((rendered[1][1] - 0.6).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_rebuilds_instance_when_buffer_binding_changes() {
        let dir = mk_temp_dir("preview_buffer_rebuild");
        let main = dir.join("main.omni");
        let wav_a = dir.join("a.wav");
        let wav_b = dir.join("b.wav");

        write_file(
            &main,
            "buffers:\n  src: buffer[f32]\nouts:\n  out1\ninit:\n  counter = 1\nsample:\n  out1 = f32(counter)\n  counter = counter + 1\n",
        );
        write_wav(&wav_a, 1, 48_000, &[0.1, 0.2, 0.3, 0.4]);
        write_wav(&wav_b, 1, 48_000, &[0.8, 0.7, 0.6, 0.5]);

        let mut session = DaemonSession::default();
        session
            .start_preview(&main)
            .expect("preview should compile and start");

        let initial = session
            .render_preview_block(&main)
            .expect("initial preview render should succeed");
        assert!((initial[0][0] - 1.0).abs() < 1e-6);

        session
            .preview_mut(&main)
            .expect("active preview")
            .bind_buffer_wav_path("src", &wav_a)
            .expect("first wav buffer bind should succeed");
        let first = session
            .render_preview_block(&main)
            .expect("preview render with first buffer should succeed");
        assert!((first[0][0] - 1.0).abs() < 1e-6);

        session
            .preview_mut(&main)
            .expect("active preview")
            .bind_buffer_wav_path("src", &wav_b)
            .expect("second wav buffer bind should succeed");
        let second = session
            .render_preview_block(&main)
            .expect("preview render with rebound buffer should succeed");
        assert!((second[0][0] - 1.0).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_exposes_and_triggers_scalar_events() {
        let dir = mk_temp_dir("preview_events");
        let main = dir.join("main.omni");

        write_file(
            &main,
            "outs:\n  out1\n  out2\ninit:\n  note_state: i32 = 0\n  vel_state = 0.0\nevents:\n  note_on(note: i32, vel: f32, accent: bool):\n    note_state = note\n    vel_state = vel\n    if (accent):\n      vel_state = vel_state + 1.0\n  unsupported(values: f32[2]):\n    note_state = 1\nsample:\n  out1 = f32(note_state)\n  out2 = vel_state\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_preview(&main)
            .expect("preview should compile and start");

        let events = session.preview(&main).expect("active preview").event_info();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "note_on");
        assert_eq!(events[0].params.len(), 3);
        assert_eq!(events[0].params[0].type_repr, "i32");
        assert_eq!(events[0].params[1].type_repr, "f32");
        assert_eq!(events[0].params[2].type_repr, "bool");

        session
            .preview_mut(&main)
            .expect("active preview")
            .trigger_event(
                "note_on",
                &[
                    PreviewEventValue::Number(72.0),
                    PreviewEventValue::Number(0.25),
                    PreviewEventValue::Bool(true),
                ],
            )
            .expect("event trigger should succeed");

        let rendered = session
            .render_preview_block(&main)
            .expect("preview render after event should succeed");
        assert!((rendered[0][0] - 72.0).abs() < 1e-6);
        assert!((rendered[1][0] - 1.25).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }
}
