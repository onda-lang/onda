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

const fn completions<const N: usize>(
    events: &'static [onda_host_protocol::HostEvent; N],
) -> [PluginEventCompletion; N] {
    let mut out = [PluginEventCompletion {
        name: "",
        params: "",
    }; N];
    let mut index = 0;
    while index < N {
        out[index] = PluginEventCompletion {
            name: events[index].name,
            params: events[index].signature,
        };
        index += 1;
    }
    out
}

pub(super) const PLUGIN_MIDI_EVENT_COMPLETIONS: &[PluginEventCompletion] =
    &completions(&onda_host_protocol::PLUGIN_MIDI_EVENTS);

pub(super) const PLUGIN_HOST_CONTEXT_EVENT_COMPLETIONS: &[PluginEventCompletion] =
    &completions(&onda_host_protocol::PLUGIN_HOST_CONTEXT_EVENTS);

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
