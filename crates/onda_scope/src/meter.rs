//! Bounded per-channel level capture. Only block maxima cross the audio/UI boundary.
use crate::MAX_CHANNELS;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub channels: usize,
    pub peaks: [f32; MAX_CHANNELS],
}
impl Default for Levels {
    fn default() -> Self {
        Self {
            channels: 0,
            peaks: [0.0; MAX_CHANNELS],
        }
    }
}
impl Levels {
    pub fn values(&self) -> &[f32] {
        &self.peaks[..self.channels.min(MAX_CHANNELS)]
    }
}

pub struct MeterCapture {
    enabled: AtomicBool,
    channels: AtomicUsize,
    // Zero means no new block; positive float bits + 1 include genuine silence.
    peaks: [AtomicU32; MAX_CHANNELS],
    last: [AtomicU32; MAX_CHANNELS],
}
impl Default for MeterCapture {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            channels: AtomicUsize::new(0),
            peaks: std::array::from_fn(|_| AtomicU32::new(1)),
            last: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}
impl MeterCapture {
    pub fn set_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.clear();
        }
    }
    pub fn configure(&self, channels: usize) {
        self.clear();
        self.channels.store(channels, Ordering::Release);
    }
    fn clear(&self) {
        for peak in &self.peaks {
            peak.store(1, Ordering::Relaxed);
        }
    }
    pub fn push_interleaved(&self, samples: &[f32]) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let channels = self.channels.load(Ordering::Acquire);
        if channels == 0 || samples.len() < channels {
            return;
        }
        let mut peaks = [0.0_f32; MAX_CHANNELS];
        for frame in samples.chunks_exact(channels) {
            for (channel, &sample) in frame.iter().take(MAX_CHANNELS).enumerate() {
                if sample.is_finite() {
                    peaks[channel] = peaks[channel].max(sample.abs());
                }
            }
        }
        for (target, peak) in self.peaks.iter().zip(peaks).take(channels) {
            // Nonnegative finite IEEE floats have the same ordering as their bits.
            target.fetch_max(peak.to_bits() + 1, Ordering::Relaxed);
        }
    }
    /// Consume accumulated maxima, retaining the last reading if no new audio
    /// arrived. A new silent block clears that reading.
    pub fn take(&self) -> Levels {
        let channels = self.channels.load(Ordering::Acquire).min(MAX_CHANNELS);
        if !self.enabled.load(Ordering::Acquire) {
            return Levels {
                channels,
                ..Default::default()
            };
        }
        Levels {
            channels,
            peaks: std::array::from_fn(|i| {
                if i < channels {
                    let peak = self.peaks[i].swap(0, Ordering::Relaxed);
                    if peak != 0 {
                        self.last[i].store(peak - 1, Ordering::Relaxed);
                    }
                    f32::from_bits(self.last[i].load(Ordering::Relaxed))
                } else {
                    0.0
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn polling_between_audio_blocks_does_not_report_silence() {
        let capture = MeterCapture::default();
        capture.configure(2);
        capture.set_enabled(true);
        let mut next_audio_frame = 0;
        // A 48 kHz stream with 2048-frame blocks and a 30 Hz display.
        for display_frame in (0..48_000).step_by(1600) {
            while next_audio_frame <= display_frame {
                capture.push_interleaved(&[0.25, -0.5]);
                next_audio_frame += 2048;
            }
            assert_eq!(capture.take().values(), &[0.25, 0.5]);
            capture.push_interleaved(&[]);
            capture.push_interleaved(&[0.0]);
            assert_eq!(capture.take().values(), &[0.25, 0.5]);
        }
        capture.push_interleaved(&[0.0, 0.0]);
        assert_eq!(capture.take().values(), &[0.0, 0.0]);
        assert_eq!(capture.take().values(), &[0.0, 0.0]);
    }

    #[test]
    fn concurrent_capture_does_not_insert_silence_between_blocks() {
        let capture = std::sync::Arc::new(MeterCapture::default());
        capture.configure(2);
        capture.set_enabled(true);
        capture.push_interleaved(&[0.25, -0.5]);
        let producer = capture.clone();
        let producer = std::thread::spawn(move || {
            for _ in 0..10_000 {
                producer.push_interleaved(&[0.25, -0.5]);
            }
        });
        for _ in 0..10_000 {
            assert_eq!(capture.take().values(), &[0.25, 0.5]);
        }
        producer.join().expect("meter producer panicked");
        capture.set_enabled(false);
        assert_eq!(capture.take().values(), &[0.0, 0.0]);
        capture.set_enabled(true);
        assert_eq!(capture.take().values(), &[0.0, 0.0]);
    }

    #[test]
    fn captures_each_channel_without_cancellation_and_retains_unreplaced_peaks() {
        let capture = MeterCapture::default();
        capture.configure(4);
        capture.set_enabled(true);
        capture.push_interleaved(&[0.2, -1.2, 0.7, -0.4, 0.1, 0.1, 0.1, 0.1]);
        assert_eq!(capture.take().values(), &[0.2, 1.2, 0.7, 0.4]);
        assert_eq!(capture.take().values(), &[0.2, 1.2, 0.7, 0.4]);
        capture.configure(8);
        assert_eq!(capture.take().values(), &[0.0; 8]);
        capture.push_interleaved(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.3, -1.3]);
        let result = capture.take();
        assert_eq!(result.channels, 8);
        assert_eq!(result.values(), &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.3, 1.3]);
        capture.set_enabled(false);
        capture.push_interleaved(&[1.0; 8]);
        assert_eq!(capture.take().values(), &[0.0; 8]);

        capture.configure(MAX_CHANNELS);
        capture.set_enabled(true);
        let mut samples = vec![0.0; MAX_CHANNELS * 2];
        samples[0] = f32::NAN;
        samples[MAX_CHANNELS - 1] = -1.5;
        samples[MAX_CHANNELS] = 0.2;
        samples[MAX_CHANNELS + 1] = f32::INFINITY;
        capture.push_interleaved(&samples);
        let result = capture.take();
        assert_eq!(result.values().len(), MAX_CHANNELS);
        assert_eq!(result.peaks[0], 0.2);
        assert_eq!(result.peaks[MAX_CHANNELS - 1], 1.5);
        assert!(result.peaks[1..MAX_CHANNELS - 1]
            .iter()
            .all(|&peak| peak == 0.0));
        assert_eq!(capture.take(), result);
    }
}
