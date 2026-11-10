use crate::{PresentMode, PresentationOptions, Renderer, RunUi};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = info)]
    fn log_renderer(message: &str);
}

pub struct SurfaceRenderer {
    pub renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    size: [u32; 2],
    present_modes: Vec<PresentMode>,
}
impl SurfaceRenderer {
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        presentation: PresentationOptions,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::LowPower,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let info = adapter.get_info();
        let description = format!(
            "Onda UI renderer: backend={:?}; adapter={}; device={:?}; driver={} {}",
            info.backend, info.name, info.device_type, info.driver, info.driver_info,
        );
        #[cfg(not(target_arch = "wasm32"))]
        eprintln!("{description}");
        #[cfg(target_arch = "wasm32")]
        log_renderer(&description);
        let caps = surface.get_capabilities(&adapter);
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or("No compatible UI surface configuration")?;
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(config.format);
        let present_modes: Vec<_> = caps
            .present_modes
            .into_iter()
            .map(PresentMode::from)
            .collect();
        config.present_mode = presentation.resolve(&present_modes)?;
        config.desired_maximum_frame_latency = presentation.maximum_frame_latency;
        let renderer = Renderer::new(&adapter, config.format).await?;
        surface.configure(&renderer.device, &config);
        Ok(Self {
            renderer,
            surface,
            config,
            size: [width, height],
            present_modes,
        })
    }
    pub fn present_modes(&self) -> &[PresentMode] {
        &self.present_modes
    }
    pub fn present_mode_mask(&self) -> u32 {
        self.present_modes
            .iter()
            .fold(0, |mask, mode| mask | (1 << *mode as u32))
    }
    pub fn presentation(&self) -> PresentationOptions {
        PresentationOptions {
            present_mode: self.config.present_mode.into(),
            maximum_frame_latency: self.config.desired_maximum_frame_latency,
        }
    }
    pub fn set_presentation(&mut self, options: PresentationOptions) -> Result<(), String> {
        let present_mode = options.resolve(&self.present_modes)?;
        if present_mode != self.config.present_mode
            || options.maximum_frame_latency != self.config.desired_maximum_frame_latency
        {
            self.config.present_mode = present_mode;
            self.config.desired_maximum_frame_latency = options.maximum_frame_latency;
            if !self.size.contains(&0) {
                self.surface.configure(&self.renderer.device, &self.config);
            }
        }
        Ok(())
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.size == [width, height] {
            return;
        }
        self.size = [width, height];
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.renderer.device, &self.config);
    }
    /// False means the surface is temporarily unavailable; keep the frame dirty
    /// and retry on a later host display tick. Acquisition or presentation can wait
    /// for GPU/vsync backpressure; this function does not perform software pacing.
    pub fn render(&mut self, ui: &mut RunUi, scale: f32) -> Result<bool, String> {
        if self.size.contains(&0) {
            return Ok(false);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.renderer.device, &self.config);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err("GPU surface was lost; reopen the run view".into());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("UI surface validation failed".into());
            }
        };
        let view = frame.texture.create_view(&Default::default());
        self.renderer
            .render(ui, &view, self.config.width, self.config.height, scale)?;
        self.renderer.queue.present(frame);
        Ok(true)
    }
}
