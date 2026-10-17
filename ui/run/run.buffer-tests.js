// Real browser coverage for OS drop routing and playground File transfers.
export async function testBufferDrops(check, waitFrames) {
  for (const browser of [false, true]) {
    const frame = document.createElement("iframe");
    frame.style.cssText = "position:fixed;left:0;top:0;width:720px;height:840px;border:0";
    const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
    frame.src = browser ? "/browser-layout" : "/layout";
    const messages = [];
    const receive = event => {
      if (event.source === frame.contentWindow && event.data?.__ondaRunWebview) {
        messages.push(event.data.message);
      }
    };
    window.addEventListener("message", receive);
    document.body.append(frame);
    try {
      await loaded;
      const view = frame.contentWindow;
      const doc = view.document;
      const publish = message => {
        if (browser) view.postMessage({ __ondaRunHost: true, message }, location.origin);
        else view._onHostMessage(message);
      };
      const waveform = { minValue: -0.5, maxValue: 1.5, minimums: [-0.5, 0], maximums: [1, 1.5] };
      const buffers = [
        { name: "left", type: "f32[]" },
        { name: "right", type: "f32[]", loadedFrames: 4, loadedChannels: 1,
          loadedSampleRate: 48000, loadedPath: "/samples/original.wav", waveform },
      ];
      const state = update => publish({ type: "state", state: update });
      const bindings = () => (browser ? messages : view.__testMessages)
        .filter(message => message.type === "bindBufferFile");
      const cards = () => [...doc.querySelectorAll(".buffer-card")];
      const checkHighlight = () => {
        const [left, right] = cards().map(card => view.getComputedStyle(card));
        check(left.backgroundColor !== right.backgroundColor
          && left.borderTopColor !== right.borderTopColor,
          "the target buffer has a distinct background and border during a file drag");
      };
      state({ path: "sampler.onda", running: false, connected: false,
        supportsScope: false, buffers, params: [], events: [] });
      await waitFrames(2, view);
      check(cards()[0].textContent.includes("Choose or drop a file to bind the buffer"),
        "unbound buffers advertise file drops");
      check(cards()[1].querySelector("canvas").width > 0
        && cards()[1].textContent.includes("original.wav")
        && cards()[1].textContent.includes("48 kHz"),
        "loaded buffer cards retain their filename, metadata, and waveform");

      if (browser) {
        const dispatch = (type, target, files = [], text = null) => {
          const dataTransfer = new view.DataTransfer();
          for (const file of files) dataTransfer.items.add(file);
          if (text !== null) dataTransfer.setData("text/plain", text);
          const rect = target.getBoundingClientRect();
          const event = new view.DragEvent(type, {
            dataTransfer, bubbles: true, cancelable: true,
            clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
          });
          target.dispatchEvent(event);
          return event;
        };
        const file = new view.File([new Uint8Array([1, 2, 3])], "clip.WAV", { type: "audio/wav" });
        const typed = new view.File([new Uint8Array([4, 5])], "typed.ondabuffer");
        let label = cards()[1].querySelector(".param-name");
        const over = dispatch("dragover", label, [file]);
        check(over.defaultPrevented && cards()[1].classList.contains("file-hover")
          && !cards()[0].classList.contains("file-hover"),
          "a browser file drag highlights only the card beneath a nested element");
        checkHighlight();
        label.dispatchEvent(new view.DragEvent("dragleave", {
          bubbles: true, relatedTarget: cards()[1].querySelector(".buffer-file"),
        }));
        check(cards()[1].classList.contains("file-hover"),
          "moving between card children preserves its drop highlight");
        state({ status: "background update" });
        await waitFrames(2, view);
        check(cards()[1].classList.contains("file-hover")
          && !cards()[0].classList.contains("file-hover"),
          "browser file highlights survive host state updates");
        checkHighlight();
        label = cards()[1].querySelector(".param-name");
        dispatch("drop", label, [file]);
        await waitFrames(2, view);
        const binding = bindings().at(-1);
        check(binding?.name === "right" && binding.file instanceof File
          && binding.file.name === "clip.WAV"
          && [...new Uint8Array(await binding.file.arrayBuffer())].join() === "1,2,3",
          "browser drops send the chosen buffer and an intact File through the host bridge");
        check(!doc.querySelector(".file-hover"), "dropping clears the browser highlight");
        dispatch("drop", cards()[0], [typed]);
        await waitFrames(2, view);
        check(bindings().at(-1)?.name === "left" && bindings().at(-1)?.file.name === typed.name,
          "typed buffer files can bind an empty browser buffer");
        const count = bindings().length;
        for (const [target, files] of [
          [cards()[0], [file, typed]],
          [cards()[0], [new view.File(["source"], "main.onda")]],
          [doc.querySelector(".header"), [file]],
        ]) {
          check(dispatch("drop", target, files).defaultPrevented,
            "invalid file drops cannot navigate the browser away from the playground");
        }
        check(!dispatch("dragover", cards()[0], [], "some selected text").defaultPrevented,
          "ordinary text dragging retains its default behavior");
        dispatch("dragover", cards()[0], [file]);
        doc.body.dispatchEvent(new view.DragEvent("dragleave", { bubbles: true }));
        check(!doc.querySelector(".file-hover"), "leaving the browser clears its drop highlight");
        state({ status: "another background update" });
        await waitFrames(2, view);
        check(!doc.querySelector(".file-hover"),
          "host state updates do not restore a cancelled browser drag");
        dispatch("dragover", cards()[0], [file]);
        doc.body.dispatchEvent(new view.DragEvent("dragend", { bubbles: true }));
        state({ status: "update after drag end" });
        await waitFrames(2, view);
        check(!doc.querySelector(".file-hover"),
          "ending a browser drag clears its position before the next host update");
        dispatch("dragover", cards()[0].querySelector(".param-name"), [file]);
        state({ buffers: [...buffers].reverse() });
        await waitFrames(2, view);
        check(cards()[0].dataset.bufferName === "right"
          && cards()[0].classList.contains("file-hover")
          && !cards()[1].classList.contains("file-hover"),
          "browser highlights follow the cursor when state updates move cards");
        state({ path: "" });
        await waitFrames(2, view);
        check(!doc.querySelector(".file-hover"), "unloading clears the browser drop highlight");
        dispatch("drop", cards()[0], [file]);
        await waitFrames(2, view);
        check(bindings().length === count,
          "multiple files, unsupported files, outside drops, and unloaded browser controls do not bind");
      } else {
        const position = index => {
          const rect = cards()[index].querySelector(".buffer-file").getBoundingClientRect();
          return { x: rect.left + 5, y: rect.top + rect.height / 2 };
        };
        const drag = (phase, paths, point = position(1)) => publish({
          type: "bufferFileDrag", phase, paths, position: point,
        });
        drag("enter", ["/samples/new.wav"]);
        check(cards()[1].classList.contains("file-hover"),
          "native webviews highlight the buffer at the OS cursor position");
        checkHighlight();
        state({ status: "background update" });
        check(cards()[1].classList.contains("file-hover"),
          "native file highlights survive host state updates");
        drag("over", null, position(0));
        check(cards()[0].classList.contains("file-hover") && !cards()[1].classList.contains("file-hover"),
          "native drags move the highlight between buffers");
        drag("leave", null);
        check(!doc.querySelector(".file-hover"), "native drag cancellation clears the highlight");
        state({ status: "update after cancellation" });
        check(!doc.querySelector(".file-hover"),
          "host state updates do not restore a cancelled native drag");
        drag("drop", ["/samples/new.ondabuffer"]);
        check(bindings().at(-1)?.name === "right"
          && bindings().at(-1)?.filePath === "/samples/new.ondabuffer",
          "native drops use the existing file-path binding command for the target buffer");
        state({ status: "update after drop" });
        check(!doc.querySelector(".file-hover"),
          "dropping clears native drag state before the next host update");
        const count = bindings().length;
        drag("drop", ["a.wav", "b.wav"]);
        drag("drop", ["main.onda"]);
        drag("drop", ["clip.wav"], { x: 0, y: 0 });
        state({ path: "" });
        drag("drop", ["clip.wav"]);
        check(bindings().length === count,
          "native drops reject multiple files, unsupported files, outside drops, and unloaded controls");
      }
      state({ path: "sampler.onda" });
      await waitFrames(2, view);
      check(!doc.querySelector(".file-hover"),
        "loading a source does not restore the drag cleared during unload");
    } finally {
      window.removeEventListener("message", receive);
      frame.remove();
    }
  }
}

