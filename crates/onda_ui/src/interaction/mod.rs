//! Transient input ownership. Rendering observes this state; only input handling
//! changes gestures, focus, menus, hover feedback, and owned MIDI notes.
mod focus;
mod input;
mod midi;
mod select;
mod tooltip;
mod touch;

pub(crate) use focus::{FocusEntry, FocusLayer, FocusTarget};
pub use input::{Input, Key};
pub(crate) use midi::{DEFAULT_VELOCITY, KEYS};
pub(crate) use select::{build_menu, SelectId};
pub(crate) use tooltip::{build as build_tooltip, Tooltip};

use crate::{
    model::RunUi,
    scene::{Rect, Scene, Target},
};
use midi::Midi;
use serde_json::Value;

pub(crate) struct Interaction {
    gesture: Option<input::Gesture>,
    focus: Option<Target>,
    focus_visible: bool,
    pointer: [f32; 2],
    devices_open: bool,
    popup_scroll: f32,
    scrollbar_drag: Option<f32>,
    chooser: Option<select::Menu>,
    hover: Option<tooltip::Hover>,
    touch: Option<touch::Touch>,
    midi: Midi,
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            gesture: None,
            focus: None,
            focus_visible: false,
            pointer: [-1.0, -1.0],
            devices_open: false,
            popup_scroll: 0.0,
            scrollbar_drag: None,
            chooser: None,
            hover: None,
            touch: None,
            midi: Midi::default(),
        }
    }
}

impl Interaction {
    fn cancel(&mut self) -> (Option<usize>, Vec<Value>) {
        self.hover = None;
        self.touch = None;
        self.scrollbar_drag = None;
        self.chooser = None;
        self.focus = None;
        (
            self.gesture.take().map(|gesture| gesture.index),
            self.midi.release(),
        )
    }
    pub(crate) fn pointer(&self) -> [f32; 2] {
        self.pointer
    }
    pub(crate) fn focus(&self) -> Option<&Target> {
        self.focus.as_ref()
    }
    pub(crate) fn focus_field(&mut self, field: crate::editing::Field) {
        self.focus = Some(Target::Field(field));
    }
    pub(crate) fn settings_open(&self) -> bool {
        self.devices_open
    }
    pub(crate) fn popup_scroll(&self) -> f32 {
        self.popup_scroll
    }
    pub(crate) fn menu(&self) -> Option<&select::Menu> {
        self.chooser.as_ref()
    }
    pub(crate) fn dragging_control(&self, index: usize) -> bool {
        self.gesture.as_ref().is_some_and(|g| g.index == index)
    }
    pub(crate) fn dragging_scrollbar(&self) -> bool {
        self.scrollbar_drag.is_some()
    }
    pub(crate) fn midi(&self) -> &Midi {
        &self.midi
    }
    pub(crate) fn set_octave(&mut self, octave: i32) -> Vec<Value> {
        let notes = self.midi.release();
        self.midi.octave = octave;
        notes
    }
    pub(crate) fn set_velocity(&mut self, velocity: f64) {
        self.midi.velocity = velocity;
    }
    pub(crate) fn restore_midi(&mut self, view: &Value) {
        self.midi.octave = view["octave"]
            .as_i64()
            .unwrap_or(i64::from(midi::DEFAULT_OCTAVE))
            .clamp(-1, 7) as i32;
        self.midi.velocity = view["velocity"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(midi::DEFAULT_VELOCITY)
            .clamp(0.0, 1.0);
    }
    pub(crate) fn reset_midi_settings(&mut self) {
        self.midi.octave = midi::DEFAULT_OCTAVE;
        self.midi.velocity = midi::DEFAULT_VELOCITY;
    }
    pub(crate) fn obscures(&self, scene: &Scene, x: f32, y: f32) -> bool {
        (self.devices_open && Rect(scene.popup_rect).contains(x, y))
            || (self.chooser.is_some() && Rect(scene.menu_rect).contains(x, y))
    }
    pub(crate) fn clamp_popup_scroll(&mut self, max: f32) -> bool {
        if self.devices_open && self.popup_scroll > max {
            self.popup_scroll = max;
            true
        } else {
            false
        }
    }
}

impl RunUi {
    pub(crate) fn active_control(&self) -> Option<usize> {
        self.interaction
            .gesture
            .as_ref()
            .map(|gesture| gesture.index)
            .or_else(|| self.editor.active_control())
    }

    pub(crate) fn cancel_interaction(&mut self) {
        let (control, notes) = self.interaction.cancel();
        if let Some(index) = control {
            self.commands.push(self.controls[index].end_command());
        }
        self.cancel_editing();
        self.commands.extend(notes);
    }

    pub(crate) fn release_midi(&mut self) {
        self.commands.extend(self.interaction.midi.release());
        self.interaction.midi.monitored.fill(false);
    }

    pub(crate) fn monitor_midi(&mut self, notes: &Value) {
        self.interaction.midi.monitor(notes);
        if self.presentation.open(crate::presentation::Section::Midi) {
            self.invalidate();
        }
    }
}
