//! Full production capture path versus the previous writer kernel. CPU only.
use std::{hint::black_box, time::Instant};
#[path = "../../../../crates/onda_run/src/playback/scope_capture.rs"]
mod scope_capture;
use scope_capture::ScopeCapture;
struct Current {
    buffer: Vec<f32>,
    channels: usize,
    write_pos: usize,
    frames_written: usize,
}
impl Current {
    #[inline(never)]
    fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buffer.len();
        if cap == 0 {
            return;
        }
        for (i, &sample) in samples.iter().enumerate() {
            self.buffer[(self.write_pos + i) % cap] = sample;
        }
        self.write_pos = (self.write_pos + samples.len()) % cap;
        self.frames_written += samples.len() / self.channels.max(1);
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn main() {
    println!("{{\"rounds\":7,\"block_frames\":512,\"rows\":[");
    for (index, channels) in [2, 8, 64].into_iter().enumerate() {
        let frames = black_box(512);
        let mut previous = Current {
            buffer: vec![0.0; 4096 * channels],
            channels,
            write_pos: 0,
            frames_written: 0,
        };
        let capture = ScopeCapture::default();
        capture.reconfigure(4096, channels);
        let data: Vec<_> = (0..frames * channels).map(|i| i as f32).collect();
        let mut samples: [Vec<f64>; 3] = Default::default();
        for round in 0..7 {
            for variant in if round % 2 == 0 { [0, 1, 2] } else { [2, 1, 0] } {
                capture.set_enabled(variant == 1).unwrap();
                let iterations = if variant == 0 { 10000 } else { 100000 };
                let start = Instant::now();
                for _ in 0..iterations {
                    if variant == 0 {
                        previous.push_interleaved(black_box(&data));
                    } else {
                        black_box(&capture).push_interleaved(black_box(&data));
                    }
                }
                samples[variant].push(start.elapsed().as_secs_f64() * 1e9 / iterations as f64);
            }
        }
        black_box(previous.buffer);
        if index > 0 {
            println!(",");
        }
        println!("{{\"channels\":{channels},\"previous_writer_ns\":{},\"enabled_capture_ns\":{},\"disabled_capture_ns\":{}}}",median(samples[0].clone()),median(samples[1].clone()),median(samples[2].clone()));
    }
    println!("]}}");
}
