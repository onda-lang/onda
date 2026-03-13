import * as childProcess from "child_process";
import * as net from "net";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

type PatchScalarValue = boolean | number | null;

interface PatchParamPayload {
  index: number;
  name: string;
  type: string;
  default: number | null;
  rangeMin: number | null;
  rangeMax: number | null;
  scalar: boolean;
}

interface PatchParamState extends PatchParamPayload {
  value: PatchScalarValue;
  smoothingMs: number;
}

interface PatchReadyEvent {
  event: "ready";
  path: string;
  port: number;
  params: PatchParamPayload[];
  outputChannels: number;
}

interface PatchPanelState {
  running: boolean;
  connected: boolean;
  path?: string;
  status: string;
  error?: string;
  params: PatchParamState[];
}

interface PendingControlRequest {
  resolve: (value: any) => void;
  reject: (error: Error) => void;
}

interface PatchParamDispatchState {
  runtimeValue?: PatchScalarValue;
  targetValue?: PatchScalarValue;
  timer?: NodeJS.Timeout;
}

let client: LanguageClient | undefined;
let extensionContext: vscode.ExtensionContext | undefined;
let patchProcess: childProcess.ChildProcessWithoutNullStreams | undefined;
let patchPath: string | undefined;
let patchOutput: vscode.OutputChannel | undefined;
let serverOutput: vscode.OutputChannel | undefined;
let patchPanel: vscode.WebviewPanel | undefined;
let patchControlSocket: net.Socket | undefined;
let patchControlBuffer = "";
let patchStdoutBuffer = "";
let patchControlRequestId = 0;
let stoppingPatchPid: number | undefined;
const pendingPatchRequests = new Map<number, PendingControlRequest>();
const patchParamDispatch = new Map<string, PatchParamDispatchState>();
let patchPanelState: PatchPanelState = {
  running: false,
  connected: false,
  status: "Stopped",
  params: [],
};

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  extensionContext = context;
  patchOutput = vscode.window.createOutputChannel("Omni Patch");
  serverOutput = vscode.window.createOutputChannel("Omni Language Server");
  context.subscriptions.push(patchOutput, serverOutput);

  context.subscriptions.push(
    vscode.commands.registerCommand("omni.restartLanguageServer", async () => {
      await restartClient();
    }),
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("omni.runPatch", async () => {
      await runPatch();
    }),
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("omni.stopPatch", async () => {
      await stopPatch();
    }),
  );
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument(async (document) => {
      await restartPatchForSavedDocument(document);
    }),
  );

  await startClient(context);
}

export async function deactivate(): Promise<void> {
  await stopPatch({ silent: true });

  if (!client) {
    return;
  }
  const activeClient = client;
  client = undefined;
  try {
    await activeClient.stop();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.includes("Client is not running")) {
      throw error;
    }
  }
}

async function restartClient(): Promise<void> {
  await deactivate();
  if (!extensionContext) {
    throw new Error("Omni extension context is not initialized");
  }
  await startClient(extensionContext);
}

