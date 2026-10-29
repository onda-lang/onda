//! Producer capture is opt-in. Disabling it skips the lock and copy entirely.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

pub(super) struct ScopeCapture {
    enabled: AtomicBool,
    ring: Mutex<ScopeRing>,
}
impl Default for ScopeCapture {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            ring: Mutex::new(ScopeRing::new(0, 0)),
        }
    }
}
impl ScopeCapture {
    pub(super) fn subscription(&self) -> impl Drop + '_ {
        Subscription(self)
    }
    pub(super) fn reconfigure(&self, frames: usize, channels: usize) {
        if let Ok(mut ring) = self.ring.lock() {
            *ring = ScopeRing::new(frames, channels);
        }
    }
    pub(super) fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut ring = self
            .ring
            .lock()
            .map_err(|_| "failed to lock scope ring".to_owned())?;
        if self.enabled.swap(enabled, Ordering::Release) != enabled {
            ring.frames_written = 0;
        }
        Ok(())
    }
    pub(super) fn push_interleaved(&self, samples: &[f32]) {
        if self.enabled.load(Ordering::Acquire) {
            if let Ok(mut ring) = self.ring.try_lock() {
                // The subscription may have changed while acquiring the ring.
                if self.enabled.load(Ordering::Relaxed) {
                    ring.push_interleaved(samples);
                }
            }
        }
    }
    pub(super) fn snapshot(&self, frames: usize) -> Result<ScopeSnapshot, String> {
        self.ring
            .lock()
            .map(|ring| ring.snapshot(frames))
            .map_err(|_| "failed to lock scope ring".to_owned())
    }
    pub(super) fn envelope(
        &self,
        frames: usize,
        columns: usize,
    ) -> Result<onda_scope::Envelope, String> {
        self.ring
            .lock()
            .map(|ring| ring.envelope(frames, columns))
            .map_err(|_| "failed to lock scope ring".to_owned())
    }
}

struct Subscription<'a>(&'a ScopeCapture);
impl Drop for Subscription<'_> {
    fn drop(&mut self) {
        let _ = self.0.set_enabled(false);
    }
}

pub(super) struct ScopeSnapshot {
    pub(super) channels: usize,
    pub(super) samples: Vec<f32>,
}

struct ScopeRing {
    buffer: Vec<f32>,
    channels: usize,
    write_pos: usize,
    frames_written: usize,
}

impl ScopeRing {
    fn new(capacity_frames: usize, channels: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity_frames * channels],
            channels,
            write_pos: 0,
            frames_written: 0,
        }
    }

    fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buffer.len();
        if cap == 0 {
            return;
        }
        // Only the newest capacity of samples can survive an oversized block.
        let skipped = samples.len().saturating_sub(cap);
        let samples = &samples[skipped..];
        let start = (self.write_pos + skipped % cap) % cap;
        let first = samples.len().min(cap - start);
        self.buffer[start..start + first].copy_from_slice(&samples[..first]);
        self.buffer[..samples.len() - first].copy_from_slice(&samples[first..]);
        self.write_pos = (start + samples.len()) % cap;
        self.frames_written = self
            .frames_written
            .saturating_add((skipped + samples.len()) / self.channels.max(1));
    }

    fn window(&self, max_frames: usize) -> (usize, usize) {
        let cap = self.buffer.len();
        if cap == 0 {
            return (0, 0);
        }
        let frames = max_frames
            .min(self.frames_written)
            .min(cap / self.channels.max(1));
        (
            frames,
            (self.write_pos + cap - frames * self.channels) % cap,
        )
    }
    fn snapshot(&self, max_frames: usize) -> ScopeSnapshot {
        let (frames, start) = self.window(max_frames);
        ScopeSnapshot {
            channels: self.channels,
            samples: {
                let len = frames * self.channels;
                let first = len.min(self.buffer.len() - start);
                let mut samples = Vec::with_capacity(len);
                samples.extend_from_slice(&self.buffer[start..start + first]);
                samples.extend_from_slice(&self.buffer[..len - first]);
                samples
            },
        }
    }
    fn envelope(&self, max_frames: usize, columns: usize) -> onda_scope::Envelope {
        let (frames, start) = self.window(max_frames);
        let mut envelope = onda_scope::Envelope::default();
        envelope.update(self.channels, frames, columns, |frame, ch| {
            self.buffer[(start + frame * self.channels + ch) % self.buffer.len()]
        });
        envelope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_envelope_preserves_wrapped_ring_extrema_and_recent_window() {
        let mut ring = ScopeRing::new(4, 2);
        ring.push_interleaved(&[0.1, 0.2, 1.0, -1.0, -0.5, 0.5, 0.0, 0.0, 0.7, -0.7]);
        let mut expected = onda_scope::Envelope::default();
        expected.set_interleaved(2, &ring.snapshot(4).samples, 2);
        assert_eq!(ring.envelope(4, 2), expected);
        assert_eq!(
            expected.ranges(),
            &[[-16384, 32767], [0, 22937], [-32767, 16384], [-22937, 0]]
        );
        expected.set_interleaved(2, &ring.snapshot(2).samples, 1);
        assert_eq!(ring.envelope(2, 1), expected);
        assert_eq!(ScopeRing::new(0, 0).envelope(100, 100).columns(), 0);
    }

    #[test]
    fn capture_is_opt_in_and_reconfiguration_preserves_subscription() {
        let capture = ScopeCapture::default();
        capture.reconfigure(4, 2);
        capture.push_interleaved(&[1.0, 2.0]);
        assert!(capture.snapshot(4).unwrap().samples.is_empty());
        capture.set_enabled(true).unwrap();
        capture.push_interleaved(&[1.0, 2.0]);
        assert_eq!(capture.snapshot(4).unwrap().samples, [1.0, 2.0]);
        capture.reconfigure(4, 1);
        capture.push_interleaved(&[3.0]);
        assert_eq!(capture.snapshot(4).unwrap().samples, [3.0]);
        capture.set_enabled(false).unwrap();
        capture.push_interleaved(&[4.0]);
        assert!(capture.snapshot(4).unwrap().samples.is_empty());
        capture.set_enabled(true).unwrap();
        assert!(capture.snapshot(4).unwrap().samples.is_empty());
    }
    #[test]
    fn dropping_a_client_subscription_disables_and_clears_capture() {
        let capture = ScopeCapture::default();
        capture.reconfigure(4, 1);
        {
            let _subscription = capture.subscription();
            capture.set_enabled(true).unwrap();
            capture.push_interleaved(&[1.0]);
            assert_eq!(capture.snapshot(4).unwrap().samples, [1.0]);
        }
        capture.push_interleaved(&[2.0]);
        assert!(capture.snapshot(4).unwrap().samples.is_empty());
    }

    #[test]
    fn bulk_writes_match_sample_by_sample_ring_for_wraps_and_oversized_blocks() {
        for channels in [1, 2, 8] {
            let mut ring = ScopeRing::new(7, channels);
            let mut all = Vec::new();
            for frames in [0, 1, 6, 2, 7, 29, 3] {
                let block: Vec<_> = (0..frames * channels)
                    .map(|i| (all.len() + i) as f32)
                    .collect();
                ring.push_interleaved(&block);
                all.extend(block);
                for window in [0, 1, 4, 7, 100] {
                    let len = all.len().min(window.min(7) * channels);
                    assert_eq!(ring.snapshot(window).samples, all[all.len() - len..]);
                }
            }
        }
    }
}
