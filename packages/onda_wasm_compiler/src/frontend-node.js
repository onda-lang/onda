import { readFile } from "node:fs/promises";

export function defaultFrontendInput() {
  return readFile(new URL(
    "../dist/frontend/onda_compiler_web_bg.wasm",
    import.meta.url,
  ));
}
