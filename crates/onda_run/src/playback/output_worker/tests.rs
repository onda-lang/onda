use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;
thread_local! { static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }

fn count() {
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn dense_midi_and_packed_output_do_not_allocate_on_producer() {
    let path = std::env::temp_dir().join(format!(
        "onda_output_allocations_{}.onda",
        std::process::id()
    ));
    std::fs::write(&path, "delegate report(key: i32)\nevent note_on(id: i32, channel: i32, key: i32, velocity: f32):\n  report(key)\n  print(velocity)\nsample:\n  out1 = 0.0\n").unwrap();
    let mut session = DaemonSession::default();
    session
        .start_run_with_options(
            &path,
            RunOptions {
                block_size: 128,
                ..Default::default()
            },
        )
        .unwrap();
    let run = session.run_mut(&path).unwrap();
    run.set_delegate_collection_enabled(true);
    let (sender, receiver) = mpsc::sync_channel(1);
    let output = OutputTransport {
        sender,
        dropped_delegates: Arc::new(AtomicU32::new(0)),
        pending_delegate_overflow: Arc::new(AtomicU32::new(0)),
        dropped_prints: Arc::new(AtomicU32::new(0)),
        pending_print_overflow: Arc::new(AtomicU32::new(0)),
    };
    let (sender, _prints) = mpsc::sync_channel(1);
    let prints = PrintTransport {
        sender,
        dropped_occurrences: Arc::new(AtomicU32::new(0)),
        pending_overflow: Arc::new(AtomicU32::new(0)),
    };
    let worker = OutputWorker::new(run, Some(output.clone()), prints);
    let mut events = Vec::with_capacity(MAX_PENDING_EVENTS);
    let mut notes = HashSet::with_capacity(16 * 128);
    let mut rendered = [0.0; 128];
    let mut latencies = [0.0_f64; 256];
    ALLOCATIONS.with(|count| count.set(Some(0)));
    for (block, latency) in latencies.iter_mut().enumerate() {
        let started = Instant::now();
        for frame in 0..128 {
            let message = MidiMessage {
                kind: midi::MidiMessageKind::NoteOn,
                channel: 0,
                key_or_controller: frame as i32,
                value: 0.5,
            };
            let (name, values) = message.event();
            events.push(PendingRunEvent {
                frame,
                sequence: frame,
                name: Cow::Borrowed(name),
                values: PendingEventValues::Midi(values),
                physical_midi: Some(message),
                reply: None,
            });
        }
        events.sort_unstable_by_key(|event| (event.frame, event.sequence));
        let scheduled = events.iter().map(|event| RunScheduledEvent {
            frame: event.frame as usize,
            name: &event.name,
            values: &event.values,
        });
        run.render_block_event_iter_interleaved(&mut rendered, scheduled)
            .unwrap();
        worker.publish(run, block as u64).unwrap();
        reply_to_run_events(events.drain(..), Ok(()), &mut notes);
        *latency = started.elapsed().as_secs_f64() * 1e6;
    }
    let allocations = ALLOCATIONS.with(|count| count.replace(None).unwrap());
    assert_eq!(allocations, 0);
    drop(worker); // Drains the worker before observing transport counters.
    let first = receiver.try_recv().unwrap();
    assert_eq!(first.delegates.occurrences.len(), 128);
    assert_eq!(first.prints.entries.len(), 128);
    assert_eq!(first.delegates.occurrences[0].sequence, 0);
    assert_eq!(first.prints.entries[0].sequence, 1);
    assert_eq!(output.dropped_delegates.load(Ordering::Relaxed), 255 * 128);
    assert_eq!(output.dropped_prints.load(Ordering::Relaxed), 255 * 128);
    latencies.sort_by(f64::total_cmp);
    eprintln!("dense MIDI + output producer: allocations={allocations}, p99={:.2}us, max={:.2}us (debug build, uncontrolled host)", latencies[253], latencies[255]);
    std::fs::remove_file(path).unwrap();
}
