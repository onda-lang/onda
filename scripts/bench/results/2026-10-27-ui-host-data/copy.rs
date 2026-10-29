
use std::{hint::black_box, time::Instant};
struct Current { buffer: Vec<f32>, channels: usize, write_pos: usize, frames_written: usize }
struct Bulk { buffer: Vec<f32>, channels: usize, write_pos: usize, frames_written: usize, enabled: bool }
impl Current {     #[inline(never)]
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
    } }
impl Bulk {     #[inline(never)]
    fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buffer.len();
        if cap == 0 || !self.enabled {
            return;
        }
        let samples = &samples[samples.len().saturating_sub(cap)..];
        let first = samples.len().min(cap - self.write_pos);
        self.buffer[self.write_pos..self.write_pos + first].copy_from_slice(&samples[..first]);
        self.buffer[..samples.len() - first].copy_from_slice(&samples[first..]);
        self.write_pos = (self.write_pos + samples.len()) % cap;
        self.frames_written += samples.len() / self.channels.max(1);
    } }
fn same(a: &Current, b: &Bulk) {
    let available_a = (a.frames_written * a.channels).min(a.buffer.len());
    let available_b = (b.frames_written * b.channels).min(b.buffer.len());
    assert_eq!(available_a, available_b);
    for i in 0..available_a {
        let ia = (a.write_pos + a.buffer.len() - available_a + i) % a.buffer.len();
        let ib = (b.write_pos + b.buffer.len() - available_b + i) % b.buffer.len();
        assert_eq!(a.buffer[ia], b.buffer[ib]);
    }
}
fn main() {
    // Check short, wrapped, exact-capacity, and oversized blocks before timing.
    for channels in [1, 2, 8, 64] {
        let capacity = 17 * channels;
        let mut a = Current {buffer:vec![0.;capacity], channels, write_pos:0, frames_written:0};
        let mut b = Bulk {buffer:vec![0.;capacity], channels, write_pos:0, frames_written:0, enabled:true};
        for frames in [3, 16, 2, 17, 64, 1, 20] {
            let data: Vec<f32> = (0..frames * channels).map(|i| i as f32 + a.frames_written as f32).collect();
            a.push_interleaved(&data); b.push_interleaved(&data); same(&a, &b);
        }
    }
    println!("{{\"logical_windows_equal\":true,\"capacity_frames\":4096,\"rounds\":7,\"rows\":[");
    let mut first = true;
    for channels in [2, 8, 64] {
        for frames in [64, 512] {
            let capacity = black_box(4096 * channels);
            let mut a = Current {buffer:vec![0.;capacity], channels, write_pos:0, frames_written:0};
            let mut b = Bulk {buffer:vec![0.;capacity], channels, write_pos:0, frames_written:0, enabled:true};
            let data: Vec<f32> = (0..channels * frames).map(|i| (i as f32 * 0.1).sin()).collect();
            let calibrate = Instant::now();
            for _ in 0..1000 { a.push_interleaved(black_box(&data)); }
            let each = calibrate.elapsed().as_secs_f64() / 1000.;
            let iterations = (0.04 / each).clamp(1000., 2_000_000.) as usize;
            let mut current = vec![];
            let mut bulk = vec![];
            for round in 0..7 {
                for variant in if round % 2 == 0 { [0,1] } else { [1,0] } {
                    let start = Instant::now();
                    if variant == 0 {
                        for _ in 0..iterations { a.push_interleaved(black_box(&data)); }
                        current.push(start.elapsed().as_secs_f64() * 1e9 / iterations as f64);
                    } else {
                        for _ in 0..iterations { b.push_interleaved(black_box(&data)); }
                        bulk.push(start.elapsed().as_secs_f64() * 1e9 / iterations as f64);
                    }
                }
            }
            black_box(a.buffer); black_box(b.buffer);
            current.sort_by(f64::total_cmp); bulk.sort_by(f64::total_cmp);
            if !first { println!(","); } first = false;
            print!("{{\"channels\":{channels},\"block_frames\":{frames},\"iterations_per_round\":{iterations},\"current_ns\":{},\"bulk_ns\":{},\"ratio\":{}}}",current[3],bulk[3],current[3]/bulk[3]);
        }
    }
    println!("]}}");
}
