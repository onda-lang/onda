import assert from "node:assert/strict";
import test from "node:test";
import binaryen from "binaryen";

import { optimizeGuardedAddresses, sinkGuardedAddresses } from "../src/compiler/guarded-addresses.js";

function guardedStoreShape({ fields = 12, optimize = false, split = false, gateStore, entryBases, changeBase, alwaysStore, nestedLoop, laterRead } = {}) {
  const m = new binaryen.Module();
  m.setMemory(1, 1, "memory");
  const get = index => m.local.get(index, binaryen.i32);
  const offset = fields;
  const sideParam = fields + 1;
  const enabled = fields + 2;
  const paramCount = fields + 3;
  const bases = Array.from({ length: fields }, (_, i) => 4 * i);
  const addresses = bases.map((_, i) => paramCount + i);
  const gatePtr = paramCount + fields;
  const groups = split
    ? [addresses.slice(0, fields / 2), addresses.slice(fields / 2)]
    : [addresses];
  const decisions = groups.map(group => {
    const store = n => m.block(null, group.map(addr =>
      m.i32.store(0, 4, get(addr), m.i32.add(m.i32.load(0, 4, get(addr)), m.i32.const(n)))));
    const maybeStore = n => alwaysStore ? store(n) : m.if(get(enabled), store(n));
    const update = maybeStore(7);
    const reset = maybeStore(9);
    const left = m.block(null, [
      ...(changeBase ? [m.local.set(offset, m.i32.const(256))] : []),
      nestedLoop ? m.loop("inner", update) : update,
    ]);
    const condition = gateStore
      ? m.i32.ne(m.i32.load(0, 4, get(gatePtr)), m.i32.const(0)) : get(sideParam);
    return { branch: m.if(condition, left, reset), update, reset };
  });
  const storeGate = gateStore ? [m.i32.store(0, 4,
    m.local.tee(gatePtr, m.i32.const(512), binaryen.i32),
    gateStore === "literal" ? m.i32.const(1) : get(sideParam))] : [];
  const loopBody = m.block(null, [
    ...storeGate,
    ...addresses.map((addr, i) => m.local.set(addr, m.i32.add(get(i), get(offset)))),
    ...decisions.map(d => d.branch),
    ...(laterRead ? [m.drop(get(addresses[0]))] : []),
  ]);
  const loop = m.loop("outer", loopBody);
  let body = loop;
  if (entryBases) {
    m.addGlobal("base", binaryen.i32, true, m.i32.const(0));
    const wrap = entryBases === "wrap";
    const prefix = [m.global.set("base", m.i32.const(wrap ? -64 : 0))];
    if (wrap) {
      prefix.push(m.local.set(gatePtr, m.i32.add(m.global.get("base", binaryen.i32), m.i32.const(0))));
    }
    for (let i = 0; i < fields; i++) {
      if (entryBases === "split" && i === Math.floor(fields / 2)) {
        prefix.push(m.global.set("base", m.i32.const(128)));
      }
      prefix.push(m.local.set(i, m.i32.add(m.global.get("base", binaryen.i32), m.i32.const(4 * i + (wrap ? 64 : 0)))));
    }
    body = m.block(null, [...prefix, m.global.set("base", m.i32.const(256)), loop]);
  }
  const func = m.addFunction("run", binaryen.createType(Array(paramCount).fill(binaryen.i32)),
    binaryen.none, Array(fields + Number(Boolean(gateStore || entryBases === "wrap"))).fill(binaryen.i32), body);
  m.addFunctionExport("run", "run");
  try {
    const passes = [];
    if (optimize) {
      const runPasses = m.runPassesOnFunction.bind(m);
      m.runPassesOnFunction = (target, names) => {
        passes.push(...names);
        runPasses(target, names);
      };
      optimizeGuardedAddresses(m, func);
    } else {
      sinkGuardedAddresses(m, body);
    }
    assert.ok(m.validate());
    const { run, memory } = new WebAssembly.Instance(new WebAssembly.Module(m.emitBinary())).exports;
    const data = new Int32Array(memory.buffer);
    for (const side of [0, 1]) {
      for (const gate of [0, 1]) {
        data.fill(0);
        run(...bases, 8, side, gate);
        const value = gate || alwaysStore ? (side || gateStore === "literal" ? 7 : 9) : 0;
        for (let i = 0; i < fields; i++) {
          const shift = entryBases === "split" && i >= Math.floor(fields / 2) ? 32 : 0;
          assert.equal(data[2 + i + shift], value);
        }
        assert.equal(data[64], 0);
      }
    }
    if (optimize) return passes;
    function sharedBaseAt(store) {
      let address = binaryen.Store.getPtr(store);
      if (binaryen.getExpressionId(binaryen.Binary.getRight(address)) === binaryen.ConstId) {
        address = binaryen.Binary.getLeft(address);
      }
      return binaryen.LocalGet.getIndex(binaryen.Binary.getRight(address));
    }
    function usesAddressAtStore(guard) {
      const stores = binaryen.Block.getChildren(binaryen.If.getIfTrue(guard));
      return stores.every(store => {
        const load = binaryen.Binary.getLeft(binaryen.Store.getValue(store));
        return [binaryen.Store.getPtr(store), binaryen.Load.getPtr(load)].every(
          ptr => binaryen.getExpressionId(ptr) === binaryen.BinaryId,
        );
      });
    }
    const stores = entryBases && binaryen.Block.getChildren(binaryen.If.getIfTrue(decisions[0].update));
    return {
      ...(entryBases && { bases: stores.map(sharedBaseAt) }),
      entry: binaryen.getExpressionId(binaryen.Block.getChildAt(loopBody, storeGate.length)),
      guarded: !alwaysStore && decisions.every(d => usesAddressAtStore(d.update) && usesAddressAtStore(d.reset)),
    };
  } finally {
    m.dispose();
  }
}

