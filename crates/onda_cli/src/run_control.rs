use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use onda_daemon::{RunBufferChannels, RunBufferInfo, RunEventInfo, RunEventValue, RunParamInfo};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug)]
pub(crate) enum PlaybackControlCommand {
    GetParams {
        reply: mpsc::Sender<Result<Vec<RunParamInfo>, String>>,
    },
    GetBuffers {
        reply: mpsc::Sender<Result<Vec<RunBufferInfo>, String>>,
    },
    GetEvents {
        reply: mpsc::Sender<Result<Vec<RunEventInfo>, String>>,
    },
    GetDevices {
        reply: mpsc::Sender<Result<(Vec<String>, Vec<String>), String>>,
    },
    SetParam {
        name: String,
        value: f64,
        reply: Option<mpsc::Sender<Result<(), String>>>,
    },
    TriggerEvent {
        name: String,
        values: Vec<RunEventValue>,
        reply: Option<mpsc::Sender<Result<(), String>>>,
    },
    BindBufferWav {
        name: String,
        path: PathBuf,
        reply: mpsc::Sender<Result<(), String>>,
    },
    ClearBuffer {
        name: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

struct ScopeSnapshot {
    channels: usize,
    samples: Vec<f32>,
}

pub(crate) struct ScopeRing {
    buffer: Vec<f32>,
    channels: usize,
    write_pos: usize,
    frames_written: usize,
}

impl ScopeRing {
    pub(crate) fn new(capacity_frames: usize, channels: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity_frames * channels],
            channels,
            write_pos: 0,
            frames_written: 0,
        }
    }

    pub(crate) fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buffer.len();
        if cap == 0 {
            return;
        }
        for (i, &sample) in samples.iter().enumerate() {
            self.buffer[(self.write_pos + i) % cap] = sample;
        }
        self.write_pos = (self.write_pos + samples.len()) % cap;
        self.frames_written += samples.len() / self.channels.max(1);
    }

    fn snapshot(&self, max_frames: usize) -> ScopeSnapshot {
        let total_frames = self.buffer.len() / self.channels.max(1);
        let available = total_frames.min(self.frames_written);
        let frames = max_frames.min(available);
        let sample_count = frames * self.channels;
        let cap = self.buffer.len();
        let start = (self.write_pos + cap - sample_count) % cap;
        let mut samples = Vec::with_capacity(sample_count);
        for i in 0..sample_count {
            samples.push(self.buffer[(start + i) % cap]);
        }
        ScopeSnapshot {
            channels: self.channels,
            samples,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PlaybackControlRequest {
    #[serde(default)]
    pub(crate) id: Option<Value>,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) value: Option<Value>,
    #[serde(default)]
    pub(crate) values: Option<Vec<Value>>,
    #[serde(default, rename = "maxFrames")]
    pub(crate) max_frames: Option<usize>,
}

pub(crate) fn run_param_json(param: &RunParamInfo) -> Value {
    json!({
        "index": param.index,
        "name": param.name,
        "type": param.type_repr,
        "value": param.value,
        "default": param.default,
        "rangeMin": param.range_min,
        "rangeMax": param.range_max,
        "scalar": param.scalar,
    })
}

pub(crate) fn run_buffer_json(buffer: &RunBufferInfo) -> Value {
    let (channels_kind, channels_static) = match buffer.channels {
        RunBufferChannels::Mono => ("mono", None),
        RunBufferChannels::Static(channels) => ("static", Some(channels)),
        RunBufferChannels::Dynamic => ("dynamic", None),
    };
    json!({
        "index": buffer.index,
        "name": buffer.name,
        "type": buffer.type_repr,
        "channelsKind": channels_kind,
        "channelsStatic": channels_static,
        "loadedPath": buffer.loaded_path,
    })
}

pub(crate) fn run_event_json(event: &RunEventInfo) -> Value {
    json!({
        "index": event.index,
        "name": event.name,
        "args": event.params.iter().map(|param| json!({
            "index": param.index,
            "name": param.name,
            "type": param.type_repr,
            "default": run_event_value_json(&param.value),
            "value": run_event_value_json(&param.value),
        })).collect::<Vec<_>>(),
    })
}

fn run_event_value_json(value: &RunEventValue) -> Value {
    match value {
        RunEventValue::Bool(value) => Value::Bool(*value),
        RunEventValue::Number(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    }
}

pub(crate) fn write_json_line(
    writer: &mut impl Write,
    value: &Value,
) -> Result<(), std::io::Error> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub(crate) fn spawn_run_control_server(
    listener: TcpListener,
    control_tx: mpsc::Sender<PlaybackControlCommand>,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }

        while !stop_flag.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(err) =
                        handle_run_control_client(stream, &control_tx, &scope_ring, &stop_flag)
                    {
                        eprintln!("run control client error: {err}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    eprintln!("run control accept error: {err}");
                    break;
                }
            }
        }
    })
}

