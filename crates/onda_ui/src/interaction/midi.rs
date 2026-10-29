use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const DEFAULT_OCTAVE: i32 = 4;
pub(crate) const DEFAULT_VELOCITY: f64 = 1.0;

pub(crate) const KEYS: [(char, u8); 16] = [
    ('a', 0),
    ('w', 1),
    ('s', 2),
    ('e', 3),
    ('d', 4),
    ('f', 5),
    ('t', 6),
    ('g', 7),
    ('y', 8),
    ('h', 9),
    ('u', 10),
    ('j', 11),
    ('k', 12),
    ('o', 13),
    ('l', 14),
    ('p', 15),
];

pub(crate) struct Midi {
    pub octave: i32,
    pub velocity: f64,
    pub pointer: Option<u8>,
    pub pointer_down: bool,
    pub keyboard: BTreeMap<String, u8>,
    pub touches: BTreeMap<u64, Option<u8>>,
    pub pressed_keys: BTreeSet<String>,
    pub monitored: [bool; 128],
}
impl Default for Midi {
    fn default() -> Self {
        Self {
            octave: DEFAULT_OCTAVE,
            velocity: DEFAULT_VELOCITY,
            pointer: None,
            pointer_down: false,
            keyboard: BTreeMap::new(),
            touches: BTreeMap::new(),
            pressed_keys: BTreeSet::new(),
            monitored: [false; 128],
        }
    }
}
impl Midi {
    pub fn held(&self, key: u8) -> bool {
        self.pointer == Some(key)
            || self.keyboard.values().any(|k| *k == key)
            || self.touches.values().any(|note| *note == Some(key))
    }
    pub fn command(&self, key: u8, pressed: bool) -> Value {
        json!({"type":"midiNote","key":key,"pressed":pressed,"velocity":if pressed {self.velocity} else {0.0}})
    }
    pub fn release(&mut self) -> Vec<Value> {
        let values = (0..128)
            .filter(|k| self.held(*k))
            .map(|k| self.command(k, false))
            .collect();
        self.pointer = None;
        self.pointer_down = false;
        self.keyboard.clear();
        self.touches.clear();
        self.pressed_keys.clear();
        values
    }
    pub fn release_keyboard(&mut self) -> Vec<Value> {
        self.pressed_keys.clear();
        let notes: BTreeSet<_> = std::mem::take(&mut self.keyboard).into_values().collect();
        notes
            .into_iter()
            .filter(|key| !self.held(*key))
            .map(|key| self.command(key, false))
            .collect()
    }
    pub fn shift_octave(&mut self, delta: i32) -> Vec<Value> {
        let next = (self.octave + delta).clamp(-1, 7);
        if next == self.octave {
            return Vec::new();
        }
        // A held octave key must not shift again on keyboard auto-repeat.
        let keys = std::mem::take(&mut self.pressed_keys);
        let commands = self.release();
        self.pressed_keys = keys;
        self.octave = next;
        commands
    }
    pub fn touch_note(&mut self, id: u64, note: Option<u8>, ended: bool) -> Vec<Value> {
        let old = self.touches.get(&id).copied().flatten();
        let held = note.is_some_and(|n| self.held(n));
        if ended {
            self.touches.remove(&id);
        } else {
            self.touches.insert(id, note);
        }
        let mut commands = Vec::new();
        if let Some(old) = old.filter(|n| !self.held(*n)) {
            commands.push(self.command(old, false));
        }
        if let Some(note) = note.filter(|_| !ended && !held) {
            commands.push(self.command(note, true));
        }
        commands
    }
    pub fn monitor(&mut self, notes: &Value) {
        self.monitored.fill(false);
        if let Some(notes) = notes.as_array() {
            for key in notes.iter().filter_map(Value::as_u64).filter(|n| *n < 128) {
                self.monitored[key as usize] = true;
            }
        }
    }
}