test("computes indexed addresses at guarded memory accesses in both arms", () => {
  assert.deepEqual(guardedStoreShape(), { entry: binaryen.NopId, guarded: true });
});

test("shares fixed field bases only within one entry snapshot", () => {
  assert.deepEqual(guardedStoreShape({ entryBases: "shared" }).bases, Array(12).fill(0));
  assert.deepEqual(guardedStoreShape({ entryBases: "split" }).bases,
    [...Array(6).fill(0), ...Array(6).fill(6)]);
});

test("preserves wrapping i32 field offsets at guarded memory operations", () => {
  const result = guardedStoreShape({ entryBases: "wrap" });
  assert.equal(result.entry, binaryen.NopId);
  assert.equal(new Set(result.bases).size, 1);
});

test("keeps short groups of addresses shared across a decision", () => {
  assert.equal(guardedStoreShape({ fields: 8 }).entry, binaryen.LocalSetId);
  assert.equal(guardedStoreShape({ fields: 12, split: true }).entry, binaryen.LocalSetId);
});

test("keeps addresses shared across a comparison with a just-stored literal", () => {
  assert.equal(guardedStoreShape({ gateStore: "literal" }).entry, binaryen.LocalSetId);
  assert.equal(guardedStoreShape({ gateStore: "dynamic" }).entry, binaryen.NopId);
});

test("keeps the original address when movement is unsafe or reaches every path", () => {
  for (const options of [{ changeBase: true }, { alwaysStore: true }, { nestedLoop: true }, { laterRead: true }]) {
    assert.equal(guardedStoreShape(options).entry, binaryen.LocalSetId);
  }
});

test("runs SSA and cleanup only for candidate functions", () => {
  assert.deepEqual(guardedStoreShape({ fields: 8, optimize: true }), []);
  assert.ok(guardedStoreShape({ optimize: true }).includes("ssa-nomerge"));
});
