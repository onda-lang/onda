use crate::{Renderer, RunUi};

pub struct SurfaceRenderer {
    pub renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    size: [u32; 2],
}
impl SurfaceRenderer {
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::LowPower,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
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
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 1;
        let renderer = Renderer::new(&adapter, config.format).await?;
        surface.configure(&renderer.device, &config);
        Ok(Self {
            renderer,
            surface,
            config,
            size: [width, height],
        })
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
    /// and retry on the host's next display tick, without blocking its event loop.
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