async function runPatch(preferredPath?: string, options?: { restart?: boolean }): Promise<void> {
  const fsPath = await resolvePatchPath(preferredPath);
  if (!fsPath) {
    return;
  }

  ensurePatchPanel();
  const preservedParams =
    patchPanelState.path === fsPath ? patchPanelState.params : [];
  patchPanelState = {
    running: false,
    connected: false,
    path: fsPath,
    status: `Starting ${path.basename(fsPath)}...`,
    error: undefined,
    params: preservedParams,
  };
  postPatchPanelState();

  if (patchProcess && patchPath === fsPath && !options?.restart) {
    patchPanel?.reveal(vscode.ViewColumn.Beside);
    return;
  }

  await stopPatch({ silent: true, preservePath: fsPath });

  const { command, extraArgs } = omniExecutableConfig();
  const args = [...extraArgs, "preview", "play", fsPath, "--forever", "--control-json"];
  const cwd = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(fsPath))?.uri.fsPath ?? path.dirname(fsPath);
  const child = childProcess.spawn(command, args, {
    cwd,
    stdio: "pipe",
  });

  patchProcess = child;
  patchPath = fsPath;
  patchStdoutBuffer = "";

  patchOutput?.appendLine(`$ ${command} ${args.map(shellQuote).join(" ")}`);
  patchOutput?.show(true);
  patchPanel?.reveal(vscode.ViewColumn.Beside);

  child.stdout.on("data", (chunk: Buffer) => {
    handlePatchStdout(chunk.toString());
  });
  child.stderr.on("data", (chunk: Buffer) => {
    patchOutput?.append(chunk.toString());
  });
  child.once("error", (error: Error) => {
    const failedPath = fsPath;
    if (patchProcess === child) {
      clearPatchRuntimeState({ preservePath: failedPath });
    }
    patchPanelState = {
      ...patchPanelState,
      running: false,
      connected: false,
      path: failedPath,
      status: "Failed to start",
      error: error.message,
    };
    postPatchPanelState();
    void vscode.window.showErrorMessage(
      `Failed to start Omni patch${failedPath ? ` (${path.basename(failedPath)})` : ""}: ${error.message}`,
    );
  });
  child.once("exit", (code: number | null, signal: NodeJS.Signals | null) => {
    const finishedPath = fsPath;
    const expectedStop = child.pid !== undefined && stoppingPatchPid === child.pid;
    if (expectedStop) {
      stoppingPatchPid = undefined;
    }
    if (patchProcess === child) {
      clearPatchRuntimeState({ preservePath: finishedPath });
    }
    patchPanelState = {
      ...patchPanelState,
      running: false,
      connected: false,
      path: finishedPath,
      status: expectedStop ? "Stopped" : "Patch exited",
      error: expectedStop ? undefined : signal ? `signal ${signal}` : `exit code ${code ?? "unknown"}`,
    };
    postPatchPanelState();
    if (expectedStop) {
      return;
    }
    if (signal === null && code === 0) {
      return;
    }
    const reason = signal ? `signal ${signal}` : `exit code ${code ?? "unknown"}`;
    void vscode.window.showWarningMessage(
      `Omni patch stopped${finishedPath ? ` (${path.basename(finishedPath)})` : ""}: ${reason}`,
    );
  });
}

async function stopPatch(options?: { silent?: boolean; preservePath?: string }): Promise<void> {
  if (!patchProcess) {
    if (!options?.silent) {
      void vscode.window.showInformationMessage("No Omni patch is currently running.");
    }
    patchPanelState = {
      ...patchPanelState,
      running: false,
      connected: false,
      status: "Stopped",
    };
    postPatchPanelState();
    return;
  }

  const child = patchProcess;
  const runningPath = patchPath;
  clearPatchRuntimeState({ preservePath: options?.preservePath ?? runningPath });
  stoppingPatchPid = child.pid;
  child.kill();

  patchPanelState = {
    ...patchPanelState,
    running: false,
    connected: false,
    path: options?.preservePath ?? runningPath,
    status: "Stopped",
    error: undefined,
  };
  postPatchPanelState();

  if (!options?.silent && runningPath) {
    void vscode.window.showInformationMessage(`Stopped Omni patch: ${path.basename(runningPath)}`);
  }
}

function clearPatchRuntimeState(options?: { preservePath?: string }): void {
  patchProcess = undefined;
  patchPath = options?.preservePath;
  patchStdoutBuffer = "";
  closePatchControlSocket();
}

async function restartPatchForSavedDocument(document: vscode.TextDocument): Promise<void> {
  if (!patchProcess || !patchPath) {
    return;
  }
  if (document.languageId !== "omni" || document.uri.scheme !== "file") {
    return;
  }
  if (path.resolve(document.uri.fsPath) !== path.resolve(patchPath)) {
    return;
  }
  await runPatch(document.uri.fsPath, { restart: true });
}

async function resolvePatchPath(preferredPath?: string): Promise<string | undefined> {
  if (preferredPath) {
    return preferredPath;
  }
  const document = await currentPatchDocument();
  if (!document) {
    return undefined;
  }

  if (document.isDirty) {
    const saved = await document.save();
    if (!saved) {
      void vscode.window.showErrorMessage("Omni patch must be saved before playback starts.");
      return undefined;
    }
  }

  return document.uri.fsPath;
}

