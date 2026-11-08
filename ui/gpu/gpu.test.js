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
      if (path === "/touch-host") {
        res.setHeader("Content-Type", "text/html");
        res.end(`<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1">
          <style>body{margin:0;height:2400px}iframe{display:block;margin-top:100px;
          width:360px;height:680px;border:0}</style><iframe src="/index.html?backend=webgl"></iframe>
          <script>window.commands=[];addEventListener('message',event=>{
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

for (const backend of ["webgl", "webgpu"]) {
  test(`resize retains the ${backend} frame until its replacement is drawn`, {
    timeout:60000,
    skip:backend === "webgpu" && process.env.ONDA_UI_TEST_WEBGPU === "0",
  }, async t => {
    const origin = await serveGpuUi(t);
    const chrome = await openChrome(t, JSON.parse(process.env.ONDA_UI_CHROME_ARGS ||
      (backend === "webgpu" && process.platform === "linux"
        ? '["--use-angle=vulkan","--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan","--disable-vulkan-surface"]'
        : '["--use-angle=swiftshader"]')));
    const page = await chrome.page({width:720,height:840,
      source:"window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
    if (backend === "webgpu") {
      await page.navigate(origin + "/away");
      await eventually(() => page.evaluate(`(async()=>{
        const adapter=await navigator.gpu?.requestAdapter();
        return Boolean(adapter) && adapter.info?.isFallbackAdapter!==true;
      })()`), "hardware GPU adapter");
    }
    await page.navigate(origin + "/index.html" + (backend === "webgl" ? "?backend=webgl" : ""));
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'resize.onda',supportsScope:false,
      params:[{name:'gain',type:'f64',value:0.5}]}});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await page.evaluate("document.querySelector('canvas').dataset.backend"), backend);
    // Hold the host's next draw while the browser lays out and composites the resized page.
    await page.evaluate(`window.originalRAF=requestAnimationFrame;window.delayedFrames=[];
      window.requestAnimationFrame=cb=>(delayedFrames.push(cb),10000+delayedFrames.length)`);
    for (const [width, height, scale] of [[900,700,1],[480,900,2],[800,600,1]]) {
      await page.call("Emulation.setDeviceMetricsOverride", {width,height,deviceScaleFactor:scale,mobile:false});
      await page.evaluate("dispatchEvent(new Event('resize'));new Promise(r=>setTimeout(r,30))");
      assert.deepEqual(await page.evaluate("[document.querySelector('canvas').width,document.querySelector('canvas').height]"),
        [720,840], "layout must retain the previous backing store while drawing is deferred");
    }
    assert.equal(await page.evaluate("delayedFrames.length"), 1, "resize bursts must coalesce");
    await page.evaluate(`window.requestAnimationFrame=originalRAF;
      for(const cb of delayedFrames)originalRAF(cb);
      new Promise(r=>originalRAF(()=>originalRAF(r)))`);
    assert.deepEqual(await page.evaluate("[document.querySelector('canvas').width,document.querySelector('canvas').height]"), [800,600]);
    assert.equal(await page.evaluate("document.querySelector('#status').hidden"), true);
  });
}

test("UI wakeups save without drawing, show idle feedback once, and return to sleep", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:840,
    source:"window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  await page.evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const update=CanvasUi.prototype.update,render=CanvasUi.prototype.render;
    window.draws=0;window.updates=[];
    CanvasUi.prototype.update=function(){
      window.testUi=this;const redraw=update.call(this);updates.push(redraw);return redraw;
    };
    CanvasUi.prototype.render=function(){draws++;return render.call(this)};
    _onHostMessage({type:'state',state:{path:'schedule.onda',supportsScope:false,
      supportsViewState:true,supportsTransport:false,viewState:{readyId:1},
      params:Array.from({length:60},(_,i)=>({name:'p'+i,type:'f64',value:0}))}});
  })()`);
  await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===1)"), "initial state");
  await page.evaluate(`draws=0;updates=[];commands=[];
    document.querySelector('canvas').dispatchEvent(new WheelEvent('wheel',{
      deltaY:20,clientX:700,clientY:400,bubbles:true,cancelable:true}));`);
  await eventually(() => page.evaluate("commands.some(c=>c.type==='viewState'&&c.state.scrollTop===20)"), "idle scroll persistence");
  assert.equal(await page.evaluate("draws"), 1, "the delayed save must not submit another frame");
  assert.equal(await page.evaluate("updates.includes(false)"), true, "a clean timed update must still deliver commands");
  assert.equal(await page.evaluate("testUi.wake_delay_ms()===undefined"), true);
  await page.call("Input.dispatchMouseEvent", {type:"mouseMoved",x:100,y:35});
  await eventually(() => page.evaluate("testUi.wake_delay_ms()>0"), "hover deadline");
  const draws = await page.evaluate("draws");
  await eventually(() => page.evaluate(`draws===${draws + 1}&&testUi.wake_delay_ms()===undefined`), "one idle tooltip frame");
  const updates = await page.evaluate("updates.length");
  await page.evaluate("new Promise(r=>setTimeout(r,150))");
  assert.equal(await page.evaluate("updates.length"), updates, "idle UI must stop waking");
});

test("hosts own cadence and presentation while explicit canvas renders stay unpaced", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:840,
    source:"window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  await page.evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const update=CanvasUi.prototype.update;
    CanvasUi.prototype.update=function(){window.testUi=this;return update.call(this)};
    _onHostMessage({type:'state',state:{path:'host-pacing.onda',supportsScope:false,
      supportsTransport:false,viewState:{readyId:99},
      params:[{name:'gain',type:'f64',value:0.5,rangeMin:0,rangeMax:1}]}});
  })()`);
  await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===99)"), "paced readiness");
  const capabilities = await page.evaluate(`(async()=>{
    const {FramePacer,PresentMode}=await import('./pkg/onda_ui.js');
    const unlimited=new FramePacer(undefined), limited=new FramePacer(120);
    unlimited.timer_submitted(100);limited.timer_submitted(100);
    const delays=[unlimited.timer_delay_ms(100),limited.timer_delay_ms(100)];
    unlimited.free();limited.free();
    const jittered=new FramePacer(60);
    const phase=[3,29.667,56.333,63.667].map(timestamp=>{
      const delay=jittered.display_delay_ms(timestamp);
      jittered.display_submitted(timestamp);
      return [delay,jittered.display_delay_ms(timestamp)>0];
    });
    jittered.free();
    const mode=testUi.present_mode(),mask=testUi.present_modes();
    let rejected=false;
    try{testUi.set_presentation(PresentMode.Fifo,0)}catch{rejected=true}
    const unchanged=testUi.present_mode()===mode;
    testUi.set_presentation(PresentMode.MailboxOrFifo,2);
    const fallback=testUi.present_mode();
    testUi.set_presentation(PresentMode.Fifo,2);
    const explicit=[testUi.render(),testUi.render()];
    return {delays,phase,mode,mask,fallback,rejected,unchanged,explicit,clean:!testUi.update()};
  })()`);
  assert.equal(capabilities.delays[0], 0);
  assert.ok(Math.abs(capabilities.delays[1] - 1000 / 120) < 1e-9);
  assert.deepEqual(capabilities.phase, Array(4).fill([0,true]));
  assert.equal(capabilities.mode, 2); // PresentMode.Fifo
  assert.ok(capabilities.mask & (1 << capabilities.mode));
  assert.equal(capabilities.mask & (1 << 6), 0, "capabilities contain concrete modes only");
  assert.equal(capabilities.fallback, capabilities.mask & (1 << 5) ? 5 : 2);
  assert.ok(capabilities.rejected && capabilities.unchanged && capabilities.clean);
  assert.deepEqual(capabilities.explicit, [true,true]);

  await page.evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const message=CanvasUi.prototype.message,render=CanvasUi.prototype.render;
    window.frameTimes=[];window.lastRendered=undefined;let applied;
    CanvasUi.prototype.message=function(json){
      const data=JSON.parse(json);
      if(data.type==='parameterValues')applied=data.state.params?.[0]?.value;
      return message.call(this,json);
    };
    CanvasUi.prototype.render=function(){
      const result=render.call(this);
      if(result){frameTimes.push(performance.now());lastRendered=applied}
      return result;
    };
    await new Promise(resolve=>{
      let sequence=0;
      const timer=setInterval(()=>_onHostMessage({type:'parameterValues',state:{
        params:[{name:'gain',value:(++sequence%100)/100}]}}),4);
      setTimeout(()=>{
        clearInterval(timer);
        _onHostMessage({type:'parameterValues',state:{params:[{name:'gain',value:0.875}]}});
        resolve();
      },350);
    });
  })()`);
  await eventually(() => page.evaluate("lastRendered===0.875"), "final value without further input");
  const times = await page.evaluate("frameTimes");
  assert.ok(times.length > 1);
  assert.ok(times.length <= Math.ceil((times.at(-1)-times[0])*60/1000)+2,
    `host exceeded its cadence: ${times.length} frames`);
  await page.evaluate("new Promise(resolve=>setTimeout(resolve,100))");
  assert.equal(await page.evaluate("frameTimes.length"), times.length, "clean host must stop drawing");
});

