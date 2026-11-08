//! Optional host-owned cadence arithmetic. No clock, waiting, or rendering policy.
//!
//! Choose one pair for a pacer's lifetime:
//! - Timer hosts: [`FramePacer::timer_delay_ms`] and [`FramePacer::timer_submitted`].
//! - Display callbacks: [`FramePacer::display_delay_ms`] and [`FramePacer::display_submitted`].
//! Record only successful submissions. Neither pair measures when pixels reach the display.

/// Optional cadence arithmetic using host-supplied timestamps.
/// Pair `timer_delay_ms` with `timer_submitted`, or `display_delay_ms` with
/// `display_submitted`. Record successful renders using the documented timestamp.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct FramePacer {
    interval_ms: f64,
    origin_ms: f64,
    next_frame: u64,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
impl FramePacer {
    /// None is uncapped. Otherwise the rate must be finite and positive.
    #[cfg_attr(
        target_arch = "wasm32",
        wasm_bindgen::prelude::wasm_bindgen(constructor)
    )]
    pub fn new(rate_hz: Option<f64>) -> Result<FramePacer, String> {
        let interval_ms = match rate_hz {
            None => 0.0,
            Some(rate) if rate.is_finite() && rate > 0.0 && (1000.0 / rate).is_finite() => {
                1000.0 / rate
            }
            _ => return Err("Frame rate must be finite and positive".into()),
        };
        Ok(Self {
            interval_ms,
            origin_ms: 0.0,
            next_frame: 0,
        })
    }

    /// Milliseconds until a timer-driven frame is due; zero means ready.
    /// Supply finite, monotonic host-clock milliseconds. Round positive delays up
    /// to the timer's resolution. Repeated queries never postpone the deadline.
    /// Pair with [`Self::timer_submitted`].
    pub fn timer_delay_ms(&self, now_ms: f64) -> f64 {
        if self.interval_ms == 0.0 || self.next_frame == 0 {
            return 0.0;
        }
        let deadline = self.origin_ms + self.next_frame as f64 * self.interval_ms;
        remaining_ms(deadline, now_ms)
    }

    /// Milliseconds until the next display slot opens; zero means ready.
    /// Each slot is centered on its nominal deadline. Use host-clock milliseconds
    /// to arm a display callback early, then check again with the callback's display
    /// timestamp (the RAF argument on the web), rather than its execution time.
    /// Pair with [`Self::display_submitted`].
    pub fn display_delay_ms(&self, now_ms: f64) -> f64 {
        self.timer_delay_ms(now_ms + self.interval_ms * 0.5)
    }

    /// After a successful render, record the host-clock timestamp captured
    /// immediately BEFORE rendering. Do not pass completion time.
    /// Preserves phase and skips expired deadlines without queuing catch-up frames.
    /// Pair with [`Self::timer_delay_ms`].
    pub fn timer_submitted(&mut self, started_ms: f64) {
        self.advance(started_ms, 0.0);
    }

    /// After a successful render, record the callback's display timestamp
    /// (the RAF argument on the web). Do not pass callback execution or completion time.
    /// Preserves phase and skips expired slots without queuing catch-up frames.
    /// Pair with [`Self::display_delay_ms`].
    pub fn display_submitted(&mut self, display_ms: f64) {
        self.advance(display_ms, self.interval_ms * 0.5);
    }
}

impl FramePacer {
    fn advance(&mut self, started_ms: f64, lead_ms: f64) {
        if self.interval_ms == 0.0 {
            return;
        }
        if self.next_frame == 0 {
            self.origin_ms = started_ms;
            self.next_frame = 1;
            return;
        }
        let cutoff_ms = started_ms + lead_ms;
        let elapsed = ((cutoff_ms - self.origin_ms) / self.interval_ms).floor() as u64;
        self.next_frame = self
            .next_frame
            .saturating_add(1)
            .max(elapsed.saturating_add(1));
        // Division and deadline reconstruction can round opposite ways at a boundary.
        let deadline = self.origin_ms + self.next_frame as f64 * self.interval_ms;
        if remaining_ms(deadline, cutoff_ms) == 0.0 {
            self.next_frame = self.next_frame.saturating_add(1);
        }
    }
}

