// Shared JSON-RPC client for the Wasm-hosted Onda language server.
const projectRootUri = "file:///onda-project/";

export function projectPathToUri(path) {
  return projectRootUri + path.split("/").map(encodeURIComponent).join("/");
}

export function projectUriToPath(uri) {
  if (typeof uri !== "string" || !uri.startsWith(projectRootUri)) return null;
  try {
    return uri
      .slice(projectRootUri.length)
      .split("/")
      .map(decodeURIComponent)
      .join("/");
  } catch {
    return null;
  }
}

export class OndaBrowserLsp {
  constructor(compiler, { onDiagnostics, onError } = {}) {
    this.compiler = compiler;
    this.onDiagnostics = onDiagnostics;
    this.onError = onError;
    this.documents = new Map();
    this.changeTimers = new Map();
    this.nextRequestId = 1;
    this.queue = Promise.resolve();
  }

  async initialize(options) {
    await this.compiler.setLspAnalysisOptions(options);
    const result = await this.request("initialize", {
      processId: null,
      rootUri: projectRootUri.slice(0, -1),
      capabilities: {
        textDocument: {
          completion: { completionItem: { snippetSupport: false } },
          semanticTokens: {},
        },
      },
      clientInfo: { name: "onda-browser", version: "1" },
    });
    await this.notify("initialized", {});
    return result;
  }

  setAnalysisOptions(options) {
    return this.enqueue(async () => {
      await this.compiler.setLspAnalysisOptions(options);
      await this.notify("workspace/didChangeWatchedFiles", { changes: [] });
    });
  }

  syncProject(project) {
    const snapshot = {
      entry: project.entry,
      sources: { ...project.sources },
    };
    return this.enqueue(async () => {
      const nextPaths = new Set(Object.keys(snapshot.sources));
      for (const path of [...this.documents.keys()]) {
        if (nextPaths.has(path)) continue;
        this.cancelPendingChange(path);
        await this.notify("textDocument/didClose", {
          textDocument: { uri: projectPathToUri(path) },
        });
        this.documents.delete(path);
        this.onDiagnostics?.(path, []);
      }

      const orderedPaths = [...nextPaths].sort((left, right) => {
        if (left === snapshot.entry) return 1;
        if (right === snapshot.entry) return -1;
        return left.localeCompare(right);
      });
      for (const path of orderedPaths) {
        const source = snapshot.sources[path];
        const document = this.documents.get(path);
        if (!document) {
          this.documents.set(path, { source, version: 1, sentSource: source });
          await this.notify("textDocument/didOpen", {
            textDocument: {
              uri: projectPathToUri(path),
              languageId: "onda",
              version: 1,
              text: source,
            },
          });
        } else if (document.source !== source) {
          document.source = source;
          this.scheduleChange(path);
        }
      }
    });
  }

  completion(path, position, context) {
    return this.documentRequest(path, "textDocument/completion", {
      position,
      context,
    });
  }

  hover(path, position) {
    return this.documentRequest(path, "textDocument/hover", { position });
  }

  definition(path, position) {
    return this.documentRequest(path, "textDocument/definition", { position });
  }

  definitionAtUri(uri, position) {
    return this.request("textDocument/definition", {
      textDocument: { uri },
      position,
    });
  }

  virtualDocument(uri) {
    return this.request("onda/virtualDocument", { uri });
  }

  documentSymbols(path) {
    return this.documentRequest(path, "textDocument/documentSymbol");
  }

  semanticTokens(path) {
    return this.documentRequest(path, "textDocument/semanticTokens/full");
  }

  async documentRequest(path, method, params = {}) {
    await this.flushDocument(path);
    return this.request(method, {
      textDocument: { uri: projectPathToUri(path) },
      ...params,
    });
  }

  scheduleChange(path) {
    this.cancelPendingChange(path);
    const timer = setTimeout(() => {
      this.changeTimers.delete(path);
      this.enqueue(() => this.sendDocumentChange(path)).catch((error) => this.reportError(error));
    }, 400);
    this.changeTimers.set(path, timer);
  }

  cancelPendingChange(path) {
    const timer = this.changeTimers.get(path);
    if (timer !== undefined) clearTimeout(timer);
    this.changeTimers.delete(path);
  }

  async flushDocument(path) {
    await this.queue;
    if (!this.changeTimers.has(path)) return;
    this.cancelPendingChange(path);
    await this.enqueue(() => this.sendDocumentChange(path));
  }

  async sendDocumentChange(path) {
    const document = this.documents.get(path);
    if (!document || document.source === document.sentSource) return;
    document.version += 1;
    document.sentSource = document.source;
    await this.notify("textDocument/didChange", {
      textDocument: {
        uri: projectPathToUri(path),
        version: document.version,
      },
      contentChanges: [{ text: document.source }],
    });
  }

  async request(method, params) {
    const id = this.nextRequestId++;
    const responses = await this.compiler.sendLspMessage({
      jsonrpc: "2.0",
      id,
      method,
      params,
    });
    const response = this.routeMessages(responses, id);
    if (!response) throw new Error(`Onda LSP did not respond to '${method}'`);
    if (response.error) throw new Error(response.error.message ?? `Onda LSP '${method}' failed`);
    return response.result;
  }

  async notify(method, params) {
    const responses = await this.compiler.sendLspMessage({
      jsonrpc: "2.0",
      method,
      params,
    });
    this.routeMessages(responses);
  }

  routeMessages(messages, responseId) {
    let response = null;
    for (const message of messages) {
      if (message?.id === responseId && !message.method) response = message;
      if (message?.method === "textDocument/publishDiagnostics") {
        const path = projectUriToPath(message.params?.uri);
        if (path) this.onDiagnostics?.(path, message.params?.diagnostics ?? []);
      }
    }
    return response;
  }

  enqueue(operation) {
    const result = this.queue.then(operation);
    this.queue = result.catch((error) => this.reportError(error));
    return result;
  }

  reportError(error) {
    this.onError?.(error);
  }

  dispose() {
    for (const path of this.changeTimers.keys()) this.cancelPendingChange(path);
    this.documents.clear();
  }
}
