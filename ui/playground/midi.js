import HOST_EVENT_CATALOG from "../../crates/onda_host_protocol/events.json" with { type: "json" };

export const COMPUTER_KEYBOARD_MIDI_INPUT = "Computer Keyboard";
export const CONNECT_MIDI_INPUT = "Connect MIDI device…";

const HOST_EVENTS = new Map(HOST_EVENT_CATALOG.events.map(({ name, params }) => [name, params]));
const MIDI_EVENT_NAMES = new Set(HOST_EVENT_CATALOG.events
  .filter(({ family }) => family === "midi")
  .map(({ name }) => name));

const EDITABLE_KEYBOARD_TARGETS =
  'input, textarea, select, [contenteditable]:not([contenteditable="false"]), .cm-editor';

export function isMidiKeyboardEditingTarget(target) {
  return typeof target?.closest === "function"
    && target.closest(EDITABLE_KEYBOARD_TARGETS) !== null;
}

function exactSignature(event, expected) {
  const params = event.params ?? [];
  return params.length === expected.length && params.every((param, index) =>
    param.name === expected[index][0] && param.type_repr === expected[index][1]
  );
}

export function partitionHostEvents(events) {
  const visible = [];
  const declaredMidi = new Set();
  for (const event of events) {
    const expected = HOST_EVENTS.get(event.name);
    if (!expected) {
      visible.push(event);
      continue;
    }
    if (!exactSignature(event, expected)) {
      const signature = expected.map(([name, type]) => `${name}: ${type}`).join(", ");
      throw new Error(
        `canonical host event '${event.name}' must use the exact signature (${signature})`,
      );
    }
    if (MIDI_EVENT_NAMES.has(event.name)) declaredMidi.add(event.name);
  }
  return {
    visible,
    declaredMidi,
    midi: {
      available: declaredMidi.size > 0,
      noteOn: declaredMidi.has("note_on"),
      noteOff: declaredMidi.has("note_off"),
    },
  };
}

export function parseMidiMessage(data) {
  if (!data || data.length < 1) return null;
  const status = data[0];
  if (status < 0x80 || status >= 0xf0) return null;
  const kind = status & 0xf0;
  const channel = status & 0x0f;
  const byte = (index) => {
    const value = data[index];
    return Number.isInteger(value) && value >= 0 && value < 0x80 ? value : null;
  };
  const first = byte(1);
  const second = byte(2);
  switch (kind) {
    case 0x80:
      if (first === null || second === null) return null;
      return { name: "note_off", values: [-1, channel, first, second / 127] };
    case 0x90:
      if (first === null || second === null) return null;
      return second === 0
        ? { name: "note_off", values: [-1, channel, first, 0] }
        : { name: "note_on", values: [-1, channel, first, second / 127] };
    case 0xa0:
      if (first === null || second === null) return null;
      return { name: "poly_pressure", values: [channel, first, second / 127] };
    case 0xb0:
      if (first === null || second === null) return null;
      return { name: "cc", values: [channel, first, second / 127] };
    case 0xc0:
      if (first === null) return null;
      return { name: "program_change", values: [channel, first] };
    case 0xd0:
      if (first === null) return null;
      return { name: "channel_pressure", values: [channel, first / 127] };
    case 0xe0: {
      if (first === null || second === null) return null;
      const value = first | (second << 7);
      const normalized = value <= 8192
        ? value / 16384
        : 0.5 + (value - 8192) / 16382;
      return { name: "pitch_bend", values: [channel, normalized] };
    }
    default:
      return null;
  }
}

export class BrowserMidiInputs {
  constructor({
    onState,
    onEvent,
    onError,
    requestAccess = globalThis.navigator?.requestMIDIAccess?.bind(globalThis.navigator),
  }) {
    this.onState = onState;
    this.onEvent = onEvent;
    this.onError = onError;
    this.access = null;
    this.current = COMPUTER_KEYBOARD_MIDI_INPUT;
    this.currentId = null;
    this.inputs = new Map();
    this.inputIds = new Map();
    this.declared = new Set();
    this.activeNotes = new Map();
    this.permissionUnavailable = false;
    this.requestAccess = requestAccess;
    this.publish();
  }

  setDeclared(events) {
    this.declared = new Set(events);
  }

  async refresh(requestPermission = true) {
    if (!this.access && requestPermission) await this.ensureAccess();
    this.rebuildInputs();
  }

