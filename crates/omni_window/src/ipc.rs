use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tao::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Manages the TCP connection to the `omni preview play --control-json` subprocess.
#[derive(Clone)]
pub struct IpcBridge {
    writer: Arc<Mutex<Option<BufWriter<TcpStream>>>>,
    request_id: Arc<AtomicU64>,
}

impl IpcBridge {
    pub fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(None)),
            request_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Connect to the TCP control socket at the given port.
    /// Spawns a background reader thread that forwards responses as `UserEvent::TcpResponse`.
    pub fn connect(&self, port: u16, proxy: EventLoopProxy<UserEvent>) -> Result<(), String> {
        self.disconnect();

        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("TCP connect failed: {e}"))?;

        let reader_stream = stream
            .try_clone()
            .map_err(|e| format!("TCP clone failed: {e}"))?;

        if let Ok(mut writer) = self.writer.lock() {
            *writer = Some(BufWriter::new(stream));
        }

        // Background reader thread.
        thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let trimmed = line.trim().to_owned();
                        if !trimmed.is_empty() {
                            let _ = proxy.send_event(UserEvent::TcpResponse(trimmed));
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    /// Disconnect from the TCP control socket.
    pub fn disconnect(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            if let Some(writer) = writer.as_mut() {
                let _ = writer.get_ref().shutdown(std::net::Shutdown::Both);
            }
            *writer = None;
        }
    }

    /// Send a JSON command to the subprocess over TCP.
    pub fn send_command(&self, command: &str, payload: &Value) {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        self.send_command_inner(Some(id), command, payload);
    }

    /// Send a JSON notification that does not expect a reply.
    pub fn send_command_notification(&self, command: &str, payload: &Value) {
        self.send_command_inner(None, command, payload);
    }

    fn send_command_inner(&self, id: Option<u64>, command: &str, payload: &Value) {
        let mut request = json!({
            "command": command,
        });
        if let Some(id) = id {
            if let Value::Object(ref mut req_map) = request {
                req_map.insert("id".to_owned(), Value::from(id));
            }
        }

        // Merge payload fields into the request object.
        if let Value::Object(map) = payload {
            if let Value::Object(ref mut req_map) = request {
                for (k, v) in map {
                    req_map.insert(k.clone(), v.clone());
                }
            }
        }

        if let Ok(mut writer) = self.writer.lock() {
            let Some(writer) = writer.as_mut() else {
                return;
            };
            let line = serde_json::to_string(&request).unwrap_or_default();
            let _ = writer.write_all(line.as_bytes());
            let _ = writer.write_all(b"\n");
            let _ = writer.flush();
        }
    }

    /// Returns true if we have an active TCP connection.
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.writer
            .lock()
            .map(|writer| writer.is_some())
            .unwrap_or(false)
    }
}
