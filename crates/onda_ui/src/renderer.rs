use crate::{
    model::RunUi,
    scene::{Label, Palette, Shape, TextStyle},
};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::util::DeviceExt;

mod log;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 4],
    scope: [f32; 4],
    trace: [f32; 4],
    scope_color: [f32; 4],
    accent_strong: [f32; 4],
}
struct Text {
    text: String,
    width: f32,
    height: Option<f32>,
    size: f32,
    style: TextStyle,
    caret: Option<usize>,
    buffer: Buffer,
}
impl Text {
    fn new(font_system: &mut FontSystem) -> Self {
        Self {
            text: String::new(),
            width: 0.0,
            height: None,
            size: 0.0,
            style: TextStyle::default(),
            caret: None,
            buffer: Buffer::new(font_system, Metrics::new(13.0, 18.2)),
        }
    }
    fn update(&mut self, l: &Label, font_system: &mut FontSystem) {
        let t = self;
        if t.text != l.text
            || t.width != l.width
            || t.height != l.height
            || t.size != l.size
            || t.style != l.style
            || t.caret != l.caret
        {
            t.buffer.set_metrics(Metrics::new(l.size, l.size * 1.4));
            t.buffer.set_size(
                Some((l.width - if l.caret.is_some() { 1.0 } else { 0.0 }).max(1.0)),
                l.height,
            );
            t.buffer.set_wrap(if l.style.wrap {
                glyphon::Wrap::WordOrGlyph
            } else {
                glyphon::Wrap::None
            });
            t.buffer.set_text(
                &l.text,
                &Attrs::new()
                    .family(if l.style.mono {
                        Family::Monospace
                    } else {
                        Family::SansSerif
                    })
                    .weight(if l.style.bold {
                        glyphon::Weight::SEMIBOLD
                    } else {
                        glyphon::Weight::NORMAL
                    }),
                Shaping::Advanced,
                None,
            );
            if let Some(caret) = l.caret {
                t.buffer
                    .shape_until_cursor(font_system, glyphon::Cursor::new(0, caret), false);
            } else {
                t.buffer.set_scroll(Default::default());
                t.buffer.shape_until_scroll(font_system, false);
            }
            t.text = l.text.clone();
            t.width = l.width;
            t.height = l.height;
            t.size = l.size;
            t.style = l.style;
            t.caret = l.caret;
        }
    }
}

struct UploadBuffer {
    buffer: wgpu::Buffer,
    capacity: usize,
}
impl UploadBuffer {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("UI instances"),
                size: 256,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 256,
        }
    }
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.len() > self.capacity {
            self.capacity = data.len().next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("UI instances"),
                size: self.capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, 0, data);
        }
    }
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    shapes: wgpu::RenderPipeline,
    traces: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: UploadBuffer,
    samples: UploadBuffer,
    font_system: FontSystem,
    loaded_fonts: std::collections::HashSet<String>,
    font_check: Option<(usize, u64)>,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    overlay_renderer: TextRenderer,
    menu_renderer: TextRenderer,
    srgb: bool,
    text: Vec<Text>,
    prepared: Option<(usize, u64, [u32; 2], f32)>,
    scope_revision: Option<(usize, u64)>,
    diagnostic: Buffer,
    editor: Text,
    tooltip: Text,
    tooltip_max_width: f32,
    diagnostic_key: Option<(String, f32)>,
    log: log::Metrics,
    decorations: Vec<Shape>,
    decoration_layers: [usize; 4],
    decoration_buffer: UploadBuffer,
}