test("vsynced host keeps 60 Hz through display and callback jitter", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:840,
    // Hold every RAF, including the host's first draw, so its pacer starts on
    // the synthetic clock instead of inheriting an uncontrolled real-time phase.
    source:`window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail));
      window.testFrames=new Map();window.testFrameId=0;
      window.requestAnimationFrame=callback=>{
        const id=++testFrameId;testFrames.set(id,callback);return id;
      };
      window.cancelAnimationFrame=id=>testFrames.delete(id);`});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  await page.evaluate(`_onHostMessage({type:'state',state:{path:'jitter.onda',
    supportsScope:false,supportsTransport:false,viewState:{readyId:98},
    params:[{name:'gain',type:'f64',value:0.5,rangeMin:0,rangeMax:1}]}})`);
  const result = await page.evaluate(`(async()=>{
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const render=CanvasUi.prototype.render,message=CanvasUi.prototype.message;
    let draws=0,applied,lastRendered;
    CanvasUi.prototype.message=function(json){
      const data=JSON.parse(json);
      if(data.type==='parameterValues')applied=data.state.params?.[0]?.value;
      return message.call(this,json);
    };
    CanvasUi.prototype.render=function(){
      const result=render.call(this);
      if(result){draws++;lastRendered=applied}
      return result;
    };
    // Drive the actual host and Wasm renderer with independent display and
    // execution clocks, including input phases that can miss a late RAF timer.
    let base=performance.now()+1000;
    let now=base-5,id=testFrameId,maxPending=0;
    const timers=new Map(),frames=testFrames;
    performance.now=()=>now;
    window.setTimeout=(callback,delay=0)=>{
      timers.set(++id,{callback,due:now+delay});return id;
    };
    window.clearTimeout=id=>timers.delete(id);
    window.requestAnimationFrame=callback=>(frames.set(++id,callback),id);
    window.cancelAnimationFrame=id=>frames.delete(id);
    function advance(time){
      for(;;){
        const next=[...timers].filter(([,timer])=>timer.due<=time)
          .sort((a,b)=>a[1].due-b[1].due)[0];
        if(!next)break;
        now=next[1].due;timers.delete(next[0]);next[1].callback();
      }
      now=time;
    }
    const results=[];
    for(const [jitter,lags,inputOffset] of [
      [0.1,[0.75,0.25],4], [0.1,[0.05,0.05],12.6], [0.5,[0.75,0.75],12.6],
    ]){
      draws=0;lastRendered=undefined;maxPending=0;
      advance(base-5);
      const value=value=>_onHostMessage({type:'parameterValues',state:{params:[{name:'gain',value}]}});
      value(1/120);
      for(let tick=0;tick<120;tick++){
        const timestamp=base+tick*1000/60+(tick%2===0?jitter:-jitter);
        advance(timestamp+lags[tick%2]);
        maxPending=Math.max(maxPending,frames.size);
        const callbacks=[...frames.values()];frames.clear();
        for(const callback of callbacks)callback(timestamp);
        if(tick<119){advance(timestamp+inputOffset);value((tick+2)/120)}
      }
      results.push({draws,lastRendered,maxPending,timers:timers.size,frames:frames.size});
      base+=5000;
    }
    return results;
  })()`);
  assert.deepEqual(result, Array(3).fill({draws:120,lastRendered:1,maxPending:1,timers:0,frames:0}));
  assert.equal(await page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===98)"), true);
});