fn remaining_ms(deadline: f64, now_ms: f64) -> f64 {
    let remaining = deadline - now_ms;
    // Only account for floating-point roundoff, not scheduling jitter.
    let roundoff = deadline.abs().max(now_ms.abs()).max(1.0) * f64::EPSILON * 4.0;
    if remaining <= roundoff {
        0.0
    } else {
        remaining
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_rates_follow_display_opportunities_without_phase_drift() {
        for rate in [
            23.976_f64, 24.0, 29.97, 30.0, 59.94, 60.0, 90.0, 120.0, 144.0,
        ] {
            for display in [59.94_f64, 60.0, 75.0, 119.88, 120.0, 144.0, 240.0, 360.0] {
                for jitter in [0.0, 0.1] {
                    let mut pacer = FramePacer::new(Some(rate)).unwrap();
                    let mut frames = 0;
                    let mut occupied_slots = std::collections::BTreeSet::new();
                    for tick in 0..(display * 10.0) as u32 {
                        let offset = if tick % 2 == 0 { jitter } else { -jitter };
                        let now = tick as f64 * 1000.0 / display + offset;
                        let slot = (now - jitter) * rate / 1000.0 + 0.5;
                        let roundoff = slot.abs().max(1.0) * f64::EPSILON * 4.0;
                        occupied_slots.insert((slot + roundoff).floor() as u64);
                        if pacer.display_delay_ms(now) == 0.0 {
                            pacer.display_submitted(now);
                            frames += 1;
                            assert!(pacer.display_delay_ms(now) > 0.0);
                        }
                    }
                    // Jitter can leave a slot empty while placing two display
                    // opportunities in another; draw once per occupied slot.
                    assert_eq!(
                        frames,
                        occupied_slots.len(),
                        "{rate} fps on {display} Hz with {jitter} ms jitter: {frames}"
                    );
                }
            }
        }
    }

    #[test]
    fn irregular_display_opportunities_preserve_the_cadence_phase() {
        for (rate, display, jitter, expected) in [(60.0, 75.0, 3.0, 525), (120.0, 144.0, 2.0, 960)]
        {
            for epoch in [0.0, 1e9, 1e12] {
                let mut pacer = FramePacer::new(Some(rate)).unwrap();
                let mut frames = 0;
                for tick in 0..(display * 10.0) as u32 {
                    let offset = if tick % 2 == 0 { jitter } else { -jitter };
                    let now = epoch + tick as f64 * 1000.0 / display + offset;
                    if pacer.display_delay_ms(now) == 0.0 {
                        pacer.display_submitted(now);
                        frames += 1;
                        assert!(pacer.display_delay_ms(now) > 0.0);
                    }
                }
                assert_eq!(
                    frames, expected,
                    "{rate} fps on {display} Hz at epoch {epoch}"
                );
            }
        }
    }

    #[test]
    fn display_slots_do_not_advance_timer_deadlines() {
        let mut pacer = FramePacer::new(Some(60.0)).unwrap();
        pacer.display_submitted(0.1);
        let early_frame = 1000.0 / 60.0 - 0.1;
        assert_eq!(pacer.display_delay_ms(early_frame), 0.0);
        assert!(pacer.timer_delay_ms(early_frame) > 0.0);
        pacer.display_submitted(early_frame);
        assert!(pacer.display_delay_ms(early_frame) > 0.0);
        assert!((pacer.timer_delay_ms(early_frame) - (1000.0 / 60.0 + 0.2)).abs() < 1e-10);
    }

    #[test]
    fn timer_frames_keep_phase_without_waiting_for_display_slots() {
        let mut pacer = FramePacer::new(Some(60.0)).unwrap();
        pacer.timer_submitted(3.0);
        pacer.timer_submitted(29.667);
        assert!((pacer.timer_delay_ms(29.667) - (3.0 + 2000.0 / 60.0 - 29.667)).abs() < 1e-10);
        pacer.timer_submitted(105.0);
        assert!((pacer.timer_delay_ms(105.0) - (3.0 + 7000.0 / 60.0 - 105.0)).abs() < 1e-10);
    }

    #[test]
    fn queries_preserve_deadlines_and_stalls_do_not_queue_frames() {
        let mut pacer = FramePacer::new(Some(60.0)).unwrap();
        pacer.timer_submitted(0.0);
        for now in [0.0, 1.0, 4.0, 10.0, 16.0] {
            assert!((pacer.timer_delay_ms(now) + now - 1000.0 / 60.0).abs() < 1e-10);
        }
        for now in [100.0, 3_600_000.0] {
            assert_eq!(pacer.timer_delay_ms(now), 0.0);
            pacer.timer_submitted(now);
            assert!((pacer.timer_delay_ms(now) - 1000.0 / 60.0).abs() < 1e-6);
        }
    }

    #[test]
    fn uncapped_mode_never_defers_and_invalid_rates_are_rejected() {
        let mut pacer = FramePacer::new(None).unwrap();
        for now in [0.0, 0.01, 1.0, 100.0] {
            pacer.timer_submitted(now);
            assert_eq!(pacer.timer_delay_ms(now), 0.0);
        }
        for rate in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::MIN_POSITIVE / 1000.0,
        ] {
            assert!(FramePacer::new(Some(rate)).is_err());
        }
    }
}
