import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import test from "node:test";
import { openChrome, eventually } from "../chrome-test-session.js";
import { checkRegressions } from "./regression-checks.mjs";

async function serveGpuUi(t, beforeRead) {
  const server = createServer(async (req, res) => {
    try {
      const path = new URL(req.url, "http://localhost").pathname;
      if (path === "/away") {
        res.setHeader("Content-Type", "text/html");
        res.end("<!doctype html><title>Away</title>");
        return;
      }
      if (path === "/scroll-host") {
        res.setHeader("Content-Type", "text/html");
        res.end(`<!doctype html><style>
          body{margin:0;height:2400px}iframe{display:block;margin:100px 0 0 30px;
          width:480px;height:600px;border:0;zoom:1.5}
          </style><iframe src="/index.html?backend=webgl"></iframe><script>
          window.commands=[];addEventListener('message',event=>{
            if(event.origin===location.origin&&event.data?.__ondaRunUi)
              commands.push(event.data.message);
          });</script>`);
        return;
      }
      if (
        !/^\/(?:index\.html|host\.js|browser\.js|protocol\.js|text-input\.js|pkg\/(?:fonts\/)?[\w.-]+)$/.test(
          path,
        )
      ) {
        res.writeHead(404).end();
        return;
      }
      await beforeRead?.(path);
      const data = await readFile(new URL(`.${path}`, import.meta.url));
      res.setHeader(
        "Content-Type",
        path.endsWith(".wasm")
          ? "application/wasm"
          : path.endsWith(".js")
            ? "text/javascript"
            : "text/html",
      );
      res.end(data);
    } catch (error) {
      res.writeHead(500).end(String(error));
    }
  });
  t.after(() => new Promise(resolve => {
    server.close(resolve);
    server.closeAllConnections();
  }));
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${server.address().port}`;
}

test("GPU wheel scrolling chains to the page when the canvas cannot scroll", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:1200,height:800});
  await page.navigate(origin + "/scroll-host");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "embedded renderer");
  let readyId=0;
  const state=async update=>{
    await page.evaluate(`document.querySelector('iframe').contentWindow._onHostMessage({type:'state',state:${JSON.stringify({
      path:"scroll.onda",supportsScope:false,supportsTransport:false,supportsViewState:true,
      ...update,viewState:{reset:true,readyId:++readyId},
    })}})`);
    await eventually(() => page.evaluate(`commands.some(c=>c.type==='runViewReady'&&c.readyId===${readyId})`), "rendered scroll state");
  };
  const wheel=async (deltaY, {x=300,y=300}={})=>{
    await page.call("Input.dispatchMouseEvent",{type:"mouseMoved",x,y});
    await page.call("Input.dispatchMouseEvent",{type:"mouseWheel",x,y,deltaX:0,deltaY});
  };
  const editPoint=async()=>{
    await page.evaluate("document.querySelector('iframe').contentDocument.querySelector('canvas').focus({preventScroll:true})");
    for(let i=0;i<16;i++) {
      await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Tab",code:"Tab"});
      if(await page.evaluate("document.querySelector('iframe').contentDocument.activeElement.tagName==='TEXTAREA'")) break;
    }
    return page.evaluate(`(()=>{
      const iframe=document.querySelector('iframe'),editor=iframe.contentDocument.activeElement;
      if(editor.tagName!=='TEXTAREA') throw new Error('Expected an active numeric editor');
      const outer=iframe.getBoundingClientRect(),inner=editor.getBoundingClientRect();
      const scale=outer.width/iframe.clientWidth;
      return {x:outer.left+(inner.left+inner.width/2)*scale,
        y:outer.top+(inner.top+inner.height/2)*scale};
    })()`);
  };
  await state({params:[]});
  await wheel(200);
  await eventually(() => page.evaluate("scrollY>0"), "page scroll while the zoomed canvas has no scrollbar", 3000);

  await page.evaluate("scrollTo(0,0)");
  await state({params:Array.from({length:80},(_,i)=>({name:"p"+i,type:"f64",value:0}))});
  await wheel(200,await editPoint());
  await eventually(() => page.evaluate("commands.findLast(c=>c.type==='viewState')?.state.scrollTop>0"), "inner document scroll over a numeric editor",3000);
  assert.equal(await page.evaluate("scrollY"),0,"scrollable canvas consumes the wheel");
  assert.equal(await page.evaluate("commands.some(c=>c.type==='setParam')"),false,"scrolling must preserve the numeric draft");
  await wheel(100000);
  await eventually(() => page.evaluate("commands.findLast(c=>c.type==='viewState')?.state.scrollTop>1000"), "inner document bottom");
  assert.equal(await page.evaluate("scrollY"),0);
  await wheel(200);
  await eventually(() => page.evaluate("scrollY>0"), "page scroll at the canvas bottom", 3000);
  await page.evaluate("scrollTo(0,300)");
  await state({});
  await wheel(-200,await editPoint());
  await eventually(() => page.evaluate("scrollY<300"), "page scroll over a numeric editor at the canvas top", 3000);
});

test("partial states during GPU initialization retain schemas and queued values", { timeout: 60000 }, async t => {
  const wasm = Promise.withResolvers();
  t.after(() => wasm.resolve());
  const origin = await serveGpuUi(t, path => path.endsWith(".wasm") ? wasm.promise : undefined);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const session = await chrome.page({ width: 720, height: 840, source: `
    window.commands = [];
    window.addEventListener("ondaCommand", event => commands.push(event.detail));
  ` });
  await session.call("Page.navigate", { url: `${origin}/index.html?backend=webgl` });
  await eventually(() => session.evaluate("typeof _onHostMessage === 'function'"), "initializing GPU receiver");
  await session.evaluate(`
    _onHostMessage({type:"state",state:{path:"queued.onda",supportsScope:false,
      params:[{name:"frequency",type:"f64",value:440}]}});
    _onHostMessage({type:"parameterValues",state:{params:[{name:"frequency",value:880}]}});
    _onHostMessage({type:"state",state:{running:true}});
  `);
  wasm.resolve();
  await eventually(() => session.evaluate("commands.some(c => c.type === 'uiReady')"), "initialized GPU renderer");
  await session.evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const render=CanvasUi.prototype.render;
    window.allowRender=false;window.rendered=false;
    CanvasUi.prototype.render=function(){
      if(!allowRender)return false;
      rendered=render.call(this);return rendered;
    };
    _onHostMessage({type:"state",state:{viewState:{readyId:42}}});
    _onHostMessage({type:"state",state:{running:false}});
  })()`);
  assert.equal(await session.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===42)"), false,
    "readiness must wait for a successful canvas render");
  await session.evaluate("allowRender=true");
  await eventually(() => session.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===42)"), "presented project view");
  assert.equal(await session.evaluate("rendered"), true);
  for (let i = 0; i < 12; i++) {
    await session.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab" });
    if (await session.evaluate("document.activeElement.tagName === 'TEXTAREA'")) break;
  }
  assert.equal(await session.evaluate("document.activeElement.value"), "880");
  assert.equal(await session.evaluate("document.getElementById('status').hidden"), true,
    await session.evaluate("document.getElementById('status').textContent"));
});

