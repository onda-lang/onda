use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use super::{
    child_exit_error, ControllerEvent, RawReadyEvent, ReadyEvent, RunHostOptions, RunParamWire,
    COMPUTER_KEYBOARD_MIDI_INPUT,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
pub(super) struct ChildSession {
    child: Option<Child>,
    stderr_buffer: Arc<Mutex<String>>,
    stderr_reader: Option<thread::JoinHandle<()>>,
}

impl ChildSession {
    pub(super) fn is_active(&self) -> bool {
        self.child.is_some()
    }

    pub(super) fn spawn(
        onda_path: &Path,
        options: &RunHostOptions,
        generation: u64,
        event_tx: super::EventSender,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(&options.onda_bin);
        cmd.arg("run")
            .arg("play")
            .arg(onda_path)
            .arg("--forever")
            .arg("--control-json")
            .arg("--sample-rate")
            .arg(options.sample_rate_hz.to_string())
            .arg("--block-size")
            .arg(options.block_samples.to_string())
            .arg("--param-smoothing-ms")
            .arg((options.param_smoothing_seconds * 1000.0).to_string())
            .arg("--opt-level")
            .arg(options.opt_level.as_str());

        if let Some(input_device) = &options.input_device {
            cmd.arg("--input-device").arg(input_device);
        }
        if let Some(output_device) = &options.output_device {
            cmd.arg("--output-device").arg(output_device);
        }
        if let Some(midi_input_device) = options
            .midi_input_device
            .as_deref()
            .filter(|name| *name != COMPUTER_KEYBOARD_MIDI_INPUT)
        {
            cmd.arg("--midi-input-device").arg(midi_input_device);
        }
        if options.fast_math {
            cmd.arg("--fast-math");
        }
        if options.show_meta {
            cmd.arg("--meta");
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn onda run: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture subprocess stdout".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture subprocess stderr".to_owned())?;
        let stderr_buffer = Arc::new(Mutex::new(String::new()));

        let stderr_sink = Arc::clone(&stderr_buffer);
        let stderr_reader = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        eprintln!("[onda run] {line}");
                        if let Ok(mut slot) = stderr_sink.lock() {
                            append_stderr(&mut slot, &line);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(raw) = serde_json::from_str::<RawReadyEvent>(trimmed) {
                    if raw.event == "ready" {
                        let params = match raw
                            .params
                            .unwrap_or_default()
                            .into_iter()
                            .map(RunParamWire::into_host_value)
                            .collect::<Result<Vec<_>, _>>()
                        {
                            Ok(params) => params,
                            Err(error) => {
                                eprintln!("[onda run stdout] invalid ready event: {error}");
                                continue;
                            }
                        };
                        let ready = ReadyEvent {
                            path: raw.path.unwrap_or_default(),
                            port: raw.port.unwrap_or(0),
                            params,
                            buffers: raw.buffers.unwrap_or_default(),
                            events: raw.events.unwrap_or_default(),
                            midi: raw.midi,
                            delegates: raw.delegates.unwrap_or_default(),
                            output_channels: raw.output_channels.unwrap_or(0),
                            input_devices: raw.input_devices.unwrap_or_default(),
                            output_devices: raw.output_devices.unwrap_or_default(),
                            midi_input_devices: raw.midi_input_devices.unwrap_or_default(),
                            current_input_device: raw.current_input_device,
                            current_output_device: raw.current_output_device,
                            current_midi_input_device: raw.current_midi_input_device,
                        };
                        let _ = event_tx.send(ControllerEvent::ChildReady {
                            generation,
                            ready: Box::new(ready),
                        });
                        continue;
                    }
                }
                eprintln!("[onda run stdout] {trimmed}");
            }
        });

        Ok(Self {
            child: Some(child),
            stderr_buffer,
            stderr_reader: Some(stderr_reader),
        })
    }

    pub(super) fn try_take_exit(&mut self) -> Option<(Option<i32>, Option<String>)> {
        let child = self.child.as_mut()?;
        let status = child.try_wait().ok()??;
        let status_error = (!status.success()).then(|| format_exit_status(&status));
        self.child = None;
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        let stderr_error = self.stderr_buffer.lock().ok().and_then(|slot| {
            let trimmed = slot.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        });
        let error = match (stderr_error, status_error) {
            (Some(stderr), Some(status)) => Some(format!("{stderr}\n{status}")),
            (Some(stderr), None) => Some(stderr),
            (None, status) => status,
        };
        Some((status.code(), error))
    }

    pub(super) fn kill(&mut self) {
        if let Some(ref mut child) = self.child {
            terminate_run_child(child);
            let _ = child.wait();
        }
        self.child = None;
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(target_os = "windows")]
fn terminate_run_child(child: &mut Child) {
    let pid = child.id().to_string();
    let mut cmd = Command::new("taskkill");
    cmd.arg("/PID").arg(pid).arg("/T").arg("/F");
    cmd.creation_flags(CREATE_NO_WINDOW);
    if cmd.status().map(|status| status.success()).unwrap_or(false) {
        return;
    }
    let _ = child.kill();
}

#[cfg(not(target_os = "windows"))]
fn terminate_run_child(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
fn format_exit_status(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map_or_else(
        || child_exit_error(status.code()),
        |signal| format!("Runtime error: audio processing terminated by signal {signal}"),
    )
}

#[cfg(not(unix))]
fn format_exit_status(status: &std::process::ExitStatus) -> String {
    child_exit_error(status.code())
}

fn append_stderr(buffer: &mut String, line: &str) {
    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(line);
    let start = buffer.ceil_char_boundary(buffer.len().saturating_sub(4000));
    buffer.drain(..start);
}

#[cfg(test)]
mod tests {
    use super::append_stderr;

    #[test]
    fn stderr_tail_remains_bounded_and_valid_utf8() {
        for text in [
            "é".repeat(2000) + "a",
            "🦀".repeat(2000) + "end",
            "x".repeat(5000),
        ] {
            let mut buffer = String::from("previous diagnostic");
            append_stderr(&mut buffer, &text);
            assert!(buffer.len() <= 4000);
            assert!(text.ends_with(&buffer));
            assert!(!buffer.is_empty());
        }
    }
}
