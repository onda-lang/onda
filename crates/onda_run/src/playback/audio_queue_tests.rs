use super::*;
use std::fs;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn many_channel_programs_supply_complete_device_callbacks_and_preserve_scope_samples() {
    let directory = std::env::temp_dir().join(format!(
        "onda_audio_queue_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    for (channels, start_paused) in [2, 30, 32, 33, 64]
        .into_iter()
        .flat_map(|channels| [(channels, false), (channels, true)])
    {
        let path = directory.join(format!("{channels}.onda"));
        fs::write(
            &path,
            format!("ins {channels}\nouts {channels}\nsample:\n  out1 = 0.5\n"),
        )
        .unwrap();
        let block_samples = 512;
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let scope = Arc::new(ScopeCapture::default());
        scope.set_mode("waveform").unwrap();
        let (_midi, midi_rx) = mpsc::sync_channel(1);
        let (startup_tx, startup_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let (prints, _print_rx) = mpsc::sync_channel(1);
        let thread = spawn_run_render_thread(
            PlaybackLaunch {
                input: path,
                project_file_digests: None,
                compile_inputs: Default::default(),
                duration: None,
                sample_rate_hz: 48_000,
                block_samples,
                param_smoothing_seconds: 0.0,
                opt_level: Default::default(),
                input_device: None,
                output_device: None,
                midi_input_device: None,
                fast_math: Default::default(),
                show_meta: false,
                control_json: false,
                start_paused,
                param_sets: Vec::new(),
                buffer_bindings: Vec::new(),
                project_buffer_bindings: Vec::new(),
            },
            RenderThreadContext {
                stream_errors: StreamErrorState::default(),
                midi_rx,
                midi_overflowed: Arc::new(AtomicBool::new(false)),
                midi_reset_requested: Arc::new(AtomicBool::new(false)),
                scope_ring: Arc::clone(&scope),
                stop_flag: Arc::clone(&stop),
                render_error: Arc::clone(&error),
                startup_tx,
                control_rx: Some(control_rx),
                output_transport: None,
                print_transport: PrintTransport {
                    sender: prints,
                    dropped_occurrences: Arc::new(AtomicU32::new(0)),
                    pending_overflow: Arc::new(AtomicU32::new(0)),
                },
            },
        );
        let started = startup_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap_or_else(|e| panic!("{}", e.message));
        assert_eq!(started.metadata.input_channels, channels);
        assert_eq!(started.metadata.output_channels, channels);
        if start_paused {
            // A control reply is a render-thread barrier: initialization finished,
            // but no DSP samples may be produced until the host asks to play.
            let (reply, response) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetParams { reply })
                .unwrap();
            response
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert!(started.samples.is_empty());
            let (reply, response) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::Play { reply })
                .unwrap();
            response
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
        }
        wait_for_prefill(
            &started.samples,
            render_ahead_samples(block_samples, channels, started.samples.capacity()),
            &stop,
            &error,
        )
        .unwrap();
        let captured_scope = scope.snapshot(block_samples).unwrap();
        let mut callback = vec![f32::NAN; block_samples * channels];
        let consumed = started.samples.pop_slice_aligned(&mut callback, channels);
        stop.store(true, Ordering::Release);
        thread.join().unwrap();
        assert_eq!(captured_scope.samples.len(), callback.len());
        for frame in captured_scope.samples.chunks_exact(channels) {
            assert_eq!(frame[0], 0.5, "the producer/scope signal must be clean");
        }
        assert_eq!(
            consumed,
            callback.len(),
            "{channels} channels: device callback contains missing frames despite a clean scope"
        );
        assert_eq!(callback, captured_scope.samples);
        assert!(
            started.input.capacity() >= block_samples * channels,
            "the input queue must also hold complete program-channel blocks"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}
