import * as childProcess from "child_process";
import * as fs from "fs";
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
  value: PatchScalarValue;
  default: number | null;
  rangeMin: number | null;
  rangeMax: number | null;
  scalar: boolean;
}

interface PatchParamState extends PatchParamPayload {}

interface PatchBufferPayload {
  index: number;
  name: string;
  type: string;
  channelsKind: "mono" | "static" | "dynamic";
  channelsStatic: number | null;
  loadedPath: string | null;
}

interface PatchBufferState extends PatchBufferPayload {}

interface PatchReadyEvent {
  event: "ready";
  path: string;
  port: number;
  params: PatchParamPayload[];
  buffers: PatchBufferPayload[];
  outputChannels: number;
  inputDevices: string[];
  outputDevices: string[];
  currentInputDevice: string | null;
  currentOutputDevice: string | null;
}

interface PatchPanelState {
  running: boolean;
  connected: boolean;
  path?: string;
  status: string;
  error?: string;
  outputChannels: number;
  buffers: PatchBufferState[];
  params: PatchParamState[];
  inputDevices: string[];
  outputDevices: string[];
  currentInputDevice: string | null;
  currentOutputDevice: string | null;
}

interface PendingControlRequest {
  resolve: (value: any) => void;
  reject: (error: Error) => void;
}

let client: LanguageClient | undefined;
let extensionContext: vscode.ExtensionContext | undefined;
let patchProcess: childProcess.ChildProcessWithoutNullStreams | undefined;
let patchPath: string | undefined;
let patchOutput: vscode.OutputChannel | undefined;
let serverOutput: vscode.OutputChannel | undefined;
let patchPanel: vscode.WebviewPanel | undefined;
let patchPanelReady = false;
let patchControlSocket: net.Socket | undefined;
let patchControlBuffer = "";
let patchStdoutBuffer = "";
let patchControlRequestId = 0;
let stoppingPatchPid: number | undefined;
const pendingPatchRequests = new Map<number, PendingControlRequest>();
let scopePollingTimer: NodeJS.Timeout | undefined;
let scopePollingInFlight = false;
let patchPanelState: PatchPanelState = {
  running: false,
  connected: false,
  status: "Stopped",
  outputChannels: 0,
  buffers: [],
  params: [],
  inputDevices: [],
  outputDevices: [],
  currentInputDevice: null,
  currentOutputDevice: null,
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
    outputChannels: patchPanelState.path === fsPath ? patchPanelState.outputChannels : 0,
    buffers: patchPanelState.path === fsPath ? patchPanelState.buffers : [],
    params: preservedParams,
    inputDevices: patchPanelState.inputDevices,
    outputDevices: patchPanelState.outputDevices,
    currentInputDevice: patchPanelState.currentInputDevice,
    currentOutputDevice: patchPanelState.currentOutputDevice,
  };
  postPatchPanelState();

  if (patchProcess && patchPath === fsPath && !options?.restart) {
    revealPatchPanel();
    return;
  }

  await stopPatch({ silent: true, preservePath: fsPath });

  const { command, extraArgs } = omniExecutableConfig();
  const args = [...extraArgs, "preview", "play", fsPath, "--forever", "--control-json"];
  if (patchPanelState.currentInputDevice) {
    args.push("--input-device", patchPanelState.currentInputDevice);
  }
  if (patchPanelState.currentOutputDevice) {
    args.push("--output-device", patchPanelState.currentOutputDevice);
  }
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
  revealPatchPanel();

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
    patchOutput?.show(true);
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
    patchOutput?.show(true);
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
        outputChannels: payload.outputChannels ?? 0,
        buffers: mergePatchBuffers(payload.buffers ?? [], patchPanelState.buffers),
        params: mergePatchParams(payload.params, patchPanelState.params),
        inputDevices: payload.inputDevices ?? [],
        outputDevices: payload.outputDevices ?? [],
        currentInputDevice: payload.currentInputDevice ?? null,
        currentOutputDevice: payload.currentOutputDevice ?? null,
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

// Merge new param metadata with previously-preserved user values (across restarts).
// Default value hydration is handled by the webview (preview.html).
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
      };
    });
}