async function currentPatchDocument(): Promise<vscode.TextDocument | undefined> {
  const editor = vscode.window.activeTextEditor;
  const document = editor?.document;
  if (!document || document.languageId !== "omni") {
    void vscode.window.showErrorMessage("Open an Omni file to run a patch.");
    return undefined;
  }
  if (document.uri.scheme !== "file") {
    void vscode.window.showErrorMessage("Omni patch playback currently requires a saved file on disk.");
    return undefined;
  }
  return document;
}

function omniExecutableConfig(): { command: string; extraArgs: string[] } {
  const config = vscode.workspace.getConfiguration("omni");
  return {
    command: config.get<string>("server.path", "omni"),
    extraArgs: config.get<string[]>("server.args", []),
  };
}

function shellQuote(value: string): string {
  return /\s/.test(value) ? JSON.stringify(value) : value;
}

function handlePatchStdout(chunk: string): void {
  patchStdoutBuffer += chunk;
  for (;;) {
    const newline = patchStdoutBuffer.indexOf("\n");
    if (newline < 0) {
      break;
    }
    const line = patchStdoutBuffer.slice(0, newline).trim();
    patchStdoutBuffer = patchStdoutBuffer.slice(newline + 1);
    if (line.length === 0) {
      continue;
    }
    handlePatchStdoutLine(line);
  }
}

function handlePatchStdoutLine(line: string): void {
  try {
    const payload = JSON.parse(line) as PatchReadyEvent;
    if (payload.event === "ready") {
      patchPanelState = {
        running: true,
        connected: false,
        path: patchPath,
        status: "Running",
        error: undefined,
        params: mergePatchParams(payload.params, patchPanelState.params),
      };
      postPatchPanelState();
      connectPatchControl(payload.port);
      return;
    }
  } catch {
    // Fall through to raw output logging.
  }

  patchOutput?.appendLine(line);
}

function hydratePatchParams(params: PatchParamPayload[]): PatchParamState[] {
  return params
    .filter((param) => param.scalar)
    .map((param) => ({
      ...param,
      value: initialParamValue(param),
      smoothingMs: defaultParamSmoothingMs(param),
    }));
}

function mergePatchParams(
  params: PatchParamPayload[],
  existing: PatchParamState[],
): PatchParamState[] {
  return params
    .filter((param) => param.scalar)
    .map((param) => {
      const previous = existing.find((item) => item.name === param.name);
      return {
        ...param,
        value: previous?.value ?? initialParamValue(param),
        smoothingMs: previous?.smoothingMs ?? defaultParamSmoothingMs(param),
      };
    });
}

function initialParamValue(param: PatchParamPayload): PatchScalarValue {
  if (param.type === "bool") {
    if (param.default === null) {
      return false;
    }
    return param.default !== 0;
  }
  if (param.default !== null) {
    return param.default;
  }
  if (param.rangeMin !== null) {
    return param.rangeMin;
  }
  return 0;
}

function defaultParamSmoothingMs(param: PatchParamPayload): number {
  return param.type === "f32" || param.type === "f64" ? 20 : 0;
}

function connectPatchControl(port: number): void {
  closePatchControlSocket();

  const socket = net.createConnection({ host: "127.0.0.1", port });
  patchControlSocket = socket;
  patchControlBuffer = "";

  socket.setEncoding("utf8");
  socket.on("connect", () => {
    patchPanelState = {
      ...patchPanelState,
      connected: true,
      status: "Running",
      error: undefined,
    };
    postPatchPanelState();
    void refreshPatchParams().then(() => reapplyCachedPatchParams());
  });
  socket.on("data", (chunk: string) => {
    patchControlBuffer += chunk;
    for (;;) {
      const newline = patchControlBuffer.indexOf("\n");
      if (newline < 0) {
        break;
      }
      const line = patchControlBuffer.slice(0, newline).trim();
      patchControlBuffer = patchControlBuffer.slice(newline + 1);
      if (line.length === 0) {
        continue;
      }
      handlePatchControlLine(line);
    }
  });
  socket.on("error", (error: Error) => {
    patchPanelState = {
      ...patchPanelState,
      connected: false,
      error: error.message,
    };
    postPatchPanelState();
  });
  socket.on("close", () => {
    if (patchControlSocket === socket) {
      patchControlSocket = undefined;
      patchControlBuffer = "";
      rejectPendingPatchRequests(new Error("Patch control connection closed."));
      patchPanelState = {
        ...patchPanelState,
        connected: false,
      };
      postPatchPanelState();
    }
  });
}

