//! Native rendering comparison. The legacy OpenGL path uses Linux EGL pbuffers.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        linux::main()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("The native comparison currently requires Linux/EGL".into())
    }
}
#[cfg(target_os = "linux")]
mod linux {
    //! Matched input/window benchmark of production egui panels and the shared UI.
    //! GPU completion is synchronized for both paths; neither includes presentation.
    use glow::HasContext;
    use glutin::{
        config::{ConfigSurfaceTypes, ConfigTemplateBuilder},
        context::{ContextApi, ContextAttributesBuilder, Version},
        prelude::*,
        surface::{PbufferSurface, SurfaceAttributesBuilder},
    };
    use onda_ui::{Renderer, RunUi};
    use serde_json::{Value, json};
    use std::{
        ffi::CString,
        num::NonZeroU32,
        sync::Arc,
        time::{Duration, Instant},
    };

    fn statistics(mut values: Vec<f64>) -> Value {
        values.sort_by(f64::total_cmp);
        json!({"median_ms":values[values.len()/2],"p95_ms":values[values.len()*95/100],"mean_ms":values.iter().sum::<f64>()/values.len() as f64})
    }
    fn params() -> Vec<Value> {
        (0..64)
            .map(|i| {
                json!({"name":format!("Parameter {}",i+1),"type":"f64","scalar":true,
    "value":i as f64/64.0,"default":0.5,"rangeMin":0.0,"rangeMax":1.0,"scale":"linear"})
            })
            .collect()
    }
    fn samples(frame: usize) -> Vec<f32> {
        (0..1024)
            .flat_map(|i| {
                [
                    (i as f32 * 0.03 + frame as f32 * 0.02).sin() * 0.7,
                    (i as f32 * 0.05 + frame as f32 * 0.01).sin() * 0.5,
                ]
            })
            .collect()
    }

