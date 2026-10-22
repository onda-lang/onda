//! Optional compiler allocation counters. LLVM's C++ allocations are excluded;
//! Linux VmHWM supplies a separate process-wide peak resident memory measure.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

struct Allocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

fn record(bytes: usize) {
    if ENABLED.load(Relaxed) {
        CALLS.fetch_add(1, Relaxed);
        BYTES.fetch_add(bytes as u64, Relaxed);
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

pub fn begin(enabled: bool) {
    CALLS.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    ENABLED.store(enabled, Relaxed);
}

pub fn finish() -> Option<serde_json::Value> {
    if !ENABLED.swap(false, Relaxed) {
        return None;
    }
    let calls = CALLS.load(Relaxed);
    let bytes = BYTES.load(Relaxed);
    let peak_rss_kib = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
            })
        });
    Some(
        serde_json::json!({ "rust_allocation_calls": calls, "rust_requested_bytes": bytes, "process_peak_rss_kib": peak_rss_kib }),
    )
}
