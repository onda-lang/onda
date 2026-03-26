use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use serde::Deserialize;
use serde_json::Value;
use tao::event_loop::EventLoopProxy;

use crate::{PreviewWindowOptions, UserEvent};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Parsed `{"event": "ready", ...}` from the subprocess stdout.
#[derive(Debug, Clone)]
pub struct ReadyEvent {
    pub path: String,
    pub port: u16,
    pub params: Vec<Value>,
    pub buffers: Vec<Value>,
    pub events: Vec<Value>,
    pub output_channels: usize,
}

#[derive(Deserialize)]
struct RawReadyEvent {
    event: String,
    path: Option<String>,
    port: Option<u16>,
    params: Option<Vec<Value>>,
    buffers: Option<Vec<Value>>,
    events: Option<Vec<Value>>,
    #[serde(rename = "outputChannels")]
    output_channels: Option<usize>,
}

/// Manages a running `omni preview play --control-json` subprocess.
pub struct ChildSession {
    child: Option<Child>,
    stderr_buffer: Arc<Mutex<String>>,
}

impl ChildSession {
    /// Spawn `omni preview play <path> --forever --control-json` and start a background
    /// thread that reads stdout looking for the ready event.
    pub fn spawn(
        omni_path: &Path,
        options: &PreviewWindowOptions,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(&options.omni_bin);
        cmd.arg("preview")
            .arg("play")
            .arg(omni_path)
            .arg("--forever")
            .arg("--control-json")
            .arg("--sample-rate")
            .arg(options.sample_rate_hz.to_string())
            .arg("--block")
            .arg(options.block_frames.to_string());

        if let Some(input_device) = &options.input_device {
            cmd.arg("--input-device").arg(input_device);
        }
        if let Some(output_device) = &options.output_device {
            cmd.arg("--output-device").arg(output_device);
        }

        if options.fast_math {
            cmd.arg("--fast-math");
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn omni preview: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture subprocess stdout".to_owned())?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture subprocess stderr".to_owned())?;
        let stderr_buffer = Arc::new(Mutex::new(String::new()));

        // Background thread: read stderr and print it.
        let stderr_sink = Arc::clone(&stderr_buffer);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        eprintln!("[omni preview] {line}");
                        if let Ok(mut slot) = stderr_sink.lock() {
                            if !slot.is_empty() {
                                slot.push('\n');
                            }
                            slot.push_str(&line);
                            if slot.len() > 4000 {
                                let keep_from = slot.len().saturating_sub(4000);
                                *slot = slot[keep_from..].to_owned();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Background thread: read stdout looking for the ready event.
        let stdout_proxy = proxy.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => break,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(raw) = serde_json::from_str::<RawReadyEvent>(trimmed) {
                    if raw.event == "ready" {
                        let ready = ReadyEvent {
                            path: raw.path.unwrap_or_default(),
                            port: raw.port.unwrap_or(0),
                            params: raw.params.unwrap_or_default(),
                            buffers: raw.buffers.unwrap_or_default(),
                            events: raw.events.unwrap_or_default(),
                            output_channels: raw.output_channels.unwrap_or(0),
                        };
                        let _ = stdout_proxy.send_event(UserEvent::ChildReady(ready));
                        continue;
                    }
                }
                // Non-ready stdout lines are informational.
                eprintln!("[omni preview stdout] {trimmed}");
            }
        });

        // Background thread: wait for child exit.
        let pid_proxy = proxy;
        let child_id = child.id();
        thread::spawn(move || {
            // We cannot call child.wait() since we don't own the Child here.
            // Instead, we poll via OS-level means. For simplicity, we use a pid-based loop.
            // The actual child object is owned by ChildSession.
            // This thread just monitors for orphaned exit via /proc or waitpid.
            // For a more robust approach we'd use the child.wait() method,
            // but that requires ownership. Instead we rely on the stdout/stderr
            // threads ending when the child exits, plus the event loop checking.
            let _ = (child_id, pid_proxy);
        });

        Ok(Self {
            child: Some(child),
            stderr_buffer,
        })
    }

    pub fn try_take_exit(&mut self) -> Option<(Option<i32>, Option<String>)> {
        let child = self.child.as_mut()?;
        let status = child.try_wait().ok()??;
        self.child = None;
        let error = self.stderr_buffer.lock().ok().and_then(|slot| {
            let trimmed = slot.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        });
        Some((status.code(), error))
    }

    /// Kill the child process if it's still running.
    pub fn kill(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        self.kill();
    }
}
