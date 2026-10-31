//! A compiler-free host using its own GPU device, render target and parameter.
//! Run with `cargo run -p onda_ui --example host`.
use onda_ui::{Input, Key, Renderer, RunUi};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pollster::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // These resources belong to the application and can also render other content.
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance.request_adapter(&Default::default()).await?;
    let (device, queue) = adapter.request_device(&Default::default()).await?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let (width, height, scale) = (480, 360, 1.0);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("host UI texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let mut renderer = Renderer::from_device(device.clone(), queue.clone(), format);
    let mut ui = RunUi::default();
    let mut gain = 0.25;
    ui.resize(width as f32 / scale, height as f32 / scale, scale);
    ui.message(json!({"type":"state","state":{
        "path":"host.onda", "status":"Host-owned parameter",
        "supportsTransport":false, "supportsScope":false,
        "params":[{"name":"gain", "type":"f64", "value":gain,
            "default":0.25, "rangeMin":0.0, "rangeMax":1.0}]
    }}));

    // A window host translates platform events into Input. Keyboard traversal
    // lets this headless example exercise editing without hard-coded coordinates.
    for _ in 0..8 {
        renderer.input(
            &mut ui,
            Input::Key {
                key: Key::Tab,
                shift: false,
            },
        );
        if ui.editing() {
            break;
        }
    }
    assert!(ui.editing(), "gain field must be reachable");
    renderer.input(
        &mut ui,
        Input::Text {
            text: "0.75".into(),
        },
    );
    renderer.input(
        &mut ui,
        Input::Key {
            key: Key::Enter,
            shift: false,
        },
    );
    for command in ui.take_commands() {
        if command["type"] == "setParam" && command["name"] == "gain" {
            gain = command["value"].as_f64().expect("finite gain");
            // Apply the host's value, then acknowledge it without replacing metadata.
            ui.message(json!({"type":"parameterValues","state":{
                "params":[{"name":"gain","value":gain}]
            }}));
        }
    }
    assert_eq!(gain, 0.75);

    // render clears and submits this texture. The application can sample it in
    // its own next pass; a surface host presents through SurfaceRenderer instead.
    let submission = renderer.render(
        &mut ui,
        &target.create_view(&Default::default()),
        width,
        height,
        scale,
    )?;
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(std::time::Duration::from_secs(10)),
    })?;
    println!("Rendered {width}×{height} on the host device; gain = {gain}");
    Ok(())
}
