//! Startup through the real render worker and control server, without opening audio devices.
use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const CONTROL_SOURCE: &str = "params:\n  divisor: i32 = 0\ninit:\n  value = i32(1) / divisor\n  print(value)\nsample:\n  out1 = f32(value)\n";

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onda_startup_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self(fs::canonicalize(root).unwrap());
        fixture.write(source);
        fixture
    }

    fn write(&self, source: &str) {
        fs::write(self.0.join("main.onda"), source).unwrap();
    }

    fn asset(&self, name: &str, value: i32) -> PathBuf {
        let path = self.0.join(format!("{name}.ondabuffer"));
        let asset = onda_project::BufferAsset::new(
            1,
            1,
            48_000.0,
            onda_project::BufferSamples::I32(vec![value]),
        )
        .unwrap();
        fs::write(&path, onda_project::encode_ondabuffer(&asset).unwrap()).unwrap();
        path
    }

    fn launch(&self, start_paused: bool, divisor: Option<i32>) -> PlaybackLaunch {
        PlaybackLaunch {
            input: self.0.join("main.onda"),
            project_file_digests: None,
            compile_inputs: Default::default(),
            duration: None,
            sample_rate_hz: 48_000,
            block_samples: 64,
            param_smoothing_seconds: 10.0,
            opt_level: Default::default(),
            input_device: None,
            output_device: None,
            midi_input_device: None,
            fast_math: false,
            show_meta: false,
            control_json: true,
            start_paused,
            param_sets: divisor
                .map(|value| ("divisor".to_owned(), RunParamValue::I32(value)))
                .into_iter()
                .collect(),
            buffer_bindings: Vec::new(),
            project_buffer_bindings: Vec::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct RenderHarness {
    started: PlaybackStarted,
    controls: mpsc::Sender<PlaybackControlCommand>,
    prints: mpsc::Receiver<RunPrintBatch>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    scope: Arc<ScopeCapture>,
    worker: Option<thread::JoinHandle<()>>,
    server: Option<thread::JoinHandle<()>>,
}

impl RenderHarness {
    fn new(launch: PlaybackLaunch) -> Self {
        let (_midi, midi_rx) = mpsc::sync_channel(1);
        let (startup_tx, startup_rx) = mpsc::channel();
        let (controls, control_rx) = mpsc::channel();
        let (prints, print_rx) = mpsc::sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let scope = Arc::new(ScopeCapture::default());
        let worker = spawn_run_render_thread(
            launch,
            RenderThreadContext {
                stream_errors: StreamErrorState::default(),
                midi_rx,
                midi_overflowed: Default::default(),
                midi_reset_requested: Default::default(),
                scope_ring: scope.clone(),
                stop_flag: stop.clone(),
                render_error: error.clone(),
                startup_tx,
                control_rx: Some(control_rx),
                output_transport: None,
                print_transport: PrintTransport {
                    sender: prints,
                    dropped_occurrences: Default::default(),
                    pending_overflow: Default::default(),
                },
            },
        );
        let started = startup_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap_or_else(|error| panic!("{}", error.message));
        Self {
            started,
            controls,
            prints: print_rx,
            stop,
            error,
            scope,
            worker: Some(worker),
            server: None,
        }
    }

    fn request<T>(
        &self,
        command: impl FnOnce(PlaybackReply<T>) -> PlaybackControlCommand,
    ) -> Result<T, String> {
        let (reply, response) = mpsc::channel();
        self.controls.send(command(reply)).unwrap();
        response.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn samples(&self) -> Vec<f32> {
        let count = 64 * self.started.metadata.output_channels;
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.started.samples.len() < count {
            let error = self.error.lock().unwrap().clone();
            assert!(error.is_none(), "{error:?}");
            assert!(
                Instant::now() < deadline,
                "render worker did not produce a block"
            );
            thread::sleep(Duration::from_millis(1));
        }
        let mut samples = vec![0.0; count];
        assert_eq!(
            self.started
                .samples
                .pop_slice_aligned(&mut samples, self.started.metadata.output_channels),
            count
        );
        samples
    }

    fn connect_controller(&mut self, controller: &mut crate::RunController) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_output, output_rx) = mpsc::sync_channel(1);
        self.server = Some(spawn_run_control_server(
            listener,
            RunControlServerContext {
                control_tx: self.controls.clone(),
                scope_ring: self.scope.clone(),
                stop_flag: self.stop.clone(),
                output_rx,
                dropped_delegate_occurrences: Default::default(),
                pending_delegate_overflow: Default::default(),
                dropped_print_occurrences: Default::default(),
                pending_print_overflow: Default::default(),
            },
        ));
        let startup = &self.started.metadata;
        let metadata = run_metadata_json(
            &startup.params,
            &startup.buffers,
            &startup.events,
            &startup.midi,
            &startup.delegates,
            startup.output_channels,
        );
        let params: Vec<crate::RunParamWire> =
            serde_json::from_value(metadata["params"].clone()).unwrap();
        controller.advance_child_generation();
        controller.launch_path = startup.path.clone();
        controller.onda_path = startup.path.clone();
        let snapshot = crate::source_snapshot_from_paths(&startup.path, &[]);
        controller.watched_sources = snapshot.paths();
        controller.watched_paths = snapshot.watch_paths();
        controller.source_compilation = crate::SourceCompilationState::Compiling(snapshot);
        controller.compiled_watch_revision = Some(controller.source_watch_revision.current());
        controller.handle_child_ready(crate::ReadyEvent {
            path: display_path(&startup.path),
            port,
            params: params
                .into_iter()
                .map(crate::RunParamWire::into_host_value)
                .collect::<Result<_, _>>()
                .unwrap(),
            buffers: metadata["buffers"].as_array().unwrap().clone(),
            events: metadata["events"].as_array().unwrap().clone(),
            midi: startup.midi,
            delegates: metadata["delegates"].as_array().unwrap().clone(),
            output_channels: startup.output_channels,
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            midi_input_devices: Vec::new(),
            current_input_device: None,
            current_output_device: None,
            current_midi_input_device: None,
        });
    }
}

impl Drop for RenderHarness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
    }
}