for (const clipboard of ["denied", "unavailable"])
  test(`clipboard ${clipboard} leaves rendering and readiness working`, { timeout: 30000 }, async t => {
    const origin = await serveGpuUi(t);
    const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
    await chrome.call("Browser.setPermission", {
      permission: { name: "clipboard-write" }, setting: "denied", origin,
    });
    const page = await chrome.page({ width: 720, height: 840, source: `
      window.commands=[];window.clipboardErrors=[];
      addEventListener('ondaCommand',event=>commands.push(event.detail));
    ` });
    await page.navigate(origin + "/index.html?backend=webgl");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU renderer");
    await page.evaluate(`(async()=>{
      const {CanvasUi}=await import('./pkg/onda_ui.js');
      const message=CanvasUi.prototype.message;
      CanvasUi.prototype.message=function(text){
        const value=JSON.parse(text);
        if(value.state?.error) clipboardErrors.push(value.state.error);
        return message.call(this,text);
      };
      ${clipboard === "unavailable" ? 'Object.defineProperty(navigator,"clipboard",{value:undefined});' : ""}
      _onHostMessage({type:'state',state:{path:'copy.onda',running:false,
        supportsScope:false,logText:'Copy this text\\n',viewState:{readyId:1}}});
    })()`);
    await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===1)"), "log view");
    for (const type of ["mousePressed", "mouseReleased"])
      await page.call("Input.dispatchMouseEvent", { type, x: 610, y: 116, button: "left", clickCount: 1 });
    await eventually(() => page.evaluate("clipboardErrors.length || commands.some(c=>c.type==='runViewError')"), "clipboard error feedback");
    assert.equal(await page.evaluate("document.getElementById('status').hidden"), true,
      "a clipboard error must not mark the working renderer as failed");
    assert.match(await page.evaluate("clipboardErrors.at(-1)"), /Could not copy clipboard text:/);
    assert.equal(await page.evaluate("commands.some(c=>c.type==='runViewError')"), false);

    await page.evaluate("commands=[];_onHostMessage({type:'requestRunViewReady'})");
    assert.equal(await page.evaluate("commands.some(c=>c.type==='uiReady')"), true);
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'next.onda',error:null,
      viewState:{readyId:2}}})`);
    await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===2)"), "render after clipboard failure");
  });

test("fallback font failures retry without blocking transport", { timeout: 30000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720, height:840, source:`
    window.commands=[];window.fontAttempts=0;window.loadedFonts=[];window.fontWarnings=[];
    addEventListener('ondaCommand',event=>commands.push(event.detail));
    const fetchFont=window.fetch;
    window.fetch=function(...args) {
      if(String(args[0]).includes('NotoSansCJK')) {
        fontAttempts++;
        if(fontAttempts===1) return Promise.reject(new TypeError('Temporary font network failure'));
        if(fontAttempts===2) return Promise.resolve(new Response('',{status:503}));
      }
      return fetchFont.apply(this,args);
    };
    console.warn=(...args)=>fontWarnings.push(args.map(String).join(' '));
  `});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU renderer", 10000);
  await page.evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const load=CanvasUi.prototype.load_font;
    CanvasUi.prototype.load_font=function(name,bytes) {
      load.call(this,name,bytes);loadedFonts.push(name);
    };
    _onHostMessage({type:'state',state:{path:'世界.onda',running:false,supportsScope:false}});
  })()`);
  await eventually(() => page.evaluate("fontAttempts===1 && (fontWarnings.length===1 || !document.getElementById('status').hidden)"), "failed font request", 5000);
  assert.equal(await page.evaluate("document.getElementById('status').hidden"), true,
    "an optional font failure must not replace the working UI with an error overlay");
  assert.equal(await page.evaluate("document.elementFromPoint(656,48).id"), "onda-ui");
  for (const type of ["mousePressed", "mouseReleased"])
    await page.call("Input.dispatchMouseEvent", {type, x:656, y:48, button:"left", clickCount:1});
  await eventually(() => page.evaluate("commands.some(c=>c.type==='start')"), "transport after font failure", 5000);

  await eventually(() => page.evaluate("fontAttempts===2 && fontWarnings.length===2"), "HTTP font failure", 5000);
  await page.evaluate(`(async()=>{
    for(let i=0;i<10;i++) _onHostMessage({type:'state',state:{status:'Update '+i}});
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
  })()`);
  assert.equal(await page.evaluate("fontAttempts"), 2, "state updates must respect retry backoff");
  await eventually(() => page.evaluate("loadedFonts.length===1"), "successful font retry", 5000);
  assert.equal(await page.evaluate("fontAttempts"), 3);
  assert.equal(await page.evaluate("document.getElementById('status').hidden"), true);
  await page.evaluate(`(async()=>{
    _onHostMessage({type:'state',state:{path:'another.onda'}});
    _onHostMessage({type:'state',state:{path:'世界.onda'}});
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
  })()`);
  assert.equal(await page.evaluate("fontAttempts"), 3, "loaded fonts must not be fetched again");
});

