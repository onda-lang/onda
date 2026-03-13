import * as childProcess from "child_process";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let extensionContext: vscode.ExtensionContext | undefined;
let patchProcess: childProcess.ChildProcessWithoutNullStreams | undefined;
let patchPath: string | undefined;
let patchOutput: vscode.OutputChannel | undefined;
let serverOutput: vscode.OutputChannel | undefined;
let stoppingPatchPid: number | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  extensionContext = context;
  patchOutput = vscode.window.createOutputChannel("Omni Patch");
  serverOutput = vscode.window.createOutputChannel("Omni Language Server");
  context.subscriptions.push(patchOutput);
  context.subscriptions.push(serverOutput);

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

  await startClient(context);
}

export async function deactivate(): Promise<void> {
  await stopPatch();

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

async function runPatch(): Promise<void> {
  const document = await currentPatchDocument();
  if (!document) {
    return;
  }

  if (document.isDirty) {
    const saved = await document.save();
    if (!saved) {
      void vscode.window.showErrorMessage("Omni patch must be saved before playback starts.");
      return;
    }
  }

  const fsPath = document.uri.fsPath;
  if (patchProcess && patchPath === fsPath) {
    void vscode.window.showInformationMessage(`Omni patch already running: ${path.basename(fsPath)}`);
    return;
  }

  await stopPatch({ silent: true });

  const { command, extraArgs } = omniExecutableConfig();
  const args = [...extraArgs, "preview", "play", fsPath, "--forever"];
  const cwd = vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath ?? path.dirname(fsPath);
  const child = childProcess.spawn(command, args, {
    cwd,
    stdio: "pipe",
  });

  patchProcess = child;
  patchPath = fsPath;

  patchOutput?.appendLine(`$ ${command} ${args.map(shellQuote).join(" ")}`);

  child.stdout.on("data", (chunk: Buffer) => {
    patchOutput?.append(chunk.toString());
  });
  child.stderr.on("data", (chunk: Buffer) => {
    patchOutput?.append(chunk.toString());
  });
  child.once("error", (error: Error) => {
    const failedPath = fsPath;
    if (patchProcess === child) {
      clearPatchState();
    }
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
      clearPatchState();
    }
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

  patchOutput?.show(true);
  void vscode.window.showInformationMessage(`Running Omni patch: ${path.basename(fsPath)}`);
}

async function stopPatch(options?: { silent?: boolean }): Promise<void> {
  if (!patchProcess) {
    if (!options?.silent) {
      void vscode.window.showInformationMessage("No Omni patch is currently running.");
    }
    return;
  }

  const child = patchProcess;
  const runningPath = patchPath;
  clearPatchState();
  stoppingPatchPid = child.pid;
  child.kill();

  if (!options?.silent && runningPath) {
    void vscode.window.showInformationMessage(`Stopped Omni patch: ${path.basename(runningPath)}`);
  }
}

function clearPatchState(): void {
  patchProcess = undefined;
  patchPath = undefined;
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
    documentSelector: [
      { scheme: "file", language: "omni" },
    ],
    synchronize: {
      fileEvents: fileWatcher,
    },
    outputChannel: serverOutput,
    traceOutputChannel: serverOutput,
  };

  client = new LanguageClient("omni-lsp", "Omni Language Server", serverOptions, clientOptions);
  await client.start();
}
