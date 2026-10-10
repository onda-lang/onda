// Executed against the real run view DOM by run.test.js.
const results = [];
function check(condition, message) {
  if (!condition) throw new Error(message);
  results.push(message);
}
function send(state) {
  window._onHostMessage({ type: "state", state: structuredClone(state) });
}
async function waitFrames(count) {
  for (let index = 0; index < count; index++) {
    await new Promise(resolve => requestAnimationFrame(resolve));
  }
}
function edit(input, value) {
  input.focus();
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}
function pointerControl(input, captureTarget = input) {
  // Synthetic pointers cannot acquire browser capture; model only that API.
  let captured = null;
  captureTarget.setPointerCapture = id => { captured = id; };
  captureTarget.hasPointerCapture = id => captured === id;
  captureTarget.releasePointerCapture = () => { captured = null; };
  return (type, clientX, options = {}) => input.dispatchEvent(new PointerEvent(type, {
    pointerId: 1, clientX, button: 0, bubbles: true, cancelable: true, ...options,
  }));
}
function refresh() {
  send({ events, params, logText: "background update", logRevealed: true });
}
const events = [{
  name: "shape",
  args: [
    { name: "level", type: "f32", arrayLength: 1, default: 0.25 },
    { name: "id", type: "i64", arrayLength: 1, default: "0" },
    { name: "points", type: "f32[]", isSlice: true, arrayLength: 0, default: [] },
  ],
}];
const params = [
  { name: "gain", type: "f32", value: 0.25, default: 0.25,
    rangeMin: 0, rangeMax: 1, scale: "linear" },
  { name: "offset", type: "f32", value: 2, default: 2 },
];
try {
  const shell = document.querySelector(".shell");
  const scrollNode = shell;
  const paramsList = document.getElementById("params");
  const resetButton = document.getElementById("reset-params");
  const ondaVersion = document.getElementById("onda-version");
  check(!ondaVersion.hidden && ondaVersion.textContent === "Onda 0.0.0-test",
    "the run view shows the host-provided Onda version");
  check(getComputedStyle(shell).visibility === "hidden",
    "run view stays hidden until its first host state");
  check(getComputedStyle(resetButton).transitionDuration === "0s",
    "buttons do not animate the initial theme while the view is pending");
  send({ running: true, connected: true, path: "test.onda", status: "Active",
    ondaVersion: "0.0.1-browser", supportsTransport: false, supportsViewState: true,
    events, params });
  const smoothingControl = document.getElementById("param-smoothing-control");
  const smoothingInput = document.getElementById("param-smoothing-ms");
  check(smoothingControl.hidden, "hosts without smoothing support hide its control");
  send({ supportsParamSmoothing: true, paramSmoothingMs: 50, paramSmoothingDefaultMs: 50 });
  check(!smoothingControl.hidden && smoothingInput.value === "50",
    "smoothing duration is exposed in the Params header with the host preference");
  send({ paramSmoothingMs: 30.0000001 });
  check(smoothingInput.value === "30", "smoothing displays whole milliseconds despite host float noise");
  send({ paramSmoothingMs: 9 });
  const smoothingWidth = smoothingControl.getBoundingClientRect().width;
  const smoothingInputWidth = smoothingInput.getBoundingClientRect().width;
  send({ paramSmoothingMs: 1000 });
  check(smoothingControl.getBoundingClientRect().width === smoothingWidth
    && smoothingInput.getBoundingClientRect().width === smoothingInputWidth,
    "smoothing control and number widths stay stable as the digit count changes");
  send({ paramSmoothingMs: 30 });
  shell.style.width = "480px";
  const header = document.querySelector("#params-section .params-toolbar");
  check(header.querySelector(".section-heading").lastElementChild === resetButton
    && resetButton.classList.contains("secondary"),
    "Reset is a button beside the Params title");
  check([...header.children].every(node => Math.abs(
    node.getBoundingClientRect().top + node.getBoundingClientRect().height / 2
    - (header.getBoundingClientRect().top + header.getBoundingClientRect().height / 2)
  ) < 1), "Params controls stay aligned in one row at the default webview width");
  check(getComputedStyle(smoothingInput).borderTopWidth === "0px"
    && getComputedStyle(smoothingControl).borderTopWidth === "1px",
    "Smooth, the value, and ms share one field outline");
  shell.style.width = "";
  edit(smoothingInput, "15.5");
  refresh();
  check(document.activeElement === smoothingInput && smoothingInput.value === "15.5",
    "background host updates retain the smoothing draft");
  const smoothingMessages = () => window.__testMessages.filter(message => message.type === "setParamSmoothing");
  smoothingInput.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  check(smoothingMessages().at(-1)?.milliseconds === 16 && smoothingMessages().length === 1
    && smoothingInput.value === "16",
    "Enter rounds smoothing to whole milliseconds and commits once");
  send({ paramSmoothingMs: 16 });
  edit(smoothingInput, "0");
  smoothingInput.blur();
  check(smoothingMessages().at(-1)?.milliseconds === 0,
    "blur commits zero milliseconds to disable smoothing");
  send({ paramSmoothingMs: 0 });
  for (const draft of ["-1", ""]) {
    edit(smoothingInput, draft);
    smoothingInput.blur();
    check(smoothingMessages().length === 2 && smoothingInput.value === "0",
      "invalid smoothing drafts restore the host value without being sent");
  }
  edit(smoothingInput, "50");
  smoothingInput.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  check(smoothingInput.value === "0" && smoothingMessages().length === 2,
    "Escape cancels the smoothing draft");
  const smoothPointer = pointerControl(smoothingControl);
  smoothPointer("pointerdown", 100);
  smoothPointer("pointermove", 120);
  check(smoothingInput.value === "20" && smoothingMessages().at(-1)?.milliseconds === 20,
    "dragging the Smooth or ms label updates its duration live");
  send({ paramSmoothingMs: 0 });
  check(smoothingInput.value === "20", "host refreshes retain a smoothing drag");
  send({ paramSmoothingMs: 20 });
  const messagesBeforeFineDrag = smoothingMessages().length;
  smoothPointer("pointermove", 124, { shiftKey: true });
  check(smoothingInput.value === "20" && smoothingMessages().length === messagesBeforeFineDrag,
    "fine smoothing drags accumulate without displaying or sending fractional milliseconds");
  smoothPointer("pointermove", 130, { shiftKey: true });
  smoothPointer("pointerup", 130, { shiftKey: true });
  check(smoothingInput.value === "21" && smoothingMessages().at(-1)?.milliseconds === 21
    && smoothingInput.readOnly,
    "Shift finely adjusts smoothing and releasing stays in drag mode");
  smoothPointer("pointerdown", 100);
  smoothPointer("pointermove", 0);
  smoothPointer("pointercancel", 0);
  check(smoothingInput.value === "0" && smoothingMessages().at(-1)?.milliseconds === 0,
    "smoothing drags clamp at zero and handle cancellation");
  edit(smoothingInput, "99");
  smoothingControl.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
  check(smoothingInput.value === "50" && smoothingMessages().at(-1)?.milliseconds === 50
    && smoothingInput.readOnly && document.activeElement !== smoothingInput,
    "double-click anywhere on smoothing restores the launch option and discards the text draft");
  smoothPointer("pointerdown", 100);
  smoothPointer("pointerup", 100);
  check(document.activeElement === smoothingInput && !smoothingInput.readOnly,
    "clicking the smoothing label selects its number for typing");
  smoothingInput.blur();
  const smoothNumberPointer = pointerControl(smoothingInput, smoothingControl);
  smoothNumberPointer("pointerdown", 100);
  smoothNumberPointer("pointermove", 110);
  smoothNumberPointer("pointerup", 110);
  check(smoothingInput.value === "60" && smoothingInput.readOnly,
    "the smoothing number shares the surrounding control's drag gesture");
  smoothingInput.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
  check(smoothingInput.value === "50" && smoothingInput.readOnly,
    "double-clicking the smoothing number still resets it");
  for (const milliseconds of [0, 12.5, 30]) {
    send({ paramSmoothingMs: 100, paramSmoothingDefaultMs: milliseconds });
    edit(smoothingInput, "99");
    smoothingControl.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    check(smoothingInput.value === String(Math.round(milliseconds))
      && smoothingMessages().at(-1)?.milliseconds === milliseconds && smoothingInput.readOnly,
      `smoothing reset preserves the exact ${milliseconds} ms launch option`);
    smoothingInput.blur();
    refresh();
    check(smoothingMessages().at(-1)?.milliseconds === milliseconds,
      "blur and host refresh do not round a fractional smoothing reset");
    const messagesBeforeFocus = smoothingMessages().length;
    smoothingInput.focus();
    check(smoothingInput.value === String(milliseconds),
      "focusing smoothing exposes the exact configured duration");
    smoothingInput.blur();
    check(smoothingMessages().length === messagesBeforeFocus,
      "focusing and blurring smoothing does not change its exact reset value");
  }
  shell.style.width = "320px";
  check([...document.querySelector("#params-section .params-toolbar").children].every(node =>
    node.getBoundingClientRect().right <= shell.getBoundingClientRect().right),
    "Params header controls wrap within a narrow run view");
  shell.style.width = "";
  check(document.getElementById("params").dataset.layout === "knobs"
    && document.querySelector('[data-param-layout="knobs"]').getAttribute("aria-pressed") === "true"
    && document.querySelector("#params .param-knob"),
    "the run view defaults to knobs without a saved layout preference");
  check(getComputedStyle(shell).visibility === "visible",
    "a first host state without view state reveals the run view");
  check(ondaVersion.textContent === "Onda 0.0.1-browser",
    "the run view accepts the browser host's Onda version state");
  check(getComputedStyle(resetButton).transitionDuration === "0.12s, 0.12s, 0.12s",
    "button transitions resume after the view is ready");
  check(document.getElementById("midi-keyboard").hidden,
    "the run view starts without a MIDI keyboard");
  paramsList.style.minHeight = `${window.innerHeight + 400}px`;
  scrollNode.scrollTop = 160;
  check(scrollNode.scrollTop > 0 && document.scrollingElement.scrollTop === 0,
    "the content scrolls when no MIDI keyboard is shown");
  scrollNode.scrollTop = 0;
  paramsList.style.minHeight = "";
  send({ midi: { noteOn: true }, params: Array.from({ length: 24 }, (_, index) => ({
    ...params[0], name: `gain${index}`,
  })) });
  await waitFrames(1);
  const midiKeyboard = document.getElementById("midi-keyboard");
  const contentScrollNode = [shell, document.scrollingElement].find(node =>
    node.scrollHeight > node.clientHeight);
  check(contentScrollNode && contentScrollNode.getBoundingClientRect().bottom
    <= midiKeyboard.getBoundingClientRect().top,
    `the run view scrollbar ends above the MIDI keyboard (${window.innerWidth} × ${window.innerHeight})`);
  const keyboardTop = midiKeyboard.getBoundingClientRect().top;
  scrollNode.scrollTop = scrollNode.scrollHeight;
  await waitFrames(1);
  check(scrollNode.scrollTop > 0 && document.scrollingElement.scrollTop === 0
    && midiKeyboard.getBoundingClientRect().top === keyboardTop
    && paramsList.lastElementChild.getBoundingClientRect().bottom <= keyboardTop,
    "scrolling reveals the last parameter while the MIDI keyboard stays in place");
  scrollNode.scrollTop = 0;
  send({ midi: { noteOn: false }, params });
  check(Math.abs(shell.getBoundingClientRect().bottom - window.innerHeight) < 1,
    "hiding the MIDI keyboard returns its space to the content");
  send({ supportsScope: true });
  for (const section of ["scope", "events", "params"]) {
    document.getElementById(`${section}-toggle`).click();
  }
  await waitFrames(1);
  check(["scope-section", "events-section", "params-section"].every(id =>
    document.getElementById(id).getBoundingClientRect().height < 80),
  "collapsed run sections stay at header height when the viewport has spare space");
  for (const section of ["scope", "events", "params"]) {
    document.getElementById(`${section}-toggle`).click();
  }
  send({ supportsScope: false });
  const midiVelocity = document.getElementById("midi-velocity");
  check(midiVelocity?.getAttribute("role") === "slider",
    "MIDI velocity uses the shared slider control");
  midiVelocity.focus();
  midiVelocity.dispatchEvent(new KeyboardEvent("keydown", {
    key: "ArrowLeft", bubbles: true, cancelable: true,
  }));
  check(document.querySelector("#midi-velocity-value").value === "0.99",
    "MIDI velocity slider supports precise keyboard adjustment");
  const numberEvents = [{ name: "numbers", args: [
    { name: "single", type: "f32", default: 0.25 },
    { name: "double", type: "f64", default: -2 },
    { name: "count", type: "i32", default: 3 },
    { name: "exact", type: "i64", default: "9007199254740993" },
    { name: "fixed", type: "f32[2]", arrayLength: 2, default: [1, 2] },
    { name: "slice", type: "i64[]", isSlice: true, default: ["9007199254740993"] },
  ] }];
  send({ events: numberEvents });
  const numberInputs = [...document.querySelectorAll('#events input[data-draggable]')];
  const [single, double, count, exact, firstElement, secondElement, sliceElement] = numberInputs;
  check(numberInputs.length === 7 && numberInputs.every(input => input.readOnly
    && getComputedStyle(input).cursor === "ew-resize"),
    "event scalars and array elements all use draggable number fields");
  const eventMessages = () => window.__testMessages.filter(message => message.type === "triggerEvent");
  const beforeEventDrag = eventMessages().length;
  for (const [input, expected] of [[single, "0.45"], [double, "-1.8"], [count, "8"],
    [exact, "9007199254740998"], [firstElement, "1.2"], [sliceElement, "9007199254740998"]]) {
    const pointer = pointerControl(input);
    pointer("pointerdown", 100);
    pointer("pointermove", 120);
    send({ events: numberEvents, logText: "during event drag" });
    pointer("pointerup", 120);
    check(input.value === expected && input.readOnly && document.contains(input),
      `${input.getAttribute("aria-label")} drags use the shared sensitivity and survive refreshes`);
  }
  check(eventMessages().length === beforeEventDrag,
    "editing event numbers does not trigger the event");
  document.querySelector(".event-trigger").click();
  check(JSON.stringify(eventMessages().at(-1)?.values)
    === JSON.stringify([0.45, -1.8, 8, "9007199254740998", [1.2, 2], ["9007199254740998"]]),
    "Trigger sends dragged scalars and array elements with exact i64 payloads");
  const exactPointer = pointerControl(exact);
  exactPointer("pointerdown", 100);
  exactPointer("pointermove", 140, { shiftKey: true });
  exactPointer("pointerup", 140, { shiftKey: true });
  check(exact.value === "9007199254740999", "i64 Shift drags preserve values above float precision");
  edit(exact, "9223372036854775806");
  exact.blur();
  exactPointer("pointerdown", 100);
  exactPointer("pointermove", 140);
  check(exact.value === "9223372036854775807", "i64 drags clamp exactly at the signed maximum");
  exactPointer("pointermove", 136);
  exactPointer("pointerup", 136);
  check(exact.value === "9223372036854775806", "i64 drags reverse immediately from the upper bound");
  edit(exact, "-9223372036854775807");
  exact.blur();
  exactPointer("pointerdown", 100);
  exactPointer("pointermove", 60);
  check(exact.value === "-9223372036854775808", "i64 drags clamp exactly at the signed minimum");
  exactPointer("pointermove", 64);
  exactPointer("pointerup", 64);
  check(exact.value === "-9223372036854775807", "i64 drags reverse immediately from the lower bound");
  for (const [initial, moves] of [
    ["9223372036854775806", [[5, "9223372036854775807"], [2, "9223372036854775806"]]],
    ["-9223372036854775807", [[-5, "-9223372036854775808"], [-2, "-9223372036854775807"]]],
    // Rounding onto a bound must preserve fractional motion toward the interior.
    ["9223372036854775806", [[3, "9223372036854775807"], [1, "9223372036854775806"]]],
    ["-9223372036854775807", [[-3, "-9223372036854775808"], [-1, "-9223372036854775807"]]],
  ]) {
    for (const shiftKey of [false, true]) {
      edit(exact, initial);
      exact.blur();
      exactPointer("pointerdown", 100);
      for (const [pixels, expected] of moves) {
        exactPointer("pointermove", 100 + pixels * (shiftKey ? 10 : 1), { shiftKey });
        check(exact.value === expected,
          `i64 ${initial} preserves inward motion and discards fractional overshoot at ${pixels} pixels, Shift=${shiftKey}`);
      }
      const [pixels, expected] = moves.at(-1);
      exactPointer("pointerup", 100 + pixels * (shiftKey ? 10 : 1), { shiftKey });
      document.querySelector(".event-trigger").click();
      check(exact.value === expected && eventMessages().at(-1)?.values[3] === expected,
        "i64 boundary reversal stays exact on release and in the event payload");
    }
  }
  edit(count, "2147483646");
  count.blur();
  const countPointer = pointerControl(count);
  countPointer("pointerdown", 100);
  countPointer("pointermove", 120);
  countPointer("pointerup", 120);
  check(count.value === "2147483647", "i32 event drags respect the type's bounds");
  for (const [input, expected] of [[single, "0.25"], [double, "-2"], [count, "3"],
    [exact, "9007199254740993"], [firstElement, "1"], [sliceElement, "9007199254740993"]]) {
    edit(input, input === exact || input === sliceElement ? "-" : "9");
    input.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    check(input.value === expected && input.readOnly && document.activeElement !== input,
      `${input.getAttribute("aria-label")} double-click resets its own declared default and clears drafts`);
  }
  check(secondElement.value === "2", "resetting an event array element retains the other elements");
  const singlePointer = pointerControl(single);
  singlePointer("pointerdown", 100);
  singlePointer("pointerup", 100);
  check(document.activeElement === single && !single.readOnly, "clicking an event number enters text editing");
  edit(single, "1.234");
  single.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  check(single.value === "0.25" && single.readOnly, "Escape restores the event value from before typing");
  edit(single, "1.234");
  single.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  check(single.value === "1.234" && single.readOnly, "Enter retains a typed event value and restores dragging");
  edit(exact, "9223372036854775808");
  check(document.querySelector(".event-trigger").disabled, "invalid i64 text disables Trigger");
  exact.blur();
  check(exact.value === "9007199254740993" && !document.querySelector(".event-trigger").disabled,
    "leaving invalid i64 text restores the last valid value and enables Trigger");
  edit(single, "");
  check(document.querySelector(".event-trigger").disabled, "empty event number drafts disable Trigger");
  single.blur();
  check(single.value === "1.234" && !document.querySelector(".event-trigger").disabled,
    "leaving an empty event number restores the last valid value");
  const sliceArg = sliceElement.closest(".event-array-arg");
  [...sliceArg.querySelectorAll("button")].find(button => button.textContent === "+").click();
  const addedElement = document.querySelector('input[aria-label="slice [1]"]');
  check(addedElement?.readOnly && addedElement.value === "0", "new slice elements also use the shared number widget");
  const addedPointer = pointerControl(addedElement);
  addedPointer("pointerdown", 100);
  addedPointer("pointermove", 112);
  addedPointer("pointerup", 112);
  addedElement.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  check(addedElement.value === "0", "new slice elements without a declared default reset to zero");
  send({ connected: false, events: numberEvents });
  const disabledEvent = document.querySelector('#events input');
  const disabledPointer = pointerControl(disabledEvent);
  const disabledValue = disabledEvent.value;
  disabledPointer("pointerdown", 100);
  disabledPointer("pointermove", 120);
  disabledPointer("pointerup", 120);
  disabledEvent.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  check(disabledEvent.disabled && disabledEvent.value === disabledValue,
    "disabled event number fields ignore dragging and reset gestures");
  const integerEvents = [{ name: "integers", args: [
    { name: "scalar", type: "i32", default: 3 },
    { name: "elements", type: "i32[2]", arrayLength: 2, default: [3, 7] },
  ] }];
  send({ connected: true, events: integerEvents });
  const integerInputs = [...document.querySelectorAll('#events input[data-draggable]')];
  for (const [index, input] of integerInputs.slice(0, 2).entries()) {
    const pointer = pointerControl(input);
    const sentInteger = () => {
      document.querySelector(".event-trigger").click();
      const values = eventMessages().at(-1)?.values;
      return index === 0 ? values?.[0] : values?.[1]?.[0];
    };
    for (const shiftKey of [false, true]) {
      input.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
      pointer("pointerdown", 100);
      const steps = shiftKey ? [[3, 3], [19, 3], [20, 4], [19, 3]]
        : [[3, 4], [4, 4], [5, 4], [6, 5], [5, 4]];
      for (const [pixels, expected] of steps) {
        pointer("pointermove", 100 + pixels, { shiftKey });
        check(input.value === String(expected) && sentInteger() === expected,
          `${input.getAttribute("aria-label")} ${shiftKey ? "fine" : "normal"} drags display the integer they send at ${pixels} pixels`);
      }
      const [pixels, expected] = steps.at(-1);
      pointer("pointerup", 100 + pixels, { shiftKey });
      check(input.readOnly && input.value === String(expected) && sentInteger() === expected,
        `${input.getAttribute("aria-label")} release keeps its canonical integer`);
      pointer("pointerdown", 100);
      pointer("pointermove", 103);
      pointer("pointerup", 103);
      check(input.value === String(expected + 1) && sentInteger() === expected + 1,
        `${input.getAttribute("aria-label")} later drags start from the displayed integer`);
    }
    check(integerInputs[2].value === "7", "integer element drags retain neighboring elements");
  }
  const preciseEvents = [{ name: "precise", args: [
    { name: "scalar", type: "f64", default: 0.123456789 },
    { name: "elements", type: "f64[1]", arrayLength: 1, default: [0.0000123456789] },
  ] }];
  send({ connected: true, events: preciseEvents });
  const preciseInputs = [...document.querySelectorAll('#events input[data-draggable]')];
  const preciseDefaults = [preciseEvents[0].args[0].default, preciseEvents[0].args[1].default[0]];
  const triggerPrecise = () => {
    document.querySelector(".event-trigger").click();
    const values = eventMessages().at(-1)?.values;
    return [values?.[0], values?.[1]?.[0]];
  };
  for (const [index, input] of preciseInputs.entries()) {
    const pointer = pointerControl(input);
    check(input.value === ["0.1235", "0"][index]
      && triggerPrecise()[index] === preciseDefaults[index],
      `${input.getAttribute("aria-label")} displays compact decimals while retaining its exact default`);
    pointer("pointerdown", 100);
    pointer("pointermove", 104, { shiftKey: true });
    check(input.value === ["0.1275", "0.004"][index],
      `${input.getAttribute("aria-label")} fine drags display compact decimals`);
    pointer("pointerup", 104, { shiftKey: true });
    const firstTarget = preciseDefaults[index] + 4 * 0.01 * 0.1;
    check(triggerPrecise()[index] === firstTarget,
      `${input.getAttribute("aria-label")} fine drags preserve full event precision`);
    pointer("pointerdown", 100);
    pointer("pointermove", 104, { shiftKey: true });
    send({ events: preciseEvents, logText: "precise event drag" });
    pointer("pointerup", 104, { shiftKey: true });
    check(input.value === ["0.1315", "0.008"][index]
      && triggerPrecise()[index] === firstTarget + 4 * 0.01 * 0.1,
      `${input.getAttribute("aria-label")} later drags start from the exact event value`);
    input.focus();
    check(input.value === ["0.13146", "0.00801"][index],
      `${input.getAttribute("aria-label")} typing starts with at most five decimal places`);
    edit(input, "9");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    check(input.value === ["0.1315", "0.008"][index]
      && triggerPrecise()[index] === firstTarget + 4 * 0.01 * 0.1,
      `${input.getAttribute("aria-label")} Escape restores the exact value behind the compact display`);
    pointer("pointerdown", 100);
    pointer("pointermove", 103.7);
    pointer("pointermove", 107.2);
    pointer("pointerup", 107.2);
    check(input.value === ["0.2035", "0.08"][index]
      && Math.abs(triggerPrecise()[index] - (firstTarget + 0.004 + 0.072)) < 1e-15,
      `${input.getAttribute("aria-label")} fractional pointer movement stays compact without rounding its payload`);
    edit(input, "9");
    input.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    check(triggerPrecise()[index] === preciseDefaults[index],
      `${input.getAttribute("aria-label")} resets restore the exact floating default`);
    input.focus();
    check(input.value === ["0.12346", "1.23457e-5"][index],
      `${input.getAttribute("aria-label")} focusing shortens ordinary and tiny values`);
    input.blur();
    check(triggerPrecise()[index] === preciseDefaults[index],
      `${input.getAttribute("aria-label")} focus and blur preserve event precision`);
    const typed = preciseDefaults[index] + 0.000000001;
    edit(input, String(typed));
    check(document.activeElement === input && !input.validity.customError
      && !document.querySelector(".event-trigger").disabled,
      `${input.getAttribute("aria-label")} accepts fractional event drafts outside its keyboard increment grid`);
    const messagesBeforeTypingTrigger = eventMessages().length;
    check(triggerPrecise()[index] === typed
      && eventMessages().length === messagesBeforeTypingTrigger + 1,
      `${input.getAttribute("aria-label")} sends its exact typed value without requiring blur`);
    input.blur();
    check(input.value === ["0.1235", "0"][index] && triggerPrecise()[index] === typed,
      `${input.getAttribute("aria-label")} leaving a precise draft restores the compact display and retains the typed value`);
  }
  preciseInputs[0].focus();
  const preciseValues = triggerPrecise();
  const preciseView = window.__testMessages.findLast(message => message.type === "viewState")?.state;
  check(preciseView.events[0].drafts[0][0] === null,
    "saving a focused shortened event draft retains only its exact stored value");
  send({ events: [] });
  send({ events: preciseEvents, viewState: preciseView });
  await waitFrames(4);
  check([...document.querySelectorAll('#events input[data-draggable]')]
    .every((input, index) => input.value === ["0.1235", "0"][index])
    && JSON.stringify(triggerPrecise()) === JSON.stringify(preciseValues),
    "restoring event fields keeps their compact displays and exact payloads");
  send({ connected: true, events });
  const scalar = document.querySelector("#events input");
  edit(scalar, "0.75");
  for (let i = 0; i < 20; ++i) refresh();
  check(document.querySelector("#events input") === scalar,
    "event controls survive metadata and log refreshes");
  check(document.activeElement === scalar && scalar.value === "0.75",
    "event focus and edited value survive refreshes");
  document.querySelector(".event-trigger").click();
  check(window.__testMessages.at(-1).values[0] === 0.75,
    "retained event handlers dispatch the edited arguments");

  const integer = document.querySelector('#events input[type="text"]');
  edit(integer, "-");
  integer.setSelectionRange(1, 1);
  refresh();
  check(document.activeElement === integer && integer.value === "-"
    && integer.selectionStart === 1,
    "incomplete i64 drafts and caret survive refreshes");
  send({ events, resetEventArguments: true });
  check(document.querySelector('#events input[type="text"]').value === "0",
    "explicit reset discards a draft even when its canonical value is unchanged");
  check(document.querySelector("#events input").value === "0.75",
    "reset flag alone does not invent replacement values");
  send({ events: events.map(event => ({ ...event,
    args: event.args.map(arg => ({ ...arg, value: arg.default })),
  })), resetEventArguments: true });
  check(document.querySelector("#events input").value === "0.25",
    "new program or explicit argument reset restores defaults");
  check(window.__testMessages.findLast(message => message.type === "viewState")
    ?.state.events[0].values[0] === 0.25,
    "argument resets replace the retained view snapshot");

  document.querySelector('[aria-label="Add element"]').click();
  let arrayInput = document.querySelector(".event-array-element input");
  edit(arrayInput, "0.625");
  refresh();
  check(document.querySelector(".event-array-element input") === arrayInput
    && arrayInput.value === "0.625", "slice edits survive refreshes");
  document.querySelector(".event-trigger").click();
  check(window.__testMessages.at(-1).values[2][0] === 0.625,
    "slice handlers retain current values");
  document.querySelector('[aria-label="Remove last element"]').click();
  check(!document.querySelector(".event-array-element input"),
    "slice shape changes rebuild the necessary controls");

  for (let index = 0; index < params.length; ++index) {
    const input = document.querySelectorAll('#params input[type="number"]')[index];
    edit(input, "0.83");
    refresh();
    check(input.value === "0.83" && document.activeElement === input,
      `numeric draft ${index} survives a host refresh`);
    input.dispatchEvent(new Event("change", { bubbles: true }));
    input.blur();
    const command = window.__testMessages.at(-1);
    check(command.type === "setParam" && command.name === params[index].name
      && Math.abs(command.value - 0.83) < 0.001 && command.commit === true,
      `numeric draft ${index} commits its edited value`);
    input.focus();
    params[index].value = 0.6;
    refresh();
    check(Math.abs(Number(input.value) - 0.83) < 0.001,
      `focused field ${index} is not overwritten by automation`);
    input.blur();
    check(Math.abs(Number(input.value) - 0.6) < 0.001,
      `field ${index} reconciles host state when focus leaves without an edit`);
  }

  const paramMessages = () => window.__testMessages.filter(message => message.type === "setParam");
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const type of ["f64", "i64"]) {
      for (const ranged of [false, true]) {
        for (const drag of [false, true]) {
          const param = { name: "latest", type, value: 20, default: 20,
            ...(ranged ? { rangeMin: 0, rangeMax: 100, scale: "linear",
              ...(type === "i64" ? { step: 1, stepCount: 100 } : {}) } : {}) };
          send({ params: [param] });
          const number = document.querySelector('#params input');
          const pointer = pointerControl(number);
          const before = paramMessages().length;
          pointer("pointerdown", 100);
          for (const [value, pixels] of [[60, 0], [70, 2], [80, 2]]) {
            send({ params: [{ ...param, value }] });
            pointer("pointermove", 100 + pixels);
            check(Number(number.value) === value && paramMessages().length === before,
              `${layout} ${type} ranged=${ranged} presses retain host updates before dragging`);
          }
          if (drag) {
            pointer("pointermove", 115);
            pointer("pointerup", 115);
            const expected = ranged || type === "i64" ? 84 : 80.15;
            const committed = paramMessages().at(-1);
            check(committed?.value === expected && committed.commit === true,
              `${layout} ${type} ranged=${ranged} drags start from the latest host value`);
          } else {
            pointer("pointerup", 102);
            number.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
            check(Number(number.value) === 80 && paramMessages().length === before,
              `${layout} ${type} ranged=${ranged} clicks preserve the latest host value`);
          }
        }
      }
    }
  }
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const type of ["f32", "f64"]) {
      for (const [initial, text] of [[1023.095538565032, "1023.09554"], [8, "8"], [-0, "0"],
        [0.0000123456789, "1.23457e-5"], [-1.23456789e-8, "-1.23457e-8"],
        [1.23456789e20, "1.23457e20"], [Number.MIN_VALUE, "4.94066e-324"]]) {
        send({ params: [{ name: "precision", type, value: initial, default: initial }] });
        const number = document.querySelector('#params input');
        const before = paramMessages().length;
        const pointer = pointerControl(number);
        pointer("pointerdown", 100);
        pointer("pointerup", 100);
        check(number.value === text && !number.readOnly,
          `${layout} ${type} click-to-edit formats ${initial} as ${text}`);
        number.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
        number.focus();
        number.blur();
        check(paramMessages().length === before && number.readOnly,
          `${layout} ${type} opening and closing a shortened draft preserves the exact value`);
        number.focus();
        send({ params: [{ name: "precision", type, value: 43.123456789, default: initial }] });
        check(number.value === text, `${layout} ${type} host refresh preserves the shortened draft`);
        number.blur();
        number.focus();
        check(number.value === "43.12346" && paramMessages().length === before,
          `${layout} ${type} an untouched draft reconciles the exact incoming host value`);
        number.blur();
        edit(number, "1023.123456789");
        number.dispatchEvent(new Event("change", { bubbles: true }));
        number.blur();
        check(paramMessages().at(-1)?.value === 1023.123456789,
          `${layout} ${type} accepts typed precision beyond five decimal places`);
      }
    }
  }

  document.querySelector('[data-param-layout="knobs"]').click();
  for (const [scale, curve, step] of [["log", null, null], ["linear", -4, null],
    ["linear", 4, null], ["linear", null, 10]]) {
    send({ params: [{ name: "fineKnob", type: "f64", value: 80, default: 80,
      rangeMin: 80, rangeMax: 12000, scale, curve, step, stepCount: step === null ? null : 1192 }] });
    const knob = document.querySelector('#params .param-knob');
    const pointer = pointerControl(knob);
    pointer("pointerdown", 100, { clientY: 100 });
    for (const [pixels, shiftKey, normalized] of [[25, true, 0.01], [50, false, 0.11],
      [50, true, 0.11], [75, true, 0.12], [3000, false, 1], [2997.5, true, 0.999],
      [-3000, false, 0], [-2997.5, true, 0.001]]) {
      pointer("pointermove", 100, { clientY: 100 - pixels, shiftKey });
      let expected = normalized === 0 ? 80 : normalized === 1 ? 12000
        : scale === "log" ? 80 * Math.pow(150, normalized)
        : curve === null ? 80 + 11920 * normalized
        : 80 + 11920 * Math.expm1(curve * normalized) / Math.expm1(curve);
      if (step !== null) expected = 80 + Math.round((expected - 80) / step) * step;
      const actual = Number(knob.getAttribute("aria-valuenow"));
      check(Math.abs(actual / expected - 1) < 1e-12,
        `${scale} curve=${curve} step=${step} knob applies Shift=${shiftKey} per movement at ${pixels} pixels`);
    }
    const beforeRelease = Number(knob.getAttribute("aria-valuenow"));
    pointer("pointerup", 100, { clientY: 3097.5, shiftKey: true });
    check(paramMessages().at(-1)?.commit === true && paramMessages().at(-1).value === beforeRelease,
      `${scale} curve=${curve} knob release preserves the fine drag value`);
  }

  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const shiftKey of [false, true]) {
      const initial = 0.0020037;
      send({ params: [{ name: "tiny", type: "f64", value: initial, default: initial,
        rangeMin: 0, rangeMax: 0.008, scale: "linear" }] });
      const number = document.querySelector('#params input');
      const pointer = pointerControl(number);
      pointer("pointerdown", 100);
      pointer("pointermove", 120, { shiftKey });
      await waitFrames(1);
      const expected = initial + 20 * 0.008 / 375 * (shiftKey ? 0.1 : 1);
      const live = paramMessages().at(-1);
      pointer("pointerup", 120, { shiftKey });
      check(Math.abs(live.value - expected) < 1e-15
        && live.commit === false && paramMessages().at(-1).value === live.value,
        `${layout} ${shiftKey ? "fine" : "normal"} number drags preserve canonical precision on start and release`);
    }
  }

  // Match the egui poly_saw cutoff regression: equal movement gives equal ratios.
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const type of ["f32", "f64"]) {
      for (const initial of [80, 800, 4000]) {
        for (const shiftKey of [false, true]) {
          const cutoff = { name: "cutoff", type, value: initial, default: initial,
            rangeMin: 80, rangeMax: 12000, scale: "log", unit: "Hz" };
          send({ params: [cutoff] });
          const number = document.querySelector('#params input');
          const pointer = pointerControl(number);
          const distance = shiftKey ? 10 : 1;
          pointer("pointerdown", 100);
          for (const pixels of [37.5, 75]) {
            pointer("pointermove", 100 + pixels * distance, { shiftKey });
            await waitFrames(1);
            const expected = initial * Math.pow(150, pixels / 375);
            const live = paramMessages().at(-1);
            const rangeValue = Number(number.closest(".param").querySelector('[role="slider"]')
              .getAttribute("aria-valuenow"));
            check(live.commit === false && Math.abs(live.value / expected - 1) < 1e-12
              && rangeValue === live.value,
              `${layout} ${type} log drags follow equal ratios from ${initial} Hz at ${pixels} pixels, Shift=${shiftKey}`);
            send({ params: [cutoff] });
            check(document.querySelector('#params input') === number
              && Number(number.closest(".param").querySelector('[role="slider"]')
                .getAttribute("aria-valuenow")) === live.value,
              `${layout} log drags retain their canonical value through host refreshes`);
          }
          pointer("pointerup", 100 + 75 * distance, { shiftKey });
          const expected = initial * Math.pow(150, 0.2);
          check(paramMessages().at(-1)?.commit === true
            && Math.abs(paramMessages().at(-1).value / expected - 1) < 1e-12,
            `${layout} ${type} log drags preserve their exact value on release`);
        }
      }
    }
  }

  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const [scale, curve] of [["log", null], ["linear", -4], ["linear", 4]]) {
      send({ params: [{ name: "scaled", type: "f64", value: 80, default: 80,
        rangeMin: 80, rangeMax: 12000, scale, curve }] });
      const number = document.querySelector('#params input');
      const pointer = pointerControl(number);
      const range = number.closest(".param").querySelector('[role="slider"]');
      pointer("pointerdown", 100);
      for (const [pixels, normalized] of [[187.5, 0.5], [375, 1], [562.5, 1],
        [525, 0.9], [-150, 0], [-112.5, 0.1]]) {
        pointer("pointermove", 100 + pixels);
        const actual = Number(range.getAttribute("aria-valuenow"));
        const expected = normalized === 0 ? 80 : normalized === 1 ? 12000
          : scale === "log" ? 80 * Math.pow(150, normalized)
          : 80 + 11920 * Math.expm1(curve * normalized) / Math.expm1(curve);
        check(Math.abs(actual / expected - 1) < 1e-12,
          `${layout} ${scale} curve=${curve} number drags follow the declared mapping and reverse at bounds (${pixels} pixels)`);
      }
      pointer("pointerup", -12.5);
      check(paramMessages().at(-1)?.value === Number(range.getAttribute("aria-valuenow")),
        `${layout} scaled number drag release preserves the range control value`);
      edit(number, "440");
      number.dispatchEvent(new Event("change", { bubbles: true }));
      number.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      check(paramMessages().at(-1)?.value === 440 && number.readOnly,
        `${layout} scaled numbers accept typed values in plain units`);
      number.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
      check(paramMessages().at(-1)?.value === 80,
        `${layout} scaled number resets restore the plain default`);
    }
  }

  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const type of ["f32", "f64"]) {
      for (const shiftKey of [false, true]) {
        const initial = 0.0020037;
        send({ params: [{ name: "unbounded", type, value: initial, default: initial }] });
        const number = document.querySelector('#params input');
        const pointer = pointerControl(number);
        pointer("pointerdown", 100);
        pointer("pointermove", 104, { shiftKey });
        await waitFrames(1);
        const delta = 4 * 0.01 * (shiftKey ? 0.1 : 1);
        const expected = initial + delta;
        const live = paramMessages().at(-1);
        pointer("pointerup", 104, { shiftKey });
        check(Math.abs(live.value - expected) < 1e-15 && live.commit === false
          && paramMessages().at(-1)?.value === live.value,
          `${layout} ${type} unbounded ${shiftKey ? "fine" : "normal"} drags preserve precision`);
        pointer("pointerdown", 100);
        pointer("pointermove", 104, { shiftKey });
        pointer("pointerup", 104, { shiftKey });
        check(Math.abs(paramMessages().at(-1)?.value - (expected + delta)) < 1e-15,
          `${layout} ${type} later unbounded drags retain the exact starting value`);
        number.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
        check(paramMessages().at(-1)?.value === initial && number.readOnly,
          `${layout} ${type} unbounded resets restore the exact declared default`);
        const beforeFocus = paramMessages().length;
        number.focus();
        number.blur();
        check(paramMessages().length === beforeFocus,
          `${layout} ${type} focus and blur preserve unbounded reset precision`);
      }
    }
  }

  for (const layout of ["knobs", "sliders"]) {
    send({ params });
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    const [gain, offset] = document.querySelectorAll('#params input[type="number"]');
    const gainPointer = pointerControl(gain);
    const before = paramMessages().length;
    check(gain.readOnly && getComputedStyle(gain).cursor === "ew-resize",
      `${layout} numeric fields advertise dragging`);
    gainPointer("pointerdown", 100, { button: 2 });
    gainPointer("pointermove", 120);
    gainPointer("pointerup", 120);
    check(paramMessages().length === before, `${layout} ignores right-button drags`);
    gainPointer("pointerdown", 100);
    gainPointer("pointermove", 120, { pointerId: 2 });
    check(paramMessages().length === before, `${layout} ignores unrelated pointers`);
    gainPointer("pointermove", 120);
    await waitFrames(1);
    const expectedGain = 0.6 + 20 / 375;
    check(Math.abs(Number(gain.value) - expectedGain) < 0.0005
      && paramMessages().at(-1)?.commit === false,
      `${layout} number drags use 375 pixels per range and send live targets`);
    refresh();
    check(Math.abs(Number(gain.value) - expectedGain) < 0.0005
      && document.querySelector('#params input') === gain,
      `${layout} number drags survive host updates`);
    gainPointer("pointermove", 128, { shiftKey: true });
    gainPointer("pointerup", 128, { shiftKey: true });
    const committed = paramMessages().at(-1);
    const expectedFineGain = expectedGain + 0.8 / 375;
    check(Math.abs(Number(gain.value) - expectedFineGain) < 0.0005
      && Math.abs(committed.value - expectedFineGain) < 1e-12 && committed.commit === true
      && !gain.hasPointerCapture(1) && gain.readOnly,
      `${layout} Shift drags provide fine control and commit on release`);
    const afterCommit = paramMessages().length;
    await waitFrames(1);
    check(paramMessages().length === afterCommit,
      `${layout} release discards the pending live message`);
    gainPointer("pointerdown", 100);
    gainPointer("pointermove", 10000);
    check(gain.value === "1" && gain.closest(".param").querySelector('[role="slider"]')
      .getAttribute("aria-valuenow") === "1", `${layout} number drags clamp and synchronize the range control`);
    gainPointer("pointermove", 9990);
    check(Math.abs(Number(gain.value) - (1 - 10 / 375)) < 0.0005,
      `${layout} reverses immediately after reaching a bound`);
    gainPointer("lostpointercapture", 9990);
    check(paramMessages().at(-1)?.commit === true,
      `${layout} losing capture commits the last value`);
    const offsetPointer = pointerControl(offset);
    offsetPointer("pointerdown", 100);
    offsetPointer("pointermove", 0);
    offsetPointer("pointercancel", 0);
    check(offset.value === "-0.4" && paramMessages().at(-1)?.name === "offset"
      && paramMessages().at(-1)?.commit === true,
      `${layout} unbounded number drags support negatives and cancellation`);
    const beforeClick = paramMessages().length;
    gainPointer("pointerdown", 100);
    gainPointer("pointerup", 101);
    check(document.activeElement === gain && !gain.readOnly
      && paramMessages().length === beforeClick,
      `${layout} clicks enter text editing without changing the value`);
    gainPointer("pointerdown", 100);
    gainPointer("pointermove", 130);
    gainPointer("pointerup", 130);
    check(paramMessages().length === beforeClick,
      `${layout} text editing leaves pointer selection alone`);
    edit(gain, "0.75");
    gain.dispatchEvent(new Event("change", { bubbles: true }));
    gain.blur();
    check(gain.readOnly && paramMessages().at(-1)?.value === 0.75,
      `${layout} typed values commit and blur restores dragging`);
    edit(gain, "0.99");
    gain.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    check(gain.value === "0.25" && gain.readOnly && document.activeElement !== gain
      && paramMessages().at(-1)?.value === 0.25 && paramMessages().at(-1)?.commit === true
      && gain.closest(".param").querySelector('[role="slider"]')
        .getAttribute("aria-valuenow") === "0.25",
      `${layout} double-click restores the declared default, discards drafts, and synchronizes the range control`);
    offset.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    check(offset.value === "2" && paramMessages().at(-1)?.name === "offset"
      && paramMessages().at(-1)?.value === 2,
      `${layout} double-click restores unbounded parameter defaults`);
    gainPointer("pointerdown", 100);
    gainPointer("pointermove", 120);
    gain.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    const afterReset = paramMessages().length;
    gainPointer("pointerup", 120);
    await waitFrames(1);
    check(gain.value === "0.25" && paramMessages().length === afterReset,
      `${layout} double-click cancels pending drag updates so they cannot overwrite the reset`);
  }
  for (const type of ["f32", "f64", "i32", "i64"]) {
    const ranged = [{ name: "ranged", type, value: 0, default: 0,
      rangeMin: 0, rangeMax: 160, scale: "linear",
      step: type.startsWith("i") ? 1 : null, stepCount: type.startsWith("i") ? 160 : null }];
    send({ params: ranged });
    document.querySelector('[data-param-layout="knobs"]').click();
    const number = document.querySelector('#params input');
    const numberPointer = pointerControl(number);
    numberPointer("pointerdown", 100);
    numberPointer("pointermove", 193.75);
    numberPointer("pointerup", 193.75);
    check(number.value === "40" && paramMessages().at(-1)?.value === 40,
      `${type} number drags cover one quarter of the range in 93.75 pixels`);
    send({ params: ranged });
    const knobPointer = pointerControl(document.querySelector('#params .param-knob'));
    knobPointer("pointerdown", 100, { clientY: 250 });
    knobPointer("pointermove", 100, { clientY: 187.5 });
    knobPointer("pointerup", 100, { clientY: 187.5 });
    check(number.value === "40" && paramMessages().at(-1)?.value === 40,
      `${type} knob drags cover one quarter of the range in 62.5 pixels`);
    knobPointer("pointerdown", 100, { clientY: 250 });
    knobPointer("pointermove", 100, { clientY: 0 });
    knobPointer("pointerup", 100, { clientY: 0 });
    check(number.value === "160" && paramMessages().at(-1)?.value === 160,
      `${type} knob drags cover the full range in 250 pixels`);
    knobPointer("pointerdown", 100, { clientY: 250 });
    knobPointer("pointermove", 100, { clientY: -750 });
    knobPointer("pointermove", 100, { clientY: -740 });
    knobPointer("pointerup", 100, { clientY: -740 });
    check(Number(number.value) === (type.startsWith("i") ? 154 : 153.6),
      `${type} knob drags reverse immediately after overshooting the upper bound`);
  }
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const maximum of [1e308, Number.MAX_VALUE]) {
      const wide = [{ name: "wide", type: "f64", value: 0, default: 0,
        rangeMin: -maximum, rangeMax: maximum, scale: "linear" }];
      for (const direction of [-1, 1]) {
        send({ params: wide });
        const number = document.querySelector('#params input');
        const pointer = pointerControl(number);
        pointer("pointerdown", 100);
        pointer("pointermove", 100 + direction * 187.5);
        pointer("pointerup", 100 + direction * 187.5);
        const committed = paramMessages().at(-1);
        check(committed?.commit === true
          && Math.abs(committed.value / maximum - direction) < 1e-15,
          `${layout} number drags cover wide finite ranges without overflowing their span`);
      }
    }
  }
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    for (const [minimum, maximum] of [
      [0, 1e-321], [-1e-321, 1e-321], [0, Number.MIN_VALUE],
      [-1e308, 1], [1e16, 1e16 + 100],
    ]) {
      for (const shiftKey of [false, true]) {
        for (const incremental of [false, true]) {
          send({ params: [{ name: "extreme", type: "f64", value: minimum, default: minimum,
            rangeMin: minimum, rangeMax: maximum, scale: "linear" }] });
          const number = document.querySelector('#params input');
          const pointer = pointerControl(number);
          const distance = shiftKey ? 10 : 1;
          pointer("pointerdown", 100);
          const moves = incremental
            ? Array.from({ length: 745 }, (_, index) => (index + 6) / 2) : [187.5, 375];
          for (const pixels of [...moves, 750, 562.5]) {
            pointer("pointermove", 100 + pixels * distance, { shiftKey });
            const value = Number(number.closest(".param").querySelector('[role="slider"]')
              .getAttribute("aria-valuenow"));
            if (pixels === 187.5 || pixels === 562.5) {
              check(value === minimum + (maximum - minimum) * 0.5,
                `${layout} extreme-range drags reach the midpoint and reverse after overshooting`);
            } else if (pixels === 375 || pixels === 750) {
              check(value === maximum,
                `${layout} extreme-range ${shiftKey ? "fine" : "normal"} ${incremental ? "incremental" : "single"} drags reach the endpoint`);
            }
          }
          pointer("pointerup", 100 + 562.5 * distance, { shiftKey });
          check(paramMessages().at(-1)?.value === minimum + (maximum - minimum) * 0.5,
            `${layout} extreme-range drag release preserves its canonical value`);
        }
      }
    }
  }
  document.querySelector('[data-param-layout="sliders"]').click();
  const stepped = [
    { name: "mode", type: "i32", value: 0, default: 0,
      rangeMin: 0, rangeMax: 10, scale: "linear", step: 2, stepCount: 5 },
    { name: "pitch", type: "f64", value: 100, default: 100,
      rangeMin: 20, rangeMax: 1000, scale: "log" },
  ];
  send({ params: stepped });
  const [mode, pitch] = document.querySelectorAll('#params input[type="number"]');
  const modePointer = pointerControl(mode);
  modePointer("pointerdown", 100);
  for (const [pixels, expected] of [[18.75, "0"], [37, "0"], [37.5, "2"], [37, "0"], [37.5, "2"]]) {
    modePointer("pointermove", 100 + pixels);
    check(mode.value === expected,
      `integer number drags snap once at the declared step midpoint after ${pixels} pixels`);
  }
  modePointer("pointerup", 137.5);
  check(mode.value === "2" && paramMessages().at(-1)?.value === 2,
    "integer number drags respect the declared step grid");
  mode.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
  check(mode.value === "0" && paramMessages().at(-1)?.value === 0,
    "integer double-click reset respects the default on its step grid");
  const pitchPointer = pointerControl(pitch);
  pitchPointer("pointerdown", 100);
  pitchPointer("pointermove", 120);
  const expectedPitch = 100 * Math.pow(50, 20 / 375);
  check(Math.abs(Number(pitch.closest(".param").querySelector('[role="slider"]')
    .getAttribute("aria-valuenow")) / expectedPitch - 1) < 1e-12
    && Math.abs(Number(pitch.value) - expectedPitch) < 0.05,
    "logarithmic parameter number drags follow the log domain and synchronize the range control");
  document.querySelector('[data-param-layout="knobs"]').click();
  check(paramMessages().at(-1)?.name === "pitch" && paramMessages().at(-1)?.commit === true,
    "changing layout during a number drag commits and releases the old gesture");
  const afterRebuild = paramMessages().length;
  pitchPointer("pointermove", 200);
  pitchPointer("pointerup", 200);
  check(paramMessages().length === afterRebuild, "removed number controls stop sending drag updates");
  const obsolete = { name: "obsolete", type: "f64", value: 0.2, default: 0.2,
    rangeMin: 0, rangeMax: 1, scale: "linear" };
  for (const [description, nextState] of [
    ["removal", { params: [] }],
    ["range change", { params: [{ ...obsolete, rangeMax: 100, value: 50, default: 50 }] }],
    ["type change", { params: [{ name: "obsolete", type: "bool", value: false, default: false }] }],
    ["scale change", { params: [{ ...obsolete, rangeMin: 0.01, scale: "log" }] }],
    ["program change", { path: "replacement.onda", params: [{ ...obsolete, value: 0.7 }] }],
  ]) {
    send({ path: "test.onda", params: [obsolete] });
    const number = document.querySelector('#params input');
    const pointer = pointerControl(number);
    pointer("pointerdown", 100);
    pointer("pointermove", 137.5);
    const beforeReplacement = paramMessages().length;
    send(nextState);
    pointer("pointermove", 175);
    pointer("pointerup", 175);
    await waitFrames(1);
    check(paramMessages().length === beforeReplacement && !number.hasPointerCapture(1),
      `${description} discards obsolete number drags and pending live updates without committing`);
    const replacement = document.querySelector('#params input');
    if (replacement && replacement.type === "number") {
      check(Number(replacement.value) === nextState.params[0].value,
        `${description} preserves the replacement parameter value`);
    }
  }
  send({ path: "test.onda" });
  send({ params: [{ name: "missingDefault", type: "f32", value: 5,
    rangeMin: 2, rangeMax: 10, scale: "linear" }] });
  document.querySelector('#params input').dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  check(paramMessages().at(-1)?.value === 2,
    "double-click without a declared default uses the range minimum");
  send({ path: "", params });
  const beforeDisabledReset = paramMessages().length;
  document.querySelector('#params input').dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  check(paramMessages().length === beforeDisabledReset, "disabled number controls ignore double-click reset");
  send({ path: "test.onda", params });

  const paramsNode = document.getElementById("params");
  const longName = "a_boolean_parameter_name_that_exceeds_the_card_width";
  send({ params: [
    ...["autoplay", "limiter", longName].map(name =>
      ({ name, type: "bool", value: false, default: false })),
    params[0],
    { name: "enabled[0]", type: "bool", value: false, default: false,
      array: { name: "enabled", length: 1, index: 0 } },
  ] });
  for (const width of [408, 476]) {
    paramsNode.style.width = `${width}px`;
    for (const layout of ["sliders", "knobs"]) {
      document.querySelector(`[data-param-layout="${layout}"]`).click();
      await waitFrames(1);
      const boolCards = [...paramsNode.querySelectorAll(".param-bool")];
      for (const card of boolCards) {
        const name = card.querySelector(".param-name");
        const type = card.querySelector(".param-type");
        const head = card.querySelector(".param-head");
        check(name.textContent === longName
          ? name.scrollWidth > name.clientWidth
          : name.scrollWidth <= name.clientWidth,
        `${layout} at ${width}px truncates ${name.textContent} only when it exceeds the available width`);
        check(type.getBoundingClientRect().right <= head.getBoundingClientRect().right + 1,
          `${layout} at ${width}px keeps ${name.textContent}'s type inside its heading`);
      }
      const number = paramsNode.querySelector('input[type="number"]');
      const numericCard = number.closest(".param");
      check(number.clientWidth > 0
        && number.getBoundingClientRect().right <= numericCard.getBoundingClientRect().right
        && (layout === "knobs" || numericCard.querySelector(".param-type")
          .getBoundingClientRect().right <= number.getBoundingClientRect().left + 1),
      `${layout} at ${width}px preserves the numeric value layout`);
    }
  }
  paramsNode.style.width = "";

  const arrayParams = ["f32", "f64", "i32", "i64", "bool"].flatMap(type =>
    [0, 1].map(index => ({ name: `${type}Values[${index}]`, type,
      value: type === "bool" ? false : index, default: type === "bool" ? false : index,
      rangeMin: type === "bool" ? null : 0, rangeMax: type === "bool" ? null : 10,
      scale: type === "bool" ? null : "linear",
      step: type.startsWith("i") ? 1 : null, stepCount: type.startsWith("i") ? 10 : null,
      array: { name: `${type}Values`, length: 2, index },
    })));
  send({ params: arrayParams });
  check(document.querySelectorAll(".param-array-heading").length === 5,
    "primitive arrays have one group heading each");
  check(document.querySelectorAll("#params .param").length === 10,
    "array controls follow their declared lengths");
  const arrayGroups = [...document.querySelectorAll("#params .param-array")];
  check(arrayGroups.every(group => group.open && group.querySelectorAll(".param").length === 2),
    "each array starts expanded with its own controls");
  arrayGroups[0].querySelector("summary").click();
  check(!arrayGroups[0].open && arrayGroups.slice(1).every(group => group.open),
    "array headings collapse their own group independently");
  arrayParams[0].value = 3;
  send({ params: arrayParams });
  check(document.querySelector(".param-array") === arrayGroups[0] && !arrayGroups[0].open,
    "collapsed arrays survive host refreshes");
  arrayGroups[0].querySelector("summary").click();
  check(arrayGroups[0].open
    && Number(arrayGroups[0].querySelector('input[type="number"]').value) === 3,
    "expanding an array shows values updated while collapsed");
  arrayGroups[0].querySelector('input[type="number"]').dispatchEvent(
    new MouseEvent("dblclick", { bubbles: true, cancelable: true }),
  );
  check(paramMessages().at(-1)?.name === "f32Values[0]" && paramMessages().at(-1)?.value === 0,
    "double-click resets only the addressed parameter array element");
  arrayGroups[0].querySelector("summary").click();
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`[data-param-layout="${layout}"]`).click();
    const group = document.querySelector(".param-array");
    check(!group.open && document.querySelectorAll(".param-array[open]").length === 4,
      `${layout} layout preserves each array's fold state`);
    check(document.querySelectorAll(`#params .param-${layout === "knobs" ? "knob" : "slider"}`).length === 8,
      `${layout} layout renders numeric array controls`);
  }
  const boolInput = document.querySelector('#params input[type="checkbox"]');
  boolInput.click();
  check(window.__testMessages.at(-1).name === "boolValues[0]"
    && window.__testMessages.at(-1).value === true,
    "boolean array controls send an indexed address");
  send({ params: arrayParams });
  check(document.querySelector('#params input[type="checkbox"]') === boolInput,
    "array controls survive metadata refreshes");

  const structuredEvents = [{ name: "configure", args: [{ name: "patch", type: "Patch",
    default: { notes: [{ gain: 0.5, id: "9007199254740993" }], pair: [2, true] } }] }];
  send({ events: structuredEvents });
  const structuredGroup = document.querySelector(".event-structured-arg");
  check(structuredGroup && structuredGroup.open,
    "structured event arguments are expanded by default");
  const structuredHeading = structuredGroup.querySelector("summary");
  check(getComputedStyle(structuredHeading, "::before").content.includes("▾"),
    "structured event headings show a disclosure arrow");
  structuredHeading.click();
  check(!structuredGroup.open, "structured event arguments can be collapsed");
  const structured = document.querySelector("#events textarea");
  check(JSON.parse(structured.value).notes[0].id === "9007199254740993",
    "structured defaults preserve nested values and exact i64 strings");
  check(getComputedStyle(structured).resize === "none",
    "structured event editors do not show a manual resize handle");
  check(getComputedStyle(structured).fontFamily.includes("monospace"),
    "structured event editors use the shared monospace font stack");
  structured.focus();
  check(getComputedStyle(structured).outlineStyle === "none",
    "structured event editors suppress the browser-native focus outline");
  check(structured.rows === structured.value.split("\n").length,
    "structured event editors size to their formatted payload");
  edit(structured, JSON.stringify({ values: Array.from({ length: 30 }, (_, index) => index) }, null, 2));
  check(structured.rows === 20, "structured event editors show at most 20 lines");
  edit(structured, '{"notes":');
  send({ events: structuredEvents, logText: "update" });
  check(document.querySelector("#events textarea") === structured && structured.value === '{"notes":'
    && !document.querySelector(".event-structured-arg").open
    && document.querySelector(".event-trigger").disabled,
    "structured fold state and invalid JSON drafts survive refreshes");
  const patch = { notes: [{ gain: 0.75, id: "9223372036854775807" }], pair: [3, false] };
  edit(structured, JSON.stringify(patch));
  document.querySelector(".event-trigger").click();
  check(JSON.stringify(window.__testMessages.at(-1).values[0]) === JSON.stringify(patch),
    "structured event controls dispatch complete nested values");
  send({ events });

  send({ running: true, supportsScope: true });
  const scopeCanvas = document.getElementById("scope-canvas");
  window._onHostMessage({ type: "scopeData", channels: 1, samples: Array(1024).fill(0) });
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const initialScopeWidth = scopeCanvas.width;
  scopeCanvas.style.width = "401px";
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  check(initialScopeWidth > 0 && scopeCanvas.width !== initialScopeWidth,
    "scope backing canvas follows layout changes without new audio data");
  scopeCanvas.style.width = "";

  send({ midi: { noteOn: true }, currentMidiInputDevice: "Computer Keyboard" });
  const octaveInput = document.getElementById("midi-octave");
  const piano = document.getElementById("piano");
  piano.style.width = "210px";
  await waitFrames(1);
  const whiteKeys = [...piano.querySelectorAll(".piano-key.white")];
  const blackKeys = [...piano.querySelectorAll(".piano-key.black")];
  const blackKeyBoundaries = [1, 2, 4, 5, 6, 8, 9, 11, 12, 13];
  check(blackKeys.every((blackKey, index) => {
    const blackCenter = blackKey.getBoundingClientRect().left
      + blackKey.getBoundingClientRect().width / 2;
    const whiteBoundary = whiteKeys[blackKeyBoundaries[index] - 1]
      .getBoundingClientRect().right;
    return Math.abs(blackCenter - whiteBoundary) < 0.75;
  }), "black piano keys remain aligned when the keyboard is narrow");
  piano.style.width = "";
  const key = (code, type = "keydown", target = document) => target.dispatchEvent(
    new KeyboardEvent(type, { code, bubbles: true, cancelable: true }),
  );
  const velocityNumber = document.getElementById("midi-velocity-value");
  check(velocityNumber.readOnly && velocityNumber.dataset.draggable !== undefined,
    "MIDI velocity also has the shared draggable number control");
  edit(velocityNumber, "0.81");
  velocityNumber.blur();
  const velocityPointer = pointerControl(velocityNumber);
  velocityPointer("pointerdown", 100);
  velocityPointer("pointermove", 120);
  velocityPointer("pointerup", 120);
  check(velocityNumber.value === "0.86"
    && Math.abs(Number(midiVelocity.getAttribute("aria-valuenow")) - 0.86) < 1e-6,
    "MIDI velocity number drags use the ranged sensitivity and synchronize its slider");
  velocityPointer("pointerdown", 100);
  velocityPointer("pointermove", 150, { shiftKey: true });
  velocityPointer("pointerup", 150, { shiftKey: true });
  check(velocityNumber.value === "0.87", "MIDI velocity supports fine number dragging");
  edit(velocityNumber, "0.42");
  check(document.activeElement === velocityNumber, "MIDI velocity accepts focus for typing");
  velocityNumber.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  check(velocityNumber.value === "0.42" && velocityNumber.readOnly
    && Math.abs(Number(midiVelocity.getAttribute("aria-valuenow")) - 0.42) < 1e-6,
    "MIDI velocity supports typing and Enter");
  velocityNumber.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  check(velocityNumber.value === "1.00", "MIDI velocity double-click restores its default");
  edit(velocityNumber, "0.81");
  velocityNumber.blur();
  check(octaveInput.readOnly && getComputedStyle(octaveInput).cursor === "ew-resize",
    "MIDI octave uses the shared draggable number widget");
  key("KeyA");
  const octavePointer = pointerControl(octaveInput);
  octavePointer("pointerdown", 100);
  octavePointer("pointermove", 120);
  octavePointer("pointerup", 120);
  check(octaveInput.value === "6"
    && window.__testMessages.findLast(message => message.type === "midiNote")?.pressed === false
    && octaveInput.readOnly,
    "dragging octave changes whole octaves and releases held notes");
  octavePointer("pointerdown", 100);
  octavePointer("pointermove", 200, { shiftKey: true });
  octavePointer("pointerup", 200, { shiftKey: true });
  check(octaveInput.value === "7", "Shift provides fine octave adjustment");
  for (const [initial, moves] of [
    [6, [[14, 7], [8, 6]]],
    [7, [[3, 7], [-3, 6]]],
    [0, [[-14, -1], [-8, 0]]],
    [-1, [[-3, -1], [3, 0]]],
    [6, [[8, 7], [4, 6]]],
    [0, [[-8, -1], [-4, 0]]],
  ]) {
    for (const shiftKey of [false, true]) {
      edit(octaveInput, String(initial));
      octaveInput.blur();
      octavePointer("pointerdown", 100);
      for (const [pixels, expected] of moves) {
        octavePointer("pointermove", 100 + pixels * (shiftKey ? 10 : 1), { shiftKey });
        check(octaveInput.value === String(expected),
          `MIDI octave ${initial} preserves inward motion and discards fractional overshoot at ${pixels} pixels, Shift=${shiftKey}`);
      }
      const [pixels, expected] = moves.at(-1);
      octavePointer("pointerup", 100 + pixels * (shiftKey ? 10 : 1), { shiftKey });
      check(octaveInput.value === String(expected), "MIDI octave reversal survives release");
    }
  }
  octaveInput.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  check(octaveInput.value === "4" && octaveInput.readOnly, "double-click resets MIDI octave to four");
  edit(octaveInput, "2");
  octaveInput.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  check(octaveInput.value === "2" && octaveInput.readOnly, "MIDI octave supports click-to-type and Enter");
  octaveInput.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  key("KeyA");
  check(window.__testMessages.at(-1).type === "midiNote"
    && window.__testMessages.at(-1).key === 60
    && window.__testMessages.at(-1).pressed,
    "computer keyboard plays from the selected octave");
  key("KeyZ");
  check(octaveInput.value === "3" && window.__testMessages.at(-1).key === 60
    && !window.__testMessages.at(-1).pressed,
    "Z lowers the octave and releases held notes");
  key("KeyA");
  check(window.__testMessages.at(-1).key === 48
    && window.__testMessages.at(-1).pressed,
    "notes use the new octave after Z");
  window._onHostMessage({ type: "computerKey", code: "KeyX", pressed: true });
  check(octaveInput.value === "4" && !window.__testMessages.at(-1).pressed,
    "forwarded X raises the octave and releases held notes");
  octaveInput.value = "7";
  key("KeyX");
  check(octaveInput.value === "7", "octave shortcuts respect the upper limit");
  octaveInput.value = "-1";
  key("KeyZ");
  check(octaveInput.value === "-1", "octave shortcuts respect the lower limit");
  key("KeyX", "keydown", octaveInput);
  check(octaveInput.value === "-1", "octave shortcuts leave editable controls alone");

  send({ events: structuredEvents, params: arrayParams });
  const savedPatch = document.querySelector("#events textarea");
  edit(savedPatch, '{"unfinished":');
  document.querySelector(".event-structured-arg summary").click();
  if (document.querySelector(".param-array").open) {
    document.querySelector(".param-array summary").click();
  }
  document.getElementById("events-toggle").click();
  octaveInput.value = "2";
  octaveInput.dispatchEvent(new Event("change", { bubbles: true }));
  const velocitySlider = document.getElementById("midi-velocity");
  velocitySlider.dispatchEvent(new KeyboardEvent("keydown", {
    key: "ArrowRight", bubbles: true, cancelable: true,
  }));
  check(velocityNumber.value === "0.82",
    "velocity slider remains synchronized after editing its number");
  paramsList.style.minHeight = `${window.innerHeight + 400}px`;
  check(scrollNode.scrollHeight > scrollNode.clientHeight
    && document.scrollingElement.scrollTop === 0,
    "the content owns run view scrolling");
  scrollNode.scrollTop = 200;
  await new Promise(resolve => setTimeout(resolve, 130));
  const savedView = window.__testMessages.findLast(message => message.type === "viewState")?.state;
  check(Math.abs(savedView?.scrollY - 200) < 1 && savedView.octave === 2
    && savedView.events[0].drafts[0][0] === '{"unfinished":'
    && savedView.sections.events === false,
    "view snapshot includes content scroll, keyboard, event draft, and section state");
  send({ events: structuredEvents, resetEventArguments: true });
  octaveInput.value = "4";
  document.getElementById("events-toggle").click();
  scrollNode.scrollTop = 0;
  const restoreCount = () => window.__testMessages.filter(
    message => message.type === "runViewReady").length;
  const restoresBefore = restoreCount();
  send({ viewState: savedView, status: "Ready marker" });
  send({ viewState: savedView, status: "Ready marker" });
  check(restoreCount() === restoresBefore,
    "view restore does not report ready before scroll settles");
  check(getComputedStyle(shell).visibility === "hidden",
    "run view content stays hidden while its scroll is being restored");
  await waitFrames(2);
  check(restoreCount() === restoresBefore,
    "view restore waits for a frame with the restored scroll position");
  await waitFrames(1);
  check(restoreCount() === restoresBefore + 1,
    "run view reports ready after scroll settles");
  check(getComputedStyle(shell).visibility === "visible",
    "run view content appears when restoration is ready");
  check(document.getElementById("status").textContent.includes("Ready marker"),
    "view readiness follows the host state render");
  check(octaveInput.value === "2"
    && document.querySelector("#midi-velocity-value").value === "0.82"
    && document.querySelector("#events textarea").value === '{"unfinished":'
    && document.querySelector(".event-trigger").disabled,
    "view restore recovers keyboard controls and incomplete event drafts");
  check(!document.querySelector(".event-structured-arg").open
    && !document.querySelector(".param-array").open
    && document.getElementById("events-toggle").getAttribute("aria-expanded") === "false"
    && Math.abs(scrollNode.scrollTop - savedView.scrollY) < 1,
    "view restore recovers folds and scroll position");
  paramsList.style.minHeight = "";

  send({ viewState: { reset: true, readyId: 42 }, events: structuredEvents,
    resetEventArguments: true });
  await waitFrames(3);
  check(window.__testMessages.findLast(message => message.type === "runViewReady")?.readyId === 42,
    "view readiness identifies the reset it completed");
  check(octaveInput.value === "4"
    && document.querySelector("#midi-velocity-value").value === "1.00"
    && document.querySelector(".event-structured-arg").open
    && document.querySelector(".param-array").open
    && document.getElementById("events-toggle").getAttribute("aria-expanded") === "true",
    "loading a project without view state restores view defaults");

  const beforeDeferred = restoreCount();
  send({ connected: false, events: [], status: "Compiling", viewState: savedView });
  await waitFrames(3);
  check(restoreCount() === beforeDeferred,
    "view readiness waits for the event schema needed to restore saved drafts");
  send({ events: structuredEvents, resetEventArguments: true });
  send({ connected: true, status: "Active" });
  await waitFrames(3);
  check(restoreCount() === beforeDeferred + 1,
    "view readiness follows separately delivered schema and connection updates");
  check(document.querySelector("#events textarea").value === '{"unfinished":'
    && document.querySelector(".event-trigger").disabled,
    "event drafts restore after the compiler publishes the event schema");

  const beforeCoDeliveredSchema = restoreCount();
  send({ connected: false, status: "Compiling...", events: structuredEvents,
    resetEventArguments: true, viewState: savedView });
  await waitFrames(3);
  check(restoreCount() === beforeCoDeliveredSchema,
    "a schema delivered with saved view state waits for connection");
  send({ connected: true, status: "Active" });
  await waitFrames(3);
  check(restoreCount() === beforeCoDeliveredSchema + 1
    && document.querySelector("#events textarea").value === '{"unfinished":',
    "a co-delivered schema restores drafts when connection arrives separately");

  const beforeRetainedSchema = restoreCount();
  send({ connected: false, status: "Compiling...", events: structuredEvents,
    viewState: savedView });
  await waitFrames(3);
  check(restoreCount() === beforeRetainedSchema,
    "compilation waits for a fresh event schema even when old controls remain");
  send({ connected: true, status: "Active", events: structuredEvents,
    resetEventArguments: true });
  await waitFrames(3);
  check(restoreCount() === beforeRetainedSchema + 1
    && document.querySelector("#events textarea").value === '{"unfinished":',
    "readiness follows restoration against the new event schema");

  send({ connected: true, status: "Compiling...", events: structuredEvents,
    viewState: savedView });
  send({ connected: true, status: "Active", events: structuredEvents,
    resetEventArguments: true });
  check(document.querySelector("#events textarea").value === '{"unfinished":',
    "event drafts survive a retained engine being replaced after compilation");

  send({ events });
  send({ connected: false });
  check(document.querySelector(".event-trigger").disabled
    && document.querySelector("#events input").disabled,
    "disconnect disables retained event controls");
  send({ connected: true, events: [] });
  check(!document.querySelector(".event-trigger"), "unload removes event controls");

  await waitFrames(3);
  const beforeSuperseded = restoreCount();
  shell.style.minHeight = `${window.innerHeight + 400}px`;
  send({ path: "test.onda", connected: true, status: "Active",
    events: structuredEvents, viewState: savedView });
  await waitFrames(2);
  check(scrollNode.scrollTop > 0 && restoreCount() === beforeSuperseded,
    "the prior file can scroll before its readiness callback runs");
  send({ path: "next.onda", connected: true, status: "Next",
    events: structuredEvents });
  await waitFrames(3);
  check(scrollNode.scrollTop === 0 && restoreCount() === beforeSuperseded + 1,
    "a newer file resets prior scroll and reports only its own readiness");
  scrollNode.scrollTop = 200;
  check(scrollNode.scrollTop > 0, "the current file can scroll after its restore completes");
  const beforeOrdinarySwitch = restoreCount();
  send({ path: "third.onda", status: "Third", events: structuredEvents });
  check(scrollNode.scrollTop === 0 && restoreCount() === beforeOrdinarySwitch,
    "a later file switch also starts at the top without reporting another restore");

  const beforeFailure = restoreCount();
  scrollNode.scrollTop = 0;
  send({ path: "test.onda", connected: false, status: "Compiling",
    error: "", events: [], viewState: savedView });
  send({ connected: false, status: "Stopped", error: "Compile failed" });
  await waitFrames(2);
  check(restoreCount() === beforeFailure,
    "an immediate compilation failure waits for the saved scroll position");
  await waitFrames(1);
  check(scrollNode.scrollTop > 0 && restoreCount() === beforeFailure + 1,
    "an immediate failure restores scroll before reporting readiness");
  const viewStatesBeforeFailureEdit = window.__testMessages.filter(
    message => message.type === "viewState").length;
  octaveInput.value = "3";
  octaveInput.dispatchEvent(new Event("change", { bubbles: true }));
  const viewStatesAfterFailureEdit = window.__testMessages.filter(
    message => message.type === "viewState");
  check(viewStatesAfterFailureEdit.length === viewStatesBeforeFailureEdit + 1
    && viewStatesAfterFailureEdit.at(-1).state.octave === 3,
    "a terminal view publishes edits after abandoning the missing event schema");

  const beforeDeferredStop = restoreCount();
  send({ connected: false, status: "Compiling", error: "",
    events: [], viewState: savedView });
  await waitFrames(3);
  check(restoreCount() === beforeDeferredStop,
    "a compilation remains pending until its terminal state arrives");
  send({ connected: false, status: "Stopped", error: "" });
  await waitFrames(3);
  check(restoreCount() === beforeDeferredStop + 1,
    "a stopped view reports readiness without waiting for a missing schema");
  await fetch("/result", { method: "POST", body: JSON.stringify({ results }) });
} catch (error) {
  await fetch("/result", { method: "POST",
    body: JSON.stringify({ results, error: `${error.message}\n${error.stack}` }) });
}
