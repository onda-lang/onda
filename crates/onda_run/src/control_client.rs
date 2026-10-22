use super::ControllerEvent;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};
use std::thread;

#[derive(Clone)]
pub(super) struct IpcBridge {
    writer: Arc<Mutex<Option<BufWriter<TcpStream>>>>,
    request_id: Arc<AtomicU64>,
}

impl IpcBridge {
    pub(super) fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(None)),
            request_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(super) fn connect(
        &self,
        port: u16,
        generation: u64,
        event_tx: Sender<ControllerEvent>,
    ) -> Result<(), String> {
        self.disconnect();

        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("TCP connect failed: {e}"))?;
        let reader_stream = stream
            .try_clone()
            .map_err(|e| format!("TCP clone failed: {e}"))?;

        if let Ok(mut writer) = self.writer.lock() {
            *writer = Some(BufWriter::new(stream));
        }

        thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let trimmed = line.trim().to_owned();
                        if !trimmed.is_empty() {
                            let _ = event_tx.send(ControllerEvent::TcpResponse {
                                generation,
                                line: trimmed,
                            });
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    pub(super) fn disconnect(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            if let Some(writer) = writer.as_mut() {
                let _ = writer.get_ref().shutdown(std::net::Shutdown::Both);
            }
            *writer = None;
        }
    }

    pub(super) fn send_command(&self, command: &str, payload: &Value) -> Option<u64> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        self.send_command_inner(Some(id), command, payload)
            .then_some(id)
    }

    pub(super) fn send_command_notification(&self, command: &str, payload: &Value) {
        let _ = self.send_command_inner(None, command, payload);
    }

    fn send_command_inner(&self, id: Option<u64>, command: &str, payload: &Value) -> bool {
        let mut request = json!({ "command": command });
        if let Some(id) = id {
            if let Value::Object(ref mut req_map) = request {
                req_map.insert("id".to_owned(), Value::from(id));
            }
        }

        if let Value::Object(map) = payload {
            if let Value::Object(ref mut req_map) = request {
                for (key, value) in map {
                    req_map.insert(key.clone(), value.clone());
                }
            }
        }

        if let Ok(mut writer) = self.writer.lock() {
            let Some(writer) = writer.as_mut() else {
                return false;
            };
            let line = serde_json::to_string(&request).unwrap_or_default();
            return writer.write_all(line.as_bytes()).is_ok()
                && writer.write_all(b"\n").is_ok()
                && writer.flush().is_ok();
        }
        false
    }

    pub(super) fn is_connected(&self) -> bool {
        self.writer
            .lock()
            .map(|writer| writer.is_some())
            .unwrap_or(false)
    }
}
