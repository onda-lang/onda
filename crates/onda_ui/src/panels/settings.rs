use crate::{
    icons::Icon,
    interaction::SelectId,
    model::RunUi,
    scene::{Palette, Scene, Target},
};
use serde_json::json;

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, top, w, _] = rect;
    let mut y = top;
    if ui.state["supportsThemeSelection"] != false {
        let mode = if ui.light { "Light" } else { "Dark" };
        s.text_row(format!("Theme: {mode}"), [x, y + 6.0, w - 38.0, 20.0], p);
        let button = [x + w - 32.0, y, 32.0, 32.0];
        s.button(ui, "", button, Target::Theme, p);
        s.icon(Icon::Theme, [button[0] + 16.0, y + 16.0], p.ink_color());
        s.tooltip(button, "Toggle dark / light theme".into());
        y += 44.0;
    }
    for (label, id, key) in [
        ("Sample rate", SelectId::SampleRate, "sampleRateHz"),
        ("Block size", SelectId::BlockSize, "blockSamples"),
    ] {
        if ui.state.get(key).is_none()
            || (id == SelectId::SampleRate && ui.state.get("hostBlockSamples").is_some())
        {
            continue;
        }
        s.text_row(label, [x, y, w, 20.0], p);
        y += 24.0;
        s.select(ui, id, [x, y, w, 32.0], p);
        y += 44.0;
    }
    if ui.state["supportsFastMath"] == true {
        s.button(
            ui,
            if ui.state["fastMath"] == true {
                "Fast Math: On"
            } else {
                "Fast Math: Off"
            },
            [x, y, w, 32.0],
            Target::FastMath,
            p,
        );
        y += 44.0;
    }
    for (label, id, supported) in [
        (
            "Audio input",
            SelectId::AudioInput,
            ui.state["supportsDeviceSelection"] == true,
        ),
        (
            "Audio output",
            SelectId::AudioOutput,
            ui.state["supportsDeviceSelection"] == true,
        ),
        (
            "MIDI input",
            SelectId::MidiInput,
            ui.state["supportsMidiInputSelection"] == true,
        ),
    ] {
        if !supported {
            continue;
        }
        s.text_row(label, [x, y, w, 20.0], p);
        y += 24.0;
        s.select(ui, id, [x, y, w, 32.0], p);
        y += 44.0;
    }
    if ui.state["supportsDeviceSelection"] == true || ui.state["supportsMidiInputSelection"] == true
    {
        s.button(
            ui,
            "Refresh devices",
            [x, y, w, 32.0],
            Target::Command(json!({"type":"refreshDevices"})),
            p,
        );
        y += 44.0;
    }
    (y - top - 12.0).max(0.0)
}
