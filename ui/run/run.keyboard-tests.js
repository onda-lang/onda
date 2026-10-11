import assert from "node:assert/strict";
import { once } from "node:events";

// Trusted keyboard actions exercise native input/change behavior that dispatched
// DOM events cannot reproduce. Firefox exposes BiDi without a separate driver.
export async function testNumberKeyboardEditing(t, endpoint, pageUrl) {
  const socket = new WebSocket(`${endpoint}/session`);
  t.after(() => socket.close());
  await once(socket, "open", { signal: t.signal });
  let nextId = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const reply = JSON.parse(data);
    const request = pending.get(reply.id);
    if (!request) return;
    pending.delete(reply.id);
    if (reply.type === "error") request.reject(new Error(JSON.stringify(reply)));
    else request.resolve(reply.result);
  });
  const failPending = error => {
    for (const request of pending.values()) request.reject(error);
    pending.clear();
  };
  socket.addEventListener("close", () => failPending(new Error("Browser connection closed")));
  t.signal.addEventListener("abort", () => failPending(new Error("Keyboard test timed out")), { once: true });
  const call = (method, params) => {
    const request = Promise.withResolvers();
    const id = ++nextId;
    pending.set(id, request);
    socket.send(JSON.stringify({ id, method, params }));
    return request.promise;
  };
  await call("session.new", { capabilities: {} });
  let context;
  const evaluate = async expression => {
    const reply = await call("script.evaluate", {
      expression, target: { context }, awaitPromise: true,
    });
    assert.equal(reply.type, "success", JSON.stringify(reply));
    return reply.result.value;
  };
  const keys = values => call("input.performActions", {
    context,
    actions: [{ type: "key", id: "keyboard", actions: values.flatMap(value => [
      { type: "keyDown", value }, { type: "keyUp", value },
    ]) }],
  });
  const initial = 1023.095538565032;
  const initialize = async layout => {
    // Each case starts with a fresh document and focus, including after Tab has
    // left the page for browser chrome.
    if (context) await call("browsingContext.close", { context });
    ({ context } = await call("browsingContext.create", { type: "tab" }));
    await call("browsingContext.navigate", { context, url: pageUrl, wait: "complete" });
    await evaluate(`
      window._onHostMessage({ type: "state", state: {
        running: true, connected: true, path: "keyboard.onda", events: [],
        params: [{ name: "precision", type: "f64",
          value: ${initial}, default: ${initial} }],
      }});
      document.querySelector('[data-param-layout="${layout}"]').click();
      window.__testMessages.length = 0;
      window.number = document.querySelector('#params input');
      number.focus();
      number.select();
    `);
    assert.equal(await evaluate("number.readOnly"), false, "field must enter editing before keyboard actions");
    assert.equal(await evaluate("number.value"), "1023.09554");
  };
  const messages = async () => JSON.parse(await evaluate(
    `JSON.stringify(__testMessages.filter(message => message.type === "setParam"))`,
  ));
  const enter = "\uE007";
  const escape = "\uE00C";
  const tab = "\uE004";
  const up = "\uE013";
  await initialize("knobs");
  await evaluate(`
    window._onHostMessage({ type: "state", state: {
      supportsTransport: true, supportsSourceSelection: true,
      supportsDeviceSelection: true, supportsProjectExport: true, canExportProject: true,
      inputDevices: ['Input'], outputDevices: ['Output'],
    }});
    document.getElementById('settings-toggle').focus();
  `);
  await keys([enter]);
  assert.equal(await evaluate("document.getElementById('settings-popover').matches(':popover-open')"), true);
  assert.equal(await evaluate("document.activeElement.id"), "input-device");
  await keys([escape]);
  assert.equal(await evaluate("document.getElementById('settings-popover').matches(':popover-open')"), false);
  assert.equal(await evaluate("document.activeElement.id"), "settings-toggle");
  await keys([enter]);
  await call("input.performActions", {
    context,
    actions: [{ type: "pointer", id: "header-pointer", parameters: { pointerType: "mouse" }, actions: [
      { type: "pointerMove", x: 30, y: 30, duration: 0 },
      { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 },
    ] }],
  });
  assert.equal(await evaluate("document.getElementById('settings-popover').matches(':popover-open')"), false);
  await evaluate("document.getElementById('save-project').focus()");
  await keys([enter]);
  assert.equal(await evaluate("__testMessages.at(-1).type"), "saveProjectAs");
  await keys([tab]);
  assert.equal(await evaluate("document.activeElement.id"), "unload");
  await keys([enter]);
  assert.equal(await evaluate("__testMessages.at(-1).type"), "unload");
  t.diagnostic("device settings support Escape, outside clicks, and focus return; direct file icons support keyboard activation");
  await initialize("knobs");
  await evaluate(`window._onHostMessage({ type: 'state', state: {
    supportsTransport: true, supportsDeviceSelection: true, running: false,
  }}); document.getElementById('start').focus(); __testMessages.length = 0;`);
  await keys([enter]);
  assert.equal(await evaluate("__testMessages.at(-1).type"), "start");
  await evaluate("window._onHostMessage({ type: 'state', state: { running: true } })");
  assert.equal(await evaluate("document.activeElement.id"), "stop");
  await keys([enter]);
  assert.equal(await evaluate("__testMessages.at(-1).type"), "stop");
  await evaluate("window._onHostMessage({ type: 'state', state: { running: false } })");
  assert.equal(await evaluate("document.activeElement.id"), "start");
  await keys([tab]);
  assert.equal(await evaluate("document.activeElement.id"), "settings-toggle");
  t.diagnostic("Play/Stop retain keyboard focus through host updates and Tab advances to Devices");
  await evaluate(`window._onHostMessage({ type: 'state', state: {
    supportsSourceSelection: true, supportsProjectExport: true, canExportProject: true,
  }})`);
  for (const width of [720, 400, 320, 240, 180, 720]) {
    await call("browsingContext.setViewport", {
      context, viewport: { width, height: 840 }, devicePixelRatio: 1,
    });
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
    const order = JSON.parse(await evaluate(`JSON.stringify(
      ['start', 'settings-toggle', 'save-project', 'unload']
        .map(id => ({ id, rect: document.getElementById(id).getBoundingClientRect() }))
        .sort((a, b) => Math.round(a.rect.top + a.rect.height / 2)
          - Math.round(b.rect.top + b.rect.height / 2) || a.rect.left - b.rect.left)
        .map(control => control.id)
    )`));
    await evaluate(`document.getElementById(${JSON.stringify(order[0])}).focus()`);
    for (const id of order.slice(1)) {
      await keys([tab]);
      assert.equal(await evaluate("document.activeElement.id"), id,
        `Tab follows the visible header rows at ${width}px`);
    }
  }
  for (const id of ["start", "settings-toggle"]) {
    await evaluate(`document.getElementById(${JSON.stringify(id)}).focus()`);
    for (const width of [320, 180, 720]) {
      await call("browsingContext.setViewport", {
        context, viewport: { width, height: 840 }, devicePixelRatio: 1,
      });
      await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
      assert.equal(await evaluate("document.activeElement.id"), id,
        `resizing to ${width}px preserves ${id} focus`);
    }
  }
  t.diagnostic("compact and stacked headers follow visual Tab order and retain focus while resizing");
  const chord = (modifier, key) => call("input.performActions", {
    context,
    actions: [{ type: "key", id: "keyboard", actions: [
      { type: "keyDown", value: modifier }, { type: "keyDown", value: key },
      { type: "keyUp", value: key }, { type: "keyUp", value: modifier },
    ] }],
  });
  for (const modifier of ["\uE009", "\uE03D"]) {
    for (const key of ["s", enter]) {
      await initialize("knobs");
      await evaluate(`window._onHostMessage({ type: 'state', state: {
        supportsTransport: true, running: false,
      }}); __testMessages.length = 0;`);
      await chord(modifier, key);
      assert.deepEqual(JSON.parse(await evaluate("JSON.stringify(__testMessages)")),
        [{ type: "start", path: "keyboard.onda" }],
        "save/run shortcuts play once while a number field is focused without committing its draft");
      assert.equal(await evaluate("document.activeElement === number"), true);
    }
    await initialize("knobs");
    await evaluate(`window._onHostMessage({ type: 'state', state: {
      supportsTransport: true, midi: { available: true, noteOn: true },
      currentMidiInputDevice: 'Computer Keyboard',
    }}); number.blur(); __testMessages.length = 0;`);
    await call("input.performActions", { context, actions: [{ type: "key", id: "keyboard",
      actions: [{ type: "keyDown", value: "a" }] }] });
    await chord(modifier, ".");
    await call("input.performActions", { context, actions: [{ type: "key", id: "keyboard",
      actions: [{ type: "keyUp", value: "a" }] }] });
    const sent = JSON.parse(await evaluate("JSON.stringify(__testMessages)"));
    assert.deepEqual(sent.map(message => [message.type, message.pressed]),
      [["midiNote", true], ["midiNote", false], ["stop", undefined]],
      "the stop shortcut releases held MIDI notes before stopping, without duplicate releases");
  }
  t.diagnostic("Ctrl/Command+S and Enter play from focused fields; Ctrl/Command+. releases held notes and stops");
  for (const layout of ["knobs", "sliders"]) {
    for (const end of [enter, escape, tab]) {
      await initialize(layout);
      await keys([end]);
      assert.deepEqual(await messages(), [], `${layout}: untouched drafts retain precision`);
    }
    await initialize(layout);
    await keys(["9", "9", escape]);
    assert.deepEqual(await messages(), [], `${layout}: Escape must not commit the shortened draft`);

    await initialize(layout);
    await keys(["9", "9"]);
    await evaluate(`window._onHostMessage({ type: "state", state: { params: [{
      name: number.getAttribute("aria-label"), type: "f64", value: 2048.987654321,
      default: ${initial},
    }] }});`);
    await keys([escape]);
    assert.deepEqual(await messages(), [], `${layout}: Escape reconciles host automation without a commit`);
    assert.equal(await evaluate("number.value"), "2048.988");

    for (const [inputKeys, expected, description] of [
      [["9", "9", enter], 99, "Enter commits typed input once"],
      [[up, enter], 1023.096, "Enter commits an arrow edit matching the idle display"],
      [[up, tab], 1023.096, "blur commits an arrow edit matching the idle display"],
      [[..."1023.096", enter], 1023.096, "typing the idle display still changes the canonical value"],
    ]) {
      await initialize(layout);
      await keys(inputKeys);
      const sent = await messages();
      assert.equal(sent.length, 1, `${layout}: ${description}`);
      assert.equal(sent[0].value, expected, `${layout}: ${description}`);
      assert.equal(sent[0].commit, true);
      assert.equal(await evaluate("number.readOnly"), true);
    }
    t.diagnostic(`${layout}: trusted keyboard edits preserve cancellation precision and commit on Enter or blur`);
  }
  const eventValues = [initial, 0.0000123456789];
  const initializeEvent = async index => {
    await initialize("knobs");
    await evaluate(`
      window._onHostMessage({ type: "state", state: {
        params: [], events: [{ name: "precision", args: [
          { name: "scalar", type: "f64", default: ${eventValues[0]} },
          { name: "elements", type: "f64[1]", arrayLength: 1, default: [${eventValues[1]}] },
        ] }],
      }});
      window.number = document.querySelectorAll('#events input')[${index}];
      number.focus();
      number.select();
    `);
    assert.equal(await evaluate("number.value"), ["1023.09554", "1.23457e-5"][index]);
  };
  const triggeredValues = async () => JSON.parse(await evaluate(`
    document.querySelector('.event-trigger').click();
    JSON.stringify(__testMessages.findLast(message => message.type === 'triggerEvent').values);
  `));
  for (const index of [0, 1]) {
    for (const inputKeys of [[enter], [escape], [tab], ["9", "9", escape]]) {
      await initializeEvent(index);
      await keys(inputKeys);
      assert.deepEqual(await triggeredValues(), [eventValues[0], [eventValues[1]]],
        `event field ${index}: ${JSON.stringify(inputKeys)} retains the exact event values`);
    }
    await initializeEvent(index);
    await keys([..."0.123456789012345", enter]);
    const values = await triggeredValues();
    assert.equal(index === 0 ? values[0] : values[1][0], 0.123456789012345,
      "event editors accept more precise typed values than their initial draft");
  }
  t.diagnostic("event scalars and array elements shorten focused drafts without losing stored or typed precision");
  await call("session.end", {});
}