test("GPU rejects malformed scope packets without losing the frame or later updates", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:840,
    source:"window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  await page.evaluate(`_onHostMessage({type:'state',state:{path:'packets.onda',running:true,
    viewState:{scopePinned:true,readyId:1}}});
    _onHostMessage({type:'scopeData',packet:new Uint8Array([79,83,81,49,1,0,1,0,0,192,0,64])});
    new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
  const frame = (await page.call("Page.captureScreenshot")).data;
  for (const packet of ["new Uint8Array([0,1,2])", "'!!!'", "'T1NRMQEAAQA='"]) {
    await page.evaluate(`_onHostMessage({type:'scopeData',packet:${packet}});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await page.evaluate("document.querySelector('#status').hidden"), true,
      "an invalid packet must not fail the renderer");
    assert.equal((await page.call("Page.captureScreenshot")).data, frame,
      "an invalid packet must retain the displayed frame");
  }
  await page.evaluate(`_onHostMessage({type:'state',state:{status:'Still working',viewState:{readyId:2}}})`);
  await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===2)"), "later state rendering");
  assert.equal(await page.evaluate("commands.some(c=>c.type==='runViewError')"), false);
});

test("combined visualization frames render the latest waveform and retain coalesced meter peaks", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:840,
    source:"window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  const result = await page.evaluate(`(async()=>{
    _onHostMessage({type:'state',state:{path:'combined.onda',running:true,outputChannels:2}});
    await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
    const {CanvasUi}=await import('./pkg/onda_ui.js');
    const frames=[], meters=[];
    const nativeScope=CanvasUi.prototype.scope_packet, nativeMessage=CanvasUi.prototype.message;
    CanvasUi.prototype.scope_packet=function(bytes){frames.push(Array.from(bytes));return nativeScope.call(this,bytes)};
    CanvasUi.prototype.message=function(value){const m=JSON.parse(value);if(m.type==='meterData')meters.push(m);return nativeMessage.call(this,value)};
    const packet=new Uint8Array([79,83,81,49,2,0,1,0,0,0,0,0,0,0,0,0]);
    _onHostMessage({type:'scopeData',channels:2,peaks:[1.2,0.2],packet:packet.slice()});
    new DataView(packet.buffer).setInt16(10,1000,true);
    _onHostMessage({type:'scopeData',channels:2,peaks:[0.1,0.7],packet});
    await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
    return {frames,meters};
  })()`);
  assert.equal(result.frames.length, 1);
  assert.equal(new DataView(Uint8Array.from(result.frames[0]).buffer).getInt16(10,true), 1000);
  assert.deepEqual(result.meters, [{type:"meterData",channels:2,peaks:[1.2,0.7]}]);
});

for (const backend of ["webgl", "webgpu"]) {
  test(`GPU device loss stops capture and exposes an error (${backend})`, {
    timeout:60000,
    skip:backend === "webgpu" && process.env.ONDA_UI_TEST_WEBGPU === "0",
  }, async t => {
    const origin = await serveGpuUi(t);
    const chrome = await openChrome(t, JSON.parse(process.env.ONDA_UI_CHROME_ARGS ||
      (backend === "webgpu" && process.platform === "linux"
        ? '["--use-angle=vulkan","--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan","--disable-vulkan-surface"]'
        : '["--use-angle=swiftshader"]')));
    const page = await chrome.page({width:720,height:840,source:`
      window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail));
      if (typeof GPUAdapter !== 'undefined') {
        const requestDevice=GPUAdapter.prototype.requestDevice;
        GPUAdapter.prototype.requestDevice=async function(...args) {
          const device=await requestDevice.apply(this,args);
          window.testDevice=device;return device;
        };
      }`});
    if (backend === "webgpu") {
      await page.navigate(origin + "/away");
      await eventually(() => page.evaluate(`(async()=>{
        const adapter=await navigator.gpu?.requestAdapter();
        return Boolean(adapter) && adapter.info?.isFallbackAdapter!==true;
      })()`), "hardware GPU adapter");
    }
    await page.navigate(origin + "/index.html" + (backend === "webgl" ? "?backend=webgl" : ""));
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
    assert.equal(await page.evaluate("document.querySelector('canvas').dataset.backend"), backend);
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'loss.onda',running:true,
      midi:{noteOn:true,noteOff:true},viewState:{readyId:1}}})`);
    await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===1)"), "state rendering");
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='scopeConfig').visible"), true,
      "a fresh source starts with the scope pinned");
    await page.evaluate("document.querySelector('canvas').focus()");
    await page.call("Input.dispatchKeyEvent", {type:"keyDown",key:"a",code:"KeyA"});
    assert.equal(await page.evaluate("commands.some(c=>c.type==='midiNote'&&c.pressed)"), true);
    await page.evaluate(backend === "webgpu" ? "testDevice.destroy()" :
      "document.querySelector('canvas').getContext('webgl2').getExtension('WEBGL_lose_context').loseContext()");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewError')"), "GPU loss feedback", 3000);
    assert.equal(await page.evaluate("document.querySelector('#status').hidden"), false);
    assert.match(await page.evaluate("document.querySelector('#status').textContent"), /lost|destroyed/i);
    assert.deepEqual(await page.evaluate(`(()=>{
      const config=commands.findLast(c=>c.type==='scopeConfig');
      return [config.visible,config.meter];
    })()`), [false,false]);
    assert.equal(await page.evaluate("commands.some(c=>c.type==='midiNote'&&!c.pressed)"), true,
      "GPU loss must release held notes without waiting for another input event");
    await page.evaluate(`_onHostMessage({type:'state',state:{viewState:{readyId:2}}});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===2)"), false,
      "a failed renderer must not acknowledge another frame");
    await page.call("Page.reload", {ignoreCache:true});
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU recovery after reloading");
    assert.equal(await page.evaluate("document.querySelector('#status').hidden"), true);
    // A page in the back/forward cache still owns its renderer. Loss while
    // suspended must survive restoration instead of silently disabling the UI.
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'cached-loss.onda',running:true}});
      dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));`);
    await page.evaluate(backend === "webgpu" ? "testDevice.destroy()" :
      "document.querySelector('canvas').getContext('webgl2').getExtension('WEBGL_lose_context').loseContext()");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewError')"), "cached GPU loss feedback", 3000);
    await page.evaluate(`dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));
      _onHostMessage({type:'state',state:{viewState:{readyId:3}}});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await page.evaluate("commands.some(c=>c.type==='runViewReady'&&c.readyId===3)"), false);
    assert.equal(await page.evaluate("document.querySelector('#status').hidden"), false);
  });
}

test("GPU capture pauses offscreen and resumes after scrolling or resizing", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:400});
  await page.call("Page.addScriptToEvaluateOnNewDocument", {
    source: "window.commands=[];addEventListener('ondaCommand',event=>commands.push(event.detail))",
  });
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU renderer");
  await page.evaluate(`_onHostMessage({type:"state",state:{
    path:"capture.onda",running:true,supportsViewState:true,
    params:Array.from({length:16},(_,i)=>({name:"p"+i,type:"f64",value:0.5})),
    viewState:{scopePinned:true}
  }})`);
  const capture = (visible, meter) => eventually(() => page.evaluate(`(()=>{
    const config=commands.findLast(c=>c.type==='scopeConfig');
    return config?.visible===${visible} && config.meter===${meter};
  })()`), `capture visibility: waveform=${visible}, meter=${meter}`);
  const wheel = async deltaY => {
    await page.call("Input.dispatchMouseEvent", {type:"mouseMoved",x:2,y:380});
    await page.call("Input.dispatchMouseEvent", {type:"mouseWheel",x:2,y:380,deltaX:0,deltaY});
  };
  await capture(true, true);
  // Rendering measures diagnostic text and can move the scope below the viewport.
  await page.evaluate(`_onHostMessage({type:"state",state:{
    error:Array(30).fill("A diagnostic line").join("\\n")
  }})`);
  await capture(false, true);
  await page.evaluate('_onHostMessage({type:"state",state:{error:null}})');
  await capture(true, true);
  await wheel(80);
  await capture(true, false);
  await wheel(1000);
  await capture(false, false);
  await wheel(-10000);
  await capture(true, true);
  await wheel(1000);
  await capture(false, false);
  await page.call("Emulation.setDeviceMetricsOverride", {
    width:720,height:2000,deviceScaleFactor:1,mobile:false,
  });
  await capture(true, true);
});

test("GPU wheel scrolling chains to the page when the canvas cannot scroll", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:1200,height:800});
  await page.navigate(origin + "/scroll-host");
  await eventually(() => page.evaluate("globalThis.commands?.some(c=>c.type==='uiReady')"), "embedded renderer");
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
  assert.equal(await page.evaluate("commands.some(c=>c.type==='setParam')"),false,"scrolling an unedited field must not publish a value");
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

test("GPU touch pans chain unused distance to the page", { timeout:60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:400,height:800});
  await page.call("Emulation.setDeviceMetricsOverride", {width:400,height:800,deviceScaleFactor:1,mobile:true});
  await page.call("Emulation.setTouchEmulationEnabled", {enabled:true,maxTouchPoints:2});
  await page.navigate(origin + "/touch-host");
  await eventually(() => page.evaluate("globalThis.commands?.some(c=>c.type==='uiReady')"), "embedded touch renderer");
  let readyId = 0;
  const state = async update => {
    await page.evaluate(`document.querySelector('iframe').contentWindow._onHostMessage({type:'state',state:${JSON.stringify({
      path:"touch.onda",supportsScope:false,supportsTransport:false,supportsViewState:true,
      params:[],...update,viewState:{reset:true,readyId:++readyId},
    })}})`);
    await eventually(() => page.evaluate(`commands.some(c=>c.type==='runViewReady'&&c.readyId===${readyId})`), "rendered touch state");
  };
  const swipe = async (x, from, to) => {
    await page.call("Input.dispatchTouchEvent", {type:"touchStart",touchPoints:[{id:1,x,y:from}]});
    for (let step = 1; step <= 10; step++)
      await page.call("Input.dispatchTouchEvent", {type:"touchMove",touchPoints:[{id:1,x,y:from+(to-from)*step/10}]});
    await page.call("Input.dispatchTouchEvent", {type:"touchEnd",touchPoints:[]});
  };
  await state({});
  await swipe(180, 650, 350);
  await eventually(() => page.evaluate("scrollY>250"), "full page pan through a fitting canvas", 3000);

  await page.evaluate("scrollTo(0,0)");
  await state({params:Array.from({length:80},(_,i)=>({name:"p"+i,type:"f64",value:0}))});
  await swipe(8, 650, 350);
  await eventually(() => page.evaluate("commands.findLast(c=>c.type==='viewState')?.state.scrollTop>250"), "internal touch pan");
  assert.equal(await page.evaluate("scrollY"), 0, "the inner document consumes available distance");
  await page.evaluate("document.querySelector('iframe').contentDocument.querySelector('canvas').dispatchEvent(new WheelEvent('wheel',{deltaY:100000,clientX:8,clientY:400,cancelable:true}))");
  await eventually(() => page.evaluate("commands.findLast(c=>c.type==='viewState')?.state.scrollTop>1000"), "inner document bottom");
  await swipe(8, 650, 350);
  await eventually(() => page.evaluate("scrollY>250"), "page pan at the canvas bottom", 3000);

  await page.evaluate("scrollTo(0,300)");
  await state({});
  await swipe(180, 150, 350);
  await eventually(() => page.evaluate("scrollY<150"), "page pan at the canvas top", 3000);

  await page.evaluate("scrollTo(0,0);document.querySelector('iframe').style.zoom='1.5'");
  await state({});
  await swipe(180, 650, 350);
  await eventually(() => page.evaluate("scrollY>250&&scrollY<350"), "page pan through a scaled iframe", 3000);

  await page.evaluate(`(()=>{
    scrollTo(0,0);
    const frame=document.querySelector('iframe'),container=document.createElement('div');
    container.id='scroller';
    container.style.cssText='height:680px;width:360px;overflow:auto;margin-top:100px';
    frame.before(container);
    frame.style.zoom='';frame.style.marginTop='0';
    container.append(frame);
    const spacer=document.createElement('div');spacer.style.height='100px';container.append(spacer);
    document.documentElement.style.scrollBehavior='smooth';
  })()`);
  await eventually(() => page.evaluate("commands.filter(c=>c.type==='uiReady').length>1"), "reparented iframe renderer");
  await state({});
  await swipe(180, 650, 590);
  await eventually(() => page.evaluate("document.getElementById('scroller').scrollTop>40"), "nearest scrollable ancestor", 3000);
  assert.equal(await page.evaluate("scrollY"), 0);
  await page.evaluate("document.getElementById('scroller').scrollTop=50");
  await swipe(180, 650, 350);
  await eventually(() => page.evaluate("scrollY>200"), "remaining pan reaches the page in the same gesture", 3000);
  assert.equal(await page.evaluate("document.getElementById('scroller').scrollTop"), 100);

  await page.evaluate("scrollTo({top:0,behavior:'instant'});document.getElementById('scroller').style.overscrollBehaviorY='contain'");
  await swipe(180, 600, 400);
  assert.equal(await page.evaluate("scrollY"), 0, "respect the containing element's overscroll policy");

  await page.evaluate("document.getElementById('scroller').lastElementChild.remove()");
  await swipe(180, 600, 400);
  assert.equal(await page.evaluate("scrollY"), 0, "containment also applies when the ancestor's content fits");
});

test("diagnostic reflow resolves and publishes hidden drafts without further input", { timeout: 60000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({ width: 720, height: 400,
    source: "window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))" });
  await page.navigate(`${origin}/index.html?backend=webgl`);
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  await page.evaluate(`_onHostMessage({type:'state',state:{
    path:'diagnostic.onda',supportsScope:false,supportsTransport:false,
    error:'compile error word '.repeat(18),
    params:[{name:'gain',type:'f64',value:0}]
  }})`);
  await eventually(() => page.evaluate("commands.some(c=>c.type==='runViewReady')"), "diagnostic layout");
  for (let i = 0; i < 12; i++) {
    await page.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab" });
    if (await page.evaluate("document.activeElement.tagName==='TEXTAREA'")) break;
  }
  assert.equal(await page.evaluate("document.activeElement.tagName"), "TEXTAREA");
  await page.call("Input.insertText", { text: "0.75" });
  assert.equal(await page.evaluate("document.activeElement.value"), "0.75");
  assert.equal(await page.evaluate("commands.some(c=>c.type==='setParam')"), false);

  await page.call("Emulation.setDeviceMetricsOverride", {
    width: 360, height: 400, deviceScaleFactor: 1, mobile: false,
  });
  await eventually(() => page.evaluate("commands.some(c=>c.type==='setParam')"), "hidden draft commit");
  assert.deepEqual(await page.evaluate("commands.filter(c=>c.type==='setParam')"), [
    { type: "setParam", name: "gain", value: 0.75, commit: true },
  ]);
  assert.equal(await page.evaluate("document.querySelector('.gpu-text-input').hidden"), true);
  assert.equal(await page.evaluate("document.activeElement.tagName"), "CANVAS");
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

test("failed GPU initialization releases queued messages and ignores updates until reload", { timeout: 30000 }, async t => {
  const wasm = Promise.withResolvers();
  t.after(() => wasm.resolve());
  const origin = await serveGpuUi(t, path => path.endsWith(".wasm") ? wasm.promise : undefined);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({ source: `
    window.commands = [];
    window.addEventListener("ondaCommand", event => commands.push(event.detail));
  ` });
  await page.navigate(`${origin}/index.html?backend=webgl`);
  await eventually(() => page.evaluate("typeof _onHostMessage === 'function'"), "initializing GPU receiver");
  await page.evaluate(`(async () => {
    const {CanvasUi} = await import('./pkg/onda_ui.js');
    CanvasUi.create = async () => { throw new Error('Test renderer initialization failure'); };
    window.messages = [];
    window.publishUpdates = () => {
      for (const message of [
        {type:'state', state:{path:'failed.onda', logText:'x'.repeat(65536)}},
        {type:'parameterValues', state:{params:[{name:'frequency', value:880}]}},
        {type:'scopeData', channels:1, samples:[0.5]},
        {type:'meterData', channels:1, peaks:[0.5]},
        {type:'midiActivity', activeNotes:[60]},
        {type:'bufferFileDrag', phase:'leave'},
        {type:'sourceFileDrag', phase:'leave'},
      ]) {
        messages.push(new WeakRef(message));
        _onHostMessage(message);
      }
    };
    publishUpdates();
  })()`);
  wasm.resolve();
  await eventually(() => page.evaluate("commands.some(c => c.type === 'runViewError')"), "renderer initialization failure");
  await page.evaluate(`
    for (let i = 0; i < 100; i++) publishUpdates();
    _onHostMessage({type:'requestRunViewReady'});
  `);
  assert.equal(await page.evaluate("commands.filter(c => c.type === 'runViewError').length"), 2,
    "readiness requests must replay the initialization error");
  assert.equal(await page.evaluate("commands.some(c => c.type === 'uiReady')"), false);
  await page.call("HeapProfiler.collectGarbage");
  assert.equal(await page.evaluate("messages.filter(message => message.deref()).length"), 0,
    "failed initialization must release existing publications and retain no new updates");
  assert.match(await page.evaluate("document.getElementById('status').textContent"), /Test renderer initialization failure/);

  await page.call("Page.reload", {ignoreCache:true});
  await eventually(() => page.evaluate("commands.some(c => c.type === 'uiReady')"), "renderer recovery after reload");
  assert.equal(await page.evaluate("document.getElementById('status').hidden"), true);
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
    await eventually(() => page.evaluate("globalThis.commands?.some(c=>c.type==='uiReady')"), "GPU renderer");
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
      await page.call("Input.dispatchMouseEvent", { type, x: 610, y: 100, button: "left", clickCount: 1 });
    await eventually(() => page.evaluate("clipboardErrors.length || commands.some(c=>c.type==='runViewError')"), "clipboard error feedback");
    assert.equal(await page.evaluate("document.getElementById('status').hidden"), true,
      "a clipboard error must not mark the working renderer as failed");
    assert.match(await page.evaluate("clipboardErrors.at(-1)"), /Could not copy clipboard text:/);
    assert.equal(await page.evaluate("commands.some(c=>c.type==='runViewError')"), false);

    await page.evaluate("commands=[];_onHostMessage({type:'requestRunViewReady'})");
    assert.equal(await page.evaluate("globalThis.commands?.some(c=>c.type==='uiReady')"), true);
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
  await eventually(() => page.evaluate("globalThis.commands?.some(c=>c.type==='uiReady')"), "GPU renderer", 10000);
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
  assert.equal(await page.evaluate("document.elementFromPoint(624,48).id"), "onda-ui");
  for (const type of ["mousePressed", "mouseReleased"])
    await page.call("Input.dispatchMouseEvent", {type, x:624, y:48, button:"left", clickCount:1});
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

test("automatic backend selection uses WebGL for software adapters", async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({ source: `
    Object.defineProperty(navigator, "gpu", { value: {
      requestAdapter: async () => ({ info: { isFallbackAdapter: true } }),
    } });
    window.commands = [];
    window.addEventListener("ondaCommand", event => commands.push(event.detail));
  ` });
  await page.navigate(`${origin}/index.html`);
  await eventually(() => page.evaluate("commands.some(c => c.type === 'uiReady')"), "automatic WebGL fallback");
  assert.equal(await page.evaluate("document.querySelector('canvas').dataset.backend"), "webgl");
});

for (const startup of ["dark", "light"]) {
  test(`theme detects ${startup} once, toggles binary choices, and permits page-owned appearance`, { timeout:30000 }, async t => {
    const origin = await serveGpuUi(t);
    const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
    const page = await chrome.page({width:720, height:840, source:`
      window.commands=[];
      window.acquireVsCodeApi=()=>({postMessage:m=>commands.push(m),getState:()=>null,setState(){}});
    `});
    const opposite = startup === "dark" ? "light" : "dark";
    await page.call("Emulation.setEmulatedMedia", {features:[{name:"prefers-color-scheme",value:opposite}]});
    await page.navigate(origin + "/index.html?backend=webgl");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "theme renderer");
    await page.evaluate(`document.body.className='vscode-${startup}';
      _onHostMessage({type:'state',state:{path:'',themeMode:null,supportsSourceSelection:true}});`);
    await eventually(() => page.evaluate(`document.documentElement.dataset.theme==='${startup}'`), "detected startup theme");
    await page.evaluate(`document.body.className='vscode-${opposite}';
      _onHostMessage({type:'state',state:{themeMode:null}});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await page.evaluate("document.documentElement.dataset.theme"), startup, "later host changes do not replace the startup choice");
    for (const expected of [opposite, startup]) {
      for (const type of ["mousePressed", "mouseReleased"])
        await page.call("Input.dispatchMouseEvent", {type,x:682,y:38,button:"left",clickCount:1});
      await eventually(() => page.evaluate(`commands.findLast(c=>c.type==='setTheme')?.mode==='${expected}'`), "binary theme toggle");
      assert.equal(await page.evaluate("document.documentElement.dataset.theme"), expected);
      await page.evaluate("_onHostMessage({type:'state',state:{themeMode:null}})");
      assert.equal(await page.evaluate("document.documentElement.dataset.theme"), expected);
    }
    for (const expected of [opposite, startup]) {
      await page.evaluate(`_onHostMessage({type:'state',state:{themeMode:null,hostTheme:'${expected}',supportsThemeSelection:false}})`);
      assert.equal(await page.evaluate("document.documentElement.dataset.theme"), expected, "embedded editors follow the page");
    }
  });
}

