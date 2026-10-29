// Production DOM page versus the production Wasm canvas, with the same
// viewport, 64 controls, and 1024 stereo samples. No synthetic GPU fence is
// compared against a browser main-thread timer; timings below are CPU only.
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root=resolve(dirname(fileURLToPath(import.meta.url)),"../..");
const allPanels=process.env.ONDA_UI_PANELS==="1";
const panelFixture=JSON.parse(await readFile(resolve(root,"ui/gpu/fixtures/panels.json"),"utf8"));
const panels=allPanels?["params","inputControls","buffers","events","log","midi","settings"]:["params"];
const rounds=allPanels?1:3;
const profile=await mkdtemp(join(tmpdir(),"onda-ui-compare-"));
let browser,socket;
const server=createServer(async(req,res)=>{
  try {
    const path=new URL(req.url,"http://localhost").pathname;
    const routes={"/old.html":"ui/run/run.html","/gpu.html":"ui/gpu/index.html","/host.js":"ui/gpu/host.js","/browser.js":"ui/gpu/browser.js","/protocol.js":"ui/gpu/protocol.js",
      "/text-input.js":"ui/gpu/text-input.js","/number-input.js":"ui/number-input.js","/param-control.js":"packages/onda_processor_abi/src/param-control.js","/buffers.js":"ui/run/buffers.js"};
    const source=routes[path] || (/^\/pkg\/(?:fonts\/)?[\w.-]+$/.test(path) ? `ui/gpu${path}` : null);
    if (!source) {res.writeHead(404).end();return;}
    let data=await readFile(resolve(root,source));
    if (path.endsWith(".html")) data=data.toString().replace("<head>",`<head><script>
      window.__hostBridge={mode:'wry',theme:'dark'};
      window.messages=[];window.ipc={postMessage(s){messages.push(JSON.parse(s));}};
      window.frameCpu=[];window.measure=false;
      const nativeRAF=window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame=callback=>nativeRAF(time=>{
        const start=performance.now();callback(time);
        if(window.measure&&!callback.benchmarkDriver) frameCpu.push(performance.now()-start);
      });
      const context=HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext=function(type,...args){this.dataset.context=type;return context.call(this,type,...args);};
    </script>`);
    res.setHeader("Content-Type",path.endsWith(".wasm")?"application/wasm":path.endsWith(".js")?"text/javascript":"text/html");res.end(data);
  } catch(error) {res.writeHead(500).end(String(error));}
});
await new Promise(r=>server.listen(0,"127.0.0.1",r));
const endpoint=Promise.withResolvers();let stderr="";
try {
  browser=spawn(process.env.CHROME_BIN||"chromium",["--headless","--no-sandbox","--disable-dev-shm-usage",
    "--enable-unsafe-webgpu","--ignore-gpu-blocklist",...JSON.parse(process.env.ONDA_UI_CHROME_ARGS||'["--use-angle=vulkan","--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan","--disable-vulkan-surface"]'),"--remote-debugging-port=0",`--user-data-dir=${profile}`,"about:blank"],
    {env:{...process.env,DISPLAY:process.env.DISPLAY||":1"},stdio:["ignore","ignore","pipe"]});
  browser.on("error",endpoint.reject);
  browser.stderr.on("data",chunk=>{stderr=(stderr+chunk).slice(-32000);const url=stderr.match(/DevTools listening on (ws:\/\/\S+)/)?.[1];if(url)endpoint.resolve(url);});
  const timeout=setTimeout(()=>endpoint.reject(new Error(stderr)),20000);
  let url;try {url=await endpoint.promise;}finally{clearTimeout(timeout);}
  socket=new WebSocket(url);await once(socket,"open");
  let id=0;const pending=new Map();
  socket.addEventListener("message",event=>{const m=JSON.parse(event.data);if(!m.id)return;const p=pending.get(m.id);pending.delete(m.id);if(m.error)p.reject(new Error(JSON.stringify(m.error)));else p.resolve(m.result);});
  const call=(method,params={},sessionId)=>{const task=Promise.withResolvers();const key=++id;pending.set(key,task);socket.send(JSON.stringify({id:key,method,params,sessionId}));return task.promise;};
  const system=await call("SystemInfo.getInfo");
  console.error(JSON.stringify({devices:system.gpu.devices,renderer:system.gpu.auxAttributes.gl_renderer,features:system.gpu.featureStatus}));
  const output={browser:await call("Browser.getVersion"),gpu:system.gpu,viewport:[768,840],dpr:1,metric_time_domain:"threadTicks",controls:allPanels?panelFixture.params.length:64,
    complete_panels:allPanels,fixture:allPanels?"ui/gpu/fixtures/panels.json":null,
    channels:2,samples_per_channel:1024,warmup_updates:60,measured_updates:180,results:[]};
  for(const panel of panels) for (const automate of [false,true]) for(let round=0;round<rounds;round++) for(const variant of round%2?["old","gpu"]:["gpu","old"]) {
    const {targetId}=await call("Target.createTarget",{url:"about:blank"});
    const {sessionId}=await call("Target.attachToTarget",{targetId,flatten:true});
    const page=(method,params={})=>call(method,params,sessionId);
    const evaluate=async expression=>{const r=await page("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value;};
    await page("Page.enable");await page("Performance.enable",{timeDomain:"threadTicks"});
    await page("Emulation.setDeviceMetricsOverride",{width:768,height:840,deviceScaleFactor:1,mobile:false});
    await page("Page.navigate",{url:`http://127.0.0.1:${server.address().port}/${variant==='old'?'old':'gpu'}.html`});
    await evaluate(`new Promise((resolve,reject)=>{const start=Date.now();const timer=setInterval(()=>{
      if(window.messages?.some(m=>m.type==='webviewReady')){clearInterval(timer);resolve(true);}
      else if(Date.now()-start>30000){clearInterval(timer);reject(new Error(document.getElementById('status')?.textContent||'Page not ready'));}
    },20);})`);
    await evaluate(`(() => {
      const params=Array.from({length:64},(_,i)=>({name:'Parameter '+(i+1),type:'f64',scalar:true,value:i/64,default:.5,rangeMin:0,rangeMax:1,scale:'linear'}));
      _onHostMessage({type:'state',state:{path:'electric.onda',status:'Playing',running:true,connected:true,outputChannels:2,
        params,paramLayout:'knobs',supportsScope:true,supportsTransport:true,supportsSourceSelection:false,
        supportsDeviceSelection:false,supportsRunSettings:false,supportsParamSmoothing:false}});
      if(${allPanels}) {
        const state=${JSON.stringify(panelFixture)};
        state.viewState={path:state.path,sections:Object.fromEntries(["scope","params","inputControls","buffers","events","log","midi"].map(p=>[p,p==="scope"||p===${JSON.stringify(panel)}]))};
        _onHostMessage({type:"state",state});
      }
      window.scopeFrames=Array.from({length:240},(_,f)=>({type:'scopeData',channels:2,
        samples:Array.from({length:2048},(_,i)=>Math.sin(Math.floor(i/2)*(i%2?.05:.03)+f*.02)*(i%2?.5:.7))}));
      if(${variant==='gpu'}) scopeFrames=scopeFrames.map(frame=>{
        const columns=messages.findLast(m=>m.type==='scopeConfig')?.columns||348;
        const bytes=new Uint8Array(8+2*columns*4),view=new DataView(bytes.buffer);
        bytes.set([79,83,81,49]);view.setUint16(4,2,true);view.setUint16(6,columns,true);
        for(let ch=0;ch<2;ch++)for(let col=0;col<columns;col++){
          let lo=32767,hi=-32767;
          for(let i=Math.floor(col*1024/columns);i<Math.floor((col+1)*1024/columns);i++){
            const q=Math.round(Math.max(-1,Math.min(1,frame.samples[i*2+ch]))*32767);lo=Math.min(lo,q);hi=Math.max(hi,q);
          }
          const offset=8+(ch*columns+col)*4;view.setInt16(offset,lo,true);view.setInt16(offset+2,hi,true);
        }
        return {type:'scopeData',packet:btoa(String.fromCharCode(...bytes))};
      });
      window.valueFrames=scopeFrames.map((_,f)=>({type:'parameterValues',state:{params:params.map((p,i)=>({name:p.name,value:((f+i)%100)/100}))}}));
      if(${allPanels}) valueFrames=scopeFrames.map((_,f)=>{
        const fixture=${JSON.stringify(panelFixture)};
        const panel=${JSON.stringify(panel)};
        if(["params","inputControls"].includes(panel)) return {type:"parameterValues",state:{[panel]:fixture[panel].map((p,i)=>({name:p.name,value:((f+i)%100)/100}))}};
        if(panel==="midi") return {type:"midiActivity",activeNotes:[60+f%24]};
        if(panel==="buffers") {for(const b of fixture.buffers)b.waveform.maximums=b.waveform.maximums.map((_,i)=>Math.abs(Math.sin((i+f)*.11)));return {type:"state",state:{buffers:fixture.buffers}};}
        if(panel==="events"){fixture.events[0].args[0].value=(f%100)/100;return {type:"state",state:{events:fixture.events,resetEventArguments:true}};}
        if(panel==="log") return {type:"state",state:{logText:fixture.logText+"Frame "+f+"\\n"}};
        return {type:"state",state:{paramSmoothingMs:f%100}};
      });
      window.runFrames=count=>new Promise(resolve=>{
        let i=0;const tick=()=>{const start=performance.now();
          if(${automate})_onHostMessage(valueFrames[i]);_onHostMessage(scopeFrames[i]);
          if(window.measure)window.handlerCpu.push(performance.now()-start);
          if(++i<count)requestAnimationFrame(tick);else {const done=()=>{const finish=()=>resolve();finish.benchmarkDriver=true;requestAnimationFrame(finish);};done.benchmarkDriver=true;requestAnimationFrame(done);}
        };tick.benchmarkDriver=true;requestAnimationFrame(tick);
      });
      return runFrames(60);
    })()`);
    if(allPanels && panel==="settings" && variant==="gpu") {
      for(const type of ["mousePressed","mouseReleased"])await page("Input.dispatchMouseEvent",{type,x:650,y:48,button:"left",clickCount:1});
      await evaluate("runFrames(60)");
    }
    const before=await page("Performance.getMetrics");
    await evaluate(`window.frameCpu=[];window.handlerCpu=[];window.measure=true;window.benchStart=performance.now();runFrames(180)`);
    const measured=await evaluate(`(() => {
      window.measure=false;
      const stats=values=>{values.sort((a,b)=>a-b);return {count:values.length,median_ms:values[Math.floor(values.length/2)],p95_ms:values[Math.floor(values.length*.95)],mean_ms:values.reduce((a,b)=>a+b,0)/values.length}};
      return {handler:stats(handlerCpu),render_callback:stats(frameCpu),wall_ms:performance.now()-benchStart,
        context:document.querySelector('canvas')?.dataset.context,scope_wire_bytes:JSON.stringify(scopeFrames[0]).length,
        error:${variant==='gpu'}&&document.getElementById('status')?.hidden===false?document.getElementById('status').textContent:null};
    })()`);
    if(measured.error)throw new Error(measured.error);
    const after=await page("Performance.getMetrics");
    const metrics=Object.fromEntries(after.metrics.map(({name,value})=>[name,value]));
    for(const {name,value} of before.metrics) if(["TaskDuration","ScriptDuration","LayoutDuration","RecalcStyleDuration"].includes(name))metrics[name]=(metrics[name]-value)*1000;
    const result={implementation:variant==='old'?'dom_canvas2d':'shared_wasm',panel,scenario:automate?(allPanels?'scope_and_panel_updates':'scope_and_64_values'):'scope_only',round,...measured,
      cpu_task_ms_per_update:metrics.TaskDuration/180,script_ms_per_update:metrics.ScriptDuration/180,
      layout_ms_per_update:metrics.LayoutDuration/180,recalc_ms_per_update:metrics.RecalcStyleDuration/180};
    output.results.push(result);console.error(JSON.stringify(result));
    await call("Target.closeTarget",{targetId});
  }
  const destination=process.argv[2]||resolve(root,"target/bench-ui-browser.json");
  await writeFile(destination,JSON.stringify(output,null,2)+"\n");console.log(`Wrote ${destination}`);
}catch(error) { console.error(stderr.slice(-12000)); throw error; }finally {
  socket?.close();if(browser&&browser.exitCode===null){browser.kill();await once(browser,"exit");}
  await new Promise(r=>{server.close(r);server.closeAllConnections();});
  await rm(profile,{recursive:true,force:true,maxRetries:5,retryDelay:100});
}
