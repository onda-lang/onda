//! Capture the native run's effective configuration and buffer bindings.
use super::*;
use onda_project::{ProjectConstValue, ProjectLimits};
use onda_semantics::{AnalysisOptions, ConstValue, TypedConstValue};

impl RunController {
    pub fn save_as_project(&self, destination: &Path) -> Result<(), String> {
        let input =
            onda_project::resolve_project_input(&self.launch_path, ProjectLimits::default())
                .map_err(|error| error.to_string())?;
        let options = AnalysisOptions {
            sample_rate: self.options.sample_rate_hz as f32,
            block_size: self.options.block_samples,
        };
        let loaded = load_program_file(input.entry_path()).map_err(|error| {
            project_io::format_diagnostics("cannot load the current program", &error.diagnostics)
        })?;
        let inputs = project_io::resolve_compile_inputs(
            &input,
            &loaded.program,
            options,
            &self.options.overrides.constants,
        )
        .map_err(|diags| {
            project_io::format_diagnostics("invalid configuration constant", &diags)
        })?;
        // Source defaults stay in the captured code, including unevaluated arrays.
        let constants = inputs
            .constants
            .into_iter()
            .map(|(name, value)| Ok((name, project_constant(value)?)))
            .collect::<Result<BTreeMap<_, _>, String>>()?;

        let mut bindings = self
            .options
            .overrides
            .buffers
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        for buffer in &self.state.buffers {
            let Some(name) = buffer.get("name").and_then(Value::as_str) else {
                continue;
            };
            if buffer
                .get("loadedSamples")
                .and_then(Value::as_u64)
                .is_none()
            {
                bindings.insert(name.to_owned(), None);
            } else if let Some(path) = buffer.get("loadedPath").and_then(Value::as_str) {
                bindings.insert(name.to_owned(), Some(PathBuf::from(path)));
            }
        }
        // Binding acknowledgements arrive before the full display metadata refresh.
        bindings.extend(
            self.acknowledged_buffers
                .iter()
                .map(|(name, binding)| (name.clone(), binding.file_path().map(Path::to_path_buf))),
        );
        let mut assets = BTreeMap::new();
        let mut asset_file_names = BTreeMap::new();
        let mut capture = |name: String, asset, path: Option<PathBuf>| {
            if let Some(file_name) = path
                .as_deref()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
            {
                asset_file_names.insert(name.clone(), file_name.to_owned());
            }
            assets.insert(name, asset);
        };
        if let Some(project) = input.project() {
            let excluded = bindings.keys().map(String::as_str).collect();
            for (name, (asset, path)) in project
                .load_buffer_assets_excluding(ProjectLimits::default(), &excluded, None)
                .map_err(|error| error.to_string())?
            {
                capture(name, asset, path);
            }
        }
        for (name, path) in bindings {
            if let Some(path) = path {
                let asset = onda_project::load_buffer_file(&path, ProjectLimits::default())
                    .map_err(|error| format!("failed to capture buffer '{name}': {error}"))?;
                capture(name, asset, Some(path));
            }
        }
        let file_name = project_io::project_file_name_from_target(destination)?;
        let plan = project_io::package_project_plan(
            input.entry_path(),
            input.project().map(|project| project.root.as_path()),
            constants,
            assets,
            &file_name,
            &asset_file_names,
            options,
        )?;
        project_io::publish_project_plan(destination, &plan)
    }
}

fn project_constant(value: ConstValue) -> Result<ProjectConstValue, String> {
    match value {
        ConstValue::Scalar(value) => project_scalar(value),
        ConstValue::Array { values, .. } => values
            .into_iter()
            .map(project_scalar)
            .collect::<Result<Vec<_>, _>>()
            .map(ProjectConstValue::Array),
    }
}

