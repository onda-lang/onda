mod run_session;

pub use onda_semantics::{AnalysisSession, AnalysisSnapshot, DocumentVersion, OpenDocument};
pub use run_session::{
    InitialBufferBinding, RunBufferChannels, RunBufferInfo, RunBufferWaveform, RunBuildError,
    RunDelegateBatch, RunDelegateInfo, RunDelegateOccurrence, RunDelegateParamInfo,
    RunDelegateValue, RunEventInfo, RunEventParamInfo, RunEventValue, RunOptions, RunParamInfo,
    RunPrintBatch, RunPrintEntry, RunPrintValue, RunSession,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use onda_frontend::Diagnostic;
use onda_semantics::AnalysisOptions;
use onda_semantics::CompileInputs;

use onda_semantics::normalize_session_path;

#[derive(Debug, Clone, Copy, Default)]
pub struct DaemonConfig {
    pub analysis: AnalysisOptions,
    pub run: RunOptions,
}

#[derive(Debug, Default)]
pub struct DaemonSession {
    config: DaemonConfig,
    analysis: AnalysisSession,
    runs: HashMap<PathBuf, RunSession>,
}

impl DaemonSession {
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            analysis: AnalysisSession::new(),
            runs: HashMap::new(),
        }
    }

    pub fn config(&self) -> DaemonConfig {
        self.config
    }

    pub fn set_config(&mut self, config: DaemonConfig) {
        self.config = config;
        self.runs.clear();
    }

    pub fn set_analysis_options(&mut self, options: AnalysisOptions) {
        self.config.analysis = options;
    }

    pub fn set_run_options(&mut self, options: RunOptions) {
        self.config.run = options;
        self.runs.clear();
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
        self.runs.remove(&normalized);
        self.analysis.close_document(normalized)
    }

    pub fn analyze_document(&self, path: impl AsRef<Path>) -> AnalysisSnapshot {
        self.analysis.analyze_document(path, self.config.analysis)
    }

    pub fn start_run(&mut self, path: impl AsRef<Path>) -> Result<&RunSession, RunBuildError> {
        self.start_run_with_options(path, self.config.run)
    }

    pub fn start_run_with_options(
        &mut self,
        path: impl AsRef<Path>,
        options: RunOptions,
    ) -> Result<&RunSession, RunBuildError> {
        self.start_run_with_options_and_initial_buffers(path, options, std::iter::empty())
    }

    pub fn start_run_with_initial_buffers(
        &mut self,
        path: impl AsRef<Path>,
        initial_buffers: impl IntoIterator<Item = InitialBufferBinding>,
    ) -> Result<&RunSession, RunBuildError> {
        self.start_run_with_options_and_initial_buffers(path, self.config.run, initial_buffers)
    }

    pub fn start_run_with_options_and_initial_buffers(
        &mut self,
        path: impl AsRef<Path>,
        options: RunOptions,
        initial_buffers: impl IntoIterator<Item = InitialBufferBinding>,
    ) -> Result<&RunSession, RunBuildError> {
        self.start_run_with_options_inputs_and_initial_buffers(
            path,
            options,
            &CompileInputs::default(),
            initial_buffers,
        )
    }

    pub fn start_run_with_options_inputs_and_initial_buffers(
        &mut self,
        path: impl AsRef<Path>,
        options: RunOptions,
        inputs: &CompileInputs,
        initial_buffers: impl IntoIterator<Item = InitialBufferBinding>,
    ) -> Result<&RunSession, RunBuildError> {
        let normalized = normalize_session_path(path.as_ref());
        let run = RunSession::build_with_inputs_and_initial_buffers(
            &self.analysis,
            &normalized,
            options,
            inputs,
            initial_buffers,
        )?;
        self.runs.insert(normalized.clone(), run);
        Ok(self
            .runs
            .get(&normalized)
            .expect("run inserted into session"))
    }

    pub fn rebuild_run(&mut self, path: impl AsRef<Path>) -> Result<&RunSession, RunBuildError> {
        let normalized = normalize_session_path(path.as_ref());
        let (options, inputs) = self
            .runs
            .get(&normalized)
            .map(|run| (run.options(), run.compile_inputs().clone()))
            .unwrap_or((self.config.run, CompileInputs::default()));
        self.start_run_with_options_inputs_and_initial_buffers(
            normalized,
            options,
            &inputs,
            std::iter::empty(),
        )
    }

    pub fn run(&self, path: impl AsRef<Path>) -> Option<&RunSession> {
        let normalized = normalize_session_path(path.as_ref());
        self.runs.get(&normalized)
    }

    pub fn run_mut(&mut self, path: impl AsRef<Path>) -> Option<&mut RunSession> {
        let normalized = normalize_session_path(path.as_ref());
        self.runs.get_mut(&normalized)
    }

    pub fn stop_run(&mut self, path: impl AsRef<Path>) -> Option<RunSession> {
        let normalized = normalize_session_path(path.as_ref());
        self.runs.remove(&normalized)
    }

    pub fn render_run_block(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Vec<Vec<f32>>, Diagnostic> {
        let normalized = normalize_session_path(path.as_ref());
        let run = self.runs.get_mut(&normalized).ok_or_else(|| {
            Diagnostic::runtime(
                format!("run is not active for '{}'", normalized.display()),
                0,
                0,
            )
        })?;
        run.render_block()
    }

    pub fn render_run_block_interleaved(
        &mut self,
        path: impl AsRef<Path>,
        rendered: &mut [f32],
    ) -> Result<(), Diagnostic> {
        let normalized = normalize_session_path(path.as_ref());
        let run = self.runs.get_mut(&normalized).ok_or_else(|| {
            Diagnostic::runtime(
                format!("run is not active for '{}'", normalized.display()),
                0,
                0,
            )
        })?;
        run.render_block_interleaved(rendered)
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
        let dir = std::env::temp_dir().join(format!("onda_daemon_{prefix}_{nanos}"));
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
        let main = dir.join("main.onda");
        let lib = dir.join("lib.onda");

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
        let main = dir.join("main.onda");
        let lib = dir.join("lib.onda");

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
        let main = dir.join("main.onda");

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
    fn run_renders_and_updates_scalar_param() {
        let dir = mk_temp_dir("run_render");
        let main = dir.join("main.onda");

        write_file(
            &main,
            "outs:\n  out1\nparams:\n  gain = 0.25 {0.0, 1.0}\nsample:\n  out1 = gain\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run_with_options(
                &main,
                RunOptions {
                    float_param_smoothing_ms: 0.0,
                    ..RunOptions::default()
                },
            )
            .expect("run should compile and start");

        let param_info = session.run(&main).expect("active run").param_info();
        assert_eq!(param_info.len(), 1);
        assert_eq!(param_info[0].name, "gain");
        assert_eq!(param_info[0].default, Some(0.25));

        let first = session
            .render_run_block(&main)
            .expect("first run render should succeed");
        assert_eq!(first.len(), 1);
        assert!(first[0].iter().all(|sample| (*sample - 0.25).abs() < 1e-6));

        session
            .run_mut(&main)
            .expect("active run")
            .set_param_f64("gain", 0.5)
            .expect("param update should succeed");
        let second = session
            .render_run_block(&main)
            .expect("second run render should succeed");
        assert!(second[0].iter().all(|sample| (*sample - 0.5).abs() < 1e-6));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_collects_initialization_process_and_event_prints() {
        let dir = mk_temp_dir("run_prints");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "outs:\n  out1\ninit:\n  print(\"boot\\n\", -0.0)\nevent report(value: i64):\n  print(\"event\", value)\nsample:\n  print(\"frame\", 7, true)\n  out1 = 0.0\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run_with_options(
                &main,
                RunOptions {
                    block_size: 2,
                    ..RunOptions::default()
                },
            )
            .expect("run should start");
        let run = session.run_mut(&main).expect("active run");

        let init = run.take_print_batch().expect("init prints should decode");
        assert_eq!(init.text, "boot\\n: -0.0\n");
        assert_eq!(init.entries.len(), 1);
        assert_eq!(init.entries[0].label.as_deref(), Some("boot\n"));
        assert_eq!(init.entries[0].source_file.as_deref(), Some("main.onda"));

        run.render_block().expect("process should run");
        let process = run
            .take_print_batch()
            .expect("process prints should decode");
        assert_eq!(process.text, "frame: 7 true\nframe: 7 true\n");
        assert_eq!(process.entries.len(), 2);

        run.trigger_event("report", &[RunEventValue::I64(9_007_199_254_740_993)])
            .expect("event should run");
        let event = run.take_print_batch().expect("event prints should decode");
        assert_eq!(event.text, "event: 9007199254740993\n");
        assert_eq!(event.entries[0].values[0].type_repr, "i64");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_initialization_returns_only_the_runtime_diagnostic() {
        let dir = mk_temp_dir("run_failed_init_prints");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "params:\n  divisor: i32 = 0\nouts:\n  out1\ninit:\n  print(\"before failure\", 7)\n  value = i32(1) / divisor\nsample:\n  out1 = f32(value)\n",
        );

        let mut session = DaemonSession::default();
        let error = session
            .start_run(&main)
            .expect_err("division by zero should fail generated initialization");
        let RunBuildError::Runtime(diagnostic) = error else {
            panic!("expected a runtime initialization failure");
        };
        assert!(diagnostic.message.contains("runtime safety check"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_uses_neutral_descriptors_for_unbound_f32_buffers() {
        let dir = mk_temp_dir("run_buffer");
        let main = dir.join("main.onda");
        let wav = dir.join("input.wav");
        let wav_alt = dir.join("input_alt.wav");

        write_file(
            &main,
            "buffers:\n  src: buffer<f32>\n  spare: buffer<f32>\nouts:\n  out1\ninit:\n  idx = 0\nsample:\n  out1 = src[idx]\n  idx = idx + 1\n",
        );
        write_wav(&wav, 1, 48_000, &[0.1, 0.2, 0.3, 0.4]);
        write_wav(&wav_alt, 1, 48_000, &[0.9, 0.8, 0.7, 0.6]);

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");

        let buffer_info = session.run(&main).expect("active run").buffer_info();
        assert_eq!(buffer_info.len(), 2);
        assert_eq!(buffer_info[0].name, "src");
        assert!(buffer_info[0].loaded_path.is_none());
        assert_eq!(buffer_info[0].loaded_frames, None);
        assert_eq!(buffer_info[0].loaded_channels, None);
        assert_eq!(buffer_info[0].loaded_sample_rate_hz, None);

        let unbound = session
            .render_run_block(&main)
            .expect("unbound buffers should process through neutral descriptors");
        assert!(unbound[0].iter().all(|sample| *sample == 0.0));

        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_wav_path("src", &wav)
            .expect("wav buffer bind should succeed");
        let loaded = &session.run(&main).expect("active run").buffer_info()[0];
        assert_eq!(loaded.loaded_frames, Some(4));
        assert_eq!(loaded.loaded_channels, Some(1));
        assert_eq!(loaded.loaded_sample_rate_hz, Some(48_000.0));
        let waveform = loaded.waveform.as_ref().expect("loaded waveform preview");
        assert!((waveform.min_value - 0.1).abs() < 1e-6);
        assert!((waveform.max_value - 0.4).abs() < 1e-6);
        assert_eq!(waveform.minimums.len(), 4);
        assert_eq!(waveform.maximums.len(), 4);

        let partially_bound = session
            .render_run_block(&main)
            .expect("unused unbound buffers should remain neutral");
        assert!((partially_bound[0][0] - 0.1).abs() < 1e-6);
        assert!((partially_bound[0][1] - 0.2).abs() < 1e-6);
        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_wav_path("spare", &wav)
            .expect("spare wav buffer bind should succeed");

        let rendered = session
            .render_run_block(&main)
            .expect("run render with bound wav should succeed");
        assert!((rendered[0][0] - 0.1).abs() < 1e-6);
        assert!((rendered[0][1] - 0.2).abs() < 1e-6);
        assert!((rendered[0][2] - 0.3).abs() < 1e-6);
        assert!((rendered[0][3] - 0.4).abs() < 1e-6);

        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_wav_path("src", &wav_alt)
            .expect("second wav buffer bind should succeed");
        let rebound = session
            .render_run_block(&main)
            .expect("run render with rebound wav should succeed");
        assert!((rebound[0][0] - 0.9).abs() < 1e-6);
        assert!((rebound[0][1] - 0.8).abs() < 1e-6);

        session
            .run_mut(&main)
            .expect("active run")
            .clear_buffer("src")
            .expect("clearing a buffer should succeed");
        let cleared = session
            .render_run_block(&main)
            .expect("cleared buffers should fall back to neutral descriptors");
        assert!(cleared[0].iter().all(|sample| *sample == 0.0));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_builds_once_with_all_initial_buffer_bindings() {
        let dir = mk_temp_dir("run_initial_buffers");
        let main = dir.join("main.onda");
        let left = dir.join("left.wav");
        let right = dir.join("right.wav");
        write_file(
            &main,
            "buffers:\n  left: buffer<f32>\n  right: buffer<f32>\nouts:\n  out1\nsample:\n  out1 = left[0] + right[0]\n",
        );
        write_wav(&left, 1, 48_000, &[0.25]);
        write_wav(&right, 1, 48_000, &[0.5]);

        let bindings = [
            InitialBufferBinding::load_file("left", &left, onda_project::ProjectLimits::default())
                .expect("load left initial binding"),
            InitialBufferBinding::load_file(
                "right",
                &right,
                onda_project::ProjectLimits::default(),
            )
            .expect("load right initial binding"),
        ];
        let mut session = DaemonSession::default();
        session
            .start_run_with_initial_buffers(&main, bindings)
            .expect("run should start with all buffers already bound");

        let info = session.run(&main).expect("active run").buffer_info();
        assert_eq!(
            info[0].loaded_path.as_deref(),
            Some(left.to_string_lossy().as_ref())
        );
        assert_eq!(
            info[1].loaded_path.as_deref(),
            Some(right.to_string_lossy().as_ref())
        );
        let rendered = session
            .render_run_block(&main)
            .expect("initially bound run should render");
        assert!(rendered[0]
            .iter()
            .all(|sample| (*sample - 0.75).abs() < 1e-6));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_binds_typed_i32_buffer_asset() {
        let dir = mk_temp_dir("run_i32_buffer");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "buffers:\n  values: buffer<i32>\nouts:\n  out1\ninit:\n  idx = 0\nsample:\n  out1 = f32(values[idx])\n  idx = idx + 1\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");
        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_asset(
                "values",
                onda_project::BufferAsset::new(
                    512,
                    1,
                    1.0,
                    onda_project::BufferSamples::I32({
                        let mut values = vec![0; 512];
                        values[..4].copy_from_slice(&[1, -2, 3, 4]);
                        values
                    }),
                )
                .expect("valid i32 buffer asset"),
            )
            .expect("i32 buffer bind should succeed");

        let info = session.run(&main).expect("active run").buffer_info();
        let waveform = info[0].waveform.as_ref().expect("typed waveform preview");
        assert_eq!(waveform.min_value, -2.0);
        assert_eq!(waveform.max_value, 4.0);
        assert_eq!(waveform.minimums.len(), 128);
        assert_eq!(waveform.maximums.len(), 128);
        assert_eq!(waveform.minimums[0], -2.0);
        assert_eq!(waveform.maximums[0], 4.0);
        assert!(waveform.minimums[1..].iter().all(|value| *value == 0.0));
        assert!(waveform.maximums[1..].iter().all(|value| *value == 0.0));

        let rendered = session
            .render_run_block(&main)
            .expect("run render with bound i32 buffer should succeed");
        assert_eq!(&rendered[0][..4], &[1.0, -2.0, 3.0, 4.0]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_binds_multichannel_wav_file_to_f32_buffer() {
        let dir = mk_temp_dir("run_buffer_stereo");
        let main = dir.join("main.onda");
        let wav = dir.join("stereo.wav");

        write_file(
            &main,
            "buffers:\n  src: buffer<f32[2]>\nouts:\n  out1\n  out2\ninit:\n  idx = 0\nsample:\n  out1 = src[0, idx]\n  out2 = src[1, idx]\n  idx = idx + 1\n",
        );
        write_wav(&wav, 2, 48_000, &[0.1, 0.5, 0.2, 0.6, 0.3, 0.7, 0.4, 0.8]);

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");

        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_wav_path("src", &wav)
            .expect("stereo wav buffer bind should succeed");

        let rendered = session
            .render_run_block(&main)
            .expect("run render with bound stereo wav should succeed");
        assert!((rendered[0][0] - 0.1).abs() < 1e-6);
        assert!((rendered[1][0] - 0.5).abs() < 1e-6);
        assert!((rendered[0][1] - 0.2).abs() < 1e-6);
        assert!((rendered[1][1] - 0.6).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejected_buffer_rebind_preserves_the_active_binding() {
        let dir = mk_temp_dir("run_rejected_buffer_rebind");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "buffers:\n  src: buffer<f32[2]>\nouts:\n  out1\nsample:\n  out1 = src[0, 0]\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");
        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_samples("src", vec![0.25, 0.75], 2, 48_000.0)
            .expect("valid stereo binding should succeed");

        let error = session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_samples("src", vec![0.9], 1, 48_000.0)
            .expect_err("mono data must not replace a static stereo binding");
        assert!(error.message.contains("expects 2 channels"), "{error:?}");

        let run = session.run(&main).expect("active run");
        assert_eq!(run.buffer_info()[0].loaded_channels, Some(2));
        let rendered = session
            .render_run_block(&main)
            .expect("the previous binding should remain processable");
        assert!(rendered[0]
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 1e-6));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_renders_single_stereo_output_port() {
        let dir = mk_temp_dir("run_stereo_output_port");
        let main = dir.join("main.onda");

        write_file(
            &main,
            "outs:\n  out: f32[2]\nsample:\n  out[0] = 0.25\n  out[1] = 0.5\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");

        let run = session.run(&main).expect("active run");
        assert_eq!(run.output_channel_count(), 2);

        let rendered = session
            .render_run_block(&main)
            .expect("run render with stereo output port should succeed");
        assert_eq!(rendered.len(), 2);
        assert!(rendered[0]
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 1e-6));
        assert!(rendered[1]
            .iter()
            .all(|sample| (*sample - 0.5).abs() < 1e-6));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_rebuilds_instance_when_buffer_binding_changes() {
        let dir = mk_temp_dir("run_buffer_rebuild");
        let main = dir.join("main.onda");
        let wav_a = dir.join("a.wav");
        let wav_b = dir.join("b.wav");

        write_file(
            &main,
            "buffers:\n  src: buffer<f32>\nouts:\n  out1\ninit:\n  counter = 1\nsample:\n  out1 = f32(counter)\n  counter = counter + 1\n",
        );
        write_wav(&wav_a, 1, 48_000, &[0.1, 0.2, 0.3, 0.4]);
        write_wav(&wav_b, 1, 48_000, &[0.8, 0.7, 0.6, 0.5]);

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");

        let unbound = session
            .render_run_block(&main)
            .expect("unbound buffers should not prevent processing");
        assert!((unbound[0][0] - 1.0).abs() < 1e-6);

        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_wav_path("src", &wav_a)
            .expect("first wav buffer bind should succeed");
        let first = session
            .render_run_block(&main)
            .expect("run render with first buffer should succeed");
        assert!((first[0][0] - 1.0).abs() < 1e-6);

        session
            .run_mut(&main)
            .expect("active run")
            .bind_buffer_wav_path("src", &wav_b)
            .expect("second wav buffer bind should succeed");
        let second = session
            .render_run_block(&main)
            .expect("run render with rebound buffer should succeed");
        assert!((second[0][0] - 1.0).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_restart_reuses_compiled_program_with_fresh_runtime_state() {
        let dir = mk_temp_dir("run_restart");
        let main = dir.join("main.onda");

        write_file(
            &main,
            "outs:\n  out1\nparams:\n  offset = 2.0\ninit:\n  counter = 1.0\nsample:\n  out1 = counter + offset\n  counter = counter + 1.0\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run_with_options(
                &main,
                RunOptions {
                    float_param_smoothing_ms: 0.0,
                    ..RunOptions::default()
                },
            )
            .expect("run should compile and start");
        session
            .run_mut(&main)
            .expect("active run")
            .set_param_f64("offset", 4.0)
            .expect("param update should succeed");

        let first = session
            .render_run_block(&main)
            .expect("first render should succeed");
        assert!((first[0][0] - 5.0).abs() < 1e-6);
        assert!(first[0][1] > first[0][0]);

        session
            .run_mut(&main)
            .expect("active run")
            .restart()
            .expect("cached program should create a fresh instance");
        let restarted = session
            .render_run_block(&main)
            .expect("restarted render should succeed");

        assert!((restarted[0][0] - 5.0).abs() < 1e-6);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resetting_parameters_does_not_rewind_processor_state() {
        let dir = mk_temp_dir("run_reset");
        let main = dir.join("main.onda");

        write_file(
            &main,
            "outs:\n  out1\nparams:\n  offset = 2.0\ninit:\n  counter = offset\nsample:\n  out1 = counter\n  counter = counter + 1.0\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run_with_options(
                &main,
                RunOptions {
                    float_param_smoothing_ms: 0.0,
                    ..RunOptions::default()
                },
            )
            .expect("run should compile and start");
        session
            .run_mut(&main)
            .expect("active run")
            .set_param_f64("offset", 4.0)
            .expect("param update should succeed");

        let first = session
            .render_run_block(&main)
            .expect("first render should succeed");
        assert!((first[0][0] - 2.0).abs() < 1e-6);
        assert!(first[0][1] > first[0][0]);

        session
            .run_mut(&main)
            .expect("active run")
            .reset_params()
            .expect("parameter reset should succeed");
        let params_reset = session
            .render_run_block(&main)
            .expect("parameter-reset render should succeed");

        assert!(params_reset[0][0] > first[0][0]);
        assert_eq!(
            session.run(&main).expect("active run").param_info()[0].value,
            Some(2.0)
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_exposes_all_events_and_triggers_scalar_events() {
        let dir = mk_temp_dir("run_events");
        let main = dir.join("main.onda");

        write_file(
            &main,
            "outs:\n  out1\n  out2\ninit:\n  note_state: i32 = 0\n  vel_state = 0.0\nevents:\n  note_on(note: i32, vel: f32, accent: bool):\n    note_state = note\n    vel_state = vel\n    if (accent):\n      vel_state = vel_state + 1.0\n  array_event(values: f32[2]):\n    vel_state = values[0] + values[1]\n  slice_event(values: f32[]):\n    vel_state = values[0] * values[1]\nsample:\n  out1 = f32(note_state)\n  out2 = vel_state\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("run should compile and start");

        let events = session.run(&main).expect("active run").event_info();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].name, "note_on");
        assert_eq!(events[0].params.len(), 3);
        assert_eq!(events[0].params[0].type_repr, "i32");
        assert_eq!(events[0].params[1].type_repr, "f32");
        assert_eq!(events[0].params[2].type_repr, "bool");
        assert_eq!(events[1].name, "array_event");
        assert_eq!(events[1].params[0].type_repr, "f32[2]");
        assert_eq!(
            events[1].params[0].value,
            RunEventValue::Array(vec![RunEventValue::Number(0.0), RunEventValue::Number(0.0),])
        );
        assert_eq!(events[2].name, "slice_event");
        assert_eq!(events[2].params[0].type_repr, "f32[]");
        assert_eq!(events[2].params[0].value, RunEventValue::Array(Vec::new()));

        session
            .run_mut(&main)
            .expect("active run")
            .trigger_event(
                "array_event",
                &[RunEventValue::Array(vec![
                    RunEventValue::Number(0.25),
                    RunEventValue::Number(0.5),
                ])],
            )
            .expect("fixed-array event trigger should succeed");
        let rendered = session
            .render_run_block(&main)
            .expect("run render after array event should succeed");
        assert!((rendered[1][0] - 0.75).abs() < 1e-6);

        session
            .run_mut(&main)
            .expect("active run")
            .trigger_event(
                "slice_event",
                &[RunEventValue::Array(vec![
                    RunEventValue::Number(0.5),
                    RunEventValue::Number(0.25),
                ])],
            )
            .expect("slice event trigger should succeed");
        let rendered = session
            .render_run_block(&main)
            .expect("run render after slice event should succeed");
        assert!((rendered[1][0] - 0.125).abs() < 1e-6);

        session
            .run_mut(&main)
            .expect("active run")
            .trigger_event(
                "note_on",
                &[
                    RunEventValue::Number(72.0),
                    RunEventValue::Number(0.25),
                    RunEventValue::Bool(true),
                ],
            )
            .expect("event trigger should succeed");

        let rendered = session
            .render_run_block(&main)
            .expect("run render after event should succeed");
        assert!((rendered[0][0] - 72.0).abs() < 1e-6);
        assert!((rendered[1][0] - 1.25).abs() < 1e-6);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_transports_delegate_payloads_from_event_and_process_calls() {
        let dir = mk_temp_dir("run_delegates");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "delegate report(code: i32, values: f32[])\n\ninit:\n  pending = true\n\nevent trigger(values: f32[]):\n  report(7, values)\n\nsample:\n  if pending:\n    pending = false\n    report(9, [0.25, 0.5])\n  out1 = 0.0\n",
        );

        let mut session = DaemonSession::default();
        session
            .start_run(&main)
            .expect("delegate run should compile and start");
        let run = session.run_mut(&main).expect("active delegate run");
        run.set_delegate_collection_enabled(true);
        assert_eq!(run.delegate_info()[0].name, "report");
        run.trigger_event(
            "trigger",
            &[RunEventValue::Array(vec![
                RunEventValue::Number(1.25),
                RunEventValue::Number(-2.5),
            ])],
        )
        .expect("delegate-producing event should run");
        let event_batch = run
            .take_delegate_batch()
            .expect("event delegate batch should decode");
        assert_eq!(event_batch.overflow_count, 0);
        assert_eq!(event_batch.occurrences.len(), 1);
        assert_eq!(event_batch.occurrences[0].name, "report");
        assert_eq!(
            event_batch.occurrences[0].values[1].value,
            RunEventValue::Array(vec![
                RunEventValue::Number(1.25),
                RunEventValue::Number(-2.5),
            ])
        );

        run.render_block_interleaved(&mut vec![0.0; RunOptions::default().block_size])
            .expect("delegate-producing process call should run");
        let process_batch = run
            .take_delegate_batch()
            .expect("process delegate batch should decode");
        assert_eq!(process_batch.occurrences.len(), 1);
        assert_eq!(
            process_batch.occurrences[0].values[0].value,
            RunEventValue::Number(9.0)
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn when_bindings_do_not_shadow_owner_state() {
        let dir = mk_temp_dir("run_delegate_binding_hygiene");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "delegate fired(payload: i32)\n\ninit:\n  payload: i32 = 42\n  result: i32 = 0\n\nevent trigger():\n  fired(7)\n\nwhen fired(value):\n  result = payload\n\nsample:\n  out1 = f32(result)\n",
        );

        let mut session = DaemonSession::default();
        session.start_run(&main).expect("run should start");
        session
            .run_mut(&main)
            .expect("active run")
            .trigger_event("trigger", &[])
            .expect("event should run");
        let rendered = session
            .render_run_block(&main)
            .expect("run render should succeed");
        assert_eq!(rendered[0][0], 42.0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn namespaced_proc_delegate_calls_remain_owner_local() {
        let dir = mk_temp_dir("run_namespaced_delegate");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "namespace N:\n  def fired():\n    return\n\n  proc Child:\n    delegate fired()\n    event trigger():\n      fired()\n    sample:\n      out1 = 0.0\n\ndelegate observed()\n\ninit:\n  child = N::Child()\n\nwhen child.fired():\n  observed()\n\nevent trigger():\n  child.trigger()\n\nsample:\n  out1 = child()\n",
        );

        let mut session = DaemonSession::default();
        session.start_run(&main).expect("run should start");
        let run = session.run_mut(&main).expect("active run");
        run.set_delegate_collection_enabled(true);
        run.trigger_event("trigger", &[]).expect("event should run");
        let batch = run.take_delegate_batch().expect("batch should decode");
        assert_eq!(batch.occurrences.len(), 1);
        assert_eq!(batch.occurrences[0].name, "observed");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_delegate_batches_preserve_i64_payloads() {
        let dir = mk_temp_dir("run_delegate_i64");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "delegate report(value: i64)\n\nevent trigger():\n  report(9007199254740993)\n\nsample:\n  out1 = 0.0\n",
        );

        let mut session = DaemonSession::default();
        session.start_run(&main).expect("run should start");
        let run = session.run_mut(&main).expect("active run");
        run.set_delegate_collection_enabled(true);
        run.trigger_event("trigger", &[]).expect("event should run");
        let batch = run.take_delegate_batch().expect("batch should decode");
        assert_eq!(
            batch.occurrences[0].values[0].value,
            RunEventValue::I64(9_007_199_254_740_993)
        );

        fs::remove_dir_all(&dir).ok();
    }
}
