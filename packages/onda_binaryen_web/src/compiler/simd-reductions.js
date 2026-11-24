// Pack independent elements of fixed-size state arrays, keeping their final
// reduction in scalar source order. Unsupported statements use scalar lowering.
import binaryen from "binaryen";

const local = (value) => value?.kind === "local" ? value.data : null;
const dest = (place) => place?.projections.length === 0 ? local(place.base) : null;
const laneCount = 4;
const isFloat = (type) => type === "f32" || type === "f64";
const opcodes = new Map([["add", "add"], ["subtract", "sub"], ["multiply", "mul"], ["divide", "div"]]);

// Resolve transport ordinals once. Lowering may revisit a body when falling back
// to scalar code, so it looks up plans by statement identity.
export function indexOrderedReductions(mir) {
  const plans = new WeakMap();
  if (!mir.ordered_reductions?.length) return plans;
  const byOrdinal = new Map(mir.ordered_reductions.map(plan => [plan.loop_ordinal, plan]));
  let ordinal = 0;
  function visit(block) {
    for (const statement of block.statements) {
      const { kind, data } = statement.kind;
      if (kind === "loop") {
        const plan = byOrdinal.get(ordinal++);
        if (plan) plans.set(statement, plan);
        visit(data.body);
      } else if (kind === "if") {
        visit(data.then_block);
        visit(data.else_block);
      }
    }
  }
  visit(mir.functions[mir.entry_points.process].body);
  return plans;
}

export function vectorizeBankReduction(compiler, statement, context, scalarLoop, competingLoop) {
  const loop = compiler.orderedReductions.get(statement);
  const { body } = statement.kind.data;
  if (!compiler.options.simd || !compiler.options.optimize || !loop) return;
  // Short banks often lose to the scalar schedule when the alternate arm
  // executes another loop. Standalone banks can pay off with one vector.
  const minimum = competingLoop ? 4 * laneCount : laneCount;
  if (!(loop.last - loop.first + 1 >= minimum)) return;
  // The proof identifies the reduction assignment by its unique result.
  const startOfReduction = body.statements.findIndex(({ kind: { kind, data } }) =>
    kind === "assign" && dest(data.destination) === loop.sum_result);
  if (startOfReduction < 0) return;
  const plan = planStateReduction(compiler, context, loop, body.statements.slice(0, startOfReduction));
  if (plan) return emitStateReduction(compiler, context, loop, plan, scalarLoop);
}

// Lower the operations supported by contiguous SIMD lanes. MIR has already
// proved independence. Forward same-lane writes in statement order.
function planStateReduction(compiler, context, loop, body) {
  const assigned = new Set([loop.index, loop.sum, loop.condition, loop.sum_result]);
  for (const { kind: { kind, data } } of body) {
    const ids = kind === "assign" ? [dest(data.destination)] : kind === "call" ? data.results : [];
    for (const id of ids) assigned.add(id);
  }
  const m = compiler.module;
  const vectors = { f32: m.f32x4, f64: m.f64x2, i32: m.i32x4 };
  const indexValue = () => compiler.loadLocal(loop.index, context);
  const nodes = [];
  const node = (type, emit, invariant = false) => {
    const value = {
      type, emit, invariant, slots: [],
      get(half = 0) {
        return m.local.get(this.slots[type === "f64" ? half : 0], binaryen.v128);
      },
    };
    nodes.push(value);
    return value;
  };
  const induction = node("i32", () => m.i32x4.add(m.i32x4.splat(indexValue()),
    m.v128.const([0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0])));
  const bank = new Map();
  const writes = new Map();
  const root = { locals: new Map([[loop.index, induction]]), params: [] };
  const value = (v, frame) => {
    if (v.kind === "constant" && isFloat(v.data.type)) {
      return node(v.data.type, () => vectors[v.data.type].splat(compiler.compileValue(v, context)));
    }
    const id = local(v);
    if (id === null) return;
    if (frame.locals.has(id)) return frame.locals.get(id);
    if (frame === root && !assigned.has(id) && !compiler.localScalarRefLayout[context.functionId][id] && isFloat(context.localScalars[id])) {
      const type = context.localScalars[id];
      return node(type, () => vectors[type].splat(compiler.loadLocal(id, context)));
    }
  };
  const reference = (p, frame) => {
    if (p.base.kind === "parameter" && !p.projections.length) return frame.params[p.base.data];
    const projection = p.projections[0];
    if (p.base.kind !== "state" || p.projections.length !== 1 || projection.kind !== "index" ||
        frame.locals.get(local(projection.data.index)) !== induction) return;
    const id = p.base.data;
    const array = compiler.type(compiler.mir.state[id].ty);
    if (array.kind !== "array") return;
    const type = compiler.type(array.data.element).data;
    if (isFloat(type)) return { state: id, type };
  };
  const address = (ref) => m.i32.add(compiler.stateAddress(ref.state, context),
    m.i32.mul(indexValue(), m.i32.const(compiler.scalarSize(ref.type))));
  const load = (ref) => {
    if (!ref || ref.state === undefined) return ref;
    if (!bank.has(ref.state)) bank.set(ref.state, node(ref.type, (half) => m.v128.load(16 * half, 1, address(ref))));
    return bank.get(ref.state);
  };
  const evaluate = ({ kind, data }, frame) => {
    if (kind === "use") return value(data, frame);
    if (kind === "load") return dest(data) === null ? load(reference(data, frame)) : value(data.base, frame);
    if (kind === "cast") {
      const a = value(data.value, frame);
      if (!a) return;
      if (a.type === data.to) return a;
      if (data.to === "f32" && a.type === "i32") return node("f32", () => m.f32x4.convert_i32x4_s(a.get()));
      if (data.to === "f64" && (a.type === "i32" || a.type === "f32")) {
        const cast = a.type === "i32" ? m.f64x2.convert_low_i32x4_s : m.f64x2.promote_low_f32x4;
        return node("f64", (half) => cast(half === 0 ? a.get() :
          m.i8x16.shuffle(a.get(), a.get(), [8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7])));
      }
      if (data.to === "f32" && a.type === "f64") return node("f32", () => {
        const demote = (slot) => m.f32x4.demote_f64x2_zero(m.local.get(slot, binaryen.v128));
        return m.i8x16.shuffle(demote(a.slots[0]), demote(a.slots[1]), [0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23]);
      });
    }
    if (kind === "binary") {
      const a = value(data.lhs, frame);
      const b = value(data.rhs, frame);
      const op = opcodes.get(data.op);
      if (a && b && isFloat(a.type) && a.type === b.type && op) return node(a.type, (half) => vectors[a.type][op](a.get(half), b.get(half)));
    }
    if (kind === "input_load" && frame === root && data.element === null) {
      const port = compiler.inputLayout[data.input];
      if (!isFloat(port.scalar)) return;
      return node(port.scalar, () => vectors[port.scalar].splat(compiler.compileInputLoad(data, context)), true);
    }
  };
  // Inline straight-line calls whose references resolve to the current
  // element, including several references to that same element.
  // Bound code growth across the entire expanded call tree.
  let budget = 128;
  function analyze(list, frame) {
    for (const { kind: { kind, data } } of list) {
      if (--budget < 0) return;
      if (kind === "assign") {
        const rhs = evaluate(data.value, frame);
        if (!rhs) return;
        const id = dest(data.destination);
        if (id !== null) frame.locals.set(id, rhs);
        else {
          const ref = reference(data.destination, frame);
          if (ref?.state === undefined || rhs.type !== ref.type) return;
          bank.set(ref.state, rhs);
          writes.set(ref.state, ref);
        }
      } else if (kind === "call") {
        const callee = compiler.mir.functions[data.function];
        if (callee.attributes?.inline === "never" || callee.locals.some((l) => compiler.type(l.ty).kind !== "scalar")) return;
        const params = data.args.map((arg) => {
          if (arg.kind === "value") return value(arg.data, frame);
          if (arg.kind === "place") return reference(arg.data, frame);
        });
        if (params.some((p) => !p)) return;
        const results = analyze(callee.body.statements, { locals: new Map(), params });
        if (!results || results.length !== data.results.length) return;
        data.results.forEach((id, i) => frame.locals.set(id, results[i]));
      } else if (kind === "return" && frame !== root) {
        const results = data.values.map((v) => value(v, frame));
        return results.every(Boolean) ? results : undefined;
      } else return;
    }
    return [];
  }
  if (!analyze(body, root) || !writes.size) return;
  const term = value(loop.term, root);
  if (term?.type !== context.localScalars[loop.sum]) return;

  return { nodes, writes, bank, root, term, address };
}

