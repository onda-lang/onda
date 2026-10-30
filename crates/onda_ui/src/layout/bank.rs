//! Responsive bank actions share the toolbar menu's input and scrolling behavior.
use crate::{
    editing::{Field, Setting},
    icons::Icon,
    interaction::FocusLayer,
    model::RunUi,
    scene::{Palette, Rect, Scene, Target},
};
use serde_json::json;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Full,
    Compact,
    Menu,
}
pub(super) struct Header {
    pub title: &'static str,
    pub title_size: f32,
    pub reset_inline: bool,
    pub tools_width: f32,
    mode: Mode,
}
fn smoothing_supported(ui: &RunUi, input: bool) -> bool {
    ui.state[if input {
        "supportsInputControlSmoothing"
    } else {
        "supportsParamSmoothing"
    }] == true
}
impl Header {
    pub fn new(ui: &RunUi, input: bool, width: f32) -> Self {
        let smoothing = smoothing_supported(ui, input);
        let full_width = if smoothing { 172.0 } else { 54.0 };
        let compact_width = if smoothing { 132.0 } else { 54.0 };
        let mut title = if input { "Input Controls" } else { "Params" };
        let title_size = if width < 128.0 {
            11.0
        } else if width < 160.0 {
            12.0
        } else {
            14.0
        };
        let mode = if width >= crate::fonts::heading_width(title, title_size) + 77.0 + full_width {
            Mode::Full
        } else {
            if input {
                title = "Inputs";
            }
            if width >= crate::fonts::heading_width(title, title_size) + 77.0 + compact_width {
                Mode::Compact
            } else {
                Mode::Menu
            }
        };
        let tools_width = match mode {
            Mode::Full => full_width,
            Mode::Compact => compact_width,
            Mode::Menu => 28.0,
        };
        Self {
            title,
            title_size,
            reset_inline: width
                >= crate::fonts::heading_width(title, title_size) + 77.0 + tools_width,
            tools_width,
            mode,
        }
    }
    pub fn build(&self, ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4], input: bool) {
        let [x, y, w, _] = rect;
        let mut bx = x + w - 9.0 - self.tools_width;
        let by = y + 7.0;
        if self.mode == Mode::Menu {
            super::header::icon_button(
                ui,
                s,
                p,
                [bx, by - 1.0, 28.0, 28.0],
                Icon::More,
                "Smoothing, layout and reset",
                Target::BankMenu(input),
                ui.interaction.toolbar_menu == Some(crate::interaction::ToolbarMenu::Bank(input)),
            );
            return;
        }
        let compact = self.mode == Mode::Compact;
        if smoothing_supported(ui, input) {
            let width = if compact { 72.0 } else { 112.0 };
            smoothing(ui, s, p, [bx, by, width, 26.0], input, compact);
            bx += width + 6.0;
        }
        layout(ui, s, p, [bx, by, 54.0, 26.0], input);
    }
}
fn smoothing(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4], input: bool, compact: bool) {
    let field = Field::Setting(if input {
        Setting::InputSmoothing
    } else {
        Setting::ParamSmoothing
    });
    let value = ui.field_value(&field).as_f64().unwrap_or(0.0);
    s.field(
        ui,
        field,
        rect,
        format!("{}{value:.0} ms", if compact { "" } else { "Smooth " }),
        p,
    );
    s.tooltip(rect, format!("Smoothing: {value:.0} ms"));
}
fn layout(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4], input: bool) {
    let [x, y, w, h] = rect;
    let knobs = ui.bank_knobs(input);
    s.panel(rect, p.field, p.border, 5.0);
    for (i, (label, selected, icon)) in [
        ("Sliders", false, Icon::Sliders),
        ("Knobs", true, Icon::Knob),
    ]
    .into_iter()
    .enumerate()
    {
        let b = [
            x + 2.0 + i as f32 * (w - 2.0) * 0.5,
            y + 2.0,
            (w - 6.0) * 0.5,
            h - 4.0,
        ];
        let target = Target::Layout {
            input,
            knobs: selected,
        };
        let hover = s.hovered(ui, b) || ui.focused(&target);
        let fill = if knobs == selected {
            p.accent_soft
        } else {
            p.field
        };
        if hover {
            s.panel(b, fill, p.accent, 3.0);
        } else {
            s.shape(b, fill, 3.0);
        }
        s.icon(
            icon,
            [b[0] + b[2] * 0.5, b[1] + b[3] * 0.5],
            if knobs == selected || hover {
                p.ink_color()
            } else {
                p.muted_color()
            },
        );
        s.tooltip(b, label.into());
        s.hit(b, target);
    }
}
pub(super) fn menu(ui: &RunUi, s: &mut Scene, p: Palette, input: bool) {
    let Some(anchor) = s
        .hits
        .iter()
        .find(|h| h.target == Target::BankMenu(input))
        .map(|h| h.rect.0)
    else {
        return;
    };
    let supported = smoothing_supported(ui, input);
    let content_height = if supported { 116.0 } else { 80.0 };
    let w = (ui.size[0] - 24.0).clamp(1.0, 240.0);
    let below = ui.size[1] - anchor[1] - anchor[3] - 12.0;
    let above = anchor[1] - 12.0;
    let down = below >= content_height || below >= above;
    let h = content_height.min(if down { below } else { above }.max(1.0));
    let r = [
        (anchor[0] + anchor[2] - w).max(12.0),
        if down {
            anchor[1] + anchor[3] + 4.0
        } else {
            anchor[1] - h - 4.0
        },
        w,
        h,
    ];
    s.toolbar_menu_rect = r;
    s.toolbar_scroll_max = (content_height - h).max(0.0);
    s.focus_layer = FocusLayer::Toolbar;
    s.panel(r, p.panel, p.border, 7.0);
    s.hit(r, Target::Overlay);
    let clip = s.clip;
    s.clip = Rect(r).intersect(Rect(clip)).0;
    let mut y = r[1] + 8.0 - ui.interaction.toolbar_scroll.min(s.toolbar_scroll_max);
    let width = w - 16.0;
    if supported {
        smoothing(ui, s, p, [r[0] + 8.0, y, width, 28.0], input, width < 112.0);
        y += 36.0;
    }
    layout(ui, s, p, [r[0] + 8.0, y, width, 28.0], input);
    y += 36.0;
    s.button(
        ui,
        "Reset",
        [r[0] + 8.0, y, width, 28.0],
        Target::Command(json!({"type":if input {"resetInputControls"} else {"resetParams"}})),
        p,
    );
    s.clip = clip;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Input, Key};

    fn click(ui: &mut RunUi, target: Target) {
        ui.prepare_scene();
        let r = ui
            .scene
            .hits
            .iter()
            .find(|h| h.target == target)
            .unwrap()
            .rect
            .0;
        ui.input(Input::PointerDown {
            x: r[0] + r[2] * 0.5,
            y: r[1] + r[3] * 0.5,
            clicks: 1,
        });
        ui.input(Input::PointerUp);
        ui.prepare_scene();
    }

    #[test]
    fn compact_menus_edit_smoothing_switch_layout_and_reset_each_bank() {
        for input in [false, true] {
            let mut ui = RunUi::default();
            ui.resize(160.0, 840.0, 2.5);
            ui.message(json!({"type":"state","state":{
                "path":"banks.onda","supportsScope":false,"supportsViewState":true,
                "supportsParamSmoothing":true,"supportsInputControlSmoothing":true,
                "params":[{"name":"gain","type":"f64","value":0.5}],
                "inputControls":[{"name":"input","type":"f64","value":0.5}]
            }}));
            click(&mut ui, Target::BankMenu(input));
            let field = Field::Setting(if input {
                Setting::InputSmoothing
            } else {
                Setting::ParamSmoothing
            });
            ui.input(Input::Key {
                key: Key::Tab,
                shift: false,
            });
            assert!(ui.focused(&Target::Field(field.clone())));
            ui.input(Input::EditText {
                text: "75".into(),
                anchor: 2,
                cursor: 2,
            });
            ui.input(Input::Key {
                key: Key::Enter,
                shift: false,
            });
            assert_eq!(ui.field_value(&field).as_f64(), Some(75.0));
            assert!(ui.interaction.toolbar_menu.is_some());
            for knobs in [true, false] {
                click(&mut ui, Target::Layout { input, knobs });
                assert_eq!(ui.bank_knobs(input), knobs);
                assert!(ui.interaction.toolbar_menu.is_some());
            }
            let command = json!({"type": if input { "resetInputControls" } else { "resetParams" }});
            click(&mut ui, Target::Command(command.clone()));
            assert!(ui.take_commands().contains(&command));
            ui.input(Input::Key {
                key: Key::Escape,
                shift: false,
            });
            assert!(ui.interaction.toolbar_menu.is_none());
            click(&mut ui, Target::BankMenu(input));
            ui.input(Input::PointerDown {
                x: 1.0,
                y: 1.0,
                clicks: 1,
            });
            assert!(ui.interaction.toolbar_menu.is_none());
        }
    }
}
