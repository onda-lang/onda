import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "esbuild";
import { openBrowserSession } from "../browser-test-session.js";

// Exercise the real controller, editor, embedded run view, and browser autoplay
// policy. Only compilation and the processor are replaced with a native tone.
const compilerFixture = `
  export const ONDA_VERSION = "0.0.0-test";
  export async function createCompiler() {
    return {
      async setLspAnalysisOptions() {},
      async sendLspMessage(message) {
        return message.id === undefined ? [] : [{ id: message.id, result: {} }];
      },
      async compileWorkspace(project, options) {
        return { artifact: { wasm: new Uint8Array(), metadata: { compile: {
          sample_rate: options.sampleRate, block_size: options.blockSize,
        }, metadata: { inputs: [], outputs: [{ array_len: 1 }],
          params: [], buffers: [], events: [] } } } };
      },
      async dispose() {},
    };
  }
`;
const processorFixture = `
  export { flattenedAudioChannelCount } from ${JSON.stringify(fileURLToPath(
    new URL("../../packages/onda_webaudio/src/index.js", import.meta.url)))};
  export async function compileOndaProcessorModule() { return null; }
  export async function createOndaAudioProcessorInitialized(context) {
    const node = context.createOscillator();
    node.start();
    window.__processorsCreated += 1;
    return { node, async setParamSmoothingSeconds() {},
      close() { node.stop(); window.__processorsClosed += 1; } };
  }
`;
const instrumentation = `<script>
  window.__audioContexts = [];
  window.__processorsCreated = 0;
  window.__processorsClosed = 0;
  const NativeAudioContext = window.AudioContext;
  window.AudioContext = class extends NativeAudioContext {
    constructor(options) {
      super(options);
      this.resumes = 0;
      this.states = [];
      this.addEventListener("statechange", () => this.states.push(this.state));
      window.__audioContexts.push(this);
    }
    resume() { this.resumes += 1; return super.resume(); }
  };
  window.__waitFor = predicate => new Promise((resolve, reject) => {
    const deadline = performance.now() + 5000;
    const poll = () => {
      if (predicate()) resolve();
      else if (performance.now() >= deadline) reject(new Error("Startup did not settle"));
      else setTimeout(poll, 10);
    };
    poll();
  });
</script>`;