function emitStateReduction(compiler, context, loop, plan, scalarLoop) {
  const { nodes, writes, bank, root, term, address } = plan;
  const m = compiler.module;
  const vectors = { f32: m.f32x4, f64: m.f64x2, i32: m.i32x4 };
  const indexValue = () => compiler.loadLocal(loop.index, context);
  const stride = laneCount;
  const preheader = [];
  const instructions = [];
  for (const n of nodes) {
    const dest = n.invariant ? preheader : instructions;
    for (let half = 0; half < (n.type === "f64" ? 2 : 1); half++) {
      const slot = compiler.allocateGeneratedLocal(context, "v128", "bank.vector");
      n.slots.push(slot);
      dest.push(m.local.set(slot, n.emit(half)));
    }
  }
  for (const ref of writes.values()) {
    const value = bank.get(ref.state);
    for (let half = 0; half < value.slots.length; half++) {
      instructions.push(m.v128.store(16 * half, 1, address(ref), value.get(half)));
    }
  }
  let reduced = compiler.loadLocal(loop.sum, context);
  for (let lane = 0; lane < stride; lane++) {
    const half = Math.floor(lane / 2);
    const scalar = vectors[term.type].extract_lane(term.get(half), term.type === "f64" ? lane % 2 : lane);
    reduced = m[term.type].add(reduced, scalar);
  }
  instructions.push(compiler.storeLocal(loop.sum, reduced, context));
  instructions.push(compiler.storeLocal(loop.index, m.i32.add(indexValue(), m.i32.const(stride)), context));
  const label = `$onda.bank.vector.${compiler.nextLabel++}`;
  const enough = () => m.i32.lt_u(indexValue(), m.i32.const(loop.last - stride + 2));
  instructions.push(m.br_if(label, enough()));
  // A final complete vector must leave each externally visible local at the
  // value of its last lane. A partial vector runs the original scalar tail.
  const exit = `$onda.bank.done.${compiler.nextLabel++}`;
  const restore = [];
  for (const [id, v] of root.locals) {
    if (id === loop.index) continue;
    restore.push(compiler.storeLocal(id, vectors[v.type].extract_lane(v.get(1), v.type === "f64" ? 1 : 3), context));
  }
  restore.push(compiler.storeLocal(loop.index, m.i32.const(loop.last), context));
  restore.push(compiler.storeLocal(loop.condition, m.i32.const(1), context));
  restore.push(compiler.storeLocal(loop.sum_result, compiler.loadLocal(loop.sum, context), context));
  restore.push(m.br(exit));
  const vectorLoop = m.block(null, [...preheader, m.loop(label, m.block(null, instructions)),
    m.if(m.i32.eq(indexValue(), m.i32.const(loop.last + 1)), m.block(null, restore))]);
  return m.block(exit, [m.if(enough(), vectorLoop), scalarLoop]);
}
