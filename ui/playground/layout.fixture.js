// Render the real editor in the layout fixtures without starting the compiler.
import { OndaProjectEditor } from "./editor.js";
import { bindEditorFontSize } from "./editor-font-size.js";
import { bindCompileSettingLabels } from "./compile-setting-labels.js";
import "./layout.css";
import source from "./default.onda";

const editor = new OndaProjectEditor({
  parent: document.querySelector("[data-editor]"),
  tabs: document.querySelector("[data-file-tabs]"),
  initialProject: {
    entry: "main.onda",
    active: "main.onda",
    sources: { "main.onda": source },
  },
});
const status = document.querySelector("[data-status]");
status.textContent = "Playing";
status.classList.add("playing");
const fontSize = document.querySelector("[data-editor-font-size]");
bindEditorFontSize(fontSize, editor);
bindCompileSettingLabels(document);
