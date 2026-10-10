import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { testNumberKeyboardEditing } from "./run.keyboard-tests.js";

// Real browser coverage with no DOM emulation or browser-driver dependency.
// FIREFOX_BIN may select a locally installed Firefox executable.
test("run view preserves editing across host updates", { timeout: 30_000 }, async (t) => {
  const profile = await mkdtemp(join(tmpdir(), "onda-run-view-test-"));
  let browser;
  let server;
  t.after(async () => {
    if (browser?.pid && browser.exitCode === null && browser.signalCode === null) {
      const exited = once(browser, "exit");
      browser.kill();
      await exited;
    }
    if (server?.listening) {
      await new Promise(resolve => {
        server.close(resolve);
        server.closeAllConnections();
      });
    }
    await rm(profile, { recursive: true, force: true });
  });
  const bridge = `<script>
    window.__hostBridge = { mode: "wry", theme: "light", ondaVersion: "0.0.0-test" };
    window.__testMessages = [];
    window.ipc = { postMessage(message) {
      window.__testMessages.push(JSON.parse(message));
    }};
  </script>`;
  const html = (await readFile(new URL("./run.html", import.meta.url), "utf8"))
    .replace("<head>", "<head>" + bridge);
  const routes = new Map([
    ["/", ["text/html", html.replace("</body>",
      '<script type="module" src="/tests.js"></script></body>')]],
    ["/keyboard", ["text/html", html]],
    ["/tests.js", ["text/javascript",
      await readFile(new URL("./run.browser-tests.js", import.meta.url))]],
    ["/number-input.js", ["text/javascript",
      await readFile(new URL("../number-input.js", import.meta.url))]],
    ["/param-control.js", ["text/javascript", await readFile(new URL(
      "../../packages/onda_processor_abi/src/param-control.js", import.meta.url))]],
  ]);
  const report = Promise.withResolvers();
  const debugEndpoint = Promise.withResolvers();
  server = createServer(async (request, response) => {
    if (request.method === "POST" && request.url === "/result") {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      try { report.resolve(JSON.parse(Buffer.concat(chunks).toString())); }
      catch (error) { report.reject(error); }
      response.end();
      return;
    }
    const route = routes.get(request.url);
    response.writeHead(route ? 200 : 404,
      { "Content-Type": route?.[0] ?? "text/plain" });
    response.end(route?.[1] ?? "Not found");
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  browser = spawn(process.env.FIREFOX_BIN || "firefox", [
    "--headless", "--no-remote", "--profile", profile,
    "--remote-debugging-port", "0",
    `http://127.0.0.1:${server.address().port}/`,
  ], { stdio: ["ignore", "ignore", "pipe"] });
  let browserOutput = "";
  browser.stderr.setEncoding("utf8");
  browser.stderr.on("data", chunk => {
    browserOutput += chunk;
    const endpoint = browserOutput.match(/WebDriver BiDi listening on (ws:\/\/\S+)/)?.[1];
    if (endpoint) debugEndpoint.resolve(endpoint);
  });
  const browserFailed = error => {
    report.reject(error);
    debugEndpoint.reject(error);
  };
  browser.on("error", browserFailed);
  browser.on("exit", code => browserFailed(new Error(`Firefox exited: ${code}\n${browserOutput}`)));
  const [result, endpoint] = await Promise.race([
    Promise.all([report.promise, debugEndpoint.promise]),
    new Promise((_, reject) => t.signal.addEventListener("abort", () =>
      reject(new Error("Browser test timed out")), { once: true })),
  ]);
  assert.equal(result.error, undefined, result.error);
  assert.ok(result.results.length >= 20);
  for (const message of result.results) t.diagnostic(message);
  await testNumberKeyboardEditing(t, endpoint, `http://127.0.0.1:${server.address().port}/keyboard`);
});
