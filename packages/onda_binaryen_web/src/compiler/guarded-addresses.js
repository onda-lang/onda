import binaryen from "binaryen";
import { prepareFunctionBody, rewriteExpressionChildren } from "./expression-tree.js";

// Duplicating a few additions can slow compact loops. Reserve this pass for
// decisions that keep a group of pointer temporaries live across the branch.
const MIN_LIVE_ADDRESSES = 12;

// Binaryen can push a set into one arm. When both arms read the pointer,
// find the innermost guard enclosing all dereferences on each side.
function guardedDecision(definition, reads) {
  const targets = new Map();
  for (const read of reads) {
    // A statement dominates its later siblings and their children. Staying
    // in the same loop also prevents moving work across iterations.
    if (!read.memoryAccess || read.loop !== definition.loop ||
        read.position <= definition.start || read.position > definition.block.end) {
      return null;
    }
    let outer = read.arm;
    while (outer && outer.parent !== definition.arm) outer = outer.parent;
    if (!outer || outer.start <= definition.start) return null;

    // Reads are in traversal order. Widen the first read's arm just enough
    // to enclose each subsequent read on this side of the decision.
    let target = targets.get(outer) ?? read.arm;
    while (target.end < read.position) target = target.parent;
    targets.set(outer, target);
  }
  if (targets.size !== 2) return null;
  const [left, right] = targets.keys();
  // Duplicating at the entry to both arms would still run the addition
  // on every path. At least one arm must have a deeper guard.
  if (left.owner !== right.owner ||
      (targets.get(left) === left && targets.get(right) === right)) {
    return null;
  }
  return left.owner;
}

function addressSum(set) {
  if (binaryen.LocalSet.isTee(set)) return 0;
  const value = binaryen.LocalSet.getValue(set);
  if (binaryen.getExpressionId(value) !== binaryen.BinaryId ||
      binaryen.Binary.getOp(value) !== binaryen.AddInt32 ||
      binaryen.getExpressionId(binaryen.Binary.getLeft(value)) !== binaryen.LocalGetId ||
      binaryen.getExpressionId(binaryen.Binary.getRight(value)) !== binaryen.LocalGetId) return 0;
  return value;
}

export function sinkGuardedAddresses(module, body) {
  const reads = new Map();
  const writes = new Map();
  const definitions = [];
  const constantStores = new Map();
  let position = 0;
  let complete = true;

  function scan(expr, loop, arm, block = null, childIndex = -1, memoryAccess = 0) {
    const start = ++position;
    const id = binaryen.getExpressionId(expr);
    if (id === binaryen.LocalGetId) {
      const index = binaryen.LocalGet.getIndex(expr);
      if (!reads.has(index)) reads.set(index, []);
      reads.get(index).push({ position, arm, loop, memoryAccess });
      return;
    }
    if (id === binaryen.LocalSetId) {
      const index = binaryen.LocalSet.getIndex(expr);
      writes.set(index, writes.has(index) ? Infinity : start);
      const value = block && loop && addressSum(expr);
      if (value) {
        const operands = [binaryen.Binary.getLeft(value), binaryen.Binary.getRight(value)]
          .map(input => binaryen.LocalGet.getIndex(input));
        definitions.push({ index, value, operands, start, block, childIndex, loop, arm });
      }
    }
    if (id === binaryen.BlockId) {
      const scope = { expr, end: 0 };
      const children = binaryen.Block.getChildren(expr);
      children.forEach((child, i) => scan(child, loop, arm, scope, i));
      scope.end = position;
      return;
    }
    if (id === binaryen.IfId) {
      const decision = { condition: binaryen.If.getCondition(expr), block, start };
      scan(decision.condition, loop, arm);
      for (const whenTrue of [true, false]) {
        const child = whenTrue ? binaryen.If.getIfTrue(expr) : binaryen.If.getIfFalse(expr);
        if (!child) continue;
        const branch = { parent: arm, owner: decision, start: position + 1, end: 0 };
        scan(child, loop, branch);
        branch.end = position;
      }
      return;
    }
    if (id === binaryen.LoopId) loop = expr;
    const ptr = id === binaryen.LoadId ? binaryen.Load.getPtr(expr)
      : id === binaryen.StoreId ? binaryen.Store.getPtr(expr) : 0;
    if (block && id === binaryen.StoreId && binaryen.getExpressionId(ptr) === binaryen.LocalSetId &&
        binaryen.getExpressionId(binaryen.Store.getValue(expr)) === binaryen.ConstId) {
      constantStores.set(binaryen.LocalSet.getIndex(ptr), {
        block, start, offset: binaryen.Store.getOffset(expr),
      });
    }
    if (!rewriteExpressionChildren(expr, child => {
      scan(child, loop, arm, null, -1, child === ptr ? expr : 0);
      return child;
    })) complete = false;
  }

  scan(body, null, null);
  // An unsupported subtree could hide a write to the index or base.
  if (!complete || definitions.length < MIN_LIVE_ADDRESSES) return;
  const decisions = new Map();
  for (const def of definitions) {
    // A single earlier write (or a parameter) keeps the base and index
    // stable until all uses.
    if (writes.get(def.index) !== def.start ||
        def.operands.some(index => (writes.get(index) ?? 0) >= def.start)) {
      continue;
    }
    const decision = guardedDecision(def, reads.get(def.index) ?? []);
    if (!decision) continue;
    if (!decisions.has(decision)) decisions.set(decision, []);
    decisions.get(decision).push(def);
  }

  let fieldBases;
  for (const [decision, group] of decisions) {
    if (group.length < MIN_LIVE_ADDRESSES) continue;
    // An inlined setter may test a field it just assigned a literal. Keep
    // addresses shared when that pattern suggests a predictable branch.
    if (testsConstantStore(decision, constantStores)) continue;
    fieldBases ??= entryFieldBases(body);
    for (const def of group) {
      const pointer = indexedFieldAddress(module, def, fieldBases, writes);
      for (const { memoryAccess } of reads.get(def.index)) {
        const address = module.copyExpression(pointer);
        if (binaryen.getExpressionId(memoryAccess) === binaryen.LoadId) {
          binaryen.Load.setPtr(memoryAccess, address);
        } else {
          binaryen.Store.setPtr(memoryAccess, address);
        }
      }
      binaryen.Block.setChildAt(def.block.expr, def.childIndex, module.nop());
    }
  }
}

