use std::io::{self, BufRead, BufWriter, Write};
use std::path::Path;

use onda_daemon::{
    DaemonConfig, DaemonSession, DocumentVersion, RunBuildError, RunDelegateBatch, RunDelegateInfo,
    RunOptions, RunParamInfo, RunPrintBatch, INTERACTIVE_PARAM_SMOOTHING_SECONDS,
};
use onda_frontend::Diagnostic;
use onda_run::{event_value_from_json, event_value_to_json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn run_stdio_loop() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut session = DaemonSession::new(DaemonConfig::for_run(RunOptions {
        param_smoothing_seconds: INTERACTIVE_PARAM_SMOOTHING_SECONDS,
        ..RunOptions::default()
    }));
    let mut writer = BufWriter::new(stdout.lock());

    for line in stdin.lock().lines() {
        let line = line.map_err(|err| format!("failed to read daemon stdio input: {err}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RequestEnvelope>(&line) {
            Ok(request) => handle_request(&mut session, request),
            Err(err) => ResponseEnvelope::error(None, format!("invalid request json: {err}")),
        };
        serde_json::to_writer(&mut writer, &response)
            .map_err(|err| format!("failed to write daemon stdio response: {err}"))?;
        writer
            .write_all(b"\n")
            .map_err(|err| format!("failed to write daemon stdio newline: {err}"))?;
        writer
            .flush()
            .map_err(|err| format!("failed to flush daemon stdio output: {err}"))?;
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct RequestEnvelope {
    #[serde(default)]
    id: Option<u64>,
    #[serde(flatten)]
    request: Request,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Request {
    Ping,
    Initialize {
        #[serde(default)]
        sample_rate_hz: Option<u32>,
        #[serde(default)]
        block_samples: Option<usize>,
        #[serde(default)]
        param_smoothing_seconds: Option<f64>,
        #[serde(default)]
        fast_math: Option<bool>,
    },
    Open {
        path: String,
        version: i32,
        text: String,
    },
    Update {
        path: String,
        version: i32,
        text: String,
    },
    Close {
        path: String,
    },
    Diagnose {
        path: String,
    },
    RunSources {
        path: String,
    },
    RunMetadata {
        path: String,
    },
    RunStart {
        path: String,
    },
    RunStop {
        path: String,
    },
    RunParams {
        path: String,
    },
    RunSetParam {
        path: String,
        name: String,
        value: Value,
    },
    RunBindBuffer {
        path: String,
        name: String,
        samples: Vec<f32>,
        channels: usize,
        sample_rate_hz: f32,
    },
    RunRender {
        path: String,
        #[serde(default)]
        include_sample_bits: bool,
    },
    RunRenderSegments {
        path: String,
        segments: Vec<ProcessSegmentRequest>,
        #[serde(default)]
        include_sample_bits: bool,
    },
    RunTriggerEvent {
        path: String,
        name: String,
        values: Vec<Value>,
    },
    RunSnapshot {
        path: String,
    },
    RunRestore {
        path: String,
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct ProcessSegmentRequest {
    start_frame: usize,
    frames: usize,
    flags: u32,
}

#[derive(Debug, Serialize)]
struct ResponseEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl ResponseEnvelope {
    fn ok(id: Option<u64>, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<u64>, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error.into()),
        }
    }

    fn error_with_result(id: Option<u64>, error: impl Into<String>, result: Value) -> Self {
        Self {
            id,
            ok: false,
            result: Some(result),
            error: Some(error.into()),
        }
    }
}

fn handle_request(session: &mut DaemonSession, envelope: RequestEnvelope) -> ResponseEnvelope {
    let id = envelope.id;
    let mut error_result = None;
    let result = match envelope.request {
        Request::Ping => Ok(json!({ "status": "ok" })),
        Request::Initialize {
            sample_rate_hz,
            block_samples,
            param_smoothing_seconds,
            fast_math,
        } => {
            let current = session.config();
            let param_smoothing_seconds =
                param_smoothing_seconds.unwrap_or(current.run.param_smoothing_seconds);
            if !param_smoothing_seconds.is_finite() || param_smoothing_seconds < 0.0 {
                return ResponseEnvelope::error(
                    id,
                    "param_smoothing_seconds must be finite and non-negative",
                );
            }
            let run = RunOptions {
                sample_rate: sample_rate_hz.unwrap_or(current.run.sample_rate as u32) as f32,
                block_size: block_samples.unwrap_or(current.run.block_size),
                param_smoothing_seconds,
                fast_math: fast_math.unwrap_or(current.run.fast_math),
                opt_level: current.run.opt_level,
            };
            let config = DaemonConfig::for_run(run);
            session.set_config(config);
            Ok(json!({
                "sample_rate_hz": run.sample_rate,
                "block_samples": run.block_size,
                "param_smoothing_seconds": run.param_smoothing_seconds,
                "fast_math": run.fast_math,
            }))
        }
        Request::Open {
            path,
            version,
            text,
        } => {
            let path = session.open_document(path, DocumentVersion(version), text);
            Ok(json!({
                "path": display_path(&path),
                "version": version,
            }))
        }
        Request::Update {
            path,
            version,
            text,
        } => {
            let path = session.update_document(path, DocumentVersion(version), text);
            Ok(json!({
                "path": display_path(&path),
                "version": version,
            }))
        }
        Request::Close { path } => {
            let removed = session.close_document(path).is_some();
            Ok(json!({ "closed": removed }))
        }
        Request::Diagnose { path } => {
            let snapshot = session.analyze_document(path);
            Ok(json!({
                "path": display_path(&snapshot.path),
                "version": snapshot.version.map(|v| v.0),
                "ok": snapshot.diagnostics.is_empty(),
                "diagnostics": snapshot.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
            }))
        }
        Request::RunSources { path } => {
            serde_json::to_value(onda_run::run_source_graph(Path::new(&path)))
                .map_err(|error| format!("failed to encode source graph: {error}"))
        }
        Request::RunMetadata { path } => (|| {
            let options = session.config().run;
            let mut project = crate::project_cmd::resolve_run_project(
                Path::new(&path),
                &[],
                options.analysis_options(),
                true,
            )
            .map_err(|error| {
                if let Some(sources) = error.sources {
                    error_result = Some(json!({ "sources": sources }));
                }
                error.message
            })?;
            let buffers = project.buffers.into_iter().map(|(name, asset, path)| {
                onda_daemon::InitialBufferBinding::from_asset(name, asset, path)
            });
            // Metadata queries must not register or replace an active run.
            let run = onda_daemon::RunSession::build_with_inputs_and_initial_buffers(
                session.analysis(),
                &project.entry,
                options,
                &project.compile_inputs,
                buffers,
            )
            .map_err(|error| {
                error_result = Some(json!({
                    "sources": onda_run::RunSourceGraph::from_failed_sources(
                        &error.sources,
                        project.file_digests.take().expect("source reporting requested"),
                    ),
                }));
                run_build_error_string("run_metadata failed", &error)
            })?;
            let mut sources = onda_run::RunSourceGraph::from_compiled_sources(
                run.source_manifest(),
                project.file_digests.expect("source reporting requested"),
            );
            let mut metadata = onda_run::run_session_metadata(&run).inspect_err(|_| {
                sources.complete = false;
                error_result = Some(json!({ "sources": sources }));
            })?;
            metadata["sources"] = serde_json::to_value(sources)
                .map_err(|error| format!("failed to encode source graph: {error}"))?;
            Ok(metadata)
        })(),
        Request::RunStart { path } => match session.start_run(&path) {
            Ok(run) => {
                let result = json!({
                    "path": display_path(run.path()),
                    "version": run.version().map(|v| v.0),
                    "params": run.param_info().iter().map(run_param_json).collect::<Vec<_>>(),
                    "delegates": run.delegate_info().iter().map(run_delegate_json).collect::<Vec<_>>(),
                    "output_channels": run.output_channel_count(),
                });
                let run = session
                    .run_mut(&path)
                    .expect("run should be active after successful start");
                run.set_delegate_collection_enabled(true);
                let prints = run
                    .take_print_batch()
                    .map_err(|diag| diagnostic_string("run_start print decoding failed", &diag));
                prints.map(|prints| attach_run_print_batch(result, &prints))
            }
            Err(err) => Err(run_build_error_string("run_start failed", &err)),
        },
        Request::RunStop { path } => {
            let stopped = session.stop_run(path).is_some();
            Ok(json!({ "stopped": stopped }))
        }
        Request::RunParams { path } => session
            .run(path)
            .map(|run| {
                json!({
                    "params": run.param_info().iter().map(run_param_json).collect::<Vec<_>>(),
                    "output_channels": run.output_channel_count(),
                })
            })
            .ok_or_else(|| "run is not active".to_owned()),
        Request::RunSetParam { path, name, value } => session
            .run_mut(path)
            .ok_or_else(|| "run is not active".to_owned())
            .and_then(|run| {
                run.set_param(&name, onda_run::param_value_from_json(&value)?)
                    .map_err(|diag| diagnostic_string("run_set_param failed", &diag))
            })
            .map(|_| json!({ "status": "ok" })),
        Request::RunBindBuffer {
            path,
            name,
            samples,
            channels,
            sample_rate_hz,
        } => (|| {
            let run = session
                .run_mut(path)
                .ok_or_else(|| "run is not active".to_owned())?;
            let execution = run.bind_buffer_samples(&name, samples, channels, sample_rate_hz);
            let prints = run.take_print_batch().map_err(|diag| {
                diagnostic_string("run_bind_buffer print decoding failed", &diag)
            })?;
            match execution {
                Ok(()) => Ok(attach_run_print_batch(json!({ "status": "ok" }), &prints)),
                Err(diag) => {
                    error_result = Some(attach_run_print_batch(
                        json!({ "status": "failed" }),
                        &prints,
                    ));
                    Err(diagnostic_string("run_bind_buffer failed", &diag))
                }
            }
        })(),
        Request::RunRender {
            path,
            include_sample_bits,
        } => (|| {
            let run = session
                .run_mut(path)
                .ok_or_else(|| "run is not active".to_owned())?;
            let execution = run.render_block();
            let batch = run
                .take_delegate_batch()
                .map_err(|diag| diagnostic_string("run_render delegate decoding failed", &diag))?;
            let prints = run
                .take_print_batch()
                .map_err(|diag| diagnostic_string("run_render print decoding failed", &diag))?;
            match execution {
                Ok(channels) => Ok(attach_run_print_batch(
                    attach_run_delegate_batch(
                        rendered_channels_json(channels, include_sample_bits),
                        &batch,
                    ),
                    &prints,
                )),
                Err(diag) => {
                    error_result = Some(attach_run_print_batch(
                        attach_run_delegate_batch(json!({ "status": "failed" }), &batch),
                        &prints,
                    ));
                    Err(diagnostic_string("run_render failed", &diag))
                }
            }
        })(),
        Request::RunRenderSegments {
            path,
            segments,
            include_sample_bits,
        } => (|| {
            let run = session
                .run_mut(path)
                .ok_or_else(|| "run is not active".to_owned())?;
            let segments = segments
                .into_iter()
                .map(|segment| (segment.start_frame, segment.frames, segment.flags))
                .collect::<Vec<_>>();
            let execution = run.render_block_segments(&segments);
            let batch = run.take_delegate_batch().map_err(|diag| {
                diagnostic_string("run_render_segments delegate decoding failed", &diag)
            })?;
            let prints = run.take_print_batch().map_err(|diag| {
                diagnostic_string("run_render_segments print decoding failed", &diag)
            })?;
            match execution {
                Ok(channels) => Ok(attach_run_print_batch(
                    attach_run_delegate_batch(
                        rendered_channels_json(channels, include_sample_bits),
                        &batch,
                    ),
                    &prints,
                )),
                Err(diag) => {
                    error_result = Some(attach_run_print_batch(
                        attach_run_delegate_batch(json!({ "status": "failed" }), &batch),
                        &prints,
                    ));
                    Err(diagnostic_string("run_render_segments failed", &diag))
                }
            }
        })(),
        Request::RunTriggerEvent { path, name, values } => (|| {
            let run = session
                .run_mut(path)
                .ok_or_else(|| "run is not active".to_owned())?;
            let values = values
                .into_iter()
                .map(event_value_from_json)
                .collect::<Result<Vec<_>, _>>()?;
            let execution = run.trigger_event(&name, &values);
            let batch = run.take_delegate_batch().map_err(|diag| {
                diagnostic_string("run_trigger_event delegate decoding failed", &diag)
            })?;
            let prints = run.take_print_batch().map_err(|diag| {
                diagnostic_string("run_trigger_event print decoding failed", &diag)
            })?;
            match execution {
                Ok(()) => Ok(attach_run_print_batch(
                    attach_run_delegate_batch(json!({ "status": "ok" }), &batch),
                    &prints,
                )),
                Err(diag) => {
                    error_result = Some(attach_run_print_batch(
                        attach_run_delegate_batch(json!({ "status": "failed" }), &batch),
                        &prints,
                    ));
                    Err(diagnostic_string("run_trigger_event failed", &diag))
                }
            }
        })(),
        Request::RunSnapshot { path } => session
            .run(path)
            .ok_or_else(|| "run is not active".to_owned())
            .and_then(|run| {
                run.snapshot_state_bytes()
                    .map(|bytes| json!({ "bytes": bytes }))
                    .map_err(|diag| diagnostic_string("run_snapshot failed", &diag))
            }),
        Request::RunRestore { path, bytes } => session
            .run_mut(path)
            .ok_or_else(|| "run is not active".to_owned())
            .and_then(|run| {
                run.restore_state_bytes(&bytes)
                    .map_err(|diag| diagnostic_string("run_restore failed", &diag))
            })
            .map(|_| json!({ "status": "ok" })),
    };

    match result {
        Ok(result) => ResponseEnvelope::ok(id, result),
        Err(err) => match error_result {
            Some(result) => ResponseEnvelope::error_with_result(id, err, result),
            None => ResponseEnvelope::error(id, err),
        },
    }
}

fn rendered_channels_json(channels: Vec<Vec<f32>>, include_sample_bits: bool) -> Value {
    let frames = channels.first().map(Vec::len).unwrap_or(0);
    let channel_bits = include_sample_bits.then(|| {
        channels
            .iter()
            .map(|channel| {
                channel
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    });
    let mut result = json!({
        "frames": frames,
        "channels": channels,
    });
    if let Some(channel_bits) = channel_bits {
        result["channel_bits"] = json!(channel_bits);
    }
    result
}

fn run_delegate_json(delegate: &RunDelegateInfo) -> Value {
    json!({
        "index": delegate.index,
        "name": delegate.name,
        "params": delegate.params.iter().map(|param| json!({
            "index": param.index,
            "name": param.name,
            "type_repr": param.type_repr,
        })).collect::<Vec<_>>(),
    })
}

fn attach_run_delegate_batch(mut result: Value, batch: &RunDelegateBatch) -> Value {
    let occurrences = batch
        .occurrences
        .iter()
        .map(|occurrence| {
            let values = occurrence
                .values
                .iter()
                .map(|entry| (entry.name.clone(), event_value_to_json(&entry.value)))
                .collect::<serde_json::Map<_, _>>();
            json!({
                "sequence": occurrence.sequence,
                "index": occurrence.index,
                "name": occurrence.name,
                "values": values,
            })
        })
        .collect::<Vec<_>>();
    result["delegate_occurrences"] = Value::Array(occurrences);
    result["delegate_overflow_count"] = json!(batch.overflow_count);
    result
}

fn attach_run_print_batch(mut result: Value, batch: &RunPrintBatch) -> Value {
    result["print"] = json!({
        "text": batch.text,
        "entries": batch.entries.iter().map(|entry| json!({
            "sequence": entry.sequence,
            "site_index": entry.site_index,
            "label": entry.label,
            "source": {
                "file": entry.source_file,
                "line": entry.line,
                "column": entry.column,
                "end_line": entry.end_line,
                "end_column": entry.end_column,
            },
            "lexical_owner": entry.lexical_owner,
            "declaration": entry.declaration,
            "values": entry.values.iter().map(|value| json!({
                "type": value.type_repr,
                "value": event_value_to_json(&value.value),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "overflow_count": batch.overflow_count,
        "transport_drop_count": batch.transport_drop_count,
    });
    result
}

fn diagnostic_json(diag: &Diagnostic) -> Value {
    json!({
        "code": diag.code as i32,
        "message": diag.message.clone(),
        "line": diag.line,
        "column": diag.column,
        "end_line": diag.end_line,
        "end_column": diag.end_column,
        "file": diag.file.clone(),
        "trace": diag.trace.clone(),
    })
}

fn run_param_json(param: &RunParamInfo) -> Value {
    json!({
        "index": param.index,
        "name": param.name.clone(),
        "value": param.value.map(onda_run::param_value_to_json),
        "type_repr": param.type_repr.clone(),
        "default": param.default.map(onda_run::param_value_to_json),
        "range_min": param.range_min,
        "range_max": param.range_max,
        "scalar": param.scalar,
    })
}

fn run_build_error_string(context: &str, err: &RunBuildError) -> String {
    diagnostics_string(context, &err.diagnostics)
}

fn diagnostics_string(context: &str, diagnostics: &[Diagnostic]) -> String {
    let messages = diagnostics
        .iter()
        .map(diagnostic_summary)
        .collect::<Vec<_>>()
        .join(" | ");
    format!("{context}: {messages}")
}

fn diagnostic_string(context: &str, diag: &Diagnostic) -> String {
    format!("{context}: {}", diagnostic_summary(diag))
}

fn diagnostic_summary(diag: &Diagnostic) -> String {
    let location = match diag.file.as_deref() {
        Some(file) if diag.line > 0 => format!("{file}:{}:{}", diag.line, diag.column.max(1)),
        Some(file) => file.to_owned(),
        None if diag.line > 0 => format!("{}:{}", diag.line, diag.column.max(1)),
        None => "0:0".to_owned(),
    };
    format!("{location} [{}] {}", diag.code as i32, diag.message)
}

fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use onda_daemon::RunEventValue;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onda_cli_daemon_stdio_{prefix}_{nanos}"));
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
    fn project_resolution_failures_report_consumed_files() {
        for (name, manifest, source, asset, failed_file) in [
            ("manifest", "{", "", None, "project.ondaproject"),
            (
                "entry",
                r#"{"entry":"missing.onda"}"#,
                "",
                None,
                "missing.onda",
            ),
            (
                "parse",
                r#"{"entry":"main.onda","constants":{"Selected":1}}"#,
                "import missing\nconfig const Selected: i32 = 0\n",
                None,
                "main.onda",
            ),
            (
                "constant",
                r#"{"entry":"main.onda","constants":{"Unknown":1}}"#,
                "config const Selected: i32 = 0\n",
                None,
                "main.onda",
            ),
            (
                "asset",
                r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.ondabuffer"}}}"#,
                "buffers:\n  clip: buffer<f32>\nsample:\n  out1 = clip[0]\n",
                Some(b"invalid".as_slice()),
                "clip.ondabuffer",
            ),
            (
                "missing_asset",
                r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.ondabuffer"}}}"#,
                "buffers:\n  clip: buffer<f32>\nsample:\n  out1 = clip[0]\n",
                None,
                "clip.ondabuffer",
            ),
        ] {
            let dir = fs::canonicalize(mk_temp_dir(name)).unwrap();
            let input = dir.join("project.ondaproject");
            write_file(&input, manifest);
            write_file(&dir.join("main.onda"), source);
            if let Some(asset) = asset {
                fs::write(dir.join("clip.ondabuffer"), asset).unwrap();
            }
            let observed = onda_run::run_source_graph(&input);
            let mut session = DaemonSession::default();
            let response = handle_request(
                &mut session,
                RequestEnvelope {
                    id: Some(1),
                    request: Request::RunMetadata {
                        path: input.to_string_lossy().into_owned(),
                    },
                },
            );
            assert!(!response.ok, "{name} must fail");
            let graph = &response
                .result
                .expect("resolution failure must carry sources")["sources"];
            assert_eq!(graph["complete"], false, "{name}");
            for file in [&input, &dir.join(failed_file)] {
                assert!(
                    graph["files"].as_array().unwrap().contains(&json!(file)),
                    "{name}: {}",
                    file.display()
                );
                assert_eq!(
                    graph["contentHashes"][file.to_str().unwrap()],
                    json!(observed.content_hashes[file]),
                    "{name}"
                );
            }
            if name == "parse" {
                assert!(graph["files"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(dir.join("missing.onda"))));
            }
            assert!(session.run(dir.join("main.onda")).is_none());
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn run_sources_reports_failed_imports_without_starting_audio() {
        let dir = mk_temp_dir("run_sources");
        let main = dir.join("main.onda");
        write_file(&main, "import missing\n");
        let mut session = DaemonSession::default();
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::RunSources {
                    path: display_path(&main),
                },
            },
        );
        assert!(response.ok);
        let graph = response.result.unwrap();
        assert_eq!(graph["complete"], false);
        assert!(graph["files"]
            .as_array()
            .unwrap()
            .contains(&json!(display_path(&dir.join("missing.onda")))));
        assert!(session.run(&main).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_metadata_reports_the_consumed_sources() {
        let dir = mk_temp_dir("failed_metadata_sources");
        let main = dir.join("main.onda");
        let original = "sample:\n  out1 = 1.0\n";
        write_file(&main, original);
        let preliminary = onda_run::run_source_graph(&main);
        let mut session = DaemonSession::default();
        // An overlay makes the transient compiler read deterministic while disk
        // already contains the restored version the editor will validate.
        session.open_document(
            &main,
            DocumentVersion(1),
            "import missing\nsample:\n  out1 = unknown\n",
        );
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::RunMetadata {
                    path: display_path(&main),
                },
            },
        );
        assert!(!response.ok);
        let result = response
            .result
            .expect("failure must carry its source graph");
        assert_eq!(result["sources"]["complete"], false);
        assert_ne!(
            result["sources"]["contentHashes"][display_path(&main)],
            json!(preliminary.content_hashes[&main]),
        );
        assert!(result["sources"]["files"]
            .as_array()
            .unwrap()
            .contains(&json!(display_path(&dir.join("missing.onda")))));
        assert!(session.run(&main).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn host_event_validation_failure_reports_the_consumed_sources() {
        let dir = fs::canonicalize(mk_temp_dir("invalid_host_event_sources")).unwrap();
        let main = dir.join("main.onda");
        let original = "sample:\n  out1 = 1.0\n";
        let transient = "import voice\nevent note_on(value: f32):\n  print(value)\nsample:\n  out1 = voice_gain\n";
        write_file(&main, transient);
        write_file(&dir.join("voice.onda"), "const voice_gain = 0.5\n");
        let mut expected = onda_run::run_source_graph(&main);
        expected.complete = false;
        write_file(&main, original);

        let mut session = DaemonSession::default();
        session.open_document(&main, DocumentVersion(1), transient);
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::RunMetadata {
                    path: display_path(&main),
                },
            },
        );
        assert!(!response.ok);
        assert!(response
            .error
            .unwrap()
            .contains("canonical host event 'note_on'"));
        assert_eq!(
            response
                .result
                .expect("host validation failure must carry sources")["sources"],
            serde_json::to_value(expected).unwrap(),
        );
        assert!(session.run(&main).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_metadata_compiles_project_constants_without_audio() {
        let dir = mk_temp_dir("run_metadata_project");
        write_file(&dir.join("main.onda"), "config const Selected: i32 = 1\nparams:\n  gain = f32(Selected)\nsample:\n  out1 = gain\n");
        let manifest = dir.join("project.ondaproject");
        write_file(
            &manifest,
            r#"{"entry":"main.onda","constants":{"Selected":7}}"#,
        );
        let mut session = DaemonSession::default();
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::RunMetadata {
                    path: display_path(&manifest),
                },
            },
        );
        assert!(response.ok, "{:?}", response.error);
        let result = response.result.unwrap();
        assert_eq!(result["outputChannels"], 1);
        assert_eq!(result["params"][0]["defaultRepr"], "7");
        assert!(session.run(dir.join("main.onda")).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_metadata_reports_dependencies_added_after_discovery() {
        let dir = mk_temp_dir("run_metadata_sources");
        let main = dir.join("main.onda");
        let voice = dir.join("voice.onda");
        let child = dir.join("child.onda");
        write_file(
            &main,
            "import voice\nparams:\n  gain = voice_gain\nsample:\n  out1 = gain\n",
        );
        write_file(&voice, "const voice_gain = 1.0\n");
        let mut session = DaemonSession::default();
        let request = |session: &mut DaemonSession, command: &str| {
            let response = handle_request(
                session,
                serde_json::from_value(json!({
                    "command": command, "path": display_path(&main),
                }))
                .unwrap(),
            );
            assert!(response.ok, "{:?}", response.error);
            response.result.unwrap()
        };
        let discovered = request(&mut session, "run_sources");
        write_file(&voice, "import child\nconst voice_gain = child_gain\n");
        write_file(&child, "const child_gain = 9.0\n");
        let metadata = request(&mut session, "run_metadata");
        assert_eq!(metadata["params"][0]["defaultRepr"], "9");
        let sources = &metadata["sources"];
        assert_eq!(sources, &request(&mut session, "run_sources"));
        assert!(sources["files"]
            .as_array()
            .unwrap()
            .contains(&json!(display_path(&child))));
        assert_ne!(sources["contentHashes"], discovered["contentHashes"]);
        assert!(session.run(&main).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_parameter_json_preserves_wide_integers_and_large_floats() {
        let dir = mk_temp_dir("run_parameter_precision");
        let main = dir.join("main.onda");
        write_file(&main, "params:\n  integer: i64 = 9007199254740993\n  floating: f64 = 0\nsample:\n  out1 = f32(integer - 9007199254740992)\n  out2 = f32(floating / 10000000000000000000.0)\n");
        let path = display_path(&main);
        let mut session = DaemonSession::new(DaemonConfig::for_run(RunOptions {
            block_size: 1,
            param_smoothing_seconds: 0.0,
            ..RunOptions::default()
        }));
        let request = |session: &mut DaemonSession, command: Value| {
            let response = handle_request(session, serde_json::from_value(command).unwrap());
            assert!(response.ok, "{:?}", response.error);
            response.result.unwrap()
        };
        request(
            &mut session,
            json!({ "command": "run_start", "path": path }),
        );
        for (name, value) in [
            ("integer", json!("9007199254740995")),
            ("floating", json!(10_000_000_000_000_000_000_u64)),
        ] {
            request(
                &mut session,
                json!({ "command": "run_set_param", "path": path, "name": name, "value": value }),
            );
        }
        let rendered = request(
            &mut session,
            json!({ "command": "run_render", "path": path }),
        );
        assert_eq!(rendered["channels"], json!([[3.0], [1.0]]));
        let params = request(
            &mut session,
            json!({ "command": "run_params", "path": path }),
        );
        assert_eq!(params["params"][0]["value"], "9007199254740995");
        assert_eq!(params["params"][0]["default"], "9007199254740993");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_metadata_preserves_active_run_state() {
        let dir = mk_temp_dir("run_metadata_active");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "buffers:\n  src: buffer<f32>\nparams:\n  gain = 1.0\ndelegate report(value: f32)\nevent trigger():\n  report(gain)\ninit:\n  phase = 0.0\nsample:\n  phase = phase + 1.0\n  out1 = phase + gain + src[0]\n",
        );
        let path = display_path(&main);
        let mut session = DaemonSession::new(DaemonConfig::for_run(RunOptions {
            block_size: 1,
            param_smoothing_seconds: 0.0,
            ..RunOptions::default()
        }));
        let request = |session: &mut DaemonSession, command: Value| {
            let response = handle_request(
                session,
                serde_json::from_value(command).expect("valid request"),
            );
            assert!(response.ok, "request failed: {:?}", response.error);
            response.result.expect("request result")
        };

        request(
            &mut session,
            json!({ "command": "run_start", "path": path }),
        );
        request(
            &mut session,
            json!({
                "command": "run_bind_buffer", "path": path, "name": "src",
                "samples": [0.5], "channels": 1, "sample_rate_hz": 48000.0,
            }),
        );
        request(
            &mut session,
            json!({ "command": "run_set_param", "path": path, "name": "gain", "value": 2.0 }),
        );
        let before = request(
            &mut session,
            json!({ "command": "run_render", "path": path }),
        );
        assert_eq!(before["channels"], json!([[3.5]]));
        let snapshot = session.run(&main).unwrap().snapshot_state_bytes().unwrap();

        let metadata = request(
            &mut session,
            json!({ "command": "run_metadata", "path": path }),
        );
        assert_eq!(metadata["params"][0]["defaultRepr"], "1");
        assert_eq!(metadata["buffers"][0]["loadedFrames"], Value::Null);
        let run = session.run(&main).expect("active run retained");
        assert_eq!(run.snapshot_state_bytes().unwrap(), snapshot);
        assert_eq!(run.buffer_info()[0].loaded_frames, Some(1));
        let after = request(
            &mut session,
            json!({ "command": "run_render", "path": path }),
        );
        assert_eq!(after["channels"], json!([[4.5]]));
        let event = request(
            &mut session,
            json!({ "command": "run_trigger_event", "path": path, "name": "trigger", "values": [] }),
        );
        assert_eq!(event["delegate_occurrences"][0]["values"]["value"], 2.0);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn initialize_updates_session_config() {
        let mut session = DaemonSession::default();
        let response = handle_request(
            &mut session,
            serde_json::from_value(json!({
                "id": 1,
                "command": "initialize",
                "sample_rate_hz": 44_100,
                "block_samples": 256,
                "param_smoothing_seconds": 0.03,
                "fast_math": true,
            }))
            .expect("initialize request"),
        );

        assert!(response.ok);
        assert_eq!(
            response.result.expect("initialize result")["block_samples"],
            256
        );
        assert_eq!(session.config().analysis.block_size, 256);
        assert_eq!(session.config().analysis.sample_rate, 44_100.0);
        assert_eq!(session.config().run.param_smoothing_seconds, 0.03);
        assert!(session.config().run.fast_math);
    }

    #[test]
    fn daemon_json_preserves_decimal_i64_event_values() {
        let value = event_value_from_json(Value::String("9007199254740993".to_owned()))
            .expect("decimal i64 input should parse");
        assert_eq!(value, RunEventValue::I64(9_007_199_254_740_993));
        assert_eq!(
            event_value_to_json(&value),
            Value::String("9007199254740993".to_owned())
        );
        assert!(event_value_from_json(Value::String("1.5".to_owned())).is_err());
    }

    #[test]
    fn run_render_command_round_trips() {
        let dir = mk_temp_dir("run_render");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "outs:\n  out1\nparams:\n  gain = 0.25 {0.0, 1.0}\nsample:\n  out1 = gain\n",
        );

        let mut session = DaemonSession::default();
        let start = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(2),
                request: Request::RunStart {
                    path: main.to_string_lossy().into_owned(),
                },
            },
        );
        assert!(start.ok, "start response: {:?}", start.error);

        let render = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(3),
                request: Request::RunRender {
                    path: main.to_string_lossy().into_owned(),
                    include_sample_bits: true,
                },
            },
        );
        assert!(render.ok, "render response: {:?}", render.error);
        let result = render.result.expect("render result");
        assert_eq!(result["frames"].as_u64(), Some(512));
        assert_eq!(result["channels"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["channel_bits"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            result["channel_bits"][0][0].as_u64(),
            Some(0.25_f32.to_bits().into())
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_run_start_returns_only_the_error() {
        let dir = mk_temp_dir("run_start_failure_print");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "params:\n  divisor: i32 = 0\nouts:\n  out1\ninit:\n  print(\"before failure\", 7)\n  value = i32(1) / divisor\nsample:\n  out1 = f32(value)\n",
        );

        let mut session = DaemonSession::default();
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(4),
                request: Request::RunStart {
                    path: main.to_string_lossy().into_owned(),
                },
            },
        );

        assert!(!response.ok);
        assert!(response.error.is_some());
        assert!(response.result.is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn successful_run_start_returns_initialization_prints() {
        let dir = mk_temp_dir("run_start_print");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "outs:\n  out1\ninit:\n  print(\"ready\", true)\nsample:\n  out1 = 0.0\n",
        );

        let mut session = DaemonSession::default();
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(7),
                request: Request::RunStart {
                    path: main.to_string_lossy().into_owned(),
                },
            },
        );

        assert!(response.ok, "start response: {:?}", response.error);
        let result = response.result.expect("start result");
        assert_eq!(result["print"]["text"], "ready: true\n");
        assert_eq!(result["print"]["entries"].as_array().map(Vec::len), Some(1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_render_returns_prints_emitted_before_the_error() {
        let dir = mk_temp_dir("run_render_failure_print");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "params:\n  divisor: i32 = 0\nouts:\n  out1\nsample:\n  print(\"before failure\", 9)\n  value: i32 = i32(1) / divisor\n  out1 = f32(value)\n",
        );
        let path = main.to_string_lossy().into_owned();
        let mut session = DaemonSession::default();
        let start = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(5),
                request: Request::RunStart { path: path.clone() },
            },
        );
        assert!(start.ok, "start response: {:?}", start.error);

        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(6),
                request: Request::RunRender {
                    path,
                    include_sample_bits: false,
                },
            },
        );

        assert!(!response.ok);
        assert!(response.error.is_some());
        let result = response.result.expect("failure output");
        assert_eq!(result["status"], "failed");
        assert_eq!(result["print"]["text"], "before failure: 9\n");
        assert_eq!(result["delegate_occurrences"], json!([]));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_bind_buffer_command_supplies_render_data() {
        let dir = mk_temp_dir("run_bind_buffer");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "buffers:\n  src: f32\nouts:\n  out1\nsample:\n  out1 = src[0]\n",
        );
        let path = main.to_string_lossy().into_owned();
        let mut session = DaemonSession::default();
        let request = |session: &mut DaemonSession, request| {
            handle_request(
                session,
                RequestEnvelope {
                    id: Some(1),
                    request,
                },
            )
        };

        assert!(request(&mut session, Request::RunStart { path: path.clone() }).ok);
        let bind = request(
            &mut session,
            Request::RunBindBuffer {
                path: path.clone(),
                name: "src".to_owned(),
                samples: vec![0.5],
                channels: 1,
                sample_rate_hz: 48_000.0,
            },
        );
        assert!(bind.ok, "bind response: {:?}", bind.error);

        let render = request(
            &mut session,
            Request::RunRender {
                path,
                include_sample_bits: false,
            },
        );
        assert!(render.ok, "render response: {:?}", render.error);
        assert_eq!(render.result.expect("render result")["channels"][0][0], 0.5);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_snapshot_and_restore_commands_replay_persistent_state() {
        let dir = mk_temp_dir("run_snapshot_restore");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "init:\n  phase = 0.0\nsample:\n  phase = phase + 1.0\n  out1 = phase\n",
        );
        let path = main.to_string_lossy().into_owned();
        let mut session = DaemonSession::default();
        let request = |session: &mut DaemonSession, request| {
            handle_request(
                session,
                RequestEnvelope {
                    id: Some(1),
                    request,
                },
            )
        };

        assert!(request(&mut session, Request::RunStart { path: path.clone() }).ok);
        assert!(
            request(
                &mut session,
                Request::RunRender {
                    path: path.clone(),
                    include_sample_bits: false,
                }
            )
            .ok
        );
        let snapshot = request(&mut session, Request::RunSnapshot { path: path.clone() });
        assert!(snapshot.ok, "snapshot response: {:?}", snapshot.error);
        let bytes = snapshot.result.expect("snapshot result")["bytes"]
            .as_array()
            .expect("snapshot bytes")
            .iter()
            .map(|byte| byte.as_u64().expect("byte") as u8)
            .collect::<Vec<_>>();

        let advanced = request(
            &mut session,
            Request::RunRender {
                path: path.clone(),
                include_sample_bits: false,
            },
        );
        let advanced_first = advanced.result.expect("advanced render")["channels"][0][0]
            .as_f64()
            .expect("advanced sample");
        let restored = request(
            &mut session,
            Request::RunRestore {
                path: path.clone(),
                bytes,
            },
        );
        assert!(restored.ok, "restore response: {:?}", restored.error);
        let replayed = request(
            &mut session,
            Request::RunRender {
                path,
                include_sample_bits: false,
            },
        );
        let replayed_first = replayed.result.expect("replayed render")["channels"][0][0]
            .as_f64()
            .expect("replayed sample");
        assert_eq!(advanced_first, replayed_first);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stdio_runs_collect_delegate_occurrences() {
        let dir = mk_temp_dir("run_delegate");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "delegate report(value: i32)\n\nevent trigger(value: i32):\n  report(value)\n\nsample:\n  out1 = 0.0\n",
        );
        let path = main.to_string_lossy().into_owned();
        let mut session = DaemonSession::default();

        let start = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::RunStart { path: path.clone() },
            },
        );
        assert!(start.ok, "start response: {:?}", start.error);

        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(2),
                request: Request::RunTriggerEvent {
                    path,
                    name: "trigger".to_owned(),
                    values: vec![json!(7.0)],
                },
            },
        );
        assert!(response.ok, "event response: {:?}", response.error);
        let result = response.result.expect("event result");
        assert_eq!(result["delegate_overflow_count"], 0);
        assert_eq!(result["delegate_occurrences"][0]["name"], "report");
        assert_eq!(
            result["delegate_occurrences"][0]["values"]["value"].as_f64(),
            Some(7.0)
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stdio_preserves_non_finite_delegate_values() {
        let dir = mk_temp_dir("run_non_finite_delegate");
        let main = dir.join("main.onda");
        write_file(
            &main,
            "delegate report(value: f32)\n\nevent trigger():\n  report(sqrt(-1.0))\n\nsample:\n  out1 = 0.0\n",
        );
        let path = main.to_string_lossy().into_owned();
        let mut session = DaemonSession::default();

        let start = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::RunStart { path: path.clone() },
            },
        );
        assert!(start.ok, "start response: {:?}", start.error);

        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(2),
                request: Request::RunTriggerEvent {
                    path,
                    name: "trigger".to_owned(),
                    values: Vec::new(),
                },
            },
        );
        assert!(response.ok, "event response: {:?}", response.error);
        let result = response.result.expect("event result");
        assert_eq!(
            result["delegate_occurrences"][0]["values"]["value"],
            Value::String("NaN".to_owned())
        );

        fs::remove_dir_all(&dir).ok();
    }
}
