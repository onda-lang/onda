//! Pixel checks catch surface color-space errors that scene tests cannot detect.
use crate::{scene::Palette, Renderer, RunUi};
use serde_json::json;

#[test]
fn horizontal_log_scrolling_keeps_output_reachable() {
    pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
            .await
            .unwrap();
        let render = |renderer: &mut Renderer, ui: &mut RunUi, width| {
            let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("log scrolling regression"),
                size: wgpu::Extent3d {
                    width,
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
            renderer
                .render(
                    ui,
                    &texture.create_view(&Default::default()),
                    width,
                    840,
                    1.0,
                )
                .unwrap();
            let [x, y, _, _] = ui.scene.log_rect;
            ui.input(crate::Input::PointerMove {
                x: x + 10.0,
                y: y + 10.0,
                fine: false,
            });
        };
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"log.onda","supportsScope":false,"supportsViewState":true,
            "logText":"Hello\n","logRevealed":true
        }}));
        render(&mut renderer, &mut ui, 768);
        assert!(
            !ui.scroll(300.0, true),
            "fitting output must not pan offscreen"
        );

        // The longest line still determines the extent when it is vertically offscreen.
        let text = format!("{}\n{}", "Long output ".repeat(80), "Hello\n".repeat(30));
        ui.message(json!({"type":"state","state":{"logText":text}}));
        render(&mut renderer, &mut ui, 768);
        assert!(ui.scroll(1_000_000.0, true));
        let narrow_end = ui.presentation.log_x;
        assert!(narrow_end > 0.0 && narrow_end < 10_000.0);
        assert!(
            !ui.scroll(1.0, true),
            "panning must stop at the last character"
        );
        render(&mut renderer, &mut ui, 1024);
        assert!(
            ui.presentation.log_x < narrow_end,
            "resizing must clamp the offset"
        );
        // Context occupies a fixed gutter; changing only its metadata must update the bound.
        let wide_end = ui.presentation.log_x;
        ui.message(json!({"type":"state","state":{"logEntries":[{
            "source":{"file":"long/path/to/the/source.onda","line":12}
        }]}}));
        render(&mut renderer, &mut ui, 1024);
        assert!(ui.scroll(1_000_000.0, true));
        assert!(ui.presentation.log_x > wide_end);

        ui.message(json!({"type":"state","state":{"viewState":{
            "readyId":"oversized-scroll","logScrollX":1_000_000.0
        }}}));
        render(&mut renderer, &mut ui, 1024);
        assert!(
            ui.presentation.log_x < 10_000.0,
            "restored offsets must also be bounded"
        );

        ui.message(json!({"type":"state","state":{"logText":"Short again\n"}}));
        render(&mut renderer, &mut ui, 768);
        assert_eq!(ui.presentation.log_x, 0.0);
        assert!(!ui.scroll(300.0, true));

        ui.message(json!({"type":"state","state":{"logText":text}}));
        render(&mut renderer, &mut ui, 768);
        assert!(ui.scroll(300.0, true));
        ui.prepare_scene();
        let clear = ui.scene.hits.iter().find(|hit| {
            matches!(&hit.target, crate::scene::Target::Command(command) if command["type"] == "clearLog")
        }).unwrap().rect.0;
        ui.input(crate::Input::PointerDown {
            x: clear[0] + clear[2] * 0.5,
            y: clear[1] + clear[3] * 0.5,
            clicks: 1,
        });
        ui.input(crate::Input::PointerUp);
        assert_eq!(
            ui.presentation.log_x, 0.0,
            "Clear must reset both scroll offsets"
        );
        ui.message(json!({"type":"state","state":{"logText":"New output\n"}}));
        render(&mut renderer, &mut ui, 768);
        assert!(!ui.scroll(300.0, true));
        ui.input(crate::Input::FocusLost);
        let commands = ui.take_commands();
        let saved = commands
            .iter()
            .rev()
            .find(|command| command["type"] == "viewState")
            .unwrap();
        assert_eq!(saved["state"]["logScrollX"], 0.0);
    });
}

fn read_pixels(renderer: &Renderer, texture: &wgpu::Texture) -> Vec<u8> {
    let width = texture.width();
    let height = texture.height();
    let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rendered pixels"),
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
        texture.size(),
    );
    renderer.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        tx.send(result).unwrap();
    });
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    readback.get_mapped_range(..).unwrap().to_vec()
}