fn handle_run_control_client(
    stream: TcpStream,
    control_tx: &mpsc::Sender<PlaybackControlCommand>,
    scope_ring: &Arc<Mutex<ScopeRing>>,
    stop_flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|err| format!("failed to set control socket read timeout: {err}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|err| format!("failed to clone control socket: {err}"))?;
    let mut reader = BufReader::new(reader_stream);
    let mut writer = BufWriter::new(stream);

    while !stop_flag.load(Ordering::Acquire) {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(err) => return Err(format!("failed to read control request: {err}")),
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: PlaybackControlRequest = serde_json::from_str(trimmed)
            .map_err(|err| format!("invalid control request json: {err}"))?;
        let response = run_control_response(request, control_tx, scope_ring);
        if let Some(response) = response {
            write_json_line(&mut writer, &response)
                .map_err(|err| format!("failed to write control response: {err}"))?;
        }
    }

    Ok(())
}

pub(crate) fn run_control_response(
    request: PlaybackControlRequest,
    control_tx: &mpsc::Sender<PlaybackControlCommand>,
    scope_ring: &Arc<Mutex<ScopeRing>>,
) -> Option<Value> {
    let request_id = request.id;
    let result = match request.command.as_str() {
        "getParams" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetParams { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok(params) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "params": params.iter().map(run_param_json).collect::<Vec<_>>(),
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "getBuffers" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetBuffers { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok(buffers) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "buffers": buffers.iter().map(run_buffer_json).collect::<Vec<_>>(),
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "getEvents" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetEvents { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok(events) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "events": events.iter().map(run_event_json).collect::<Vec<_>>(),
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "getDevices" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetDevices { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok((input_devices, output_devices)) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "inputDevices": input_devices,
                                "outputDevices": output_devices,
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "setParam" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "setParam requires 'name'".to_owned())?;
                let raw_value = request
                    .value
                    .ok_or_else(|| "setParam requires 'value'".to_owned())?;
                let value = match raw_value {
                    Value::Bool(value) => {
                        if value {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Number(value) => value
                        .as_f64()
                        .ok_or_else(|| "setParam value must be numeric".to_owned())?,
                    _ => return Err("setParam value must be number or boolean".to_owned()),
                };
                if request_id.is_none() {
                    control_tx
                        .send(PlaybackControlCommand::SetParam {
                            name,
                            value,
                            reply: None,
                        })
                        .map_err(|_| "run control channel closed".to_owned())?;
                    return Ok(None);
                }
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::SetParam {
                        name,
                        value,
                        reply: Some(reply_tx),
                    })
                    .map_err(|_| "run control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "run control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": true,
                        })
                    })),
                    Err(err) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": false,
                            "error": err,
                        })
                    })),
                }
            })();
            result
        }
        "triggerEvent" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "triggerEvent requires 'name'".to_owned())?;
                let raw_values = request.values.unwrap_or_default();
                let values = raw_values
                    .into_iter()
                    .map(|value| match value {
                        Value::Bool(value) => Ok(RunEventValue::Bool(value)),
                        Value::Number(value) => value
                            .as_f64()
                            .map(RunEventValue::Number)
                            .ok_or_else(|| "triggerEvent values must be numeric".to_owned()),
                        _ => Err("triggerEvent values must be numbers or booleans".to_owned()),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if request_id.is_none() {
                    control_tx
                        .send(PlaybackControlCommand::TriggerEvent {
                            name,
                            values,
                            reply: None,
                        })
                        .map_err(|_| "run control channel closed".to_owned())?;
                    return Ok(None);
                }
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::TriggerEvent {
                        name,
                        values,
                        reply: Some(reply_tx),
                    })
                    .map_err(|_| "run control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "run control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": true,
                        })
                    })),
                    Err(err) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": false,
                            "error": err,
                        })
                    })),
                }
            })();
            result
        }
        "bindBufferWav" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "bindBufferWav requires 'name'".to_owned())?;
                let path = request
                    .path
                    .ok_or_else(|| "bindBufferWav requires 'path'".to_owned())?;
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::BindBufferWav {
                        name,
                        path: PathBuf::from(path),
                        reply: reply_tx,
                    })
                    .map_err(|_| "run control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "run control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": true,
                        })
                    })),
                    Err(err) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": false,
                            "error": err,
                        })
                    })),
                }
            })();
            result
        }
        "getScopeData" => scope_ring
            .lock()
            .map_err(|_| "failed to lock scope ring".to_owned())
            .map(|ring| {
                let snapshot = ring.snapshot(request.max_frames.unwrap_or(2048));
                Some(json!({
                    "id": request_id,
                    "ok": true,
                    "result": {
                        "channels": snapshot.channels,
                        "samples": snapshot.samples,
                    }
                }))
            }),
        "clearBuffer" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "clearBuffer requires 'name'".to_owned())?;
                let (reply_tx, reply_rx) = mpsc::channel();
                control_tx
                    .send(PlaybackControlCommand::ClearBuffer {
                        name,
                        reply: reply_tx,
                    })
                    .map_err(|_| "run control channel closed".to_owned())?;
                match reply_rx
                    .recv()
                    .map_err(|_| "run control reply channel closed".to_owned())?
                {
                    Ok(()) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": true,
                        })
                    })),
                    Err(err) => Ok(request_id.clone().map(|id| {
                        json!({
                            "id": id,
                            "ok": false,
                            "error": err,
                        })
                    })),
                }
            })();
            result
        }
        other => Err(format!("unknown command '{other}'")),
    };

    match result {
        Ok(value) => value,
        Err(err) => request_id.map(|id| {
            json!({
                "id": id,
                "ok": false,
                "error": err,
            })
        }),
    }
}
