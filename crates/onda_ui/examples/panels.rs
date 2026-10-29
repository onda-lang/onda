//! Complete-screen timings and visual artifacts using the production renderer.
use onda_ui::{Renderer, RunUi};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn stats(mut xs: Vec<f64>) -> Value {
    xs.sort_by(f64::total_cmp);
    json!({"median_ms":xs[xs.len()/2],"p95_ms":xs[xs.len()*95/100],"mean_ms":xs.iter().sum::<f64>()/xs.len() as f64})
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    pollster::block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/ui-panels".into());
    std::fs::create_dir_all(&directory)?;
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance.request_adapter(&Default::default()).await?;
    let mut r = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb).await?;
    let (w, h) = (768, 840);
    let texture = r.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("complete UI panels"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let fixture: Value =
        serde_json::from_str(include_str!("../../../ui/gpu/fixtures/panels.json"))?;
    let mut report = json!({"adapter":format!("{:?}",adapter.get_info()),"viewport":[w,h],"metric":"CPU submission and CPU+GPU completion, offscreen; excludes presentation and host transport","warmup":60,"frames":240,"results":[]});
    for panel in [
        "params",
        "inputControls",
        "buffers",
        "events",
        "log",
        "midi",
        "settings",
    ] {
        for changing in [false, true] {
            let mut ui = RunUi::default();
            let mut state = fixture.clone();
            state["themeMode"] = json!(if std::env::args().any(|arg| arg == "--light") {
                "light"
            } else {
                "dark"
            });
            let sections: serde_json::Map<_, _> = [
                "scope",
                "params",
                "inputControls",
                "buffers",
                "events",
                "log",
                "midi",
            ]
            .into_iter()
            .map(|s| {
                (
                    s.to_owned(),
                    json!(s == "scope" || s == panel || s == "midi"),
                )
            })
            .collect();
            state["viewState"] = json!({"sections":sections});
            ui.message(json!({"type":"state","state":state}));
            ui.resize(w as f32, h as f32, 1.0);
            if panel == "settings" {
                ui.input(onda_ui::Input::PointerDown {
                    x: w as f32 - 112.0,
                    y: 48.0,
                    clicks: 1,
                });
            }
            let mut cpu = Vec::new();
            let mut total = Vec::new();
            let mut initial = 0.0;
            for frame in 0..300 {
                let scope: Vec<f32> = (0..1024)
                    .flat_map(|i| {
                        [
                            (i as f32 * 0.03 + frame as f32 * 0.02).sin() * 0.7,
                            (i as f32 * 0.05 + frame as f32 * 0.01).sin() * 0.5,
                        ]
                    })
                    .collect();
                let update = if changing {
                    Some(match panel {
                        "params" | "inputControls" => {
                            json!({"type":"parameterValues","state":{panel:fixture[panel].as_array().unwrap().iter().enumerate().map(|(i,p)|json!({"name":p["name"],"value":((frame+i)%100) as f64/100.0})).collect::<Vec<_>>()}})
                        }
                        "buffers" => {
                            let mut buffers = fixture["buffers"].clone();
                            for b in buffers.as_array_mut().unwrap() {
                                for (i, v) in b["waveform"]["maximums"]
                                    .as_array_mut()
                                    .unwrap()
                                    .iter_mut()
                                    .enumerate()
                                {
                                    *v = json!(((i + frame) as f64 * 0.11).sin().abs());
                                }
                            }
                            json!({"type":"state","state":{"buffers":buffers}})
                        }
                        "events" => {
                            let mut events = fixture["events"].clone();
                            events[0]["args"][0]["value"] = json!((frame % 100) as f64 / 100.0);
                            json!({"type":"state","state":{"events":events,"resetEventArguments":true}})
                        }
                        "log" => {
                            json!({"type":"state","state":{"logText":format!("{}Frame {frame}\n",fixture["logText"].as_str().unwrap())}})
                        }
                        "midi" => json!({"type":"midiActivity","activeNotes":[60+frame%24]}),
                        _ => json!({"type":"state","state":{"paramSmoothingMs":frame%100}}),
                    })
                } else {
                    None
                };
                let start = Instant::now();
                if let Some(update) = update {
                    ui.message(update);
                }
                ui.set_scope_interleaved(2, &scope);
                let submission = r.render(&mut ui, &view, w, h, 1.0)?;
                let submitted = start.elapsed().as_secs_f64() * 1000.0;
                r.device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(Duration::from_secs(10)),
                })?;
                let completed = start.elapsed().as_secs_f64() * 1000.0;
                if frame == 0 {
                    initial = completed;
                }
                if frame >= 60 {
                    cpu.push(submitted);
                    total.push(completed);
                }
            }
            report["results"].as_array_mut().unwrap().push(json!({"panel":panel,"scenario":if changing {"scope_and_panel_updates"}else{"scope_only"},"first_frame_ms":initial,"cpu":stats(cpu),"completion":stats(total)}));
            if !changing {
                screenshot(&r, &texture, w, h, &format!("{directory}/{panel}.png"))?;
            }
        }
    }
    std::fs::write(
        format!("{directory}/native.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
fn screenshot(
    r: &Renderer,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let readback = r.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI image"),
        size: u64::from(w * h * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = r.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    r.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    r.device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let pixels = readback.get_mapped_range(..)?;
    let mut png = png::Encoder::new(std::fs::File::create(path)?, w, h);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(&pixels)?;
    Ok(())
}
