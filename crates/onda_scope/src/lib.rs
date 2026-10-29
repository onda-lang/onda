//! Bounded, display-sized scope envelopes shared by native and browser hosts.
//! A bucket retains both extrema. Samples are clamped to the displayed [-1, 1]
//! range and quantized to signed 16-bit values (error <= 1/32767).

pub const MAX_CHANNELS: usize = 64;
pub const MAX_COLUMNS: usize = 512;
pub const MAX_FRAMES: usize = 16384;
const HEADER: usize = 8;
const MAGIC: &[u8; 4] = b"OSQ1";

/// Two physical pixels per bucket, capped independently of window size/DPI.
pub fn columns_for_width(width: f32) -> usize {
    if !width.is_finite() {
        return 1;
    }
    (width * 0.5).ceil().clamp(1.0, MAX_COLUMNS as f32) as usize
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    channels: usize,
    columns: usize,
    ranges: Vec<[i16; 2]>,
}
impl Envelope {
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn columns(&self) -> usize {
        self.columns
    }
    /// Channel-major [minimum, maximum] pairs.
    pub fn ranges(&self) -> &[[i16; 2]] {
        &self.ranges
    }
    pub fn clear(&mut self) {
        self.channels = 0;
        self.columns = 0;
        self.ranges.clear();
    }
    pub fn copy_from(&mut self, other: &Self) {
        self.channels = other.channels;
        self.columns = other.columns;
        self.ranges.clear();
        self.ranges.extend_from_slice(&other.ranges);
    }
    pub fn set_interleaved(&mut self, channels: usize, samples: &[f32], columns: usize) {
        self.update(
            channels,
            samples.len() / channels.max(1),
            columns,
            |frame, ch| samples[frame * channels + ch],
        );
    }
    /// The getter can read directly from a ring, avoiding a full PCM snapshot.
    pub fn update(
        &mut self,
        channels: usize,
        frames: usize,
        columns: usize,
        sample: impl Fn(usize, usize) -> f32,
    ) {
        let channels = channels.min(MAX_CHANNELS);
        let frames = frames.min(MAX_FRAMES);
        if channels == 0 || frames == 0 {
            self.clear();
            return;
        }
        let columns = columns.clamp(1, MAX_COLUMNS).min(frames);
        self.channels = channels;
        self.columns = columns;
        self.ranges.clear();
        self.ranges.reserve(channels * columns);
        for ch in 0..channels {
            for column in 0..columns {
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for frame in column * frames / columns..(column + 1) * frames / columns {
                    let value = sample(frame, ch);
                    let value = if value.is_finite() {
                        value.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    };
                    lo = lo.min(value);
                    hi = hi.max(value);
                }
                self.ranges
                    .push([(lo * 32767.0).round() as i16, (hi * 32767.0).round() as i16]);
            }
        }
    }
    /// OSQ1, u16 channels, u16 columns, then channel-major i16 LE extrema.
    pub fn packet(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.ranges.len() * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.channels as u16).to_le_bytes());
        out.extend_from_slice(&(self.columns as u16).to_le_bytes());
        for [lo, hi] in &self.ranges {
            out.extend_from_slice(&lo.to_le_bytes());
            out.extend_from_slice(&hi.to_le_bytes());
        }
        out
    }
    /// Validation completes before replacing the last good frame. Storage is reused.
    pub fn read_packet(&mut self, packet: &[u8]) -> Result<(), &'static str> {
        if packet.len() < HEADER || &packet[..4] != MAGIC {
            return Err("Invalid scope packet header");
        }
        let channels = u16::from_le_bytes([packet[4], packet[5]]) as usize;
        let columns = u16::from_le_bytes([packet[6], packet[7]]) as usize;
        if channels > MAX_CHANNELS
            || columns > MAX_COLUMNS
            || (channels == 0) != (columns == 0)
            || packet.len() != HEADER + channels * columns * 4
        {
            return Err("Invalid scope packet dimensions");
        }
        let pairs = || {
            packet[HEADER..].as_chunks::<4>().0.iter().map(|b| {
                [
                    i16::from_le_bytes([b[0], b[1]]),
                    i16::from_le_bytes([b[2], b[3]]),
                ]
            })
        };
        if pairs().any(|[lo, hi]| lo > hi) {
            return Err("Invalid scope extrema");
        }
        self.channels = channels;
        self.columns = columns;
        self.ranges.clear();
        self.ranges.extend(pairs());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limiting_displayed_channels_preserves_the_source_stride() {
        let mut samples = vec![0.0; 65 * 2];
        samples[65] = 1.0;
        let mut frame = Envelope::default();
        frame.set_interleaved(65, &samples, 1);
        assert_eq!(frame.channels(), MAX_CHANNELS);
        assert_eq!(frame.ranges()[0], [0, 32767]);
        assert!(frame.ranges()[1..].iter().all(|r| *r == [0, 0]));
    }
    #[test]
    fn buckets_preserve_impulses_remainders_and_channel_separation() {
        let mut frame = Envelope::default();
        let samples = [
            0.0, 0.1, 0.0, 0.1, 1.0, -1.0, 0.0, 0.1, 0.0, 0.1, 0.0, 0.1, -1.0, 1.0,
        ];
        frame.set_interleaved(2, &samples, 3);
        assert_eq!(
            frame.ranges(),
            &[
                [0, 0],
                [0, 32767],
                [-32767, 0],
                [3277, 3277],
                [-32767, 3277],
                [3277, 32767]
            ]
        );
    }
    #[test]
    fn quantization_is_below_a_display_pixel_and_cleans_nonfinite_input() {
        let samples: Vec<_> = (-1000..=1000).map(|i| i as f32 / 1000.0).collect();
        let mut frame = Envelope::default();
        frame.set_interleaved(1, &samples, MAX_COLUMNS);
        for (i, [lo, hi]) in frame.ranges().iter().enumerate() {
            let part = &samples
                [i * samples.len() / frame.columns()..(i + 1) * samples.len() / frame.columns()];
            assert!((f32::from(*lo) / 32767.0 - part[0]).abs() <= 1.0 / 32767.0);
            assert!((f32::from(*hi) / 32767.0 - part[part.len() - 1]).abs() <= 1.0 / 32767.0);
        }
        frame.set_interleaved(1, &[f32::NAN, f32::INFINITY, -2.0, 2.0], 4);
        assert_eq!(
            frame.ranges(),
            &[[0, 0], [0, 0], [-32767, -32767], [32767, 32767]]
        );
    }
    #[test]
    fn packets_roundtrip_and_reject_truncated_or_invalid_frames_transactionally() {
        let mut frame = Envelope::default();
        frame.set_interleaved(2, &[0.1, 0.2, -0.5, 1.0], 2);
        let packet = frame.packet();
        let mut decoded = Envelope::default();
        decoded.read_packet(&packet).unwrap();
        assert_eq!(decoded, frame);
        for end in 0..packet.len() {
            assert!(decoded.read_packet(&packet[..end]).is_err());
            assert_eq!(decoded, frame);
        }
        let mut invalid = packet.clone();
        invalid[8..12].copy_from_slice(&[1, 0, 0, 0]);
        assert!(decoded.read_packet(&invalid).is_err());
        assert_eq!(decoded, frame);
        decoded.read_packet(&Envelope::default().packet()).unwrap();
        assert_eq!(decoded, Envelope::default());
    }
    #[test]
    fn packet_size_is_bounded_by_display_budget() {
        let mut frame = Envelope::default();
        frame.update(2, 1024, columns_for_width(696.0), |i, _| {
            if i == 131 {
                1.0
            } else {
                0.0
            }
        });
        assert_eq!(frame.columns(), 348);
        assert_eq!(frame.packet().len(), 2792);
        assert!(frame.ranges().iter().any(|r| r[1] == 32767));
        assert_eq!(columns_for_width(f32::INFINITY), 1);
        assert_eq!(columns_for_width(100000.0), MAX_COLUMNS);
    }
}
