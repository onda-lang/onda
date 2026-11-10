use super::*;
use std::os::fd::AsRawFd;

#[test]
fn accepted_control_client_clears_inherited_nonblocking_mode() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (server, _) = listener.accept().unwrap();
    // BSD/macOS accept inherits this flag from the nonblocking listener.
    // Set it explicitly so Linux exercises the same accepted-socket state.
    server.set_nonblocking(true).unwrap();
    let probe = server.try_clone().unwrap();
    let (control_tx, _control_rx) = mpsc::channel();
    let (_output_tx, output_rx) = mpsc::channel();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let context = RunControlServerContext {
        control_tx,
        scope_ring: Arc::new(ScopeCapture::default()),
        stop_flag: stop_flag.clone(),
        output_rx,
        dropped_delegate_occurrences: Arc::new(AtomicU32::new(0)),
        pending_delegate_overflow: Arc::new(AtomicU32::new(0)),
        dropped_print_occurrences: Arc::new(AtomicU32::new(0)),
        pending_print_overflow: Arc::new(AtomicU32::new(0)),
    };
    let handler = thread::spawn(move || handle_run_control_client(server, &context, 1));
    write_json_line(&mut client, &json!({"id":1,"command":"unknown"})).unwrap();
    let mut reader = BufReader::new(client);
    let mut reply = String::new();
    std::io::BufRead::read_line(&mut reader, &mut reply).unwrap();
    let response: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(response["id"], 1);
    assert_eq!(response["ok"], false);
    // Receiving a response establishes that the real handler configured the
    // connection. Inspect the shared descriptor without relying on CPU timing.
    let flags = unsafe { libc::fcntl(probe.as_raw_fd(), libc::F_GETFL) };
    stop_flag.store(true, Ordering::Release);
    reader.get_ref().shutdown(std::net::Shutdown::Both).unwrap();
    handler.join().unwrap().unwrap();
    assert_ne!(flags, -1);
    assert_eq!(
        flags & libc::O_NONBLOCK,
        0,
        "idle control reads must block; a read timeout does not clear O_NONBLOCK"
    );
}