// Real canvas, Wasm, text, shader pipelines, and pointer/keyboard events. CDP
// uses Node's WebSocket so this test needs only a Chromium executable.
for (const backend of ["webgpu", "webgl"])
  test(`GPU run screen (${backend})`, {
    timeout: 90000,
    skip: backend === "webgpu" && process.env.ONDA_UI_TEST_WEBGPU === "0"
      ? "requires a hardware WebGPU adapter" : false,
  }, async (t) => {
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
    if (backend === "webgpu") {
      await session.navigate(`${origin}/away`);
      await eventually(() => evaluate(`(async()=>{
        const adapter=await navigator.gpu?.requestAdapter();
        return Boolean(adapter) && adapter.info?.isFallbackAdapter!==true;
      })()`), "hardware GPU adapter");
    }
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
    assert.equal(await evaluate("document.querySelector('canvas').dataset.backend"), backend);
    const state = {
      type: "state",
      state: {
        path: "test.onda",
        viewState: {scopePinned:true},
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
    await click(180, 393);
    for (const key of "880")
      await page("Input.dispatchKeyEvent", { type: "keyDown", key, text: key });
    await page("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter" });
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParam').value`),
      880,
    );
    await click(405, 393);
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
    _onHostMessage({type:'state',state:{viewState:{reset:true,scopePinned:true}}});
    new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    assert.equal(
      await evaluate("appliedScopes"),
      0,
      "a reset must discard scope data queued before it",
    );
    await click(682, 354);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParamLayout').layout`),
      "knobs",
    );
    await click(103, 438);
    await click(103, 438, 2);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='setParam'&&c.name==='frequency').value`),
      220,
      "real mouse double-click must reset the knob",
    );
    await evaluate(`_onHostMessage({type:'parameterValues',state:{params:[{name:'frequency',value:440}]}})`);
    await click(103, 486);
    assert.equal(await evaluate("document.activeElement.tagName"), "TEXTAREA");
    await click(103, 486, 2);
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
        if (editing) await click(103, 486);
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
    fullState.viewState = {scopePinned:true};
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
      state.viewState = {reset:true,scopePinned:true,sections:Object.fromEntries(
        ["scope","params","inputControls","buffers","events","log","midi"].map(key=>[key,key==="scope"||key==="midi"||key===name]))};
      // Reset establishes a new fixture; subsequent edits use trusted browser input.
      await evaluate(`_onHostMessage({type:"state",state:${JSON.stringify(state)}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    };
    await section("inputControls");
    await click(180, 393);
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
    await click(586, 391);
    assert.equal(
      await evaluate(`commands.findLast(c=>c.type==='chooseBufferFile').name`),
      "Piano",
    );
    await section("events");
    await click(647, 390);
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
    await click(610,354);
    await eventually(async()=>await evaluate("navigator.clipboard.readText()")===fullState.logText,
      "Copy exports the full log through the host clipboard adapter");
    await click(668, 354);
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
      {type:"midiNote",engineGeneration:"42",key:60,pressed:false,velocity:0},
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
    await click(80,100);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='scopeConfig').visible`),false);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='viewState').state.sections.scope`),false);
    await click(80,100);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='scopeConfig').visible`),true);
    for (const themeMode of ["light","dark"]) {
      await evaluate(`_onHostMessage({type:'state',state:{themeMode:'${themeMode}'}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
      assert.equal(await evaluate(`document.getElementById('status').hidden`),true);
    }
    for (const [themeMode, hostTheme, expected] of [[null, "light", "dark"], ["light", "dark", "light"]]) {
      await evaluate(`_onHostMessage({type:'state',state:{themeMode:${JSON.stringify(themeMode)},hostTheme:'${hostTheme}'}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
      assert.equal(await evaluate("document.documentElement.dataset.theme"), expected);
    }
    // Real pointer input must scrub numbers and update hover without a held button.
    await section("params");
    await page("Input.dispatchMouseEvent",{type:"mousePressed",x:180,y:393,button:"left",clickCount:1});
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:255,y:393,button:"left",buttons:1});
    await page("Input.dispatchMouseEvent",{type:"mouseReleased",x:255,y:393,button:"left",clickCount:1});
    assert.ok(await evaluate(`commands.findLast(c=>c.type==='setParam').value>0`));
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='setParam').commit`),true);
    await click(180,393);
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"-",text:"-"});
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter"});
    await click(562,40);
    assert.equal(await evaluate(`commands.findLast(c=>c.type==='stop')?.type`),"stop");
    await click(682,354);
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:102,y:433,buttons:0});
    assert.equal(await evaluate(`document.querySelector('canvas').style.cursor`),"ns-resize");
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:102,y:491,buttons:0});
    assert.equal(await evaluate(`document.querySelector('canvas').style.cursor`),"ew-resize");
    await click(102,491);
    assert.equal(await evaluate(`getComputedStyle(document.activeElement).cursor`),"text");
    await page("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
    await section("log");
    await click(180,40);
    await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:100,y:438,buttons:0});
    await page("Input.dispatchMouseEvent",{type:"mouseWheel",x:100,y:438,deltaX:0,deltaY:-120});
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
      354,
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

