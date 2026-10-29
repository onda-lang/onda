use crate::{Input, RunUi, SurfaceRenderer};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace=console,js_name=error)]
    fn log_panic(message: &str);
}

#[wasm_bindgen]
pub struct CanvasUi {
    ui: RunUi,
    gpu: SurfaceRenderer,
    scale: f32,
    rendered: Option<(u64, u64)>,
}

#[wasm_bindgen]
impl CanvasUi {
    #[wasm_bindgen(js_name = create)]
    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        webgpu: bool,
    ) -> Result<CanvasUi, JsValue> {
        std::panic::set_hook(Box::new(|info| log_panic(&info.to_string())));
        let width = canvas.width();
        let height = canvas.height();
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = if webgpu {
            wgpu::Backends::BROWSER_WEBGPU
        } else {
            wgpu::Backends::GL
        };
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let gpu = SurfaceRenderer::new(instance, surface, width, height)
            .await
            .map_err(|e| JsValue::from_str(&e))?;
        Ok(Self {
            ui: RunUi::default(),
            gpu,
            scale: 1.0,
            rendered: None,
        })
    }
    pub fn message(&mut self, message: &str) -> Result<(), JsValue> {
        self.ui
            .message(serde_json::from_str(message).map_err(|e| JsValue::from_str(&e.to_string()))?);
        Ok(())
    }
    pub fn scope_samples(&mut self, channels: usize, samples: &[f32]) {
        self.ui.set_scope_interleaved(channels, samples);
    }
    pub fn scope_packet(&mut self, packet: &[u8]) -> Result<(), JsValue> {
        self.ui.set_scope_packet(packet).map_err(JsValue::from_str)
    }
    pub fn scope_enabled(&self) -> bool {
        self.ui.scope_enabled()
    }
    pub fn scope_columns(&self) -> usize {
        self.ui.scope_columns()
    }
    pub fn input(&mut self, input: &str) -> Result<(), JsValue> {
        let input: Input =
            serde_json::from_str(input).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.ui.input(input);
        Ok(())
    }
    pub fn scroll(&mut self, delta: f32, horizontal: bool) -> bool {
        self.ui.scroll(delta, horizontal)
    }
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        self.rendered = None;
        self.scale = scale;
        self.gpu.resize(width, height);
        self.ui
            .resize(width as f32 / scale, height as f32 / scale, scale);
    }
    pub fn render(&mut self) -> Result<bool, JsValue> {
        self.ui.tick();
        if self.rendered == Some(self.ui.revisions()) {
            return Ok(true);
        }
        let rendered = self
            .gpu
            .render(&mut self.ui, self.scale)
            .map_err(|e| JsValue::from_str(&e))?;
        if rendered {
            self.rendered = Some(self.ui.revisions());
        }
        Ok(rendered)
    }
    pub fn commands(&mut self) -> String {
        serde_json::to_string(&self.ui.take_commands()).unwrap_or_else(|_| "[]".into())
    }
    pub fn view_state_delay(&self) -> Option<f64> {
        self.ui.view_state_deadline().map(|deadline| {
            deadline
                .saturating_duration_since(web_time::Instant::now())
                .as_secs_f64()
                * 1000.0
        })
    }
    pub fn repaint_delay(&self) -> Option<f64> {
        self.ui.repaint_deadline().map(|deadline| {
            deadline
                .saturating_duration_since(web_time::Instant::now())
                .as_secs_f64()
                * 1000.0
        })
    }
    pub fn buffer_target(&mut self, x: f32, y: f32) -> Option<String> {
        self.ui.buffer_target(x, y)
    }
    pub fn cursor(&mut self) -> String {
        self.ui.cursor().into()
    }
    pub fn editing(&self) -> bool {
        self.ui.editing()
    }
    pub fn text_edit(&mut self) -> String {
        serde_json::to_string(&self.ui.text_edit()).unwrap_or_else(|_| "null".into())
    }
    pub fn fallback_fonts(&mut self) -> String {
        self.ui.prepare_scene();
        serde_json::to_string(&self.gpu.renderer.missing_fonts(&self.ui))
            .unwrap_or_else(|_| "[]".into())
    }
    pub fn load_font(&mut self, name: String, bytes: &[u8]) -> Result<(), JsValue> {
        self.gpu
            .renderer
            .add_font(name, bytes.to_vec())
            .map_err(|e| JsValue::from_str(&e))?;
        self.ui.invalidate();
        self.rendered = None;
        Ok(())
    }
    pub fn pointer_stops(&mut self, x: f32, y: f32) -> bool {
        self.ui.pointer_stops(x, y)
    }
}
