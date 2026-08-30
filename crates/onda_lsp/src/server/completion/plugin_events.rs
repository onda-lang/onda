#[derive(Debug, Clone, Copy)]
pub(super) struct PluginEventCompletion {
    pub(super) name: &'static str,
    pub(super) params: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PluginEventCompletionGroup {
    pub(super) label_prefix: &'static str,
    pub(super) description: &'static str,
    pub(super) events: &'static [PluginEventCompletion],
}

pub(super) const PLUGIN_MIDI_EVENT_COMPLETIONS: &[PluginEventCompletion] = &[
    PluginEventCompletion {
        name: "note_on",
        params: "id: i32, channel: i32, key: i32, velocity: f32",
    },
    PluginEventCompletion {
        name: "note_off",
        params: "id: i32, channel: i32, key: i32, velocity: f32",
    },
    PluginEventCompletion {
        name: "poly_pressure",
        params: "channel: i32, key: i32, pressure: f32",
    },
    PluginEventCompletion {
        name: "pitch_bend",
        params: "channel: i32, value: f32",
    },
    PluginEventCompletion {
        name: "channel_pressure",
        params: "channel: i32, pressure: f32",
    },
    PluginEventCompletion {
        name: "cc",
        params: "channel: i32, index: i32, value: f32",
    },
    PluginEventCompletion {
        name: "program_change",
        params: "channel: i32, program: i32",
    },
];

pub(super) const PLUGIN_HOST_CONTEXT_EVENT_COMPLETIONS: &[PluginEventCompletion] = &[
    PluginEventCompletion {
        name: "transport",
        params: "playing: bool, recording: bool, looping: bool",
    },
    PluginEventCompletion {
        name: "sample_position",
        params: "sample: i64",
    },
    PluginEventCompletion {
        name: "time_position",
        params: "seconds: f64",
    },
    PluginEventCompletion {
        name: "tempo",
        params: "bpm: f64",
    },
    PluginEventCompletion {
        name: "musical_position",
        params: "quarter_note: f64",
    },
    PluginEventCompletion {
        name: "bar_position",
        params: "start_quarter_note: f64",
    },
    PluginEventCompletion {
        name: "time_signature",
        params: "numerator: i32, denominator: i32",
    },
    PluginEventCompletion {
        name: "loop_region",
        params: "start_quarter_note: f64, end_quarter_note: f64",
    },
    PluginEventCompletion {
        name: "render_mode",
        params: "realtime: bool",
    },
];

pub(super) const PLUGIN_EVENT_COMPLETION_GROUPS: &[PluginEventCompletionGroup] = &[
    PluginEventCompletionGroup {
        label_prefix: "plugin_midi",
        description: "plugin MIDI",
        events: PLUGIN_MIDI_EVENT_COMPLETIONS,
    },
    PluginEventCompletionGroup {
        label_prefix: "plugin_host",
        description: "plugin host-context",
        events: PLUGIN_HOST_CONTEXT_EVENT_COMPLETIONS,
    },
];
