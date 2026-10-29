import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { extname, resolve, join } from "node:path";
import { tmpdir } from "node:os";
import test from "node:test";
import { openChrome, eventually } from "../chrome-test-session.js";

const root = resolve(new URL("../..", import.meta.url).pathname);
const demo = resolve(root, "examples/web/onda_wasm_playground");
const artifacts = resolve(root, "target/gpu-web-editor-validation");
const source = `struct Voice:
  id: i64 = 9223372036854775807
  gain: f64 = 0.5

buffers:
  clip: f32[]

params:
  freq = 440.0 {20.0, 20000.0}

init:
  phase = 0.0
  previous = freq
  print(freq)

block:
  if freq != previous:
    print(freq)
    previous = freq

sample:
  out1 = sin(phase) * 0.2
  out2 = cos(phase) * 0.2
  phase = (phase + freq * TWO_PI / SR) % TWO_PI

event configure(voice: Voice):
  print(voice.id, voice.gain)

event note_on(id: i32, channel: i32, key: i32, velocity: f32):
  print(key)

event note_off(id: i32, channel: i32, key: i32, velocity: f32):
  print(-key)
`;
const project = {
  entry: "main.onda",
  active: "main.onda",
  sources: { "main.onda": source },
};
const initScript = `
  if(window===top) {
    if(!localStorage.getItem("onda.browser-ide.project.v1"))
      localStorage.setItem("onda.browser-ide.project.v1",${JSON.stringify(JSON.stringify(project))});
    window.testAudioContexts=[];window.testResumeCalls=0;window.testCommands=[];window.testErrors=[];window.testIncoming=[];window.testAdapters=[];
    const NativeContext=AudioContext;
    window.AudioContext=class extends NativeContext {
      constructor(options){super(options);testAudioContexts.push(this);}
      resume(){testResumeCalls++;return super.resume();}
    };
    window.addEventListener("message",event=>{
      if(event.origin===location.origin && event.data?.__ondaRunUi)
        testCommands.push(event.data.message);
    });
  }
  if(navigator.gpu) {
    const request=navigator.gpu.requestAdapter.bind(navigator.gpu);
    navigator.gpu.requestAdapter=async options=>{
      const adapter=await request(options);
      top.testAdapters?.push({url:location.href,options,info:adapter?{vendor:adapter.info.vendor,fallback:adapter.info.isFallbackAdapter}:null});
      return adapter;
    };
  }
  window.addEventListener("message",event=>{if(event.data?.__ondaRunHost)top.testIncoming?.push(event.data.message);});
  window.addEventListener("error",event=>top.testErrors?.push(event.message));
  window.addEventListener("unhandledrejection",event=>top.testErrors?.push(String(event.reason)));
  const logError=console.error;
  console.error=(...args)=>{top.testErrors?.push(args.map(String).join(" "));logError(...args);};
  if(window!==top) {
    window.testInputs=[];
    document.addEventListener("pointerdown",event=>testInputs.push({x:event.offsetX,y:event.offsetY,target:event.target.id}),true);
  }
`;

