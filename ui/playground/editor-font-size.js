import "../number-input.js";

const storageKey = "onda.browser-ide.editor-font-size.v1";

// Own the preference and its editing mode together, including live previews.
export function bindEditorFontSize(input, editor) {
  const view = input.ownerDocument.defaultView;
  let value = null;
  let beforeEdit = null;
  const valid = text => {
    if (!/^[0-9]+$/.test(text ?? "")) return null;
    const size = Number(text);
    return Number.isInteger(size) && size >= Number(input.min) && size <= Number(input.max)
      ? size : null;
  };
  const sync = () => {
    input.value = String(value ?? parseFloat(view.getComputedStyle(editor.parent).fontSize));
  };
  const set = size => {
    value = size;
    editor.setFontSize(value);
    sync();
    try {
      if (value === null) view.localStorage.removeItem(storageKey);
      else view.localStorage.setItem(storageKey, String(value));
    } catch {
      // Editing also works when browser storage is disabled.
    }
  };
  try {
    value = valid(view.localStorage.getItem(storageKey));
  } catch {
    // Use automatic sizing when storage is unavailable.
  }
  editor.setFontSize(value);
  sync();
  input.addEventListener("focus", () => { beforeEdit = value; });
  input.addEventListener("input", () => {
    // The number widget marks cancellation read-only before restoring its value.
    if (input.readOnly) set(beforeEdit);
    else {
      const size = valid(input.value);
      if (size !== null) set(size);
    }
  });
  const normalize = () => {
    if (input.value === "" && !input.validity.badInput) set(null);
    else sync();
  };
  input.addEventListener("change", normalize);
  input.addEventListener("blur", normalize);
  view.addEventListener("resize", () => {
    if (value === null && input !== input.ownerDocument.activeElement) sync();
  });
  globalThis.__ONDA_NUMBER_INPUT__.bindDraggableNumber(input, {
    speed: 0.25,
    updateValue(size, _committed, action) {
      set(action === "reset" ? null : Math.round(size));
    },
  });
  return { get value() { return value; }, set };
}
