pub(crate) use crate::theme::Palette;
use crate::{
    editing::{Field, Setting},
    model::RunUi,
    presentation::Section,
    theme::CONTROL_RADIUS,
};
use bytemuck::{Pod, Zeroable};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect(pub [f32; 4]);
impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        let [rx, ry, w, h] = self.0;
        w > 0.0 && h > 0.0 && x >= rx && y >= ry && x <= rx + w && y <= ry + h
    }
    pub fn intersect(self, other: Self) -> Self {
        let [x, y, w, h] = self.0;
        let [a, b, c, d] = other.0;
        let l = x.max(a);
        let t = y.max(b);
        Self([
            l,
            t,
            (x + w).min(a + c).max(l) - l,
            (y + h).min(b + d).max(t) - t,
        ])
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Shape {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
    pub clip: [f32; 4],
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Target {
    Transport,
    Command(Value),
    Layout {
        input: bool,
        knobs: bool,
    },
    Control(usize),
    Field(Field),
    Section(Section),
    Devices,
    FastMath,
    Fold(String),
    Trigger(usize),
    Slice {
        event: usize,
        arg: usize,
        path: String,
        add: bool,
    },
    Follow,
    CopyLog,
    Note(u8),
    Choose(crate::interaction::SelectId),
    Option(usize),
    Scrollbar,
}
pub(crate) struct Hit {
    pub rect: Rect,
    pub target: Target,
}
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct TextStyle {
    pub mono: bool,
    pub bold: bool,
    pub centered: bool,
    pub wrap: bool,
}
#[derive(Clone)]
pub(crate) struct Label {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub width: f32,
    pub height: Option<f32>,
    pub color: [u8; 3],
    pub clip_top: f32,
    pub clip_left: f32,
    pub clip_right: f32,
    pub clip_bottom: f32,
    pub style: TextStyle,
    pub caret: Option<usize>,
    pub selection: Option<[usize; 2]>,
    pub preedit: Option<[usize; 2]>,
}
#[derive(Default)]
pub(crate) struct Scene {
    pub shapes: Vec<Shape>,
    pub labels: Vec<Label>,
    pub hits: Vec<Hit>,
    pub focus_targets: Vec<crate::interaction::FocusEntry>,
    pub focus_layer: crate::interaction::FocusLayer,
    pub scope_shape_end: usize,
    pub overlay_shapes: usize,
    pub overlay_labels: usize,
    pub overlay_hits: usize,
    pub menu_shapes: usize,
    pub menu_labels: usize,
    pub menu_hits: usize,
    pub menu_rect: [f32; 4],
    pub selects: Vec<(crate::interaction::SelectId, Rect)>,
    pub content_height: f32,
    pub viewport_height: f32,
    pub scrollbar: Option<crate::scrollbar::Scrollbar>,
    pub clip: [f32; 4],
    pub scope_rect: [f32; 4],
    pub scope_clip: [f32; 4],
    pub log_rect: [f32; 4],
    pub log_width: f32,
    pub log_max_scroll: f32,
    pub popup_rect: [f32; 4],
    pub popup_height: f32,
    pub source_picker: Option<Rect>,
    pub buffers: Vec<(Rect, String)>,
    pub tooltips: Vec<crate::interaction::Tooltip>,
    pub tooltip_label: Option<usize>,
}
impl Scene {
    pub fn shape(&mut self, rect: [f32; 4], color: [f32; 4], radius: f32) {
        self.shapes.push(Shape {
            rect,
            color,
            params: [radius, 0.0, 0.0, 0.0],
            clip: self.clip,
        });
    }
    pub fn primitive(&mut self, rect: [f32; 4], color: [f32; 4], params: [f32; 4]) {
        self.shapes.push(Shape {
            rect,
            color,
            params,
            clip: self.clip,
        });
    }
    pub fn panel(&mut self, rect: [f32; 4], fill: [f32; 4], border: [f32; 4], radius: f32) {
        self.shape(rect, border, radius);
        self.shape(
            [
                rect[0] + 1.0,
                rect[1] + 1.0,
                (rect[2] - 2.0).max(0.0),
                (rect[3] - 2.0).max(0.0),
            ],
            fill,
            (radius - 1.0).max(0.0),
        );
    }
    pub fn label(
        &mut self,
        text: impl Into<String>,
        position: [f32; 2],
        size: f32,
        width: f32,
        color: [u8; 3],
    ) {
        self.labels.push(Label {
            text: text.into(),
            x: position[0],
            y: position[1],
            size,
            width: width.max(1.0),
            height: Some(size * 1.4),
            color,
            clip_left: self.clip[0],
            clip_top: self.clip[1],
            clip_right: self.clip[0] + self.clip[2],
            clip_bottom: self.clip[1] + self.clip[3],
            style: TextStyle::default(),
            caret: None,
            selection: None,
            preedit: None,
        });
    }
    pub fn styled(
        &mut self,
        text: impl Into<String>,
        rect: [f32; 4],
        size: f32,
        color: [u8; 3],
        style: TextStyle,
    ) {
        self.label(
            text,
            [rect[0], rect[1] + (rect[3] - size * 1.4) * 0.5],
            size,
            rect[2],
            color,
        );
        self.labels.last_mut().unwrap().style = style;
    }
    pub fn paragraph(&mut self, text: &str, rect: [f32; 4], color: [u8; 3]) {
        self.label(text, [rect[0], rect[1]], 12.0, rect[2], color);
        let label = self.labels.last_mut().unwrap();
        label.style.mono = true;
        label.style.wrap = true;
        label.height = None;
        label.clip_top = label.clip_top.max(rect[1]);
        label.clip_bottom = label.clip_bottom.min(rect[1] + rect[3]);
    }
    pub fn hit(&mut self, rect: [f32; 4], target: Target) {
        self.focus_target(rect, &target);
        self.hits.push(Hit {
            rect: Rect(rect).intersect(Rect(self.clip)),
            target,
        });
    }
    pub fn hovered(&self, ui: &RunUi, rect: [f32; 4]) -> bool {
        let layer = if ui.interaction.menu().is_some() {
            crate::interaction::FocusLayer::Menu
        } else if ui.interaction.settings_open() {
            crate::interaction::FocusLayer::Popup
        } else {
            self.focus_layer
        };
        !ui.pointer_captured()
            && self.focus_layer == layer
            && Rect(rect)
                .intersect(Rect(self.clip))
                .contains(ui.interaction.pointer()[0], ui.interaction.pointer()[1])
    }
    pub fn button(&mut self, ui: &RunUi, text: &str, rect: [f32; 4], target: Target, p: Palette) {
        if !self.visible(rect[1], rect[3]) {
            self.hit(rect, target);
            return;
        }
        let hover = self.hovered(ui, rect) || ui.focused(&target);
        self.panel(
            rect,
            if hover { p.accent_soft } else { p.panel },
            if hover { p.accent } else { p.border },
            CONTROL_RADIUS,
        );
        self.styled(
            text,
            [rect[0] + 5.0, rect[1], rect[2] - 10.0, rect[3]],
            12.0,
            p.ink,
            TextStyle {
                centered: true,
                ..Default::default()
            },
        );
        self.hit(rect, target);
    }
    pub fn checkbox(
        &mut self,
        ui: &RunUi,
        field: Field,
        rect: [f32; 4],
        checked: bool,
        p: Palette,
    ) {
        self.toggle(
            ui,
            Target::Field(field.clone()),
            rect,
            checked,
            ui.field_enabled(&field),
            p,
        );
    }
    pub fn toggle(
        &mut self,
        ui: &RunUi,
        target: Target,
        rect: [f32; 4],
        checked: bool,
        enabled: bool,
        p: Palette,
    ) {
        self.panel(
            rect,
            if checked { p.accent } else { p.field },
            if enabled && (self.hovered(ui, rect) || ui.focused(&target)) {
                p.accent
            } else {
                p.border
            },
            4.0,
        );
        if checked {
            let color = crate::theme::rgb(if ui.light { 0xedf4fb } else { 0x08111d });
            self.line(
                [rect[0] + 5.0, rect[1] + 12.0],
                [rect[0] + 10.0, rect[1] + 17.0],
                2.0,
                color,
            );
            self.line(
                [rect[0] + 10.0, rect[1] + 17.0],
                [rect[0] + 19.0, rect[1] + 7.0],
                2.0,
                color,
            );
        }
        if enabled {
            self.hit(rect, target);
        }
    }
    pub fn field(&mut self, ui: &RunUi, field: Field, rect: [f32; 4], value: String, p: Palette) {
        if !self.visible(rect[1], rect[3]) {
            if ui.field_enabled(&field) {
                self.focus_target(rect, &Target::Field(field));
            }
            return;
        }
        if let Some(checked) = ui.field_value(&field).as_bool() {
            self.checkbox(
                ui,
                field,
                [
                    rect[0] + (rect[2] - 24.0) * 0.5,
                    rect[1] + (rect[3] - 24.0) * 0.5,
                    24.0,
                    24.0,
                ],
                checked,
                p,
            );
            return;
        }
        let enabled = ui.field_enabled(&field);
        let draft = ui.editor.draft().filter(|d| d.target == field);
        let dragging = ui.editor.dragging_field(&field);
        self.panel(
            rect,
            p.field,
            if draft.is_some_and(|d| d.invalid) {
                p.danger
            } else if enabled && (draft.is_some() || dragging || self.hovered(ui, rect)) {
                p.accent
            } else {
                p.border
            },
            CONTROL_RADIUS,
        );
        let display = draft.map(|d| d.display());
        let text = display.as_ref().map(|d| d.0.clone()).unwrap_or(value);
        let compact = matches!(
            &field,
            Field::Setting(Setting::ParamSmoothing | Setting::InputSmoothing)
        );
        self.styled(
            text,
            [rect[0] + 7.0, rect[1], (rect[2] - 14.0).max(1.0), rect[3]],
            if compact { 12.0 } else { 13.0 },
            if enabled { p.ink } else { p.muted },
            TextStyle {
                centered: true,
                mono: !compact,
                ..Default::default()
            },
        );
        let label = self.labels.last_mut().unwrap();
        label.caret = display.as_ref().map(|d| d.1);
        label.selection = display.as_ref().map(|d| d.2);
        label.preedit = display.and_then(|d| d.3);
        if ui.field_enabled(&field) {
            self.hit(rect, Target::Field(field));
        }
    }
    pub fn visible(&self, y: f32, height: f32) -> bool {
        y + height >= self.clip[1] && y < self.clip[1] + self.clip[3]
    }
    pub fn text_row(&mut self, text: impl Into<String>, rect: [f32; 4], p: Palette) {
        if self.visible(rect[1], rect[3]) {
            self.styled(text, rect, 12.0, p.muted, TextStyle::default());
        }
    }
}
pub(crate) fn build(ui: &RunUi) -> Scene {
    crate::layout::build(ui)
}