impl Renderer {
    /// Native hosts route pointer input through the same shaping used for drawing.
    /// Browser hosts use their overlaid text editor for caret and selection input.
    pub fn input(&mut self, ui: &mut RunUi, input: crate::Input) -> bool {
        let selection = match &input {
            crate::Input::PointerDown { x, y, clicks } if *clicks < 2 => {
                ui.prepare_scene();
                let same_field = ui.editor.draft().is_some_and(|d| {
                    ui.target(*x, *y) == Some(crate::scene::Target::Field(d.target.clone()))
                });
                if same_field {
                    self.text_hit(ui, *x, *y).map(|index| (index, false))
                } else {
                    None
                }
            }
            crate::Input::PointerMove { x, y, .. } if ui.editor.selecting_text() => {
                self.text_hit(ui, *x, *y).map(|index| (index, true))
            }
            _ => None,
        };
        let consumed = ui.input(input);
        if let Some((index, extend)) = selection {
            ui.input(crate::Input::SelectText { index, extend });
        }
        consumed
    }
    pub async fn new(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> Result<Self, String> {
        #[cfg(test)]
        eprintln!("[DEBUG-gpu-ci] adapter: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Onda UI"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UI globals"),
            contents: bytemuck::bytes_of(&<Globals as bytemuck::Zeroable>::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let binding_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UI globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI globals"),
            layout: &binding_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UI"),
            bind_group_layouts: &[Some(&binding_layout)],
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Onda UI"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui.wgsl").into()),
        });
        let pipeline = |entry, buffer| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    buffers: &[Some(buffer)],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let shape_attributes = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4];
        let shapes = pipeline(
            "shape_vertex",
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Shape>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &shape_attributes,
            },
        );
        let traces = pipeline(
            "trace_vertex",
            wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0=>Snorm16x4],
            },
        );
        let mut font_system =
            FontSystem::new_with_locale_and_db("en-US".into(), crate::fonts::database());
        let editor = Text::new(&mut font_system);
        let tooltip = Text::new(&mut font_system);
        let diagnostic = Buffer::new(&mut font_system, Metrics::new(12.0, 16.8));
        let log = log::Metrics::new(&mut font_system);
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::with_color_mode(
            &device,
            &queue,
            &cache,
            format,
            if format.is_srgb() {
                glyphon::ColorMode::Accurate
            } else {
                glyphon::ColorMode::Web
            },
        );
        let text_renderer = TextRenderer::new(&mut atlas, &device, Default::default(), None);
        let overlay_renderer = TextRenderer::new(&mut atlas, &device, Default::default(), None);
        let menu_renderer = TextRenderer::new(&mut atlas, &device, Default::default(), None);
        let instances = UploadBuffer::new(&device);
        let samples = UploadBuffer::new(&device);
        let decoration_buffer = UploadBuffer::new(&device);
        Ok(Self {
            device,
            queue,
            shapes,
            traces,
            globals,
            bind_group,
            instances,
            samples,
            font_system,
            loaded_fonts: Default::default(),
            font_check: None,
            swash: SwashCache::new(),
            viewport,
            atlas,
            text_renderer,
            overlay_renderer,
            menu_renderer,
            srgb: format.is_srgb(),
            text: Vec::new(),
            prepared: None,
            scope_revision: None,
            diagnostic,
            editor,
            tooltip,
            tooltip_max_width: 0.0,
            diagnostic_key: None,
            log,
            decorations: Vec::new(),
            decoration_layers: [0; 4],
            decoration_buffer,
        })
    }
    /// Encode into a caller-owned texture. Static geometry and text are prepared
    /// only when UI state, viewport, or scale changes; scope frames upload samples.
    pub fn render(
        &mut self,
        ui: &mut RunUi,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<wgpu::SubmissionIndex, String> {
        let scale = scale.max(0.25);
        ui.resize(width as f32 / scale, height as f32 / scale, scale);
        self.prepare_diagnostic(ui);
        ui.prepare_scene();
        #[cfg(not(target_arch = "wasm32"))]
        for name in self.missing_fonts(ui) {
            crate::fonts::load_fallback(&mut self.font_system, name);
            self.loaded_fonts.insert(name.into());
            self.fonts_changed();
            self.prepare_diagnostic(ui);
            ui.prepare_scene();
        }
        self.log.prepare(ui, &mut self.font_system);
        ui.prepare_scene();
        self.prepare_tooltip(ui);
        ui.prepare_scene();
        let key = (ui.id, ui.revision, [width, height], scale);
        if self.prepared != Some(key) {
            self.instances.upload(
                &self.device,
                &self.queue,
                bytemuck::cast_slice(&ui.scene.shapes),
            );
            self.prepare_text(
                &ui.scene.labels,
                ui.scene.overlay_labels,
                ui.scene.menu_labels,
                width,
                height,
                scale,
            )?;
            self.prepared = Some(key);
        }
        if self.scope_revision != Some((ui.id, ui.scope_revision)) {
            self.samples
                .upload(&self.device, &self.queue, bytemuck::cast_slice(&ui.trace));
            self.scope_revision = Some((ui.id, ui.scope_revision));
        }
        let scope = ui.scene.scope_rect;
        let palette = Palette::new(ui.light);
        let globals = Globals {
            viewport: [
                ui.size[0],
                ui.size[1],
                scale,
                if self.srgb { 1.0 } else { 0.0 },
            ],
            scope,
            trace: [
                ui.scope.columns() as f32,
                ui.scope.channels() as f32,
                0.0,
                0.0,
            ],
            scope_color: palette.accent,
            accent_strong: palette.accent_strong,
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let background = palette.clear(self.srgb);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Onda UI"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(background),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.draw_shapes(
                &mut pass,
                &ui.scene.shapes,
                &self.instances.buffer,
                0..ui.scene.scope_shape_end,
                [width, height],
                scale,
            );
            if ui.scope_enabled() && ui.state["running"] == true && !ui.trace.is_empty() {
                if let Some([x, y, w, h]) = scissor(ui.scene.scope_clip, width, height, scale) {
                    pass.set_scissor_rect(x, y, w, h);
                    pass.set_pipeline(&self.traces);
                    pass.set_vertex_buffer(0, self.samples.buffer.slice(..));
                    pass.draw(0..6, 0..ui.trace.len() as u32);
                }
            }
            self.draw_shapes(
                &mut pass,
                &ui.scene.shapes,
                &self.instances.buffer,
                ui.scene.scope_shape_end..ui.scene.overlay_shapes,
                [width, height],
                scale,
            );
            self.draw_shapes(
                &mut pass,
                &self.decorations,
                &self.decoration_buffer.buffer,
                self.decoration_layers[0]..self.decoration_layers[1],
                [width, height],
                scale,
            );
            pass.set_scissor_rect(0, 0, width, height);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .map_err(|e| e.to_string())?;
            self.draw_shapes(
                &mut pass,
                &ui.scene.shapes,
                &self.instances.buffer,
                ui.scene.overlay_shapes..ui.scene.menu_shapes,
                [width, height],
                scale,
            );
            self.draw_shapes(
                &mut pass,
                &self.decorations,
                &self.decoration_buffer.buffer,
                self.decoration_layers[1]..self.decoration_layers[2],
                [width, height],
                scale,
            );
            pass.set_scissor_rect(0, 0, width, height);
            self.overlay_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .map_err(|e| e.to_string())?;
            self.draw_shapes(
                &mut pass,
                &ui.scene.shapes,
                &self.instances.buffer,
                ui.scene.menu_shapes..ui.scene.shapes.len(),
                [width, height],
                scale,
            );
            self.draw_shapes(
                &mut pass,
                &self.decorations,
                &self.decoration_buffer.buffer,
                self.decoration_layers[2]..self.decoration_layers[3],
                [width, height],
                scale,
            );
            pass.set_scissor_rect(0, 0, width, height);
            self.menu_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .map_err(|e| e.to_string())?;
        }
        Ok(self.queue.submit([encoder.finish()]))
    }
    pub(crate) fn missing_fonts(&mut self, ui: &RunUi) -> Vec<&'static str> {
        let key = (ui.id, ui.revision);
        if self.font_check == Some(key) {
            return Vec::new();
        }
        self.font_check = Some(key);
        let mut needed = std::collections::BTreeSet::new();
        for c in ui
            .scene
            .labels
            .iter()
            .flat_map(|l| l.text.chars())
            .filter(|c| !crate::fonts::primary_covers(*c))
        {
            if crate::fonts::FALLBACKS
                .iter()
                .any(|f| self.loaded_fonts.contains(f.name) && f.covers(c))
            {
                continue;
            }
            if let Some(font) = crate::fonts::FALLBACKS.iter().find(|f| f.covers(c)) {
                needed.insert(font.name);
            }
        }
        needed.into_iter().collect()
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn add_font(&mut self, name: String, bytes: Vec<u8>) -> Result<(), String> {
        let ids = self
            .font_system
            .db_mut()
            .load_font_source(glyphon::fontdb::Source::Binary(std::sync::Arc::new(bytes)));
        if ids.is_empty() {
            return Err("Invalid fallback font".into());
        }
        self.loaded_fonts.insert(name);
        self.fonts_changed();
        Ok(())
    }
    fn fonts_changed(&mut self) {
        self.log.clear();
        self.font_system.shape_run_cache = Default::default();
        self.font_check = None;
        self.text.clear();
        self.editor = Text::new(&mut self.font_system);
        self.tooltip = Text::new(&mut self.font_system);
        self.tooltip_max_width = 0.0;
        self.diagnostic = Buffer::new(&mut self.font_system, Metrics::new(12.0, 16.8));
        self.prepared = None;
        self.diagnostic_key = None;
    }
    fn prepare_editor<'a>(&mut self, ui: &'a mut RunUi) -> Option<&'a Label> {
        ui.prepare_scene();
        #[cfg(not(target_arch = "wasm32"))]
        for name in self.missing_fonts(ui) {
            crate::fonts::load_fallback(&mut self.font_system, name);
            self.loaded_fonts.insert(name.into());
            self.fonts_changed();
        }
        let label = ui.scene.labels.iter().find(|l| l.caret.is_some())?;
        self.editor.update(label, &mut self.font_system);
        Some(label)
    }
    /// Native text systems use the same shaping, alignment and scrolling as drawing.
    pub fn text_bounds(&mut self, ui: &mut RunUi, start: usize, end: usize) -> Option<[f32; 4]> {
        let label = self.prepare_editor(ui)?;
        let a = crate::editing::byte_offset(&label.text, start);
        let b = crate::editing::byte_offset(&label.text, end);
        let (ax, ay) = self
            .editor
            .buffer
            .cursor_position(&glyphon::Cursor::new(0, a))?;
        let (bx, by) = self
            .editor
            .buffer
            .cursor_position(&glyphon::Cursor::new(0, b))?;
        Some([
            text_left(&self.editor, label) + ax.min(bx),
            label.y + ay.min(by),
            (ax - bx).abs().max(1.0),
            (ay - by).abs() + label.size * 1.4,
        ])
    }
    pub fn text_hit(&mut self, ui: &mut RunUi, x: f32, y: f32) -> Option<usize> {
        let label = self.prepare_editor(ui)?;
        let cursor = self
            .editor
            .buffer
            .hit(x - text_left(&self.editor, label), y - label.y)?;
        Some(label.text[..cursor.index].encode_utf16().count())
    }
    fn prepare_tooltip(&mut self, ui: &mut RunUi) {
        let Some(index) = ui.scene.tooltip_label else {
            return;
        };
        let label = &ui.scene.labels[index];
        let max_width = (ui.size[0] - 44.0).max(1.0);
        if self.tooltip.text != label.text || self.tooltip_max_width != max_width {
            let mut measured = label.clone();
            measured.width = max_width;
            measured.style.wrap = false;
            self.tooltip.update(&measured, &mut self.font_system);
            measured.width = self
                .tooltip
                .buffer
                .layout_runs()
                .map(|run| run.line_w)
                .fold(1.0, f32::max)
                .ceil()
                .min(max_width);
            measured.style.wrap = true;
            self.tooltip.update(&measured, &mut self.font_system);
            self.tooltip_max_width = max_width;
        }
        let height = self
            .tooltip
            .buffer
            .layout_runs()
            .map(|r| r.line_top + r.line_height)
            .fold(16.8, f32::max);
        let size = [self.tooltip.width, height];
        if ui.tooltip_size != size {
            ui.tooltip_size = size;
            ui.invalidate();
        }
    }
    fn prepare_diagnostic(&mut self, ui: &mut RunUi) {
        let error = ui.state["error"].as_str().unwrap_or("");
        let width = (ui.size[0] - if ui.loaded() { 48.0 } else { 52.0 }).max(1.0);
        if self
            .diagnostic_key
            .as_ref()
            .is_none_or(|(text, w)| text != error || *w != width)
        {
            self.diagnostic.set_size(Some(width), None);
            self.diagnostic.set_wrap(glyphon::Wrap::WordOrGlyph);
            self.diagnostic.set_text(
                error,
                &Attrs::new().family(Family::Monospace),
                Shaping::Advanced,
                None,
            );
            self.diagnostic
                .shape_until_scroll(&mut self.font_system, false);
            self.diagnostic_key = Some((error.to_owned(), width));
        }
        let height = self
            .diagnostic
            .layout_runs()
            .map(|r| r.line_top + r.line_height)
            .fold(16.8, f32::max);
        if ui.diagnostic_height != height {
            ui.diagnostic_height = height;
            ui.invalidate();
        }
    }
    fn draw_shapes(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        shapes: &[Shape],
        buffer: &wgpu::Buffer,
        range: std::ops::Range<usize>,
        size: [u32; 2],
        scale: f32,
    ) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.shapes);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        let mut start = range.start;
        while start < range.end {
            let clip = shapes[start].clip;
            let mut end = start + 1;
            while end < range.end && shapes[end].clip == clip {
                end += 1;
            }
            if let Some([x, y, w, h]) = scissor(clip, size[0], size[1], scale) {
                pass.set_scissor_rect(x, y, w, h);
                pass.draw(0..6, start as u32..end as u32);
            }
            start = end;
        }
    }
    fn prepare_text(
        &mut self,
        labels: &[Label],
        overlay: usize,
        menu: usize,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<(), String> {
        self.viewport
            .update(&self.queue, Resolution { width, height });
        self.text.truncate(labels.len());
        for (i, l) in labels.iter().enumerate() {
            if self.text.len() == i {
                self.text.push(Text::new(&mut self.font_system));
            }
            let t = &mut self.text[i];
            t.update(l, &mut self.font_system);
        }
        self.decorations.clear();
        for (layer, (renderer, range)) in [
            (&mut self.text_renderer, 0..overlay),
            (&mut self.overlay_renderer, overlay..menu),
            (&mut self.menu_renderer, menu..labels.len()),
        ]
        .into_iter()
        .enumerate()
        {
            renderer
                .prepare(
                    &self.device,
                    &self.queue,
                    &mut self.font_system,
                    &mut self.atlas,
                    &self.viewport,
                    self.text[range.clone()]
                        .iter()
                        .zip(&labels[range.clone()])
                        .map(|(t, l)| TextArea {
                            buffer: &t.buffer,
                            left: text_left(t, l) * scale,
                            top: l.y * scale,
                            scale,
                            bounds: TextBounds {
                                left: (l.clip_left.max(l.x) * scale).max(0.0) as i32,
                                top: (l.clip_top * scale).max(0.0) as i32,
                                right: ((l.x + l.width).min(l.clip_right) * scale)
                                    .clamp(0.0, width as f32)
                                    as i32,
                                bottom: (l.clip_bottom * scale).clamp(0.0, height as f32) as i32,
                            },
                            default_color: Color::rgb(l.color[0], l.color[1], l.color[2]),
                            custom_glyphs: &[],
                        }),
                    &mut self.swash,
                )
                .map_err(|e| e.to_string())?;
            for (t, l) in self.text[range.clone()].iter().zip(&labels[range]) {
                let Some(caret) = l.caret else { continue };
                let left = text_left(t, l);
                let clip = [
                    l.x.max(l.clip_left),
                    l.clip_top,
                    ((l.x + l.width).min(l.clip_right) - l.x.max(l.clip_left)).max(0.0),
                    (l.clip_bottom - l.clip_top).max(0.0),
                ];
                let color = [
                    f32::from(l.color[0]) / 255.0,
                    f32::from(l.color[1]) / 255.0,
                    f32::from(l.color[2]) / 255.0,
                    1.0,
                ];
                if let Some([a, b]) = l.selection {
                    for run in t.buffer.layout_runs() {
                        for (x, width) in run.highlight(
                            glyphon::Cursor::new(0, a.min(b)),
                            glyphon::Cursor::new(0, a.max(b)),
                        ) {
                            self.decorations.push(Shape {
                                rect: [left + x, l.y + run.line_top, width, run.line_height],
                                color: [color[0], color[1], color[2], 0.25],
                                params: [0.0; 4],
                                clip,
                            });
                        }
                    }
                }
                if let Some((x, y)) = t.buffer.cursor_position(&glyphon::Cursor::new(0, caret)) {
                    self.decorations.push(Shape {
                        rect: [left + x, l.y + y, 1.0 / scale, l.size * 1.4],
                        color,
                        params: [0.0; 4],
                        clip,
                    });
                }
                if let Some([a, b]) = l.preedit {
                    for run in t.buffer.layout_runs() {
                        for (x, width) in
                            run.highlight(glyphon::Cursor::new(0, a), glyphon::Cursor::new(0, b))
                        {
                            self.decorations.push(Shape {
                                rect: [
                                    left + x,
                                    l.y + run.line_top + run.line_height - 2.0,
                                    width,
                                    1.0 / scale,
                                ],
                                color,
                                params: [0.0; 4],
                                clip,
                            });
                        }
                    }
                }
            }
            self.decoration_layers[layer + 1] = self.decorations.len();
        }
        self.decoration_buffer.upload(
            &self.device,
            &self.queue,
            bytemuck::cast_slice(&self.decorations),
        );
        self.atlas.trim();
        // Reuse shaped runs between changing labels, retaining only recent batches.
        self.font_system.shape_run_cache.trim(2);
        Ok(())
    }
}

