use crate::{
    editing::{Field, Setting},
    icons::Icon,
    interaction::{ToolbarMenu, KEYS},
    model::RunUi,
    presentation::Section,
    scene::{Palette, Scene, Target, TextStyle},
};

const WHITE_NOTES: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const BLACK_NOTES: [(u8, usize); 5] = [(1, 1), (3, 2), (6, 4), (8, 5), (10, 6)];
const TITLE_SIZE: f32 = 14.0;
const LABEL_SIZE: f32 = 12.0;
const LABEL_GAP: f32 = 4.0;

struct KeyboardLayout {
    base: u8,
    octaves: usize,
    width: f32,
    height: f32,
}
impl KeyboardLayout {
    fn new(ui: &RunUi) -> Self {
        let available = (ui.size[0] - 24.0).max(1.0);
        let base = ((ui.interaction.midi().octave + 1) * 12).clamp(0, 96);
        let octaves =
            ((available / (7.0 * 48.0)).floor() as usize).clamp(2, (128 - base) as usize / 12);
        let width = available.min((octaves * 7) as f32 * 72.0);
        Self {
            base: base as u8,
            octaves,
            width,
            height: (width / 7.0)
                .clamp(104.0, 156.0)
                .min((ui.size[1] * 0.25).max(104.0)),
        }
    }
}