test("VS Code retains scope pinning, folds, scroll, and typed event values across webview reloads", { timeout: 60000 }, async t => {
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
  await publish({...state,viewState:{scopePinned:true}});
  for (let i = 0; i < 12; i++) {
    await session.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab" });
    if (await session.evaluate("document.activeElement.tagName === 'TEXTAREA'")) break;
  }
  assert.equal(await session.evaluate("document.activeElement.value"), "1");
  await session.call("Input.insertText", { text: "9223372036854775807" });
  await session.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter" });
  assert.equal(await session.evaluate("saved?.eventValues[0].values[0]"), "9223372036854775807");
  for (const type of ["mousePressed", "mouseReleased"])
    await session.call("Input.dispatchMouseEvent", { type, x: 60, y: 100, button: "left", clickCount: 1 });
  assert.equal(await session.evaluate("saved.sections.events"), false);
  await wheel(260);
  await eventually(() => session.evaluate("saved.scrollTop > 0"), "saved VS Code scroll");
  const saved = await session.evaluate("saved");
  assert.equal(saved.scopePinned, true);

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
  assert.equal(restored.scopePinned, true);
  assert.equal(restored.scrollTop, saved.scrollTop + 1);
  assert.deepEqual(restored.sections, saved.sections);
  assert.deepEqual(restored.eventValues, saved.eventValues);
  assert.equal(await session.evaluate("document.getElementById('status').hidden"), true);
});

