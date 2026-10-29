//! Visible event widgets over a cached structural layout and shared focus stops.
mod layout;

use crate::{
    icons::Icon,
    interaction::FocusEntry,
    model::RunUi,
    scene::{Palette, Scene, Target, TextStyle},
};
pub(crate) use layout::Layout;
use layout::WidgetKind;

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let mut cached = ui.event_layout.borrow_mut();
    if cached
        .as_ref()
        .is_none_or(|layout| !layout.matches(ui, rect[2]))
    {
        *cached = Some(Layout::new(ui, rect[2]));
    }
    let layout = cached.as_ref().unwrap();
    let top = s.clip[1] - rect[1];
    let bottom = top + s.clip[3];
    for &[x, y, w, h] in layout.visible_cards(top, bottom) {
        s.panel([rect[0] + x, rect[1] + y, w, h], p.card, p.border, 9.0);
    }
    let focus_start = s.focus_targets.len();
    for widget in layout.visible_widgets(top, bottom) {
        let [x, y, w, h] = widget.rect;
        let r = [rect[0] + x, rect[1] + y, w, h];
        if !s.visible(r[1], r[3]) {
            continue;
        }
        match &widget.kind {
            WidgetKind::Title(name) => {
                s.tooltip(r, name.clone());
                s.styled(
                    name,
                    r,
                    14.0,
                    p.ink,
                    TextStyle {
                        mono: true,
                        bold: true,
                        ..Default::default()
                    },
                );
            }
            WidgetKind::Button { text, target } => s.button(ui, text, r, target.clone(), p),
            WidgetKind::Group { label, key, open } => {
                let target = Target::Fold(key.clone());
                if s.hovered(ui, r) || ui.focused(&target) {
                    s.shape(r, p.accent_soft, 5.0);
                }
                s.icon(
                    Icon::Chevron(*open),
                    [r[0] + 6.0, r[1] + 14.0],
                    p.ink_color(),
                );
                let heading = [r[0] + 18.0, r[1], r[2] - 18.0, r[3]];
                s.tooltip(heading, label.clone());
                s.styled(
                    label,
                    heading,
                    12.0,
                    p.ink,
                    TextStyle {
                        mono: true,
                        bold: true,
                        ..Default::default()
                    },
                );
                s.hit(r, target);
            }
            WidgetKind::Scalar {
                label,
                field,
                editor,
            } => {
                let editor = [
                    rect[0] + editor[0],
                    rect[1] + editor[1],
                    editor[2],
                    editor[3],
                ];
                let ty = match field {
                    crate::editing::Field::Event(f) => f.scalar.as_str(),
                    _ => "",
                };
                let available = (editor[0] - r[0] - 8.0).max(1.0);
                let type_width = ty.len() as f32 * 6.6;
                let name_width = (label.chars().count() as f32 * 7.2)
                    .min((available - type_width - 7.0).max(1.0));
                s.tooltip([r[0], r[1], available, 28.0], format!("{label} ({ty})"));
                s.styled(
                    label,
                    [r[0], r[1], name_width, 28.0],
                    12.0,
                    p.ink,
                    TextStyle {
                        mono: true,
                        ..Default::default()
                    },
                );
                if name_width + type_width + 7.0 <= available {
                    s.styled(
                        ty,
                        [r[0] + name_width + 7.0, r[1], type_width, 28.0],
                        11.0,
                        p.muted,
                        TextStyle {
                            mono: true,
                            ..Default::default()
                        },
                    );
                }
                let text = ui.field_text(field);
                s.field(ui, field.clone(), editor, text, p);
            }
            WidgetKind::Json(field) => s.field(ui, field.clone(), r, String::new(), p),
        }
    }
    // Visible hit testing remains local to this scene. Offscreen keyboard targets
    // share the immutable layout instead of allocating every path on every hover.
    s.focus_targets.truncate(focus_start);
    s.focus_targets.push(FocusEntry::Group {
        targets: layout.focus.clone(),
        offset: [rect[0], rect[1]],
    });
    layout.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{editing::Field, events::EventField, scene::Target, Input, Key};
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn large_event_layout_is_shared_across_hover_edits_and_offscreen_traversal() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{"path":"large.onda","supportsScope":false,
            "events":[{"name":"send","args":[{"name":"values","type":"f64[]","value":vec![0.5;4096]}]}]}}));
        ui.prepare_scene();
        let cached = ui.event_layout.borrow().as_ref().unwrap().focus.clone();
        assert!(ui.scene.labels.len() < 100);
        let field = |i| {
            Field::Event(EventField {
                event: 0,
                arg: 0,
                path: format!("/{i}"),
                scalar: "f64".into(),
            })
        };
        for x in [0.0, 300.0, 0.0] {
            ui.input(Input::PointerMove {
                x,
                y: 200.0,
                fine: false,
            });
            ui.prepare_scene();
            assert!(Arc::ptr_eq(
                &cached,
                &ui.event_layout.borrow().as_ref().unwrap().focus
            ));
        }
        ui.interaction.focus_field(field(4094));
        ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
        assert_eq!(
            ui.interaction.focus().cloned(),
            Some(Target::Field(field(4095)))
        );
        assert!(ui.scroll > 100_000.0);
        ui.input(Input::Text {
            text: "0.25".into(),
        });
        ui.input(Input::Key {
            key: Key::Enter,
            shift: false,
        });
        ui.prepare_scene();
        assert_eq!(ui.events[0].args[0].value[4095], 0.25);
        assert!(Arc::ptr_eq(
            &cached,
            &ui.event_layout.borrow().as_ref().unwrap().focus
        ));
        ui.message(json!({"type":"state","state":{"events":ui.state["events"]}}));
        ui.prepare_scene();
        assert!(Arc::ptr_eq(
            &cached,
            &ui.event_layout.borrow().as_ref().unwrap().focus
        ));
        ui.restore_view(&json!({"folded":["event/0/0/"]}));
        ui.invalidate();
        ui.prepare_scene();
        let folded = ui.event_layout.borrow().as_ref().unwrap().focus.clone();
        assert!(!Arc::ptr_eq(&cached, &folded));
        assert!(folded.len() < 10);
    }
}