function closePatchControlSocket(): void {
  if (patchControlSocket) {
    patchControlSocket.destroy();
    patchControlSocket = undefined;
  }
  patchControlBuffer = "";
  clearPatchParamDispatch();
  rejectPendingPatchRequests(new Error("Patch control session ended."));
}

function handlePatchControlLine(line: string): void {
  const payload = JSON.parse(line) as { id?: number; ok?: boolean; result?: unknown; error?: string };
  if (typeof payload.id !== "number") {
    return;
  }
  const pending = pendingPatchRequests.get(payload.id);
  if (!pending) {
    return;
  }
  pendingPatchRequests.delete(payload.id);
  if (payload.ok) {
    pending.resolve(payload.result);
  } else {
    pending.reject(new Error(payload.error ?? "Patch control request failed."));
  }
}

function rejectPendingPatchRequests(error: Error): void {
  for (const pending of pendingPatchRequests.values()) {
    pending.reject(error);
  }
  pendingPatchRequests.clear();
}

async function refreshPatchParams(): Promise<void> {
  try {
    const result = await sendPatchControlRequest<{ params: PatchParamPayload[] }>("getParams");
    if (!result || !Array.isArray(result.params)) {
      return;
    }
    patchPanelState = {
      ...patchPanelState,
      params: mergePatchParams(result.params, patchPanelState.params),
    };
    postPatchPanelState();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    patchPanelState = {
      ...patchPanelState,
      error: message,
    };
    postPatchPanelState();
  }
}

async function reapplyCachedPatchParams(): Promise<void> {
  for (const param of patchPanelState.params) {
    if (param.value === null) {
      continue;
    }
    const dispatch = patchParamDispatchState(param.name);
    dispatch.runtimeValue = undefined;
    dispatch.targetValue = undefined;
    stopPatchParamSmoothing(param.name);
    queuePatchParamSend(param.name, param.value);
  }
}

function patchParamDispatchState(name: string): PatchParamDispatchState {
  let state = patchParamDispatch.get(name);
  if (!state) {
    state = {};
    patchParamDispatch.set(name, state);
  }
  return state;
}

function clearPatchParamDispatch(): void {
  for (const state of patchParamDispatch.values()) {
    if (state.timer) {
      clearInterval(state.timer);
    }
  }
  patchParamDispatch.clear();
}

function updatePatchParamState(
  name: string,
  update: (param: PatchParamState) => PatchParamState,
): PatchParamState | undefined {
  let nextParam: PatchParamState | undefined;
  patchPanelState = {
    ...patchPanelState,
    params: patchPanelState.params.map((param) => {
      if (param.name !== name) {
        return param;
      }
      nextParam = update(param);
      return nextParam;
    }),
  };
  return nextParam;
}

function currentPatchParam(name: string): PatchParamState | undefined {
  return patchPanelState.params.find((param) => param.name === name);
}

function patchParamDefaultValue(param: PatchParamState): PatchScalarValue {
  return initialParamValue(param);
}

function numericPatchValue(value: PatchScalarValue | undefined): number | undefined {
  return typeof value === "number" ? value : undefined;
}

function normalizeSmoothingMs(value: number | undefined): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return 0;
  }
  return Math.max(0, Math.round(value));
}

function stopPatchParamSmoothing(name: string): void {
  const state = patchParamDispatch.get(name);
  if (!state?.timer) {
    return;
  }
  clearInterval(state.timer);
  state.timer = undefined;
  state.targetValue = undefined;
}

