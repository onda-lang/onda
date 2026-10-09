import assert from "node:assert/strict";
import test from "node:test";

import {
  createCompiler,
  PROCESSOR_FULL_BLOCK,
  PROCESSOR_INIT_FULL,
} from "../src/index.js";

async function processor(compiler, source, optimize) {
  const blockSize = 48;
  const { artifact } = await compiler.compileSource(source, {
    sampleRate: 48_000,
    blockSize,
    codegen: { optimize },
  });
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_processor_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const allocate = (size) => {
    heap = Math.ceil(heap / 16) * 16;
    const address = heap;
    heap += Math.max(size, 1);
    if (heap > memory.buffer.byteLength) {
      memory.grow(Math.ceil((heap - memory.buffer.byteLength) / 65_536));
    }
    return address;
  };
  const params = allocate(artifact.metadata.runtime.param_size_bytes);
  const state = allocate(artifact.metadata.runtime.state_size_bytes);
  const outputs = artifact.metadata.metadata.outputs.map(() => allocate(blockSize * 4));
  const outputTable = allocate(outputs.length * 4);
  const view = new DataView(memory.buffer);
  outputs.forEach((address, index) => view.setUint32(outputTable + index * 4, address, true));
  const param = artifact.metadata.metadata.params[0];
  const setTarget = (value, element = 0) => {
    const width = param.scalar === "f32" ? 4 : 8;
    const address = params + param.byte_offset + element * width;
    if (width === 4) view.setFloat32(address, value, true);
    else view.setFloat64(address, value, true);
  };
  for (let element = 0; element < param.array_len; element++) setTarget(1, element);
  assert.equal(onda_processor_init(params, state, PROCESSOR_INIT_FULL, 0, 0, 0, 0, 0), 0);
  return {
    process(start = 0, frames = blockSize, flags = PROCESSOR_FULL_BLOCK) {
      assert.equal(onda_process(state, params, 0, outputTable, start, frames, flags, 0, 0, 0, 0, 0), 0);
      return outputs.map((address) => new Float32Array(memory.buffer, address, blockSize));
    },
  };
}

test("const array dimensions and locals resolve before processor and overload typing", async () => {
  const compiler = await createCompiler();
  try {
    const prefix = "const Unused: i32[1] = [1 / 0]\nconst Table: i32[1] = [2]\n";
    for (const body of [
      "const N = Table[0]\nproc Voice:\n  init:\n    data: f32[N] = [0.25, 0.75]\n  sample:\n    out1 = data[0]\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n",
      "proc Voice:\n  init:\n    data: f32[Table[0]] = [0.25, 0.75]\n  sample:\n    out1 = data[0]\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n",
      "def select(value: i32):\n  data: f32[Table[0]] = [0.25, 0.75]\n  return data[0] + f32(value)\ndef select(value: f32):\n  return value\nsample:\n  out1 = select(1)\n",
      "def unused<T>() -> T:\n  First = T(Unused[0])\n  Second = First + T(1)\n  return Second\nsample:\n  out1 = 0.0\n",
      "def select<T>() -> T:\n  First = T(Table[0])\n  Second = First + T(1)\n  return Second\nsample:\n  out1 = select<f32>()\n",
    ]) {
      const { artifact } = await compiler.compileSource(prefix + body);
      assert.equal(WebAssembly.validate(artifact.wasm), true);
    }
  } finally {
    await compiler.dispose();
  }
});

test("long scalar const chains compile without exhausting the Wasm stack", async () => {
  const compiler = await createCompiler();
  try {
    for (const annotation of [": i32", ""]) {
      let source = `const Table: i32[1] = [7]\nconst S0${annotation} = Table[0]\n`;
      for (let index = 1; index <= 4096; index++) {
        source += `const S${index}${annotation} = S${index - 1}\n`;
      }
      for (const body of [
        "const Copy = [S4096]\nsample:\n  out1 = 0.0\n",
        "sample:\n  out1 = f32(S4096)\n",
      ]) {
        const { artifact } = await compiler.compileSource(source + body);
        assert.equal(WebAssembly.validate(artifact.wasm), true);
      }
    }
  } finally {
    await compiler.dispose();
  }
});