// Real touch input exercises the compact header, internal overlay, and overflow.
test("compact audio header on mobile portrait, landscape, and zoomed views", {timeout:60000}, async t => {
  const origin=await serveGpuUi(t);
  const chrome=await openChrome(t,["--use-angle=swiftshader"]);
  const page=await chrome.page({width:390,height:844,source:`
    window.commands=[];window.errors=[];
    addEventListener('ondaCommand',e=>commands.push(e.detail));
    addEventListener('error',e=>errors.push(e.message));
  `});
  await page.navigate(origin+"/index.html?backend=webgl");
  await eventually(()=>page.evaluate("commands.some(c=>c.type==='uiReady')"),"mobile UI");
  const {mkdir}=await import("node:fs/promises");
  const directory=new URL("../../target/scope-mobile/",import.meta.url);
  await mkdir(directory,{recursive:true});
  const tap=async(x,y)=>{
    await page.call("Input.dispatchTouchEvent",{type:"touchStart",touchPoints:[{id:1,x,y}]});
    await page.call("Input.dispatchTouchEvent",{type:"touchEnd",touchPoints:[]});
    await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  };
  for(const [width,height,zoom,channels] of [[320,568,1,4],[360,800,1,2],[390,844,1,4],[430,932,1,8],[844,390,1,8],[390,844,2,4],[360,800,2.5,8],[1280,800,1,64],[360,800,1,64]]) {
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape"});
    const w=Math.round(width/zoom),h=Math.round(height/zoom);
    await page.call("Emulation.setDeviceMetricsOverride",{width:w,height:h,deviceScaleFactor:zoom,mobile:true});
    await page.call("Emulation.setTouchEmulationEnabled",{enabled:true,maxTouchPoints:1});
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'resonance.onda',running:true,status:'Running',
      outputChannels:${channels},supportsViewState:true,supportsSourceSelection:true,
      supportsDeviceSelection:true,supportsProjectExport:true,canExportProject:true,
      paramLayout:'knobs',params:[{name:'gain',type:'f64',value:0.65,rangeMin:0,rangeMax:1},
      {name:'frequency',type:'f64',value:440,rangeMin:20,rangeMax:20000}],viewState:{reset:true,scopePinned:false}}});
      _onHostMessage({type:'meterData',channels:${channels},peaks:[0.8,0.6,0.4,1.1]});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='scopeConfig').visible"),false);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='scopeConfig').meter"),true);
    const filename=`${width}x${height}-${zoom}x-${channels}ch`;
    const shot=async suffix=>writeFile(new URL(filename+suffix+".png",directory),Buffer.from((await page.call("Page.captureScreenshot",{format:"png"})).data,"base64"));
    await shot("-header");
    const header=w-28,minimal=header<182,dense=header<452,padding=minimal?4:10;
    const utility=dense?28:92,transport=dense?28:56,gap=dense?6:12;
    const sourceWidth=minimal?padding:header>=252?132:90;
    const available=header-padding-sourceWidth-8-gap-utility-transport-4;
    const fullMeterWidth=Math.max(32,(minimal?12:26)+channels*6);
    const meterWidth=channels>1&&fullMeterWidth>available?(minimal?32:46):fullMeterWidth;
    const mx=w-16-padding-utility-gap-transport-4-meterWidth/2;
    const cy=minimal?34:40;
    await tap(mx,cy);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='scopeConfig').visible"),true,filename);
    await page.evaluate(`_onHostMessage({type:'scopeData',channels:${channels},samples:Array.from({length:${channels}*128},(_,i)=>Math.sin(Math.floor(i/${channels})*.15)*.7)});
      new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    await shot("-overlay");
    if(channels>4) {
      await page.call("Input.dispatchTouchEvent",{type:"touchStart",touchPoints:[{id:1,x:w-60,y:210}]});
      await page.call("Input.dispatchTouchEvent",{type:"touchMove",touchPoints:[{id:1,x:w-60,y:130}]});
      await page.call("Input.dispatchTouchEvent",{type:"touchEnd",touchPoints:[]});
    }
    await tap(w-36,(minimal?62:74)+17);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='viewState').state.scopePinned"),true);
    await shot("-pinned");
    // Unpinning closes the scope and keeps the meter active.
    await tap(w-42,(minimal?68:80)+20);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='viewState').state.scopePinned"),false);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='scopeConfig').visible"),false);
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='scopeConfig').meter"),true);
    if(dense) {
      await tap(w-16-padding-14,cy);
      await shot("-options");
      await tap(w-60,(minimal?62:74)+18);
      assert.deepEqual(await page.evaluate("errors"),[]);
    }
    assert.equal(await page.evaluate("document.getElementById('status').hidden"),true);
  }
});

test("MIDI settings stay on one header row and remain editable through resizing and zoom", {timeout:60000}, async t => {
  const origin=await serveGpuUi(t);
  const chrome=await openChrome(t,["--use-angle=swiftshader"]);
  const page=await chrome.page({width:720,height:800,source:`
    window.commands=[];window.errors=[];
    addEventListener('ondaCommand',e=>commands.push(e.detail));
    addEventListener('error',e=>errors.push(e.message));
  `});
  await page.navigate(origin+"/index.html?backend=webgl");
  await eventually(()=>page.evaluate("commands.some(c=>c.type==='uiReady')"),"MIDI UI");
  const {mkdir}=await import("node:fs/promises");
  const directory=new URL("../../target/panel-responsive/",import.meta.url);
  await mkdir(directory,{recursive:true});
  const frame=()=>page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  const key=async(name,code=name,modifiers=0)=>{
    await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:name,code,modifiers});
    await frame();
  };
  const click=async(x,y)=>{
    for(const type of ["mousePressed","mouseReleased"])
      await page.call("Input.dispatchMouseEvent",{type,x,y,button:"left",clickCount:1});
    await frame();
  };
  for(const [width,scale] of [[720,1],[480,1],[440,1],[390,1],[360,1],[320,1],[240,1],[195,1],[128,2.5],[360,2]]) {
    await page.call("Emulation.setDeviceMetricsOverride",{width,height:800,deviceScaleFactor:scale,mobile:false});
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'midi-resize.onda',supportsScope:false,
      supportsViewState:true,midiKeyboardInteractive:true,midi:{noteOn:true},
      viewState:{reset:true},params:[],inputControls:[],events:[],buffers:[]}})`);
    await frame();
    const menu=width<240;
    const edit=()=>page.evaluate(`(()=>{const e=document.querySelector('.gpu-text-input');
      return {visible:!e.hidden,value:e.value,rect:e.getBoundingClientRect().toJSON()}})()`);
    if(menu) {
      await click(width-26,657);
      await key("Tab");
    } else {
      for(let stop=0;stop<8;stop++) {
        await key("Tab");
        const field=await edit();
        if(field.visible && field.value==="4") break;
      }
    }
    let field=await edit();
    assert.equal(field.visible,true,`${width}: octave remains reachable`);
    assert.equal(field.value,"4");
    if(!menu) assert.equal(field.rect.y,644,`${width}: octave stays in the header row`);
    await key("a","KeyA",2);
    await page.call("Input.insertText",{text:"5"});
    await key("Enter");
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='viewState').state.octave"),5);
    await key("Tab");
    const velocity=await edit();
    assert.equal(velocity.visible,true);
    assert.equal(Number(velocity.value),1);
    if(!menu) assert.equal(velocity.rect.y,field.rect.y,`${width}: velocity stays beside octave`);
    await key("a","KeyA",2);
    await page.call("Input.insertText",{text:"0.6"});
    await key("Enter");
    assert.equal(await page.evaluate("commands.findLast(c=>c.type==='viewState').state.velocity"),0.6);
    await key("Escape");
    const {data}=await page.call("Page.captureScreenshot",{format:"png"});
    await writeFile(new URL(`midi-${width}-${scale}x.png`,directory),Buffer.from(data,"base64"));
    assert.deepEqual(await page.evaluate("errors"),[]);
  }
});