#[test]
fn launch_overrides_and_buffers_are_applied_before_initialization() {
    let fixture = Fixture::new("params:\n  divisor: i32 = 0\nbuffers:\n  data: buffer<i32>\ninit:\n  value = i32(2) / divisor / data[0]\nsample:\n  out1 = f32(value)\n");
    let mut launch = fixture.launch(false, Some(1));
    launch
        .buffer_bindings
        .push(("data".into(), Some(fixture.asset("data", 2))));
    let harness = RenderHarness::new(launch);
    assert_eq!(harness.samples(), vec![1.0; 64]);
}

#[test]
fn retained_launch_buffers_load_once_and_honor_later_ui_requests() {
    let fixture = Fixture::new(
        "buffers:\n  clip: i32\ninit:\n  value = clip[0]\nsample:\n  out1 = f32(value)\n",
    );
    for (change, expected) in [("unchanged", 2.0), ("replace", 3.0), ("clear", 0.0)] {
        let mut controller = crate::tests::controller_without_child();
        controller.options.overrides.buffers =
            vec![("clip".into(), Some(fixture.0.join("superseded.ondabuffer")))];
        let replacement = fixture.asset("replacement", 2);
        controller.bind_buffer_file("clip", replacement.to_str().unwrap());

        let mut launch = fixture.launch(true, None);
        launch.buffer_bindings = controller.prepare_child_options().overrides.buffers;
        let mut harness = RenderHarness::new(launch);
        // Startup owns the loaded snapshot, even if its file disappears before ready.
        fs::remove_file(&replacement).unwrap();
        match change {
            "replace" => {
                // A newer request must reload even when its path is unchanged.
                fixture.asset("replacement", 3);
                controller.bind_buffer_file("clip", replacement.to_str().unwrap());
            }
            "clear" => controller.clear_buffer("clip"),
            _ => {}
        }
        harness.connect_controller(&mut controller);
        poll_until(&mut controller, |c| {
            c.state().running && c.state().connected
        });
        assert_eq!(harness.samples(), vec![expected; 64], "{change}");
    }
}

