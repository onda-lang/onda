//! Shared native host projection of run state.
use crate::{RunController, RunHostOptions, RunState};
pub fn host_panel_state(
    controller: Option<&RunController>,
    load_error: Option<&str>,
    options: &RunHostOptions,
    theme_mode: &str,
    default_param_smoothing_seconds: f64,
) -> serde_json::Value {
    if let Some(controller) = controller {
        let mut state = loaded_panel_state(
            controller.state(),
            load_error,
            options,
            theme_mode,
            default_param_smoothing_seconds,
        );
        state["schemaReady"] = serde_json::json!(
            !controller.state().running
                || !matches!(
                    controller.source_compilation,
                    crate::SourceCompilationState::Compiling(_)
                )
        );
        return state;
    }
    serde_json::json!({
        "running": false,
        "connected": false,
        "schemaReady": true,
        "path": "",
        "status": "No file selected",
        "error": load_error,
        "outputChannels": 0,
        "buffers": [],
        "events": [],
        "midi": { "available": false, "noteOn": false, "noteOff": false },
        "logText": "",
        "logEntries": [],
        "logRevealed": false,
        "printOverflowCount": 0,
        "printTransportDropCount": 0,
        "delegateOverflowCount": 0,
        "delegateTransportDropCount": 0,
        "params": [],
        "inputDevices": [],
        "outputDevices": [],
        "midiInputDevices": [],
        "currentInputDevice": null,
        "currentOutputDevice": null,
        "currentMidiInputDevice": null,
        "supportsSourceSelection": true,
        "supportsTransport": true,
        "supportsDeviceSelection": true,
        "supportsMidiInputSelection": true,
        "supportsRunSettings": true,
        "supportsFastMath": options.show_fast_math,
        "fastMath": options.fast_math,
        "supportsParamSmoothing": true,
        "paramSmoothingMs": options.param_smoothing_seconds * 1000.0,
        "paramSmoothingDefaultMs": default_param_smoothing_seconds * 1000.0,
        "supportsScope": true,
        "supportsProjectExport": false,
        "canExportProject": false,
        "sampleRateHz": options.sample_rate_hz,
        "blockSamples": options.block_samples,
        "themeMode": theme_mode,
    })
}

fn loaded_panel_state(
    state: &RunState,
    load_error: Option<&str>,
    options: &RunHostOptions,
    theme_mode: &str,
    default_param_smoothing_seconds: f64,
) -> serde_json::Value {
    let mut panel = serde_json::json!({
        "running": state.running,
        "connected": state.connected,
        "path": state.path,
        "status": state.status,
        "error": load_error.or(state.error.as_deref()),
        "outputChannels": state.output_channels,
        "buffers": state.buffers,
        "events": state.events,
        "midi": state.midi,
        "params": state.params,
        "inputDevices": state.input_devices,
        "outputDevices": state.output_devices,
        "midiInputDevices": state.midi_input_devices,
        "currentInputDevice": state.current_input_device,
        "currentOutputDevice": state.current_output_device,
        "currentMidiInputDevice": state.current_midi_input_device,
        "supportsSourceSelection": true,
        "supportsTransport": true,
        "supportsDeviceSelection": true,
        "supportsMidiInputSelection": true,
        "supportsRunSettings": true,
        "supportsFastMath": options.show_fast_math,
        "fastMath": options.fast_math,
        "supportsParamSmoothing": true,
        "paramSmoothingMs": options.param_smoothing_seconds * 1000.0,
        "paramSmoothingDefaultMs": default_param_smoothing_seconds * 1000.0,
        "supportsScope": true,
        "supportsProjectExport": true,
        "canExportProject": true,
        "sampleRateHz": options.sample_rate_hz,
        "blockSamples": options.block_samples,
        "themeMode": theme_mode,
    });
    if let serde_json::Value::Object(log) = log_panel_state(state) {
        panel.as_object_mut().unwrap().extend(log);
    }
    panel
}

/// Log-only publications retain control schemas, drafts, and cached layout.
pub fn log_panel_state(state: &RunState) -> serde_json::Value {
    serde_json::json!({
        "logText": state.log_text,
        "logEntries": state.log_entries,
        "logRevealed": state.log_revealed,
        "printOverflowCount": state.print_overflow_count,
        "printTransportDropCount": state.print_transport_drop_count,
        "delegateOverflowCount": state.delegate_overflow_count,
        "delegateTransportDropCount": state.delegate_transport_drop_count,
    })
}

/// Runtime acknowledgments do not replace control banks or log history.
pub fn runtime_panel_state(state: &RunState) -> serde_json::Value {
    serde_json::json!({
        "running":state.running,
        "connected":state.connected,
        "status":state.status,
        "error":state.error,
        "outputChannels":state.output_channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_panel_advertises_midi_input_selection() {
        let options = RunHostOptions::default();
        let empty = host_panel_state(None, None, &options, "dark", 0.03);
        assert_eq!(empty["supportsMidiInputSelection"], true);
        let state = RunState::new(std::path::Path::new("parity.onda"), &options);
        let loaded = loaded_panel_state(&state, None, &options, "dark", 0.03);
        assert_eq!(loaded["supportsMidiInputSelection"], true);
    }
}
