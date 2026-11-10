import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

export function gpuUiVersion() {
  const workspace = readFileSync(resolve(root, "Cargo.toml"), "utf8")
    .split(/^\[workspace\.package\]\s*$/m)[1]?.split(/^\[/m)[0];
  const version = /^version\s*=\s*"([^"]+)"\s*$/m.exec(workspace ?? "")?.[1];
  if (!version) throw new Error("Missing Onda workspace version");
  return version;
}

// Derive required fonts and notices from the authored assets, not a possibly
// incomplete build directory. Build certification and staging share this list.
export function gpuUiFiles() {
  return [
    "index.html", "host.js", "browser.js", "protocol.js", "text-input.js",
    "pkg/onda_ui.js", "pkg/onda_ui_bg.wasm",
    "pkg/fonts/Inter.ttf", "pkg/fonts/JetBrainsMono.ttf",
    ...readdirSync(resolve(root, "assets/fonts"))
      .filter(name => /^OFL-.*\.txt$/.test(name)).map(name => `pkg/${name}`),
    ...readdirSync(resolve(root, "assets/fonts/fallback"))
      .map(name => `pkg/fonts/${name}`),
  ].sort();
}