#[test]
fn circular_strokes_keep_antialiased_edges_at_all_ui_sizes() {
    pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
            eprintln!("No GPU adapter available for logo rendering checks");
            return;
        };
        let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
            .await
            .unwrap();
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let background = linear(Palette::new(false).background[0]);
        for scale in [1.0, 1.25, 1.5, 2.0, 4.0] {
            let width = (256.0 * scale) as u32;
            let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("circular stroke edge regression"),
                size: wgpu::Extent3d {
                    width,
                    height: width,
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
            for (size, thin_ring) in [(76.0, false), (32.0, false), (64.0, true)] {
                for offset in [0.0, 0.35] {
                    let mut ui = RunUi::default();
                    ui.resize(256.0, 256.0, scale);
                    ui.prepare_scene();
                    ui.scene = crate::scene::Scene::default();
                    ui.scene.clip = [0.0, 0.0, 256.0, 256.0];
                    let center = 128.0 + offset;
                    if thin_ring {
                        ui.scene.ring([center, center], size * 0.5, 1.0, [1.0; 4]);
                    } else {
                        let origin = center - size * 0.5;
                        ui.scene.logo([origin, origin, size, size], [1.0; 4]);
                    }
                    ui.scene.scope_shape_end = ui.scene.shapes.len();
                    ui.scene.overlay_shapes = ui.scene.shapes.len();
                    ui.scene.menu_shapes = ui.scene.shapes.len();
                    renderer
                        .render(&mut ui, &view, width, width, scale)
                        .unwrap();
                    let pixels = read_pixels(&renderer, &texture);
                    // Include both edges of a thin ring, where derivatives can
                    // cross its centerline, and the silhouette of each logo.
                    let center = center * scale;
                    let radius = if thin_ring {
                        size * 0.5
                    } else {
                        168.0 * size / 512.0 * 1.3
                    } * scale;
                    for y in 0..width {
                        for x in 0..width {
                            let radial =
                                (x as f32 + 0.5 - center).hypot(y as f32 + 0.5 - center) - radius;
                            let distance = if thin_ring {
                                radial.abs() - 0.5 * scale
                            } else {
                                radial
                            };
                            if distance.abs() > 0.5 {
                                continue;
                            }
                            let t = (distance + 0.5).clamp(0.0, 1.0);
                            let expected = 1.0 - t * t * (3.0 - 2.0 * t);
                            let red = pixels[((y * width + x) * 4) as usize] as f32 / 255.0;
                            let actual = (linear(red) - background) / (1.0 - background);
                            assert!(
                                (actual - expected).abs() < 0.16,
                                "rough circular stroke: size={size}, thin_ring={thin_ring}, scale={scale}, offset={offset}, \
                                 pixel=({x},{y}), coverage={actual}, expected={expected}"
                            );
                        }
                    }
                }
            }
        }
    });
}

#[test]
fn opening_settings_after_text_rendering_preserves_valid_gpu_bindings() {
    pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
            eprintln!("No GPU adapter available for popup rendering checks");
            return;
        };
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut renderer = Renderer::new(&adapter, format).await.unwrap();
        let (width, height) = (768, 840);
        let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("settings popup regression"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"dual_fm_osc.onda", "supportsRunSettings":true,
            "supportsDeviceSelection":true,"inputDevices":["Input A","Input B"],
            "sampleRateHz":48000, "blockSamples":512
        }}));
        renderer.render(&mut ui, &view, width, height, 1.0).unwrap();
        for open in [true, false, true] {
            ui.prepare_scene();
            let button = ui
                .scene
                .hits
                .iter()
                .find(|hit| hit.target == crate::scene::Target::Devices)
                .expect("settings button")
                .rect
                .0;
            ui.input(crate::Input::PointerDown {
                x: button[0] + button[2] * 0.5,
                y: button[1] + button[3] * 0.5,
                clicks: 1,
            });
            ui.input(crate::Input::PointerUp);
            assert_eq!(ui.interaction.settings_open(), open);
            let scope = renderer
                .device
                .push_error_scope(wgpu::ErrorFilter::Validation);
            renderer.render(&mut ui, &view, width, height, 1.0).unwrap();
            if open {
                ui.prepare_scene();
                let select = ui
                    .scene
                    .hits
                    .iter()
                    .find(|h| {
                        h.target
                            == crate::scene::Target::Choose(
                                crate::interaction::SelectId::AudioInput,
                            )
                    })
                    .expect("input dropdown")
                    .rect
                    .0;
                ui.input(crate::Input::PointerDown {
                    x: select[0] + select[2] * 0.5,
                    y: select[1] + select[3] * 0.5,
                    clicks: 1,
                });
                ui.input(crate::Input::PointerUp);
                assert!(ui.interaction.menu().is_some());
                renderer.render(&mut ui, &view, width, height, 1.0).unwrap();
            }
            renderer
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "settings popup GPU validation failed: {error:?}"
            );
        }
    });
}

#[test]
fn both_surface_formats_preserve_the_theme_and_use_one_scope_color() {
    pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
            eprintln!("No GPU adapter available for pixel checks");
            return;
        };
        for format in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
        ] {
            let mut renderer = Renderer::new(&adapter, format).await.unwrap();
            let (width, height) = (768, 840);
            let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("theme regression"),
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
            for light in [false, true] {
                let mut ui = RunUi::default();
                ui.message(json!({"type":"state","state":{"path":"colors.onda","running":true,"themeMode":if light{"light"}else{"dark"}}}));
                ui.set_scope_interleaved(2, &[0.25, 0.25, 0.25, 0.25]);
                renderer.render(&mut ui, &view, width, height, 1.0).unwrap();
                let pixels = read_pixels(&renderer, &texture);
                let p = Palette::new(light);
                let matches = |pixel: &[u8], color: [f32; 4]| {
                    (0..3).all(|i| (pixel[i] as f32 - color[i] * 255.0).abs() <= 1.0)
                };
                assert!(
                    matches(&pixels[..4], p.background),
                    "{format:?}, light={light}: {:?}",
                    &pixels[..4]
                );
                let [x, y, w, h] = ui.scene.scope_rect;
                for channel in 0..2 {
                    let top = (y + h * channel as f32 / 2.0).ceil() as usize;
                    let bottom = (y + h * (channel + 1) as f32 / 2.0).floor() as usize;
                    let left = x.ceil() as usize;
                    let right = (x + w).floor() as usize;
                    assert!(
                        (top..bottom)
                            .flat_map(|row| {
                                let start = (row * width as usize + left) * 4;
                                let end = (row * width as usize + right) * 4;
                                pixels[start..end].as_chunks::<4>().0.iter()
                            })
                            .filter(|pixel| matches(pixel.as_slice(), p.accent))
                            .count()
                            > 200,
                        "channel {channel} must use the first channel's color on {format:?}, light={light}"
                    );
                }
            }
        }
    });
}