pub(crate) fn height(ui: &RunUi) -> f32 {
    KeyboardLayout::new(ui).height + 62.0
}

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) {
    let [x, y, w, _] = rect;
    s.shape(rect, p.panel, 0.0);
    s.shape([x, y, w, 1.0], p.border, 0.0);
    let open = ui.presentation.open(Section::Midi);
    s.icon(
        Icon::Chevron(open),
        [x + 19.0, y + 23.0],
        if ui.focused(&Target::Section(Section::Midi)) {
            p.accent
        } else {
            p.ink_color()
        },
    );
    let mut title = "MIDI Keyboard";
    let full_width = settings_width(false);
    let compact_width = settings_width(true);
    let compact = w < 56.0 + crate::fonts::heading_width(title, TITLE_SIZE) + full_width;
    let mut tools_width = if compact { compact_width } else { full_width };
    if w < 56.0 + crate::fonts::heading_width(title, TITLE_SIZE) + tools_width {
        title = "MIDI";
    }
    let menu = w < 56.0 + crate::fonts::heading_width(title, TITLE_SIZE) + tools_width;
    if menu {
        tools_width = 28.0;
    }
    let title_width = (w - 44.0 - if open { tools_width + 12.0 } else { 0.0 }).max(1.0);
    let heading = [x + 32.0, y + 9.0, title_width, 28.0];
    s.styled(
        title,
        heading,
        TITLE_SIZE,
        p.ink,
        TextStyle {
            bold: true,
            ..Default::default()
        },
    );
    s.text_tooltip(heading, "MIDI Keyboard".into());
    s.hit(
        [x + 10.0, y + 6.0, title_width + 22.0, 32.0],
        Target::Section(Section::Midi),
    );
    if !open {
        return;
    }
    let mut bx = x + w - 12.0 - tools_width;
    if menu {
        crate::layout::toolbar::button(
            ui,
            s,
            p,
            [bx, y + 9.0, 28.0, 28.0],
            ToolbarMenu::Midi,
            "MIDI settings",
        );
    } else {
        for (label, setting, field_width) in settings(compact) {
            let label_width = label_width(label);
            setting_control(
                ui,
                s,
                p,
                label,
                setting,
                [bx, y + 10.0, label_width + LABEL_GAP + field_width, 26.0],
                label_width,
            );
            bx += label_width + LABEL_GAP + field_width + if compact { 8.0 } else { 14.0 };
        }
    }
    let layout = KeyboardLayout::new(ui);
    let keys = [
        x + (w - layout.width) * 0.5,
        y + 52.0,
        layout.width,
        layout.height,
    ];
    s.shape(keys, crate::theme::blend(p.scope, p.panel, 0.8), 6.0);
    let base = layout.base;
    let width = keys[2] / (layout.octaves * 7) as f32;
    let computer_keys = if ui.state["showComputerKeyLabels"] == false {
        &[][..]
    } else {
        &KEYS[..]
    };
    let whites = || {
        (0..layout.octaves * 7).map(|i| {
            let offset = (i / 7 * 12) as u8 + WHITE_NOTES[i % 7];
            (
                offset,
                [
                    keys[0] + i as f32 * width + 1.0,
                    keys[1] + 1.0,
                    (width - 2.0).max(0.0),
                    keys[3] - 2.0,
                ],
            )
        })
    };
    let blacks = || {
        (0..layout.octaves).flat_map(|octave| {
            BLACK_NOTES.into_iter().map(move |(offset, boundary)| {
                (
                    (octave * 12) as u8 + offset,
                    [
                        keys[0] + (octave * 7 + boundary) as f32 * width - width * 0.325,
                        keys[1],
                        width * 0.65,
                        keys[3] * 0.62,
                    ],
                )
            })
        })
    };
    let hovered = if ui.midi_interactive() {
        blacks()
            .chain(whites())
            .find(|(_, r)| s.hovered(ui, *r))
            .map(|(offset, _)| base + offset)
    } else {
        None
    };
    for (offset, r) in whites() {
        let note = base + offset;
        let active =
            ui.interaction.midi().held(note) || ui.interaction.midi().monitored[note as usize];
        s.panel(
            r,
            if active {
                p.piano_active
            } else if hovered == Some(note) {
                crate::theme::blend(p.piano_active, p.piano_white, 0.35)
            } else {
                p.piano_white
            },
            p.border,
            4.0,
        );
        if ui.midi_interactive() {
            s.hit(r, Target::Note(note));
        }
        if let Some((key, _)) = computer_keys.iter().find(|(_, o)| *o == offset) {
            s.styled(
                key.to_ascii_uppercase().to_string(),
                [r[0], r[1] + r[3] - 20.0, r[2], 18.0],
                11.0,
                crate::theme::text(0x143a63),
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
        }
    }
    for (offset, r) in blacks() {
        let note = base + offset;
        let active =
            ui.interaction.midi().held(note) || ui.interaction.midi().monitored[note as usize];
        s.panel(
            r,
            if active {
                p.piano_black_active
            } else if hovered == Some(note) {
                crate::theme::blend(p.piano_black_active, p.piano_black, 0.25)
            } else {
                p.piano_black
            },
            p.border,
            4.0,
        );
        if ui.midi_interactive() {
            s.hit(r, Target::Note(note));
        }
        if let Some((key, _)) = computer_keys.iter().find(|(_, o)| *o == offset) {
            s.styled(
                key.to_ascii_uppercase().to_string(),
                [r[0], r[1] + r[3] - 18.0, r[2], 16.0],
                11.0,
                crate::theme::text(0xd8e5f3),
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
        }
    }
}

fn settings(compact: bool) -> [(&'static str, Setting, f32); 2] {
    if compact {
        [
            ("Oct", Setting::Octave, 36.0),
            ("Vel", Setting::Velocity, 46.0),
        ]
    } else {
        [
            ("Octave", Setting::Octave, 50.0),
            ("Velocity", Setting::Velocity, 70.0),
        ]
    }
}

fn label_width(label: &str) -> f32 {
    crate::fonts::proportional_width(label, LABEL_SIZE, false).ceil()
}

fn settings_width(compact: bool) -> f32 {
    settings(compact)
        .iter()
        .map(|(label, _, field)| label_width(label) + LABEL_GAP + field)
        .sum::<f32>()
        + if compact { 8.0 } else { 14.0 }
}

fn setting_control(
    ui: &RunUi,
    s: &mut Scene,
    p: Palette,
    label: &str,
    setting: Setting,
    rect: [f32; 4],
    label_width: f32,
) {
    let [x, y, w, h] = rect;
    let label_top = y + crate::fonts::proportional_baseline_offset(TITLE_SIZE)
        - crate::fonts::proportional_baseline_offset(LABEL_SIZE);
    s.styled(
        label,
        [x, label_top, label_width, h],
        LABEL_SIZE,
        p.muted,
        TextStyle::default(),
    );
    let field = Field::Setting(setting);
    let bounds = [
        x + label_width + LABEL_GAP,
        y,
        (w - label_width - LABEL_GAP).max(1.0),
        h,
    ];
    s.tooltip(
        bounds,
        if label == "Oct" {
            "Octave"
        } else if label == "Vel" {
            "Velocity"
        } else {
            label
        }
        .into(),
    );
    s.field(
        ui,
        field.clone(),
        bounds,
        ui.field_value(&field).to_string(),
        p,
    );
}

pub(crate) fn menu(ui: &RunUi, s: &mut Scene, p: Palette) {
    crate::layout::toolbar::menu(ui, s, p, ToolbarMenu::Midi, 80.0, |s, r| {
        let width = settings(false)
            .iter()
            .map(|(label, _, _)| label_width(label))
            .fold(0.0, f32::max);
        for (i, (label, setting, _)) in settings(false).into_iter().enumerate() {
            setting_control(
                ui,
                s,
                p,
                label,
                setting,
                [r[0] + 8.0, r[1] + 8.0 + i as f32 * 36.0, r[2] - 16.0, 28.0],
                width,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Input;
    use serde_json::json;

    fn fixture() -> RunUi {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"keyboard.onda","supportsScope":false,"midiKeyboardInteractive":true,"midi":{"noteOn":true}
        }}));
        ui
    }

    #[test]
    fn keyboard_requires_note_on_support_even_when_the_host_enables_interaction() {
        let mut ui = fixture();
        for note_on in [false, true, false] {
            ui.message(json!({"type":"state","state":{
                "midi":{"available":true,"noteOn":note_on,"noteOff":true}
            }}));
            ui.prepare_scene();
            assert_eq!(crate::layout::dock_height(&ui) > 0.0, note_on);
            assert_eq!(ui.midi_interactive(), note_on);
        }
    }

    #[test]
    fn keyboard_reflows_and_keeps_every_note_hit_inside_the_dock() {
        let mut ui = fixture();
        for (width, height, octave, octaves) in [
            (320.0, 840.0, 4, 2),
            (720.0, 840.0, 4, 2),
            (960.0, 840.0, 4, 2),
            (1040.0, 840.0, 4, 3),
            (1440.0, 840.0, 4, 4),
            (1760.0, 840.0, 4, 5),
            (2560.0, 840.0, 4, 5),
            (1760.0, 400.0, 4, 5),
            (4000.0, 840.0, 7, 2),
            (4000.0, 840.0, -1, 10),
        ] {
            ui.resize(width, height, 1.0);
            ui.interaction.set_octave(octave);
            ui.invalidate();
            ui.prepare_scene();
            let layout = KeyboardLayout::new(&ui);
            assert_eq!(layout.octaves, octaves);
            assert!((104.0..=156.0).contains(&layout.height));
            let notes: Vec<_> = ui
                .scene
                .hits
                .iter()
                .filter_map(|hit| {
                    if let Target::Note(note) = hit.target {
                        Some((note, hit.rect.0))
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(notes.len(), octaves * 12);
            for (note, rect) in notes {
                assert!(note < 128);
                assert!(rect[0] >= 12.0 && rect[0] + rect[2] <= width - 11.99);
                assert!(rect[1] >= ui.scene.viewport_height);
                assert!(rect[1] + rect[3] <= height - 9.99);
                assert!(rect[2] <= 72.0);
                assert_eq!(
                    ui.target(rect[0] + rect[2] * 0.5, rect[1] + rect[3] - 2.0),
                    Some(Target::Note(note))
                );
            }
            let left = (width - layout.width) * 0.5;
            assert!(left >= 12.0);
            assert_eq!(ui.target(left - 1.0, height - 20.0), None);
        }
    }

    #[test]
    fn piano_hover_follows_the_topmost_key_and_preserves_held_feedback() {
        let mut ui = fixture();
        ui.prepare_scene();
        let rect = |ui: &RunUi, note| {
            ui.scene
                .hits
                .iter()
                .find(|hit| hit.target == Target::Note(note))
                .unwrap()
                .rect
                .0
        };
        let fill = |ui: &RunUi, note| {
            let r = rect(ui, note);
            let index = ui
                .scene
                .shapes
                .iter()
                .position(|shape| {
                    shape
                        .rect
                        .into_iter()
                        .zip(r)
                        .all(|(a, b)| (a - b).abs() < 0.01)
                })
                .unwrap();
            ui.scene.shapes[index + 1].color
        };
        let p = Palette::new(false);
        let black = rect(&ui, 61);
        ui.input(Input::PointerMove {
            x: black[0] + black[2] * 0.5,
            y: black[1] + 10.0,
            fine: false,
        });
        ui.prepare_scene();
        assert_ne!(fill(&ui, 61), p.piano_black);
        assert_eq!(fill(&ui, 60), p.piano_white);
        let white = rect(&ui, 60);
        let x = white[0] + white[2] * 0.5;
        let y = white[1] + white[3] - 5.0;
        ui.input(Input::PointerMove { x, y, fine: false });
        ui.prepare_scene();
        assert_ne!(fill(&ui, 60), p.piano_white);
        assert_eq!(fill(&ui, 61), p.piano_black);
        ui.input(Input::PointerDown { x, y, clicks: 1 });
        ui.prepare_scene();
        assert_eq!(fill(&ui, 60), p.piano_active);
        ui.input(Input::PointerUp);
        ui.toggle_settings();
        ui.invalidate();
        ui.prepare_scene();
        assert_eq!(fill(&ui, 60), p.piano_white, "settings owns hover feedback");
        ui.toggle_settings();
        ui.invalidate();
        ui.prepare_scene();
        assert_ne!(fill(&ui, 60), p.piano_white, "hover resumes after closing");
    }
}