function queuePatchParamSend(name: string, value: PatchScalarValue): void {
  if (value === null || !patchPanelState.connected) {
    return;
  }
  const dispatch = patchParamDispatchState(name);
  dispatch.runtimeValue = value;
  void sendPatchControlRequest("setParam", { name, value })
    .then(() => {
      if (!patchPanelState.error) {
        return;
      }
      patchPanelState = {
        ...patchPanelState,
        error: undefined,
      };
      postPatchPanelState();
    })
    .catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      patchPanelState = {
        ...patchPanelState,
        error: message,
      };
      postPatchPanelState();
    });
}

function startPatchParamSmoothing(name: string, targetValue: number, smoothingMs: number): void {
  const dispatch = patchParamDispatchState(name);
  dispatch.targetValue = targetValue;
  if (dispatch.timer) {
    return;
  }

  const stepMs = 16;
  dispatch.timer = setInterval(() => {
    const param = currentPatchParam(name);
    const currentDispatch = patchParamDispatch.get(name);
    if (!param || !currentDispatch) {
      stopPatchParamSmoothing(name);
      return;
    }
    const target = numericPatchValue(currentDispatch.targetValue);
    if (target === undefined) {
      stopPatchParamSmoothing(name);
      return;
    }
    const current =
      numericPatchValue(currentDispatch.runtimeValue) ??
      numericPatchValue(param.value) ??
      target;
    const alpha = Math.min(1, stepMs / Math.max(smoothingMs, stepMs));
    const next = current + (target - current) * alpha;
    currentDispatch.runtimeValue = next;
    queuePatchParamSend(name, next);
    if (Math.abs(target - next) <= Math.max(0.0001, Math.abs(target) * 0.001)) {
      currentDispatch.runtimeValue = target;
      queuePatchParamSend(name, target);
      stopPatchParamSmoothing(name);
    }
  }, stepMs);
}

function applyPatchParamChange(name: string, value: PatchScalarValue): void {
  if (value === null) {
    return;
  }
  const param = updatePatchParamState(name, (current) => ({
    ...current,
    value,
  }));
  if (!param) {
    return;
  }

  if (!patchPanelState.connected) {
    return;
  }

  if (typeof value === "number" && param.smoothingMs > 0) {
    startPatchParamSmoothing(name, value, param.smoothingMs);
    return;
  }

  stopPatchParamSmoothing(name);
  queuePatchParamSend(name, value);
}

function setPatchParamSmoothing(name: string, smoothingMs: number): void {
  const param = updatePatchParamState(name, (current) => ({
    ...current,
    smoothingMs,
  }));
  if (!param) {
    return;
  }
  postPatchPanelState();

  if (smoothingMs === 0) {
    stopPatchParamSmoothing(name);
    if (patchPanelState.connected) {
      queuePatchParamSend(name, param.value);
    }
  }
}

function resetPatchParams(): void {
  clearPatchParamDispatch();
  patchPanelState = {
    ...patchPanelState,
    error: undefined,
    params: patchPanelState.params.map((param) => ({
      ...param,
      value: patchParamDefaultValue(param),
    })),
  };
  postPatchPanelState();

  if (!patchPanelState.connected) {
    return;
  }

  for (const param of patchPanelState.params) {
    queuePatchParamSend(param.name, param.value);
  }
}

function clearPatchPanelMemory(): void {
  clearPatchParamDispatch();
  patchPanelState = {
    ...patchPanelState,
    params: [],
  };
}

function sendPatchControlRequest<T>(command: string, payload?: Record<string, unknown>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (!patchControlSocket || patchControlSocket.destroyed) {
      reject(new Error("Patch control connection is not available."));
      return;
    }
    const id = ++patchControlRequestId;
    pendingPatchRequests.set(id, { resolve, reject });
    const request = JSON.stringify({
      id,
      command,
      ...payload,
    });
    patchControlSocket.write(`${request}\n`, (error?: Error | null) => {
      if (!error) {
        return;
      }
      pendingPatchRequests.delete(id);
      reject(error);
    });
  });
}

