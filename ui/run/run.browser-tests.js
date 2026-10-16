// Executed against the real run view DOM by run.test.js.
const results = [];
function check(condition, message) {
  if (!condition) throw new Error(message);
  results.push(message);
}
function send(state) {
  window._onHostMessage({ type: "state", state: structuredClone(state) });
}
async function waitFrames(count, view = window) {
  for (let index = 0; index < count; index++) {
    await new Promise(resolve => view.requestAnimationFrame(resolve));
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

async function checkContentBoxScope() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:fixed;left:0;top:0;width:720px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/content-box-layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    check(!("devicePixelContentBoxSize" in view.ResizeObserverEntry.prototype),
      "the scope fixture models a browser without device-pixel observation");
    check(view.__testErrors.length === 0,
      "the run view initializes without errors when only content-box observation is supported");
    view._onHostMessage({ type: "state", state: {
      running: false, connected: false, path: "scope.onda", status: "Stopped",
      supportsScope: true, supportsViewState: true, params: [], events: [],
      viewState: { reset: true, readyId: 84 },
    }});
    await waitFrames(4, view);
    check(view.__testMessages.some(message =>
      message.type === "runViewReady" && message.readyId === 84),
      "content-box scope observation preserves the native view readiness handshake");
    view._onHostMessage({ type: "scopeData", channels: 1, samples: [0, 0.25, -0.25, 0] });
    await waitFrames(2, view);
    const canvas = doc.getElementById("scope-canvas");
    const initialWidth = canvas.width;
    for (const width of [540, 720]) {
      frame.style.width = `${width}px`;
      await waitFrames(3, view);
      const expectedWidth = Math.round(canvas.getBoundingClientRect().width * view.devicePixelRatio);
      check(canvas.width === expectedWidth && (width !== 540 || canvas.width !== initialWidth),
        `content-box scope observation resizes the bitmap at ${width} pixels without new host data`);
    }
    check(view.__testErrors.length === 0,
      "content-box scope observation remains free of errors after state restoration and resizing");
  } finally {
    frame.remove();
  }
}

async function checkEmbeddedHosts() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:fixed;left:0;top:0;width:720px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/no-popover-layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    const publish = state => view._onHostMessage({ type: "state", state });
    publish({ supportsSourceSelection: true, supportsViewState: true,
      supportsDeviceSelection: false, supportsMidiInput: false,
      viewState: { reset: true, readyId: 85 } });
    await waitFrames(4, view);
    check(view.__testMessages.some(message => message.type === "runViewReady" && message.readyId === 85)
      && view.getComputedStyle(doc.querySelector(".shell")).visibility === "visible",
    "embedded hosts without popovers complete startup and reveal the view");
    const parameter = { name: "value", type: "f64", scalar: true, value: 440, default: 440,
      rangeMin: 0, rangeMax: 1, scale: "linear" };
    publish({ path: "host.onda", running: true, connected: true,
      params: [parameter], inputControls: [{ ...parameter, enabled: true }] });
    const writes = () => view.__testMessages.filter(message =>
      message.type === "setParam" || message.type === "setInputControl");
    for (const layout of ["sliders", "knobs"]) {
      publish({ paramLayout: layout, inputControlLayout: layout });
      for (const bank of ["params", "inputControls"]) {
        const field = () => doc.querySelector(`#${bank === "params" ? "params" : "input-controls"} .param-number`);
        publish({ [bank]: [{ ...parameter }] });
        publish({ [bank]: [{ ...parameter }] });
        const before = writes().length;
        field().focus();
        field().blur();
        check(field().value === "440" && writes().length === before,
          `${bank} ${layout}: publications and unedited blur preserve out-of-range host defaults`);
        for (const value of ["9007199254740993", "-9007199254740993",
          "9223372036854775807", "-9223372036854775808"]) {
          const integer = { ...parameter, type: "i64", value, default: value, step: 1, stepCount: 1 };
          publish({ [bank]: [integer] });
          publish({ [bank]: [{ ...integer }] });
          field().focus();
          check(field().value === value, `${bank} ${layout}: exact i64 defaults survive publication and focus`);
          field().blur();
          check(field().value === value && writes().length === before,
            `${bank} ${layout}: unedited i64 blur preserves all digits without host writes`);
        }
        publish({ [bank]: [{ ...parameter, value: 0.25, default: 0.25 }] });
        field().focus();
        field().select();
        check(view.getSelection().toString() === "0.25",
          `${bank} ${layout}: visible decimal text uses a dot in an Italian document`);
        field().blur();
        for (const invalid of ["", "0,5", "0x10", "1e", "Infinity"]) {
          field().focus();
          field().value = invalid;
          field().dispatchEvent(new view.Event("input", { bubbles: true }));
          field().blur();
          check(field().value === "0.25" && writes().length === before,
            `${bank} ${layout}: invalid numeric drafts restore the host value without writing`);
        }
      }
    }
    publish({ events: [{ name: "trigger", args: [{ name: "amount", type: "f64", default: 0.25 }] }] });
    const eventNumber = doc.querySelector("#events .param-number");
    eventNumber.focus();
    eventNumber.select();
    check(view.getSelection().toString() === "0.25", "event decimals also display dots in an Italian document");
    for (const invalid of ["0,5", "0x10", "1e", "Infinity"]) {
      eventNumber.value = invalid;
      eventNumber.dispatchEvent(new view.Event("input", { bubbles: true }));
      check(eventNumber.validity.customError && doc.querySelector(".event-trigger").disabled,
        "invalid event number drafts disable triggering");
    }
    eventNumber.blur();
    check(eventNumber.value === "0.25" && !eventNumber.validity.customError,
      "leaving an invalid event number restores its last valid value");
    for (const width of [360, 480, 720]) {
      frame.style.width = `${width}px`;
      await waitFrames(2, view);
      for (const button of doc.querySelectorAll("[data-param-layout]")) {
        const bounds = button.getBoundingClientRect();
        const range = doc.createRange();
        range.selectNodeContents(button);
        const text = range.getBoundingClientRect();
        check(bounds.width > 0 && Math.abs(text.left + text.right - bounds.left - bounds.right) <= 1,
          `${width}px: layout button labels remain centered`);
      }
      doc.querySelector(".shell").dispatchEvent(new view.Event("scroll"));
      check(doc.querySelector("#settings-popover").getClientRects().length === 0,
        `${width}px: unavailable device panels leave no floating capsule`);
    }
    check(view.__testErrors.length === 0,
      "unsupported popovers never interrupt rendering or resizing: " + view.__testErrors.join("; "));
  } finally {
    frame.remove();
  }
}

async function checkThemedFormControls() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:absolute;left:-10000px;width:720px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    // VS Code injects this focus rule; use orange explicitly across browser engines.
    const hostStyle = doc.createElement("style");
    hostStyle.textContent = `@layer vscode-default {
      input:focus, select:focus, textarea:focus {
        outline: 1px solid orange;
        outline-offset: -1px;
      }
    }`;
    doc.head.append(hostStyle);
    view._onHostMessage({ type: "state", state: {
      running: true, connected: true, path: "focus.onda",
      params: [params[0], { name: "enabled", type: "bool", value: true }],
      events: [{ name: "configure", args: [
        { name: "level", type: "f32", default: 0.5 },
        { name: "patch", type: "Patch", default: { gain: 0.5 } },
        { name: "reset", type: "bool", default: false },
        { name: "gates", type: "bool[2]", default: [true, false] },
      ] }],
      midi: { available: true, noteOn: true }, currentMidiInputDevice: "Computer Keyboard",
      supportsSourceSelection: true,
      supportsRunSettings: true, sampleRateHz: 48000, blockSamples: 512,
      supportsParamSmoothing: true, paramSmoothingMs: 30,
    }});
    for (const checkbox of doc.querySelectorAll('#events input[type="checkbox"]')) {
      const toggle = checkbox.closest('.param-toggle');
      const field = toggle.getBoundingClientRect();
      const box = checkbox.getBoundingClientRect();
      check(toggle.textContent.trim() === '' && checkbox.getAttribute('aria-label')
        && Math.abs(box.left + box.width / 2 - field.left - field.width / 2) < 1
        && Math.abs(box.top + box.height / 2 - field.top - field.height / 2) < 1,
        'scalar and array boolean event controls center without redundant value labels');
    }
    for (const theme of ["dark", "light"]) {
      view._onHostMessage({ type: "state", state: { themeMode: theme } });
      const accent = theme === "dark" ? "rgb(201, 219, 242)" : "rgb(20, 58, 99)";
      for (const selector of ["#params input.param-number", "#events input.param-number",
        "#midi-octave", "#midi-velocity-value", "#params input[type=checkbox]", "#run-sample-rate"]) {
        const control = doc.querySelector(selector);
        // The sample-rate select is visible in the empty view.
        if (control.tagName === "SELECT") {
          view._onHostMessage({ type: "state", state: { path: "" } });
          await waitFrames(2);
        }
        control.focus();
        check(doc.activeElement === control && view.getComputedStyle(control).outlineColor === accent,
          `${theme}: ${selector} focus uses Onda's accent despite the host outline`);
        if (control.tagName === "SELECT") {
          view._onHostMessage({ type: "state", state: { path: "focus.onda" } });
          await waitFrames(2);
        }
      }
      check(view.getComputedStyle(doc.documentElement).colorScheme === theme,
        `${theme}: native form controls follow Onda's selected theme`);
      for (const checkbox of doc.querySelectorAll('input[type="checkbox"]')) {
        const original = checkbox.checked;
        for (const checked of [false, true]) {
          checkbox.checked = checked;
          check(view.getComputedStyle(checkbox).backgroundColor === (checked ? accent
            : theme === "dark" ? "rgb(13, 24, 39)" : "rgb(255, 255, 255)"),
            `${theme}: ${checkbox.id || "boolean parameter"} uses themed ${checked ? "checked" : "unchecked"} colors`);
        }
        checkbox.checked = original;
      }
      const number = doc.querySelector('#params input.param-number');
      check(view.getComputedStyle(number, "::selection").backgroundColor === accent,
        `${theme}: selected number text uses Onda's accent`);
      for (const selector of ["#reset-params", "#params-toggle", "#events summary", ".event-trigger"]) {
        number.focus();
        const control = doc.querySelector(selector);
        control.focus();
        check(control.matches(":focus-visible") && view.getComputedStyle(control).outlineColor === accent,
          `${theme}: ${selector} keyboard focus uses Onda's accent`);
      }
      doc.getElementById("param-smoothing-ms").focus();
      check(view.getComputedStyle(doc.getElementById("param-smoothing-control")).borderColor === accent,
        `${theme}: smoothing keeps its themed focus on the enclosing control`);
    }
  } finally {
    frame.remove();
  }
}

