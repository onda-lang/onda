import binaryen from "binaryen";

// Children are listed in Wasm evaluation order. A missing layout means the
// pass cannot prove it has seen every access in this function.
function fields(api, ...names) {
  return names.map(name => ({ get: api["get" + name], set: api["set" + name] }));
}

function indexed(api, count, element) {
  return {
    count: api["getNum" + count],
    get: api["get" + element + "At"],
    set: api["set" + element + "At"],
  };
}

const layouts = new Map([
  [binaryen.BlockId, [indexed(binaryen.Block, "Children", "Child")]],
  [binaryen.IfId, fields(binaryen.If, "Condition", "IfTrue", "IfFalse")],
  [binaryen.LoopId, fields(binaryen.Loop, "Body")],
  [binaryen.BreakId, fields(binaryen.Break, "Value", "Condition")],
  [binaryen.SwitchId, fields(binaryen.Switch, "Value", "Condition")],
  [binaryen.CallId, [indexed(binaryen.Call, "Operands", "Operand")]],
  [binaryen.CallIndirectId, [
    indexed(binaryen.CallIndirect, "Operands", "Operand"), ...fields(binaryen.CallIndirect, "Target"),
  ]],
  [binaryen.LocalSetId, fields(binaryen.LocalSet, "Value")],
  [binaryen.GlobalSetId, fields(binaryen.GlobalSet, "Value")],
  [binaryen.LoadId, fields(binaryen.Load, "Ptr")],
  [binaryen.StoreId, fields(binaryen.Store, "Ptr", "Value")],
  [binaryen.UnaryId, fields(binaryen.Unary, "Value")],
  [binaryen.BinaryId, fields(binaryen.Binary, "Left", "Right")],
  [binaryen.SelectId, fields(binaryen.Select, "IfTrue", "IfFalse", "Condition")],
  [binaryen.DropId, fields(binaryen.Drop, "Value")],
  [binaryen.ReturnId, fields(binaryen.Return, "Value")],
  [binaryen.MemoryCopyId, fields(binaryen.MemoryCopy, "Dest", "Source", "Size")],
  [binaryen.MemoryFillId, fields(binaryen.MemoryFill, "Dest", "Value", "Size")],
  [binaryen.SIMDExtractId, fields(binaryen.SIMDExtract, "Vec")],
  [binaryen.SIMDReplaceId, fields(binaryen.SIMDReplace, "Vec", "Value")],
  [binaryen.SIMDShuffleId, fields(binaryen.SIMDShuffle, "Left", "Right")],
  [binaryen.SIMDTernaryId, fields(binaryen.SIMDTernary, "A", "B", "C")],
  [binaryen.SIMDShiftId, fields(binaryen.SIMDShift, "Vec", "Shift")],
  [binaryen.SIMDLoadId, fields(binaryen.SIMDLoad, "Ptr")],
  [binaryen.SIMDLoadStoreLaneId, fields(binaryen.SIMDLoadStoreLane, "Ptr", "Vec")],
  [binaryen.TupleMakeId, [indexed(binaryen.TupleMake, "Operands", "Operand")]],
  [binaryen.TupleExtractId, fields(binaryen.TupleExtract, "Tuple")],
  ...[binaryen.ConstId, binaryen.LocalGetId, binaryen.GlobalGetId, binaryen.NopId,
    binaryen.UnreachableId, binaryen.MemorySizeId, binaryen.DataDropId].map(id => [id, []]),
]);

// A visitor can return a replacement for a child. Read-only passes leave the
// tree untouched. Unknown node kinds invalidate whole-function analyses.
export function rewriteExpressionChildren(expr, visit) {
  const layout = layouts.get(binaryen.getExpressionId(expr));
  if (!layout) return false;
  for (const slot of layout) {
    if (slot.count) {
      const count = slot.count(expr);
      for (let i = 0; i < count; i++) {
        const child = slot.get(expr, i);
        const value = visit(child);
        if (value !== undefined && value !== child) slot.set(expr, i, value);
      }
    } else {
      const child = slot.get(expr);
      if (!child) continue;
      const value = visit(child);
      if (value !== undefined && value !== child) slot.set(expr, value);
    }
  }
  return true;
}

// Leave headroom for the recursive JS optimization passes and Binaryen 132's
// emitter. Inspect one level at a time so even a fully inlined tree is safe.
const MAX_EXPRESSION_DEPTH = 256;

function inspectBody(body) {
  let level = [body];
  let next = [];
  let supported = true;
  const enqueue = child => { next.push(child); };
  for (let depth = 1; level.length; depth++) {
    if (depth > MAX_EXPRESSION_DEPTH) return "deep";
    for (const expr of level) {
      if (!rewriteExpressionChildren(expr, enqueue)) supported = false;
    }
    level.length = 0;
    [level, next] = [next, level];
  }
  return supported ? "supported" : "unknown";
}

export function prepareFunctionBody(module, func) {
  let body = binaryen.Function.getBody(func);
  if (!body) return 0;
  let shape = inspectBody(body);
  if (shape === "deep") {
    // Keep operands flat. Drop unused temporaries before coalescing so the
    // interference analysis only sees locals that still occur in the body.
    module.runPassesOnFunction(func, [
      "flatten", "simplify-locals-nonesting", "reorder-locals", "coalesce-locals", "vacuum",
    ]);
    body = binaryen.Function.getBody(func);
    shape = inspectBody(body);
  }
  return shape === "supported" ? body : 0;
}