function ensurePatchPanel(): void {
  if (patchPanel) {
    patchPanel.reveal(vscode.ViewColumn.Beside);
    postPatchPanelState();
    return;
  }

  patchPanel = vscode.window.createWebviewPanel(
    "omniPatch",
    "Omni Patch",
    vscode.ViewColumn.Beside,
    {
      enableScripts: true,
      retainContextWhenHidden: true,
    },
  );
  patchPanel.webview.html = renderPatchPanelHtml(patchPanel.webview);
  patchPanel.onDidDispose(() => {
    clearPatchPanelMemory();
    patchPanel = undefined;
  });
  patchPanel.webview.onDidReceiveMessage(async (message: unknown) => {
    const payload = message as {
      type?: string;
      path?: string;
      name?: string;
      value?: PatchScalarValue;
      smoothingMs?: number;
    };
    switch (payload.type) {
      case "start":
        await runPatch(payload.path ?? patchPanelState.path);
        break;
      case "stop":
        await stopPatch();
        break;
      case "reset":
        resetPatchParams();
        break;
      case "setParam":
        if (typeof payload.name === "string") {
          applyPatchParamChange(payload.name, payload.value ?? null);
        }
        break;
      case "setSmoothing":
        if (typeof payload.name === "string") {
          setPatchParamSmoothing(payload.name, normalizeSmoothingMs(payload.smoothingMs));
        }
        break;
      default:
        break;
    }
  });
  postPatchPanelState();
  if (patchPanelState.connected) {
    void refreshPatchParams();
  }
}

function postPatchPanelState(): void {
  if (!patchPanel) {
    return;
  }
  void patchPanel.webview.postMessage({
    type: "state",
    state: patchPanelState,
  });
}

