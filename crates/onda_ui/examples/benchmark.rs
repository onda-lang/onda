//! Offscreen correctness check and bounded rendering benchmark. This exercises
//! the production renderer and shaders, including text and stereo scope uploads.
use onda_ui::{Renderer, RunUi};
use serde_json::json;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pollster::block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance.request_adapter(&Default::default()).await?;
    println!("Adapter: {:?}", adapter.get_info());
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::new(&adapter, format).await?;
    let (width, height) = (768, 840);
    let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI verification"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"path":"oscillators / electric.onda","status":"Playing · 48 kHz · 512 samples","running":true,
        "paramLayout":"knobs","params":(0..64).map(|i|json!({"name":format!("Parameter {}",i+1),"type":"f64","value":i as f64/64.0,
            "default":0.5,"rangeMin":0.0,"rangeMax":1.0,"scale":"linear"})).collect::<Vec<_>>()}}));
    let mut samples = vec![0.0; 2048];
    let mut times = Vec::new();
    for frame in 0..150 {
        for i in 0..1024 {
            samples[i * 2] = ((i as f32 * 0.03 + frame as f32 * 0.02).sin()) * 0.7;
            samples[i * 2 + 1] = ((i as f32 * 0.05 + frame as f32 * 0.01).sin()) * 0.5;
        }
        let start = Instant::now();
        ui.set_scope_interleaved(2, &samples);
        let submission = renderer.render(&mut ui, &view, width, height, 1.0)?;
        renderer.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })?;
        if frame >= 30 {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    println!(
        "64 controls, 1024 stereo samples, {width}×{height}: median {:.3} ms, p95 {:.3} ms (CPU + GPU completion; 120 warm frames)",
        times[60], times[114]
    );
    let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI pixels"),
        size: u64::from(width * height * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    renderer.device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let pixels = readback.get_mapped_range(..)?;
    let background = &pixels[0..4];
    let changed = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p.as_slice() != background)
        .count();
    assert!(
        changed > 20000,
        "Renderer produced no substantial UI content: {changed} pixels"
    );
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/onda-gpu-ui.png".into());
    let mut encoder = png::Encoder::new(std::fs::File::create(&path)?, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    println!("Verified {changed} non-background pixels; wrote {path}");
    Ok(())
}