fn text_left(text: &Text, label: &Label) -> f32 {
    let width = text
        .buffer
        .layout_runs()
        .map(|r| r.line_w)
        .fold(0.0, f32::max);
    label.x
        + if label.style.centered {
            (label.width - width).max(0.0) * 0.5
        } else {
            0.0
        }
        - text.buffer.scroll().horizontal
}

fn scissor(rect: [f32; 4], width: u32, height: u32, scale: f32) -> Option<[u32; 4]> {
    let x = (rect[0] * scale).round().clamp(0.0, width as f32) as u32;
    let y = (rect[1] * scale).round().clamp(0.0, height as f32) as u32;
    let right = ((rect[0] + rect[2]) * scale)
        .round()
        .clamp(x as f32, width as f32) as u32;
    let bottom = ((rect[1] + rect[3]) * scale)
        .round()
        .clamp(y as f32, height as f32) as u32;
    (right > x && bottom > y).then_some([x, y, right - x, bottom - y])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{editing::Field, events::EventField, Input, Key};
    use serde_json::json;

    #[test]
    fn smoothing_labels_keep_the_complete_value_and_units_inside_the_field() {
        let mut fonts =
            FontSystem::new_with_locale_and_db("en-US".into(), crate::fonts::database());
        let mut text = Text::new(&mut fonts);
        for width in [360.0, 428.0, 768.0] {
            for milliseconds in [177.0, 1000.0, 9999.0] {
                let mut ui = RunUi::default();
                ui.resize(width, 840.0, 1.0);
                ui.message(json!({"type":"state","state":{
                    "path":"smoothing.onda","supportsScope":false,
                    "supportsParamSmoothing":true,"paramSmoothingMs":milliseconds,
                    "supportsInputControlSmoothing":true,"inputControlSmoothingMs":milliseconds,
                    "params":[{"name":"gain","type":"f64","value":0.5}],
                    "inputControls":[{"name":"input","type":"f64","value":0.5}]
                }}));
                ui.prepare_scene();
                let labels: Vec<_> = ui
                    .scene
                    .labels
                    .iter()
                    .filter(|l| l.text.starts_with("Smooth "))
                    .collect();
                assert_eq!(labels.len(), 2);
                for label in labels {
                    text.update(label, &mut fonts);
                    for run in text.buffer.layout_runs() {
                        assert!(
                            run.line_w <= label.width,
                            "{} needs {}px but the field allows {}px at viewport {width}",
                            label.text,
                            run.line_w,
                            label.width
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn short_filename_tooltips_stay_on_one_line() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
                .await
                .unwrap();
            for (width, path) in [
                (360.0, "main.onda"),
                (
                    768.0,
                    "/home/user/projects/music/synthesizers/voices/main.onda",
                ),
            ] {
                let mut ui = RunUi::default();
                ui.resize(width, 400.0, 1.0);
                ui.message(json!({"type":"state","state":{"path":path,"supportsScope":false}}));
                ui.prepare_scene();
                let filename = path.rsplit('/').next().unwrap();
                let label = ui.scene.labels.iter().find(|l| l.text == filename).unwrap();
                ui.input(Input::PointerMove {
                    x: label.x + 1.0,
                    y: label.y + 5.0,
                    fine: false,
                });
                ui.prepare_scene();
                ui.prepare_scene_at(ui.repaint_deadline().unwrap());
                renderer.prepare_tooltip(&mut ui);
                ui.prepare_scene();
                assert_eq!(
                    renderer.tooltip.buffer.layout_runs().count(),
                    1,
                    "tooltip should use the available width before wrapping {path}"
                );
            }
        });
    }

    #[test]
    fn tooltips_measure_wrapped_names_and_stay_inside_the_viewport() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
                .await
                .unwrap();
            let name = "filter_envelope_".repeat(8);
            for width in [360.0, 768.0] {
                for theme in ["light", "dark"] {
                    let mut ui = RunUi::default();
                    ui.resize(width, 400.0, 1.0);
                    ui.message(json!({"type":"state","state":{
                        "path":"names.onda","supportsScope":false,"themeMode":theme,
                        "params":[{"name":name,"type":"f64","value":0.5}]
                    }}));
                    ui.prepare_scene();
                    let label = ui.scene.labels.iter().find(|l| l.text == name).unwrap();
                    ui.input(Input::PointerMove {
                        x: label.x + 1.0,
                        y: label.y + 5.0,
                        fine: false,
                    });
                    ui.prepare_scene();
                    ui.prepare_scene_at(ui.repaint_deadline().unwrap());
                    renderer.prepare_tooltip(&mut ui);
                    ui.prepare_scene();
                    let label = &ui.scene.labels[ui.scene.tooltip_label.unwrap()];
                    assert_eq!(label.text, format!("{name} (f64)"));
                    let height = renderer
                        .tooltip
                        .buffer
                        .layout_runs()
                        .map(|r| r.line_top + r.line_height)
                        .fold(0.0, f32::max);
                    assert!(height > 16.8, "long names must wrap");
                    assert!(label.clip_bottom - label.y >= height);
                    let [x, y, w, h] = ui.scene.shapes[ui.scene.shapes.len() - 2].rect;
                    assert!(x >= 0.0 && x + w <= width && y >= 0.0 && y + h <= 400.0);
                    let revision = ui.revisions();
                    renderer.prepare_tooltip(&mut ui);
                    assert_eq!(ui.revisions(), revision, "unchanged tooltips retain layout");
                }
            }
        });
    }

    #[test]
    fn native_click_and_drag_select_the_draft_without_publishing_it() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
                return;
            };
            let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
                .await
                .unwrap();
            let mut ui = RunUi::default();
            ui.message(
                json!({"type":"state","state":{"path":"edit.onda","supportsScope":false,
                "params":[{"name":"gain","type":"f64","value":123.45}]}}),
            );
            ui.begin_field(Field::Control(0));
            ui.input(Input::Text {
                text: "1234567890".into(),
            });
            let point = renderer.text_bounds(&mut ui, 2, 2).unwrap();
            renderer.input(
                &mut ui,
                Input::PointerDown {
                    x: point[0] + 0.1,
                    y: point[1] + point[3] * 0.5,
                    clicks: 1,
                },
            );
            assert_eq!(ui.editor.draft().unwrap().selection(), 2..2);
            let point = renderer.text_bounds(&mut ui, 5, 5).unwrap();
            renderer.input(
                &mut ui,
                Input::PointerMove {
                    x: point[0] + 0.1,
                    y: point[1] + point[3] * 0.5,
                    fine: false,
                },
            );
            renderer.input(&mut ui, Input::PointerUp);
            assert_eq!(ui.editor.draft().unwrap().selection(), 2..5);
            assert_eq!(ui.editor.draft().unwrap().text, "1234567890");
            assert_eq!(ui.controls[0].value, 123.45);
            assert!(ui.take_commands().is_empty());
        });
    }

    #[test]
    fn script_fallbacks_render_real_glyphs_and_native_text_geometry_matches_shaping() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
                return;
            };
            let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
                .await
                .unwrap();
            let mut ui = RunUi::default();
            ui.message(json!({"type":"state","state":{"path":"世界.onda","supportsScope":false,
                "logText":"你好 世界\nمرحبا\nשלום\n😀","logRevealed":true,"error":"世界.onda: unknown name",
                "params":[{"name":"identifier","type":"i64","value":"9223372036854775807"}]}}));
            let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("font coverage"),
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
            renderer
                .render(
                    &mut ui,
                    &texture.create_view(&Default::default()),
                    768,
                    840,
                    1.0,
                )
                .unwrap();
            for text in renderer
                .text
                .iter()
                .filter(|t| t.text.chars().any(|c| c as u32 > 0x7f))
            {
                assert!(
                    text.buffer
                        .layout_runs()
                        .all(|r| r.glyphs.iter().all(|g| g.glyph_id != 0)),
                    "missing glyph in {}",
                    text.text
                );
            }
            assert_eq!(renderer.loaded_fonts.len(), 4);
            let revision = ui.revisions().0;
            assert!(renderer.missing_fonts(&ui).is_empty());
            assert_eq!(ui.revisions().0, revision);
            ui.begin_field(Field::Control(0));
            let caret = renderer.text_bounds(&mut ui, 19, 19).unwrap();
            let hit = renderer
                .text_hit(&mut ui, caret[0] + 0.1, caret[1] + caret[3] * 0.5)
                .unwrap();
            assert_eq!(
                hit, 19,
                "native hit testing must use the scrolled glyph positions"
            );
            let rect = ui.text_edit().unwrap()["rect"].as_array().unwrap().clone();
            let left = rect[0].as_f64().unwrap() as f32;
            assert!(caret[0] >= left && caret[0] < left + rect[2].as_f64().unwrap() as f32);
            ui.input(Input::Text {
                text: "😀9".into()
            });
            let caret = renderer.text_bounds(&mut ui, 2, 2).unwrap();
            assert_eq!(
                renderer.text_hit(&mut ui, caret[0] + 0.1, caret[1] + caret[3] * 0.5),
                Some(2)
            );
        });
    }

    #[test]
    fn diagnostics_shape_and_reserve_all_lines_at_narrow_widths() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
                return;
            };
            let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
                .await
                .unwrap();
            let error = "Cannot compile example.onda\n  --> example.onda:12:3\n12 | out1 = unknown_name\n   |        ^^^^^^^^^^^^\nunknown name: unknown_name";
            for loaded in [true, false] {
                let mut ui = RunUi::default();
                ui.resize(320.0, 840.0, 1.0);
                ui.message(json!({"type":"state","state":{"supportsSourceSelection":true,"path":if loaded {"example.onda"} else {""},"error":error}}));
                renderer.prepare_diagnostic(&mut ui);
                ui.prepare_scene();
                renderer
                    .prepare_text(
                        &ui.scene.labels,
                        ui.scene.overlay_labels,
                        ui.scene.menu_labels,
                        320,
                        840,
                        1.0,
                    )
                    .unwrap();
                let text = renderer.text.iter().find(|t| t.text == error).unwrap();
                assert!(
                    text.buffer.layout_runs().count() >= 5,
                    "every diagnostic line must be rendered"
                );
                let label = ui.scene.labels.iter().find(|l| l.text == error).unwrap();
                assert!(
                    label.clip_bottom - label.y >= 5.0 * 16.8 - 0.1,
                    "diagnostic layout must reserve all lines"
                );
            }
        });
    }

    #[test]
    fn long_integer_and_json_drafts_keep_the_caret_visible_at_both_ends() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
                return;
            };
            let mut renderer = Renderer::new(&adapter, wgpu::TextureFormat::Rgba8UnormSrgb)
                .await
                .unwrap();
            for json_editor in [false, true] {
                let mut ui = RunUi::default();
                let field = if json_editor {
                    ui.message(json!({"type":"state","state":{"path":"json.onda","supportsScope":false,
                        "events":[{"name":"levels","args":[{"name":"values","type":"f64[]","value":vec![0.5;32]}]}]}}));
                    Field::Event(EventField {
                        event: 0,
                        arg: 0,
                        path: String::new(),
                        scalar: "json".into(),
                    })
                } else {
                    ui.message(json!({"type":"state","state":{"path":"integer.onda","supportsScope":false,
                        "paramLayout":"knobs","params":[{"name":"exact","type":"i64","value":"9223372036854775807"}]}}));
                    Field::Control(0)
                };
                ui.begin_field(field);
                for key in [Key::End, Key::Home, Key::End] {
                    ui.input(Input::Key { key, shift: false });
                    ui.prepare_scene();
                    renderer
                        .prepare_text(
                            &ui.scene.labels,
                            ui.scene.overlay_labels,
                            ui.scene.menu_labels,
                            720,
                            840,
                            1.0,
                        )
                        .unwrap();
                    let text = renderer
                        .text
                        .iter()
                        .find(|text| text.caret.is_some())
                        .unwrap();
                    let caret = text.caret.unwrap();
                    let (x, _) = text
                        .buffer
                        .cursor_position(&glyphon::Cursor::new(0, caret))
                        .unwrap();
                    let scroll = text.buffer.scroll().horizontal;
                    assert!(
                        x - scroll >= -0.1 && x - scroll < text.width,
                        "caret at {x}, scroll {scroll}, width {}",
                        text.width
                    );
                    if caret == 0 {
                        assert_eq!(scroll, 0.0);
                    } else {
                        assert!(scroll > 0.0);
                    }
                }
            }
        });
    }
}
