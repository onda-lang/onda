use super::*;
use onda_daemon::{PreparedRunBuffer, RetiredRunBuffer};

const LOAD_QUEUE_CAPACITY: usize = 16;

pub(super) enum BufferLoadStatus {
    Applied,
    Superseded,
}

pub(super) fn buffer_load_response(id: Value, result: Result<BufferLoadStatus, String>) -> Value {
    match result {
        Ok(BufferLoadStatus::Applied) => json!({ "id": id, "ok": true }),
        Ok(BufferLoadStatus::Superseded) => json!({ "id": id, "ok": true, "cancelled": true }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    }
}

pub(super) fn submit_buffer_load(
    name: Option<String>,
    path: Option<String>,
    control: &mpsc::Sender<PlaybackControlCommand>,
) -> Result<mpsc::Receiver<Result<BufferLoadStatus, String>>, String> {
    let name = name.ok_or_else(|| "bindBufferWav requires 'name'".to_owned())?;
    let path = path.ok_or_else(|| "bindBufferWav requires 'path'".to_owned())?;
    let (reply, result) = mpsc::channel();
    control
        .send(PlaybackControlCommand::BindBufferWav {
            name,
            path: path.into(),
            reply,
        })
        .map_err(|_| "run control channel closed".to_owned())?;
    Ok(result)
}

pub(super) struct PendingBufferReply {
    pub id: Option<Value>,
    pub result: mpsc::Receiver<Result<BufferLoadStatus, String>>,
}

pub(super) fn write_buffer_replies(
    pending: &mut Vec<PendingBufferReply>,
    writer: &mut impl Write,
) -> Result<(), String> {
    let mut index = 0;
    while index < pending.len() {
        let result = match pending[index].result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => {
                index += 1;
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("run control reply channel closed".to_owned())
            }
        };
        let reply = pending.remove(index);
        if let Some(id) = reply.id {
            let response = buffer_load_response(id, result);
            write_json_line(writer, &response)
                .map_err(|error| format!("failed to write buffer response: {error}"))?;
        }
    }
    Ok(())
}

struct LoadRequest {
    name: String,
    path: PathBuf,
    revision: u64,
    reply: PlaybackReply<BufferLoadStatus>,
}

pub(super) struct LoadedBuffer {
    pub name: String,
    revision: u64,
    pub reply: PlaybackReply<BufferLoadStatus>,
    pub prepared: Result<Option<PreparedRunBuffer>, String>,
}

struct Reclaim {
    _prepared: Option<PreparedRunBuffer>,
    _retired: Option<RetiredRunBuffer>,
}

/// The render producer only submits paths and receives owned prepared storage.
/// One worker bounds concurrent decoding; revisions make bind/clear ordering
/// independent of I/O completion. Replaced storage is reclaimed on that worker.
pub(super) struct BufferWorker {
    requests: mpsc::SyncSender<LoadRequest>,
    completed: mpsc::Receiver<LoadedBuffer>,
    reclaim: mpsc::Sender<Reclaim>,
    revisions: HashMap<String, u64>,
}

impl BufferWorker {
    pub fn new() -> Self {
        Self::with_loader(PreparedRunBuffer::load_file)
    }

