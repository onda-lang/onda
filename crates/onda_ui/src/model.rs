use onda_mir::{ParamDomain, ParamScale, ScalarType};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::scene::Scene;
use crate::{
    editing::Editor,
    events::Event,
    interaction::Interaction,
    presentation::{Presentation, Section},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Control {
    pub metadata: Value,
    pub value: Value,
    pub input: bool,
    pub domain: Option<ParamDomain<'static>>,
}

impl Control {
    pub fn name(&self) -> &str {
        self.metadata["name"].as_str().unwrap_or("")
    }
    pub fn label(&self) -> &str {
        self.metadata["label"]
            .as_str()
            .unwrap_or_else(|| self.name())
    }
    pub fn display_name(&self) -> String {
        match self.metadata["unit"].as_str().filter(|s| !s.is_empty()) {
            Some(unit) => format!("{} ({unit})", self.label()),
            None => self.label().to_owned(),
        }
    }
    pub fn boolean(&self) -> bool {
        self.metadata["type"] == "bool"
    }
    pub fn enabled(&self) -> bool {
        self.metadata["enabled"] != false
            && !number(&self.metadata["rangeMin"])
                .zip(number(&self.metadata["rangeMax"]))
                .is_some_and(|(lo, hi)| lo == hi)
    }
    pub fn normalized(&self) -> f64 {
        if self.boolean() {
            return if self.value == true { 1.0 } else { 0.0 };
        }
        self.domain
            .map(|d| d.plain_to_normalized(number(&self.value).unwrap_or(0.0)))
            .unwrap_or(0.0)
    }
    pub fn from_metadata(mut metadata: Value, input: bool) -> Option<Self> {
        metadata["name"].as_str()?;
        let scalar = match metadata["type"].as_str()? {
            "f32" => ScalarType::F32,
            "f64" => ScalarType::F64,
            "i32" => ScalarType::I32,
            "i64" => ScalarType::I64,
            "bool" => ScalarType::Bool,
            _ => return None,
        };
        let domain = number(&metadata["rangeMin"])
            .zip(number(&metadata["rangeMax"]))
            .and_then(|(lo, hi)| {
                ParamDomain::new(
                    scalar,
                    lo,
                    hi,
                    if metadata["scale"] == "log" {
                        ParamScale::Log
                    } else {
                        ParamScale::Linear
                    },
                    number(&metadata["curve"]),
                    None,
                    number(&metadata["step"]),
                    metadata["stepCount"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok()),
                )
            });
        let mut value = metadata
            .as_object_mut()?
            .remove("value")
            .unwrap_or_else(|| {
                if scalar == ScalarType::Bool {
                    return Value::Bool(number(&metadata["normalizedValue"]).unwrap_or(0.0) >= 0.5);
                }
                domain
                    .map(|d| {
                        json!(d.normalized_to_plain(
                            number(&metadata["normalizedValue"]).unwrap_or(0.0)
                        ))
                    })
                    .unwrap_or(Value::Null)
            });
        if scalar == ScalarType::Bool {
            value = boolean_value(&value);
            if let Some(default) = metadata.get_mut("default").filter(|v| !v.is_null()) {
                *default = boolean_value(default);
            }
        }
        metadata.as_object_mut()?.remove("normalizedValue");
        Some(Self {
            metadata,
            value,
            input,
            domain,
        })
    }
    pub fn wire_value(&self) -> Value {
        if self.metadata["hostValueDomain"] == "normalized" {
            json!(self.normalized())
        } else {
            self.value.clone()
        }
    }
    pub fn end_command(&self) -> Value {
        json!({"type": if self.input {"endInputControlGesture"} else {"endParamGesture"}, "name": self.name()})
    }
    pub fn command(&self, commit: bool) -> Value {
        json!({"type": if self.input {"setInputControl"} else {"setParam"},
            "name": self.name(), "value": self.wire_value(), "commit": commit})
    }
    pub fn parse(&self, text: &str) -> Option<Value> {
        let ty = self.metadata["type"].as_str()?;
        if ty == "i64" || ty == "i32" {
            let mut n = text.trim().parse::<i64>().ok()?;
            if ty == "i32" {
                n = i64::from(i32::try_from(n).ok()?);
            }
            if let Some(d) = self.domain {
                n = d.constrain_plain(n as f64) as i64;
            }
            return Some(if ty == "i64" {
                json!(n.to_string())
            } else {
                json!(n)
            });
        }
        let n = text.trim().parse::<f64>().ok().filter(|n| n.is_finite())?;
        let n = self.domain.map(|d| d.constrain_plain(n)).unwrap_or(n);
        let n = if ty == "f32" { f64::from(n as f32) } else { n };
        n.is_finite().then(|| json!(n))
    }
}

pub(crate) fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

fn boolean_value(value: &Value) -> Value {
    Value::Bool(
        value
            .as_bool()
            .unwrap_or_else(|| number(value).is_some_and(|n| n >= 0.5)),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FileDropTarget {
    Source,
    Buffer(String),
}

pub struct RunUi {
    pub(crate) id: usize,
    pub(crate) state: Value,
    pub(crate) controls: Vec<Control>,
    control_indices: [HashMap<String, usize>; 2],
    pub(crate) control_layouts: [std::cell::RefCell<Option<crate::controls::Layout>>; 2],
    pub(crate) scene: Scene,
    pub(crate) scene_dirty: bool,
    pub(crate) revision: u64,
    pub(crate) scope_revision: u64,
    pub(crate) trace: Vec<[i16; 4]>,
    trace_shape: [usize; 2],
    pub(crate) scope: onda_scope::Envelope,
    pub(crate) scale: f32,
    pub(crate) size: [f32; 2],
    pub(crate) scroll: f32,
    pub(crate) knobs: bool,
    pub(crate) input_knobs: bool,
    pub(crate) presentation: Presentation,
    pub(crate) pending_view: Option<Value>,
    view_restore_id: Option<Value>,
    pub(crate) events: Vec<Event>,
    pub(crate) event_layout: std::cell::RefCell<Option<crate::panels::events::Layout>>,
    pub(crate) drop_target: Option<FileDropTarget>,
    pub(crate) interaction: Interaction,
    pub(crate) editor: Editor,
    pub(crate) commands: Vec<Value>,
    pub(crate) light: bool,
    pub(crate) diagnostic_height: f32,
    pub(crate) log_revision: u64,
    pub(crate) log_max_scroll_x: f32,
    pub(crate) tooltip_size: [f32; 2],
}

impl Default for RunUi {
    fn default() -> Self {
        static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            state: json!({"status":"Choose an Onda source or project", "supportsSourceSelection":false}),
            controls: Vec::new(),
            control_indices: Default::default(),
            control_layouts: Default::default(),
            scene: Scene::default(),
            scene_dirty: true,
            revision: 1,
            scope_revision: 0,
            trace: Vec::new(),
            trace_shape: [0, 0],
            scope: Default::default(),
            scale: 1.0,
            size: [720.0, 840.0],
            scroll: 0.0,
            knobs: true,
            input_knobs: true,
            presentation: Presentation::default(),
            pending_view: None,
            view_restore_id: None,
            events: Vec::new(),
            event_layout: Default::default(),
            drop_target: None,
            interaction: Interaction::default(),
            editor: Editor::default(),
            commands: Vec::new(),
            light: false,
            diagnostic_height: 16.8,
            log_revision: 0,
            log_max_scroll_x: 0.0,
            tooltip_size: [0.0, 16.8],
        }
    }
}

impl RunUi {
    /// Apply the existing run-view message schema. Values are always plain unless
    /// the metadata explicitly declares the plugin's normalized host domain.
    pub fn message(&mut self, message: Value) {
        match message["type"].as_str() {
            Some("scopeData") => {
                let channels = message["channels"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0);
                let samples = message["samples"].as_array();
                let frames = samples
                    .map(|s| s.len() / channels.max(1))
                    .unwrap_or(0)
                    .min(16384);
                self.set_scope(channels, frames, |frame, channel| {
                    samples
                        .and_then(|s| s.get(frame * channels + channel))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0) as f32
                });
            }
            Some("state") => {
                let Some(incoming) = message["state"].as_object() else {
                    return;
                };
                if ["logText", "logEntries"].iter().any(|key| {
                    incoming
                        .get(*key)
                        .is_some_and(|value| self.state[*key] != *value)
                }) {
                    self.log_revision = self.log_revision.wrapping_add(1);
                }
                if !self.presentation.open(Section::Log)
                    && incoming.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "logText"
                                | "logEntries"
                                | "logRevealed"
                                | "printOverflowCount"
                                | "printTransportDropCount"
                                | "delegateOverflowCount"
                                | "delegateTransportDropCount"
                        )
                    })
                {
                    let was_present = self.log_present();
                    for (key, value) in incoming {
                        self.state[key] = value.clone();
                    }
                    if was_present != self.log_present() {
                        self.invalidate();
                    }
                    return;
                }
                let source_changed = incoming
                    .get("path")
                    .is_some_and(|p| *p != self.state["path"]);
                let reset = incoming
                    .get("viewState")
                    .is_some_and(|v| v["reset"] == true);
                let view = incoming
                    .get("viewState")
                    .filter(|v| v.is_object())
                    .filter(|view| {
                        source_changed
                            || reset
                            || self.state.get("viewState").is_none()
                            || view
                                .get("readyId")
                                .is_some_and(|id| self.view_restore_id.as_ref() != Some(id))
                    })
                    .cloned();
                if source_changed {
                    self.publish_view_state(true);
                    self.drop_target = None;
                }
                if source_changed || reset {
                    self.knobs = true;
                    self.input_knobs = true;
                    self.state.as_object_mut().unwrap().remove("paramLayout");
                    self.state
                        .as_object_mut()
                        .unwrap()
                        .remove("inputControlLayout");
                    self.event_layout.get_mut().take();
                    self.clear_control_layouts();
                    self.cancel_interaction();
                    self.interaction.reset_midi_settings();
                    self.pending_view = None;
                    self.view_restore_id = None;
                    self.state.as_object_mut().unwrap().remove("viewState");
                    self.presentation = Presentation::default();
                    if source_changed {
                        self.events.clear();
                    } else {
                        for event in &mut self.events {
                            event.reset_arguments();
                        }
                    }
                    self.scroll = 0.0;
                    self.set_scope_interleaved(0, &[]);
                } else if view.is_some() {
                    self.cancel_interaction();
                }
                if source_changed
                    || (incoming.get("running") == Some(&Value::Bool(false))
                        && self.state["running"] == true)
                    || incoming.get("midi").is_some_and(|m| m["noteOn"] == false)
                    || incoming
                        .get("currentMidiInputDevice")
                        .is_some_and(|d| *d != self.state["currentMidiInputDevice"])
                {
                    self.release_midi();
                }
                for (bank, input) in [("inputControls", true), ("params", false)] {
                    if let Some(items) = incoming.get(bank).and_then(Value::as_array) {
                        let replacements: Vec<_> = items
                            .iter()
                            .filter_map(|p| Control::from_metadata(p.clone(), input))
                            .collect();
                        let schema_changed = !self
                            .controls
                            .iter()
                            .filter(|c| c.input == input)
                            .map(|c| &c.metadata)
                            .eq(replacements.iter().map(|c| &c.metadata));
                        if schema_changed {
                            self.cancel_interaction();
                            self.clear_control_layouts();
                        }
                        let active_name = self
                            .active_control()
                            .map(|index| &self.controls[index])
                            .filter(|c| c.input == input)
                            .map(|c| c.name().to_owned());
                        let mut replacements = replacements;
                        if let Some(name) = active_name {
                            if let Some(old) = self
                                .controls
                                .iter()
                                .find(|c| c.input == input && c.name() == name)
                            {
                                if let Some(new) =
                                    replacements.iter_mut().find(|c| c.name() == name)
                                {
                                    new.value = old.value.clone();
                                }
                            }
                        }
                        self.controls.retain(|c| c.input != input);
                        if input {
                            self.controls.splice(0..0, replacements);
                        } else {
                            self.controls.extend(replacements);
                        }
                    }
                }
                if ["inputControls", "params"]
                    .iter()
                    .any(|bank| incoming.get(*bank).is_some_and(Value::is_array))
                {
                    for indices in &mut self.control_indices {
                        indices.clear();
                    }
                    for (index, control) in self.controls.iter().enumerate() {
                        self.control_indices[control.input as usize]
                            .entry(control.name().to_owned())
                            .or_insert(index);
                    }
                }
                if let Some(items) = incoming.get("events").and_then(Value::as_array) {
                    let mut events: Vec<_> =
                        items.iter().filter_map(Event::from_metadata).collect();
                    if reset {
                        for event in &mut events {
                            event.reset_arguments();
                        }
                    }
                    let changed = events.len() != self.events.len()
                        || !events
                            .iter()
                            .zip(&self.events)
                            .all(|(a, b)| a.same_schema(b));
                    if changed || incoming.get("resetEventArguments") == Some(&Value::Bool(true)) {
                        self.event_layout.get_mut().take();
                    }
                    if changed {
                        self.cancel_interaction();
                    }
                    if !source_changed
                        && !reset
                        && incoming.get("resetEventArguments") != Some(&Value::Bool(true))
                    {
                        for event in &mut events {
                            if let Some(old) = self.events.iter().find(|e| e.same_schema(event)) {
                                for (arg, old) in event.args.iter_mut().zip(&old.args) {
                                    arg.value = old.value.clone();
                                }
                            }
                        }
                    } else {
                        self.cancel_editing();
                    }
                    self.events = events;
                }
                if let Some(view) = view {
                    self.view_restore_id = view.get("readyId").cloned();
                    self.pending_view = Some(view);
                }
                for (key, value) in incoming {
                    self.state[key] = value.clone();
                }
                self.light = self.state["themeMode"] == "light";
                if self.state["paramLayout"] == "knobs" {
                    self.knobs = true;
                }
                if self.state["paramLayout"] == "sliders" {
                    self.knobs = false;
                }
                if self.state["inputControlLayout"] == "knobs" {
                    self.input_knobs = true;
                }
                if self.state["inputControlLayout"] == "sliders" {
                    self.input_knobs = false;
                }
                if self.state["schemaReady"] != false {
                    if let Some(view) = self.pending_view.take() {
                        self.restore_view(&view);
                    }
                }
                self.invalidate();
            }
            Some("midiActivity") => {
                self.monitor_midi(&message["activeNotes"]);
            }
            Some("bufferFileDrag" | "sourceFileDrag") => {
                self.prepare_scene();
                let x = message["position"]["x"].as_f64().unwrap_or(-1.0) as f32;
                let y = message["position"]["y"].as_f64().unwrap_or(-1.0) as f32;
                let previous = self.drop_target.clone();
                let path = message["paths"].as_array().and_then(|paths| {
                    let path = paths.first()?.as_str()?;
                    // Hosts validate their own supported file formats before forwarding.
                    (!path.is_empty() && paths.iter().all(|p| p.as_str() == Some(path)))
                        .then_some(path)
                });
                self.drop_target = if message["phase"] == "leave"
                    || (message.get("paths").is_some() && path.is_none())
                {
                    None
                } else if message["type"] == "sourceFileDrag" {
                    self.scene
                        .source_picker
                        .filter(|rect| rect.contains(x, y) && !self.file_drop_obscured(x, y))
                        .map(|_| FileDropTarget::Source)
                } else {
                    self.buffer_target(x, y).map(FileDropTarget::Buffer)
                };
                if message["phase"] == "drop" {
                    if let (Some(FileDropTarget::Buffer(name)), Some(path)) =
                        (self.drop_target.take(), path)
                    {
                        self.commands
                            .push(json!({"type":"bindBufferFile","name":name,"filePath":path}));
                    }
                }
                if previous != self.drop_target {
                    self.invalidate();
                }
            }
            Some("parameterValues") => {
                let mut changed = false;
                for (bank, input) in [("params", false), ("inputControls", true)] {
                    if let Some(values) = message["state"][bank].as_array() {
                        for update in values {
                            let Some(index) = update["name"]
                                .as_str()
                                .and_then(|name| self.control_indices[input as usize].get(name))
                                .copied()
                            else {
                                continue;
                            };
                            if self.active_control() == Some(index) {
                                continue;
                            }
                            let c = &mut self.controls[index];
                            let value = update.get("value").cloned().or_else(|| {
                                if c.boolean() {
                                    return number(&update["normalizedValue"])
                                        .map(|n| Value::Bool(n >= 0.5));
                                }
                                c.domain
                                    .zip(number(&update["normalizedValue"]))
                                    .map(|(d, n)| json!(d.normalized_to_plain(n)))
                            });
                            if let Some(value) = value {
                                let value = if c.boolean() {
                                    boolean_value(&value)
                                } else {
                                    value
                                };
                                if value != c.value {
                                    c.value = value;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                if changed {
                    self.invalidate();
                }
            }
            _ => {}
        }
    }
    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        let size = [width.max(1.0), height.max(1.0)];
        if self.size != size || self.scale != scale {
            self.size = size;
            self.scale = scale;
            self.invalidate();
        }
    }
    pub fn scope_columns(&self) -> usize {
        onda_scope::columns_for_width((self.size[0] - 44.0).max(1.0) * self.scale)
    }
    pub fn set_scope_interleaved(&mut self, channels: usize, samples: &[f32]) {
        self.scope
            .set_interleaved(channels, samples, self.scope_columns());
        self.prepare_scope();
    }
    fn set_scope(&mut self, channels: usize, frames: usize, sample: impl Fn(usize, usize) -> f32) {
        self.scope
            .update(channels, frames, self.scope_columns(), sample);
        self.prepare_scope();
    }
    pub fn set_scope_envelope(&mut self, envelope: &onda_scope::Envelope) {
        self.scope.copy_from(envelope);
        self.prepare_scope();
    }
    pub fn set_scope_packet(&mut self, packet: &[u8]) -> Result<(), &'static str> {
        self.scope.read_packet(packet)?;
        self.prepare_scope();
        Ok(())
    }
    fn prepare_scope(&mut self) {
        let columns = self.scope.columns();
        let shape = [self.scope.channels(), columns];
        if shape[0] != self.trace_shape[0] {
            self.invalidate();
        }
        let mut changed = self.trace_shape != shape;
        self.trace.resize(self.scope.ranges().len(), [0; 4]);
        if columns > 0 {
            for (source, target) in self
                .scope
                .ranges()
                .chunks_exact(columns)
                .zip(self.trace.chunks_exact_mut(columns))
            {
                for (i, quad) in target.iter_mut().enumerate() {
                    let [lo, hi] = source[i];
                    let [next_lo, next_hi] = source[(i + 1).min(columns - 1)];
                    let next = [lo, hi, next_lo, next_hi];
                    changed |= *quad != next;
                    *quad = next;
                }
            }
        }
        self.trace_shape = shape;
        if changed {
            self.scope_revision = self.scope_revision.wrapping_add(1);
        }
    }
    pub fn revisions(&self) -> (u64, u64) {
        (self.revision, self.scope_revision)
    }
    pub fn take_commands(&mut self) -> Vec<Value> {
        self.publish_view_state(false);
        std::mem::take(&mut self.commands)
    }
    /// The visible buffer accepting a host-owned file at logical coordinates.
    pub fn buffer_target(&mut self, x: f32, y: f32) -> Option<String> {
        self.prepare_scene();
        if self.file_drop_obscured(x, y) {
            return None;
        }
        self.scene
            .buffers
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, name)| name.clone())
    }
    fn file_drop_obscured(&self, x: f32, y: f32) -> bool {
        self.interaction.obscures(&self.scene, x, y)
    }
    pub fn scope_enabled(&self) -> bool {
        self.presentation.open(Section::Scope) && self.state["supportsScope"] != false
    }
    pub(crate) fn log_present(&self) -> bool {
        self.state["logRevealed"] == true
            || self.state["logText"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
    }
    pub(crate) fn bank_knobs(&self, input: bool) -> bool {
        if input {
            self.input_knobs
        } else {
            self.knobs
        }
    }
    pub(crate) fn has_controls(&self, input: bool) -> bool {
        !self.control_indices[input as usize].is_empty()
    }
    pub(crate) fn clear_control_layouts(&mut self) {
        for layout in &mut self.control_layouts {
            layout.get_mut().take();
        }
    }
    pub(crate) fn midi_interactive(&self) -> bool {
        self.state["midi"]["noteOn"] == true && self.state["midiKeyboardInteractive"] != false
    }
    pub(crate) fn invalidate(&mut self) {
        self.scene_dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }
    pub(crate) fn prepare_scene(&mut self) {
        self.prepare_scene_at(web_time::Instant::now());
    }
    pub(crate) fn prepare_scene_at(&mut self, now: web_time::Instant) {
        if self.scene_dirty {
            self.scene = crate::scene::build(self);
            let max = (self.scene.content_height - self.scene.viewport_height).max(0.0);
            let scroll = self.scroll.clamp(0.0, max);
            if scroll != self.scroll {
                self.scroll = scroll;
                self.scene = crate::scene::build(self);
            }
            let max = (self.scene.popup_height - self.scene.popup_rect[3]).max(0.0);
            if self.interaction.clamp_popup_scroll(max) {
                self.scene = crate::scene::build(self);
            }
            self.scene_dirty = false;
        }
        if self.update_hover(now) {
            self.invalidate();
            self.scene = crate::scene::build(self);
            self.scene_dirty = false;
        }
    }
}
