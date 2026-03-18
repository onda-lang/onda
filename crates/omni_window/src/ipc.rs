use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tao::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Manages the TCP connection to the `omni preview play --control-json` subprocess.
pub struct IpcBridge {
    writer: Option<Arc<Mutex<BufWriter<TcpStream>>>>,
    request_id: Arc<AtomicU64>,
}

impl IpcBridge {
    pub fn new() -> Self {
        Self {
            writer: None,
            request_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Connect to the TCP control socket at the given port.
    /// Spawns a background reader thread that forwards responses as `UserEvent::TcpResponse`.
    pub fn connect(&mut self, port: u16, proxy: EventLoopProxy<UserEvent>) -> Result<(), String> {
        self.disconnect();

        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("TCP connect failed: {e}"))?;

        let reader_stream = stream
            .try_clone()
            .map_err(|e| format!("TCP clone failed: {e}"))?;

        let writer = Arc::new(Mutex::new(BufWriter::new(stream)));
        self.writer = Some(writer);

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
    pub fn disconnect(&mut self) {
        if let Some(ref writer) = self.writer {
            // Dropping the writer will close our side of the stream, which will
            // cause the reader thread to exit.
            if let Ok(w) = writer.lock() {
                let _ = w.get_ref().shutdown(std::net::Shutdown::Both);
            }
        }
        self.writer = None;
    }

    /// Send a JSON command to the subprocess over TCP.
    pub fn send_command(&mut self, command: &str, payload: &Value) {
        let Some(ref writer) = self.writer else {
            return;
        };
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);

        let mut request = json!({
            "id": id,
            "command": command,
        });

        // Merge payload fields into the request object.
        if let Value::Object(map) = payload {
            if let Value::Object(ref mut req_map) = request {
                for (k, v) in map {
                    req_map.insert(k.clone(), v.clone());
                }
            }
        }

        if let Ok(mut w) = writer.lock() {
            let line = serde_json::to_string(&request).unwrap_or_default();
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
            let _ = w.flush();
        }
    }

    /// Returns true if we have an active TCP connection.
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.writer.is_some()
    }
}