async function checkDefaultLayout() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:absolute;left:-10000px;width:720px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    const path = "/a/long/project/directory/orbit_flanger.onda";
    const state = {
      running: true, connected: true, path, status: "Running",
      outputChannels: 2, buffers: [], events: [], logRevealed: false,
      params: Array.from({ length: 8 }, (_, index) => ({
        name: `parameter_${index}`, type: "f32", scalar: true, value: 0.5, default: 0.5,
        rangeMin: 0, rangeMax: 1, scale: "linear",
      })),
      inputDevices: ["Default input"], outputDevices: ["Default output"],
      midiInputDevices: ["Computer Keyboard"],
      supportsSourceSelection: true, supportsTransport: true,
      supportsDeviceSelection: true, supportsParamSmoothing: true,
      paramSmoothingMs: 30, paramSmoothingDefaultMs: 30,
      supportsScope: true, supportsProjectExport: true, canExportProject: true,
      sampleRateHz: 48000, blockSamples: 512,
    };
    for (const themeMode of ["light", "dark"]) {
      for (const available of [false, true]) {
        view._onHostMessage({ type: "state", state: {
          ...state, themeMode, midi: { available, noteOn: available },
          currentMidiInputDevice: "Computer Keyboard",
        }});
        view._onHostMessage({ type: "scopeData", channels: 2, samples: Array(512).fill(0) });
        doc.querySelector('#params-section [data-param-layout="knobs"]').click();
        await waitFrames(2);
        const shell = doc.querySelector(".shell");
        const cards = [...doc.querySelectorAll("#params .param")];
        const rows = new Set(cards.map(card => Math.round(card.getBoundingClientRect().top)));
        check(view.innerWidth === 720 && view.innerHeight === 840,
          "run layout coverage uses the actual 720 × 840 viewport");
        check(rows.size === 2 && cards.length === 8,
          `${themeMode}, MIDI ${available}: eight knobs stay in two rows`);
        check(shell.scrollHeight === shell.clientHeight,
          `${themeMode}, MIDI ${available}: the scope and two knob rows fit without scrolling`);
        check(doc.getElementById("midi-keyboard").hidden === !available,
          "the fit check includes the visible MIDI keyboard when supported");
        check(doc.getElementById("params-section").getBoundingClientRect().bottom
          <= shell.getBoundingClientRect().bottom - parseFloat(getComputedStyle(shell).paddingBottom) + 1,
          "the whole second knob row is visible within the content area");
        check(doc.getElementById("scope-canvas").getBoundingClientRect().height === 160
          && cards.every(card => card.getBoundingClientRect().height === 140),
          "the scope plot is 160 pixels tall and knob cards retain their sizes");
        check(doc.getElementById("path").textContent === "orbit_flanger.onda"
          && doc.getElementById("path").title === path,
          "the header shows the filename and retains the full path on hover");
        const transport = [...doc.querySelectorAll(".actions button")]
          .filter(control => control.getClientRects().length > 0)
          .map(control => control.getBoundingClientRect());
        const header = doc.getElementById("run-header").getBoundingClientRect();
        check(header.height < 90 && transport.length === 3
          && transport.every(rect => rect.height === 32 && rect.width === 32
            && Math.abs(rect.top - transport[0].top) < 1),
          `${themeMode}, MIDI ${available}: the compact header keeps three equally sized icon buttons`);
        check([...doc.querySelectorAll(".actions button, .transport button")].every(button =>
          button.getAttribute("aria-label") && button.title),
        "every header icon has an accessible name and tooltip");
        const version = doc.getElementById("onda-version").getBoundingClientRect();
        const logo = doc.getElementById("hero-logo").getBoundingClientRect();
        check(Math.abs(Math.min(logo.left, version.left) - header.left - 13) < 1
          && Math.abs(version.top - logo.bottom - 4) < 1
          && Math.abs(version.left + version.width / 2 - logo.left - logo.width / 2) < 1
          && doc.getElementById("onda-version").textContent === "0.0.0-test"
          && Math.abs((logo.top + version.bottom) / 2 - transport[0].top - transport[0].height / 2) < 1,
          "the version sits centered below the logo, with the brand centered beside the controls");
        check(Math.abs(transport.at(-1).right - header.right + 13) < 1,
          "the toolbar aligns with the header's right edge");
        const source = doc.getElementById("hero-source").getBoundingClientRect();
        const brand = doc.getElementById("hero-brand").getBoundingClientRect();
        check(["path", "status"].every(id => {
          const label = doc.getElementById(id);
          const rect = label.getBoundingClientRect();
          return getComputedStyle(label).textAlign === "left"
            && Math.abs(rect.left - source.left) < 1
            && Math.abs(rect.left - brand.right - 16) < 1
            && rect.right <= transport[0].left - 12 + 1;
        }), "the filename and status share a left edge after the brand divider");
        const filename = doc.getElementById("path").getBoundingClientRect();
        const details = doc.querySelector(".hero-file-details").getBoundingClientRect();
        const playStop = doc.querySelector(".transport button:not([hidden])").getBoundingClientRect();
        check(Math.abs(playStop.left - filename.right - 16) < 1
          && Math.abs(playStop.top + playStop.height / 2 - details.top - details.height / 2) < 1
          && Math.abs(playStop.top + playStop.height / 2 - transport[0].top - transport[0].height / 2) < 1
          && playStop.width === 72 && playStop.height === 44
          && doc.querySelector(".transport button:not([hidden]) span").textContent === "Stop",
          "the labeled Play/Stop button leads the toolbar, aligned with the utility icons");
        const settings = doc.getElementById("settings-toggle");
        const panel = doc.getElementById("settings-popover");
        check(!panel.matches(":popover-open") && settings.getAttribute("aria-expanded") === "false",
          "device settings are hidden by default");
        settings.click();
        await waitFrames(2);
        const bounds = panel.getBoundingClientRect();
        const devices = [...panel.querySelectorAll("select, #refresh-devices")]
          .filter(control => control.getClientRects().length > 0)
          .map(control => control.getBoundingClientRect());
        check(panel.matches(":popover-open") && settings.getAttribute("aria-expanded") === "true"
          && bounds.left >= 12 && bounds.right <= view.innerWidth - 12
          && bounds.top >= 12 && bounds.bottom <= view.innerHeight - 12
          && doc.getElementById("run-header").getBoundingClientRect().height === header.height,
        "Settings opens within the viewport without enlarging the header");
        check(!panel.querySelector("h2"), "device settings have no redundant title");
        check(devices.every(rect => rect.height === 32
          && Math.abs(rect.width - devices[0].width) < 1),
        "device selectors and Refresh use consistent sizes in Settings");
        const input = doc.getElementById("input-device");
        input.value = "Default input";
        input.dispatchEvent(new view.Event("change", { bubbles: true }));
        check(view.__testMessages.at(-1).type === "setInputDevice"
          && view.__testMessages.at(-1).name === "Default input" && panel.matches(":popover-open"),
        "changing a device sends its selection and keeps Settings open");
        doc.getElementById("refresh-devices").click();
        check(view.__testMessages.at(-1).type === "refreshDevices",
          "Refresh remains available in Settings and sends its host command");
        doc.getElementById("save-project").click();
        await waitFrames(2);
        check(!panel.matches(":popover-open")
          && view.__testMessages.at(-1).type === "saveProjectAs",
        "the direct Save as Project icon sends its command and dismisses Settings");
      }
    }
    // Scrollbar appearance must not change the grid or leave it in a different
    // layout after Log is hidden or the viewport approaches from another width.
    const gridGeometry = () => {
      const cards = [...doc.querySelectorAll("#params .param")]
        .map(card => card.getBoundingClientRect());
      return JSON.stringify({
        contentWidth: doc.querySelector(".shell").clientWidth,
        cardWidths: cards.map(rect => rect.width),
        rows: new Set(cards.map(rect => Math.round(rect.top))).size,
      });
    };
    for (const width of [626, 638]) {
      frame.style.width = "720px";
      view._onHostMessage({ type: "state", state: { logRevealed: false } });
      await waitFrames(2);
      frame.style.width = `${width}px`;
      await waitFrames(2);
      const geometry = gridGeometry();
      for (const logRevealed of [true, false]) {
        view._onHostMessage({ type: "state", state: { logRevealed, logText: "ready" } });
        await waitFrames(2);
        check(gridGeometry() === geometry,
          `the ${width}-pixel parameter grid stays stable when Log is ${logRevealed ? "shown" : "hidden"}`);
      }
      frame.style.width = "520px";
      await waitFrames(2);
      frame.style.width = `${width}px`;
      await waitFrames(2);
      check(gridGeometry() === geometry,
        `the ${width}-pixel parameter grid is independent of resize direction`);
    }
    for (const width of [240, 280, 320, 400]) {
      frame.style.width = `${width}px`;
      await waitFrames(2);
      const header = doc.getElementById("run-header").getBoundingClientRect();
      const inset = parseFloat(getComputedStyle(doc.getElementById("run-header")).paddingLeft) + 1;
      check(["path", "status"].every(id => {
        const rect = doc.getElementById(id).getBoundingClientRect();
        return Math.abs(rect.left - header.left - inset) < 1;
      }), `the file details stay left aligned in a ${width}-pixel header`);
      const brand = doc.getElementById("hero-brand").getBoundingClientRect();
      const source = doc.querySelector(".hero-source").getBoundingClientRect();
      const playStop = doc.querySelector(".transport button:not([hidden])").getBoundingClientRect();
      check(Math.abs(playStop.left - source.right - 12) < 1,
        `the ${width}-pixel source row uses all space before Play/Stop, independently of utilities`);
      check([...doc.querySelectorAll(".actions button")]
        .filter(button => !button.hidden)
        .every(button => {
          const rect = button.getBoundingClientRect();
          return [brand, source].every(bounds => rect.right <= bounds.left
            || rect.left >= bounds.right || rect.bottom <= bounds.top || rect.top >= bounds.bottom);
        }), `header controls stay clear of the brand and file details at ${width} pixels`);
      const cards = [...doc.querySelectorAll("#params .param")];
      const rows = new Set(cards.map(card => Math.round(card.getBoundingClientRect().top)));
      check(rows.size === 4 && cards.length === 8,
        `a ${width}-pixel narrow view keeps two knobs per row`);
      check(cards.every(card => {
        const bounds = card.getBoundingClientRect();
        return [...card.querySelectorAll(".param-head, .param-knob, .param-number")].every(control => {
          const rect = control.getBoundingClientRect();
          return rect.left >= bounds.left && rect.right <= bounds.right
            && rect.top >= bounds.top && rect.bottom <= bounds.bottom;
        });
      }) && doc.querySelector(".shell").scrollWidth === doc.querySelector(".shell").clientWidth,
      `the controls fit inside narrow knob cards at ${width} pixels without horizontal scrolling`);
      doc.querySelector('#params-section [data-param-layout="sliders"]').click();
      await waitFrames(2);
      check([...doc.querySelectorAll("#params .param")].every(card => {
        const name = card.querySelector(".param-name").getBoundingClientRect();
        const value = card.querySelector(".param-number").getBoundingClientRect();
        return name.width >= 48 && (name.right <= value.left || name.bottom <= value.top);
      }), `a ${width}-pixel slider view keeps parameter names readable beside or above their values`);
      doc.querySelector('#params-section [data-param-layout="knobs"]').click();
      await waitFrames(2);
      const settings = doc.getElementById("settings-toggle");
      check(settings.getBoundingClientRect().width === 44
        && settings.getBoundingClientRect().height === 44,
      "narrow views give header icons larger touch targets");
      settings.click();
      await waitFrames(2);
      const panel = doc.getElementById("settings-popover");
      const bounds = panel.getBoundingClientRect();
      check(bounds.left >= 12 && bounds.right <= width - 12,
        `Settings stays inside a ${width}-pixel view`);
      panel.hidePopover();
      await waitFrames(1);
      const shell = doc.querySelector(".shell");
      shell.scrollTop = shell.scrollHeight;
      await waitFrames(2);
      check(Math.abs(doc.getElementById("midi-keyboard").getBoundingClientRect().top
        - doc.getElementById("params-section").getBoundingClientRect().bottom - 12) < 1,
      `the end of scrolling leaves a 12-pixel gutter above the keyboard at ${width} pixels`);
      shell.scrollTop = 0;
      await waitFrames(2);
    }
    for (const [width, zoom] of [
      [144, 1], [160, 1], [180, 1], [240, 1], [280, 1], [320, 1], [400, 1], [720, 1],
      [720, 2], [720, 3], [720, 4],
    ]) {
      frame.style.width = `${width}px`;
      doc.body.style.zoom = String(zoom);
      await waitFrames(2);
      const panel = doc.getElementById("params-section").getBoundingClientRect();
      const smoothing = doc.getElementById("param-smoothing-control").getBoundingClientRect();
      const input = doc.getElementById("param-smoothing-ms");
      const layout = doc.querySelector("#params-section .param-layout-toggle").getBoundingClientRect();
      const controls = [
        doc.querySelector("#params-section .section-heading").getBoundingClientRect(),
        doc.getElementById("reset-params").getBoundingClientRect(),
        smoothing, layout,
      ];
      check(controls.every(rect => rect.left >= panel.left + 8.5 * zoom
        && rect.right <= panel.right - 8.5 * zoom)
        && input.scrollWidth <= input.clientWidth,
      `Params settings and the complete smoothing value fit at ${width} pixels and ${zoom}× zoom`);
      check(controls.every(rect => Math.abs(rect.top + rect.height / 2
        - controls[0].top - controls[0].height / 2) < 1)
        && controls.slice(1).every((rect, index) => controls[index].right <= rect.left + 1)
        && doc.querySelector("#params-section .param-smoothing-label").getBoundingClientRect().width > 0,
        `Params, reset, Smooth, Sliders, and Knobs share one line at ${width} pixels and ${zoom}× zoom`);
      check(getComputedStyle(doc.querySelector("#params-section .params-title")).fontSize === "14px",
        `the Params title keeps its standard size at ${width} pixels and ${zoom}× zoom`);
      const actions = doc.querySelector("#params-section .section-actions");
      const controlFont = parseFloat(getComputedStyle(input).fontSize);
      check(Math.abs(controlFont - 11) < .01
        || Math.abs(actions.getBoundingClientRect().width
          - controls.slice(1).reduce((sum, rect) => sum + rect.width, 8 * zoom)) < 1,
        `Params controls use their normal size or fill the available row before shrinking at ${width} pixels and ${zoom}× zoom`);
      view._onHostMessage({ type: "state", state: { supportsParamSmoothing: false } });
      await waitFrames(2);
      const resetWithoutSmoothing = doc.getElementById("reset-params").getBoundingClientRect();
      const layoutWithoutSmoothing = doc.querySelector("#params-section .param-layout-toggle").getBoundingClientRect();
      check(parseFloat(getComputedStyle(doc.getElementById("reset-params")).fontSize) >= controlFont
        && (getComputedStyle(doc.getElementById("reset-params")).fontSize === "11px"
          || Math.abs(actions.getBoundingClientRect().width
            - resetWithoutSmoothing.width - layoutWithoutSmoothing.width - 4 * zoom) < 1),
        `hidden smoothing does not reserve space or unnecessarily shrink the remaining controls at ${width} pixels and ${zoom}× zoom`);
      view._onHostMessage({ type: "state", state: { supportsParamSmoothing: true } });
    }
    doc.body.style.zoom = "";
    await waitFrames(3);
    const stoppedTransportBounds = doc.getElementById("stop").getBoundingClientRect();
    const keyboardBounds = doc.getElementById("midi-keyboard").getBoundingClientRect();
    doc.getElementById("stop").click();
    check(view.__testMessages.at(-1).type === "stop", "the Stop icon sends its transport command");
    view._onHostMessage({ type: "state", state: { running: false, connected: false, status: "Stopped" } });
    check(!doc.getElementById("midi-keyboard").hidden
      && doc.getElementById("midi-keyboard").getBoundingClientRect().y === keyboardBounds.y
      && doc.getElementById("midi-keyboard").getBoundingClientRect().height === keyboardBounds.height,
      "Stop preserves the MIDI keyboard dock and content viewport");
    check(!doc.getElementById("start").hidden && doc.getElementById("stop").hidden,
      "stopping replaces Stop with Play in the same toolbar position");
    const playBounds = doc.getElementById("start").getBoundingClientRect();
    check(playBounds.x === stoppedTransportBounds.x && playBounds.y === stoppedTransportBounds.y
      && playBounds.width === stoppedTransportBounds.width && playBounds.height === stoppedTransportBounds.height,
      "switching playback state preserves the transport button's position and size");
    doc.getElementById("start").click();
    check(view.__testMessages.at(-1).type === "start", "the Play icon sends its transport command");
    view._onHostMessage({ type: "state", state: { running: true, connected: true, status: "Running" } });
    view._onHostMessage({ type: "state", state: {
      supportsDeviceSelection: false, midi: { available: true, noteOn: false },
    }});
    await waitFrames(2);
    doc.getElementById("settings-toggle").click();
    await waitFrames(2);
    const midiSelect = doc.getElementById("midi-input-device").getBoundingClientRect();
    const refresh = doc.getElementById("refresh-devices").getBoundingClientRect();
    check(Math.abs(midiSelect.width - refresh.width) < 1
      && midiSelect.height === 32 && refresh.height === 32,
      "MIDI-only hosts retain their selector and Refresh in Settings");
    check(doc.activeElement.id === "midi-input-device",
      "MIDI-only Settings focuses its first visible control");
    doc.getElementById("settings-popover").hidePopover();
    await waitFrames(1);
    // A playground source without MIDI has no device or file actions.
    view._onHostMessage({ type: "state", state: {
      supportsSourceSelection: false, supportsProjectExport: false,
      midi: { available: false, noteOn: false },
      ondaVersion: "0.8.26",
    }});
    for (const width of [320, 144, 160, 180, 240, 400, 480, 560, 720]) {
      frame.style.width = `${width}px`;
      for (const running of [false, true]) {
        view._onHostMessage({ type: "state", state: {
          running, path: running ? "karplus_strong.onda" : "long_filename_".repeat(100) + ".onda",
        }});
        await waitFrames(2);
        const header = doc.getElementById("run-header").getBoundingClientRect();
        const transport = doc.querySelector(".transport button:not([hidden])").getBoundingClientRect();
        const source = doc.getElementById("hero-source").getBoundingClientRect();
        const brand = doc.getElementById("hero-brand").getBoundingClientRect();
        if (width >= 240) {
          check(brand.top < source.bottom && source.top < brand.bottom,
            `a ${width}-pixel playground keeps branding, source details, and Play/Stop on one row`);
        }
        check(Math.abs(transport.right - header.right + 13) < 1,
          `a ${width}-pixel playground without utility actions aligns Play/Stop with the right gutter`);
        check(source.width > 0 && source.top < transport.bottom && transport.top < source.bottom
          && source.right <= transport.left
          && transport.width === 72 && transport.height === 44,
          `a ${width}-pixel playground keeps source details beside a 72-by-44 Play/Stop button`);
        check(doc.querySelector(".actions").getClientRects().length === 0,
          "an empty utility group reserves no toolbar space or extra row");
        if (width >= 480) {
          check(header.height < 90 && !doc.getElementById("run-header").classList.contains("narrow"),
            `a ${width}-pixel playground keeps a sparse toolbar on one row`);
        }
      }
    }
    // Allow room for the block-size unit when checking an untruncated single row.
    frame.style.width = "420px";
    for (const running of [false, true]) {
      view._onHostMessage({ type: "state", state: {
        path: "sine.onda", ondaVersion: "0.8.26",
        status: running ? "Running" : "Stopped", running,
      }});
      await waitFrames(2);
      check(doc.getElementById("run-header").getBoundingClientRect().height < 90
        && ["path", "status"].every(id => {
          const label = doc.getElementById(id);
          return label.scrollWidth <= label.clientWidth;
        }), "a 420-pixel sparse playground keeps short source details and transport on one row");
    }
    view._onHostMessage({ type: "state", state: {
      supportsSourceSelection: true, supportsProjectExport: true,
      midi: { available: true, noteOn: false },
    }});
    view._onHostMessage({ type: "state", state: { supportsDeviceSelection: true } });
    frame.style.width = "180px";
    await waitFrames(2);
    const zoomHeader = doc.getElementById("run-header").getBoundingClientRect();
    const utilityBounds = [...doc.querySelectorAll(".actions button")]
      .filter(button => !button.hidden).map(button => button.getBoundingClientRect());
    check(utilityBounds.every(rect => rect.left >= zoomHeader.left + 13
      && rect.right <= zoomHeader.right - 13)
      && new Set(utilityBounds.map(rect => Math.round(rect.top))).size === 2,
      "at high zoom the utility buttons wrap inside the header gutters");
    const longName = "long_name_".repeat(100) + ".onda";
    view._onHostMessage({ type: "state", state: { path: `C:\\project\\${longName}` } });
    check(doc.getElementById("path").textContent === longName,
      "the source header also recognizes Windows paths");
    for (const width of [144, 160, 180, 240, 320, 400, 720]) {
      frame.style.width = `${width}px`;
      await waitFrames(2);
      const shell = doc.querySelector(".shell");
      const bounds = doc.getElementById("run-header").getBoundingClientRect();
      const controls = [...doc.querySelectorAll(".actions button, .transport button")]
        .filter(control => control.getClientRects().length > 0);
      check(shell.scrollWidth === shell.clientWidth && controls.every(control => {
        const rect = control.getBoundingClientRect();
        return rect.left >= bounds.left && rect.right <= bounds.right;
      }), `source header controls stay inside a ${width}-pixel window with a long filename`);
      const filename = doc.getElementById("path");
      const playStop = doc.querySelector(".transport button:not([hidden])").getBoundingClientRect();
      check(filename.title === `C:\\project\\${longName}`
        && getComputedStyle(filename).textOverflow === "ellipsis"
        && filename.scrollWidth > filename.clientWidth
        && filename.getBoundingClientRect().right <= playStop.left,
        `long filenames truncate before Play/Stop and retain the full path on hover at ${width} pixels`);
      const source = doc.getElementById("hero-source").getBoundingClientRect();
      check(source.width > 0 && source.top < playStop.bottom && playStop.top < source.bottom,
        `long filenames never push Play/Stop below the source row at ${width} pixels`);
    }
    const sections = ["scope", "log", "buffers", "events", "params"];
    view._onHostMessage({ type: "state", state: {
      logRevealed: true, logText: "ready", supportsBufferEmbedding: true,
      buffers: [{ name: "buffer", type: "f32[]" }], events: [{ name: "ping", args: [] }],
    }});
    for (const section of ["events", "params"]) {
      const heading = doc.querySelector(`#${section}-section .section-heading`);
      const reset = section === "params" ? doc.getElementById("reset-params")
        : heading.querySelector(".reset-action");
      check(reset && Math.abs(reset.getBoundingClientRect().left
        - heading.querySelector(".params-title").getBoundingClientRect().right
        - parseFloat(getComputedStyle(heading).columnGap)) < 1,
        `${section} reset stays beside its heading`);
    }
    for (const section of sections) {
      const toggle = doc.getElementById(`${section}-toggle`);
      if (toggle.getAttribute("aria-expanded") === "true") toggle.click();
    }
    for (const themeMode of ["light", "dark"]) {
      view._onHostMessage({ type: "state", state: { themeMode } });
      for (const width of [240, 280, 320, 400, 720]) {
        frame.style.width = `${width}px`;
        await waitFrames(2);
        for (const section of sections) {
          const panel = doc.getElementById(`${section}-section`);
          const bounds = (section === "params"
            ? panel.querySelector(".section-heading") : panel).getBoundingClientRect();
          check(doc.getElementById(`${section}-body`).hidden
            && [...panel.querySelector(".section-heading").children].filter(node =>
              node.matches(".section-toggle, .params-title")).every(node => {
                const rect = node.getBoundingClientRect();
                return Math.abs(rect.top + rect.height / 2 - bounds.top - bounds.height / 2) < 1;
              }), `${themeMode}, ${width}px: collapsed ${section} arrow and title are vertically centered`);
        }
        check(doc.querySelector(".shell").scrollWidth === doc.querySelector(".shell").clientWidth,
          "collapsed headers keep their actions inside narrow views");
      }
    }
    frame.style.width = "720px";
    view._onHostMessage({ type: "state", state: {
      running: false, connected: false, path: "", params: [], buffers: [], events: [], logRevealed: false,
      midi: { available: false, noteOn: false }, supportsRunSettings: true,
    }});
    await waitFrames(2);
    const picker = doc.getElementById("file-picker");
    const version = doc.getElementById("onda-version");
    const versionBounds = version.getBoundingClientRect();
    const logoBounds = doc.getElementById("file-picker-logo").getBoundingClientRect();
    check(!picker.hidden && !version.hidden
      && Math.abs(versionBounds.top - logoBounds.bottom - 4) < 1
      && Math.abs(versionBounds.left + versionBounds.width / 2
        - logoBounds.left - logoBounds.width / 2) < 1
      && version.textContent === doc.getElementById("onda-version-number").textContent,
      "unloading centers only the version number below the file picker logo");
    check(doc.querySelector(".shell").scrollHeight === 840,
      "the empty file picker also fits the default window without scrolling");
    const pickerBounds = picker.getBoundingClientRect();
    check(Math.abs(pickerBounds.left - (view.innerWidth - pickerBounds.right)) < 1
      && Math.abs(logoBounds.left + logoBounds.width / 2 - view.innerWidth / 2) < 1,
      "the unloaded picker and logo center in the full window without reserving a scrollbar gutter");
    const fastMath = doc.getElementById("run-fast-math");
    const fastMathControl = doc.getElementById("file-picker-fast-math");
    const fastMathMessages = () => view.__testMessages.filter(message => message.type === "setFastMath");
    check(fastMathControl.hidden, "hosts opt in to the Fast Math control");
    view._onHostMessage({ type: "state", state: { supportsFastMath: true, fastMath: true } });
    check(!fastMathControl.hidden && fastMath.checked, "Fast Math starts from the host's compile option");
    const fastMathBounds = fastMathControl.getBoundingClientRect();
    const blockBounds = doc.getElementById("run-block-size").getBoundingClientRect();
    const toggleBounds = fastMath.closest("label").getBoundingClientRect();
    const checkboxBounds = fastMath.getBoundingClientRect();
    check(fastMathBounds.left >= blockBounds.right
      && Math.abs(toggleBounds.top - blockBounds.top) < 1
      && Math.abs(toggleBounds.height - blockBounds.height) < 1
      && checkboxBounds.width === 24 && checkboxBounds.height === 24
      && Math.abs(checkboxBounds.top + checkboxBounds.height / 2
        - blockBounds.top - blockBounds.height / 2) < 1,
      "the larger Fast Math checkbox aligns with the unloaded selectors' vertical center");
    fastMath.click();
    check(fastMathMessages().at(-1)?.enabled === false, "Fast Math edits request a host-owned compile option");
    view._onHostMessage({ type: "state", state: { fastMath: false } });
    check(!fastMath.checked, "host Fast Math publications update the checkbox");
    view._onHostMessage({ type: "state", state: { path: "loaded.onda" } });
    const before = fastMathMessages().length;
    fastMath.dispatchEvent(new view.Event("change", { bubbles: true }));
    check(fastMathControl.getClientRects().length === 0 && fastMathMessages().length === before,
      "loaded patches hide Fast Math and cannot change it through the unloaded control");
    for (const enabled of [true, false]) {
      view._onHostMessage({ type: "state", state: { fastMath: enabled } });
      check(doc.getElementById("status").textContent.endsWith(" · Fast Math") === enabled,
        "the loaded header shows fast math only when the active compile option is enabled");
    }
    view._onHostMessage({ type: "state", state: { path: "", supportsRunSettings: false } });
    check(fastMathControl.getClientRects().length > 0 && !fastMath.checked,
      "Fast Math remains available independently of sample-rate and block-size controls");
    view._onHostMessage({ type: "state", state: { supportsFastMath: false } });
    fastMath.dispatchEvent(new view.Event("change", { bubbles: true }));
    check(fastMathControl.hidden && fastMathMessages().length === before,
      "disabling the Fast Math capability hides the control and rejects edits");
  } finally {
    frame.remove();
  }
}
async function checkInputControls() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:absolute;left:-10000px;width:720px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    const send = state => view._onHostMessage({ type: "state", state });
    const section = doc.getElementById("input-controls-section");
    check(getComputedStyle(section).display === "none", "stock Onda hides Input Controls");
    const control = { name: "cv", label: "cutoff CV", type: "f32", value: 0.5,
      default: 0.25, rangeMin: 0, rangeMax: 1, scale: "linear", enabled: true };
    send({ path: "host.onda", inputControls: [control], inputControlLayout: "knobs",
      supportsInputControlSmoothing: true, inputControlSmoothingMs: 30,
      inputControlSmoothingDefaultMs: 30, paramLayout: "knobs", params: [{ ...control, name: "level" }] });
    check(getComputedStyle(section).display !== "none", "a host-provided input enables its section");
    doc.querySelector('[data-control-bank="inputControls"][data-param-layout="sliders"]').click();
    check(doc.getElementById("input-controls").dataset.layout === "sliders"
      && doc.getElementById("params").dataset.layout === "knobs", "input layouts are independent of Params");
    const number = doc.querySelector("#input-controls .param-number");
    number.focus(); number.value = "0.75";
    number.dispatchEvent(new Event("input", { bubbles: true }));
    send({ inputControls: [{ ...control, value: 0.2 }] });
    check(doc.querySelector("#input-controls .param-number") === number && number.value === "0.75",
      "input automation retains the numeric draft and DOM node");
    number.dispatchEvent(new Event("change", { bubbles: true })); number.blur();
    check(view.__testMessages.some(message => message.type === "setInputControl"
      && message.name === "cv" && message.value === 0.75 && message.commit), "input edits have a separate host command");
    const smooth = doc.getElementById("input-control-smoothing-ms");
    edit(smooth, "45"); smooth.dispatchEvent(new Event("change"));
    doc.getElementById("reset-input-controls").click();
    check(view.__testMessages.some(message => message.type === "setInputControlSmoothing" && message.milliseconds === 45)
      && view.__testMessages.some(message => message.type === "resetInputControls"), "input Smooth and Reset have independent host commands");
    send({ inputControls: [{ ...control, enabled: false, value: 0.4 }] });
    check(doc.querySelector("#input-controls .param-number").disabled
      && doc.querySelector("#input-controls .param-type").textContent === "External input"
      && doc.querySelector("#input-controls .param-slider").getAttribute("aria-disabled") === "true",
      "externally driven inputs disable every editor control");
    send({ inputControls: [{ ...control, rangeMin: 2, rangeMax: 2, value: 2, default: 2 }] });
    check(doc.querySelector("#input-controls .param-number").disabled
      && doc.querySelector("#input-controls .param-number").value === "2", "constant input domains render safely as read-only values");
    send({ inputControls: [] });
    check(getComputedStyle(section).display === "none", "unloading input controls hides the section");
  } finally { frame.remove(); }
}

