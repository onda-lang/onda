use std::io::{self, BufRead, BufWriter, Write};
use std::path::Path;

use onda_daemon::{
    DaemonConfig, DaemonSession, DocumentVersion, RunBuildError, RunDelegateBatch, RunDelegateInfo,
    RunEventValue, RunOptions, RunParamInfo, RunPrintBatch,
};
use onda_frontend::Diagnostic;
use onda_semantics::AnalysisOptions;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn run_stdio_loop() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut session = DaemonSession::default();
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
        block_frames: Option<usize>,
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
        value: f64,
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
        values: Vec<EventValueRequest>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum EventValueRequest {
    Bool(bool),
    Number(f64),
    I64(String),
}

impl TryFrom<EventValueRequest> for RunEventValue {
    type Error = String;

    fn try_from(value: EventValueRequest) -> Result<Self, Self::Error> {
        match value {
            EventValueRequest::Bool(value) => Ok(Self::Bool(value)),
            EventValueRequest::Number(value) => Ok(Self::Number(value)),
            EventValueRequest::I64(value) => value
                .parse()
                .map(Self::I64)
                .map_err(|_| format!("invalid decimal i64 event value '{value}'")),
        }
    }
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
            block_frames,
            fast_math,
        } => {
            let current = session.config();
            let run = RunOptions {
                sample_rate: sample_rate_hz.unwrap_or(current.run.sample_rate as u32) as f32,
                block_size: block_frames.unwrap_or(current.run.block_size),
                float_param_smoothing_ms: current.run.float_param_smoothing_ms,
                fast_math: fast_math.unwrap_or(current.run.fast_math),
                opt_level: current.run.opt_level,
            };
            let config = DaemonConfig {
                analysis: AnalysisOptions {
                    sample_rate: run.sample_rate,
                    block_size: run.block_size,
                },
                run,
            };
            session.set_config(config);
            Ok(json!({
                "sample_rate_hz": run.sample_rate,
                "block_frames": run.block_size,
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
        Request::RunStart { path } => match session.start_run(&path) {
            Ok(run) => {
                let result = json!({
                    "path": display_path(run.path()),
                    "version": run.version().map(|v| v.0),
                    "params": run.param_info().iter().map(run_param_json).collect::<Vec<_>>(),
                    "delegates": run.delegate_info().iter().map(run_delegate_json).collect::<Vec<_>>(),
                    "output_channels": run.output_channel_count(),
                });
                let prints = session
                    .run_mut(&path)
                    .expect("run should be active after successful start")
                    .take_print_batch()
                    .map_err(|diag| diagnostic_string("run_start print decoding failed", &diag));
                prints.map(|prints| attach_run_print_batch(result, &prints))
            }
            Err(err) => {
                if let RunBuildError::Initialization {
                    print_batch: Some(batch),
                    ..
                } = &err
                {
                    error_result =
                        Some(attach_run_print_batch(json!({ "status": "failed" }), batch));
                }
                Err(run_build_error_string("run_start failed", &err))
            }
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
                run.set_param_f64(&name, value)
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
                .map(TryInto::try_into)
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
                .map(|entry| (entry.name.clone(), run_event_value_json(&entry.value)))
                .collect::<serde_json::Map<_, _>>();
            json!({
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
                "value": run_event_value_json(&value.value),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "overflow_count": batch.overflow_count,
        "transport_drop_count": batch.transport_drop_count,
    });
    result
}

fn run_event_value_json(value: &RunEventValue) -> Value {
    match value {
        RunEventValue::Bool(value) => Value::Bool(*value),
        RunEventValue::Number(value) => json!(value),
        RunEventValue::I64(value) => Value::String(value.to_string()),
        RunEventValue::Array(values) => {
            Value::Array(values.iter().map(run_event_value_json).collect())
        }
    }
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
        "value": param.value,
        "type_repr": param.type_repr.clone(),
        "default": param.default,
        "range_min": param.range_min,
        "range_max": param.range_max,
        "scalar": param.scalar,
    })
}

fn run_build_error_string(context: &str, err: &RunBuildError) -> String {
    match err {
        RunBuildError::Diagnostics(diags) => diagnostics_string(context, diags),
        RunBuildError::Runtime(diag) => diagnostic_string(context, diag),
        RunBuildError::Initialization { diagnostic, .. } => diagnostic_string(context, diagnostic),
    }
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
    fn initialize_updates_session_config() {
        let mut session = DaemonSession::default();
        let response = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(1),
                request: Request::Initialize {
                    sample_rate_hz: Some(44_100),
                    block_frames: Some(256),
                    fast_math: Some(true),
                },
            },
        );

        assert!(response.ok);
        assert_eq!(session.config().analysis.block_size, 256);
        assert_eq!(session.config().analysis.sample_rate, 44_100.0);
        assert!(session.config().run.fast_math);
    }

    #[test]
    fn daemon_json_preserves_decimal_i64_event_values() {
        let value = RunEventValue::try_from(EventValueRequest::I64("9007199254740993".to_owned()))
            .expect("decimal i64 input should parse");
        assert_eq!(value, RunEventValue::I64(9_007_199_254_740_993));
        assert_eq!(
            run_event_value_json(&value),
            Value::String("9007199254740993".to_owned())
        );
        assert!(RunEventValue::try_from(EventValueRequest::I64("1.5".to_owned())).is_err());
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
    fn failed_run_start_returns_prints_emitted_before_the_error() {
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
        let result = response.result.expect("failure output");
        assert_eq!(result["status"], "failed");
        assert_eq!(result["print"]["text"], "before failure: 7\n");
        assert_eq!(result["print"]["entries"].as_array().map(Vec::len), Some(1));

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
}
