// Keep file creation inside the editor, including in hosts that block prompt().
export function bindNewFileButton(button, editor) {
  const document = button.ownerDocument;
  let pending = null;
  button.disabled = false;
  button.setAttribute("aria-expanded", "false");
  button.addEventListener("click", () => {
    if (pending) {
      pending.focus();
      return;
    }
    const entry = document.createElement("div");
    entry.className = "project-file project-file-create";
    const input = document.createElement("input");
    input.type = "text";
    input.setAttribute("aria-label", "New file name");
    input.autocomplete = "off";
    input.autocapitalize = "off";
    input.spellcheck = false;
    input.enterKeyHint = "done";
    const paths = new Set(editor.paths());
    input.value = "module.onda";
    for (let index = 2; paths.has(input.value); index += 1)
      input.value = `module${index}.onda`;
    const error = document.createElement("span");
    error.className = "project-file-create-error";
    error.setAttribute("role", "alert");
    error.hidden = true;
    entry.append(input, error);
    button.before(entry);
    pending = input;
    button.setAttribute("aria-expanded", "true");
    const listeners = new AbortController();
    const close = (restoreFocus = false) => {
      if (pending !== input) return;
      pending = null;
      listeners.abort();
      entry.remove();
      button.setAttribute("aria-expanded", "false");
      if (restoreFocus) button.focus();
    };
    input.addEventListener("input", () => {
      error.hidden = true;
      input.removeAttribute("aria-invalid");
    });
    input.addEventListener("keydown", event => {
      if (event.defaultPrevented || event.isComposing || !["Enter", "Escape"].includes(event.key)) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        close(true);
        return;
      }
      try {
        editor.add(input.value.trim());
        close();
      } catch (failure) {
        error.textContent = failure.message;
        error.hidden = false;
        input.setAttribute("aria-invalid", "true");
      }
    });
    entry.addEventListener("focusout", event => {
      if (event.relatedTarget && event.relatedTarget !== button && !entry.contains(event.relatedTarget)) close();
    });
    document.addEventListener("pointerdown", event => {
      if (event.target !== button && !entry.contains(event.target)) close();
    }, {signal:listeners.signal});
    input.focus();
    input.select();
  });
}
