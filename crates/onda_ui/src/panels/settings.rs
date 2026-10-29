use crate::{
    interaction::SelectId,
    model::RunUi,
    scene::{Palette, Scene, Target},
};
use serde_json::json;

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, top, w, _] = rect;
    let mut y = top;
    for (label, id, key) in [
        ("Sample rate", SelectId::SampleRate, "sampleRateHz"),
        ("Block size", SelectId::BlockSize, "blockSamples"),
    ] {
        if ui.state.get(key).is_none() {
            continue;
        }
        s.text_row(label, [x, y, w, 20.0], p);
        y += 24.0;
        s.select(ui, id, [x, y, w, 32.0], p);
        y += 44.0;
    }
    if ui.loaded()
        && ui.state["supportsRunSettings"] == true
        && ui.state["canSetSampleRate"] != true
    {
        s.text_row("Unload to change run settings.", [x, y, w, 24.0], p);
        y += 36.0;
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
