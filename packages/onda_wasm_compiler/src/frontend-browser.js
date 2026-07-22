export function defaultFrontendInput() {
  return new URL(
    "../dist/frontend/onda_compiler_web_bg.wasm",
    import.meta.url,
  );
}
