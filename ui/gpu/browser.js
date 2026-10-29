// Browser-only boundaries: trusted gestures, file objects, and parent envelopes.
export function browserBridge(canvas, { message, target, onInput }) {
  if (
    window.parent === window ||
    window.ipc ||
    typeof acquireVsCodeApi === "function"
  )
    return null;
  const origin = window.location.origin;
  let pointerStop = false;
  const post = (value) =>
    window.parent.postMessage(
      { __ondaRunWebview: true, message: value },
      origin,
    );

  window.addEventListener("message", (event) => {
    if (
      event.source === window.parent &&
      event.origin === origin &&
      event.data?.__ondaRunHost
    ) {
      message(event.data.message);
    }
  });

  const drag = (event) => {
    const transfer = event.dataTransfer;
    if (!transfer?.types.includes("Files")) return;
    event.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const position = {
      x: event.clientX - rect.left,
      y: event.clientY - rect.top,
    };
    const files = [...transfer.items].filter(item => item.kind === "file");
    const accepted = files.length === 1 &&
      (!transfer.files.length || /\.(wav|ondabuffer)$/i.test(transfer.files[0].name));
    const name = accepted ? target(position.x, position.y) : null;
    transfer.dropEffect = name ? "copy" : "none";
    message({
      type: "bufferFileDrag",
      phase:
        !name || event.type === "drop" ? "leave" : "over",
      position,
    });
    if (event.type === "drop" && name && transfer.files.length === 1) {
      post({ type: "bindBufferFile", name, file: transfer.files[0] });
    }
  };
  for (const type of ["dragenter", "dragover", "drop"])
    canvas.addEventListener(type, drag);
  for (const type of ["dragleave", "dragend"])
    canvas.addEventListener(type, () => message({ type: "bufferFileDrag", phase: "leave" }));

  document.addEventListener(
    "keydown",
    (event) => {
      if (
        event.altKey ||
        event.isComposing ||
        (!event.ctrlKey && !event.metaKey)
      )
        return;
      const start = event.key === "Enter" || event.key.toLowerCase() === "s";
      const stop = event.key === "." || event.code === "Period";
      if (!start && !stop) return;
      event.preventDefault();
      if (event.repeat) return;
      if (onInput) onInput({ type: "key", key: stop ? "Stop" : "Play" });
      else post({ type: start ? "start" : "stop" });
      gesture(event, stop);
    },
    { capture: true },
  );

  return {
    send(value) {
      if (value.type === "chooseBufferFile") chooseFile(value.name);
      else post(value);
    },
    gesture,
  };

  function gesture(event, stop = false) {
    if (event.type === "pointerdown") pointerStop = stop;
    if (event.type === "pointerup") {
      stop ||= pointerStop;
      pointerStop = false;
    }
    document.dispatchEvent(
      new CustomEvent("ondaRunGesture", { detail: { event, stop } }),
    );
  }
  function chooseFile(name) {
    const input = document.createElement("input");
    input.type = "file";
    input.accept =
      ".wav,.ondabuffer,audio/wav,audio/x-wav,application/vnd.onda.buffer";
    input.hidden = true;
    const cleanup = () => {
      window.removeEventListener("focus", cancel);
      input.remove();
    };
    const cancel = () =>
      setTimeout(() => {
        if (!input.files?.length) cleanup();
      }, 0);
    input.addEventListener(
      "change",
      () => {
        if (input.files?.[0])
          post({ type: "bindBufferFile", name, file: input.files[0] });
        cleanup();
      },
      { once: true },
    );
    input.addEventListener("cancel", cleanup, { once: true });
    window.addEventListener("focus", cancel, { once: true });
    document.body.append(input);
    input.click();
  }
}
