import { createCompiler } from "./index.js";

let compilerPromise;

globalThis.addEventListener("message", async (event) => {
  const message = event.data ?? {};
  const requestId = message.requestId;
  try {
    if (message.type === "initialize") {
      await compiler(message.frontendWasm);
      respond(requestId, null);
      return;
    }
    if (message.type === "compileSource") {
      const result = await (await compiler()).compileSource(
        message.source,
        message.options,
      );
      respond(requestId, result, [result.artifact.wasm.buffer]);
      return;
    }
    if (message.type === "compileProject") {
      const result = await (await compiler()).compileProject(
        message.project,
        message.options,
      );
      respond(requestId, result, [result.artifact.wasm.buffer]);
      return;
    }
    if (message.type === "lspMessage") {
      const responses = await (await compiler()).sendLspMessage(message.message);
      respond(requestId, responses);
      return;
    }
    if (message.type === "lspAnalysisOptions") {
      await (await compiler()).setLspAnalysisOptions(message.options);
      respond(requestId, null);
      return;
    }
    if (message.type === "dispose") {
      if (compilerPromise) await (await compilerPromise).dispose();
      respond(requestId, null);
      globalThis.close();
      return;
    }
    throw new Error(`unknown compiler worker request '${String(message.type)}'`);
  } catch (error) {
    globalThis.postMessage({
      type: "error",
      requestId,
      error: {
        name: error?.name ?? "Error",
        message: error?.message ?? String(error),
        stack: error?.stack,
        diagnostics: error?.diagnostics,
        sourceFiles: error?.sourceFiles,
        unresolvedSourceFiles: error?.unresolvedSourceFiles,
      },
    });
  }
});

function compiler(frontendWasm) {
  compilerPromise ??= createCompiler({ frontendWasm });
  return compilerPromise;
}

function respond(requestId, value, transfer = []) {
  globalThis.postMessage({ type: "result", requestId, value }, transfer);
}
