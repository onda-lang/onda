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
    if (message.type === "compileWorkspace") {
      const result = await (await compiler()).compileWorkspace(
        message.workspace,
        message.options,
      );
      respond(requestId, result, [result.artifact.wasm.buffer]);
      return;
    }
    if (message.type === "compileProjectImage") {
      const result = await (await compiler()).compileProjectImage(
        message.imageBytes,
        message.options,
      );
      respond(requestId, result, [result.artifact.wasm.buffer]);
      return;
    }
    if (message.type === "createProjectImage") {
      const result = await (await compiler()).createProjectImage(
        message.sourceGraph,
        message.buffers,
      );
      respond(requestId, result, [result.bytes.buffer]);
      return;
    }
    if (message.type === "inspectProjectImage") {
      respond(requestId, await (await compiler()).inspectProjectImage(message.imageBytes));
      return;
    }
    if (message.type === "loadProjectFiles") {
      const result = await (await compiler()).loadProjectFiles(
        message.files,
        message.projectFilePath,
      );
      respond(requestId, result, [result.bytes.buffer]);
      return;
    }
    if (message.type === "materializeProjectImage") {
      const result = await (await compiler()).materializeProjectImage(
        message.imageBytes,
        message.assetFileNames,
      );
      respond(requestId, result, result.files.map((file) => file.bytes.buffer));
      return;
    }
    if (message.type === "encodeBufferAsset") {
      const result = await (await compiler()).encodeBufferAsset(message.binding);
      respond(requestId, result, [result.buffer]);
      return;
    }
    if (message.type === "decodeBufferAsset") {
      const result = await (await compiler()).decodeBufferAsset(message.bytes);
      respond(requestId, result, [result.data.buffer]);
      return;
    }
    if (message.type === "decodeBufferFile") {
      const result = await (await compiler()).decodeBufferFile(message.bytes, message.path);
      respond(requestId, result, [result.data.buffer]);
      return;
    }
    if (message.type === "projectCapabilities") {
      respond(requestId, await (await compiler()).projectCapabilities());
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
