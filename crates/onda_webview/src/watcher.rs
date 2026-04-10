use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};

/// Watches a file for changes and calls `on_change` (debounced).
pub struct FileWatcher {
    // The debouncer must be kept alive for the watcher to remain active.
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

impl FileWatcher {
    /// Start watching `path` for modifications.
    ///
    /// `on_change` is called on a background thread after a 200ms debounce window.
    /// Returns `None` if watching fails (non-fatal — the preview still works, just
    /// without auto-restart on save).
    pub fn watch(path: &Path, on_change: impl Fn() + Send + 'static) -> Option<Self> {
        let (tx, rx) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(200), tx).ok()?;

        debouncer
            .watcher()
            .watch(path, notify::RecursiveMode::NonRecursive)
            .ok()?;

        std::thread::spawn(move || {
            while let Ok(Ok(events)) = rx.recv() {
                let dominated_by_any = events.iter().any(|e| e.kind == DebouncedEventKind::Any);
                if dominated_by_any {
                    on_change();
                }
            }
        });

        Some(Self {
            _debouncer: debouncer,
        })
    }
}
