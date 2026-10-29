import assert from "node:assert/strict";
import { eventually } from "../chrome-test-session.js";

export async function checkRegressions(page, evaluate) {
  await page("Emulation.setDeviceMetricsOverride", {width:720,height:840,deviceScaleFactor:1,mobile:false});
  const state = async update => evaluate(`_onHostMessage({type:"state",state:${JSON.stringify(update)}});new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
  await state({path:"hover-delay.onda",running:false,supportsScope:false,
    params:[],inputControls:[],buffers:[],events:[],logText:"",logRevealed:false,
    midi:{noteOn:false},midiKeyboardInteractive:false,viewState:{reset:true}});
  const screenshot = async () => (await page("Page.captureScreenshot")).data;
  await page("Input.dispatchMouseEvent", {type:"mouseMoved",x:700,y:800});
  const plain = await screenshot();
  await page("Input.dispatchMouseEvent", {type:"mouseMoved",x:100,y:35});
  await evaluate("new Promise(r=>setTimeout(r,100))");
  assert.equal(await screenshot(),plain,"hover popups must wait before appearing");
  await evaluate("new Promise(r=>setTimeout(r,500))");
  assert.notEqual(await screenshot(),plain,"an idle canvas must wake to show its tooltip");
  await page("Input.dispatchMouseEvent", {type:"mouseMoved",x:700,y:800});
  await evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  assert.equal(await screenshot(),plain,"leaving the label must dismiss its tooltip");
  const midi = {path:"midi-regressions.onda",running:true,connected:true,supportsScope:false,supportsTransport:true,
    buffers:[],events:[],params:[],inputControls:[],error:null,logText:"",logRevealed:false,
    midi:{noteOn:true,noteOff:true},midiKeyboardInteractive:true,currentMidiInputDevice:"Computer Keyboard",
    viewState:{reset:true}};
  await state(midi);
  await evaluate("document.querySelector('canvas').focus();commands=[]");
  const key = async (key,code,pressed=true,autoRepeat=false) => page("Input.dispatchKeyEvent", {
    type:pressed?"keyDown":"keyUp",key,code,autoRepeat,
  });
  await key("q","KeyA");await key("q","KeyA",false);
  assert.deepEqual(await evaluate("commands.filter(c=>c.type==='midiNote').map(c=>[c.key,c.pressed])"),[[60,true],[60,false]],"computer MIDI must follow physical AZERTY/QWERTZ positions");
  assert.equal(await evaluate("commands.find(c=>c.type==='midiNote'&&c.pressed).velocity"),1,"fresh MIDI keyboards must play at full velocity");
  await evaluate("commands=[]");
  await key("x","KeyX");await key("x","KeyX",true,true);await key("x","KeyX",false);
  await key("a","KeyA");await key("a","KeyA",false);
  assert.deepEqual(await evaluate("commands.filter(c=>c.type==='midiNote').map(c=>[c.key,c.pressed])"),[[72,true],[72,false]],"X must shift exactly one octave despite key repeat");
  await key("z","KeyZ");await key("z","KeyZ",false);
  await key("a","KeyA");await key("a","KeyA",false);
  assert.equal(await evaluate("commands.findLast(c=>c.type==='midiNote').key"),60);

  await state({...midi,path:"touch-chords.onda"});
  await evaluate("commands=[]");
  await page("Emulation.setTouchEmulationEnabled",{enabled:true,maxTouchPoints:3});
  const points=[{id:1,x:35,y:810},{id:2,x:135,y:810}];
  await page("Input.dispatchTouchEvent",{type:"touchStart",touchPoints:[points[0]]});
  await page("Input.dispatchTouchEvent",{type:"touchStart",touchPoints:points});
  assert.deepEqual(await evaluate("commands.filter(c=>c.type==='midiNote').map(c=>[c.key,c.pressed])"),[[60,true],[64,true]],"both fingers must reach the shared model");
  await page("Input.dispatchTouchEvent",{type:"touchEnd",touchPoints:[]});
  assert.deepEqual(await evaluate("commands.filter(c=>c.type==='midiNote'&&!c.pressed).map(c=>c.key).sort()"),[60,64]);
  await page("Emulation.setTouchEmulationEnabled",{enabled:false});

  await state({...midi,path:"世界.onda",midi:{noteOn:false},midiKeyboardInteractive:false,
    logText:"Chinese: 你好 世界\nArabic: مرحبا\nHebrew: שלום\nEmoji: 😀",logRevealed:true,
    params:[{name:"Frequency",type:"f64",value:440,default:220,rangeMin:20,rangeMax:20000,step:1,stepCount:19980},
      {name:"Gate",type:"bool",value:false},{name:"External",type:"f64",value:0,enabled:false}],
    events:[{name:"strike",args:[]}]});
  await eventually(()=>evaluate("performance.getEntriesByType('resource').filter(r=>/pkg\\/fonts\\/Noto/.test(r.name)).length>=4"),"portable Unicode fallback fonts");

  for (const scale of [2, 1]) {
    await page("Emulation.setDeviceMetricsOverride", {
      width: 720, height: 840, deviceScaleFactor: scale, mobile: false,
    });
    // CDP changes density without emitting the browser's display-change events.
    await evaluate("window.dispatchEvent(new Event('resize'))");
    await eventually(() => evaluate(`document.querySelector('canvas').width === ${720 * scale}`),
      `sharp canvas after changing display density to ${scale}x`);
  }

  await state({...midi,path:"compact-events.onda",midi:{noteOn:false},midiKeyboardInteractive:false,
    events:[{name:"play",args:[{name:"start_frame",type:"f32",default:0.13,value:0.13}]}]});
  await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:700,y:800});
  const restingTrigger = await screenshot();
  await evaluate("commands=[]");
  for (const type of ["mouseMoved","mousePressed","mouseReleased"])
    await page("Input.dispatchMouseEvent",{type,x:650,y:150,button:"left",clickCount:1});
  assert.equal(await evaluate("commands.filter(c=>c.type==='triggerEvent').length"),1);
  await page("Input.dispatchMouseEvent",{type:"mouseMoved",x:700,y:800});
  await evaluate("new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))");
  assert.equal(await screenshot(),restingTrigger,"clicking Trigger must not leave it highlighted after the pointer leaves");
  for (const type of ["mousePressed","mouseReleased"])
    await page("Input.dispatchMouseEvent",{type,x:629,y:188,button:"left",clickCount:1});
  const field = await evaluate(`(()=>{
    const e=document.activeElement,s=getComputedStyle(e);
    return {tag:e.tagName,value:e.value,height:e.getBoundingClientRect().height,
      line:parseFloat(s.lineHeight),available:e.clientHeight-parseFloat(s.paddingTop)-parseFloat(s.paddingBottom)};
  })()`);
  assert.equal(field.tag,"TEXTAREA");
  assert.equal(field.value,"0.13");
  assert.equal(field.height,24);
  assert.ok(field.line<=field.available,"compact editors must fit the complete text line");
  await key("Escape","Escape");

  await state({...midi,path:"smoothing-fonts.onda",midi:{noteOn:false},midiKeyboardInteractive:false,
    supportsParamSmoothing:true,paramSmoothingMs:1000,
    supportsInputControlSmoothing:true,inputControlSmoothingMs:177,
    params:[{name:"gain",type:"f64",value:0.5}],
    inputControls:[{name:"input",type:"f64",value:0.5}]});
  const smoothing = new Set();
  for(let i=0;i<32 && smoothing.size<2;i++) {
    await key("Tab","Tab");
    const edit=await evaluate(`(()=>{
      const e=document.activeElement,s=getComputedStyle(e);
      return {tag:e.tagName,value:e.value,size:s.fontSize,family:s.fontFamily};
    })()`);
    if(edit.tag==="TEXTAREA" && ["177","1000"].includes(edit.value)) {
      assert.equal(edit.size,"12px","smoothing editors must use the compact font size");
      assert.ok(edit.family.includes("Inter"),"browser editing must match the canvas font");
      smoothing.add(edit.value);
    }
  }
  assert.equal(smoothing.size,2,"both smoothing banks must retain their compact typography while editing");
  await key("Escape","Escape");
}
