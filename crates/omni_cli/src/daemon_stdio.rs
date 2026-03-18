use std::io::{self, BufRead, BufWriter, Write};
use std::path::Path;

use omni_daemon::{
    DaemonConfig, DaemonSession, DocumentVersion, PreviewBuildError, PreviewOptions,
    PreviewParamInfo,
};
use omni_frontend::Diagnostic;
use omni_semantics::AnalysisOptions;
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
    PreviewStart {
        path: String,
    },
    PreviewStop {
        path: String,
    },
    PreviewParams {
        path: String,
    },
    PreviewSetParam {
        path: String,
        name: String,
        value: f64,
    },
    PreviewRender {
        path: String,
    },
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
}

fn handle_request(session: &mut DaemonSession, envelope: RequestEnvelope) -> ResponseEnvelope {
    let id = envelope.id;
    let result = match envelope.request {
        Request::Ping => Ok(json!({ "status": "ok" })),
        Request::Initialize {
            sample_rate_hz,
            block_frames,
            fast_math,
        } => {
            let current = session.config();
            let preview = PreviewOptions {
                sample_rate: sample_rate_hz.unwrap_or(current.preview.sample_rate as u32) as f32,
                block_size: block_frames.unwrap_or(current.preview.block_size),
                float_param_smoothing_ms: current.preview.float_param_smoothing_ms,
                fast_math: fast_math.unwrap_or(current.preview.fast_math),
                backend: current.preview.backend,
            };
            let config = DaemonConfig {
                analysis: AnalysisOptions {
                    sample_rate: preview.sample_rate,
                    block_size: preview.block_size,
                },
                preview,
            };
            session.set_config(config);
            Ok(json!({
                "sample_rate_hz": preview.sample_rate,
                "block_frames": preview.block_size,
                "fast_math": preview.fast_math,
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
        Request::PreviewStart { path } => session
            .start_preview(&path)
            .map(|preview| {
                json!({
                    "path": display_path(preview.path()),
                    "version": preview.version().map(|v| v.0),
                    "params": preview.param_info().iter().map(preview_param_json).collect::<Vec<_>>(),
                    "output_channels": preview.output_channel_count(),
                })
            })
            .map_err(|err| preview_build_error_string("preview_start failed", &err)),
        Request::PreviewStop { path } => {
            let stopped = session.stop_preview(path).is_some();
            Ok(json!({ "stopped": stopped }))
        }
        Request::PreviewParams { path } => session
            .preview(path)
            .map(|preview| {
                json!({
                    "params": preview.param_info().iter().map(preview_param_json).collect::<Vec<_>>(),
                    "output_channels": preview.output_channel_count(),
                })
            })
            .ok_or_else(|| "preview is not active".to_owned()),
        Request::PreviewSetParam { path, name, value } => session
            .preview_mut(path)
            .ok_or_else(|| "preview is not active".to_owned())
            .and_then(|preview| {
                preview
                    .set_param_f64(&name, value)
                    .map_err(|diag| diagnostic_string("preview_set_param failed", &diag))
            })
            .map(|_| json!({ "status": "ok" })),
        Request::PreviewRender { path } => session
            .render_preview_block(path)
            .map(|channels| {
                let frames = channels.first().map(Vec::len).unwrap_or(0);
                json!({
                    "frames": frames,
                    "channels": channels,
                })
            })
            .map_err(|diag| diagnostic_string("preview_render failed", &diag)),
    };

    match result {
        Ok(result) => ResponseEnvelope::ok(id, result),
        Err(err) => ResponseEnvelope::error(id, err),
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

fn preview_param_json(param: &PreviewParamInfo) -> Value {
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

fn preview_build_error_string(context: &str, err: &PreviewBuildError) -> String {
    match err {
        PreviewBuildError::Diagnostics(diags) => diagnostics_string(context, diags),
        PreviewBuildError::Runtime(diag) => diagnostic_string(context, diag),
    }
}

fn diagnostics_string(context: &str, diagnostics: &[Diagnostic]) -> String {
    let messages = diagnostics
        .iter()
        .map(|diag| diagnostic_summary(diag))
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
        let dir = std::env::temp_dir().join(format!("omni_cli_daemon_stdio_{prefix}_{nanos}"));
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
        assert!(session.config().preview.fast_math);
    }

    #[test]
    fn preview_render_command_round_trips() {
        let dir = mk_temp_dir("preview_render");
        let main = dir.join("main.omni");
        write_file(
            &main,
            "outs:\n  out1\nparams:\n  gain = 0.25 {0.0, 1.0}\nsample:\n  out1 = gain\n",
        );

        let mut session = DaemonSession::default();
        let start = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(2),
                request: Request::PreviewStart {
                    path: main.to_string_lossy().into_owned(),
                },
            },
        );
        assert!(start.ok, "start response: {:?}", start.error);

        let render = handle_request(
            &mut session,
            RequestEnvelope {
                id: Some(3),
                request: Request::PreviewRender {
                    path: main.to_string_lossy().into_owned(),
                },
            },
        );
        assert!(render.ok, "render response: {:?}", render.error);
        let result = render.result.expect("render result");
        assert_eq!(result["frames"].as_u64(), Some(512));
        assert_eq!(result["channels"].as_array().map(Vec::len), Some(1));

        fs::remove_dir_all(&dir).ok();
    }
}