    fn with_loader(
        mut load: impl FnMut(&Path) -> Result<PreparedRunBuffer, onda_frontend::Diagnostic>
            + Send
            + 'static,
    ) -> Self {
        let (requests, incoming) = mpsc::sync_channel::<LoadRequest>(LOAD_QUEUE_CAPACITY);
        let (outgoing, completed) = mpsc::sync_channel(1);
        let (reclaim, retired) = mpsc::channel::<Reclaim>();
        thread::spawn(move || loop {
            while let Ok(storage) = retired.try_recv() {
                drop(storage);
            }
            let request = match incoming.recv_timeout(Duration::from_millis(10)) {
                Ok(request) => request,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let prepared = load(&request.path)
                .map(Some)
                .map_err(|diag| format_single_diagnostic("daemon play bind buffer failed", &diag));
            if outgoing
                .send(LoadedBuffer {
                    name: request.name,
                    revision: request.revision,
                    reply: request.reply,
                    prepared,
                })
                .is_err()
            {
                break;
            }
        });
        Self {
            requests,
            completed,
            reclaim,
            revisions: HashMap::new(),
        }
    }

    pub fn invalidate(&mut self, name: &str) -> u64 {
        let revision = self.revisions.entry(name.to_owned()).or_default();
        *revision = revision.checked_add(1).expect("buffer revision exhausted");
        *revision
    }

    pub fn load(&mut self, name: String, path: PathBuf, reply: PlaybackReply<BufferLoadStatus>) {
        let revision = self.invalidate(&name);
        if let Err(error) = self.requests.try_send(LoadRequest {
            name,
            path,
            revision,
            reply,
        }) {
            let (mpsc::TrySendError::Full(request) | mpsc::TrySendError::Disconnected(request)) =
                error;
            let _ = request
                .reply
                .send(Err("buffer loader is busy or unavailable".to_owned()));
        }
    }

    pub fn poll(&self) -> Option<LoadedBuffer> {
        self.completed.try_recv().ok()
    }

    pub fn is_current(&self, loaded: &LoadedBuffer) -> bool {
        self.revisions.get(&loaded.name) == Some(&loaded.revision)
    }

    pub fn reclaim(&self, prepared: Option<PreparedRunBuffer>, retired: Option<RetiredRunBuffer>) {
        // This channel remains connected for the worker's lifetime. On worker
        // failure the send error owns storage and is safely reclaimed here.
        let _ = self.reclaim.send(Reclaim {
            _prepared: prepared,
            _retired: retired,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_client_can_clear_and_shutdown_while_a_load_is_pending() {
        check_bind_clear_reply_order(false);
    }

    #[test]
    fn completed_bind_reply_precedes_a_later_clear_reply() {
        check_bind_clear_reply_order(true);
    }

    fn check_bind_clear_reply_order(complete_bind: bool) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let (control_tx, control_rx) = mpsc::channel();
        let (_output_tx, output_rx) = mpsc::channel();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let context = RunControlServerContext {
            control_tx,
            scope_ring: Arc::new(Mutex::new(ScopeRing::new(0, 0))),
            stop_flag: stop_flag.clone(),
            output_rx,
            dropped_delegate_occurrences: Arc::new(AtomicU32::new(0)),
            pending_delegate_overflow: Arc::new(AtomicU32::new(0)),
            dropped_print_occurrences: Arc::new(AtomicU32::new(0)),
            pending_print_overflow: Arc::new(AtomicU32::new(0)),
        };
        let handler = thread::spawn(move || handle_run_control_client(server, &context, 1));
        write_json_line(
            &mut client,
            &json!({ "id": 1, "command": "bindBufferWav", "name": "src", "path": "slow.wav" }),
        )
        .unwrap();
        let PlaybackControlCommand::BindBufferWav {
            reply: pending_load,
            ..
        } = control_rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("expected load");
        };
        write_json_line(
            &mut client,
            &json!({ "id": 2, "command": "clearBuffer", "name": "src" }),
        )
        .unwrap();
        let PlaybackControlCommand::ClearBuffer { reply, .. } =
            control_rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("expected clear before load completes");
        };
        // The server is waiting for the clear reply. Simulate a bind that
        // committed before the render producer processed that clear.
        if complete_bind {
            pending_load.send(Ok(BufferLoadStatus::Applied)).unwrap();
        }
        reply.send(Ok(())).unwrap();
        let mut reader = BufReader::new(client);
        let mut response = String::new();
        if complete_bind {
            reader.read_line(&mut response).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&response).unwrap(),
                json!({ "id": 1, "ok": true })
            );
            response.clear();
        }
        reader.read_line(&mut response).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap(),
            json!({ "id": 2, "ok": true })
        );
        stop_flag.store(true, Ordering::Release);
        handler.join().unwrap().unwrap();
        assert!(pending_load.send(Ok(BufferLoadStatus::Superseded)).is_err());
    }

    #[test]
    fn slow_load_does_not_block_and_clear_invalidates_its_result() {
        let (started, start_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let mut worker = BufferWorker::with_loader(move |_| {
            started.send(thread::current().id()).unwrap();
            resume_rx.recv().unwrap();
            PreparedRunBuffer::from_asset(
                BufferAsset {
                    frames: 1,
                    channels: 1,
                    sample_rate: 48_000.0,
                    samples: onda_project::BufferSamples::F32(vec![0.5]),
                },
                None,
            )
        });
        let (reply, _) = mpsc::channel();
        worker.load("src".into(), "unused.wav".into(), reply);
        assert_ne!(
            start_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            thread::current().id()
        );
        assert!(worker.poll().is_none());
        worker.invalidate("src");
        resume.send(()).unwrap();
        let loaded = worker
            .completed
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(!worker.is_current(&loaded));
        worker.reclaim(loaded.prepared.unwrap(), None);
    }

    #[test]
    fn newer_bind_supersedes_only_its_own_slot_even_when_it_fails() {
        let mut worker = BufferWorker::with_loader(|_| {
            Err(onda_frontend::Diagnostic::runtime("decode failed", 0, 0))
        });
        let (reply, _) = mpsc::channel();
        worker.load("src".into(), "first.wav".into(), reply.clone());
        worker.load("other".into(), "other.wav".into(), reply.clone());
        worker.load("src".into(), "second.wav".into(), reply);
        let first = worker
            .completed
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(!worker.is_current(&first));
        let other = worker
            .completed
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(worker.is_current(&other));
        let second = worker
            .completed
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(worker.is_current(&second));
        assert!(second.prepared.is_err());
    }
}
