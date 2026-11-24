import assert from "node:assert/strict";
import test from "node:test";
import binaryen from "binaryen";
import { compileTrustedMir as compileMir } from "../src/index.js";
import {
  assign,
  attributes,
  constant,
  executableMir,
  local,
  place,
  statement,
  value,
} from "../test-support/executable-mir.js";

test("restores process-global Binaryen optimization policy", () => {
  const previousFastMath = binaryen.getFastMath();
  const previousLoopInlining = binaryen.getAllowInliningFunctionsWithLoops();
  const previousSize = binaryen.getAlwaysInlineMaxSize();
  const previousPattern = binaryen.getPassArgument("no-inline");
  binaryen.setFastMath(true);
  binaryen.setAllowInliningFunctionsWithLoops(true);
  try {
    binaryen.setAlwaysInlineMaxSize(3);
    binaryen.setPassArgument("no-inline", "other-module");
    const artifact = compileMir(executableMir(), {
      fastMath: false,
      allowInliningFunctionsWithLoops: false,
    });
    assert.equal(artifact.metadata.optimization.fast_math, false);
    assert.equal(
      artifact.metadata.optimization.inline_functions_with_loops,
      false,
    );
    assert.equal(binaryen.getFastMath(), true);
    assert.equal(binaryen.getAllowInliningFunctionsWithLoops(), true);
    assert.equal(binaryen.getAlwaysInlineMaxSize(), 3);
    assert.equal(binaryen.getPassArgument("no-inline"), "other-module");
  } finally {
    binaryen.setFastMath(previousFastMath);
    binaryen.setAllowInliningFunctionsWithLoops(previousLoopInlining);
    binaryen.setAlwaysInlineMaxSize(previousSize);
    binaryen.setPassArgument("no-inline", previousPattern);
  }
});

test("MIR inline hints coexist across Wasm optimization levels", async () => {
  const mir = executableMir();
  const call = (fn) => statement("call", { function: fn, args: [], results: [] });
  const init = mir.functions[0];
  init.attributes.inline = "auto";
  init.body.statements = [call(2)];
  const applyIntrinsic = (intrinsic) => assign(place("local", 0),
    value("intrinsic", { intrinsic, args: [local(0)] }));
  mir.functions.push(
    {
      ...init, name: "wrapper", kind: { kind: "user" },
      attributes: attributes("compiler_generated", "always"),
      body: { statements: [call(3)] },
    },
    {
      ...init, name: "leaf", kind: { kind: "user" },
      attributes: attributes("source", "never"),
      locals: [{ name: "level", ty: 0 }],
      body: { statements: [
        assign(place("local", 0), value("load", place("param", 0))),
        applyIntrinsic("sin"),
        applyIntrinsic("round"),
        assign(place("state", 0), value("use", local(0))),
      ] },
    },
  );
  for (const options of [
    { optimize: false }, { optimizeLevel: 0 }, { optimizeLevel: 1, shrinkLevel: 1 },
    { optimizeLevel: 2 }, { optimizeLevel: 4 }, { optimizeLevel: 0, shrinkLevel: 2 },
  ]) {
    const { wat, wasm } = compileMir(mir, { ...options, emitText: true });
    const optimized = options.optimize !== false;
    const normalInliner = options.optimizeLevel >= 2 || options.shrinkLevel === 2;
    const hasCall = name => wat.includes(`(call $${name}\n`) || wat.includes(`(call $${name})`);
    const mode = JSON.stringify(options);
    assert.equal(hasCall("$onda.fn.1"), !optimized, mode); // 'always' process with a loop
    assert.equal(hasCall("$onda.fn.2"), !optimized, mode);
    assert.equal(hasCall("$onda.fn.3"), true, mode);
    assert.equal(hasCall("$onda.fn.0"), !normalInliner, mode);
    if (!normalInliner) {
      assert.equal(hasCall("onda_math_sin_f32"), true, mode);
      assert.equal(hasCall("$onda.math.round.f32"), true, mode);
    }
    assert.doesNotMatch(wat, /@binaryen\.inline/);
    const { instance } = await WebAssembly.instantiate(wasm);
    const { memory, __heap_base, onda_processor_init } = instance.exports;
    const params = Number(__heap_base.value);
    const state = params + 16;
    const view = new DataView(memory.buffer);
    view.setFloat32(params, 1, true);
    assert.equal(onda_processor_init(params, state, 1, 0, 0, 0, 0, 0), 0, mode);
    assert.equal(view.getFloat32(state, true), 1, mode);
  }
});