fn project_scalar(value: TypedConstValue) -> Result<ProjectConstValue, String> {
    Ok(match value {
        TypedConstValue::Bool(value) => ProjectConstValue::Bool(value),
        TypedConstValue::I64(value) => ProjectConstValue::I64(value.to_string()),
        TypedConstValue::I32(value) | TypedConstValue::Enum { ordinal: value, .. } => {
            ProjectConstValue::Number(value.into())
        }
        TypedConstValue::F32(_) | TypedConstValue::F64(_) => ProjectConstValue::Number(
            serde_json::Number::from_f64(value.to_f64())
                .ok_or_else(|| "project constants must be finite".to_owned())?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use onda_project::{BufferAsset, BufferSamples};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(source: &str, manifest: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "onda-save-current-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("main.onda"), source).unwrap();
            fs::write(root.join("main.ondaproject"), manifest).unwrap();
            Self(root)
        }

        fn controller(&self, input: &str) -> RunController {
            let mut controller = crate::tests::controller_without_child();
            controller.launch_path = self.0.join(input);
            controller.onda_path = self.0.join("main.onda");
            controller
        }

        fn asset(&self, name: &str, value: f32) -> PathBuf {
            let path = self.0.join(name);
            let asset = BufferAsset::new(1, 1, 48000.0, BufferSamples::F32(vec![value])).unwrap();
            fs::write(&path, onda_project::encode_ondabuffer(&asset).unwrap()).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn saved_source_and_project_reopen_with_effective_constants_and_buffers() {
        let source = r#"
enum Choice:
  First
  Second
config const Count: i32 = 1
config const Wide: i64 = 9223372036854775807
config const Enabled: bool = true
config const Selected: Choice = Choice::First
config const Weights: f64[] = [0.1, -0.0]
config const Frames: i32 = BLOCK_SIZE
namespace Check:
  assert(Count == 2)
  assert(Frames == BLOCK_SIZE)
params:
  volume = 0.125
buffers:
  bank: f32 {Count}
sample:
  out1 = bank[1][0]
"#;
        let fixture = Fixture::new(
            source,
            r#"{"entry":"main.onda","constants":{"Count":1},"buffers":{"bank":[null,{"file":"missing.wav"}]}}"#,
        );
        let asset = fixture.asset("current clip.ondabuffer", 0.5);
        for input in ["main.onda", "main.ondaproject"] {
            let mut controller = fixture.controller(input);
            controller.options.block_samples = 128;
            controller.options.overrides.params =
                vec![("volume".into(), onda_daemon::RunParamValue::F32(0.75))];
            controller.state.params = vec![json!({"name":"volume", "value":1.0})];
            controller.options.overrides.constants = vec![
                ("Count".into(), "2".into()),
                ("Selected".into(), "1".into()),
                ("Wide".into(), "i64(-9223372036854775808)".into()),
                ("Enabled".into(), "false".into()),
                ("Weights".into(), "[0.1, -0.0]".into()),
            ];
            controller.options.overrides.buffers = vec![("bank[1]".into(), Some(asset.clone()))];
            let destination = fixture.0.join(format!("saved-{input}"));
            controller.save_as_project(&destination).unwrap();
            let name = project_io::project_file_name_from_target(&destination).unwrap();
            let saved = onda_project::resolve_project_input(
                destination.join(name),
                ProjectLimits::default(),
            )
            .unwrap();
            let project = saved.project().unwrap();
            let constants = &project.manifest.constants;
            assert_eq!(constants["Count"], ProjectConstValue::Number(2.into()));
            assert!(
                !constants.contains_key("Frames"),
                "source defaults stay in source"
            );
            assert_eq!(constants["Selected"], ProjectConstValue::Number(1.into()));
            assert_eq!(
                constants["Wide"],
                ProjectConstValue::I64(i64::MIN.to_string())
            );
            assert_eq!(constants["Enabled"], ProjectConstValue::Bool(false));
            let ProjectConstValue::Array(weights) = &constants["Weights"] else {
                panic!("array constant")
            };
            let ProjectConstValue::Number(zero) = &weights[1] else {
                panic!("floating constant")
            };
            assert!(zero.as_f64().unwrap().is_sign_negative());
            assert_eq!(fs::read_to_string(saved.entry_path()).unwrap(), source);

            let options = onda_daemon::RunOptions {
                block_size: 128,
                ..Default::default()
            };
            let parsed = onda_frontend::parse_program_file(saved.entry_path()).unwrap();
            let inputs = project_io::resolve_compile_inputs(
                &saved,
                &parsed,
                options.analysis_options(),
                &[],
            )
            .unwrap();
            let buffers = project
                .load_buffer_assets(ProjectLimits::default())
                .unwrap()
                .into_iter()
                .map(|(name, (asset, path))| {
                    onda_daemon::InitialBufferBinding::from_asset(name, asset, path)
                });
            let mut run = onda_daemon::RunSession::build_with_inputs_and_initial_buffers(
                &onda_daemon::AnalysisSession::new(),
                saved.entry_path(),
                options,
                &inputs,
                buffers,
            )
            .unwrap();
            let mut samples = vec![0.0; 128];
            run.render_block_interleaved(&mut samples).unwrap();
            assert_eq!(samples, vec![0.5; 128]);
        }
    }

    #[test]
    fn saving_uses_ui_replacements_and_clears_without_loading_superseded_files() {
        let fixture = Fixture::new(
            "buffers:\n  clip: f32\nsample:\n  out1 = 0.0\n",
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"missing.wav"}}}"#,
        );
        let replacement = fixture.asset("replacement.ondabuffer", 0.75);
        for clear in [false, true] {
            let mut controller = fixture.controller("main.ondaproject");
            controller.options.overrides.buffers =
                vec![("clip".into(), Some(fixture.0.join("missing-cli.wav")))];
            controller.state.buffers = vec![if clear {
                json!({"name":"clip", "loadedSamples":null, "loadedPath":null})
            } else {
                json!({"name":"clip", "loadedSamples":1, "loadedPath":replacement})
            }];
            let destination = fixture.0.join(if clear { "cleared" } else { "replaced" });
            controller.save_as_project(&destination).unwrap();
            let saved = onda_project::resolve_project_input(
                destination.join(project_io::project_file_name_from_target(&destination).unwrap()),
                ProjectLimits::default(),
            )
            .unwrap();
            let assets = saved
                .project()
                .unwrap()
                .load_buffer_assets(ProjectLimits::default())
                .unwrap();
            if clear {
                assert!(assets.is_empty());
            } else {
                assert!(
                    matches!(&assets["clip"].0.samples, BufferSamples::F32(values) if values == &[0.75])
                );
            }
        }
    }

    #[test]
    fn saving_preserves_lazy_and_nonfinite_source_defaults() {
        let source = "config const Unused: i32[1] = [1 / 0]\nconfig const Nonfinite: f32 = 0.0 / 0.0\nconfig const Infinite: f64 = 1.0 / 0.0\nconfig const Selected: i32 = 1\nsample:\n  out1 = f32(Selected)\n";
        let fixture = Fixture::new(
            source,
            r#"{"entry":"main.onda","constants":{"Selected":2}}"#,
        );
        // Ordinary compilation admits these source defaults without demanding Unused.
        let mut original = onda_daemon::RunSession::build(
            &onda_daemon::AnalysisSession::new(),
            fixture.0.join("main.onda"),
            Default::default(),
        )
        .unwrap();
        let mut samples = vec![0.0; original.options().block_size];
        original.render_block_interleaved(&mut samples).unwrap();
        assert!(samples.iter().all(|value| *value == 1.0));
        for (input, cli_override, expected) in [
            ("main.onda", false, 1),
            ("main.onda", true, 3),
            ("main.ondaproject", false, 2),
            ("main.ondaproject", true, 3),
        ] {
            let mut controller = fixture.controller(input);
            if cli_override {
                controller.options.overrides.constants = vec![("Selected".into(), "3".into())];
            }
            let destination = fixture.0.join(format!("lazy-{input}-{cli_override}"));
            controller.save_as_project(&destination).unwrap();
            let saved = onda_project::resolve_project_input(
                destination.join(project_io::project_file_name_from_target(&destination).unwrap()),
                ProjectLimits::default(),
            )
            .unwrap();
            let constants = &saved.project().unwrap().manifest.constants;
            let expected_constants = if expected == 1 {
                BTreeMap::new()
            } else {
                BTreeMap::from([(
                    "Selected".into(),
                    ProjectConstValue::Number(expected.into()),
                )])
            };
            assert_eq!(constants, &expected_constants);
            assert_eq!(fs::read_to_string(saved.entry_path()).unwrap(), source);
            let options = onda_daemon::RunOptions::default();
            let parsed = onda_frontend::parse_program_file(saved.entry_path()).unwrap();
            let inputs = project_io::resolve_compile_inputs(
                &saved,
                &parsed,
                options.analysis_options(),
                &[],
            )
            .unwrap();
            let mut reopened = onda_daemon::RunSession::build_with_inputs_and_initial_buffers(
                &onda_daemon::AnalysisSession::new(),
                saved.entry_path(),
                options,
                &inputs,
                [],
            )
            .unwrap();
            reopened.render_block_interleaved(&mut samples).unwrap();
            assert!(samples.iter().all(|value| *value == expected as f32));
        }
    }

    #[test]
    fn saving_uses_binding_acknowledgements_before_metadata_refresh() {
        let fixture = Fixture::new(
            "buffers:\n  clip: f32\nsample:\n  out1 = 0.0\n",
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"old.ondabuffer"}}}"#,
        );
        let old = fixture.asset("old.ondabuffer", 0.25);
        let replacement = fixture.asset("replacement.ondabuffer", 0.75);
        for input in ["main.onda", "main.ondaproject"] {
            for (bind, success) in [(false, true), (true, true), (false, false), (true, false)] {
                let mut controller = fixture.controller(input);
                let initially_bound = !bind || !success;
                controller.options.overrides.buffers =
                    vec![("clip".into(), initially_bound.then(|| old.clone()))];
                controller.state.buffers = vec![json!({
                    "name":"clip", "loadedSamples":initially_bound.then_some(1),
                    "loadedPath":initially_bound.then_some(&old),
                })];
                controller.pending_commands.insert(
                    7,
                    if bind {
                        PendingCommand::BindBuffer {
                            name: "clip".into(),
                            path: replacement.to_str().unwrap().into(),
                        }
                    } else {
                        PendingCommand::ClearBuffer {
                            name: "clip".into(),
                        }
                    },
                );

                for acknowledged in [false, true] {
                    if acknowledged {
                        controller.handle_tcp_response(
                            &json!({"id":7,"ok":success,"error":"rejected"}).to_string(),
                        );
                    }
                    // Do not supply getBuffers: its loadedSamples reply is still in flight.
                    assert_eq!(
                        controller.state.buffers[0]["loadedSamples"],
                        json!(initially_bound.then_some(1))
                    );
                    let destination = fixture
                        .0
                        .join(format!("ack-{input}-{bind}-{success}-{acknowledged}"));
                    controller.save_as_project(&destination).unwrap();
                    let saved = onda_project::resolve_project_input(
                        destination
                            .join(project_io::project_file_name_from_target(&destination).unwrap()),
                        ProjectLimits::default(),
                    )
                    .unwrap();
                    let assets = saved
                        .project()
                        .unwrap()
                        .load_buffer_assets(ProjectLimits::default())
                        .unwrap();
                    let expected = if acknowledged && success {
                        bind.then_some(0.75)
                    } else {
                        initially_bound.then_some(0.25)
                    };
                    if let Some(value) = expected {
                        assert!(
                            matches!(&assets["clip"].0.samples, BufferSamples::F32(values) if values == &[value])
                        );
                    } else {
                        assert!(assets.is_empty(), "cleared buffer was saved again");
                    }
                }
            }
        }
    }

    #[test]
    fn saving_retains_accepted_bindings_when_interrupted_replacements_are_rejected() {
        let fixture = Fixture::new(
            "buffers:\n  clip: f32\nsample:\n  out1 = clip[0]\n",
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"old.ondabuffer"}}}"#,
        );
        let old = fixture.asset("old.ondabuffer", 0.25);
        let replacement = fixture.0.join("rejected.ondabuffer");
        let asset = BufferAsset::new(1, 1, 48000.0, BufferSamples::I32(vec![7])).unwrap();
        fs::write(
            &replacement,
            onda_project::encode_ondabuffer(&asset).unwrap(),
        )
        .unwrap();

        for input in ["main.onda", "main.ondaproject"] {
            for initially_bound in [false, true] {
                let mut controller = fixture.controller(input);
                controller.state.buffers = vec![json!({
                    "name":"clip", "loadedSamples":1, "loadedPath":old,
                })];
                controller.pending_commands.insert(
                    1,
                    if initially_bound {
                        PendingCommand::BindBuffer {
                            name: "clip".into(),
                            path: old.to_str().unwrap().into(),
                        }
                    } else {
                        PendingCommand::ClearBuffer {
                            name: "clip".into(),
                        }
                    },
                );
                controller.handle_tcp_response(r#"{"id":1,"ok":true}"#);
                controller.pending_commands.insert(
                    2,
                    if initially_bound {
                        PendingCommand::ClearBuffer {
                            name: "clip".into(),
                        }
                    } else {
                        PendingCommand::BindBuffer {
                            name: "clip".into(),
                            path: replacement.to_str().unwrap().into(),
                        }
                    },
                );
                // A restart retains the pending selection for the replacement child.
                controller.preserve_pending_buffer_bindings();
                controller.pending_commands.clear();
                let requested = controller.preserved_buffers["clip"].clone();
                let (_, _, retry) = requested.replay_request("clip");
                controller.pending_commands.insert(3, retry);
                controller.restoring = true;
                controller.handle_tcp_response(r#"{"id":3,"ok":false,"error":"rejected"}"#);
                assert_eq!(controller.state.status, "Restoration failed");
                assert_eq!(controller.preserved_buffers["clip"], requested);

                let destination = fixture
                    .0
                    .join(format!("rejected-{input}-{initially_bound}"));
                controller.save_as_project(&destination).unwrap();
                let saved = onda_project::resolve_project_input(
                    destination
                        .join(project_io::project_file_name_from_target(&destination).unwrap()),
                    ProjectLimits::default(),
                )
                .unwrap();
                let assets = saved
                    .project()
                    .unwrap()
                    .load_buffer_assets(ProjectLimits::default())
                    .unwrap();
                if initially_bound {
                    assert!(
                        matches!(&assets["clip"].0.samples, BufferSamples::F32(values) if values == &[0.25])
                    );
                } else {
                    assert!(assets.is_empty(), "the rejected replacement was saved");
                }
            }
        }
    }
}