test("bank toolbars stay compact and their menu keeps smoothing editable", {timeout:60000}, async t => {
  const origin=await serveGpuUi(t);
  const chrome=await openChrome(t,["--use-angle=swiftshader"]);
  const page=await chrome.page({width:840,height:800,source:`
    window.commands=[];window.errors=[];
    addEventListener('ondaCommand',e=>commands.push(e.detail));
    addEventListener('error',e=>errors.push(e.message));
  `});
  await page.navigate(origin+"/index.html?backend=webgl");
  await eventually(()=>page.evaluate("commands.some(c=>c.type==='uiReady')"),"bank UI");
  const {mkdir}=await import("node:fs/promises");
  const directory=new URL("../../target/bank-responsive/",import.meta.url);
  await mkdir(directory,{recursive:true});
  const frame=()=>page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  const tap=async(x,y)=>{
    await page.call("Input.dispatchTouchEvent",{type:"touchStart",touchPoints:[{id:1,x,y}]});
    await page.call("Input.dispatchTouchEvent",{type:"touchEnd",touchPoints:[]});
    await frame();
  };
  for(const [width,zoom] of [[840,1],[390,1],[320,1],[390,2],[320,2.5]]) {
    const w=Math.round(width/zoom),h=Math.round(800/zoom);
    await page.call("Emulation.setDeviceMetricsOverride",{width:w,height:h,deviceScaleFactor:zoom,mobile:true});
    await page.call("Emulation.setTouchEmulationEnabled",{enabled:true,maxTouchPoints:1});
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'banks.onda',running:false,supportsScope:false,
      supportsViewState:true,supportsParamSmoothing:true,paramSmoothingMs:30,
      paramLayout:'sliders',params:[{name:'gain',type:'f64',value:0.5,rangeMin:0,rangeMax:1}],viewState:{reset:true}}});`);
    await frame();
    const shot=async suffix=>writeFile(new URL(`${width}-${zoom}x-${suffix}.png`,directory),Buffer.from((await page.call("Page.captureScreenshot",{format:"png"})).data,"base64"));
    await shot("header");
    if(zoom>1) {
      const sectionY=w<210?68:80;
      await tap(w-39,sectionY+20);
      await shot("menu");
      // Smoothing is still a native text field inside the compact menu.
      await tap(w-60,sectionY+60);
      await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"a",code:"KeyA",modifiers:2});
      await page.call("Input.insertText",{text:"75"});
      await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter"});
      await eventually(()=>page.evaluate("commands.some(c=>c.type==='setParamSmoothing'&&c.milliseconds===75)"),"menu smoothing edit");
      await tap(w-50,sectionY+96);
      assert.equal(await page.evaluate("commands.findLast(c=>c.type==='setParamLayout')?.layout"),"knobs");
      await page.call("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape",code:"Escape"});
      await frame();
    }
    assert.deepEqual(await page.evaluate("errors"),[]);
  }
});

test("resizing a field into overflow finishes editing and releases keyboard MIDI", { timeout:30000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  for (const text of ["75", "invalid", "composing"]) {
    const page = await chrome.page({width:720,height:840,
      source:"window.commands=[];window.addEventListener('ondaCommand',e=>commands.push(e.detail))"});
    await page.navigate(origin + "/index.html?backend=webgl");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'resize.onda',running:true,
      supportsScope:false,supportsTransport:false,supportsDeviceSelection:false,
      supportsSourceSelection:false,supportsProjectExport:false,
      supportsParamSmoothing:true,paramSmoothingMs:30,
      midiKeyboardInteractive:true,midi:{noteOn:true},currentMidiInputDevice:'Computer Keyboard',
      params:[{name:'gain',type:'f64',value:0.5}]}});
      document.querySelector('canvas').focus();new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    const key = async (key, code=key, type="keyDown") => {
      await page.call("Input.dispatchKeyEvent", {type,key,code});
      await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    };
    let found = false;
    for (let i=0;i<12;i++) {
      await key("Tab");
      if (await page.evaluate("!document.querySelector('textarea').hidden && document.querySelector('textarea').value==='30'")) {
        found = true;
        break;
      }
    }
    assert.ok(found, "inline smoothing must be keyboard accessible");
    await page.call("Input.insertText", {text:text === "composing" ? "75" : text});
    if (text === "composing")
      await page.call("Input.imeSetComposition", {text:"9",selectionStart:1,selectionEnd:1});
    await page.call("Emulation.setDeviceMetricsOverride",{width:280,height:840,deviceScaleFactor:1,mobile:false});
    await eventually(() => page.evaluate("document.querySelector('textarea').hidden"), "editor entering overflow");
    if (text === "75") await eventually(() => page.evaluate(
      "commands.some(c=>c.type==='setParamSmoothing'&&c.milliseconds===75)"), "resize committing the visible draft",5000);
    await key("Enter");
    await key("a", "KeyA");
    await key("a", "KeyA", "keyUp");
    assert.equal(await page.evaluate("commands.some(c=>c.type==='midiNote'&&c.pressed)"),true);
    assert.deepEqual(await page.evaluate("commands.filter(c=>c.type==='setParamSmoothing').map(c=>c.milliseconds)"),
      text === "75" ? [75] : []);
    assert.equal(await page.evaluate("document.querySelector('textarea').hidden"),true);
  }
});

test("scrolling an edited field out of view releases keyboard MIDI", { timeout:30000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  for (const text of ["0.75", "invalid", "composing"]) {
    const page = await chrome.page({width:720,height:840,
      source:"window.commands=[];window.addEventListener('ondaCommand',e=>commands.push(e.detail))"});
    await page.navigate(origin + "/index.html?backend=webgl");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'scroll-edit.onda',running:true,
      supportsScope:false,supportsTransport:false,supportsDeviceSelection:false,
      midiKeyboardInteractive:true,midi:{noteOn:true},currentMidiInputDevice:'Computer Keyboard',
      params:Array.from({length:80},(_,i)=>({name:'p'+i,type:'f64',value:0.5}))}});
      document.querySelector('canvas').focus();new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    const key = async (key, code=key, type="keyDown") => {
      await page.call("Input.dispatchKeyEvent", {type,key,code});
      await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    };
    let found = false;
    for (let i=0;i<12;i++) {
      await key("Tab");
      if (await page.evaluate("!document.querySelector('textarea').hidden && document.querySelector('textarea').value==='0.5'")) {
        found = true;
        break;
      }
    }
    assert.ok(found, "first parameter must be keyboard accessible");
    await page.call("Input.insertText", {text:text === "composing" ? "0.75" : text});
    if (text === "composing")
      await page.call("Input.imeSetComposition", {text:"9",selectionStart:1,selectionEnd:1});
    const {x,y} = await page.evaluate(`(()=>{
      const r=document.querySelector('textarea').getBoundingClientRect();
      return {x:r.x+r.width/2,y:r.y+r.height/2};
    })()`);
    await page.call("Input.dispatchMouseEvent", {type:"mouseWheel",x,y,deltaX:0,deltaY:100000});
    await eventually(() => page.evaluate("document.querySelector('textarea').hidden"), "scrolled-out editor");
    assert.equal(await page.evaluate("document.activeElement.tagName"), "CANVAS");
    await key("a", "KeyA");
    await key("a", "KeyA", "keyUp");
    assert.deepEqual(await page.evaluate("commands.filter(c=>c.type==='midiNote').map(c=>c.pressed)"), [true,false]);
    assert.deepEqual(await page.evaluate("commands.filter(c=>c.type==='setParam').map(c=>[c.name,c.value,c.commit])"),
      text === "0.75" ? [["p0",0.75,true]] : []);
    await page.call("Input.dispatchMouseEvent", {type:"mouseWheel",x:8,y:300,deltaX:0,deltaY:-100000});
    await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
    assert.equal(await page.evaluate("document.querySelector('textarea').hidden"), true, "returning must not reopen the draft");
    if (text === "composing") {
      for (let i=0;i<12;i++) {
        await key("Tab");
        if (await page.evaluate("document.activeElement.tagName==='TEXTAREA'")) break;
      }
      assert.equal(await page.evaluate("document.activeElement.tagName"), "TEXTAREA");
      await page.call("Input.insertText", {text:"0.25"});
      await key("Enter");
      assert.deepEqual(await page.evaluate("commands.filter(c=>c.type==='setParam').map(c=>[c.name,c.value,c.commit])"),
        [["p0",0.25,true]], "cancelling composition must not block the next edit");
    }
  }
});

test("browser composition finishes before a numeric edit can commit", { timeout:30000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  for (const finish of ["commit", "cancel", "blur"]) {
    const page = await chrome.page({width:720,height:840,
      source:"window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
    await page.navigate(origin + "/index.html?backend=webgl");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
    await page.evaluate(`_onHostMessage({type:'state',state:{path:'composition.onda',supportsScope:false,
      supportsTransport:false,supportsDeviceSelection:false,
      params:[{name:'gain',type:'f64',value:0.5}]}});document.querySelector('canvas').focus()`);
    for (let i=0;i<12;i++) {
      await page.call("Input.dispatchKeyEvent", {type:"keyDown",key:"Tab",code:"Tab"});
      if (await page.evaluate("document.activeElement.tagName==='TEXTAREA'")) break;
    }
    assert.equal(await page.evaluate("document.activeElement.tagName"), "TEXTAREA");
    await page.call("Input.insertText", {text:"0.75"});
    await page.call("Input.imeSetComposition", {text:"9",selectionStart:1,selectionEnd:1});
    assert.equal(await page.evaluate("document.activeElement.value"), "0.759");
    assert.equal(await page.evaluate("commands.some(c=>c.type==='setParam')"), false);
    if (finish === "blur") {
      await page.evaluate("dispatchEvent(new Event('blur'))");
      assert.equal(await page.evaluate("commands.some(c=>c.type==='setParam')"), false);
      assert.equal(await page.evaluate("document.querySelector('textarea').hidden"), true);
      continue;
    }
    if (finish === "commit") await page.call("Input.insertText", {text:"9"});
    else await page.call("Input.imeSetComposition", {text:"",selectionStart:0,selectionEnd:0});
    await page.call("Input.dispatchKeyEvent", {type:"keyDown",key:"Enter",code:"Enter"});
    assert.deepEqual(await page.evaluate("commands.filter(c=>c.type==='setParam').map(c=>[c.name,c.value,c.commit])"),
      [["gain",finish === "commit" ? 0.759 : 0.75,true]]);
    assert.equal(await page.evaluate("document.querySelector('textarea').hidden"), true);
  }
});

for (const host of ["browser", "VS Code"]) {
  test(`${host} event float edits preserve changes between signed zeros`, { timeout:30000 }, async t => {
    const origin = await serveGpuUi(t);
    const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
    const page = await chrome.page({width:720,height:840,
      source:host === "VS Code" ? `window.commands=[];
        window.saved=JSON.parse(sessionStorage.getItem('viewState') || 'null');
        window.acquireVsCodeApi=()=>({
          postMessage:message=>commands.push(JSON.parse(JSON.stringify(message))),
          getState:()=>saved,setState:state=>{
            saved=JSON.parse(JSON.stringify(state));
            sessionStorage.setItem('viewState',JSON.stringify(saved));
          },
        });` : "window.commands=[];addEventListener('ondaCommand',e=>commands.push(e.detail))"});
    await page.navigate(origin + "/index.html?backend=webgl");
    await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
    for (const type of ["f32", "f64"]) {
      const publish = () => page.evaluate(`_onHostMessage({type:'state',state:{path:'zero-${type}.onda',supportsScope:false,
        supportsTransport:false,supportsDeviceSelection:false,supportsSourceSelection:false,supportsProjectExport:false,
        events:[{name:'probe',args:[{name:'value',type:'${type}',value:0,default:0}]}]}});
        document.querySelector('canvas').focus()`);
      await publish();
      for (const text of ["-0", "0", "-0"]) {
        for (let i=0;i<15;i++) {
          await page.call("Input.dispatchKeyEvent", {type:"keyDown",key:"Tab",code:"Tab"});
          if (await page.evaluate("document.activeElement.tagName==='TEXTAREA'")) break;
        }
        assert.equal(await page.evaluate("document.activeElement.tagName"), "TEXTAREA");
        await page.call("Input.dispatchKeyEvent", {type:"keyDown",key:"a",code:"KeyA",modifiers:2});
        await page.call("Input.insertText", {text});
        await page.call("Input.dispatchKeyEvent", {type:"keyDown",key:"Enter",code:"Enter"});
        for (const event of ["mousePressed", "mouseReleased"])
          await page.call("Input.dispatchMouseEvent", {type:event,x:650,y:140,button:"left",clickCount:1});
        assert.equal(await page.evaluate("Object.is(Number(commands.findLast(c=>c.type==='triggerEvent')?.values[0]),-0)"),
          text === "-0", `${type} edit ${text} must preserve the zero sign through dispatch`);
        if (host === "VS Code") {
          await eventually(() => page.evaluate(`saved?.eventValues[0].values[0] === ${text === "-0" ? "'-0'" : "0"}`),
            `${type} edit ${text} must preserve the zero sign in saved state`);
        }
      }
      if (host === "VS Code") {
        const loaded = page.event("Page.loadEventFired");
        await page.call("Page.reload");
        await loaded;
        await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "reloaded GPU UI");
        await publish();
        for (const event of ["mousePressed", "mouseReleased"])
          await page.call("Input.dispatchMouseEvent", {type:event,x:650,y:140,button:"left",clickCount:1});
        assert.equal(await page.evaluate("commands.findLast(c=>c.type==='triggerEvent')?.values[0]"), "-0",
          `${type} edit must retain its zero sign after a webview reload`);
      }
    }
  });
}

test("structured event controls edit and dispatch typed values", { timeout:30000 }, async t => {
  const origin = await serveGpuUi(t);
  const chrome = await openChrome(t, ["--use-angle=swiftshader"]);
  const page = await chrome.page({width:720,height:840,
    source:"window.commands=[];window.addEventListener('ondaCommand',e=>commands.push(e.detail))"});
  await page.navigate(origin + "/index.html?backend=webgl");
  await eventually(() => page.evaluate("commands.some(c=>c.type==='uiReady')"), "GPU UI");
  const value = {id:"9223372036854775807", levels:[0.25,0.5,0.75]};
  await page.evaluate(`_onHostMessage({type:'state',state:{path:'structured.onda',supportsScope:false,
    supportsTransport:false,supportsDeviceSelection:false,supportsSourceSelection:false,
    supportsProjectExport:false,events:[{name:'configure',args:[{name:'voice',value:${JSON.stringify(value)},
      shape:{kind:'struct',fields:[{name:'id',ty:{kind:'scalar',encoding:'i64'}},
        {name:'levels',ty:{kind:'slice',element:{kind:'scalar',encoding:'f32'}}}]}}]}]}})`);
  await page.evaluate("document.querySelector('canvas').focus();new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  const key = async key => {
    await page.call("Input.dispatchKeyEvent", {type:"keyDown",key});
    await page.evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  };
  const editValue = async (before, after) => {
    let found = false;
    for (let i=0;i<20;i++) {
      await key("Tab");
      if (await page.evaluate(`(()=>{const e=document.querySelector('textarea');return !e.hidden && e.value===${JSON.stringify(before)}})()`)) {
        found = true;
        break;
      }
    }
    assert.ok(found, `typed field ${before} must be keyboard accessible`);
    await page.call("Input.insertText", {text:after});
    await key("Enter");
    assert.equal(await page.evaluate("document.querySelector('textarea').hidden"),true);
  };
  await editValue(value.id,"-9223372036854775808");
  await page.evaluate("_onHostMessage({type:'state',state:{status:'Running'}})");
  await editValue("0.25","Infinity");
  await page.call("Input.dispatchMouseEvent",{type:"mousePressed",x:650,y:140,button:"left",clickCount:1});
  await page.call("Input.dispatchMouseEvent",{type:"mouseReleased",x:650,y:140,button:"left",clickCount:1});
  assert.deepEqual(await page.evaluate("commands.findLast(c=>c.type==='triggerEvent')?.values"),
    [{id:"-9223372036854775808",levels:["Infinity",0.5,0.75]}]);
});
