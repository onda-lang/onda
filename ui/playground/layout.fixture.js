// Render the real editor in the layout fixtures without starting the compiler.
import { OndaProjectEditor } from "./editor.js";
import { bindEditorFontSize } from "./editor-font-size.js";
import { bindProjectMenu } from "./project-menu.js";
import { bindNewFileButton } from "./new-file.js";
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
window.__testEditor = editor;
bindNewFileButton(document.querySelector("[data-new-file]"), editor);
const status = document.querySelector("[data-status]");
status.textContent = "Playing";
document.querySelector("[data-run-loading]").hidden = true;
document.querySelector("[data-run-view]").parentElement.setAttribute("aria-busy", "false");
const fontSize = document.querySelector("[data-editor-font-size]");
bindEditorFontSize(fontSize, editor);
bindProjectMenu(document.querySelector("[data-project-menu]"));