async function checkResponsiveParameterGrid() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:absolute;left:-10000px;width:640px;height:2400px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    const params = Array.from({ length: 17 }, (_, index) => ({
      name: `parameter_${index}`, type: "f32", scalar: true, value: 0.5, default: 0.5,
      rangeMin: 0, rangeMax: 1, scale: "linear",
    }));
    view._onHostMessage({ type: "state", state: {
      running: true, connected: true, path: "many_parameters.onda", status: "Running",
      supportsTransport: true, supportsScope: true, supportsParamSmoothing: true,
      outputChannels: 2, paramSmoothingMs: 30, sampleRateHz: 48000, blockSamples: 512,
      inputControls: params.map(param => ({ ...param, name: `input_${param.name}` })),
      buffers: [], events: [], params: [...params, ...params.map((param, index) => ({
        ...param, name: `levels[${index}]`, array: { name: "levels", length: 17, index },
      }))],
    }});
    for (const layout of ["knobs", "sliders"]) {
      doc.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
      doc.querySelector(`[data-control-bank="inputControls"][data-param-layout="${layout}"]`).click();
      for (const [width, knobColumns, sliderColumns] of [
        [640, 4, 2], [800, 5, 3], [960, 6, 4], [1120, 7, 5], [1280, 8, 5],
        [1440, 8, 6], [1600, 8, 7], [1760, 8, 8], [3200, 8, 8],
        [1280, 8, 5], [640, 4, 2],
      ]) {
        frame.style.width = `${width}px`;
        await waitFrames(2);
        const expected = layout === "knobs" ? knobColumns : sliderColumns;
        for (const grid of doc.querySelectorAll(".param-list")) {
          const cards = [...grid.querySelectorAll(":scope > .param")]
            .map(card => card.getBoundingClientRect());
          const columns = cards.filter(rect => Math.abs(rect.top - cards[0].top) < 1).length;
          const rows = new Set(cards.map(rect => Math.round(rect.top)));
          check(cards.length === 17 && columns === expected && rows.size === Math.ceil(17 / expected),
            `${width}-pixel ${layout} grid, ${grid.id || "array"}: ${expected} readable controls per row`);
          check(cards.every(rect => rect.width >= (layout === "knobs" ? 140 : 200)),
            `the ${width}-pixel ${layout} grid preserves the minimum card width`);
        }
        check([...doc.querySelectorAll("#params :is(.param-number, .param-knob, .param-slider)")].every(control => {
          const rect = control.getBoundingClientRect();
          const card = control.closest(".param").getBoundingClientRect();
          return rect.left >= card.left && rect.right <= card.right
            && rect.top >= card.top && rect.bottom <= card.bottom;
        }), `number fields and ${layout} stay inside their cards at ${width} pixels`);
        check(doc.querySelector(".shell").scrollWidth === doc.querySelector(".shell").clientWidth,
          `the ${width}-pixel run window has no horizontal overflow`);
      }
    }
    frame.style.width = "720px";
    for (const layout of ["knobs", "sliders"]) {
      doc.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
      view._onHostMessage({ type: "state", state: { params } });
      await waitFrames(2);
      const width = doc.querySelector("#params .param").getBoundingClientRect().width;
      view._onHostMessage({ type: "state", state: { params: [
        { ...params[0], name: "frequency", unit: "Hz" },
        { ...params[1], name: "levels[0]", array: { name: "levels", length: 1, index: 0 } },
      ] }});
      await waitFrames(2);
      check([...doc.querySelectorAll("#params .param")].every(card =>
        Math.abs(card.getBoundingClientRect().width - width) < 1),
        `${layout}: sparse scalar and array groups keep the same widths as a full grid`);
      const name = doc.querySelector("#params .param-name");
      check(name.textContent === "frequency" && name.title === "frequency (Hz) (f32)"
        && name.nextElementSibling.textContent === "(Hz)"
        && name.parentElement.querySelector(".param-type").textContent === "f32",
        `${layout}: units and scalar types stay visible beside parameter names`);
    }
    const longName = "oscillator_frequency_with_a_long_parameter_name";
    view._onHostMessage({ type: "state", state: { params: [
      { ...params[0], name: longName, unit: "Hz" },
      { ...params[0], name: "unbounded", label: longName, unit: "dB", rangeMin: null, rangeMax: null },
      { ...params[1], name: "times[0]", label: longName, unit: "ms",
        array: { name: "times", length: 1, index: 0 } },
    ] }});
    for (const width of [320, 360, 400, 640, 720]) {
      frame.style.width = `${width}px`;
      for (const layout of ["knobs", "sliders"]) {
        doc.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
        await waitFrames(2);
        for (const head of doc.querySelectorAll("#params .param-head")) {
          const name = head.querySelector(".param-name");
          const unit = head.querySelector(".param-unit");
          const type = head.querySelector(".param-type");
          const headRect = head.getBoundingClientRect();
          const unitRect = unit.getBoundingClientRect();
          const typeRect = type.getBoundingClientRect();
          check(name.scrollWidth > name.clientWidth && unit.scrollWidth === unit.clientWidth
            && unitRect.left >= headRect.left && unitRect.right <= headRect.right + 1
            && name.getBoundingClientRect().right + 5 <= unitRect.left
            && type.textContent === "f32" && type.scrollWidth === type.clientWidth
            && unitRect.right + 5 <= typeRect.left && typeRect.right <= headRect.right + 1,
            `${width}-pixel ${layout}: the full ${unit.textContent} unit and type stay visible beside the truncated name`);
        }
      }
    }
  } finally {
    frame.remove();
  }
}