async function serve(t, isolated = true, beforeRead) {
  const manifest = JSON.parse(
    await readFile(resolve(root, "target/website-play/manifest.json"), "utf8"),
  );
  const assetRoot = "/" + manifest.assetRoot;
  const page = (
    await readFile(resolve(root, "website/playground/index.html"), "utf8")
  )
    .replace(/^---[\s\S]*?---\s*/, "")
    .replace(
      /{{\s*site.data.onda.asset_path\s*\|\s*append:\s*'([^']+)'\s*\|\s*relative_url(\s*\|\s*jsonify)?\s*}}/g,
      (_, path, json) =>
        json ? JSON.stringify(assetRoot + path) : assetRoot + path,
    )
    .replace(
      /{{\s*'\/__result'\s*\|\s*relative_url\s*\|\s*jsonify\s*}}/g,
      '"/__result"',
    );
  assert.ok(!page.includes("{{"), "Website asset URLs must all resolve");
  const website = `<!doctype html><html data-theme="dark"><head><meta charset="utf-8">
    <link rel="stylesheet" href="/assets/site/styles.css">
    <link rel="stylesheet" href="${assetRoot}/playground.css"></head>
    <body class="play-page">${page}</body></html>`;
  const types = {
    ".html": "text/html",
    ".js": "text/javascript",
    ".css": "text/css",
    ".wasm": "application/wasm",
    ".json": "application/json",
    ".svg": "image/svg+xml",
  };
  const server = createServer(async (req, res) => {
    try {
      if (isolated) {
        res.setHeader("Cross-Origin-Opener-Policy", "same-origin");
        res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
      }
      const path = new URL(req.url, "http://localhost").pathname;
      if (path === "/embedded/") {
        const host = new URL(req.url, "http://localhost").searchParams.get("host");
        res.setHeader("Content-Type", "text/html");
        res.end(`<!doctype html><style>body{margin:0}iframe{width:100%;height:100vh;border:0;display:block}</style>
          <iframe sandbox="allow-scripts allow-same-origin" src="${host === "website" ? "/website/" : "/"}?backend=webgl"></iframe>`);
        return;
      }
      if (path === "/gpu-ready") {
        res.setHeader("Content-Type", "text/html");
        res.end("<!doctype html><title>GPU readiness</title>");
        return;
      }
      if (path === "/website/") {
        res.setHeader("Content-Type", "text/html");
        res.end(website);
        return;
      }
      const base = path.startsWith("/assets/play/")
        ? resolve(root, "target/website-play")
        : path.startsWith("/assets/site/")
          ? resolve(root, "website")
          : path.startsWith("/assets/")
            ? root
            : demo;
      const file = resolve(base, "." + (path === "/" ? "/index.html" : path));
      if (!file.startsWith(base + "/")) {
        res.writeHead(403).end();
        return;
      }
      await beforeRead?.(path);
      res.setHeader(
        "Content-Type",
        types[extname(file)] || "application/octet-stream",
      );
      res.end(await readFile(file));
    } catch (error) {
      res.writeHead(404).end(String(error));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(
    () =>
      new Promise((resolve) => {
        server.close(resolve);
        server.closeAllConnections();
      }),
  );
  return `http://127.0.0.1:${server.address().port}`;
}
function flags(autoplay, backend) {
  return [
    ...JSON.parse(
      process.env.ONDA_UI_CHROME_ARGS ||
        (backend === "webgl"
          ? '["--use-angle=swiftshader"]'
          : process.platform === "linux"
            ? '["--use-angle=vulkan","--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan","--disable-vulkan-surface"]'
            : "[]"),
    ),
    `--autoplay-policy=${autoplay ? "no-user-gesture-required" : "document-user-activation-required"}`,
  ];
}
async function openEditor(t, url, autoplay = true) {
  const chrome = await openChrome(
    t,
    flags(autoplay, new URL(url).searchParams.get("backend")),
  );
  const page = await chrome.page({
    width: 1600,
    height: 1100,
    source: initScript,
  });
  if (new URL(url).searchParams.get("backend") === "webgpu") {
    await page.navigate(new URL("/gpu-ready", url).href);
    await eventually(
      () =>
        page.evaluate(
          "(async()=>{const a=await navigator.gpu?.requestAdapter();return Boolean(a)&&a.info.isFallbackAdapter!==true;})()",
        ),
      "hardware GPU adapter",
    );
  }
  await page.navigate(url);
  chrome.beforeClose(async () => {
    await mkdir(artifacts, { recursive: true });
    await writeFile(
      resolve(
        artifacts,
        `${t.name.replace(/[^a-z0-9]+/gi, "-")}-messages.json`,
      ),
      JSON.stringify(
        await page.evaluate(
          `(()=>{const f=document.querySelector("[data-run-view]"),c=f?.contentDocument?.querySelector("canvas");return {errors:testErrors,commands:testCommands.slice(-25),incoming:testIncoming.filter(m=>m.type==="state").slice(-3),frame:{opacity:f?.style.opacity,width:f?.getBoundingClientRect().width,inputs:f?.contentWindow?.testInputs,canvas:c && {width:c.width,height:c.height,clientWidth:c.clientWidth,clientHeight:c.clientHeight}}}})()`,
        ),
        null,
        2,
      ),
    );
    const { data } = await page.call("Page.captureScreenshot", {
      format: "png",
    });
    await writeFile(
      resolve(artifacts, `${t.name.replace(/[^a-z0-9]+/gi, "-")}.png`),
      Buffer.from(data, "base64"),
    );
  });
  await eventually(
    async () => {
      const result = await page.evaluate(`(()=>{
      const frame=document.querySelector("[data-run-view]");
      const error=frame?.contentDocument?.querySelector("#status");
      if(error && !error.hidden && error.textContent.startsWith("Could not render"))throw new Error(error.textContent);
      const status=document.querySelector("[data-status]");
      if(status?.textContent==="Error")throw new Error("Editor startup failed: "+JSON.stringify({errors:testErrors,host:testIncoming.slice(-2)}));
      return frame?.contentDocument?.readyState==="complete" && frame?.style.opacity!=="0" && error?.hidden && testAudioContexts.length>0
        && status?.textContent!=="Compiling...";
    })()`);
      return result;
    },
    "compiled GPU editor and AudioWorklet",
    60000,
  );
  async function frame(expression) {
    const tree = await page.call("Page.getFrameTree");
    const id = tree.frameTree.childFrames[0].frame.id;
    const context = [...page.contexts.values()].find(
      (c) => c.auxData?.isDefault && c.auxData.frameId === id,
    );
    assert.ok(context, "GPU execution context exists");
    return page.evaluate(expression, context.id);
  }
  await frame(`(async()=>{
    const {CanvasUi}=await import(new URL("./pkg/onda_ui.js",document.querySelector("script[src]").src));
    window.probe={state:{},packets:0,nonzero:false,bytes:0};
    const render=CanvasUi.prototype.render;
    CanvasUi.prototype.render=function(){probe.ui=this;probe.renders=(probe.renders||0)+1;return render.call(this);};
    const message=CanvasUi.prototype.message;
    CanvasUi.prototype.message=function(json){
      const m=JSON.parse(json);if(m.type==="state")Object.assign(probe.state,m.state);
      return message.call(this,json);
    };
    const scope=CanvasUi.prototype.scope_packet;
    CanvasUi.prototype.scope_packet=function(bytes){
      probe.packets++;probe.bytes=bytes.length;probe.packet=Array.from(bytes);
      for(let i=8;i<bytes.length;i++)if(bytes[i]){probe.nonzero=true;break;}
      return scope.call(this,bytes);
    };
    parent.postMessage({__ondaRunUi:true,message:{type:"uiReady"}},location.origin);
  })()`);
  await eventually(
    () => frame("probe.state.connected===true"),
    "live host state",
  );
  t.diagnostic("Compiler, GPU canvas, and AudioWorklet are ready");
  await frame('document.fonts.load("600 14px Inter")');
  // Cached navigation must retain the parent compiler/bridge as well as the canvas.
  await page.evaluate("window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}))");
  await frame("window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}))");
  await page.evaluate("window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}))");
  await frame("window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}))");
  let selectedSection="params";
  const geometry = () => page.evaluate(`(()=>{
    const f=document.querySelector("[data-run-view]"),r=f.getBoundingClientRect();
    const state=Object.assign({},...testIncoming.filter(m=>m.type==="state").map(m=>m.state));
    const view=testCommands.findLast(c=>c.type==="viewState")?.state||state.viewState||{};
    const sections={},width=r.width-46;
    let y=96-(view.scrollTop||0);
    // section() folds each preceding section before using these header positions.
    for(const name of ["scope","log","buffers","events","inputControls","params"]){
      const present=name==="scope"?state.supportsScope!==false:name==="log"?Boolean(state.logRevealed||state.logText):Boolean(state[name]?.length);
      if(!present)continue;
      const toolbar=40;
      sections[name]={y,open:view.sections?.[name]!==false,toolbar};y+=toolbar+12;
    }
    const actions=Number(state.supportsDeviceSelection===true||state.supportsMidiInputSelection===true||state.supportsRunSettings===true)+Number(state.supportsProjectExport===true&&state.canExportProject===true)+Number(state.supportsSourceSelection===true);
    const knobs=(view.paramLayout??state.paramLayout??"knobs")==="knobs";
    const minimum=knobs?(width<=358?80:140):200;
    const cols=Math.min(knobs&&width<=358?2:8,Math.max(1,Math.floor((width+8)/(minimum+8)))),card=(width-(cols-1)*8)/cols;
    const selected=sections[${JSON.stringify(selectedSection)}];
    return {x:r.x,y:r.y,width:r.width,height:r.height,sections,scopeTop:sections.scope?.y,
      top:(selected?.y||0)+20,paramX:21+(knobs?card/2:card-61),paramY:(sections.params?.y||0)+(sections.params?.toolbar||40)+(knobs?114:21),
      settingsX:r.width-4-actions*40,settingsY:48,
      stopX:r.width-81-actions*40,stopY:48,midiKeyY:r.height-20};
  })()`);
  const click = async (x, y) => {
    const g = await geometry();
    await page.call("Input.dispatchMouseEvent", {
      type: "mousePressed",
      x: g.x + x,
      y: g.y + y,
      button: "left",
      clickCount: 1,
    });
    await page.call("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x: g.x + x,
      y: g.y + y,
      button: "left",
      clickCount: 1,
    });
  };
  const section = async (name) => {
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
    let g=await geometry();
    await page.call("Input.dispatchMouseEvent",{type:"mouseWheel",x:g.x+g.width-4,y:g.y+20,deltaX:0,deltaY:-100000});
    await frame("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    if(name==="settings") {g=await geometry();await click(g.settingsX,g.settingsY);return;}
    if(name==="midi") return;
    for(const key of ["scope","log","buffers","events","inputControls","params"]){
      g=await geometry();const entry=g.sections[key];
      if(entry && entry.open!==(key===name)) {
        await click(45,entry.y+20);
        await frame("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
      }
      if(key===name) break;
    }
    selectedSection=name;
  };
  const key = (key, code, modifiers = 0, type = "keyDown") =>
    page.call("Input.dispatchKeyEvent", {
      type,
      key,
      code,
      modifiers,
      ...(key.length === 1 && !modifiers && type === "keyDown"
        ? { text: key }
        : {}),
    });
  return { page, frame, geometry, click, section, key };
}

for (const host of ["example", "website"]) {
  test(`GPU ${host} initialization errors stay visible and iframe reload recovers`, { timeout:60000 }, async t => {
    let unavailable = true;
    const base = await serve(t, true, path => {
      if (unavailable && path.endsWith("/gpu/pkg/onda_ui_bg.wasm"))
        throw new Error("Injected GPU asset failure");
    });
    const chrome = await openChrome(t, flags(true, "webgl"));
    const page = await chrome.page({ width:1600, height:1100, source:initScript });
    await page.navigate(`${base}/${host === "website" ? "website/" : ""}?backend=webgl`);
    await eventually(() => page.evaluate(`(() => {
      const frame = document.querySelector('[data-run-view]');
      return document.querySelector('[data-run-loading]')?.hidden
        && getComputedStyle(frame).opacity === '1'
        && frame.contentDocument?.querySelector('#status')?.textContent.includes('Could not render');
    })()`), "visible renderer initialization failure");
    await eventually(() => page.evaluate("document.querySelector('[data-status]').textContent === 'Playing'"), "audio initialization after renderer failure");
    assert.equal(await page.evaluate("document.querySelector('[data-run-loading]').hidden"), true);
    assert.equal(await page.evaluate("getComputedStyle(document.querySelector('[data-run-view]')).opacity"), "1");
    unavailable = false;
    await page.evaluate("document.querySelector('[data-run-view]').contentWindow.location.reload()");
    await eventually(() => page.evaluate(`(() => {
      const frame = document.querySelector('[data-run-view]');
      return document.querySelector('[data-run-loading]').hidden
        && getComputedStyle(frame).opacity === '1'
        && frame.contentDocument?.querySelector('canvas')?.dataset.backend === 'webgl'
        && frame.contentDocument?.querySelector('#status')?.hidden;
    })()`), "renderer recovery after iframe reload");
  });

  test(`GPU ${host} editor selections fill the line bounds`, { timeout: 60000 }, async t => {
    const base = await serve(t);
    const chrome = await openChrome(t, flags(true, "webgl"));
    const page = await chrome.page({ width:1600, height:1100, source:initScript });
    await page.navigate(`${base}/${host === "website" ? "website/" : ""}?backend=webgl`);
    await eventually(() => page.evaluate("Boolean(document.querySelector('.cm-line'))"), "editor ready");
    for (const [theme, fontSize] of [["dark", 16], ["light", 24]]) {
      await page.evaluate(`(() => {
        document.documentElement.dataset.theme = ${JSON.stringify(theme)};
        document.querySelector("[data-editor]").style.setProperty("--onda-editor-font-size", "${fontSize}px");
      })()`);
      assert.equal(await page.evaluate("getComputedStyle(document.querySelector('.cm-content')).fontSize"), `${fontSize}px`);
      await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
      const lines = await page.evaluate(`(() => {
        const lines = [...document.querySelectorAll(".cm-line")];
        return {first:lines[1].getBoundingClientRect().toJSON(),last:lines[17].getBoundingClientRect().toJSON()};
      })()`);
      const start = {x:lines.first.x+40,y:lines.first.y+lines.first.height/2};
      const end = {x:lines.last.x+100,y:lines.last.y+lines.last.height/2};
      await page.call("Input.dispatchMouseEvent",{type:"mousePressed",...start,button:"left",buttons:1,clickCount:1});
      await page.call("Input.dispatchMouseEvent",{type:"mouseMoved",...end,button:"left",buttons:1});
      await eventually(() => page.evaluate("document.getSelection()?.toString().includes('params:')"), "drag selection updated");
      await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
      const measure = () => page.evaluate(`(() => {
        const rectangles = [...document.querySelectorAll(".cm-selectionBackground")]
          .filter(e=>getComputedStyle(e).visibility !== "hidden").map(e=>e.getBoundingClientRect().toJSON());
        const active = [...document.querySelectorAll(".cm-activeLine,.cm-activeLineGutter")]
          .map(e=>getComputedStyle(e).backgroundColor);
        return {rectangles,active};
      })()`);
      const selection = await measure();
      assert.ok(selection.active.every(color=>color==="rgba(0, 0, 0, 0)"), "drag selection hides the active row and gutter highlight");
      const first = selection.rectangles.find(r=>r.y < lines.first.bottom && r.bottom > lines.first.y);
      assert.ok(first && Math.abs(first.top-lines.first.top)<.5 && Math.abs(first.bottom-lines.first.bottom)<.5,
        `${theme} ${fontSize}px: the selected end row fills the line height`);
      const middle = selection.rectangles.find(r=>r.height>lines.first.height*2);
      assert.ok(middle && Math.abs(middle.left-lines.first.left)<.5 && Math.abs(middle.right-lines.first.right)<.5,
        `${theme} ${fontSize}px: complete selected rows fill the line width`);
      const last = selection.rectangles.find(r=>r.top < lines.last.bottom && r.bottom > lines.last.top);
      assert.ok(last && Math.abs(last.top-lines.last.top)<.5 && Math.abs(last.bottom-lines.last.bottom)<.5,
        `${theme} ${fontSize}px: the other selected end row fills the line height`);
      await page.call("Input.dispatchMouseEvent",{type:"mouseReleased",...end,button:"left",buttons:0,clickCount:1});
      assert.ok((await measure()).active.every(color=>color==="rgba(0, 0, 0, 0)"), "releasing the drag keeps selection free of the active row highlight");
      if (fontSize===24) {
        await page.evaluate("document.querySelector('.cm-scroller').scrollTo(80,65)");
        await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
        const scrolled = await page.evaluate(`(() => {
          const scroll = document.querySelector(".cm-scroller");
          return {left:scroll.scrollLeft,top:scroll.scrollTop,
            line:document.querySelectorAll(".cm-line")[1].getBoundingClientRect().toJSON()};
        })()`);
        assert.ok(scrolled.left>0 && scrolled.top>0, "the larger font exercises horizontal and vertical scrolling");
        const middle = (await measure()).rectangles.find(r=>r.height>lines.first.height*2);
        assert.ok(middle && Math.abs(middle.left-scrolled.line.left)<.5 && Math.abs(middle.right-scrolled.line.right)<.5,
          "selection edges follow the content while scrolling");
        await page.evaluate("document.querySelector('.cm-scroller').scrollTo(0,0)");
        await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
      }
      await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"ArrowRight",code:"ArrowRight"});
      await eventually(async () => {
        const collapsed = await measure();
        return collapsed.rectangles.length===0 && collapsed.active.length===2
          && collapsed.active.every(color=>color!=="rgba(0, 0, 0, 0)");
      }, "collapsing selection restores the active row and gutter highlight");
      const singleStart = {...end,x:lines.last.x+120};
      const singleEnd = {...end,x:lines.last.x+40};
      await page.call("Input.dispatchMouseEvent",{type:"mousePressed",...singleStart,button:"left",buttons:1,clickCount:1});
      await page.call("Input.dispatchMouseEvent",{type:"mouseMoved",...singleEnd,button:"left",buttons:1});
      await eventually(() => page.evaluate("Boolean(document.getSelection()?.toString()) && !document.getSelection().toString().includes('\\n')"), "single row selected in reverse");
      await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
      const single = await measure();
      assert.equal(single.rectangles.length,1);
      assert.ok(Math.abs(single.rectangles[0].top-lines.last.top)<.5
        && Math.abs(single.rectangles[0].bottom-lines.last.bottom)<.5
        && single.rectangles[0].width<lines.last.width/2,
      "a partial single-row selection fills the row height and retains its text width");
      await page.call("Input.dispatchMouseEvent",{type:"mouseReleased",...singleEnd,button:"left",buttons:0,clickCount:1});
      await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"ArrowRight",code:"ArrowRight"});
    }
  });

  test(`GPU ${host} double-click restores the smoothing default after reload`, { timeout: 60000 }, async t => {
    const base = await serve(t);
    const { page, frame, geometry, section } = await openEditor(
      t, `${base}/${host === "website" ? "website/" : ""}?backend=webgl`,
    );
    await page.evaluate("localStorage.setItem('onda.browser-ide.param-smoothing-ms.v1','55')");
    await page.call("Page.reload");
    await eventually(() => page.evaluate(`
      globalThis.testIncoming?.some(m=>m.type==='state' && m.state.paramSmoothingMs===55)
        && document.querySelector('[data-run-view]')?.style.opacity!=='0'
    `), "saved smoothing restored");
    await section("params");
    const g = await geometry();
    for (const clickCount of [1, 2]) {
      for (const type of ["mousePressed", "mouseReleased"])
        await page.call("Input.dispatchMouseEvent", {
          type, x:g.x+g.width-185,
          y:g.y+g.sections.params.y+(g.sections.params.toolbar>40?47:20),
          button:"left", clickCount,
        });
      await frame("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    }
    const milliseconds = await page.evaluate("testCommands.findLast(c=>c.type==='setParamSmoothing')?.milliseconds");
    assert.equal(milliseconds, 30, "double-click resets to the default, not the saved edit");
    assert.equal(await page.evaluate("localStorage.getItem('onda.browser-ide.param-smoothing-ms.v1')"), "30");
  });

  test(`GPU ${host} creates project files without browser dialogs`, { timeout:60000 }, async t => {
    const base = await serve(t);
    const chrome = await openChrome(t, flags(false, "webgl"));
    const page = await chrome.page({width:1600,height:1100});
    await page.navigate(`${base}/embedded/?host=${host}`);
    const document = "document.querySelector('iframe')?.contentDocument";
    await eventually(() => page.evaluate(`${document}?.querySelector('.cm-editor')!==null && ${document}?.querySelector('.cm-editor')!==undefined`), "embedded editor");
    const clickAdd = async () => {
      const r=await page.evaluate(`(()=>{const r=${document}.querySelector('[data-new-file]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
      for (const type of ["mousePressed","mouseReleased"])
        await page.call("Input.dispatchMouseEvent",{type,...r,button:"left",clickCount:1});
    };
    await clickAdd();
    const input = `${document}.querySelector('input[aria-label="New file name"]')`;
    await eventually(() => page.evaluate(`${input}===${document}.activeElement && ${input}!==null`), "inline new-file name field", 3000);
    const field = await page.evaluate(`(()=>{
      const input=${input}, entry=input.parentElement;
      const tab=${document}.querySelector('.project-file:not(.project-file-create)');
      const rect=e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}};
      return {entry:rect(entry),input:rect(input),tab:rect(tab),
        font:getComputedStyle(input).fontSize,tabFont:getComputedStyle(tab).fontSize};
    })()`);
    assert.equal(field.entry.width,field.tab.width,"new-file entry matches the tab width");
    assert.equal(field.entry.height,field.tab.height,"new-file entry matches the tab height");
    assert.ok(field.input.y>=field.entry.y && field.input.y+field.input.height<=field.entry.y+field.entry.height,
      "native input stays within the tab");
    assert.equal(field.font,field.tabFont,"filename editing uses the tab font");
    await mkdir(artifacts,{recursive:true});
    const {data}=await page.call("Page.captureScreenshot",{clip:{x:field.tab.x,y:field.tab.y,
      width:field.entry.x+field.entry.width+36-field.tab.x,height:field.tab.height+1,scale:2}});
    await writeFile(resolve(artifacts,`new-file-${host}.png`),Buffer.from(data,"base64"));
    await page.call("Input.insertText",{text:"voices/lead.onda"});
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter"});
    const names = `Array.from(${document}.querySelectorAll('.project-file-name'),e=>e.textContent)`;
    await eventually(() => page.evaluate(`${names}.includes('voices/lead.onda')`), "created and selected source file", 3000);
    assert.equal(await page.evaluate(`${document}.activeElement.classList.contains('cm-content')`),true);
    await clickAdd();
    await page.call("Input.insertText",{text:"main.onda"});
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter"});
    assert.equal(await page.evaluate(`${input}.getAttribute('aria-invalid')`),"true");
    assert.match(await page.evaluate(`${document}.querySelector('[role="alert"]').textContent`),/already exists/);
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"a",code:"KeyA",windowsVirtualKeyCode:65,modifiers:2});
    await page.call("Input.insertText",{text:"module.onda"});
    assert.equal(await page.evaluate(`${input}.getAttribute('aria-invalid')`),null);
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter"});
    await clickAdd();
    assert.equal(await page.evaluate(`${input}.value`),"module2.onda");
    await page.call("Input.insertText",{text:"cancelled.onda"});
    await clickAdd();
    assert.equal(await page.evaluate(`${input}.value`),"cancelled.onda","clicking + again keeps the pending name");
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape",code:"Escape"});
    assert.equal(await page.evaluate(input),null);
    assert.deepEqual(await page.evaluate(names),["main.onda","voices/lead.onda","module.onda"]);
    await eventually(() => page.evaluate("(()=>{const p=JSON.parse(localStorage.getItem('onda.browser-ide.project.v1'));return p?.sources?.['voices/lead.onda']!==undefined && p.sources['module.onda']!==undefined})()"), "new files persisted");
    await page.navigate(`${base}/embedded/?host=${host}`);
    await eventually(() => page.evaluate(`${document}?.querySelectorAll('.project-file-name').length===3`), "created files restored after reload");
    assert.deepEqual(await page.evaluate(names),["main.onda","voices/lead.onda","module.onda"]);
  });

  test(`GPU ${host} waits for its project before revealing the run view`, { timeout: 60000 }, async t => {
    const compiler = Promise.withResolvers();
    t.after(() => compiler.resolve());
    const base = await serve(t, true, path => path.endsWith("onda_compiler_web_bg.wasm") ? compiler.promise : undefined);
    const chrome = await openChrome(t, flags(true, "webgl"));
    const page = await chrome.page({ width:1600, height:1100, source:initScript });
    await page.call("Page.navigate", { url:`${base}/${host === "website" ? "website/" : ""}?backend=webgl` });
    await eventually(() => page.evaluate("testCommands.some(c=>c.type==='uiReady')"), "renderer ready while compiler is loading");
    assert.equal(await page.evaluate("document.querySelector('[data-run-view]').style.opacity"), "0");
    assert.deepEqual(await page.evaluate(`(() => {
      const loading = document.querySelector("[data-run-loading]");
      return { text: loading.textContent, display: getComputedStyle(loading).display,
        busy: loading.parentElement.getAttribute("aria-busy") };
    })()`), { text: "Loading compiler...", display: "grid", busy: "true" });
    assert.equal(await page.evaluate("testIncoming.some(m=>m.type==='state')"), false,
      "the playground must not publish an unloaded source picker");
    compiler.resolve();
    await eventually(() => page.evaluate("document.querySelector('[data-run-view]').style.opacity!=='0' && testAudioContexts.length>0"), "compiled project revealed");
    assert.equal(await page.evaluate(`(() => {
      const loading = document.querySelector("[data-run-loading]");
      return loading.hidden && getComputedStyle(loading).display === "none"
        && loading.parentElement.getAttribute("aria-busy") === "false";
    })()`), true);
    assert.equal(await page.evaluate("testIncoming.filter(m=>m.type==='state').every(m=>m.state.path===undefined || m.state.path==='main.onda')"), true);
    assert.equal(await page.evaluate("testErrors.length"), 0);
  });

  test(`GPU ${host} keeps loading feedback until the controls render`, { timeout: 60000 }, async t => {
    const renderer = Promise.withResolvers();
    t.after(() => renderer.resolve());
    const base = await serve(t, true, path => path.endsWith("onda_ui_bg.wasm") ? renderer.promise : undefined);
    const chrome = await openChrome(t, flags(true, "webgl"));
    const page = await chrome.page({ width:1600, height:1100, source:initScript });
    await page.navigate(`${base}/${host === "website" ? "website/" : ""}?backend=webgl`);
    await eventually(() => page.evaluate("document.querySelector('[data-status]')?.textContent==='Playing' && testAudioContexts.length>0"), "compiler and audio ready before renderer");
    assert.equal(await page.evaluate(`(() => {
      const loading = document.querySelector("[data-run-loading]");
      return !loading.hidden && getComputedStyle(loading).display === "grid"
        && loading.textContent === "Preparing controls..."
        && loading.parentElement.getAttribute("aria-busy") === "true"
        && document.querySelector("[data-run-view]").style.opacity === "0";
    })()`), true);
    renderer.resolve();
    await eventually(() => page.evaluate(`
      document.querySelector("[data-run-loading]").hidden
        && document.querySelector("[data-run-view]").style.opacity !== "0"
    `), "rendered controls replace loading feedback");
    assert.equal(await page.evaluate("testErrors.length"), 0);
  });

  test(`GPU ${host} displays compiler error locations`, { timeout: 60000 }, async t => {
    const base = await serve(t);
    const chrome = await openChrome(t, flags(true, "webgl"));
    const invalidProject = {
      entry: "voices/lead.onda",
      active: "voices/lead.onda",
      sources: { "voices/lead.onda": "sample:\n  out1 = missing\n" },
    };
    const page = await chrome.page({
      width: 1600, height: 1100,
      source: initScript + `
        localStorage.setItem("onda.browser-ide.project.v1", ${JSON.stringify(JSON.stringify(invalidProject))});
      `,
    });
    await page.navigate(`${base}/${host === "website" ? "website/" : ""}?backend=webgl`);
    await eventually(() => page.evaluate(`
      testIncoming.some(m => m.type === "state" && m.state.error)
        && document.querySelector("[data-run-view]").style.opacity !== "0"
    `), "compiler error revealed in the shared UI");
    assert.equal(await page.evaluate("document.querySelector('[data-run-loading]').hidden"), true);
    const error = await page.evaluate(`
      testIncoming.findLast(m => m.type === "state" && m.state.error).state.error
    `);
    assert.match(error, /missing/);
    assert.match(error, /voices\/lead\.onda:2:10\)$/);
  });
}

for (const [host, backend] of [
  ["example", "webgpu"],
  ["example", "webgl"],
  ["website", "webgpu"],
  ["website", "webgl"],
]) {
  test(`GPU web editor ${host} ${backend}`, {
    timeout: 120000,
    skip: backend === "webgpu" && process.env.ONDA_UI_TEST_WEBGPU === "0"
      ? "requires a hardware WebGPU adapter" : false,
  }, async (t) => {
    const base = await serve(t);
    const { page, frame, geometry, click, section, key } = await openEditor(
      t,
      `${base}/${host === "website" ? "website/" : ""}?backend=${backend}`,
    );
    assert.equal(
      await frame("document.querySelector('canvas').dataset.backend"),
      backend,
      JSON.stringify(await page.evaluate("testAdapters")),
    );
    assert.equal(await page.evaluate("crossOriginIsolated"), true);
    await eventually(
      () => frame("probe.packets>2 && probe.nonzero"),
      "real nonzero audio scope",
    );
    assert.ok(
      await frame("probe.bytes<=8+2*512*4"),
      "scope transport is bounded",
    );
    const scopePacketBytes = await frame("probe.bytes");
    await section("params");
    let g = await geometry();
    await click(g.paramX,g.paramY);
    // A pending editor scroll must not reclaim focus from a GPU numeric field.
    await page.evaluate(
      "document.querySelector('.cm-scroller').dispatchEvent(new Event('scroll'))",
    );
    await page.evaluate("window.dispatchEvent(new Event('blur'))");
    for (const char of "880") await key(char);
    await key("Enter", "Enter");
    await eventually(
      () => frame("probe.state.logText?.includes('880')"),
      "parameter reaching the real audio processor",
    );
    assert.equal(
      await frame(`probe.state.logEntries?.some(entry =>
        entry?.source?.file?.endsWith('/main.onda') && Number.isInteger(entry.source.line)
        && typeof entry.lexicalOwner === 'string')`),
      true,
      "real processor prints must retain file, line, and lexical-owner context",
    );
    await click(g.width-55,g.sections.params.y+(g.sections.params.toolbar>40?47:20));
    await eventually(()=>page.evaluate("JSON.parse(localStorage.getItem('onda.browser-ide.run-view.v1'))?.paramLayout==='knobs'"),"knob layout saved with presentation");
    const initialScreenshot = await page.call("Page.captureScreenshot", {
      format: "png",
    });
    await mkdir(artifacts, { recursive: true });
    await writeFile(
      resolve(artifacts, `${host}-${backend}-playing.png`),
      Buffer.from(initialScreenshot.data, "base64"),
    );
    // Read pixels in the render turn: WebGL drawing buffers need not persist.
    // The Linux headless Vulkan compositor can omit iframe layers in page captures.
    const pixels = await frame(`(async()=>{
      const c=document.querySelector("canvas");
      probe.ui.resize(c.width,c.height,devicePixelRatio);probe.ui.render();
      const image=c.toDataURL();
      const bitmap=await createImageBitmap(await (await fetch(image)).blob());
      const copy=document.createElement("canvas");copy.width=c.width;copy.height=c.height;
      const context=copy.getContext("2d");context.drawImage(bitmap,0,0);bitmap.close();
      const data=context.getImageData(0,0,c.width,c.height).data;
      let foregroundPixels=0;
      for(let i=0;i<data.length;i+=4)if(data[i]>140 && data[i+1]>150 && data[i+2]>170)foregroundPixels++;
      return {image,foregroundPixels};
    })()`);
    assert.ok(
      pixels.foregroundPixels > 300,
      "GPU controls and scope must draw visible pixels",
    );
    await writeFile(
      resolve(artifacts, `${host}-${backend}-canvas.png`),
      Buffer.from(pixels.image.split(",")[1], "base64"),
    );
    await section("events");
    g = await geometry();
    await click(g.width - 76, g.top + 72);
    const payload = { id: "-9223372036854775808", gain: "Infinity" };
    await frame(`(()=>{
      const clipboard=new DataTransfer();clipboard.setData("text/plain",${JSON.stringify(JSON.stringify(payload))});
      document.querySelector("canvas").dispatchEvent(new ClipboardEvent("paste",{clipboardData:clipboard,bubbles:true,cancelable:true}));
    })()`);
    await key("Enter", "Enter");
    await click(g.width - 74, g.top + 30);
    await eventually(
      () =>
        frame(
          "probe.state.logText?.includes('-9223372036854775808') && /inf/i.test(probe.state.logText)",
        ),
      "typed aggregate event and non-finite float reaching the processor",
    );
    assert.equal(
      await frame("probe.state.logEntries.length === probe.state.logText.split('\\n').length - 1"),
      true,
      "i64 print output must keep its line metadata aligned through JSON transport",
    );

    // Recompile edited source through CodeMirror, keeping the event presentation.
    const before = await page.evaluate("testAudioContexts.length");
    await page.evaluate("document.querySelector('.cm-content').focus()");
    await key("End", "End", 2);
    await page.call("Input.insertText", { text: "\n\n" });
    await key("s", "KeyS", 2);
    await eventually(
      () =>
        page.evaluate(
          `testAudioContexts.length>${before} && document.querySelector("[data-status]").textContent==="Playing"`,
        ),
      "recompile through the browser compiler",
    );
    const triggered = await page.evaluate(
      "testCommands.filter(c=>c.type==='triggerEvent').length",
    );
    await click(g.width - 74, g.top + 30);
    await eventually(
      () =>
        page.evaluate(
          `testCommands.filter(c=>c.type==="triggerEvent").length>${triggered} && testCommands.findLast(c=>c.type==="triggerEvent")?.values[0].id==="-9223372036854775808"`,
        ),
      "event values retained after recompilation",
    );

    await section("buffers");
    const temporary = await mkdtemp(join(tmpdir(), "onda-web-editor-buffer-"));
    t.after(() => rm(temporary, { recursive: true, force: true }));
    const wav = Buffer.alloc(44 + 32 * 4);
    wav.write("RIFF");
    wav.writeUInt32LE(wav.length - 8, 4);
    wav.write("WAVEfmt ", 8);
    wav.writeUInt32LE(16, 16);
    wav.writeUInt16LE(3, 20);
    wav.writeUInt16LE(1, 22);
    wav.writeUInt32LE(48000, 24);
    wav.writeUInt32LE(192000, 28);
    wav.writeUInt16LE(4, 32);
    wav.writeUInt16LE(32, 34);
    wav.write("data", 36);
    wav.writeUInt32LE(128, 40);
    for (let i = 0; i < 32; i++)
      wav.writeFloatLE(Math.sin(i) * 0.5, 44 + i * 4);
    const file = join(temporary, "picked.wav");
    await writeFile(file, wav);
    await page.call("Page.setInterceptFileChooserDialog", { enabled: true });
    let contextCount = await page.evaluate("testAudioContexts.length");
    const picker = page.event("Page.fileChooserOpened");
    g = await geometry();
    await click(g.width - 149, g.top + 41);
    const choice = await picker;
    await page.call("DOM.setFileInputFiles", {
      backendNodeId: choice.backendNodeId,
      files: [file],
    });
    await eventually(
      () =>
        frame(
          "probe.state.buffers?.[0]?.loadedPath==='picked.wav' && probe.state.buffers[0].loadedFrames===32",
        ),
      "GPU buffer picker and canonical WAV binding",
    );
    await eventually(
      () =>
        page.evaluate(
          `testAudioContexts.length>${contextCount} && testAudioContexts.at(-1).state==="running" && document.querySelector("[data-status]").textContent==="Playing"`,
        ),
      "processor restart after buffer binding",
    );
    contextCount = await page.evaluate("testAudioContexts.length");
    const bindingsBeforeRejectedDrops = await page.evaluate("testCommands.filter(c=>c.type==='bindBufferFile').length");
    for (const names of [["first.wav", "second.wav"], ["image.png"]]) {
      await frame(`(()=>{
        const transfer=new DataTransfer();
        for(const name of ${JSON.stringify(names)}) transfer.items.add(new File([new Uint8Array(1)],name));
        document.querySelector("canvas").dispatchEvent(new DragEvent("drop",{clientX:50,clientY:${g.top + 80},dataTransfer:transfer,bubbles:true,cancelable:true}));
      })()`);
    }
    await frame("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    assert.equal(await page.evaluate("testCommands.filter(c=>c.type==='bindBufferFile').length"), bindingsBeforeRejectedDrops,
      "multiple and unsupported files must leave the current binding intact");
    await frame(`(()=>{
      const transfer=new DataTransfer();transfer.items.add(new File([Uint8Array.from(${JSON.stringify([...wav])})],"dropped.wav",{type:"audio/wav"}));
      document.querySelector("canvas").dispatchEvent(new DragEvent("drop",{clientX:50,clientY:${g.top + 80},dataTransfer:transfer,bubbles:true,cancelable:true}));
    })()`);
    await eventually(
      () => frame("probe.state.buffers?.[0]?.loadedPath==='dropped.wav'"),
      "buffer drop replacement",
    );
    await eventually(
      () =>
        page.evaluate(
          `testAudioContexts.length>${contextCount} && testAudioContexts.at(-1).state==="running" && document.querySelector("[data-status]").textContent==="Playing"`,
        ),
      "processor restart after buffer drop",
    );
    contextCount = await page.evaluate("testAudioContexts.length");
    await click(g.width - 68, g.top + 41);
    await eventually(
      () => frame("probe.state.buffers?.[0]?.loadedPath===null"),
      "buffer clear",
    );
    await eventually(
      () =>
        page.evaluate(
          `testAudioContexts.length>${contextCount} && testAudioContexts.at(-1).state==="running" && document.querySelector("[data-status]").textContent==="Playing"`,
        ),
      "processor restart after buffer clear",
    );
    t.diagnostic(
      "Parameter/event dispatch, recompilation, and browser buffer operations passed",
    );

    await section("midi");
    await key("a", "KeyA");
    await key("a", "KeyA", 0, "keyUp");
    await eventually(
      () =>
        frame(
          "/(^|\\n)60(\\n|$)/.test(probe.state.logText) && /(^|\\n)-60(\\n|$)/.test(probe.state.logText)",
        ),
      "MIDI note on and off",
    );
    // Forward computer keys from the parent document, outside the source editor.
    await page.evaluate("document.body.tabIndex=-1;document.body.focus()");
    const notes = await page.evaluate(
      "testCommands.filter(c=>c.type==='midiNote').length",
    );
    await key("s", "KeyS");
    await key("s", "KeyS", 0, "keyUp");
    await eventually(
      () =>
        page.evaluate(
          `testCommands.filter(c=>c.type==="midiNote").length>=${notes + 2}`,
        ),
      "parent keyboard forwarding",
    );
    // Moving focus from the parent must not release a pointer-owned piano key.
    g = await geometry();
    await page.call("Input.dispatchMouseEvent", {
      type: "mousePressed",
      x: g.x + 35,
      y: g.y + g.midiKeyY,
      button: "left",
      clickCount: 1,
    });
    await page.evaluate("window.dispatchEvent(new Event('blur'))");
    await frame(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    assert.equal(
      await page.evaluate(
        "testCommands.findLast(c=>c.type==='midiNote')?.pressed",
      ),
      true,
    );
    await page.call("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x: g.x + 35,
      y: g.y + g.midiKeyY,
      button: "left",
      clickCount: 1,
    });
    await eventually(
      () =>
        page.evaluate(
          "testCommands.findLast(c=>c.type==='midiNote')?.pressed===false",
        ),
      "pointer MIDI release",
    );

    await section("log");
    g=await geometry();
    await click(g.width - 65, g.top - 10);
    await eventually(() => frame("probe.state.logText===''"), "clear log");
    await section("settings");
    assert.equal(await frame("probe.state.supportsMidiInputSelection"), true);

    await section("scope");
    g=await geometry();
    await click(80, g.scopeTop + 12);
    await eventually(
      () =>
        page.evaluate(
          "testCommands.findLast(c=>c.type==='scopeConfig')?.visible===false",
        ),
      "collapsed scope configuration",
    );
    await new Promise((resolve) => setTimeout(resolve, 100));
    const paused = await frame("probe.packets");
    await new Promise((resolve) => setTimeout(resolve, 150));
    assert.equal(
      await frame("probe.packets"),
      paused,
      "collapsed scope sends no data",
    );
    await click(80, g.scopeTop + 12);
    await eventually(
      () => frame(`probe.packets>${paused + 2}`),
      "scope resumes after expansion",
    );

    await page.evaluate("document.documentElement.dataset.theme='light'");
    await eventually(
      () => frame("document.documentElement.dataset.theme==='light'"),
      "theme forwarding",
    );
    // A full navigation restores the independent section folds.
    await page.call("Page.reload");
    await eventually(
      () =>
        page.evaluate(
          "document.querySelector('[data-run-view]')?.style.opacity!== '0' && document.querySelector('[data-status]')?.textContent==='Playing'",
        ),
      "playback after navigation",
      60000,
    );
    assert.equal(
      await page.evaluate(
        "JSON.parse(localStorage.getItem('onda.browser-ide.run-view.v1')).sections.log",
      ),
      true,
    );
    await eventually(
      () =>
        page.evaluate(
          "testIncoming.findLast(m=>m.type==='state' && m.state.events?.length)?.state.events[0].args[0].value.id==='-9223372036854775808'",
        ),
      "saved typed arguments restored after schema arrival",
    );
    await section("params");
    g = await geometry();
    const columns = Math.min(8,Math.max(1,Math.floor((g.width-46+8)/148)));
    const card = (g.width-46-(columns-1)*8)/columns;
    await page.call("Input.dispatchMouseEvent",{type:"mouseMoved",x:g.x+21+card/2,y:g.y+g.sections.params.y+g.sections.params.toolbar+65,buttons:0});
    assert.equal(await frame("document.querySelector('canvas').style.cursor"),"ns-resize","knob layout restored after navigation");
    await click(g.width-185,g.sections.params.y+(g.sections.params.toolbar>40?47:20));
    await key("5");
    await key("Enter", "Enter");
    await eventually(
      () =>
        page.evaluate(
          "testCommands.findLast(c=>c.type==='setParamSmoothing')?.milliseconds===5",
        ),
      "restored parameter smoothing is interactive",
    );
    assert.deepEqual(await page.evaluate("testErrors"), []);
    const report = {
      host,
      backend,
      passed: true,
      adapter: await page.evaluate(
        "testAdapters.findLast(a=>a.info && !a.info.fallback)?.info ?? null",
      ),
      scopePacketBytes,
      checks: [
        "source compilation",
        "real AudioWorklet playback",
        "bounded live scope",
        "parameter print",
        "structured event/i64/non-finite payload",
        "source recompilation",
        "event preservation",
        "file picker",
        "buffer drop/clear",
        "MIDI down/up",
        "parent keys",
        "pointer MIDI ownership through parent focus handoff",
        "logs",
        "MIDI settings",
        "scope collapse/resume",
        "theme",
        "saved presentation",
      ],
    };
    await mkdir(artifacts, { recursive: true });
    await writeFile(
      resolve(artifacts, `${host}-${backend}.json`),
      JSON.stringify(report, null, 2) + "\n",
    );
  });
}

for (const host of ["example", "website"]) {
  test(`GPU ${host} settings recompile through the shared controls`, { timeout: 90000 }, async t => {
    const base = await serve(t);
    const { page, frame, geometry, click, section, key } = await openEditor(
      t, `${base}/${host === "website" ? "website/" : ""}?backend=webgl`,
    );
    assert.equal(await page.evaluate("document.querySelectorAll('[data-sample-rate], [data-block-size]').length"), 0);
    assert.equal(await page.evaluate("document.querySelector('[data-editor-font-size]').closest('[data-project-menu]') !== null"), true);
    await section("params");
    await eventually(() => page.evaluate("testCommands.findLast(c=>c.type==='viewState')?.state.paramLayout==='knobs'"), "default knob layout");
    assert.equal(await page.evaluate("testCommands.findLast(c=>c.type==='viewState').state.velocity"), 1);

    await section("settings");
    let g = await geometry();
    await click(g.width - 150, 134);
    await key("Home", "Home");
    await key("Enter", "Enter");
    await eventually(() => frame("probe.state.sampleRateHz===44100 && probe.state.connected===true"), "sample-rate recompilation");
    assert.equal(await page.evaluate("testAudioContexts.at(-1).sampleRate"), 44100);

    await click(g.width - 150, 202);
    await key("Home", "Home");
    await key("ArrowDown", "ArrowDown");
    await key("Enter", "Enter");
    await eventually(() => frame("probe.state.blockSamples===256 && probe.state.connected===true"), "block-size recompilation");
    assert.deepEqual(await page.evaluate("JSON.parse(localStorage.getItem('onda.browser-ide.compile-options.v1'))"), { sampleRate: 44100, blockSize: 256 });

    // Removing note_on must remove keyboard support from the newly compiled UI.
    await page.evaluate("document.querySelector('.cm-content').focus()");
    await key("a", "KeyA", 2);
    await page.call("Input.insertText", { text: source.split("\nevent note_on")[0] });
    await key("s", "KeyS", 2);
    await eventually(() => frame("probe.state.midi?.noteOn===false && probe.state.connected===true"), "keyboard hidden without MIDI");
    const notes = await page.evaluate("testCommands.filter(c=>c.type==='midiNote').length");
    g = await geometry();
    await click(50, g.height - 20);
    assert.equal(await page.evaluate("testCommands.filter(c=>c.type==='midiNote').length"), notes);
    assert.deepEqual(await page.evaluate("testErrors"), []);
  });
}

test(
  "GPU web editor autoplay Stop never resumes audio",
  { timeout: 90000 },
  async (t) => {
    const base = await serve(t);
    const { page, frame, geometry, click, key } = await openEditor(
      t,
      base + "/?backend=webgl",
      false,
    );
    assert.equal(
      await page.evaluate("testAudioContexts.at(-1).state"),
      "suspended",
    );
    const resumes = await page.evaluate("testResumeCalls");
    const g = await geometry();
    await click(g.stopX,g.stopY);
    await eventually(
      () =>
        page.evaluate(
          "document.querySelector('[data-status]').textContent==='Stopped'",
        ),
      "Stop while autoplay is blocked",
    );
    await eventually(() => frame("probe.state.running===false"), "stopped GPU state");
    assert.equal(await page.evaluate("testResumeCalls"), resumes);
    await click(80, 92);
    assert.equal(await page.evaluate("testAudioContexts.length"), 1);
    await key("s", "KeyS", 2);
    await eventually(
      () => page.evaluate("testAudioContexts.at(-1).state==='running'"),
      "trusted Play after Stop",
    );
    await eventually(() => frame("probe.state.running===true"), "running GPU state");

    // Focus Stop without activating it, then suspend audio while it retains focus.
    await frame(`probe.ui.input(JSON.stringify({type:"focusLost"}));document.querySelector("canvas").focus()`);
    await key("Tab", "Tab");
    await page.evaluate("testAudioContexts.at(-1).suspend()");
    const beforeKeyboardStop = await page.evaluate("testResumeCalls");
    await key("Enter", "Enter");
    await eventually(
      () => page.evaluate("document.querySelector('[data-status]').textContent==='Stopped'"),
      "keyboard Stop while audio is suspended",
    );
    assert.equal(await page.evaluate("testResumeCalls"), beforeKeyboardStop);
    assert.notEqual(await page.evaluate("testAudioContexts.at(-1).state"), "running");
  },
);

test(
  "GPU web editor works without cross-origin isolation",
  { timeout: 90000 },
  async (t) => {
    const base = await serve(t, false);
    const { page, frame, geometry, click, section, key } = await openEditor(
      t,
      base + "/?backend=webgl",
    );
    assert.equal(await page.evaluate("crossOriginIsolated"), false);
    assert.equal(await page.evaluate("typeof SharedArrayBuffer"), "undefined");
    await eventually(
      () => frame("probe.nonzero"),
      "scope without shared memory",
    );
    await section("params");
    const g = await geometry();
    await click(g.paramX,g.paramY);
    for (const char of "880") await key(char);
    await key("Enter", "Enter");
    await section("scope");
    await eventually(
      () =>
        frame(`(()=>{
    const packet=Uint8Array.from(probe.packet || []);
    if(packet.length<8)return false;
    const view=new DataView(packet.buffer),columns=view.getUint16(6,true);
    let crossings=0,previous=0;
    for(let i=0;i<columns;i++){
      const current=view.getInt16(8+i*4,true)+view.getInt16(10+i*4,true);
      if(previous<0 && current>=0)crossings++;previous=current;
    }
    return crossings>=16;
  })()`),
      "880 Hz reaching the audio processor without print transport",
    );
    assert.deepEqual(await page.evaluate("testErrors"), []);
  },
);
