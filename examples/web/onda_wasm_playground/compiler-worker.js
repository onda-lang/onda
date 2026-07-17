let toolchainPromise = null;

function loadToolchain() {
  if (!toolchainPromise) {
    toolchainPromise = Promise.all([
      import("./onda-binaryen-web.js"),
      import("./onda-compiler-web/onda_compiler_web.js"),
    ]).then(async ([backend, compiler]) => {
      await compiler.default();
      return { backend, compiler };
    });
  }
  return toolchainPromise;
}

self.onmessage = async (event) => {
  const message = event.data ?? {};
  const requestId = message.requestId;
  try {
    self.postMessage({ type: "phase", requestId, phase: "loading" });
    const { backend, compiler } = await loadToolchain();
    if (message.type === "initialize") {
      self.postMessage({ type: "result", requestId, value: null });
      return;
    }
    if (message.type !== "compile") {
      throw new Error(`unknown compiler-worker request '${String(message.type)}'`);
    }
    self.postMessage({ type: "phase", requestId, phase: "mir" });
    const mir = compiler.compile_to_mir_messagepack(
      message.source,
      message.sampleRate,
      message.blockSize,
    );
    self.postMessage({ type: "phase", requestId, phase: "binaryen" });
    const artifact = backend.compileTrustedMir(mir, message.options ?? {});
    self.postMessage(
      { type: "result", requestId, value: artifact },
      [artifact.wasm.buffer],
    );
  } catch (error) {
    self.postMessage({
      type: "error",
      requestId,
      error: typeof error === "string" ? error : String(error?.message ?? error),
    });
  }
};
