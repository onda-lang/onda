import assert from "node:assert/strict";
import test from "node:test";

import {
  BrowserMidiInputs,
  COMPUTER_KEYBOARD_MIDI_INPUT,
  CONNECT_MIDI_INPUT,
  isMidiKeyboardEditingTarget,
  parseMidiMessage,
  partitionHostEvents,
} from "./midi.js";

test("computer MIDI keys ignore editor and form-control targets", () => {
  let selector = "";
  const editorTarget = {
    closest(value) {
      selector = value;
      return { className: "cm-editor" };
    },
  };

  assert.equal(isMidiKeyboardEditingTarget(editorTarget), true);
  assert.match(selector, /\.cm-editor/);
  assert.match(selector, /contenteditable/);
  assert.match(selector, /textarea/);
  assert.equal(isMidiKeyboardEditingTarget({ closest: () => null }), false);
  assert.equal(isMidiKeyboardEditingTarget(null), false);
});

function event(name, params) {
  return {
    name,
    params: params.map(([paramName, type]) => ({ name: paramName, type_repr: type })),
  };
}

test("canonical MIDI and host-context events are hidden from user events", () => {
  const noteOn = event("note_on", [
    ["id", "i32"],
    ["channel", "i32"],
    ["key", "i32"],
    ["velocity", "f32"],
  ]);
  const tempo = event("tempo", [["bpm", "f64"]]);
  const user = event("gate", [["enabled", "bool"]]);
  const partition = partitionHostEvents([noteOn, tempo, user]);

  assert.deepEqual(partition.visible, [user]);
  assert.deepEqual(partition.midi, { available: true, noteOn: true, noteOff: false });
  assert.deepEqual([...partition.declaredMidi], ["note_on"]);
});

test("reserved host event names require their exact signature", () => {
  assert.throws(
    () => partitionHostEvents([event("note_on", [
      ["id", "i32"],
      ["channel", "i32"],
      ["note", "i32"],
      ["velocity", "f32"],
    ])]),
    /exact signature/,
  );
});

test("parses channel voice MIDI with plugin-compatible normalization", () => {
  assert.deepEqual(parseMidiMessage([0x92, 60, 127]), {
    name: "note_on",
    values: [-1, 2, 60, 1],
  });
  assert.deepEqual(parseMidiMessage([0x92, 60, 0]), {
    name: "note_off",
    values: [-1, 2, 60, 0],
  });
  assert.deepEqual(parseMidiMessage([0xe0, 0, 64]), {
    name: "pitch_bend",
    values: [0, 0.5],
  });
  assert.equal(parseMidiMessage([0x90, 60]), null);
  assert.equal(parseMidiMessage([0x90, 60, 0x80]), null);
  assert.equal(parseMidiMessage([0xf8]), null);
});

test("Web MIDI permission failure leaves the computer keyboard usable", async () => {
  const states = [];
  const inputs = new BrowserMidiInputs({
    onState: (state) => states.push(state),
    requestAccess: async () => {
      throw new Error("denied");
    },
  });

  assert.deepEqual(states.at(-1).devices, [
    COMPUTER_KEYBOARD_MIDI_INPUT,
    CONNECT_MIDI_INPUT,
  ]);
  assert.equal(states.at(-1).current, COMPUTER_KEYBOARD_MIDI_INPUT);
  await assert.rejects(inputs.select(CONNECT_MIDI_INPUT), /permission was not granted/);
  assert.deepEqual(states.at(-1).devices, [COMPUTER_KEYBOARD_MIDI_INPUT]);
});

test("Web MIDI routes declared messages and falls back after disconnection", async () => {
  const states = [];
  const events = [];
  const input = { name: "Keys", manufacturer: "", onmidimessage: null };
  const access = { inputs: new Map([["device", input]]), onstatechange: null };
  const inputs = new BrowserMidiInputs({
    onState: (state) => states.push(state),
    onEvent: (name, values) => events.push({ name, values }),
    requestAccess: async () => access,
  });
  inputs.setDeclared(new Set(["note_on"]));

  await inputs.select(CONNECT_MIDI_INPUT);
  await inputs.select("Keys");
  input.onmidimessage({ data: [0x90, 60, 127] });
  input.onmidimessage({ data: [0x80, 60, 0] });
  assert.deepEqual(events, [{ name: "note_on", values: [-1, 0, 60, 1] }]);

  const replacement = { name: "Keys", manufacturer: "", onmidimessage: null };
  access.inputs.set("device", replacement);
  access.onstatechange();
  replacement.onmidimessage({ data: [0x90, 61, 64] });
  assert.deepEqual(events.at(-1), { name: "note_on", values: [-1, 0, 61, 64 / 127] });

  access.inputs.clear();
  access.onstatechange();
  assert.equal(states.at(-1).current, COMPUTER_KEYBOARD_MIDI_INPUT);
});