test("integer const widths and overloads survive deferred and eager evaluation in Wasm", async () => {
  const compiler = await createCompiler();
  try {
    for (const optimize of [false, true]) {
      for (const [value, expected] of [[7, 2], [4294967297, 4294967296]]) {
        for (const forceEvaluation of [false, true]) {
          for (const expression of ["Table[0]", "Selected", "Copy[0]", "Alias[0]"]) {
            const source = `const Table: i64[1] = [${value}]
${expression === "Selected" ? "const Selected = Table[0]\n" : ""}
const Copy = [Table[0]]
const Alias = Copy
params:
  harness = ${forceEvaluation ? "f32(Table[0])" : "1.0"}
def select(value: i32):
  return ${value === 7 ? "1.0" : "f32(value)"}
def select(value: i64):
  return ${value === 7 ? "2.0" : "f32(value)"}
proc Voice:
  sample:
    out1 = select(${expression})
init:
  voice = Voice()
sample:
  out1 = voice()
`;
            const dsp = await processor(compiler, source, optimize);
            assert.ok(dsp.process()[0].every((sample) => sample === expected), source);
          }
        }
      }
    }
    const { artifact } = await compiler.compileSource(`const Table: i64[1] = [1 / 0]
const Copy = [Table[0]]
const Alias = Copy
sample:
  out1 = 0.0
`);
    assert.equal(WebAssembly.validate(artifact.wasm), true);
  } finally {
    await compiler.dispose();
  }
});

test("deferred aliases, task constants, and graph sources execute in Wasm", async () => {
  const compiler = await createCompiler();
  try {
    const cases = [
      ["f64", "0.25", "f32"],
      ["i64", "7", "i32"],
    ].flatMap(([scalar, value, target]) => {
      const prefix = `const Table: ${scalar}[1] = [${value}]
const Selected = Table[0]
`;
      return [
        [prefix + `proc Voice:
  init:
    value: ${target} = Selected
  sample:
    out1 = f32(value)
init:
  voice = Voice()
sample:
  out1 = voice()
`, Number(value)],
        [prefix + `def select(value: i32):
  return 0.0
def select(value: f32):
  result: ${target} = Selected
  return f32(result)
sample:
  out1 = select(0.0)
`, Number(value)],
        [prefix + `struct Holder:
  value: ${target} = 0
  def set(self):
    self.value = Selected
init:
  holder = Holder()
sample:
  out1 = f32(holder.value)
`, 0],
      ];
    });
    for (const expression of ["Selected", "Table[0]", "select()"]) {
      cases.push([`const Table: i32[1] = [7]
const Selected = Table[0]
def select():
  return Selected
proc Voice:
  init:
    value: i32 = 0
  task work():
    value = ${expression}
    yield
  block:
    await work()
    sample:
      out1 = f32(value)
init:
  voice = Voice()
sample:
  out1 = voice()
`, 7, 0]);
    }
    cases.push([`const Table: i64[1] = [4294967297]
const Selected = Table[0]
proc Voice:
  init:
    value = Selected
  sample:
    out1 = f32((value == 4294967297))
init:
  voice = Voice()
sample:
  out1 = voice()
`, 1]);
    cases.push([`const Table: i32[1] = [7]
proc Voice:
  graph:
    f32(Table[0]) >> out1
init:
  voice = Voice()
sample:
  out1 = voice()
`, 7]);
    for (const expression of ["Selected", "Table[0]", "Table[i32(0)]"]) {
      cases.push([`const Table: f64[1] = [0.25]
const Selected = Table[0]
proc Voice:
  graph:
    ${expression} >> out1
init:
  voice = Voice()
sample:
  out1 = voice()
`, 0.25]);
    }
    // Concrete f64 array elements retain their type through indexing and scalar aliases.
    for (const expression of ["Selected", "Table[0]", "Table[i32(0)]"]) {
      cases.push([`const Table: f64[1] = [0.25]
const Selected = Table[0]
def select(value: f32):
  return 1.0
def select(value: f64):
  return 2.0
proc Voice:
  sample:
    out1 = select(${expression})
init:
  voice = Voice()
sample:
  out1 = voice()
`, 2]);
    }
    for (const optimize of [false, true]) {
      for (const [source, expected, firstBlock] of cases) {
        const dsp = await processor(compiler, `params:
  harness = 1.0
${source}`, optimize);
        if (firstBlock !== undefined) {
          assert.ok(dsp.process()[0].every((sample) => sample === firstBlock), source);
        }
        assert.ok(dsp.process()[0].every((sample) => sample === expected), source);
      }
    }
    const graph = `const Table: i32[1] = [1 / 0]
proc Voice:
  graph:
    f32(Table[0]) >> out1
`;
    await compiler.compileSource(graph + `sample:
  out1 = 0.0
`);
    await assert.rejects(compiler.compileSource(graph + `init:
  voice = Voice()
sample:
  out1 = voice()
`), /division by zero/);
  } finally {
    await compiler.dispose();
  }
});