function mergePatchBuffers(
  buffers: PatchBufferPayload[],
  existing: PatchBufferState[],
): PatchBufferState[] {
  return buffers.map((buffer) => {
    const previous = existing.find((item) => item.name === buffer.name);
    return {
      ...buffer,
      loadedPath: previous?.loadedPath ?? buffer.loadedPath,
    };
  });
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
    void Promise.all([refreshPatchParams(), refreshPatchBuffers()]).then(() => {
      reapplyCachedPatchParams();
      reapplyCachedPatchBuffers();
    });
    startScopePolling();
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
    stopScopePolling();
    patchPanelState = {
      ...patchPanelState,
      connected: false,
      error: error.message,
    };
    postPatchPanelState();
  });
  socket.on("close", () => {
    stopScopePolling();
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

async function refreshPatchBuffers(): Promise<void> {
  try {
    const result = await sendPatchControlRequest<{ buffers: PatchBufferPayload[] }>("getBuffers");
    if (!result || !Array.isArray(result.buffers)) {
      return;
    }
    patchPanelState = {
      ...patchPanelState,
      buffers: mergePatchBuffers(result.buffers, patchPanelState.buffers),
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
    queuePatchParamSend(param.name, param.value);
  }
}

function reapplyCachedPatchBuffers(): void {
  for (const buffer of patchPanelState.buffers) {
    if (!buffer.loadedPath) {
      continue;
    }
    void bindPatchBufferFile(buffer.name, buffer.loadedPath, { silent: true });
  }
}

function clearPatchParamDispatch(): void {
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

function initialParamValue(param: Pick<PatchParamPayload, "type" | "value" | "default" | "rangeMin">): PatchScalarValue {
  if (param.type === "bool") {
    if (param.value !== null && param.value !== undefined) {
      return param.value !== 0;
    }
    if (param.default !== null && param.default !== undefined) {
      return param.default !== 0;
    }
    return false;
  }
  if (param.value !== null && param.value !== undefined) {
    return param.value;
  }
  if (param.default !== null && param.default !== undefined) {
    return param.default;
  }
  if (param.rangeMin !== null && param.rangeMin !== undefined) {
    return param.rangeMin;
  }
  return 0;
}

function patchParamDefaultValue(param: PatchParamState): PatchScalarValue {
  return initialParamValue(param);
}

function queuePatchParamSend(name: string, value: PatchScalarValue): void {
  if (value === null || !patchPanelState.connected) {
    return;
  }
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

function describePatchBufferChannels(buffer: PatchBufferPayload): string {
  switch (buffer.channelsKind) {
    case "mono":
      return "mono";
    case "static":
      return `${buffer.channelsStatic ?? 0}-channel`;
    case "dynamic":
      return "dynamic channels";
    default:
      return "unknown";
  }
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
  queuePatchParamSend(name, value);
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

async function bindPatchBufferFile(
  name: string,
  filePath: string,
  options?: { silent?: boolean },
): Promise<void> {
  try {
    await sendPatchControlRequest("bindBufferWav", { name, path: filePath });
    patchPanelState = {
      ...patchPanelState,
      error: undefined,
      buffers: patchPanelState.buffers.map((buffer) =>
        buffer.name === name
          ? {
              ...buffer,
              loadedPath: filePath,
            }
          : buffer,
      ),
    };
    postPatchPanelState();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    patchPanelState = {
      ...patchPanelState,
      error: message,
    };
    postPatchPanelState();
    if (!options?.silent) {
      void vscode.window.showErrorMessage(`Failed to bind preview buffer '${name}': ${message}`);
    }
  }
}

async function clearPatchBuffer(name: string): Promise<void> {
  try {
    await sendPatchControlRequest("clearBuffer", { name });
    patchPanelState = {
      ...patchPanelState,
      error: undefined,
      buffers: patchPanelState.buffers.map((buffer) =>
        buffer.name === name
          ? {
              ...buffer,
              loadedPath: null,
            }
          : buffer,
      ),
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

async function choosePatchBufferFile(name: string): Promise<void> {
  const picked = await vscode.window.showOpenDialog({
    canSelectMany: false,
    openLabel: `Bind '${name}' buffer`,
    filters: {
      "Wave Audio": ["wav"],
    },
  });
  const filePath = picked?.[0]?.fsPath;
  if (!filePath) {
    return;
  }
  await bindPatchBufferFile(name, filePath);
}

function clearPatchPanelMemory(): void {
  clearPatchParamDispatch();
  patchPanelState = {
    ...patchPanelState,
    buffers: [],
    params: [],
    inputDevices: [],
    outputDevices: [],
    currentInputDevice: null,
    currentOutputDevice: null,
  };
}

function normalizeDeviceSelection(name: string | null | undefined): string | null {
  if (typeof name !== "string") {
    return null;
  }
  const trimmed = name.trim();
  return trimmed.length > 0 ? trimmed : null;
}

async function updatePatchDeviceSelection(
  kind: "input" | "output",
  name: string | null | undefined,
): Promise<void> {
  const next = normalizeDeviceSelection(name);
  patchPanelState = {
    ...patchPanelState,
    currentInputDevice: kind === "input" ? next : patchPanelState.currentInputDevice,
    currentOutputDevice: kind === "output" ? next : patchPanelState.currentOutputDevice,
    error: undefined,
  };
  postPatchPanelState();

  if (!patchPanelState.running || !patchPanelState.path) {
    return;
  }
  await runPatch(patchPanelState.path, { restart: true });
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
  patchPanelReady = false;
  patchPanel.onDidDispose(() => {
    stopScopePolling();
    patchPanelReady = false;
    patchPanel = undefined;
    void stopPatch({ silent: true });
    clearPatchPanelMemory();
  });
  patchPanel.webview.onDidReceiveMessage(async (message: unknown) => {
    const payload = message as {
      type?: string;
      path?: string;
      name?: string | null;
      value?: PatchScalarValue;
      filePath?: string;
    };
    switch (payload.type) {
      case "webviewReady":
        patchPanelReady = true;
        postPatchPanelState();
        if (patchPanelState.connected) {
          void Promise.all([refreshPatchParams(), refreshPatchBuffers()]);
        }
        break;
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
      case "setInputDevice":
        await updatePatchDeviceSelection("input", payload.name);
        break;
      case "setOutputDevice":
        await updatePatchDeviceSelection("output", payload.name);
        break;
      case "chooseBufferFile":
        if (typeof payload.name === "string") {
          await choosePatchBufferFile(payload.name);
        }
        break;
      case "bindBufferFile":
        if (typeof payload.name === "string" && typeof payload.filePath === "string") {
          await bindPatchBufferFile(payload.name, payload.filePath);
        }
        break;
      case "clearBuffer":
        if (typeof payload.name === "string") {
          await clearPatchBuffer(payload.name);
        }
        break;
      default:
        break;
    }
  });
  patchPanel.webview.html = renderSharedPreviewHtml(patchPanel.webview);
  postPatchPanelState();
  if (patchPanelState.connected) {
    void Promise.all([refreshPatchParams(), refreshPatchBuffers()]);
  }
}

function revealPatchPanel(): void {
  if (!patchPanel) {
    return;
  }
  patchPanel.reveal(patchPanel.viewColumn);
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

function startScopePolling(): void {
  stopScopePolling();
  scopePollingTimer = setInterval(pollScopeData, 33);
}

function stopScopePolling(): void {
  if (scopePollingTimer !== undefined) {
    clearInterval(scopePollingTimer);
    scopePollingTimer = undefined;
  }
  scopePollingInFlight = false;
}

function pollScopeData(): void {
  if (scopePollingInFlight || !patchPanelState.connected || !patchPanel || !patchPanelReady) {
    return;
  }
  scopePollingInFlight = true;
  sendPatchControlRequest<{ channels: number; samples: number[] }>("getScopeData", { maxFrames: 2048 })
    .then((result) => {
      scopePollingInFlight = false;
      if (patchPanel && patchPanelReady) {
        void patchPanel.webview.postMessage({
          type: "scopeData",
          channels: result.channels,
          samples: result.samples,
        });
      }
    })
    .catch(() => {
      scopePollingInFlight = false;
    });
}

function renderSharedPreviewHtml(webview: vscode.Webview): string {
  const csp = [
    "default-src 'none'",
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `script-src ${webview.cspSource} 'unsafe-inline'`,
  ].join("; ");

  // Locate the shared preview HTML.
  // In a packaged extension it lives at <extensionPath>/out/preview.html (copied at build time).
  // During development it also exists at <extensionPath>/../shared/preview.html.
  const extRoot = extensionContext?.extensionPath ?? __dirname;
  const candidates = [
    path.join(extRoot, "out", "preview.html"),
    path.join(extRoot, "..", "shared", "preview.html"),
  ];
  let html: string | undefined;
  let resolvedPath = "";
  for (const candidate of candidates) {
    try {
      html = fs.readFileSync(candidate, "utf-8");
      resolvedPath = candidate;
      break;
    } catch {
      // Try next candidate.
    }
  }
  if (html === undefined) {
    return `<!DOCTYPE html><html><body style="color:#e07a7a;padding:20px;font:14px sans-serif">
      <p>Could not load preview UI.</p>
      <p>Searched:<br/>${candidates.map((c) => `<code>${c}</code>`).join("<br/>")}</p>
    </body></html>`;
  }

  // Inject the VS Code host bridge before the page script runs, and add the CSP header.
  const bridgeScript = `<script>window.__hostBridge = { mode: "vscode" };</script>`;
  const cspMeta = `<meta http-equiv="Content-Security-Policy" content="${csp}" />`;

  // Insert CSP meta after <head> and bridge script before the main <script>.
  html = html.replace("<head>", `<head>\n    ${cspMeta}`);
  html = html.replace("<script>", `${bridgeScript}\n    <script>`);

  return html;
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