async function checkResponsiveMidiKeyboard() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:absolute;left:-10000px;width:720px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    view._onHostMessage({ type: "state", state: {
      running: true, connected: true, path: "keyboard.onda", supportsTransport: true,
      supportsScope: true, outputChannels: 2, midi: { available: true, noteOn: true },
      currentMidiInputDevice: "Computer Keyboard", events: [], params: [], buffers: [],
    }});
    const piano = doc.getElementById("piano");
    const dock = doc.getElementById("midi-keyboard");
    const octave = doc.getElementById("midi-octave");
    const messages = () => view.__testMessages.filter(message => message.type === "midiNote");
    const resize = async (width, height = 840) => {
      frame.style.width = `${width}px`;
      frame.style.height = `${height}px`;
      await waitFrames(3);
    };
    for (const [width, octaves] of [
      [320, 2], [720, 2], [960, 2], [1040, 3], [1440, 4], [1760, 5],
      [2560, 5], [4000, 5], [1040, 3], [720, 2],
    ]) {
      await resize(width);
      const white = [...piano.querySelectorAll(".white")];
      const black = [...piano.querySelectorAll(".black")];
      const rect = piano.getBoundingClientRect();
      check(white.length === octaves * 7 && black.length === octaves * 5,
        `the ${width}-pixel MIDI keyboard shows ${octaves} full octaves`);
      check(rect.height >= 104 && rect.height <= 156
        && white.every(key => key.getBoundingClientRect().width <= 72),
        `the ${width}-pixel MIDI keyboard keeps its height and key width bounded`);
      check(Math.abs(rect.left + rect.width / 2 - dock.getBoundingClientRect().width / 2) < 1
        && doc.querySelector(".shell").scrollWidth === doc.querySelector(".shell").clientWidth,
        `the ${width}-pixel MIDI keyboard stays centered without horizontal overflow`);
      check(black.every((key, index) => {
        const boundary = Math.floor(index / 5) * 7 + [1, 2, 4, 5, 6][index % 5];
        const blackRect = key.getBoundingClientRect();
        return Math.abs(blackRect.left + blackRect.width / 2
          - white[boundary - 1].getBoundingClientRect().right) < 0.75;
      }), `the ${width}-pixel MIDI keyboard aligns black keys with the white-key boundaries`);
    }
    check(piano.getBoundingClientRect().height === 104,
      "returning to the default run window restores the compact two-octave piano");
    const firstKey = piano.querySelector('[data-note-offset="0"]');
    firstKey.focus();
    view._onHostMessage({ type: "computerKey", code: "KeyA", pressed: true });
    const heldCount = messages().length;
    await resize(1760);
    check(doc.activeElement === firstKey && firstKey.classList.contains("active")
      && piano.querySelector('[data-note-offset="0"]') === firstKey
      && messages().length === heldCount,
      "expanding the piano preserves retained keys, focus, and held computer notes");
    const highKey = piano.querySelector('[data-note-offset="48"]');
    const pointer = pointerControl(highKey, piano);
    pointer("pointerdown", 10);
    check(highKey.classList.contains("active") && messages().at(-1).key === 108
      && messages().at(-1).pressed,
      "newly visible octaves play their actual MIDI notes");
    await resize(720);
    check(!highKey.isConnected && messages().length === heldCount + 2
      && messages().at(-1).key === 108 && !messages().at(-1).pressed
      && firstKey.classList.contains("active"),
      "shrinking the piano releases a disappearing pointer note while retaining other holds");
    pointer("lostpointercapture", 10);
    check(messages().length === heldCount + 2,
      "removing a held piano key sends exactly one note-off");
    view._onHostMessage({ type: "computerKey", code: "KeyA", pressed: false });
    check(!firstKey.classList.contains("active"), "retained computer notes still release after resizing");
    await resize(1760, 400);
    check(piano.getBoundingClientRect().height === 104
      && doc.querySelector(".shell").clientHeight > 0,
      "short run windows reserve space for controls above the piano");
    await resize(720, 620);
    view._onHostMessage({ type: "state", state: { supportsViewState: true }});
    const toggle = doc.getElementById("midi-toggle");
    const expandedHeight = dock.getBoundingClientRect().height;
    view._onHostMessage({ type: "computerKey", code: "KeyA", pressed: true });
    toggle.click();
    await waitFrames(2);
    check(piano.hidden && toggle.getAttribute("aria-expanded") === "false"
      && dock.getBoundingClientRect().height < expandedHeight - 100
      && !messages().at(-1).pressed,
      "folding the MIDI dock releases held notes and gives space back to the controls");
    const folded = view.__testMessages.filter(message => message.type === "viewState").at(-1).state;
    view._onHostMessage({ type: "state", state: { running: false } });
    view._onHostMessage({ type: "state", state: { running: true } });
    check(piano.hidden, "Play/Stop preserves the folded MIDI dock");
    view._onHostMessage({ type: "computerKey", code: "KeyS", pressed: true });
    view._onHostMessage({ type: "state", state: { status: "Running" } });
    check(messages().at(-1).pressed, "computer MIDI keeps playing through updates with the piano folded");
    view._onHostMessage({ type: "computerKey", code: "KeyS", pressed: false });
    toggle.click();
    view._onHostMessage({ type: "viewState", state: folded });
    await waitFrames(3);
    check(piano.hidden, "restoring the run view retains its MIDI fold state");
    toggle.click();
    await waitFrames(2);
    check(!piano.hidden && toggle.getAttribute("aria-expanded") === "true",
      "the folded MIDI dock can be reopened");
    for (const [value, octaves, lowest, highest] of [[7, 2, 96, 119], [-1, 10, 0, 119]]) {
      octave.value = String(value);
      octave.dispatchEvent(new view.Event("change", { bubbles: true }));
      await resize(4000);
      const notes = [...piano.children].map(key => Number(key.getAttribute("aria-label").slice(10)));
      check(notes.length === octaves * 12 && new Set(notes).size === notes.length
        && Math.min(...notes) === lowest && Math.max(...notes) === highest,
        `the expanded octave-${value} piano contains distinct valid MIDI notes`);
    }
    await resize(720);
    view._onHostMessage({ type: "state", state: { midiKeyboardInteractive: false }});
    view._onHostMessage({ type: "midiActivity", activeNotes: [48] });
    await resize(1760);
    check([...piano.children].every(key => key.tabIndex === -1)
      && piano.querySelector('[data-note-offset="48"]').classList.contains("active"),
      "expanded monitor keyboards keep new keys passive and display external notes");
  } finally {
    frame.remove();
  }
}