// Real canvas, Wasm, text, shader pipelines, and pointer/keyboard events. CDP
// uses Node's WebSocket so this test needs only a Chromium executable.
for (const backend of ["webgpu", "webgl"])
  test(`GPU run screen (${backend})`, { timeout: 90000 }, async (t) => {
    const origin = await serveGpuUi(t);
    const args = JSON.parse(
      process.env.ONDA_UI_CHROME_ARGS ||
        (backend === "webgpu" && process.platform === "linux"
          ? '["--use-angle=vulkan","--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan","--disable-vulkan-surface"]'
          : '["--use-angle=swiftshader"]'),
    );
    const chrome = await openChrome(t, args);
    const session = await chrome.page();
    const page = session.call;
    const evaluate = session.evaluate;
    await page("Page.enable");
    await page("Page.addScriptToEvaluateOnNewDocument", {
      source: `window.commands = [];
    window.navigationId=Math.random();window.cachedRestores=0;
    window.addEventListener('pageshow',event=>{if(event.persisted)cachedRestores++;});
    window.errors=[];const reportError=console.error;console.error=(...args)=>{errors.push(args.map(String).join(" "));reportError(...args);};
    window.addEventListener("ondaCommand", event => commands.push(event.detail));`,
    });
    await page("Emulation.setDeviceMetricsOverride", {
      width: 720,
      height: 840,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await page("Page.navigate", {
      url: `${origin}/index.html?backend=${backend}`,
    });
    await evaluate(`new Promise((resolve,reject) => {
    const start=Date.now(); const timer=setInterval(() => {
      if (window.commands?.some(c => c.type === "uiReady")) {clearInterval(timer);resolve(true);}
      else if (Date.now()-start>30000) {clearInterval(timer);reject(new Error(document.getElementById("status")?.textContent));}
    },20);
  })`);
    const state = {
      type: "state",
      state: {
        path: "test.onda",
        running: true,
        paramLayout: "sliders",
        params: [
          {
            name: "frequency",
            type: "f64",
            value: 440,
            default: 220,
            rangeMin: 20,
            rangeMax: 20000,
            scale: "log",
          },
          {
            name: "integer",
            type: "i64",
            value: "9223372036854775807",
            default: "9223372036854775807",
          },
        ],
      },
    };
    await evaluate(
      `_onHostMessage(${JSON.stringify(state)}); new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`,
    );
    async function click(x, y, clickCount = 1) {
      await page("Input.dispatchMouseEvent", {
        type: "mousePressed",
        x,
        y,
        button: "left",
        clickCount,
      });
      await page("Input.dispatchMouseEvent", {
        type: "mouseReleased",
        x,
        y,
        button: "left",
        clickCount,
      });
    }
    await click(180, 375);
    for (const key of "880")
      await page("Input.dispatchKeyEvent", { type: "keyDown", key, text: key });
    await page("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter" });
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParam').value`),
      880,
    );
    await click(405, 375);
    for (const key of "-9223372036854775808")
      await page("Input.dispatchKeyEvent", { type: "keyDown", key, text: key });
    await page("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter" });
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParam').value`),
      "-9223372036854775808",
    );
    await evaluate(
      `_onHostMessage({type:'scopeData',channels:2,samples:Array.from({length:2048},(_,i)=>Math.sin(i*.03))})`,
    );
    const packet = Buffer.alloc(32);
    packet.write("OSQ1");
    packet.writeUInt16LE(2, 4);
    packet.writeUInt16LE(3, 6);
    [0, 10000, -32767, 32767, -10000, 0, 0, 10000, -12000, 32767, 0, 0].forEach(
      (v, i) => packet.writeInt16LE(v, 8 + i * 2),
    );
    await evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const scope=CanvasUi.prototype.scope_packet;
    window.appliedScopes=0;
    CanvasUi.prototype.scope_packet=function(bytes){appliedScopes++;return scope.call(this,bytes);};
    for(let i=0;i<100;i++)_onHostMessage({type:'scopeData',packet:'${packet.toString("base64")}'});
    await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
  })()`);
    assert.equal(
      await evaluate("appliedScopes"),
      1,
      "scope bursts must apply only the newest frame",
    );
    await evaluate(`appliedScopes=0;
    _onHostMessage({type:'scopeData',packet:'${packet.toString("base64")}'});
    _onHostMessage({type:'state',state:{viewState:{reset:true}}});
    new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    assert.equal(
      await evaluate("appliedScopes"),
      0,
      "a reset must discard scope data queued before it",
    );
    await click(665, 336);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParamLayout').layout`),
      "knobs",
    );
    await click(103, 420);
    await click(103, 420, 2);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParam'&&c.name==='frequency').value`),
      220,
      "real mouse double-click must reset the knob",
    );
    await evaluate(`_onHostMessage({type:'parameterValues',state:{params:[{name:'frequency',value:440}]}})`);
    await click(103, 468);
    assert.equal(await evaluate("document.activeElement.tagName"), "TEXTAREA");
    await click(103, 468, 2);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParam'&&c.name==='frequency').value`),
      220,
      "the second click must reset a number even after its native editor opens",
    );
    assert.equal(await evaluate("document.activeElement.tagName"), "CANVAS");
    for (const editing of [false, true]) {
      for (const [key, code, virtualKey, command] of [
        ["Enter", "Enter", 13, "start"],
        ["s", "KeyS", 83, "start"],
        [".", "Period", 190, "stop"],
      ]) {
        if (editing) await click(103, 468);
        else await evaluate("document.getElementById('onda-ui').focus()");
        assert.equal(await evaluate("document.activeElement.tagName"), editing ? "TEXTAREA" : "CANVAS");
        const count = () => evaluate(`commands.filter(c=>c.type==='${command}').length`);
        const before = await count();
        const shortcut = {key, code, windowsVirtualKeyCode:virtualKey, modifiers:2};
        await page("Input.dispatchKeyEvent", {type:"keyDown", ...shortcut, autoRepeat:true});
        assert.equal(await count(), before, "repeated shortcuts must leave a field draft and transport untouched");
        assert.equal(await evaluate("document.activeElement.tagName"), editing ? "TEXTAREA" : "CANVAS");
        await page("Input.dispatchKeyEvent", {type:"keyDown", ...shortcut});
        for (let i=0; i<3; i++)
          await page("Input.dispatchKeyEvent", {type:"keyDown", ...shortcut, autoRepeat:true});
        await page("Input.dispatchKeyEvent", {type:"keyUp", ...shortcut});
        assert.equal(await count(), before + 1, "one shortcut press must emit one transport command");
      }
    }
    const fullState = JSON.parse(
      await readFile(
        new URL("./fixtures/panels.json", import.meta.url),
        "utf8",
      ),
    );
    fullState.paramLayout = "sliders";
    await evaluate(
      `_onHostMessage({type:'state',state:${JSON.stringify(fullState)}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`,
    );
    await evaluate(
      `for (const state of [{buffers:${JSON.stringify(fullState.buffers)}},{events:${JSON.stringify(fullState.events)}},{logText:"partial update"},{paramSmoothingMs:12}])_onHostMessage({type:"state",state});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`,
    );
    assert.equal(
      await evaluate("document.getElementById('status').hidden"),
      true,
      "partial states must work without a VS Code bridge",
    );
    const section = async (name) => {
      const state = structuredClone(fullState);
      for (const bank of ["params","inputControls","buffers","events"]) if(bank!==name) state[bank]=[];
      if(name!=="log"){state.logText="";state.logRevealed=false;}
      state.viewState = {reset:true,sections:Object.fromEntries(
        ["scope","params","inputControls","buffers","events","log","midi"].map(key=>[key,key==="scope"||key==="midi"||key===name]))};
      // Reset establishes a new fixture; subsequent edits use trusted browser input.
      await evaluate(`_onHostMessage({type:"state",state:${JSON.stringify(state)}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    };
    await section("inputControls");
    await click(180, 375);
    for (const key of "0.75")
      await page("Input.dispatchKeyEvent", { type: "keyDown", key, text: key });
    await page("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter" });
    const inputChange = await evaluate(
      `commands.findLast(c=>c.type==='setInputControl')`,
    );
    assert.equal(
      inputChange?.value,
      0.75,
      JSON.stringify(
        await evaluate(
          `({errors,commands:commands.slice(-12),error:document.getElementById('status').textContent})`,
        ),
      ),
    );
    await section("buffers");
    await click(586, 373);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='chooseBufferFile').name`),
      "Piano",
    );
    await section("events");
    await click(647, 372);
    assert.deepEqual(
      await evaluate(`commands.findLast(c=>c.type==='triggerEvent').values`),
      [0.8],
    );
    assert.equal(
      await evaluate(
        `commands.findLast(c=>c.type==='triggerEvent').engineGeneration`,
      ),
      "42",
    );
    await section("log");
    await chrome.call("Browser.grantPermissions",{origin,permissions:["clipboardReadWrite","clipboardSanitizedWrite"]});
    await click(610,336);
    await eventually(async()=>await evaluate("navigator.clipboard.readText()")===fullState.logText,
      "Copy exports the full log through the host clipboard adapter");
    await click(668, 336);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='clearLog').type`),
      "clearLog",
    );
    await section("midi");
    await evaluate(`_onHostMessage({type:'state',state:{currentMidiInputDevice:'Computer Keyboard'}})`);
    await page("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "a",
      text: "a",
    });
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='midiNote').pressed`),
      true,
    );
    await page("Input.dispatchKeyEvent", { type: "keyUp", key: "a" });
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='midiNote').pressed`),
      false,
    );
    await page("Input.dispatchKeyEvent", { type:"keyDown", key:"a", text:"a" });
    await evaluate("window.dispatchEvent(new Event('blur'))");
    assert.deepEqual(
      await evaluate(`commands.findLast(c=>c.type==='midiNote')`),
      {type:"midiNote",key:60,pressed:false,velocity:0},
      "window blur must release held notes without a visibility change",
    );
    await page("Input.dispatchKeyEvent", { type:"keyUp", key:"a" });
    await section("params");
    await click(602,48);
    await evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    assert.equal(await evaluate("document.getElementById('status').hidden"),true);
    assert.deepEqual(await evaluate("errors"),[],"opening settings must not produce GPU validation errors");
    // Escape closes the settings popup; section folds use the common view-state protocol.
    await page("Input.dispatchKeyEvent", {type:"keyDown",key:"Escape"});
    await click(80,116);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='scopeConfig').visible`),false);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='viewState').state.sections.scope`),false);
    await click(80,116);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='scopeConfig').visible`),true);
    for (const themeMode of ["light","dark"]) {
      await evaluate(`_onHostMessage({type:'state',state:{themeMode:'${themeMode}'}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
      assert.equal(await evaluate(`document.getElementById('status').hidden`),true);
    }
    // Real pointer input must scrub numbers and update hover without a held button.
    await section("params");
    await page("Input.dispatchMouseEvent",{type:"mousePressed",x:180,y:375,button:"left",clickCount:1});
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:255,y:375,button:"left",buttons:1});
    await page("Input.dispatchMouseEvent",{type:"mouseReleased",x:255,y:375,button:"left",clickCount:1});
    assert.ok(await evaluate(`commands.findLast(c=>c.type==='setParam').value>0`));
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='setParam').commit`),true);
    await click(180,375);
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"-",text:"-"});
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter"});
    await click(519,48);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='stop')?.type`),"stop");
    await click(665,336);
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:102,y:415,buttons:0});
    assert.equal(await evaluate(`document.querySelector('canvas').style.cursor`),"ns-resize");
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:102,y:473,buttons:0});
    assert.equal(await evaluate(`document.querySelector('canvas').style.cursor`),"ew-resize");
    await click(102,473);
    assert.equal(await evaluate(`getComputedStyle(document.activeElement).cursor`),"text");
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
    await section("log");
    await click(180,40);
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:100,y:420,buttons:0});
    await page("Input.dispatchMouseEvent",{type:"mouseWheel",x:100,y:420,deltaX:0,deltaY:-120});
    await evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    await eventually(() => evaluate(`commands.findLast(c=>c.type==='viewState')?.state.logFollow===false`), "settled log scroll is saved");
    // The empty screen exposes the same keyboard-operated sample/block dropdowns.
    await evaluate(`_onHostMessage({type:'state',state:{path:'',params:[],inputControls:[],events:[],buffers:[],midiKeyboardInteractive:false,midi:{noteOn:false},sampleRateHz:48000,blockSamples:512,supportsRunSettings:true,supportsFastMath:true}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    await click(243,526);
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"End"});
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter"});
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='setRunSettings').sampleRateHz`),192000);
    await section("params");
    await page("Emulation.setDeviceMetricsOverride", {
      width: 400,
      height: 600,
      deviceScaleFactor: 2,
      mobile: false,
    });
    await evaluate(
      `new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`,
    );
    assert.equal(
      await evaluate(`document.getElementById('status').hidden`),
      true,
    );
    assert.equal(await evaluate(`document.querySelector('canvas').width`), 800);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='scopeConfig').columns`),
      356,
    );
    if (process.env.ONDA_GPU_SCREENSHOT) {
      const screenshot = await page("Page.captureScreenshot", {
        format: "png",
      });
      await writeFile(
        `${process.env.ONDA_GPU_SCREENSHOT}-${backend}.png`,
        Buffer.from(screenshot.data, "base64"),
      );
    }
    // Trusted touch input pans a number card, then a tap opens a native editor.
    await page("Emulation.setTouchEmulationEnabled", { enabled:true, maxTouchPoints:1 });
    await evaluate(`_onHostMessage({type:"state",state:{path:"touch.onda",supportsScope:false,
      supportsTransport:false,midiKeyboardInteractive:false,midi:{noteOn:false},
      buffers:[],events:[],inputControls:[],logText:"",logRevealed:false,paramLayout:"sliders",
      params:Array.from({length:80},(_,i)=>({name:"Parameter "+(i+1),type:"i64",value:"0"})),
      viewState:{reset:true}}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    const touch = (type, x, y) => page("Input.dispatchTouchEvent", {
      type, touchPoints:type === "touchEnd" ? [] : [{id:0,x,y}],
    });
    await touch("touchStart",315,147);
    await touch("touchMove",315,47);
    await touch("touchEnd");
    await eventually(() => evaluate(`commands.findLast(c=>c.type==='viewState')?.state.scrollTop>=90`), "a vertical swipe must scroll instead of editing a number");
    assert.equal(await evaluate("document.activeElement.tagName"),"CANVAS");
    await evaluate(`_onHostMessage({type:'state',state:{paramLayout:'sliders',viewState:{reset:true}}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    await touch("touchStart",315,147);
    await touch("touchEnd");
    assert.equal(await evaluate("document.activeElement.tagName"),"TEXTAREA","touch editing must focus an element that opens the software keyboard");
    await page("Input.insertText",{text:"12345"});
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Home",code:"Home",windowsVirtualKeyCode:36});
    for(let i=0;i<2;i++)await page("Input.dispatchKeyEvent",{type:"keyDown",key:"ArrowRight",code:"ArrowRight",windowsVirtualKeyCode:39,modifiers:8});
    assert.deepEqual(await evaluate("[document.activeElement.selectionStart,document.activeElement.selectionEnd]"),[0,2]);
    await page("Input.insertText",{text:"9"});
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter"});
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='setParam').value`),"9345");
    await touch("touchStart",315,147);
    await touch("touchEnd");
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"c",code:"KeyC",windowsVirtualKeyCode:67,modifiers:2});
    assert.equal(await evaluate("navigator.clipboard.readText()"),"9345","native browser Copy must preserve the selection");
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"x",code:"KeyX",windowsVirtualKeyCode:88,modifiers:2});
    assert.equal(await evaluate("document.activeElement.value"),"");
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"v",code:"KeyV",windowsVirtualKeyCode:86,modifiers:2});
    await eventually(()=>evaluate("document.activeElement.value==='9345'"),"native browser Paste");
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
    await page("Emulation.setTouchEmulationEnabled",{enabled:false});
    await evaluate(`_onHostMessage({type:'state',state:{supportsTransport:true}})`);
    // Preserve the renderer through cached suspension on either backend.
    await evaluate(`window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));
      window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await evaluate("document.getElementById('status').hidden"),true);
    if (backend === "webgl") {
      const navigationId = await evaluate("navigationId");
      await session.navigate(`${origin}/away`);
      await eventually(()=>evaluate("location.pathname==='/away'"),"away page");
      await evaluate("history.back()");
      await eventually(()=>evaluate("location.pathname==='/index.html' && window.cachedRestores>=2"),"cached renderer restore");
      assert.equal(await evaluate("navigationId"),navigationId,"test must restore the cached document");
      await evaluate(`_onHostMessage({type:'state',state:{running:false}});
        document.getElementById('onda-ui').focus();
        new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
      const starts = await evaluate("commands.filter(c=>c.type==='start').length");
      await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",modifiers:2});
      assert.equal(await evaluate("commands.filter(c=>c.type==='start').length"),starts+1);
      assert.equal(await evaluate("document.getElementById('status').hidden"),true);
    }
    await checkRegressions(page,evaluate);
    assert.deepEqual(await evaluate("errors"),[],"lifecycle changes must preserve a usable renderer");
  });

test("VS Code numeric editors stay aligned with the canvas", { timeout: 30000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const session = await chrome.page({ width: 720, height: 600, source: `
    window.commands=[];
    window.acquireVsCodeApi=()=>({
      postMessage:message=>commands.push(message),getState:()=>null,setState:()=>{},
    });
  ` });
  await session.navigate(`${origin}/index.html?backend=webgl`);
  await eventually(() => session.evaluate("commands.some(c=>c.type==='uiReady')"), "VS Code GPU renderer");
  await session.evaluate(`(async()=>{
    // VS Code injects this default before the webview's own styles.
    const style=document.createElement('style');
    style.textContent='body { padding: 0 20px; }';
    document.head.prepend(style);
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const snapshot=CanvasUi.prototype.text_edit;
    CanvasUi.prototype.text_edit=function(){
      const result=snapshot.call(this),edit=JSON.parse(result);
      if(edit) window.lastEdit=edit;
      return result;
    };
  })()`);
  for (const layout of ["sliders", "knobs"]) {
    await session.evaluate(`_onHostMessage({type:'state',state:{
      path:'resonance.onda',supportsScope:false,supportsTransport:false,
      paramLayout:${JSON.stringify(layout)},viewState:{reset:true},
      params:[{name:'resonance',type:'f32',value:5.80721,rangeMin:0.5,rangeMax:10}]
    }});document.querySelector('canvas').focus({preventScroll:true});
    new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    for(let i=0;i<12;i++) {
      await session.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Tab"});
      if(await session.evaluate("document.activeElement.tagName==='TEXTAREA'")) break;
    }
    const position=await session.evaluate(`(()=>{
      const r=lastEdit.rect,c=document.querySelector('canvas').getBoundingClientRect();
      return {x:c.left+r[0]+r[2]/2,y:c.top+r[1]+r[3]/2};
    })()`);
    await session.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
    for(const type of ["mousePressed","mouseReleased"])
      await session.call("Input.dispatchMouseEvent",{type,...position,button:"left",clickCount:1});
    const geometry=await session.evaluate(`(()=>{
      const canvas=document.querySelector('canvas').getBoundingClientRect();
      const editor=document.activeElement.getBoundingClientRect(),r=lastEdit.rect;
      return {tag:document.activeElement.tagName,
        actual:[editor.left,editor.top,editor.width,editor.height],
        expected:[canvas.left+r[0],canvas.top+r[1],r[2],r[3]],
        canvas:[canvas.left,canvas.top,canvas.width,canvas.height]};
    })()`);
    assert.equal(geometry.tag,"TEXTAREA");
    for(let i=0;i<4;i++)
      assert.ok(Math.abs(geometry.actual[i]-geometry.expected[i])<0.1,
        `${layout}: native editor ${JSON.stringify(geometry.actual)} must match canvas field ${JSON.stringify(geometry.expected)}`);
    assert.deepEqual(geometry.canvas,[0,0,720,600],"host padding must not offset or clip the GPU screen");
    await session.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
  }
});