  async select(name) {
    if (!name) {
      await this.disconnect();
      this.publish();
      return;
    }
    if (name === COMPUTER_KEYBOARD_MIDI_INPUT) {
      await this.disconnect();
      this.publish();
      return;
    }
    if (name === CONNECT_MIDI_INPUT) {
      await this.ensureAccess();
      this.rebuildInputs();
      return;
    }
    if (!this.access) await this.ensureAccess();
    this.rebuildInputs(false);
    const input = this.inputs.get(name);
    if (!input) throw new Error(`MIDI input '${name}' is no longer available`);
    const id = this.inputIds.get(name);
    if (id !== this.currentId) await this.disconnect();
    this.current = name;
    this.currentId = id;
    this.connectCurrent();
    this.publish();
  }

  async ensureAccess() {
    if (this.access) return this.access;
    if (typeof this.requestAccess !== "function") {
      throw new Error("Web MIDI is not supported by this browser");
    }
    try {
      this.access = await this.requestAccess({ sysex: false });
    } catch (error) {
      this.permissionUnavailable = true;
      this.publish();
      throw new Error(`MIDI permission was not granted: ${error?.message ?? error}`, {
        cause: error,
      });
    }
    this.permissionUnavailable = false;
    this.access.onstatechange = () => this.rebuildInputs();
    return this.access;
  }

  async disconnect() {
    const input = this.inputs.get(this.current);
    this.releaseActiveNotes();
    this.detachInputs();
    this.current = COMPUTER_KEYBOARD_MIDI_INPUT;
    this.currentId = null;
    if (typeof input?.close === "function") {
      try {
        await input.close();
      } catch {
        // A port that disappeared while selected is already unavailable.
      }
    }
  }

  detachInputs() {
    for (const input of this.inputs.values()) input.onmidimessage = null;
  }

  releaseActiveNotes() {
    if (this.declared.has("note_off")) {
      for (const { channel, key } of this.activeNotes.values()) {
        this.dispatchEvent("note_off", [-1, channel, key, 0]);
      }
    }
    this.activeNotes.clear();
  }

  dispatchEvent(name, values) {
    try {
      Promise.resolve(this.onEvent?.(name, values)).catch((error) => {
        this.onError?.(error);
      });
    } catch (error) {
      this.onError?.(error);
    }
  }

  connectCurrent() {
    if (this.current === COMPUTER_KEYBOARD_MIDI_INPUT) return;
    const input = this.inputs.get(this.current);
    if (!input) return;
    input.onmidimessage = (message) => {
      const event = parseMidiMessage(message.data);
      if (!event || !this.declared.has(event.name)) return;
      if (event.name === "note_on") {
        const [, channel, key] = event.values;
        this.activeNotes.set(`${channel}:${key}`, { channel, key });
      } else if (event.name === "note_off") {
        const [, channel, key] = event.values;
        this.activeNotes.delete(`${channel}:${key}`);
      }
      this.dispatchEvent(event.name, event.values);
    };
  }

  rebuildInputs(publish = true) {
    const previousId = this.currentId;
    const previousInput = this.inputs.get(this.current);
    this.detachInputs();
    this.inputs.clear();
    this.inputIds.clear();
    const names = new Map([[COMPUTER_KEYBOARD_MIDI_INPUT, 1]]);
    const used = new Set([COMPUTER_KEYBOARD_MIDI_INPUT]);
    const labelsById = new Map();
    for (const [portId, input] of this.access?.inputs.entries() ?? []) {
      const id = String(input.id ?? portId);
      const base = input.name || input.manufacturer || "MIDI Input";
      const occurrence = (names.get(base) || 0) + 1;
      names.set(base, occurrence);
      let name = occurrence === 1 ? base : `${base} (${occurrence})`;
      if (used.has(name)) {
        let suffix = 2;
        while (used.has(`${name} [${suffix}]`)) suffix += 1;
        name = `${name} [${suffix}]`;
      }
      used.add(name);
      this.inputs.set(name, input);
      this.inputIds.set(name, id);
      labelsById.set(id, name);
    }
    if (previousId !== null) {
      const current = labelsById.get(previousId);
      const currentInput = current === undefined ? undefined : this.inputs.get(current);
      if (currentInput === undefined) {
        this.releaseActiveNotes();
        this.current = COMPUTER_KEYBOARD_MIDI_INPUT;
        this.currentId = null;
      } else if (currentInput !== previousInput) {
        this.releaseActiveNotes();
      }
      if (currentInput !== undefined) this.current = current;
    }
    this.connectCurrent();
    if (publish) this.publish();
  }

  publish() {
    const devices = [COMPUTER_KEYBOARD_MIDI_INPUT, ...this.inputs.keys()];
    if (
      !this.access
      && !this.permissionUnavailable
      && typeof this.requestAccess === "function"
    ) {
      devices.push(CONNECT_MIDI_INPUT);
    }
    this.onState?.({ devices, current: this.current });
  }
}
