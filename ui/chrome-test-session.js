// Chrome's built-in CDP endpoint supplies trusted input and AudioWorklet checks.
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export async function openChrome(t, args = []) {
  const profile = await mkdtemp(join(tmpdir(), "onda-chrome-test-"));
  let browser, socket;
  const beforeClose = [];
  t.after(async () => {
    for (const callback of beforeClose) {
      try {
        await callback();
      } catch {}
    }
    socket?.close();
    if (browser && browser.exitCode === null) {
      browser.kill();
      await once(browser, "exit");
    }
    await rm(profile, {
      recursive: true,
      force: true,
      maxRetries: 5,
      retryDelay: 100,
    });
  });
  const endpoint = Promise.withResolvers();
  let output = "";
  browser = spawn(
    process.env.CHROME_BIN || "chromium",
    [
      ...(process.env.ONDA_UI_CHROME_HEADLESS === "0" ? [] : ["--headless"]),
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--mute-audio",
      "--enable-unsafe-webgpu",
      "--enable-unsafe-swiftshader",
      "--ignore-gpu-blocklist",
      ...args,
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "about:blank",
    ],
    {
      env: { ...process.env, DISPLAY: process.env.DISPLAY || ":1" },
      stdio: ["ignore", "ignore", "pipe"],
    },
  );
  browser.on("error", endpoint.reject);
  browser.stderr.on("data", (chunk) => {
    output = (output + chunk).slice(-24000);
    const url = output.match(/DevTools listening on (ws:\/\/\S+)/)?.[1];
    if (url) endpoint.resolve(url);
  });
  const timer = setTimeout(
    () => endpoint.reject(new Error(output || "Chrome did not start")),
    20000,
  );
  let url;
  try {
    url = await endpoint.promise;
  } finally {
    clearTimeout(timer);
  }
  socket = new WebSocket(url);
  await once(socket, "open");
  let id = 0;
  const pending = new Map(),
    listeners = new Set();
  socket.addEventListener("message", ({ data }) => {
    const value = JSON.parse(data);
    if (value.id) {
      const task = pending.get(value.id);
      pending.delete(value.id);
      if (value.error) task?.reject(new Error(JSON.stringify(value.error)));
      else task?.resolve(value.result);
    } else for (const listener of listeners) listener(value);
  });
  socket.addEventListener("close", () => {
    for (const task of pending.values())
      task.reject(new Error("Chrome connection closed"));
    pending.clear();
  });
  const call = (method, params = {}, sessionId) => {
    if (socket.readyState !== WebSocket.OPEN)
      return Promise.reject(new Error("Chrome connection is not open"));
    const task = Promise.withResolvers(),
      key = ++id;
    pending.set(key, task);
    socket.send(JSON.stringify({ id: key, method, params, sessionId }));
    return task.promise;
  };
  return {
    call,
    beforeClose: (callback) => beforeClose.push(callback),
    async page({ width = 1280, height = 1000, source = "" } = {}) {
      const { targetId } = await call("Target.createTarget", {
        url: "about:blank",
      });
      const { sessionId } = await call("Target.attachToTarget", {
        targetId,
        flatten: true,
      });
      const contexts = new Map(),
        events = new Set();
      const page = (method, params = {}) => call(method, params, sessionId);
      listeners.add((message) => {
        if (message.sessionId !== sessionId) return;
        if (message.method === "Runtime.executionContextCreated")
          contexts.set(message.params.context.id, message.params.context);
        if (message.method === "Runtime.executionContextDestroyed")
          contexts.delete(message.params.executionContextId);
        if (message.method === "Runtime.executionContextsCleared")
          contexts.clear();
        for (const callback of events) callback(message);
      });
      await page("Page.enable");
      await page("Runtime.enable");
      await page("Emulation.setDeviceMetricsOverride", {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
      });
      if (source)
        await page("Page.addScriptToEvaluateOnNewDocument", { source });
      return {
        call: page,
        contexts,
        navigate: (url) => page("Page.navigate", { url }),
        async evaluate(expression, contextId) {
          const r = await page("Runtime.evaluate", {
            expression,
            awaitPromise: true,
            returnByValue: true,
            ...(contextId ? { contextId } : {}),
          });
          if (r.exceptionDetails)
            throw new Error(JSON.stringify(r.exceptionDetails));
          return r.result.value;
        },
        event(method) {
          return new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
              events.delete(listener);
              reject(new Error(`Timed out waiting for ${method}`));
            }, 10000);
            const listener = (event) => {
              if (event.method === method) {
                clearTimeout(timer);
                events.delete(listener);
                resolve(event.params);
              }
            };
            events.add(listener);
          });
        },
      };
    },
  };
}

export async function eventually(check, description, timeout = 30000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const result = await check();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 30));
  }
  throw new Error(`Timed out waiting for ${description}`);
}