test("VS Code retains folds, scroll, and typed event values across webview reloads", { timeout: 60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const session = await chrome.page({ width: 720, height: 840, source: `
    window.commands = [];
    window.saved = JSON.parse(sessionStorage.getItem("viewState") || "null");
    window.saves = 0;
    window.acquireVsCodeApi = () => ({
      postMessage: message => commands.push(message),
      getState: () => saved,
      setState: state => {
        saved = state;
        saves++;
        sessionStorage.setItem("viewState", JSON.stringify(state));
      },
    });
  ` });
  const ready = () => eventually(() => session.evaluate("commands.some(c => c.type === 'uiReady')"), "VS Code GPU renderer");
  const publish = state => session.evaluate(`_onHostMessage({type:"state",state:${JSON.stringify(state)}});
    new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
  const wheel = async deltaY => {
    await session.call("Input.dispatchMouseEvent", { type: "mouseMoved", x: 200, y: 400 });
    await session.call("Input.dispatchMouseEvent", { type: "mouseWheel", x: 200, y: 400, deltaX: 0, deltaY });
  };
  const state = {
    path: "vscode.onda", schemaReady: true, connected: true,
    supportsScope: false, supportsTransport: false,
    midiKeyboardInteractive: false, midi: { noteOn: false },
    params: Array.from({ length: 64 }, (_, i) => ({ name: `p${i}`, type: "f64", value: 0 })),
    events: [{ name: "send", args: [{ name: "value", type: "i64", value: "1" }] }],
  };
  await session.navigate(`${origin}/index.html?backend=webgl`);
  await ready();
  await publish(state);
  for (let i = 0; i < 12; i++) {
    await session.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab" });
    if (await session.evaluate("document.activeElement.tagName === 'TEXTAREA'")) break;
  }
  assert.equal(await session.evaluate("document.activeElement.value"), "1");
  await session.call("Input.insertText", { text: "9223372036854775807" });
  await session.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter" });
  assert.equal(await session.evaluate("saved?.eventValues[0].values[0]"), "9223372036854775807");
  for (const type of ["mousePressed", "mouseReleased"])
    await session.call("Input.dispatchMouseEvent", { type, x: 60, y: 116, button: "left", clickCount: 1 });
  assert.equal(await session.evaluate("saved.sections.events"), false);
  await wheel(260);
  await eventually(() => session.evaluate("saved.scrollTop > 0"), "saved VS Code scroll");
  const saved = await session.evaluate("saved");

  const loaded = session.event("Page.loadEventFired");
  await session.call("Page.reload");
  await loaded;
  await ready();
  await publish({ ...state, schemaReady: false, params: [], events: [] });
  assert.equal(await session.evaluate("saves"), 0, "loading metadata must preserve saved values");
  await publish(state);
  await wheel(1);
  await eventually(() => session.evaluate("saves > 0"), "restored VS Code presentation");
  const restored = await session.evaluate("saved");
  assert.equal(restored.scrollTop, saved.scrollTop + 1);
  assert.deepEqual(restored.sections, saved.sections);
  assert.deepEqual(restored.eventValues, saved.eventValues);
  assert.equal(await session.evaluate("document.getElementById('status').hidden"), true);
});
