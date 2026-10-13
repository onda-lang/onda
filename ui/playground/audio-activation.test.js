import assert from "node:assert/strict";
import test from "node:test";
import { bindAudioActivation } from "./audio-activation.js";

function activationFixture(t) {
  const ownerDocument = {
    addEventListener: t.mock.fn(),
    removeEventListener: t.mock.fn(),
  };
  const resume = t.mock.fn();
  const stop = t.mock.fn();
  const unbind = bindAudioActivation(ownerDocument, { resume, stop });
  const interact = event => {
    const binding = ownerDocument.addEventListener.mock.calls
      .find(call => call.arguments[0] === event.type);
    binding?.arguments[1]({ isTrusted: true, ...event });
  };
  return { ownerDocument, resume, stop, unbind, interact };
}

test("audio activation resumes synchronously on mouse, touch, pen, and keyboard input", t => {
  for (const event of [
    { type: "pointerdown", pointerType: "mouse" },
    { type: "pointerup", pointerType: "touch" },
    { type: "pointerup", pointerType: "pen" },
    { type: "keydown", key: "a" },
  ]) {
    const { interact, resume, stop } = activationFixture(t);
    interact(event);
    assert.equal(resume.mock.callCount(), 1, JSON.stringify(event));
    assert.equal(stop.mock.callCount(), 0);
  }
});

test("audio activation ignores synthetic input and gestures that do not unlock audio", t => {
  for (const event of [
    { type: "pointerdown", pointerType: "mouse", isTrusted: false },
    { type: "pointerup", pointerType: "touch", isTrusted: false },
    { type: "keydown", key: "a", isTrusted: false },
    { type: "pointerup", pointerType: "mouse" },
    { type: "pointerdown", pointerType: "touch" },
    { type: "pointerdown", pointerType: "pen" },
    { type: "keydown", key: "a", repeat: true },
    { type: "keydown", key: "a", isComposing: true },
    ...["Escape", "Control", "Meta", "Alt", "Shift"].map(key => ({ type: "keydown", key })),
  ]) {
    const { interact, resume, stop } = activationFixture(t);
    interact(event);
    assert.equal(resume.mock.callCount(), 0, JSON.stringify(event));
    assert.equal(stop.mock.callCount(), 0);
  }
});

test("the Stop button cancels playback synchronously without resuming audio", t => {
  const target = { closest: selector => selector === "#stop" ? { disabled: false } : null };
  for (const event of [
    { type: "pointerdown", pointerType: "mouse" },
    { type: "pointerup", pointerType: "touch" },
    { type: "keydown", key: "Enter" },
    { type: "keydown", key: " " },
  ]) {
    const { interact, resume, stop } = activationFixture(t);
    interact({ ...event, target });
    assert.equal(stop.mock.callCount(), 1, JSON.stringify(event));
    assert.equal(resume.mock.callCount(), 0);
  }
});

test("Ctrl/Command+Period cancels playback without resuming audio", t => {
  for (const modifier of ["ctrlKey", "metaKey"]) {
    for (const key of [{ key: "." }, { key: ">", code: "Period" }]) {
      const { interact, resume, stop } = activationFixture(t);
      interact({ type: "keydown", [modifier]: true, ...key });
      assert.equal(stop.mock.callCount(), 1);
      assert.equal(resume.mock.callCount(), 0);
    }
  }
});

test("ordinary input on Stop and Alt+Period do not invoke Stop", t => {
  for (const event of [
    { type: "keydown", key: "a", target: { closest: () => ({ disabled: false }) } },
    { type: "pointerdown", pointerType: "mouse", target: { closest: () => ({ disabled: true }) } },
    { type: "keydown", key: ".", ctrlKey: true, altKey: true },
  ]) {
    const { interact, resume, stop } = activationFixture(t);
    interact(event);
    assert.equal(resume.mock.callCount(), 1);
    assert.equal(stop.mock.callCount(), 0);
  }
});

test("audio activation captures input and removes its listeners when unbound", t => {
  const { ownerDocument, unbind } = activationFixture(t);
  const bindings = ownerDocument.addEventListener.mock.calls.map(call => call.arguments);
  assert.deepEqual(bindings.map(([type, , options]) => [type, options]), [
    ["pointerdown", { capture: true }],
    ["pointerup", { capture: true }],
    ["keydown", { capture: true }],
  ]);
  unbind();
  assert.deepEqual(ownerDocument.removeEventListener.mock.calls.map(call => call.arguments), bindings);
});