// State layout exposes fixed field bases as consecutive entry assignments of
// the form "base = global + offset". One snapshot can address every field in
// such a run. Any intervening statement or change of global ends the run.
function entryFieldBases(body) {
  const bases = new Map();
  if (binaryen.getExpressionId(body) !== binaryen.BlockId) return bases;
  let run;
  for (const statement of binaryen.Block.getChildren(body)) {
    const field = fixedFieldBase(statement);
    if (!field) {
      run = null;
      continue;
    }
    if (run?.global !== field.global) run = { global: field.global, lowest: field };
    if (field.offset < run.lowest.offset) run.lowest = field;
    bases.set(field.index, { field, run });
  }
  return bases;
}

function fixedFieldBase(statement) {
  if (binaryen.getExpressionId(statement) !== binaryen.LocalSetId) return null;
  const add = binaryen.LocalSet.getValue(statement);
  if (binaryen.getExpressionId(add) !== binaryen.BinaryId ||
      binaryen.Binary.getOp(add) !== binaryen.AddInt32) return null;
  const base = binaryen.Binary.getLeft(add);
  const offset = binaryen.Binary.getRight(add);
  if (binaryen.getExpressionId(base) !== binaryen.GlobalGetId ||
      binaryen.getExpressionId(offset) !== binaryen.ConstId) return null;
  return {
    index: binaryen.LocalSet.getIndex(statement),
    global: binaryen.GlobalGet.getName(base),
    offset: binaryen.Const.getValueI32(offset),
  };
}

function indexedFieldAddress(module, def, bases, writes) {
  const side = def.operands.findIndex(index => bases.has(index));
  if (side < 0) return def.value;
  const { field, run } = bases.get(def.operands[side]);
  if (writes.get(run.lowest.index) >= def.start) return def.value;
  const get = index => module.local.get(index, binaryen.i32);
  const common = module.i32.add(get(def.operands[1 - side]), get(run.lowest.index));
  // Keep the displacement in i32 arithmetic so the effective address wraps
  // exactly as it did before, including at the end of the address space.
  const delta = field.offset - run.lowest.offset;
  return delta ? module.i32.add(common, module.i32.const(delta)) : common;
}

function testsConstantStore({ condition, block, start }, stores) {
  if (binaryen.getExpressionId(condition) !== binaryen.BinaryId) return false;
  const left = binaryen.Binary.getLeft(condition);
  const right = binaryen.Binary.getRight(condition);
  const load = binaryen.getExpressionId(left) === binaryen.ConstId ? right
    : binaryen.getExpressionId(right) === binaryen.ConstId ? left : 0;
  if (!load || binaryen.getExpressionId(load) !== binaryen.LoadId) return false;
  const ptr = binaryen.Load.getPtr(load);
  if (binaryen.getExpressionId(ptr) !== binaryen.LocalGetId) return false;
  const store = stores.get(binaryen.LocalGet.getIndex(ptr));
  return store && store.block === block && store.start < start &&
    store.offset === binaryen.Load.getOffset(load);
}

// SSA gives each write a fresh local but preserves the address additions.
// Count those definitions first so only candidate functions pay for SSA and cleanup.
function hasAddressGroup(body) {
  let found = false;
  function visit(expr, loop) {
    const id = binaryen.getExpressionId(expr);
    if (id === binaryen.LoopId) loop = { addresses: 0, decision: false };
    if (loop) {
      if (id === binaryen.LocalSetId && addressSum(expr)) loop.addresses++;
      if (id === binaryen.IfId && binaryen.If.getIfFalse(expr)) loop.decision = true;
      if (loop.decision && loop.addresses >= MIN_LIVE_ADDRESSES) {
        found = true;
        return;
      }
    }
    rewriteExpressionChildren(expr, child => { if (!found) visit(child, loop); });
  }
  visit(body, null);
  return found;
}

export function optimizeGuardedAddresses(module, func) {
  // Also bounds expression depth for Binaryen's emitter on the skipped path.
  const body = prepareFunctionBody(module, func);
  if (!body || binaryen.Function.getNumLocals(func) < MIN_LIVE_ADDRESSES || !hasAddressGroup(body)) return;
  module.runPassesOnFunction(func, ["ssa-nomerge"]);
  const ssa = prepareFunctionBody(module, func);
  if (ssa) sinkGuardedAddresses(module, ssa);
  module.runPassesOnFunction(func, ["code-pushing", "simplify-locals", "coalesce-locals", "vacuum"]);
  prepareFunctionBody(module, func);
}