    pub fn main() -> Result<(), Box<dyn std::error::Error>> {
        pollster::block_on(run())
    }
    async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let frames: Vec<_> = (0..360).map(samples).collect();
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance.request_adapter(&Default::default()).await?;
        let mut gpu = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb).await?;
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("comparison"),
            size: wgpu::Extent3d {
                width: 768,
                height: 840,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut output = json!({"window":[768,840],"controls":64,"channels":2,"samples_per_channel":1024,
        "warmup_min_frames":60,"warmup_min_ms":250,"measured_frames":300,"wgpu_adapter":format!("{:?}",adapter.get_info()),"results":[]});

        // EGL device contexts render to a pbuffer and do not open a desktop window.
        let egl_index = std::env::var("ONDA_BENCH_EGL_DEVICE")
            .ok()
            .map(|s| s.parse::<usize>())
            .transpose()?
            .unwrap_or(0);
        let device = glutin::api::egl::device::Device::query_devices()?
            .nth(egl_index)
            .ok_or("No selected EGL device")?;
        let display = unsafe { glutin::api::egl::display::Display::with_device(&device, None) }?;
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_surface_type(ConfigSurfaceTypes::PBUFFER)
            .build();
        let config = unsafe { display.find_configs(template) }?
            .next()
            .ok_or("No EGL pbuffer configuration")?;
        let surface = unsafe {
            display.create_pbuffer_surface(
                &config,
                &SurfaceAttributesBuilder::<PbufferSurface>::new()
                    .build(NonZeroU32::new(768).unwrap(), NonZeroU32::new(840).unwrap()),
            )
        }?;
        let context = unsafe {
            display.create_context(
                &config,
                &ContextAttributesBuilder::new()
                    .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
                    .build(None),
            )
        }?
        .make_current(&surface)?;
        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function(|name| {
                display.get_proc_address(&CString::new(name).unwrap())
            })
        });
        output["opengl_adapter"] = json!(unsafe { gl.get_parameter_string(glow::RENDERER) });
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None, true)?;
        for changing in [0, 1, 64] {
            let values: Vec<Vec<Value>> = (0..360)
                .map(|frame| {
                    (0..64)
                        .map(|i| {
                            json!(if i < changing {
                                ((frame + i) % 100) as f64 / 100.0
                            } else {
                                i as f64 / 64.0
                            })
                        })
                        .collect()
                })
                .collect();
            let updates: Vec<Value> = values.iter().map(|v| {
                json!({"type":"parameterValues","state":{"params":v.iter().take(changing).enumerate()
                    .map(|(i,v)|json!({"name":format!("Parameter {}",i+1),"value":v})).collect::<Vec<_>>()}})
            }).collect();
            // Alternate implementations within each round to reduce clock/thermal bias.
            for round in 0..3 {
                for old in if round % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let mut scene = onda_egui::benchmark::Scene::new(params());
                    let mut ui = RunUi::default();
                    ui.message(json!({"type":"state","state":{"path":"electric.onda","status":"Playing","running":true,
                    "supportsTransport":true,"paramLayout":"knobs","params":params()}}));
                    let mut cpu = vec![];
                    let mut total = vec![];
                    let mut publication = vec![];
                    let mut scope = vec![];
                    let mut rendering = vec![];
                    // A few dozen fast frames do not give CPU/GPU clocks time to settle.
                    let warmup = Instant::now();
                    for tick in 0.. {
                        let frame = tick % frames.len();
                        let measuring =
                            tick >= 60 && warmup.elapsed() >= Duration::from_millis(250);
                        let start = Instant::now();
                        if old {
                            let (output, primitives) = scene.render(
                                &frames[frame],
                                (changing > 0).then_some(values[frame].as_slice()),
                                tick as u32,
                            );
                            unsafe {
                                gl.clear_color(0.018, 0.028, 0.042, 1.0);
                                gl.clear(glow::COLOR_BUFFER_BIT);
                            }
                            painter.paint_and_update_textures(
                                [768, 840],
                                output.pixels_per_point,
                                &primitives,
                                &output.textures_delta,
                            );
                            let submitted = start.elapsed().as_secs_f64() * 1000.0;
                            unsafe {
                                gl.finish();
                            }
                            if measuring {
                                cpu.push(submitted);
                                total.push(start.elapsed().as_secs_f64() * 1000.0);
                            }
                        } else {
                            if changing > 0 {
                                ui.message(updates[frame].clone());
                            }
                            let updated = start.elapsed().as_secs_f64() * 1000.0;
                            ui.set_scope_interleaved(2, &frames[frame]);
                            let reduced = start.elapsed().as_secs_f64() * 1000.0;
                            let submitted = gpu.render(&mut ui, &view, 768, 840, 1.0)?;
                            let encoded = start.elapsed().as_secs_f64() * 1000.0;
                            gpu.device.poll(wgpu::PollType::Wait {
                                submission_index: Some(submitted),
                                timeout: Some(Duration::from_secs(5)),
                            })?;
                            if measuring {
                                cpu.push(encoded);
                                total.push(start.elapsed().as_secs_f64() * 1000.0);
                                publication.push(updated);
                                scope.push(reduced - updated);
                                rendering.push(encoded - reduced);
                            }
                        }
                        if cpu.len() == 300 {
                            break;
                        }
                    }
                    let mut result = json!({"implementation":if old {"egui_glow"} else {"shared_wgpu"},"scenario":match changing {0=>"scope_only",1=>"scope_and_1_value",_=>"scope_and_64_values"},
                    "round":round,"cpu_submit":statistics(cpu),"cpu_and_gpu":statistics(total)});
                    if !old {
                        result["value_publication"] = statistics(publication);
                        result["scope_reduction"] = statistics(scope);
                        result["render_submit"] = statistics(rendering);
                    }
                    eprintln!("{result}");
                    output["results"].as_array_mut().unwrap().push(result);
                }
            }
        }
        painter.destroy();
        drop(context);
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}
