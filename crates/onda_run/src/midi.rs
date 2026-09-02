use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::time::Duration;
use std::time::Instant;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use midir::{Ignore, MidiInput, MidiInputConnection, MidiInputPort};
use onda_daemon::RunEventValue;

use crate::COMPUTER_KEYBOARD_MIDI_INPUT;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const DEVICE_DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MidiMessageKind {
    NoteOn,
    NoteOff,
    PolyPressure,
    PitchBend,
    ChannelPressure,
    ControlChange,
    ProgramChange,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MidiMessage {
    pub kind: MidiMessageKind,
    pub channel: i32,
    pub key_or_controller: i32,
    pub value: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedMidiMessage {
    pub timestamp: Instant,
    pub message: MidiMessage,
}

impl MidiMessage {
    pub(crate) fn event(self) -> (&'static str, Vec<RunEventValue>) {
        let number = |value| RunEventValue::Number(f64::from(value));
        let integer = |value| RunEventValue::Number(f64::from(value));
        match self.kind {
            MidiMessageKind::NoteOn => (
                "note_on",
                vec![
                    integer(-1),
                    integer(self.channel),
                    integer(self.key_or_controller),
                    number(self.value),
                ],
            ),
            MidiMessageKind::NoteOff => (
                "note_off",
                vec![
                    integer(-1),
                    integer(self.channel),
                    integer(self.key_or_controller),
                    number(self.value),
                ],
            ),
            MidiMessageKind::PolyPressure => (
                "poly_pressure",
                vec![
                    integer(self.channel),
                    integer(self.key_or_controller),
                    number(self.value),
                ],
            ),
            MidiMessageKind::PitchBend => (
                "pitch_bend",
                vec![integer(self.channel), number(self.value)],
            ),
            MidiMessageKind::ChannelPressure => (
                "channel_pressure",
                vec![integer(self.channel), number(self.value)],
            ),
            MidiMessageKind::ControlChange => (
                "cc",
                vec![
                    integer(self.channel),
                    integer(self.key_or_controller),
                    number(self.value),
                ],
            ),
            MidiMessageKind::ProgramChange => (
                "program_change",
                vec![integer(self.channel), integer(self.key_or_controller)],
            ),
        }
    }
}

pub(crate) fn parse_message(bytes: &[u8]) -> Option<MidiMessage> {
    let (&status, data) = bytes.split_first()?;
    if !(0x80..0xf0).contains(&status) {
        return None;
    }
    let channel = i32::from(status & 0x0f);
    let seven_bit = |index: usize| data.get(index).copied().filter(|value| *value < 0x80);
    let normalized = |value: u8| f32::from(value) / 127.0;
    let mut message = MidiMessage {
        kind: MidiMessageKind::NoteOn,
        channel,
        key_or_controller: 0,
        value: 0.0,
    };
    match status & 0xf0 {
        0x80 => {
            message.kind = MidiMessageKind::NoteOff;
            message.key_or_controller = i32::from(seven_bit(0)?);
            message.value = normalized(seven_bit(1)?);
        }
        0x90 => {
            message.key_or_controller = i32::from(seven_bit(0)?);
            let velocity = seven_bit(1)?;
            message.kind = if velocity == 0 {
                MidiMessageKind::NoteOff
            } else {
                MidiMessageKind::NoteOn
            };
            message.value = normalized(velocity);
        }
        0xa0 => {
            message.kind = MidiMessageKind::PolyPressure;
            message.key_or_controller = i32::from(seven_bit(0)?);
            message.value = normalized(seven_bit(1)?);
        }
        0xb0 => {
            message.kind = MidiMessageKind::ControlChange;
            message.key_or_controller = i32::from(seven_bit(0)?);
            message.value = normalized(seven_bit(1)?);
        }
        0xc0 => {
            message.kind = MidiMessageKind::ProgramChange;
            message.key_or_controller = i32::from(seven_bit(0)?);
        }
        0xd0 => {
            message.kind = MidiMessageKind::ChannelPressure;
            message.value = normalized(seven_bit(0)?);
        }
        0xe0 => {
            message.kind = MidiMessageKind::PitchBend;
            let value = u16::from(seven_bit(0)?) | (u16::from(seven_bit(1)?) << 7);
            message.value = normalize_pitch_bend(value);
        }
        _ => return None,
    }
    Some(message)
}

fn normalize_pitch_bend(value: u16) -> f32 {
    const CENTER: u16 = 8192;
    if value <= CENTER {
        f32::from(value) / 16384.0
    } else {
        0.5 + f32::from(value - CENTER) / 16382.0
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn input_devices() -> Vec<String> {
    available_inputs().map_or_else(
        |_| Vec::new(),
        |devices| devices.into_iter().map(|device| device.label).collect(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn input_devices() -> Vec<String> {
    Vec::new()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) struct MidiInputManager {
    identity: InputIdentity,
    connection: Option<MidiInputConnection<InputCallbackState>>,
    sender: SyncSender<TimedMidiMessage>,
    overflowed: Arc<AtomicBool>,
    reset_requested: Arc<AtomicBool>,
    last_discovery: Instant,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl MidiInputManager {
    pub(crate) fn open(
        requested_label: &str,
        sender: SyncSender<TimedMidiMessage>,
        overflowed: Arc<AtomicBool>,
        reset_requested: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let devices = available_inputs()?;
        let device = requested_input(&devices, requested_label)
            .cloned()
            .ok_or_else(|| format!("unknown MIDI input device '{requested_label}'"))?;
        let identity = device.identity();
        let connection = connect_input(device, sender.clone(), Arc::clone(&overflowed))?;
        Ok(Self {
            identity,
            connection: Some(connection),
            sender,
            overflowed,
            reset_requested,
            last_discovery: Instant::now(),
        })
    }

    pub(crate) fn poll(&mut self) {
        if self.last_discovery.elapsed() < DEVICE_DISCOVERY_INTERVAL {
            return;
        }
        self.last_discovery = Instant::now();
        let Ok(devices) = available_inputs() else {
            return;
        };
        if self.connection.is_some() && devices.iter().any(|device| device.id == self.identity.id) {
            return;
        }
        if self.connection.take().is_some() {
            self.reset_requested.store(true, Ordering::Release);
        }
        let Some(device) = resolve_identity(&devices, &self.identity).cloned() else {
            return;
        };
        let identity = device.identity();
        if let Ok(connection) =
            connect_input(device, self.sender.clone(), Arc::clone(&self.overflowed))
        {
            self.identity = identity;
            self.connection = Some(connection);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) struct MidiInputManager;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl MidiInputManager {
    pub(crate) fn open(
        _requested_name: &str,
        _sender: SyncSender<TimedMidiMessage>,
        _overflowed: Arc<AtomicBool>,
        _reset_requested: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        Err("native MIDI input is only supported on Linux, macOS, and Windows".to_owned())
    }

    pub(crate) fn poll(&mut self) {}
}

fn enqueue_message(
    sender: &SyncSender<TimedMidiMessage>,
    overflowed: &AtomicBool,
    message: TimedMidiMessage,
) {
    if matches!(sender.try_send(message), Err(TrySendError::Full(_))) {
        overflowed.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputIdentity {
    id: String,
    name: String,
    ordinal: usize,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone)]
struct AvailableInput {
    label: String,
    id: String,
    name: String,
    ordinal: usize,
    port: MidiInputPort,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl AvailableInput {
    fn identity(&self) -> InputIdentity {
        InputIdentity {
            id: self.id.clone(),
            name: self.name.clone(),
            ordinal: self.ordinal,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn available_inputs() -> Result<Vec<AvailableInput>, String> {
    let input = MidiInput::new("onda device discovery")
        .map_err(|error| format!("failed to initialize MIDI input: {error}"))?;
    let ports = input.ports();
    let names = ports
        .iter()
        .map(|port| {
            input
                .port_name(port)
                .unwrap_or_else(|_| "Unavailable MIDI input".to_owned())
        })
        .collect::<Vec<_>>();
    let labels = disambiguate_names(&names);
    let mut ordinals = HashMap::<String, usize>::new();
    Ok(ports
        .into_iter()
        .zip(names)
        .zip(labels)
        .map(|((port, name), label)| {
            let ordinal = ordinals.entry(name.clone()).or_default();
            let device = AvailableInput {
                label,
                id: port.id(),
                name,
                ordinal: *ordinal,
                port,
            };
            *ordinal += 1;
            device
        })
        .collect())
}

fn disambiguate_names(names: &[String]) -> Vec<String> {
    let mut totals = HashMap::<&str, usize>::new();
    totals.insert(COMPUTER_KEYBOARD_MIDI_INPUT, 1);
    for name in names {
        *totals.entry(name).or_default() += 1;
    }
    let mut ordinals = HashMap::<&str, usize>::new();
    ordinals.insert(COMPUTER_KEYBOARD_MIDI_INPUT, 1);
    let mut used = HashSet::from([COMPUTER_KEYBOARD_MIDI_INPUT.to_owned()]);
    names
        .iter()
        .map(|name| {
            let ordinal = ordinals.entry(name).or_default();
            *ordinal += 1;
            let base = if totals[name.as_str()] == 1 {
                name.clone()
            } else {
                format!("{name} ({ordinal})")
            };
            if used.insert(base.clone()) {
                return base;
            }
            let mut suffix = 2;
            loop {
                let label = format!("{base} [{suffix}]");
                if used.insert(label.clone()) {
                    break label;
                }
                suffix += 1;
            }
        })
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn requested_input<'a>(
    devices: &'a [AvailableInput],
    requested: &str,
) -> Option<&'a AvailableInput> {
    devices
        .iter()
        .find(|device| device.label == requested)
        .or_else(|| {
            let mut matches = devices.iter().filter(|device| device.name == requested);
            let device = matches.next()?;
            matches.next().is_none().then_some(device)
        })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn resolve_identity<'a>(
    devices: &'a [AvailableInput],
    identity: &InputIdentity,
) -> Option<&'a AvailableInput> {
    devices
        .iter()
        .find(|device| device.id == identity.id)
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.name == identity.name && device.ordinal == identity.ordinal)
        })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct InputCallbackState {
    sender: SyncSender<TimedMidiMessage>,
    overflowed: Arc<AtomicBool>,
    clock_anchor: Option<(u64, Instant)>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl InputCallbackState {
    fn timestamp(&mut self, timestamp_micros: u64) -> Instant {
        let now = Instant::now();
        match self.clock_anchor {
            Some((anchor_micros, anchor_time)) if timestamp_micros >= anchor_micros => anchor_time
                .checked_add(Duration::from_micros(timestamp_micros - anchor_micros))
                .unwrap_or(now),
            _ => {
                self.clock_anchor = Some((timestamp_micros, now));
                now
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn connect_input(
    device: AvailableInput,
    sender: SyncSender<TimedMidiMessage>,
    overflowed: Arc<AtomicBool>,
) -> Result<MidiInputConnection<InputCallbackState>, String> {
    let mut input = MidiInput::new("onda run")
        .map_err(|error| format!("failed to initialize MIDI input: {error}"))?;
    input.ignore(Ignore::All);
    let label = device.label;
    input
        .connect(
            &device.port,
            "onda run input",
            move |timestamp, bytes, state| {
                if let Some(message) = parse_message(bytes) {
                    let message = TimedMidiMessage {
                        timestamp: state.timestamp(timestamp),
                        message,
                    };
                    enqueue_message(&state.sender, &state.overflowed, message);
                }
            },
            InputCallbackState {
                sender,
                overflowed,
                clock_anchor: None,
            },
        )
        .map_err(|error| format!("failed to open MIDI input '{label}': {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use std::time::Instant;

    use super::{
        disambiguate_names, enqueue_message, parse_message, MidiMessage, MidiMessageKind,
        TimedMidiMessage,
    };

    #[test]
    fn parses_channel_messages_using_plugin_normalization() {
        let note = parse_message(&[0x92, 60, 127]).expect("note on");
        assert_eq!(note.kind, MidiMessageKind::NoteOn);
        assert_eq!(note.channel, 2);
        assert_eq!(note.key_or_controller, 60);
        assert_eq!(note.value, 1.0);

        let off = parse_message(&[0x92, 60, 0]).expect("zero velocity note off");
        assert_eq!(off.kind, MidiMessageKind::NoteOff);

        let bend = parse_message(&[0xe0, 0, 64]).expect("center pitch bend");
        assert_eq!(bend.kind, MidiMessageKind::PitchBend);
        assert_eq!(bend.value, 0.5);

        let cases = [
            (
                [0x80, 60, 64],
                MidiMessageKind::NoteOff,
                60,
                64.0_f32 / 127.0,
            ),
            (
                [0xa0, 61, 32],
                MidiMessageKind::PolyPressure,
                61,
                32.0_f32 / 127.0,
            ),
            (
                [0xb0, 74, 96],
                MidiMessageKind::ControlChange,
                74,
                96.0_f32 / 127.0,
            ),
            ([0xc0, 12, 0], MidiMessageKind::ProgramChange, 12, 0.0_f32),
            (
                [0xd0, 48, 0],
                MidiMessageKind::ChannelPressure,
                0,
                48.0_f32 / 127.0,
            ),
        ];
        for (bytes, kind, key_or_controller, value) in cases {
            let message = parse_message(&bytes).expect("channel message");
            assert_eq!(message.kind, kind);
            assert_eq!(message.key_or_controller, key_or_controller);
            assert!((message.value - value).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn ignores_system_and_incomplete_messages() {
        assert_eq!(parse_message(&[0xf8]), None);
        assert_eq!(parse_message(&[0x90, 60]), None);
        assert_eq!(parse_message(&[0x01, 60, 100]), None);
    }

    #[test]
    fn full_input_queue_reports_overflow_without_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let overflowed = AtomicBool::new(false);
        let message = MidiMessage {
            kind: MidiMessageKind::NoteOn,
            channel: 0,
            key_or_controller: 60,
            value: 1.0,
        };

        let message = TimedMidiMessage {
            timestamp: Instant::now(),
            message,
        };
        enqueue_message(&sender, &overflowed, message);
        assert!(!overflowed.load(Ordering::Acquire));
        enqueue_message(&sender, &overflowed, message);
        assert!(overflowed.load(Ordering::Acquire));
    }

    #[test]
    fn duplicate_input_names_get_stable_unique_labels() {
        let names = vec![
            "Keyboard".to_owned(),
            "Keyboard".to_owned(),
            "Keyboard (1)".to_owned(),
            "Drums".to_owned(),
        ];
        assert_eq!(
            disambiguate_names(&names),
            vec!["Keyboard (1)", "Keyboard (2)", "Keyboard (1) [2]", "Drums"]
        );
    }

    #[test]
    fn physical_input_names_do_not_collide_with_the_virtual_keyboard() {
        let names = vec!["Computer Keyboard".to_owned()];
        assert_eq!(disambiguate_names(&names), vec!["Computer Keyboard (2)"]);
    }
}