#[test]
fn runtime_launch_values_yield_to_ui_edits_and_reset_across_reload() {
    let fixture = Fixture::new(
        "params:\n  divisor: i32 = 1\ninit:\n  value = divisor\nsample:\n  out1 = f32(value)\n",
    );
    let launch = || fixture.launch(true, Some(2));
    let mut controller = crate::tests::controller_without_child();
    controller.options.overrides.params = vec![("divisor".into(), RunParamValue::I32(2))];
    let mut harness = RenderHarness::new(launch());
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    assert_eq!(harness.samples(), vec![2.0; 64]);
    controller.set_param("divisor", json!(3));
    drop(harness);

    let mut harness = RenderHarness::new(launch());
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    assert_eq!(harness.samples(), vec![3.0; 64]);
    controller.reset_params();
    poll_until(&mut controller, |c| {
        !c.pending_commands
            .values()
            .any(|command| matches!(command, crate::PendingCommand::ResetParams))
    });
    drop(harness);

    let mut harness = RenderHarness::new(launch());
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    assert_eq!(harness.samples(), vec![1.0; 64]);
}

#[test]
fn saving_uses_new_child_bindings_before_restoration_acknowledgements() {
    let fixture = Fixture::new("buffers:\n  clip: i32\nsample:\n  out1 = f32(clip[0])\n");
    let mut controller = crate::tests::controller_without_child();
    let mut harness = RenderHarness::new(fixture.launch(true, None));
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    controller.clear_buffer("clip");
    poll_until(&mut controller, |c| {
        c.preserved_buffers.get("clip") == Some(&crate::PreservedBufferBinding::Cleared)
    });
    drop(harness);
    controller.bridge.disconnect();
    controller.pending_commands.clear();

    let replacement = fixture.asset("replacement", 2);
    controller.bind_buffer_file("clip", replacement.to_str().unwrap());
    let mut launch = fixture.launch(true, None);
    launch.buffer_bindings = vec![("clip".into(), Some(replacement))];
    let mut harness = RenderHarness::new(launch);
    harness.connect_controller(&mut controller);
    // Save directly from startup metadata, before polling any restore acknowledgements.
    assert!(controller.restoring);
    let destination = fixture.0.join("saved");
    controller.save_as_project(&destination).unwrap();
    let saved = onda_project::resolve_project_input(
        destination.join("saved.ondaproject"),
        ProjectLimits::default(),
    )
    .unwrap();
    let assets = saved
        .project()
        .unwrap()
        .load_buffer_assets(ProjectLimits::default())
        .unwrap();
    assert!(
        matches!(&assets["clip"].0.samples, onda_project::BufferSamples::I32(values) if values == &[2])
    );
}

#[test]
fn playback_combines_typed_parameters_constants_and_indexed_buffers() {
    let fixture = Fixture::new("config const Count: i32 = 1\nparams:\n  gains: f32[Count] = 0.0\n  enabled: bool = false\n  wide: i64 = 0\nbuffers:\n  bank: i32 {Count}\ninit:\n  valid = Count == 2 && enabled && gains[1] == 0.5 && wide == 9007199254740993 && bank[1][0] == 2\n  value = 1 / i32(valid)\nsample:\n  out1 = f32(value)\n");
    let mut launch = fixture.launch(false, None);
    let parsed = onda_frontend::parse_program_file(&launch.input).unwrap();
    launch.compile_inputs = onda_semantics::compile_inputs_from_literals(
        &parsed,
        [("Count".into(), "2".into())],
        Default::default(),
    )
    .unwrap();
    launch.param_sets = vec![
        ("gains[1]".into(), RunParamValue::F32(0.5)),
        (
            "enabled".into(),
            crate::parse_run_param_value("true").unwrap(),
        ),
        (
            "wide".into(),
            crate::parse_run_param_value("9007199254740993").unwrap(),
        ),
    ];
    launch.buffer_bindings = vec![("bank[1]".into(), Some(fixture.asset("clip", 2)))];
    let harness = RenderHarness::new(launch);
    assert_eq!(harness.samples(), vec![1.0; 64]);
}

#[test]
fn paused_startup_exposes_controls_without_initializing() {
    let fixture = Fixture::new(CONTROL_SOURCE);
    for divisor in [None, Some(1)] {
        let harness = RenderHarness::new(fixture.launch(true, divisor));
        let params = harness
            .request(|reply| PlaybackControlCommand::GetParams { reply })
            .unwrap();
        assert_eq!(
            params[0].value,
            Some(RunParamValue::I32(divisor.unwrap_or(0)))
        );
        assert!(harness.started.samples.is_empty());
        assert!(harness.prints.try_recv().is_err());
        harness
            .request(|reply| PlaybackControlCommand::SetParam {
                name: "divisor".into(),
                value: RunParamValue::I32(1),
                reply: Some(reply),
            })
            .unwrap();
        harness
            .request(|reply| PlaybackControlCommand::Play { reply })
            .unwrap();
        assert_eq!(harness.samples(), vec![1.0; 64]);
        assert_eq!(
            harness
                .prints
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .entries
                .len(),
            1
        );
    }
}