async function testAudioActivation(t, autoplayAllowed) {
    const bundle = await build({
      entryPoints: [fileURLToPath(new URL("./live.js", import.meta.url))],
      outfile: "live.js", bundle: true, write: false, format: "esm",
      loader: { ".onda": "text" },
      plugins: [{ name: "audio-startup-fixture", setup(build) {
        build.onResolve({ filter: /^@onda-lang\/(wasm-compiler|webaudio)$/ }, args =>
          ({ path: args.path, namespace: "fixture" }));
        build.onLoad({ filter: /.*/, namespace: "fixture" }, args => ({
          contents: args.path.endsWith("wasm-compiler") ? compilerFixture : processorFixture,
          resolveDir: fileURLToPath(new URL(".", import.meta.url)),
        }));
      } }],
    });
    const example = await readFile(new URL(
      "../../examples/web/onda_wasm_playground/index.html", import.meta.url), "utf8");
    const page = `<!doctype html><html data-theme="dark"><head>
      <meta charset="utf-8">${example.match(/<style>[\s\S]*?<\/style>/)[0]}
      <link rel="stylesheet" href="/live.css">${instrumentation}</head><body class="play-page">
      ${example.match(/<main[\s\S]*?<\/main>/)[0]}
      <script type="module" src="/live.js"></script></body></html>`;
    const routes = new Map([
      ["/", ["text/html", page]],
      ["/live.js", ["text/javascript", bundle.outputFiles.find(file => file.path.endsWith(".js")).text]],
      ["/live.css", ["text/css", bundle.outputFiles.find(file => file.path.endsWith(".css")).text]],
      ["/run.html", ["text/html", await readFile(new URL("../run/run.html", import.meta.url))]],
      ["/number-input.js", ["text/javascript", await readFile(new URL("../number-input.js", import.meta.url))]],
      ["/param-control.js", ["text/javascript", await readFile(new URL(
        "../../packages/onda_processor_abi/src/param-control.js", import.meta.url))]],
    ]);
    const profile = await mkdtemp(join(tmpdir(), "onda-audio-startup-"));
    let browser;
    const server = createServer((request, response) => {
      const route = routes.get(new URL(request.url, "http://localhost").pathname);
      response.writeHead(route ? 200 : 404, { "Content-Type": route?.[0] ?? "text/plain" });
      response.end(route?.[1] ?? "Not found");
    });
    t.after(async () => {
      if (browser?.pid && browser.exitCode === null && browser.signalCode === null) {
        const exited = once(browser, "exit");
        browser.kill();
        await exited;
      }
      if (server.listening) {
        await new Promise(resolve => {
          server.close(resolve);
          server.closeAllConnections();
        });
      }
      await rm(profile, { recursive: true, force: true });
    });
    await writeFile(join(profile, "user.js"), [
      `user_pref("media.autoplay.default", ${autoplayAllowed ? 0 : 1});`,
      'user_pref("media.autoplay.block-webaudio", true);',
      'user_pref("media.autoplay.blocking_policy", 0);',
    ].join("\n"));
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const url = `http://127.0.0.1:${server.address().port}/`;
    const endpoint = Promise.withResolvers();
    browser = spawn(process.env.FIREFOX_BIN || "firefox", [
      "--headless", "--no-remote", "--profile", profile,
      "--remote-debugging-port", "0", "about:blank",
    ], { stdio: ["ignore", "ignore", "pipe"] });
    let browserOutput = "";
    browser.stderr.setEncoding("utf8");
    browser.stderr.on("data", chunk => {
      browserOutput += chunk;
      const address = browserOutput.match(/WebDriver BiDi listening on (ws:\/\/\S+)/)?.[1];
      if (address) endpoint.resolve(address);
    });
    browser.on("error", endpoint.reject);
    browser.on("exit", code => endpoint.reject(new Error(`Firefox exited: ${code}\n${browserOutput}`)));
    t.signal.addEventListener("abort", () => endpoint.reject(new Error("Browser test timed out")), { once: true });
    const call = await openBrowserSession(t, await endpoint.promise);
    let context;
    const evaluate = async expression => {
      const reply = await call("script.evaluate", {
        expression, target: { context }, awaitPromise: true,
      });
      assert.equal(reply.type, "success", JSON.stringify(reply));
      return reply.result.value;
    };
    const initialize = async () => {
      if (context) await call("browsingContext.close", { context });
      ({ context } = await call("browsingContext.create", { type: "tab" }));
      await call("browsingContext.setViewport", {
        context, viewport: { width: 1366, height: 1000 }, devicePixelRatio: 1,
      });
      await call("browsingContext.navigate", { context, url, wait: "complete" });
      await evaluate(`__waitFor(() => __audioContexts.length && __processorsCreated
        && document.querySelector('[data-run-view]').style.opacity === ""
        && !document.querySelector('[data-run-view]').contentDocument.querySelector('#stop').hidden)
        .catch(error => { throw new Error(error.message + ": " +
          document.querySelector('[data-status]').textContent + "; " +
          document.querySelector('[data-run-view]').contentDocument.querySelector('#error').textContent); })`);
    };
    const settle = () => evaluate("new Promise(resolve => setTimeout(resolve, 100))");
    const pointer = async (selector, embedded, pointerType = "mouse") => {
      const point = JSON.parse(await evaluate(`JSON.stringify((() => {
        const frame = document.querySelector('[data-run-view]');
        const owner = ${embedded} ? frame.contentDocument : document;
        const rect = owner.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
        const origin = ${embedded} ? frame.getBoundingClientRect() : { left: 0, top: 0 };
        return { x: Math.round(origin.left + rect.left + Math.min(10, rect.width / 2)),
          y: Math.round(origin.top + rect.top + Math.min(10, rect.height / 2)) };
      })())`));
      await call("input.performActions", { context,
        actions: [{ type: "pointer", id: pointerType, parameters: { pointerType }, actions: [
          { type: "pointerMove", ...point, duration: 0 },
          { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 },
        ] }],
      });
      await settle();
    };
    const keys = values => call("input.performActions", { context,
      actions: [{ type: "key", id: "keyboard", actions: values.flatMap(value => [
        { type: "keyDown", value }, { type: "keyUp", value },
      ]) }],
    });
    const status = () => evaluate(
      "document.querySelector('[data-run-view]').contentDocument.querySelector('#status').textContent",
    );

    if (autoplayAllowed) {
      await initialize();
      await evaluate("__waitFor(() => __audioContexts[0].state === 'running')");
      assert.doesNotMatch(await status(), /click|tap|press/i);
      assert.equal(await evaluate("document.querySelector('[data-status]').textContent"), "Playing");
      return;
    }

    for (const pointerType of ["mouse", "touch"]) {
      await t.test(`${pointerType} interaction inside the embedded view starts audio`, async () => {
        await initialize();
        assert.equal(await evaluate("__audioContexts[0].state"), "suspended");
        assert.equal(await evaluate(
          "document.querySelector('[data-run-view]').contentDocument.querySelector('#stop').hidden"), false);
        await pointer("#path", true, pointerType);
        assert.equal(await evaluate("__audioContexts[0].state"), "running");
        assert.doesNotMatch(await status(), /click|tap|press/i);
        assert.equal(await evaluate("document.querySelector('[data-status]').textContent"), "Playing");
      });
    }
    await t.test("keyboard input in the editor and embedded view starts audio", async () => {
      for (const embedded of [false, true]) {
        await initialize();
        await evaluate(embedded
          ? "document.querySelector('[data-run-view]').contentDocument.querySelector('#midi-velocity-value').focus()"
          : "document.querySelector('.cm-content').focus()");
        await keys(["1"]);
        await settle();
        assert.equal(await evaluate("__audioContexts[0].state"), "running");
        assert.doesNotMatch(await status(), /click|tap|press/i);
      }
    });
    await t.test("outer-page interaction clears the blocked-audio prompt", async () => {
      await initialize();
      await pointer(".cm-content", false);
      assert.equal(await evaluate("__audioContexts[0].state"), "running");
      assert.doesNotMatch(await status(), /click|tap|press/i);
    });
    await t.test("the message follows audio state changes and embedded-view reloads", async () => {
      await initialize();
      await pointer("#path", true);
      await evaluate("__audioContexts[0].suspend()");
      await settle();
      assert.match(await status(), /Click, tap, or press a key/);
      assert.equal(await evaluate("document.querySelector('[data-status]').textContent"), "Audio ready");
      await evaluate("document.querySelector('[data-run-view]').contentWindow.location.reload()");
      await evaluate(`__waitFor(() => document.querySelector('[data-run-view]').style.opacity === ""
        && !document.querySelector('[data-run-view]').contentDocument.querySelector('#stop')?.hidden)`);
      await pointer("#path", true);
      assert.equal(await evaluate("__audioContexts[0].state"), "running");
      assert.doesNotMatch(await status(), /click|tap|press/i);
    });
    await t.test("Stop cancels blocked playback without resuming it", async () => {
      await initialize();
      const resumes = await evaluate("__audioContexts[0].resumes");
      await pointer("#stop", true);
      assert.equal(await evaluate("__audioContexts[0].state"), "closed");
      assert.equal(await evaluate("__audioContexts[0].resumes"), resumes);
      assert.equal(await evaluate("__audioContexts[0].states.includes('running')"), false);
      await pointer("#path", true);
      await pointer(".cm-content", false);
      await keys(["1"]);
      await settle();
      assert.equal(await evaluate("__audioContexts.length"), 1);
      assert.equal(await evaluate("document.querySelector('[data-status]').textContent"), "Stopped");
      assert.doesNotMatch(await status(), /click|tap|press/i);
    });
    await t.test("touch and keyboard Stop cancel playback without unlocking audio", async () => {
      for (const gesture of ["touch", "Enter", " ", "shortcut"]) {
        await initialize();
        const resumes = await evaluate("__audioContexts[0].resumes");
        if (gesture === "touch") await pointer("#stop", true, "touch");
        else {
          await evaluate("document.querySelector('[data-run-view]').contentDocument.querySelector('#stop').focus()");
          if (gesture === "shortcut") {
            await call("input.performActions", { context, actions: [{ type: "key", id: "keyboard", actions: [
              { type: "keyDown", value: "\uE009" }, { type: "keyDown", value: "." },
              { type: "keyUp", value: "." }, { type: "keyUp", value: "\uE009" },
            ] }] });
          } else await keys([gesture === "Enter" ? "\uE007" : gesture]);
          await settle();
        }
        assert.equal(await evaluate("__audioContexts[0].state"), "closed", gesture);
        assert.equal(await evaluate("__audioContexts[0].resumes"), resumes, gesture);
        assert.equal(await evaluate("__audioContexts[0].states.includes('running')"), false, gesture);
        assert.equal(await evaluate("document.querySelector('[data-status]').textContent"), "Stopped", gesture);
      }
    });
}

for (const autoplayAllowed of [false, true]) {
  test(`playground audio activation (${autoplayAllowed ? "autoplay allowed" : "autoplay blocked"})`,
    { timeout: 30_000 }, t => testAudioActivation(t, autoplayAllowed));
}