async function checkHomepageLayout() {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:absolute;left:-10000px;height:840px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/homepage-layout";
  document.body.append(frame);
  try {
    await loaded;
    const doc = frame.contentDocument;
    for (const width of [320, 360, 720, 1440]) {
      frame.style.width = `${width}px`;
      await waitFrames(2);
      const block = doc.querySelector(".home-example-code").getBoundingClientRect();
      const code = doc.querySelector(".home-example-code pre").getBoundingClientRect();
      const button = doc.querySelector(".home-example-code .primary-button").getBoundingClientRect();
      check(code.top === block.top && button.top >= code.top && button.bottom < code.bottom
        && button.left >= code.left && button.right <= code.right,
        `the homepage playground button overlays the code without reserving a row at ${width} pixels`);
      const text = doc.querySelector(".home-example-code code").firstChild;
      const firstLine = doc.createRange();
      firstLine.setStart(text, 0);
      firstLine.setEnd(text, text.textContent.indexOf("\n"));
      check(firstLine.getBoundingClientRect().right <= button.left,
        `the homepage playground button leaves the first import readable at ${width} pixels`);
    }
  } finally {
    frame.remove();
  }
}

async function checkPlaygroundLayout() {
  const controlGeometry = new Map();
  const params = Array.from({ length: 17 }, (_, index) => ({
    name: `parameter_${index}`, type: "f32", scalar: true, value: 0.5, default: 0.5,
    rangeMin: 0, rangeMax: 1, scale: "linear",
  }));
  for (const host of ["website", "example"]) {
    const stackedWidth = 1280;
    const frame = document.createElement("iframe");
    frame.style.cssText = "position:absolute;left:-10000px;width:1440px;height:1080px;border:0";
    const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
    frame.src = `/${host}-layout`;
    document.body.append(frame);
    try {
      await loaded;
      const doc = frame.contentDocument;
      const runFrame = doc.querySelector("[data-run-view]");
      const run = runFrame.contentWindow;
      const runDoc = run.document;
      run._onHostMessage({ type: "state", state: {
        running: true, connected: true, path: "main.onda", status: "Running",
        outputChannels: 2, params, buffers: [], events: [], logRevealed: false,
        midi: { available: true, noteOn: true }, currentMidiInputDevice: "Computer Keyboard",
        midiInputDevices: ["Computer Keyboard"],
        supportsTransport: true, supportsSourceSelection: false, supportsDeviceSelection: false,
        supportsParamSmoothing: true, paramSmoothingMs: 30, supportsScope: true,
        sampleRateHz: 48000, blockSamples: 512,
      }});
      const editor = doc.querySelector("[data-editor]");
      check(getComputedStyle(doc.querySelector("[data-editor-font-size]")).backgroundColor
        === getComputedStyle(doc.querySelector("[data-sample-rate]")).backgroundColor,
        `${host}: the font number field keeps the compile controls' themed background while editing text`);
      if (host === "website") {
        for (const theme of ["light", "dark"]) {
          doc.documentElement.dataset.theme = theme;
          check(getComputedStyle(doc.documentElement).colorScheme === theme,
            `website: native controls follow the selected ${theme} theme`);
        }
      }
      for (const width of [280, 320, 360, 390, 399, 400, 480, 720, 320, 480]) {
        frame.style.width = `${width}px`;
        await waitFrames(3);
        const grid = doc.querySelector(".play-control-grid").getBoundingClientRect();
        const controls = [...doc.querySelectorAll(".play-control-grid input, .play-control-grid select")];
        const bounds = controls.map(control => control.getBoundingClientRect());
        check(bounds.every(rect => Math.abs(rect.top - bounds[0].top) < 1
          && rect.left >= grid.left - 1 && rect.right <= grid.right + 1)
          && bounds.every((rect, index) => index === 0 || rect.left >= bounds[index - 1].right),
          `${host}: compile settings shrink together on one row at ${width} pixels`);
        const context = doc.createElement("canvas").getContext("2d");
        for (const select of controls.filter(control => control.tagName === "SELECT")) {
          const style = getComputedStyle(select);
          context.font = `${style.fontSize} ${style.fontFamily}`;
          check([...select.options].every(option =>
            option.label === (width < 400 ? option.value : option.textContent)
            && option.getAttribute("aria-label") === option.textContent
            && context.measureText(option.label).width <= select.clientWidth
              - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) - 24),
          `${host}: select values fit and retain accessible units at ${width} pixels`);
        }
      }
      let desktopEditorWidth;
      let desktopEditorHeight;
      let previousWidePanes;
      for (const [width, height, websiteColumns, exampleColumns] of [
        [320, 720], [480, 840], [768, 900], [1024, 900], [1280, 900], [1281, 900],
        [1366, 768], [1366, 900], [1440, 1080], [1441, 1080], [1536, 1080], [1706, 1080],
        [1920, 1080], [2000, 1200, [5, 3]], [2560, 1440, [5, 3], [5, 3]],
        [2844, 1600, [5, 4], [5, 4]], [3413, 1800, [7, 5], [7, 5]],
        [3840, 2000, [7, 5], [7, 5]], [5120, 2600, [8, 7], [8, 7]],
        [6400, 3300, [8, 8], [8, 8]],
      ]) {
        frame.style.width = `${width}px`;
        frame.style.height = `${height}px`;
        await waitFrames(3);
        const editorPanel = editor.closest("section").getBoundingClientRect();
        const runPanel = runFrame.closest("aside").getBoundingClientRect();
        const status = doc.querySelector("[data-status]").getBoundingClientRect();
        const shortcuts = doc.querySelector("[aria-label='Playground keyboard shortcuts']")
          .getBoundingClientRect();
        check(status.top < shortcuts.bottom && shortcuts.top < status.bottom
          && status.right <= shortcuts.left,
          `${host}: Playing and the shortcuts stay on one line at ${width} pixels`);
        check([...doc.querySelectorAll("[aria-label='Playground keyboard shortcuts'] kbd")]
          .every(hint => hint.getBoundingClientRect().width > 0),
          `${host}: both keyboard shortcut hints remain visible at ${width} pixels`);
        if (width >= 480) {
          check([...doc.querySelectorAll("[aria-label='Playground keyboard shortcuts'] > span")]
            .every(shortcut => {
              const label = shortcut.firstElementChild.getBoundingClientRect();
              const keys = shortcut.lastElementChild.getBoundingClientRect();
              return label.right <= keys.left && label.top < keys.bottom && keys.top < label.bottom;
            }), `${host}: shortcut labels and keys stay horizontal when space is available at ${width} pixels`);
        }
        check(doc.documentElement.scrollWidth <= doc.documentElement.clientWidth
          && runDoc.querySelector(".shell").scrollWidth === runDoc.querySelector(".shell").clientWidth,
          `${host}: the ${width}-pixel playground and run controls avoid horizontal overflow`);
        check(width <= stackedWidth ? runPanel.top >= editorPanel.bottom :
          Math.abs(runPanel.top - editorPanel.top) < 1 && runPanel.left >= editorPanel.right,
        `${host}: the workspace ${width <= stackedWidth ? "stacks" : "uses two columns"} at ${width} pixels`);
        check(doc.querySelector(".cm-editor").getBoundingClientRect().height >= editor.clientHeight - 1,
          `${host}: the real CodeMirror editor fills its available height at ${width} pixels`);
        const geometry = JSON.stringify([
          ".project-file", ".project-file-select", ".project-file-add",
          ".project-file-menu-trigger", ".project-file-actions button",
        ].map(selector => {
          const style = getComputedStyle(doc.querySelector(selector));
          return ["fontSize", "lineHeight", "paddingTop", "paddingBottom", "paddingLeft",
            "paddingRight", "height", selector === ".project-file-actions button" ? "minWidth" : "width", "borderTopWidth", "borderRadius"]
            .map(property => style[property]);
        }));
        if (host === "website") controlGeometry.set(width, geometry);
        else check(controlGeometry.get(width) === geometry,
          `project tabs and actions have matching geometry in both playgrounds at ${width} pixels`);
        const expectedEditorFont = width <= 720 && !frame.contentWindow.matchMedia("(pointer: coarse)").matches ? 12 : 16;
        check(getComputedStyle(doc.querySelector(".cm-content")).fontSize === `${expectedEditorFont}px`
          && getComputedStyle(doc.querySelector(".cm-lineNumbers")).fontSize === `${expectedEditorFont}px`
          && doc.querySelector("[data-editor-font-size]").value === String(expectedEditorFont),
          `${host}: the editor, line numbers, and font control use ${expectedEditorFont} pixels at ${width} pixels`);
        if (width > stackedWidth) {
          check(editorPanel.bottom <= height && runPanel.bottom <= height,
            `${host}: the editor and full run pane fit within the desktop viewport`);
          check(editorPanel.width > runPanel.width
            && editor.getBoundingClientRect().width > runFrame.getBoundingClientRect().width,
            `${host}: the editor stays wider than the run view at ${width} pixels`);
        }
        for (const layout of ["knobs", "sliders"]) {
          runDoc.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
          await waitFrames(2);
          if (width > stackedWidth) {
            const cards = [...runDoc.querySelectorAll("#params .param")]
              .map(card => card.getBoundingClientRect());
            const columns = cards.filter(rect => Math.abs(rect.top - cards[0].top) < 1).length;
            const wide = width >= 1706;
            const expected = (host === "website" ? websiteColumns : exampleColumns)
              ?? [wide ? 4 : 3, wide ? 3 : 2];
            check(columns === expected[layout === "knobs" ? 0 : 1],
              `${host}: the ${width}-pixel desktop fits ${columns} ${layout} per row`);
          }
        }
        if (width === 1366 && height === 768) {
          runDoc.querySelector('#params-section [data-param-layout="knobs"]').click();
          await waitFrames(2);
          const card = runDoc.querySelector("#params .param").getBoundingClientRect();
          const shell = runDoc.querySelector(".shell").getBoundingClientRect();
          check(runDoc.getElementById("scope-canvas").getBoundingClientRect().height === 96
            && card.bottom <= shell.bottom,
            `${host}: compact scope leaves a full knob row visible on a 1366 × 768 laptop`);
        }
        if (width === 1920) {
          desktopEditorWidth = editor.clientWidth;
          desktopEditorHeight = editor.clientHeight;
        } else if (width === 2560) {
          check(editor.clientWidth > desktopEditorWidth,
            `${host}: a wider desktop gives its extra space to the editor`);
          check(editor.clientHeight > desktopEditorHeight,
            `${host}: a taller desktop increases the editor height`);
        }
        if (width >= 2560) {
          const panes = {
            editorWidth: editor.clientWidth, editorHeight: editor.clientHeight,
            runWidth: runFrame.getBoundingClientRect().width, runHeight: runPanel.height,
          };
          if (previousWidePanes) {
            check(Object.entries(panes).every(([key, value]) => value > previousWidePanes[key]),
              `${host}: both panes grow in width and height at ${width} pixels, beyond their old caps`);
          }
          previousWidePanes = panes;
        }
      }
      const shortcutHelp = doc.querySelector("[aria-label='Playground keyboard shortcuts']");
      shortcutHelp.style.width = "160px";
      await waitFrames(2);
      check([...shortcutHelp.children].every(shortcut =>
        shortcut.lastElementChild.getBoundingClientRect().top >= shortcut.firstElementChild.getBoundingClientRect().bottom),
        `${host}: shortcut labels wrap above their keys when the available row width is constrained`);
      shortcutHelp.style.width = "";
      const fontInput = doc.querySelector("[data-editor-font-size]");
      const storageKey = "onda.browser-ide.editor-font-size.v1";
      const storedFont = () => frame.contentWindow.localStorage.getItem(storageKey);
      const typeFont = value => {
        fontInput.focus();
        fontInput.value = value;
        fontInput.dispatchEvent(new frame.contentWindow.Event("input", { bubbles: true }));
      };
      const cancelFont = () => fontInput.dispatchEvent(new frame.contentWindow.KeyboardEvent(
        "keydown", { key: "Escape", bubbles: true, cancelable: true },
      ));
      const resizeFont = async width => {
        frame.style.width = `${width}px`;
        await waitFrames(3);
      };
      await resizeFont(1440);
      typeFont("");
      fontInput.blur();
      fontInput.focus();
      cancelFont();
      check(storedFont() === null,
        `${host}: cancelling an unchanged font field preserves automatic sizing`);
      typeFont("23");
      check(storedFont() === "23"
        && getComputedStyle(doc.querySelector(".cm-content")).fontSize === "23px",
        `${host}: font edits preview and persist the selected size`);
      cancelFont();
      await resizeFont(600);
      const automaticFont = frame.contentWindow.matchMedia("(pointer: coarse)").matches ? 16 : 12;
      check(storedFont() === null && fontInput.value === String(automaticFont)
        && getComputedStyle(doc.querySelector(".cm-content")).fontSize === `${automaticFont}px`,
        `${host}: cancelling a preview restores automatic sizing across breakpoints`);
      typeFont("20");
      fontInput.blur();
      for (const invalid of ["0x10", "0b10000", "16.5", "16.0", "1.6e1", "5", "41", "Infinity"]) {
        typeFont(invalid);
        fontInput.blur();
        check(storedFont() === "20" && fontInput.value === "20"
          && getComputedStyle(doc.querySelector(".cm-content")).fontSize === "20px",
          `${host}: invalid font draft ${invalid} preserves the integer font preference`);
      }
      for (const size of [6, 8, 12, 40]) {
        typeFont(String(size));
        check(storedFont() === String(size)
          && getComputedStyle(doc.querySelector(".cm-content")).fontSize === `${size}px`
          && getComputedStyle(doc.querySelector(".cm-lineNumbers")).fontSize === `${size}px`,
          `${host}: the editor and line numbers honor an explicit ${size}-pixel font`);
        cancelFont();
      }
      typeFont("28");
      cancelFont();
      check(storedFont() === "20" && fontInput.value === "20"
        && getComputedStyle(doc.querySelector(".cm-content")).fontSize === "20px",
        `${host}: cancelling a fixed font edit restores its previous preference`);
      await resizeFont(1440);
      check(fontInput.value === "20",
        `${host}: explicit font sizes survive viewport changes`);
      fontInput.dispatchEvent(new frame.contentWindow.MouseEvent("dblclick", { bubbles: true }));
      check(storedFont() === null && fontInput.value === "16",
        `${host}: double-click restores automatic font sizing`);
    } finally {
      frame.remove();
    }
  }
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
  check(matchMedia("(pointer: coarse)").matches === window.__testCoarsePointer,
    "the browser uses the requested primary pointer capability");
  await checkHomepageLayout();
  await checkDefaultLayout();
  await checkContentBoxScope();
  await checkEmbeddedHosts();
  await checkThemedFormControls();
  await checkInputControls();
  await checkResponsiveParameterGrid();
  await checkResponsiveMidiKeyboard();
  await checkPlaygroundLayout();
  const shell = document.querySelector(".shell");
  const scrollNode = shell;
  const paramsList = document.getElementById("params");
  const resetButton = document.getElementById("reset-params");
  const ondaVersion = document.getElementById("onda-version");
  check(!ondaVersion.hidden && ondaVersion.querySelector("#onda-version-number").textContent === "0.0.0-test",
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
  check(header.querySelector(".section-actions").firstElementChild === resetButton
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
    && document.querySelector('#params-section [data-param-layout="knobs"]').getAttribute("aria-pressed") === "true"
    && document.querySelector("#params .param-knob"),
    "the run view defaults to knobs without a saved layout preference");
  check(getComputedStyle(shell).visibility === "visible",
    "a first host state without view state reveals the run view");
  check(ondaVersion.querySelector("#onda-version-number").textContent === "0.0.1-browser",
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
  const collapsedScope = document.getElementById("scope-section");
  const scopeBounds = collapsedScope.getBoundingClientRect();
  check([...collapsedScope.querySelector(".section-heading").children].every(node => {
    const rect = node.getBoundingClientRect();
    return Math.abs(rect.top + rect.height / 2 - scopeBounds.top - scopeBounds.height / 2) < 1;
  }), "the collapsed Scope arrow and title are vertically centered");
  check(Math.abs(scopeBounds.height
    - document.getElementById("params-section").getBoundingClientRect().height) < 1,
  "collapsed Scope and Params use the same header height");
  for (const section of ["scope", "events", "params"]) {
    document.getElementById(`${section}-toggle`).click();
  }
  send({ supportsScope: false });
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

  const integer = document.querySelector('#events input[pattern]');
  edit(integer, "-");
  integer.setSelectionRange(1, 1);
  refresh();
  check(document.activeElement === integer && integer.value === "-"
    && integer.selectionStart === 1,
    "incomplete i64 drafts and caret survive refreshes");
  send({ events, resetEventArguments: true });
  check(document.querySelector('#events input[pattern]').value === "0",
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
    const input = document.querySelectorAll('#params input.param-number')[index];
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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

  document.querySelector('#params-section [data-param-layout="knobs"]').click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
    const [gain, offset] = document.querySelectorAll('#params input.param-number');
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
    document.querySelector('#params-section [data-param-layout="knobs"]').click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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
  document.querySelector('#params-section [data-param-layout="sliders"]').click();
  const stepped = [
    { name: "mode", type: "i32", value: 0, default: 0,
      rangeMin: 0, rangeMax: 10, scale: "linear", step: 2, stepCount: 5 },
    { name: "pitch", type: "f64", value: 100, default: 100,
      rangeMin: 20, rangeMax: 1000, scale: "log" },
  ];
  send({ params: stepped });
  const [mode, pitch] = document.querySelectorAll('#params input.param-number');
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
  document.querySelector('#params-section [data-param-layout="knobs"]').click();
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
    if (replacement && replacement.classList.contains("param-number")) {
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
      document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
      await waitFrames(1);
      const boolCards = [...paramsNode.querySelectorAll(".param-bool")];
      for (const card of boolCards) {
        const name = card.querySelector(".param-name");
        const head = card.querySelector(".param-head");
        check(name.textContent === longName
          ? name.scrollWidth > name.clientWidth
          : name.scrollWidth <= name.clientWidth,
        `${layout} at ${width}px truncates ${name.textContent} only when it exceeds the available width`);
        const type = card.querySelector(".param-type");
        check(type.textContent === "bool" && name.title.endsWith("(bool)")
          && name.getBoundingClientRect().right <= type.getBoundingClientRect().left
          && type.getBoundingClientRect().right <= head.getBoundingClientRect().right + 1,
          `${layout} at ${width}px keeps the name readable and its type visible beside it`);
        const toggle = card.querySelector('.param-toggle');
        const checkbox = toggle.querySelector('input');
        const box = checkbox.getBoundingClientRect();
        const controls = card.querySelector('.param-controls').getBoundingClientRect();
        check(toggle.textContent.trim() === '' && checkbox.getAttribute('aria-label') === name.textContent
          && box.width === 24 && box.height === 24
          && Math.abs(box.left + box.width / 2 - controls.left - controls.width / 2) < 1
          && Math.abs(box.top + box.height / 2 - controls.top - controls.height / 2) < 1,
          `${layout} at ${width}px centers boolean checkboxes without redundant value text`);
      }
      const number = paramsNode.querySelector('input.param-number');
      const numericCard = number.closest(".param");
      check(number.clientWidth > 0
        && number.getBoundingClientRect().right <= numericCard.getBoundingClientRect().right
        && (layout === "knobs" || numericCard.querySelector(".param-name")
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
    && Number(arrayGroups[0].querySelector('input.param-number').value) === 3,
    "expanding an array shows values updated while collapsed");
  arrayGroups[0].querySelector('input.param-number').dispatchEvent(
    new MouseEvent("dblclick", { bubbles: true, cancelable: true }),
  );
  check(paramMessages().at(-1)?.name === "f32Values[0]" && paramMessages().at(-1)?.value === 0,
    "double-click resets only the addressed parameter array element");
  arrayGroups[0].querySelector("summary").click();
  for (const layout of ["knobs", "sliders"]) {
    document.querySelector(`#params-section [data-param-layout="${layout}"]`).click();
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

  send({ running: true, supportsScope: true, outputChannels: 1 });
  const scopeCanvas = document.getElementById("scope-canvas");
  window._onHostMessage({ type: "scopeData", channels: 1, samples: Array(1024).fill(0) });
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const initialScopeWidth = scopeCanvas.width;
  scopeCanvas.style.width = "401px";
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  check(initialScopeWidth > 0 && scopeCanvas.width !== initialScopeWidth,
    "scope backing canvas follows layout changes without new audio data");
  scopeCanvas.style.width = "";
  const scopeWaveform = Array.from({ length: 1024 }, (_, index) => Math.sin(index * 0.03) * 0.7);
  window._onHostMessage({ type: "scopeData", channels: 1, samples: scopeWaveform });
  await waitFrames(2);
  const activeScope = scopeCanvas.toDataURL();
  const scopeBeforeStop = document.getElementById("scope-section").getBoundingClientRect();
  const paramsBeforeStop = document.getElementById("params-section").getBoundingClientRect();
  send({ running: false, connected: false, status: "Stopped" });
  await waitFrames(2);
  const stoppedScope = scopeCanvas.toDataURL();
  const scopeAlpha = () => scopeCanvas.getContext("2d")
    .getImageData(0, 0, scopeCanvas.width, scopeCanvas.height).data
    .filter((_, index) => index % 4 === 3);
  check(scopeAlpha().some(alpha => alpha > 0) && scopeAlpha().every(alpha => alpha < 64),
    "stopping leaves only the faint gray reference guide, without a bright waveform or zero trace");
  check(getComputedStyle(document.getElementById("scope-section")).display === "block"
    && stoppedScope !== activeScope
    && document.getElementById("scope-section").getBoundingClientRect().height === scopeBeforeStop.height
    && document.getElementById("params-section").getBoundingClientRect().top === paramsBeforeStop.top,
    "Stop keeps the scope and parameter layout in place while removing the waveform");
  window._onHostMessage({ type: "scopeData", channels: 1, samples: Array(1024).fill(0) });
  await waitFrames(2);
  check(scopeCanvas.toDataURL() === stoppedScope,
    "a late silent scope capture cannot add a bright zero trace while stopped");
  window._onHostMessage({ type: "scopeData", channels: 0, samples: [] });
  await waitFrames(2);
  check(scopeCanvas.toDataURL() === stoppedScope,
    "clearing captured samples keeps the gray guide for the known output channel");
  send({ path: "another-scope.onda" });
  await waitFrames(2);
  check(scopeCanvas.toDataURL() === stoppedScope,
    "selecting another source retains only the reference guide while stopped");
  send({ path: "test.onda", running: true, connected: true, status: "Active" });
  window._onHostMessage({ type: "scopeData", channels: 1, samples: scopeWaveform });
  await waitFrames(2);
  check(scopeCanvas.toDataURL() === activeScope,
    "playing again restores the waveform over the reference guide");

  send({ midi: { noteOn: true }, currentMidiInputDevice: "Computer Keyboard" });
  const octaveInput = document.getElementById("midi-octave");
  const piano = document.getElementById("piano");
  piano.style.width = "210px";
  await waitFrames(1);
  const whiteKeys = [...piano.querySelectorAll(".piano-key.white")];
  const blackKeys = [...piano.querySelectorAll(".piano-key.black")];
  check(blackKeys.every((blackKey, index) => {
    const blackCenter = blackKey.getBoundingClientRect().left
      + blackKey.getBoundingClientRect().width / 2;
    const boundary = Math.floor(index / 5) * 7 + [1, 2, 4, 5, 6][index % 5];
    const whiteBoundary = whiteKeys[boundary - 1]
      .getBoundingClientRect().right;
    return Math.abs(blackCenter - whiteBoundary) < 0.75;
  }), "black piano keys remain aligned when the keyboard is narrow");
  piano.style.width = "";
  const key = (code, type = "keydown", target = document) => target.dispatchEvent(
    new KeyboardEvent(type, { code, bubbles: true, cancelable: true }),
  );
  send({ running: false, connected: false });
  const midiMessages = () => window.__testMessages.filter(message => message.type === "midiNote");
  const stoppedMessageCount = midiMessages().length;
  const stoppedPointer = pointerControl(whiteKeys[0], piano);
  stoppedPointer("pointerdown", 10);
  check(whiteKeys[0].classList.contains("active") && !document.getElementById("midi-keyboard").hidden,
    "stopped piano clicks keep the dock visible and highlight the pressed key");
  stoppedPointer("pointerup", 10);
  key("KeyA");
  check(whiteKeys[0].classList.contains("active"), "computer MIDI keys also highlight while stopped");
  key("KeyA", "keyup");
  check(!whiteKeys[0].classList.contains("active") && midiMessages().length === stoppedMessageCount,
    "stopped piano and computer key presses send no processor events");
  send({ running: true, connected: true });
  check(midiMessages().length === stoppedMessageCount, "restarting does not replay stopped MIDI clicks");
  for (const inactive of [{ running: false, connected: false }, { running: true, connected: false }]) {
    for (const pointer of [false, true]) {
      send(inactive);
      const count = midiMessages().length;
      if (pointer) stoppedPointer("pointerdown", 10);
      else key("KeyA");
      send({ running: true, connected: true });
      check(whiteKeys[0].classList.contains("active"), "resuming preserves a stopped key's visual hold");
      if (pointer) stoppedPointer("pointerup", 10);
      else key("KeyA", "keyup");
      check(midiMessages().length === count,
        "releasing a stopped or disconnected hold after resuming sends no unmatched MIDI note-off");
    }
  }
  const velocityNumber = document.getElementById("midi-velocity-value");
  const octaveRect = octaveInput.getBoundingClientRect();
  const velocityRect = velocityNumber.getBoundingClientRect();
  check(octaveRect.width === 56 && velocityRect.width === octaveRect.width
    && octaveRect.height === 26 && velocityRect.height === octaveRect.height
    && !document.querySelector(".midi-control .param-slider"),
    "MIDI octave and velocity use matching number boxes without a velocity slider");
  check(velocityNumber.readOnly && velocityNumber.dataset.draggable !== undefined,
    "MIDI velocity uses the shared draggable number control");
  edit(velocityNumber, "0.81");
  velocityNumber.blur();
  const velocityPointer = pointerControl(velocityNumber);
  velocityPointer("pointerdown", 100);
  velocityPointer("pointermove", 120);
  velocityPointer("pointerup", 120);
  check(velocityNumber.value === "0.86",
    "MIDI velocity number drags use the ranged sensitivity");
  velocityPointer("pointerdown", 100);
  velocityPointer("pointermove", 150, { shiftKey: true });
  velocityPointer("pointerup", 150, { shiftKey: true });
  check(velocityNumber.value === "0.87", "MIDI velocity supports fine number dragging");
  edit(velocityNumber, "0.42");
  check(document.activeElement === velocityNumber, "MIDI velocity accepts focus for typing");
  velocityNumber.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  check(velocityNumber.value === "0.42" && velocityNumber.readOnly,
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
    && Math.abs(window.__testMessages.at(-1).velocity - 0.81) < 1e-6
    && window.__testMessages.at(-1).pressed,
    "computer keyboard plays with the selected octave and velocity");
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
  octaveInput.dispatchEvent(new Event("change", { bubbles: true }));
  key("KeyX");
  check(octaveInput.value === "7", "octave shortcuts respect the upper limit");
  octaveInput.value = "-1";
  octaveInput.dispatchEvent(new Event("change", { bubbles: true }));
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
  edit(velocityNumber, "0.82");
  velocityNumber.blur();
  check(velocityNumber.value === "0.82",
    "velocity number edits set the value saved with the view state");
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
  paramsList.style.minHeight = `${window.innerHeight + 400}px`;
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
