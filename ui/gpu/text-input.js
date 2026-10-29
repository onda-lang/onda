// Native browser editing supplies mobile keyboards, selection, clipboard, and IME.
// Rust retains the draft, validation, and commit/cancel policy.
export function createTextInput(canvas, { snapshot, input, gesture }) {
  const editor = document.createElement("textarea");
  editor.className = "gpu-text-input";
  editor.rows = 1;
  editor.wrap = "off";
  editor.spellcheck = false;
  editor.autocapitalize = "off";
  editor.autocomplete = "off";
  editor.maxLength = 65536;
  editor.hidden = true;
  document.body.append(editor);
  let syncing = false;
  let composing = false;
  let blurring = false;
  let appliedSelection;

  function publish() {
    if (syncing || composing || editor.hidden) return;
    const backward = editor.selectionDirection === "backward";
    input({ type: "editText", text: editor.value,
      anchor: backward ? editor.selectionEnd : editor.selectionStart,
      cursor: backward ? editor.selectionStart : editor.selectionEnd });
  }
  editor.addEventListener("input", publish);
  editor.addEventListener("select", publish);
  document.addEventListener("selectionchange", () => {
    if (document.activeElement === editor) publish();
  });
  editor.addEventListener("compositionstart", () => { composing = true; });
  editor.addEventListener("compositionend", () => { composing = false; publish(); });
  for (const type of ["pointerdown", "pointerup"])
    editor.addEventListener(type, event => gesture?.(event));
  editor.addEventListener("mousedown", event => {
    if (event.button !== 0 || event.detail !== 2 || composing || !snapshot()?.resettable) return;
    event.preventDefault();
    const rect = editor.getBoundingClientRect();
    input({ type:"pointerDown", x:rect.left + rect.width / 2, y:rect.top + rect.height / 2, clicks:2 });
    input({ type:"pointerUp" });
  });
  editor.addEventListener("keydown", event => {
    if (event.defaultPrevented || event.isComposing) return;
    const shortcut = event.ctrlKey || event.metaKey;
    const transport = shortcut && ["s", ".", "enter"].includes(event.key.toLowerCase());
    if (transport && event.repeat) {
      event.preventDefault();
      return;
    }
    gesture?.(event, transport && event.key === ".");
    if (transport || ["Enter", "Escape", "Tab", "ArrowUp", "ArrowDown"].includes(event.key)) {
      event.preventDefault();
      publish();
      input({ type:"key", key:transport ? event.key === "." ? "Stop" : "Play" : event.key, shift:event.shiftKey });
    }
  });
  editor.addEventListener("blur", event => {
    if (syncing || event.relatedTarget === canvas) return;
    blurring = true;
    try {
      publish();
      input({ type:"key", key:"Enter" });
      input({ type:"focusLost" });
    } finally { blurring = false; }
  });

  return {
    element: editor,
    sync() {
      const edit = snapshot();
      if (composing && edit) return;
      if (!edit && editor.hidden) return;
      syncing = true;
      try {
        if (!edit) {
          appliedSelection = undefined;
          composing = false;
          const focused = document.activeElement === editor;
          editor.hidden = true;
          if (focused) canvas.focus({ preventScroll:true });
          return;
        }
        const [x,y,width,height] = edit.rect;
        editor.style.setProperty("--field-font-size", `${edit.fontSize}px`);
        Object.assign(editor.style, { left:`${x}px`, top:`${y}px`, width:`${width}px`, height:`${height}px`,
          lineHeight:`${Math.max(1, height - 2)}px`,
          fontFamily:edit.monospace ? '"JetBrains Mono",monospace' : '"Inter",sans-serif',
          textAlign:edit.centered ? "center" : "left" });
        editor.dataset.invalid = String(edit.invalid);
        editor.hidden = false;
        if (editor.value !== edit.text) editor.value = edit.text;
        const start = Math.min(edit.anchor, edit.cursor), end = Math.max(edit.anchor, edit.cursor);
        const direction = edit.anchor > edit.cursor ? "backward" : "forward";
        const selection = JSON.stringify([edit.text, start, end, direction]);
        // Native selection events can arrive after a render/resize callback.
        // An unchanged model must not undo the browser's pending caret movement.
        if (selection !== appliedSelection && (editor.selectionStart !== start || editor.selectionEnd !== end || editor.selectionDirection !== direction))
          editor.setSelectionRange(start, end, direction);
        appliedSelection = selection;
        if (!blurring && document.activeElement !== editor) editor.focus({ preventScroll:true });
      } finally { syncing = false; }
    },
  };
}