fn poll_until(
    controller: &mut crate::RunController,
    check: impl Fn(&crate::RunController) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        controller.poll();
        if check(controller) {
            return;
        }
        assert!(
            controller.state().error.is_none(),
            "{:?}",
            controller.state().error
        );
        assert!(
            Instant::now() < deadline,
            "timed out: {}",
            controller.state().status
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn native_reload_restores_all_controls_and_buffers_and_retries_failed_restoration() {
    let source = |init| {
        format!("params:\n  divisor: i32 = 0\nbuffers:\n  first: buffer<i32>\n  second: buffer<i32>\ninit:\n  value = {init}\nsample:\n  out1 = f32(value)\n")
    };
    let fixture = Fixture::new(&source("0"));
    let mut controller = crate::tests::controller_without_child();
    let mut harness = RenderHarness::new(fixture.launch(true, None));
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    controller.set_param("divisor", json!(1));
    for (name, value) in [("first", 2), ("second", 3)] {
        controller.bind_buffer_file(name, fixture.asset(name, value).to_str().unwrap());
    }
    poll_until(&mut controller, |c| {
        c.state().buffers.iter().all(|b| b["loadedSamples"] == 1)
    });
    drop(harness);

    fixture.write(&source(
        "i32(1) / divisor + i32(6) / first[0] + i32(6) / second[0]",
    ));
    let mut harness = RenderHarness::new(fixture.launch(true, None));
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    assert_eq!(harness.samples(), vec![6.0; 64]);
    drop(harness);

    fs::remove_file(fixture.0.join("first.ondabuffer")).unwrap();
    let mut harness = RenderHarness::new(fixture.launch(true, None));
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| c.state().error.is_some());
    assert!(!controller.state().running && !controller.state().connected);
    drop(harness);

    fixture.asset("first", 2);
    controller.processing_requested = true;
    let mut harness = RenderHarness::new(fixture.launch(true, None));
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    assert_eq!(harness.samples(), vec![6.0; 64]);
}

#[test]
fn overlapping_play_retry_recovers_after_faulting_initialization() {
    let fixture = Fixture::new(CONTROL_SOURCE);
    let mut harness = RenderHarness::new(fixture.launch(true, None));
    let mut controller = crate::tests::controller_without_child();
    controller.processing_requested = false;
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| c.state().connected && !c.restoring);

    controller.processing_requested = true;
    controller.request_play().unwrap();
    controller.set_param("divisor", json!(1));
    controller.request_play().unwrap();
    // The real server replies to the faulting Play before the repaired retry.
    poll_until(&mut controller, |c| c.state().running);
    assert!(controller.processing_requested);
    assert!(controller.state().error.is_none());
    assert_eq!(harness.samples(), vec![1.0; 64]);
}

#[test]
fn smoothing_changed_while_compiling_is_restored_before_play() {
    let fixture = Fixture::new("params:\n  gain: f32 = 0.0\nsample:\n  out1 = gain\n");
    let mut launch = fixture.launch(true, None);
    launch.param_smoothing_seconds = 10.0;
    let mut harness = RenderHarness::new(launch);
    let mut controller = crate::tests::controller_without_child();
    controller.options.param_smoothing_seconds = 10.0;
    // The child has the launch duration while the UI changes it before readiness.
    controller.set_param_smoothing_seconds(0.0).unwrap();
    harness.connect_controller(&mut controller);
    poll_until(&mut controller, |c| {
        c.state().running && c.state().connected
    });
    harness.samples(); // Start the first-block gate before editing a live target.
    harness
        .request(|reply| PlaybackControlCommand::SetParam {
            name: "gain".into(),
            value: RunParamValue::F32(1.0),
            reply: Some(reply),
        })
        .unwrap();
    // Discard queued audio so the assertion observes the acknowledged edit.
    let mut discarded = vec![0.0; harness.started.samples.len()];
    harness.started.samples.pop_slice_aligned(&mut discarded, 1);
    assert_eq!(controller.options.param_smoothing_seconds, 0.0);
    assert_eq!(harness.samples(), vec![1.0; 64]);
}
