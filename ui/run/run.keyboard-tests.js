import assert from "node:assert/strict";
import { openBrowserSession } from "../browser-test-session.js";

// Trusted keyboard actions exercise native input/change behavior that dispatched
// DOM events cannot reproduce. Firefox exposes BiDi without a separate driver.
export async function testNumberKeyboardEditing(t, endpoint, pageUrl) {
  const call = await openBrowserSession(t, endpoint);
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
      document.documentElement.lang = "it-IT";
      window._onHostMessage({ type: "state", state: {
        running: true, connected: true, path: "keyboard.onda", events: [],
        params: [{ name: "precision", type: "f64",
          value: ${initial}, default: ${initial} }],
      }});
      document.querySelector('#params-section [data-param-layout="${layout}"]').click();
      window.__testMessages.length = 0;
      window.number = document.querySelector('#params input');
      number.focus();
      number.select();
    `);
    assert.equal(await evaluate("number.readOnly"), false, "field must enter editing before keyboard actions");
    assert.equal(await evaluate("number.value"), "1023.09554");
    assert.equal(await evaluate("getSelection().toString()"), "1023.09554",
      "visible decimals use a dot with an Italian document locale");
  };
  const messages = async () => JSON.parse(await evaluate(
    `JSON.stringify(__testMessages.filter(message => message.type === "setParam"))`,
  ));
  const midiMessages = async () => JSON.parse(await evaluate(
    `JSON.stringify(__testMessages.filter(message => message.type === "midiNote"))`,
  ));
  const enter = "\uE007";
  const escape = "\uE00C";
  const tab = "\uE004";
  const up = "\uE013";
  const down = "\uE015";
  await initialize("knobs");
  await evaluate(`window._onHostMessage({ type: "state", state: {
    params: [{ name: "precision", type: "f64", value: 10, default: 10,
      rangeMin: 0, rangeMax: 100, scale: "linear", step: 1, stepCount: 100 }],
  }}); window.number = document.querySelector('#params input');
  window.numberChanges = 0;
  number.addEventListener('change', () => numberChanges++);`);
  for (const draft of ["10.", "+10", " 10 "]) {
    for (const [arrow, expected] of [[up, 11], [down, 9]]) {
      await evaluate("number.focus(); number.select(); __testMessages.length = 0; numberChanges = 0");
      await keys([...draft, arrow]);
      assert.equal(await evaluate("number.value"), String(expected),
        `arrow stepping parses the decimal draft ${JSON.stringify(draft)}`);
      assert.deepEqual(await messages(), [], "arrow edits remain drafts until committed");
      await keys([enter]);
      assert.deepEqual((await messages()).map(message => [message.value, message.commit]),
        [[expected, true]], "Enter commits the stepped decimal once");
      assert.equal(await evaluate("numberChanges"), 1, "arrow edits emit one change on commit");
    }
  }
  t.diagnostic("arrow keys step accepted decimal notation without losing its value");
  for (const [arrow, end, octave] of [[up, enter, 5], [down, tab, 3], [up, escape, 4], [null, enter, 4]]) {
    await initialize("knobs");
    await evaluate(`window._onHostMessage({ type: "state", state: {
      midi: { available: true, noteOn: true }, currentMidiInputDevice: "Computer Keyboard",
      supportsViewState: true,
    }}); number.blur(); __testMessages.length = 0;`);
    await call("input.performActions", { context, actions: [{ type: "key", id: "keyboard",
      actions: [{ type: "keyDown", value: "a" }] }] });
    await evaluate("document.getElementById('midi-octave').focus()");
    if (arrow) await keys([arrow]);
    await evaluate(`window._onHostMessage({ type: "state", state: { status: "Playing" } });
      window.dispatchEvent(new Event("resize"));`);
    if (arrow) {
      assert.equal(await evaluate("__testMessages.findLast(message => message.type === 'viewState').state.octave"),
        4, "saved view state keeps the committed octave while editing");
    }
    assert.equal(await evaluate("document.querySelector('#piano .piano-key').getAttribute('aria-label')"),
      "MIDI note 60", "host updates and resizing keep the committed octave while editing");
    assert.equal(await evaluate("document.querySelector('#piano [aria-label=\"MIDI note 60\"]').classList.contains('active')"),
      true, "host updates preserve the held note highlight while editing the octave");
    await keys([end]);
    assert.equal(await evaluate("document.getElementById('midi-octave').value"), String(octave));
    if (arrow) {
      assert.equal(await evaluate("__testMessages.findLast(message => message.type === 'viewState').state.octave"),
        octave, "saved view state reflects the committed octave after Enter, blur, or cancellation");
    }
    assert.deepEqual((await midiMessages()).map(note => [note.key, note.pressed]),
      octave === 4 ? [[60, true]] : [[60, true], [60, false]],
      "octave edits release held notes on commit while cancelled or untouched drafts retain them");
    assert.equal(await evaluate("document.querySelector('#piano .piano-key').getAttribute('aria-label')"),
      "MIDI note " + (octave + 1) * 12, "piano labels reflect the committed octave");
    if (octave === 4) {
      assert.equal(await evaluate("document.querySelector('#piano [aria-label=\"MIDI note 60\"]').classList.contains('active')"),
        true, "cancelled and untouched drafts retain the held note highlight");
    }
    await call("input.performActions", { context, actions: [{ type: "key", id: "keyboard",
      actions: [{ type: "keyUp", value: "a" }] }] });
    assert.equal((await midiMessages()).length, 2, "releasing the key does not send a duplicate note-off");
  }
  t.diagnostic("octave arrow edits commit on Enter or blur, releasing notes and refreshing the piano");
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
  for (const layout of ["knobs", "sliders"]) {
    await initialize(layout);
    await evaluate(`window._onHostMessage({ type: "state", state: {
      midi: { available: true, noteOn: true }, currentMidiInputDevice: "Computer Keyboard",
      params: ["gain", "mix"].map(name => ({
        name, type: "f32", value: 0.5, default: 0.5,
        rangeMin: 0, rangeMax: 1, scale: "linear",
      })),
    }}); window.range = document.querySelector('#params [role="slider"]');
    window.number = document.querySelector('#params input');`);
    const position = JSON.parse(await evaluate(`JSON.stringify((() => {
      const rect = range.getBoundingClientRect();
      return { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2) };
    })())`));
    for (const pointerType of ["mouse", "touch"]) {
      await evaluate("number.focus(); number.select(); __testMessages.length = 0");
      await keys(["0", ".", "7"]);
      await call("input.performActions", { context, actions: [{
        type: "pointer", id: `${layout}-${pointerType}`, parameters: { pointerType }, actions: [
          { type: "pointerMove", ...position, duration: 0 },
          { type: "pointerDown", button: 0 },
        ],
      }] });
      assert.equal(await evaluate("document.activeElement === range"), false,
        `${layout}: ${pointerType} interaction does not focus the range control`);
      if (layout === "knobs") {
        assert.equal((await messages())[0]?.value, 0.7,
          `${pointerType}: clicking the knob commits the number draft before using its value`);
      }
      await call("input.releaseActions", { context });
      assert.equal(await evaluate("number.readOnly"), true,
        `${layout}: ${pointerType} interaction ends number editing`);
      if (layout === "knobs") {
        assert.equal(await evaluate("number.value"), "0.7",
          `${pointerType}: releasing the knob preserves the committed number draft`);
      }
      await keys(["a"]);
      assert.deepEqual((await midiMessages()).map(note => [note.key, note.pressed]), [[60, true], [60, false]],
        `${layout}: ${pointerType} interaction leaves computer MIDI available`);
      await evaluate("__testMessages.length = 0");
      await evaluate("range.focus()");
      assert.equal(await evaluate("document.activeElement === range"), false,
        `${layout}: range controls cannot receive programmatic focus`);
      await evaluate("number.focus()");
      await keys([tab]);
      assert.equal(await evaluate("document.activeElement === document.querySelectorAll('#params input')[1]"), true,
        `${layout}: Tab skips range controls and advances between number fields`);
    }
  }
  t.diagnostic("knobs commit number drafts before pointer interaction; range controls leave MIDI available and Tab advances between number fields");
  await initialize("knobs");
  await evaluate(`window._onHostMessage({ type: "state", state: {
    midi: { available: true, noteOn: true }, currentMidiInputDevice: "Computer Keyboard",
  }});
  window.afterPiano = document.createElement('button');
  afterPiano.textContent = 'After piano';
  afterPiano.style.cssText = 'position:fixed;bottom:0;right:0';
  document.body.appendChild(afterPiano);
  document.getElementById('midi-velocity-value').focus();`);
  await keys([tab]);
  assert.equal(await evaluate("document.activeElement === afterPiano"), true,
    "Tab skips all piano keys");
  const pianoPosition = JSON.parse(await evaluate(`JSON.stringify((() => {
    const rect = document.querySelector('.piano-key.white').getBoundingClientRect();
    return { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.bottom - 10) };
  })())`));
  for (const pointerType of ["mouse", "touch"]) {
    await evaluate("document.getElementById('midi-velocity-value').focus()");
    await call("input.performActions", { context, actions: [{
      type: "pointer", id: `piano-${pointerType}`, parameters: { pointerType }, actions: [
        { type: "pointerMove", ...pianoPosition, duration: 0 },
        { type: "pointerDown", button: 0 },
        { type: "pointerUp", button: 0 },
      ],
    }] });
    assert.equal(await evaluate("document.getElementById('midi-velocity-value').readOnly"), true,
      `${pointerType}: playing the piano ends number editing`);
    await evaluate("__testMessages.length = 0");
    await keys(["a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j", "k", "o", "l", "p"]);
    const notes = await midiMessages();
    assert.deepEqual(notes.map(note => [note.key, note.pressed]),
      Array.from({ length: 16 }, (_, offset) => [[60 + offset, true], [60 + offset, false]]).flat(),
      `${pointerType}: all computer MIDI keys play and release after piano interaction`);
    await keys(["z"]);
    assert.equal(await evaluate("document.getElementById('midi-octave').value"), "3");
    await keys(["x"]);
    assert.equal(await evaluate("document.getElementById('midi-octave').value"), "4");
  }
  t.diagnostic("piano keys are skipped by Tab; all MIDI note keys and octave shortcuts work after mouse and touch interaction");
  for (const pointerType of ["mouse", "touch"]) {
    await initialize("knobs");
    await call("browsingContext.setViewport", {
      context, viewport: { width: 1400, height: 900 }, devicePixelRatio: 1,
    });
    await evaluate(`window._onHostMessage({ type: "state", state: {
      midi: { available: true, noteOn: true }, currentMidiInputDevice: "Computer Keyboard",
    }});
    new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));`);
    await evaluate(`window.pianoKey = document.querySelector('#piano [data-note-offset="24"]');
    window.octave = document.getElementById('midi-octave');
    window.pianoErrors = [];
    window.addEventListener('error', event => pianoErrors.push(event.message));
    octave.focus(); octave.select(); __testMessages.length = 0;`);
    const position = JSON.parse(await evaluate(`JSON.stringify((() => {
      const rect = pianoKey.getBoundingClientRect();
      return { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.bottom - 10) };
    })())`));
    await keys(["7"]);
    await call("input.performActions", { context, actions: [{
      type: "pointer", id: `octave-${pointerType}`, parameters: { pointerType }, actions: [
        { type: "pointerMove", ...position, duration: 0 },
        { type: "pointerDown", button: 0 },
      ],
    }] });
    assert.equal(await evaluate("octave.readOnly && !pianoKey.isConnected"), true,
      `${pointerType}: committing octave seven removes the pressed third-octave key`);
    assert.deepEqual(JSON.parse(await evaluate("JSON.stringify(pianoErrors)")), [],
      `${pointerType}: octave editing cannot invalidate pointer capture`);
    assert.deepEqual((await midiMessages()).map(note => [note.key, note.pressed]), [[120, true]],
      `${pointerType}: the piano plays using the committed octave`);
    await call("input.performActions", { context, actions: [{
      type: "pointer", id: `octave-${pointerType}`, parameters: { pointerType }, actions: [
        { type: "pointerMove", x: 5, y: 5, duration: 0 },
        { type: "pointerUp", button: 0 },
      ],
    }] });
    assert.deepEqual((await midiMessages()).map(note => [note.key, note.pressed]), [[120, true], [120, false]],
      `${pointerType}: releasing outside the piano ends the note even when its key disappeared`);
  }
  t.diagnostic("piano notes play and release across the layout change caused by committing an octave edit");
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
