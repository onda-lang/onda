use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use web_time::{Duration, Instant};

impl crate::model::RunUi {
    /// Hosts should drain commands again at this deadline, even while drawing is idle.
    pub fn view_state_deadline(&self) -> Option<Instant> {
        if self.state["supportsViewState"] == true
            && self.pending_view.is_none()
            && !self.editor.has_number_drag()
        {
            self.presentation.save_after
        } else {
            None
        }
    }
    pub(crate) fn publish_view_state(&mut self, flush: bool) {
        if !self.presentation.dirty
            || self.pending_view.is_some()
            || self.state["supportsViewState"] != true
            || (!flush
                && (self.editor.has_number_drag()
                    || self
                        .presentation
                        .save_after
                        .is_some_and(|t| t > Instant::now())))
        {
            return;
        }
        self.presentation.scroll = self.scroll;
        let mut view = self.presentation.snapshot();
        view["path"] = self.state["path"].clone();
        view["octave"] = json!(self.interaction.midi().octave);
        view["velocity"] = json!(self.interaction.midi().velocity);
        view["paramLayout"] = json!(if self.knobs { "knobs" } else { "sliders" });
        view["inputControlLayout"] = json!(if self.input_knobs { "knobs" } else { "sliders" });
        view["eventValues"] = json!(self
            .events
            .iter()
            .map(|e| json!({
                "name":e.name,
                "args":e.args.iter().map(|a| json!({
                    "name":a.name,"shape":a.shape,"default":a.default
                })).collect::<Vec<_>>(),
                "values":e.args.iter().map(|a| &a.value).collect::<Vec<_>>()
            }))
            .collect::<Vec<_>>());
        self.commands.push(json!({"type":"viewState","state":view}));
        self.presentation.dirty = false;
        self.presentation.save_after = None;
    }
    pub(crate) fn restore_view(&mut self, view: &Value) {
        self.event_layout.get_mut().take();
        self.clear_control_layouts();
        self.presentation.restore(view);
        for (key, layout) in [
            ("paramLayout", &mut self.knobs),
            ("inputControlLayout", &mut self.input_knobs),
        ] {
            if let Some(value @ ("knobs" | "sliders")) = view[key].as_str() {
                *layout = value == "knobs";
                self.state[key] = json!(value);
            }
        }
        self.scroll = self.presentation.scroll;
        self.interaction.restore_midi(view);
        if let Some(saved) = view["eventValues"].as_array() {
            for event in &mut self.events {
                if let Some(values) = saved
                    .iter()
                    .find(|e| e["name"] == event.name)
                    .filter(|e| {
                        crate::events::Event::from_metadata(e)
                            .is_some_and(|saved| event.same_schema(&saved))
                    })
                    .and_then(|e| e["values"].as_array())
                {
                    if values.len() == event.args.len() {
                        for (arg, value) in event.args.iter_mut().zip(values) {
                            if crate::events::valid(&arg.shape, value) {
                                arg.value = value.clone();
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{editing::Field, events::EventField, Input, Key, RunUi};

    fn fixture() -> RunUi {
        let mut ui = RunUi::default();
        ui.resize(720.0, 400.0, 1.0);
        ui.message(json!({"type":"state","state":{
            "path":"saved.onda","supportsScope":false,"supportsViewState":true,
            "params":(0..60).map(|i| json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>(),
            "events":[{"name":"send","args":[{"name":"value","type":"i64","value":"1"}]}]
        }}));
        ui
    }

    #[test]
    fn saved_event_values_require_the_same_argument_signature() {
        let args = json!([
            {"name":"pitch","type":"f64","default":69.0,"value":60.0},
            {"name":"velocity","type":"f64","default":0.5,"value":0.8}
        ]);
        let mut ui = fixture();
        ui.message(json!({"type":"state","state":{"events":[{"name":"note","args":args}]}}));
        ui.presentation.changed();
        let saved = ui.take_commands().pop().unwrap()["state"].clone();
        for change in ["none", "reorder", "rename", "type", "default"] {
            let mut args = args.clone();
            match change {
                "reorder" => args.as_array_mut().unwrap().reverse(),
                "rename" => args[0]["name"] = json!("frequency"),
                "type" => args[0]["type"] = json!("i32"),
                "default" => args[0]["default"] = json!(72.0),
                _ => {}
            }
            for arg in args.as_array_mut().unwrap() {
                arg.as_object_mut().unwrap().remove("value");
            }
            let mut restored = fixture();
            restored.message(json!({"type":"state","state":{
                "events":[{"name":"note","args":args}],"viewState":saved
            }}));
            for (i, arg) in restored.events[0].args.iter().enumerate() {
                let expected = if change == "none" {
                    &saved["eventValues"][0]["values"][i]
                } else {
                    &arg.default
                };
                assert_eq!(&arg.value, expected, "{change}");
            }
        }
    }

    #[test]
    fn new_host_restore_replaces_same_source_presentation_and_event_values_once() {
        let mut ui = fixture();
        let view = |id, open, value| {
            json!({"readyId":id,"sections":{"params":open},
            "eventValues":[{"name":"send","args":[{"name":"value",
                "shape":{"kind":"scalar","encoding":"i64"},"default":"0"}],"values":[value]}]})
        };
        ui.message(json!({"type":"state","state":{"viewState":view(1,false,"12")}}));
        ui.message(json!({"type":"state","state":{"viewState":view(2,true,"34")}}));
        assert!(ui.presentation.open(Section::Params));
        assert_eq!(ui.events[0].args[0].value, "34");
        ui.presentation.toggle(Section::Params);
        ui.events[0].args[0].value = json!("56");
        // Ordinary echoes must not erase local edits or forget the last restore ID.
        ui.message(json!({"type":"state","state":{"viewState":{"sections":{"params":true}}}}));
        ui.message(json!({"type":"state","state":{"viewState":view(2,true,"34")}}));
        assert!(!ui.presentation.open(Section::Params));
        assert_eq!(ui.events[0].args[0].value, "56");
        ui.message(
            json!({"type":"state","state":{"schemaReady":false,"viewState":view(3,true,"78")}}),
        );
        assert!(!ui.presentation.open(Section::Params));
        ui.message(json!({"type":"state","state":{"schemaReady":true}}));
        assert!(ui.presentation.open(Section::Params));
        assert_eq!(ui.events[0].args[0].value, "78");
    }

    #[test]
    fn scrolling_publishes_once_at_the_deadline_without_further_input() {
        let mut ui = fixture();
        ui.input(Input::Scroll { delta: 20.0 });
        assert!(ui.take_commands().is_empty());
        assert!(ui.view_state_deadline().is_some());
        ui.presentation.save_after = Some(Instant::now() - Duration::from_millis(1));
        let commands = ui.take_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["state"]["scrollTop"], 20.0);
        assert!(ui.take_commands().is_empty());
        assert!(ui.view_state_deadline().is_none());
    }

    #[test]
    fn committed_event_edits_flush_pending_scroll_without_delaying_values() {
        for input in [
            Input::Key {
                key: Key::Enter,
                shift: false,
            },
            Input::FocusLost,
        ] {
            let mut ui = fixture();
            ui.input(Input::Scroll { delta: 20.0 });
            ui.begin_field(Field::Event(EventField {
                event: 0,
                arg: 0,
                path: String::new(),
                scalar: "i64".into(),
            }));
            ui.input(Input::Text {
                text: "9223372036854775807".into(),
            });
            ui.input(input);
            let commands = ui.take_commands();
            assert_eq!(
                commands[0]["state"]["eventValues"][0]["values"][0],
                "9223372036854775807"
            );
            assert_eq!(commands[0]["state"]["scrollTop"], 20.0);
            assert!(ui.view_state_deadline().is_none());
        }
    }

    fn event_drag_fixture() -> (RunUi, [f32; 2]) {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"drag.onda","supportsScope":false,"supportsTransport":false,
            "supportsViewState":true,
            "events":[{"name":"send","args":[{"name":"values","type":"f64[]","value":vec![0.5;4096]}]}]
        }}));
        let field = Field::Event(EventField {
            event: 0,
            arg: 0,
            path: "/0".into(),
            scalar: "f64".into(),
        });
        ui.begin_field(field);
        let edit = ui.text_edit().unwrap();
        let position = [
            edit["rect"][0].as_f64().unwrap() as f32 + 10.0,
            edit["rect"][1].as_f64().unwrap() as f32 + 10.0,
        ];
        ui.input(Input::Key {
            key: Key::Escape,
            shift: false,
        });
        assert!(ui.take_commands().is_empty());
        (ui, position)
    }

    #[test]
    fn event_drags_save_once_when_released_or_cancelled() {
        for end in [
            Input::PointerUp,
            Input::FocusLost,
            Input::Key {
                key: Key::Escape,
                shift: false,
            },
        ] {
            let (mut ui, [x, y]) = event_drag_fixture();
            ui.input(Input::PointerDown { x, y, clicks: 1 });
            for delta in [10.0, 20.0, 30.0] {
                ui.input(Input::PointerMove {
                    x: x + delta,
                    y,
                    fine: false,
                });
                assert!(ui.events[0].args[0].value[0].as_f64().unwrap() > 0.5);
                assert!(
                    ui.take_commands().is_empty(),
                    "dragging must not save snapshots"
                );
                assert!(ui.view_state_deadline().is_none());
            }
            let value = ui.events[0].args[0].value[0].clone();
            ui.input(end);
            let commands = ui.take_commands();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0]["type"], "viewState");
            assert_eq!(
                commands[0]["state"]["eventValues"][0]["values"][0][0],
                value
            );
            assert!(ui.take_commands().is_empty());
        }
    }

    #[test]
    fn touch_event_drags_save_on_release_and_cancellation() {
        for end in [Input::TouchEnd { id: 1 }, Input::TouchCancel { id: 1 }] {
            let (mut ui, [x, y]) = event_drag_fixture();
            ui.input(Input::TouchStart { id: 1, x, y });
            ui.input(Input::TouchMove {
                id: 1,
                x: x + 30.0,
                y,
                delta_y: None,
            });
            assert!(ui.take_commands().is_empty());
            let value = ui.events[0].args[0].value[0].clone();
            assert!(value.as_f64().unwrap() > 0.5);
            ui.input(end);
            let commands = ui.take_commands();
            assert_eq!(commands.len(), 1);
            assert_eq!(
                commands[0]["state"]["eventValues"][0]["values"][0][0],
                value
            );
        }
    }

    #[test]
    fn switching_sources_flushes_the_previous_event_drag() {
        let (mut ui, [x, y]) = event_drag_fixture();
        ui.input(Input::PointerDown { x, y, clicks: 1 });
        ui.input(Input::PointerMove {
            x: x + 30.0,
            y,
            fine: false,
        });
        assert!(ui.take_commands().is_empty());
        let value = ui.events[0].args[0].value[0].clone();
        ui.message(json!({"type":"state","state":{"path":"next.onda","events":[]}}));
        let commands = ui.take_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["state"]["path"], "drag.onda");
        assert_eq!(
            commands[0]["state"]["eventValues"][0]["values"][0][0],
            value
        );
    }