test("switching physical MIDI inputs detaches the previous device", async () => {
  const events = [];
  let firstCloseCount = 0;
  const first = {
    name: "First",
    manufacturer: "",
    onmidimessage: null,
    close: async () => { firstCloseCount += 1; },
  };
  const second = { name: "Second", manufacturer: "", onmidimessage: null };
  const access = {
    inputs: new Map([["first", first], ["second", second]]),
    onstatechange: null,
  };
  const inputs = new BrowserMidiInputs({
    onState: () => {},
    onEvent: (name, values) => events.push({ name, values }),
    requestAccess: async () => access,
  });
  inputs.setDeclared(new Set(["note_on", "note_off"]));

  await inputs.select("First");
  first.onmidimessage({ data: [0x90, 60, 100] });
  await inputs.select("Second");

  assert.equal(first.onmidimessage, null);
  assert.equal(firstCloseCount, 1);
  assert.equal(typeof second.onmidimessage, "function");
  assert.deepEqual(events.at(-1), { name: "note_off", values: [-1, 0, 60, 0] });
});

test("duplicate-name MIDI selection survives another device disconnecting", async () => {
  const states = [];
  const first = { id: "first", name: "Keys", manufacturer: "", onmidimessage: null };
  const selected = { id: "selected", name: "Keys", manufacturer: "", onmidimessage: null };
  const access = {
    inputs: new Map([[first.id, first], [selected.id, selected]]),
    onstatechange: null,
  };
  const inputs = new BrowserMidiInputs({
    onState: (state) => states.push(state),
    requestAccess: async () => access,
  });

  await inputs.select("Keys (2)");
  access.inputs.delete(first.id);
  access.onstatechange();

  assert.deepEqual(states.at(-1), {
    devices: [COMPUTER_KEYBOARD_MIDI_INPUT, "Keys"],
    current: "Keys",
  });
  assert.equal(typeof selected.onmidimessage, "function");
});

test("physical MIDI names cannot collide with the computer keyboard", async () => {
  const states = [];
  const input = {
    name: COMPUTER_KEYBOARD_MIDI_INPUT,
    manufacturer: "",
    onmidimessage: null,
  };
  const access = { inputs: new Map([["device", input]]), onstatechange: null };
  const inputs = new BrowserMidiInputs({
    onState: (state) => states.push(state),
    requestAccess: async () => access,
  });

  await inputs.select(CONNECT_MIDI_INPUT);
  assert.deepEqual(states.at(-1).devices, [
    COMPUTER_KEYBOARD_MIDI_INPUT,
    `${COMPUTER_KEYBOARD_MIDI_INPUT} (2)`,
  ]);
  await inputs.select(`${COMPUTER_KEYBOARD_MIDI_INPUT} (2)`);
  assert.equal(typeof input.onmidimessage, "function");
});

test("Web MIDI reports asynchronous event-dispatch failures", async () => {
  const errors = [];
  const input = { name: "Keys", manufacturer: "", onmidimessage: null };
  const access = { inputs: new Map([["device", input]]), onstatechange: null };
  const inputs = new BrowserMidiInputs({
    onState: () => {},
    onEvent: async () => {
      throw new Error("event failed");
    },
    onError: (error) => errors.push(error.message),
    requestAccess: async () => access,
  });
  inputs.setDeclared(new Set(["note_on"]));
  await inputs.select("Keys");

  input.onmidimessage({ data: [0x90, 60, 100] });
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(errors, ["event failed"]);
});

test("disconnecting a held Web MIDI note releases it before fallback", async () => {
  const events = [];
  const input = { name: "Keys", manufacturer: "", onmidimessage: null };
  const access = { inputs: new Map([["device", input]]), onstatechange: null };
  const inputs = new BrowserMidiInputs({
    onState: () => {},
    onEvent: (name, values) => events.push({ name, values }),
    requestAccess: async () => access,
  });
  inputs.setDeclared(new Set(["note_on", "note_off"]));
  await inputs.select("Keys");
  input.onmidimessage({ data: [0x91, 64, 127] });

  access.inputs.clear();
  access.onstatechange();

  assert.deepEqual(events, [
    { name: "note_on", values: [-1, 1, 64, 1] },
    { name: "note_off", values: [-1, 1, 64, 0] },
  ]);
});
