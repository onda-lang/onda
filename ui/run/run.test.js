import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "esbuild";
import { testNumberKeyboardEditing } from "./run.keyboard-tests.js";

// Real browser coverage with no DOM emulation or browser-driver dependency.
// FIREFOX_BIN may select a locally installed Firefox executable.
test("run view and playground preserve editing and responsive layouts", { timeout: 30_000 }, async (t) => {
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
  const website = await readFile(new URL("../../website/playground/index.html", import.meta.url), "utf8");
  const siteHeader = (await readFile(new URL("../../website/_includes/header.html", import.meta.url), "utf8"))
    .replace(/{% if[^%]*%}[\s\S]*?{% endif %}/g, "")
    .replace(/{{\s*'([^']*)'[^}]*}}/g, "$1");
  const example = await readFile(new URL("../../examples/web/onda_wasm_playground/index.html", import.meta.url), "utf8");
  const editor = await build({
    entryPoints: [fileURLToPath(new URL("../playground/layout.fixture.js", import.meta.url))],
    outfile: "layout-editor.js",
    bundle: true, write: false, format: "esm", loader: { ".onda": "text" },
  });
  const playgroundFixture = (page, styles, header = "") => `<!doctype html>
    <html lang="en" data-theme="dark"><head><meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">${styles}
    <link rel="stylesheet" href="/playground.css"></head>
    <body class="play-page">${header}${page.match(/<main[\s\S]*?<\/main>/)[0]
      .replace("data-run-view", 'src="/layout" data-run-view')}
    <script type="module" src="/layout-editor.js"></script></body></html>`;
  const routes = new Map([
    ["/", ["text/html", html.replace("</body>",
      '<script type="module" src="/tests.js"></script></body>')]],
    ["/keyboard", ["text/html", html]],
    ["/layout", ["text/html", html]],
    ["/website-layout", ["text/html", playgroundFixture(website,
      '<link rel="stylesheet" href="/site.css">', siteHeader)]],
    ["/example-layout", ["text/html", playgroundFixture(example,
      example.match(/<style>[\s\S]*?<\/style>/)[0])]],
    ["/site.css", ["text/css", await readFile(new URL("../../website/assets/site/styles.css", import.meta.url))]],
    ["/layout-editor.js", ["text/javascript", editor.outputFiles.find(file => file.path.endsWith(".js")).text]],
    ["/playground.css", ["text/css", editor.outputFiles.find(file => file.path.endsWith(".css")).text]],
    ["/assets/svg/onda-logo-circle.svg", ["image/svg+xml",
      await readFile(new URL("../../assets/svg/onda-logo-circle.svg", import.meta.url))]],
    ["/assets/svg/onda-logo-dark-circle.svg", ["image/svg+xml",
      await readFile(new URL("../../assets/svg/onda-logo-dark-circle.svg", import.meta.url))]],
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