// Parameter publications must preserve the surrounding document, even in webviews
// without scroll anchoring. Real buffer changes must commit before canvas layout.
export async function testBufferRendering(check, waitFrames, pointerControl) {
  const frame = document.createElement("iframe");
  frame.style.cssText = "position:fixed;left:0;top:0;width:720px;height:480px;border:0";
  const loaded = new Promise(resolve => frame.addEventListener("load", resolve, { once: true }));
  frame.src = "/layout";
  document.body.append(frame);
  try {
    await loaded;
    const view = frame.contentWindow;
    const doc = view.document;
    const publish = state => view._onHostMessage({ type: "state", state: structuredClone(state) });
    const values = state => view._onHostMessage({ type: "parameterValues", state });
    const waveform = { minValue: -1, maxValue: 1, minimums: [-1, 0], maximums: [0, 1] };
    const buffers = Array.from({ length: 4 }, (_, index) => ({
      name: "waves[" + index + "]", type: "buffer<f32>", loadedFrames: 32,
      loadedChannels: 1, loadedSampleRate: 48000, waveform,
    }));
    const params = [
      { name: "auto_play", type: "bool", scalar: true, value: true, default: true },
      { name: "level", type: "f32", scalar: true, value: 0.5, default: 0.5,
        rangeMin: 0, rangeMax: 1, scale: "linear" },
    ];
    publish({ path: "garden.ondaproject", running: true, connected: true,
      supportsScope: true, paramLayout: "knobs", buffers, params, inputControls: [],
      events: [{ name: "bang", args: [{ name: "amount", type: "f32", value: 0.5, default: 0.5 }] }],
    });
    await waitFrames(3, view);
    const shell = doc.querySelector(".shell");
    shell.style.overflowAnchor = "none";
    shell.scrollTop = shell.scrollHeight;
    const before = shell.scrollTop;
    check(before > 0, "the buffer regression fixture scrolls to its parameters");
    const card = doc.querySelector(".buffer-card");
    const canvas = card.querySelector("canvas");
    const toggle = doc.querySelector('#params input[type="checkbox"]');
    const number = doc.querySelector("#params .param-number");
    const event = doc.querySelector(".event-card");
    toggle.click();
    params[0].value = false;
    publish({ params, buffers });
    await waitFrames(2, view);
    check(shell.scrollTop === before && doc.querySelector(".buffer-card") === card
      && doc.querySelector(".buffer-card canvas") === canvas,
    "full parameter publications preserve scroll and unchanged buffer cards and canvases");

    // A values-only publication must bypass unrelated rendering altogether.
    const mutations = [];
    const observer = new view.MutationObserver(records => mutations.push(...records));
    observer.observe(doc.querySelector("#buffers"), { subtree: true, childList: true, attributes: true });
    observer.observe(doc.querySelector("#events"), { subtree: true, childList: true, attributes: true });
    values({ params: [{ name: "auto_play", value: true }, { name: "level", value: 0.75 }] });
    await waitFrames(2, view);
    observer.disconnect();
    check(toggle.checked && Number(number.value) === 0.75 && mutations.length === 0
      && doc.querySelector(".event-card") === event && shell.scrollTop === before,
    "value publications update controls without touching buffers, event drafts, or scroll");
    number.focus({ preventScroll: true });
    number.value = "0.625";
    number.dispatchEvent(new view.Event("input", { bubbles: true }));
    values({ params: [{ name: "level", value: 0.25 }] });
    check(number.value === "0.625" && doc.activeElement === number,
      "value publications preserve the focused numeric draft");
    number.blur();

    const knob = doc.querySelector("#params .param-knob");
    const pointer = pointerControl(knob);
    pointer("pointerdown", 0, { clientY: 100 });
    pointer("pointermove", 0, { clientY: 80 });
    const draggedValue = knob.dataset.value;
    const dragScroll = shell.scrollTop;
    values({ params: [{ name: "level", value: 0.1 }] });
    await waitFrames(2, view);
    check(knob.dataset.value === draggedValue && shell.scrollTop === dragScroll
      && doc.querySelector("#params .param-knob") === knob,
    "host value publications preserve the active knob gesture and scroll position");
    pointer("pointerup", 0, { clientY: 80 });
    values({ params: [{ name: "level", value: 0.25 }] });
    check(Number(number.value) === 0.25,
      "host values resume updating the knob after its gesture commits");

    shell.scrollTop = shell.scrollHeight;
    const beforeBufferChange = shell.scrollTop;
    buffers[3].loadedPath = "/replacement.wav";
    buffers[3].waveform = { ...waveform, maximums: [1, 0] };
    publish({ buffers });
    await waitFrames(2, view);
    check(shell.scrollTop === beforeBufferChange
      && doc.querySelectorAll(".buffer-card")[3].textContent.includes("replacement.wav"),
    "real buffer changes commit complete cards before measuring waveforms and preserve scroll");
    publish({ running: false, connected: false });
    check([...doc.querySelectorAll("#buffers button")].every(button => !button.disabled),
      "loaded stopped programs retain editable buffer bindings");
    publish({ path: "" });
    check([...doc.querySelectorAll("#buffers button")].every(button => button.disabled),
      "buffer editability changes even when the buffer descriptions are unchanged");
    publish({ buffers: [] });
    check(doc.querySelectorAll(".buffer-card").length === 0,
      "removing buffers clears their retained cards");
  } finally {
    frame.remove();
  }
}