test("identical functions retain the inline policy of each call", async () => {
  for (const hints of [["always", "never"], ["never", "always"]]) {
    const mir = executableMir();
    const init = mir.functions[0];
    init.body.statements = [2, 3].map(id => statement("call", { results: [], function: id, args: [] }));
    for (const hint of hints) {
      mir.functions.push({
        ...init, name: hint, kind: { kind: "user" }, attributes: attributes("compiler_generated", hint),
        locals: [{ name: "counter", ty: 0 }],
        body: { statements: [
          assign(place("local", 0), value("load", place("state", 0))),
          assign(place("state", 0), value("binary", { op: "add", lhs: local(0), rhs: constant("f32", 1) })),
        ] },
      });
    }
    for (const options of [{ optimizeLevel: 1 }, { optimizeLevel: 2 }, { optimizeLevel: 4 }, { optimizeLevel: 0, shrinkLevel: 2 }]) {
      const { wat, wasm } = compileMir(mir, { ...options, emitText: true });
      const calls = [...wat.matchAll(/\(call \$\$onda\.fn\.([23])\b/g)].map(match => Number(match[1]));
      assert.deepEqual(calls, [2 + hints.indexOf("never")], JSON.stringify({ hints, options }));
      const { instance } = await WebAssembly.instantiate(wasm);
      const { memory, __heap_base, onda_processor_init } = instance.exports;
      const state = Number(__heap_base.value);
      assert.equal(onda_processor_init(0, state, 1, 0, 0, 0, 0, 0), 0);
      assert.equal(new DataView(memory.buffer).getFloat32(state, true), 2);
    }
  }
});

test("an always hint does not force identical heuristic callees with loops to inline", () => {
  const state = place("state", 0);
  const set = (id, expression) => assign(place("local", id), expression);
  const add = (lhs, rhs) => value("binary", { op: "add", lhs, rhs });
  const i32 = n => constant("i32", n);
  const loop = [
    set(0, value("use", i32(0))),
    set(2, value("load", state)),
    set(3, value("load", place("param", 0))),
    statement("loop", { body: { statements: [
      set(2, add(local(2), local(3))),
      set(0, add(local(0), i32(1))),
      set(1, value("compare", { op: "equal", lhs: local(0), rhs: i32(500) })),
      statement("if", {
        condition: local(1),
        then_block: { statements: [statement("break")] },
        else_block: { statements: [] },
      }),
    ] } }),
    assign(state, value("use", local(2))),
  ];

  for (const heuristic of ["auto", "prefer"]) {
    for (const hints of [[heuristic, "always"], ["always", heuristic]]) {
      const mir = executableMir();
      for (const [index, hint] of hints.entries()) {
        const call = () => statement("call", { function: 2 + index, args: [], results: [] });
        mir.functions[0].body.statements.push(call(), call());
        mir.functions.push({
          ...mir.functions[0],
          name: hint,
          kind: { kind: "user" },
          attributes: attributes("compiler_generated", hint),
          locals: [{ ty: 2 }, { ty: 1 }, { ty: 0 }, { ty: 0 }],
          body: { statements: loop },
        });
      }
      for (const optimizeLevel of [1, 2, 4]) {
        const { wat } = compileMir(mir, { optimizeLevel, emitText: true });
        const calls = [...wat.matchAll(/\(call \$\$onda\.fn\.([23])\b/g)].map(match => Number(match[1]));
        const shared = 2 + hints.indexOf(heuristic);
        assert.deepEqual(calls, [shared, shared], JSON.stringify({ hints, optimizeLevel }));
      }
    }
  }
});