function renderPatchPanelHtml(webview: vscode.Webview): string {
  const nonce = String(Date.now());
  const csp = [
    "default-src 'none'",
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `script-src 'nonce-${nonce}'`,
  ].join("; ");

  return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Omni Patch</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #15171b;
        --panel: #1f2329;
        --panel-strong: #282d35;
        --text: #f3f5f7;
        --muted: #9ba6b2;
        --accent: #7fd1b9;
        --accent-strong: #57b89b;
        --danger: #e07a7a;
        --border: #353c46;
      }

      * { box-sizing: border-box; }
      body {
        margin: 0;
        padding: 18px;
        background: linear-gradient(180deg, #121418 0%, #181c22 100%);
        color: var(--text);
        font: 13px/1.45 ui-sans-serif, system-ui, sans-serif;
      }

      .shell {
        display: grid;
        gap: 16px;
      }

      .header, .params {
        background: var(--panel);
        border: 1px solid var(--border);
        border-radius: 14px;
        padding: 14px;
      }

      .title {
        font-size: 16px;
        font-weight: 700;
      }

      .meta {
        margin-top: 6px;
        color: var(--muted);
        word-break: break-word;
      }

      .status {
        margin-top: 6px;
        color: var(--accent);
      }

      .error {
        margin-top: 8px;
        color: var(--danger);
      }

      .actions {
        margin-top: 12px;
        display: flex;
        gap: 10px;
      }

      button {
        border: 0;
        border-radius: 999px;
        padding: 10px 16px;
        font: inherit;
        font-weight: 700;
        cursor: pointer;
      }

      button.primary {
        background: var(--accent);
        color: #0b1713;
      }

      button.primary:hover {
        background: var(--accent-strong);
      }

      button.secondary {
        background: var(--panel-strong);
        color: var(--text);
      }

      button:disabled {
        cursor: default;
        opacity: 0.5;
      }

      .params-header {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        gap: 12px;
        margin-bottom: 12px;
      }

      .params-title {
        font-size: 14px;
        font-weight: 700;
      }

      .params-subtitle {
        color: var(--muted);
      }

      .param-list {
        display: grid;
        gap: 12px;
      }

      .param {
        background: var(--panel-strong);
        border: 1px solid var(--border);
        border-radius: 12px;
        padding: 12px;
      }

      .param-head {
        display: flex;
        justify-content: space-between;
        gap: 12px;
      }

      .param-name {
        font-weight: 700;
      }

      .param-type {
        color: var(--muted);
      }

      .param-settings {
        display: grid;
        gap: 8px;
        margin-top: 10px;
      }

      .param-setting {
        display: grid;
        gap: 6px;
      }

      .param-setting-label {
        color: var(--muted);
        font-size: 11px;
        letter-spacing: 0.02em;
        text-transform: uppercase;
      }

      .param-controls {
        display: grid;
        gap: 10px;
        margin-top: 10px;
      }

      .param-range {
        width: 100%;
      }

      .param-number {
        width: 100%;
        border: 1px solid var(--border);
        border-radius: 10px;
        background: #111418;
        color: var(--text);
        padding: 8px 10px;
        font: inherit;
      }

      .param-toggle {
        display: inline-flex;
        align-items: center;
        gap: 8px;
        color: var(--text);
      }

      .empty {
        color: var(--muted);
      }
    </style>
  </head>
  <body>
    <div class="shell">
      <section class="header">
        <div class="title">Omni Patch</div>
        <div class="meta" id="path"></div>
        <div class="status" id="status"></div>
        <div class="error" id="error"></div>
        <div class="actions">
          <button class="primary" id="start">Start</button>
          <button class="secondary" id="stop">Stop</button>
          <button class="secondary" id="reset">Reset</button>
        </div>
      </section>
      <section class="params">
        <div class="params-header">
          <div class="params-title">Params</div>
          <div class="params-subtitle" id="params-summary"></div>
        </div>
        <div class="param-list" id="params"></div>
      </section>
    </div>
    <script nonce="${nonce}">
      const vscode = acquireVsCodeApi();
      const state = {
        running: false,
        connected: false,
        path: "",
        status: "Stopped",
        error: "",
        params: [],
      };

      const startButton = document.getElementById("start");
      const stopButton = document.getElementById("stop");
      const resetButton = document.getElementById("reset");
      const pathNode = document.getElementById("path");
      const statusNode = document.getElementById("status");
      const errorNode = document.getElementById("error");
      const paramsSummaryNode = document.getElementById("params-summary");
      const paramsNode = document.getElementById("params");

      startButton.addEventListener("click", () => {
        vscode.postMessage({ type: "start", path: state.path || undefined });
      });
      stopButton.addEventListener("click", () => {
        vscode.postMessage({ type: "stop" });
      });
      resetButton.addEventListener("click", () => {
        vscode.postMessage({ type: "reset" });
      });

      window.addEventListener("message", (event) => {
        const message = event.data;
        if (!message || message.type !== "state") {
          return;
        }
        Object.assign(state, message.state);
        render();
      });

      function render() {
        pathNode.textContent = state.path ? state.path : "No patch selected";
        statusNode.textContent = state.connected ? state.status : state.running ? state.status + " (connecting controls...)" : state.status;
        errorNode.textContent = state.error || "";
        paramsSummaryNode.textContent = state.params.length === 0 ? "No scalar params" : state.params.length + " control" + (state.params.length === 1 ? "" : "s");

        startButton.disabled = !state.path || state.running;
        stopButton.disabled = !state.running;
        resetButton.disabled = state.params.length === 0;

        paramsNode.replaceChildren();
        if (state.params.length === 0) {
          const empty = document.createElement("div");
          empty.className = "empty";
          empty.textContent = "This patch exposes no scalar params.";
          paramsNode.appendChild(empty);
          return;
        }

        for (const param of state.params) {
          const card = document.createElement("div");
          card.className = "param";

          const head = document.createElement("div");
          head.className = "param-head";
          head.innerHTML = '<div class="param-name"></div><div class="param-type"></div>';
          head.querySelector(".param-name").textContent = param.name;
          head.querySelector(".param-type").textContent = param.type;
          card.appendChild(head);

          const controls = document.createElement("div");
          controls.className = "param-controls";
          card.appendChild(controls);

          if (param.type === "bool") {
            const label = document.createElement("label");
            label.className = "param-toggle";
            const input = document.createElement("input");
            input.type = "checkbox";
            input.checked = Boolean(param.value);
            input.disabled = !state.connected;
            input.addEventListener("change", () => {
              vscode.postMessage({ type: "setParam", name: param.name, value: input.checked });
            });
            const text = document.createElement("span");
            text.textContent = input.checked ? "On" : "Off";
            input.addEventListener("change", () => {
              text.textContent = input.checked ? "On" : "Off";
            });
            label.append(input, text);
            controls.appendChild(label);
          } else {
            const hasRange = Number.isFinite(param.rangeMin) && Number.isFinite(param.rangeMax);
            const step = param.type === "i32" || param.type === "i64" ? "1" : "0.001";
            const currentValue = typeof param.value === "number" ? param.value : 0;

            if (hasRange) {
              const range = document.createElement("input");
              range.className = "param-range";
              range.type = "range";
              range.min = String(param.rangeMin);
              range.max = String(param.rangeMax);
              range.step = step;
              range.value = String(currentValue);
              range.disabled = !state.connected;
              controls.appendChild(range);

              const number = document.createElement("input");
              number.className = "param-number";
              number.type = "number";
              number.min = String(param.rangeMin);
              number.max = String(param.rangeMax);
              number.step = step;
              number.value = String(currentValue);
              number.disabled = !state.connected;
              controls.appendChild(number);

              const sync = (nextValue) => {
                range.value = nextValue;
                number.value = nextValue;
              };

              range.addEventListener("input", () => {
                sync(range.value);
                vscode.postMessage({ type: "setParam", name: param.name, value: Number(range.value) });
              });
              number.addEventListener("change", () => {
                sync(number.value);
                vscode.postMessage({ type: "setParam", name: param.name, value: Number(number.value) });
              });
            } else {
              const number = document.createElement("input");
              number.className = "param-number";
              number.type = "number";
              number.step = step;
              number.value = String(currentValue);
              number.disabled = !state.connected;
              number.addEventListener("change", () => {
                vscode.postMessage({ type: "setParam", name: param.name, value: Number(number.value) });
              });
              controls.appendChild(number);
            }

            const settings = document.createElement("div");
            settings.className = "param-settings";
            const smoothing = document.createElement("div");
            smoothing.className = "param-setting";
            const smoothingLabel = document.createElement("div");
            smoothingLabel.className = "param-setting-label";
            smoothingLabel.textContent = "Smoothing (ms)";
            const smoothingInput = document.createElement("input");
            smoothingInput.className = "param-number";
            smoothingInput.type = "number";
            smoothingInput.min = "0";
            smoothingInput.step = "1";
            smoothingInput.value = String(param.smoothingMs || 0);
            smoothingInput.addEventListener("change", () => {
              const nextValue = Math.max(0, Math.round(Number(smoothingInput.value) || 0));
              smoothingInput.value = String(nextValue);
              vscode.postMessage({ type: "setSmoothing", name: param.name, smoothingMs: nextValue });
            });
            smoothing.append(smoothingLabel, smoothingInput);
            settings.appendChild(smoothing);
            controls.appendChild(settings);
          }

          paramsNode.appendChild(card);
        }
      }

      render();
    </script>
  </body>
</html>`;
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  const { command, extraArgs } = omniExecutableConfig();
  const args = [...extraArgs, "lsp"];
  const fileWatcher = vscode.workspace.createFileSystemWatcher("**/*.omni");
  context.subscriptions.push(fileWatcher);

  const serverOptions: ServerOptions = {
    run: {
      command,
      args,
      transport: TransportKind.stdio,
    },
    debug: {
      command,
      args,
      transport: TransportKind.stdio,
    },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "omni" }],
    synchronize: {
      fileEvents: fileWatcher,
    },
    outputChannel: serverOutput,
    traceOutputChannel: serverOutput,
  };

  client = new LanguageClient("omni-lsp", "Omni Language Server", serverOptions, clientOptions);
  await client.start();
}