    #[test]
    fn source_changes_flush_the_previous_source_and_resets_discard_pending_scroll() {
        let mut ui = fixture();
        ui.input(Input::Scroll { delta: 20.0 });
        ui.message(json!({"type":"state","state":{"path":"next.onda"}}));
        let commands = ui.take_commands();
        assert_eq!(commands[0]["state"]["path"], "saved.onda");
        assert_eq!(commands[0]["state"]["scrollTop"], 20.0);
        ui.input(Input::Scroll { delta: 10.0 });
        ui.message(json!({"type":"state","state":{"viewState":{"reset":true}}}));
        assert!(ui.take_commands().is_empty());
        assert!(ui.view_state_deadline().is_none());
    }

    #[test]
    fn unchanged_scroll_and_unsupported_persistence_do_no_work() {
        let mut ui = fixture();
        let revision = ui.revisions();
        ui.input(Input::Scroll { delta: -10.0 });
        assert_eq!(ui.revisions(), revision);
        assert!(ui.take_commands().is_empty());
        assert!(ui.view_state_deadline().is_none());
        ui.message(json!({"type":"state","state":{"supportsViewState":false}}));
        ui.input(Input::Scroll { delta: 20.0 });
        ui.input(Input::FocusLost);
        assert!(ui.take_commands().is_empty());
        assert!(ui.view_state_deadline().is_none());
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Section {
    Scope,
    Log,
    Buffers,
    Events,
    InputControls,
    Params,
    Midi,
}
impl Section {
    pub const ALL: [Self; 7] = [
        Self::Scope,
        Self::Log,
        Self::Buffers,
        Self::Events,
        Self::InputControls,
        Self::Params,
        Self::Midi,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Log => "log",
            Self::Buffers => "buffers",
            Self::Events => "events",
            Self::InputControls => "inputControls",
            Self::Params => "params",
            Self::Midi => "midi",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Scope => "Scope",
            Self::Log => "Log",
            Self::Buffers => "Buffers",
            Self::Events => "Events",
            Self::InputControls => "Input Controls",
            Self::Params => "Params",
            Self::Midi => "MIDI Keyboard",
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap()
    }
}
pub(crate) struct Presentation {
    pub sections: [bool; 7],
    pub scroll: f32,
    pub folded: BTreeSet<String>,
    pub log_follow: bool,
    pub log_scroll: f32,
    pub log_x: f32,
    pub dirty: bool,
    save_after: Option<Instant>,
}
impl Default for Presentation {
    fn default() -> Self {
        Self {
            sections: [true; 7],
            scroll: 0.0,
            folded: BTreeSet::new(),
            log_follow: true,
            log_scroll: 0.0,
            log_x: 0.0,
            dirty: false,
            save_after: None,
        }
    }
}
impl Presentation {
    pub fn changed(&mut self) {
        self.dirty = true;
        self.flush();
    }
    pub fn scrolled(&mut self) {
        // A pending committed edit keeps its immediate publication.
        if !self.dirty || self.save_after.is_some() {
            self.save_after = Some(Instant::now() + Duration::from_millis(100));
        }
        self.dirty = true;
    }
    pub fn flush(&mut self) {
        self.save_after = None;
    }
    pub fn open(&self, section: Section) -> bool {
        self.sections[section.index()]
    }
    pub fn toggle(&mut self, section: Section) {
        self.sections[section.index()] = !self.open(section);
        self.changed();
    }
    pub fn restore(&mut self, value: &Value) {
        if value["reset"] == true {
            *self = Self::default();
        }
        for section in Section::ALL {
            if let Some(open) = value["sections"][section.name()].as_bool() {
                self.sections[section.index()] = open;
            }
        }
        let scroll = |key| {
            value[key]
                .as_f64()
                .filter(|n| n.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 10_000_000.0) as f32
        };
        self.scroll = scroll("scrollTop");
        self.log_scroll = scroll("logScrollTop");
        self.log_x = scroll("logScrollX");
        if let Some(values) = value["folded"].as_array() {
            self.folded = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        self.log_follow = value["logFollow"].as_bool().unwrap_or(true);
        self.dirty = false;
        self.save_after = None;
    }
    pub fn snapshot(&self) -> Value {
        let sections: serde_json::Map<_, _> = Section::ALL
            .into_iter()
            .map(|s| (s.name().to_owned(), json!(self.open(s))))
            .collect();
        json!({"sections":sections,"scrollTop":self.scroll,"folded":self.folded,
            "logFollow":self.log_follow,"logScrollTop":self.log_scroll,"logScrollX":self.log_x})
    }
}
