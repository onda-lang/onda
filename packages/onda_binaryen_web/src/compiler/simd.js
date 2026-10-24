// Vectorize independent sample iterations. The proof admits only straight-line
// float arithmetic, invariant reads, and contiguous audio I/O. Stateful loops,
// calls, gathers, and loop-carried values retain scalar lowering.
import binaryen from "binaryen";
import { POINTER_GLOBALS } from "./shared.js";

const local = (value) => value?.kind === "local" ? value.data : null;
const destination = (data) => data?.destination?.projections?.length === 0
  ? local(data.destination.base) : null;
const isOne = (value) => value?.kind === "constant"
  && value.data.type === "i32" && value.data.value === 1;
const same = (a, b) => local(a) !== null && local(a) === local(b);

export function vectorizeSampleLoop(compiler, statement, context, scalarLoop) {
  if (!compiler.options.simd || context.function.kind.kind !== "process") return null;
  const loop = statement.kind.data.body.statements;
  if (loop.length !== 2 || loop[0].kind.kind !== "assign" || loop[1].kind.kind !== "if") return null;
  const test = loop[0].kind.data;
  const branch = loop[1].kind.data;
  const comparison = test.value;
  if (comparison.kind !== "compare" || comparison.data.op !== "less"
      || local(comparison.data.lhs) === null || destination(test) !== local(branch.condition)
      || branch.else_block.statements.length !== 1
      || branch.else_block.statements[0].kind.kind !== "break") return null;
  const index = local(comparison.data.lhs);
  const limit = comparison.data.rhs;
  const body = branch.then_block.statements;
  const step = body.at(-1)?.kind;
  if (step?.kind !== "assign" || destination(step.data) !== index
      || step.data.value.kind !== "binary" || step.data.value.data.op !== "add"
      || local(step.data.value.data.lhs) !== index || !isOne(step.data.value.data.rhs)) return null;
  // Require the actual segment length, not an arbitrary integer loop bound.
  const limitDefinitions = context.function.body.statements.filter((item) =>
    item.kind.kind === "assign" && destination(item.kind.data) === local(limit));
  if (limitDefinitions.length !== 1) return null;
  const limitLoad = limitDefinitions[0].kind.data.value;
  if (limitLoad.kind !== "load" || limitLoad.data.base.kind !== "parameter"
      || limitLoad.data.base.data !== 1 || limitLoad.data.projections.length !== 0) return null;
  const frameStatement = body[0]?.kind;
  if (frameStatement?.kind !== "assign" || frameStatement.data.value.kind !== "process_frame"
      || local(frameStatement.data.value.data.offset) !== index) return null;
  const frame = destination(frameStatement.data);
  const operations = body.slice(1, -1);
  const written = new Set();
  let scalar;
  const ports = new Map();
  const addPort = (kind, data) => {
    const layout = kind === "input" ? compiler.inputLayout[data.input] : compiler.outputLayout[data.output];
    if (!layout || layout.scalar !== scalar || data.element !== null || local(data.frame) !== frame) return false;
    ports.set(`${kind}:${kind === "input" ? data.input : data.output}`, { kind, data, layout });
    return true;
  };
  for (const operation of operations) {
    if (operation.kind.kind === "assign") {
      const id = destination(operation.kind.data);
      if (id === null || id === index || id === frame || compiler.localScalarRefLayout[context.functionId][id]) return null;
      const type = context.localScalars[id];
      if (!['f32', 'f64'].includes(type) || (scalar && scalar !== type)) return null;
      scalar = type;
      written.add(id);
    } else if (operation.kind.kind !== "output_store") return null;
  }
  if (!scalar) return null;
  const defined = new Set();
  const validValue = (value) => value?.kind === "constant" ? value.data.type === scalar
    : local(value) !== null && context.localScalars[value.data] === scalar
      && (!written.has(value.data) || defined.has(value.data));
  let stores = 0;
  for (const operation of operations) {
    const { kind, data } = operation.kind;
    if (kind === "output_store") {
      if (!validValue(data.value) || !addPort("output", data)) return null;
      stores += 1;
      continue;
    }
    const value = data.value;
    switch (value.kind) {
      case "use": if (!validValue(value.data)) return null; break;
      case "binary":
        if (!['add', 'subtract', 'multiply', 'divide'].includes(value.data.op)
            || !validValue(value.data.lhs) || !validValue(value.data.rhs)) return null;
        break;
      case "input_load": if (!addPort("input", value.data)) return null; break;
      // Parameters and state cannot change in the admitted loop. A projected
      // load could depend on the induction variable, so leave it scalar.
      case "load":
        if (!['state', 'param', 'parameter'].includes(value.data.base.kind) || value.data.projections.length) return null;
        break;
      default: return null;
    }
    defined.add(destination(data));
  }
  if (!stores) return null;

  const m = compiler.module;
  const lanes = scalar === "f32" ? 4 : 2;
  const vector = scalar === "f32" ? m.f32x4 : m.f64x2;
  const locals = new Map([...written].map((id) => [id, compiler.allocateGeneratedLocal(context, "v128", "sample.vector")]));
  const getVector = (value) => locals.has(local(value))
    ? m.local.get(locals.get(value.data), binaryen.v128)
    : vector.splat(compiler.compileValue(value, context));
  const pointers = new Map([...ports].map(([key, { kind, data, layout }]) => [key, () => compiler.audioChannelPointer(
    kind === "input" ? POINTER_GLOBALS.inputs : POINTER_GLOBALS.outputs,
    layout, data.element, data.bounds, context,
  )]));
  const address = (kind, data) => m.i32.add(
    pointers.get(`${kind}:${kind === "input" ? data.input : data.output}`)(),
    m.i32.mul(compiler.compileProcessFrame({ offset: comparison.data.lhs }, context), m.i32.const(compiler.scalarSize(scalar))),
  );
  const vectorBody = [];
  const binary = { add: 'add', subtract: 'sub', multiply: 'mul', divide: 'div' };
  for (const operation of operations) {
    const { kind, data } = operation.kind;
    if (kind === "output_store") {
      vectorBody.push(m.v128.store(0, 1, address("output", data), getVector(data.value)));
      continue;
    }
    const value = data.value;
    let expression;
    if (value.kind === "use") expression = getVector(value.data);
    else if (value.kind === "binary") expression = vector[binary[value.data.op]](getVector(value.data.lhs), getVector(value.data.rhs));
    else if (value.kind === "input_load") expression = m.v128.load(0, 1, address("input", value.data));
    else expression = vector.splat(compiler.compileRvalue(value, scalar, context));
    vectorBody.push(m.local.set(locals.get(destination(data)), expression));
  }
  // Preserve locals observed by block-post code, including the last frame.
  for (const [id, vectorLocal] of locals) {
    vectorBody.push(compiler.storePlace({ base: { kind: "local", data: id }, projections: [] },
      vector.extract_lane(m.local.get(vectorLocal, binaryen.v128), lanes - 1), scalar, context));
  }
  vectorBody.push(compiler.storePlace(frameStatement.data.destination,
    m.i32.add(compiler.compileProcessFrame({ offset: comparison.data.lhs }, context), m.i32.const(lanes - 1)), "i32", context));
  vectorBody.push(compiler.storePlace(step.data.destination,
    m.i32.add(compiler.compileValue(comparison.data.lhs, context), m.i32.const(lanes)), "i32", context));
  const label = `$onda.sample.vector.${compiler.nextLabel++}`;
  const vectorLoop = m.loop(label, m.if(
    m.i32.le_s(compiler.compileValue(comparison.data.lhs, context), m.i32.sub(compiler.compileValue(limit, context), m.i32.const(lanes))),
    m.block(null, [...vectorBody, m.br(label)]),
  ));
  // Exact in-place buffers preserve per-lane order. Partial overlap can carry
  // data between iterations, so version the loop with a disjointness check.
  const accessed = [...ports.keys()];
  let safe = m.i32.const(1);
  for (let i = 0; i < accessed.length; i += 1) {
    for (let j = i + 1; j < accessed.length; j += 1) {
      if (accessed[i].startsWith("input:") && accessed[j].startsWith("input:")) continue;
      const a = pointers.get(accessed[i]);
      const b = pointers.get(accessed[j]);
      const distance = m.select(m.i32.lt_u(a(), b()), m.i32.sub(b(), a()), m.i32.sub(a(), b()), binaryen.i32);
      safe = m.i32.and(safe, m.i32.or(m.i32.eq(a(), b()), m.i32.ge_u(distance, m.i32.const(compiler.mir.config.block_size * compiler.scalarSize(scalar)))));
    }
  }
  return m.block(null, [m.if(safe, vectorLoop), scalarLoop]);
}
