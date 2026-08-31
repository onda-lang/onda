import binaryen from "binaryen";
import {
  SUPPORTED_MIR_SCHEMA_VERSION,
  OndaBinaryenError,
  ONDA_MATH_KERNEL_WASM,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  validateProcessorMetadata,
  PAGE_BYTES,
  STATIC_BASE,
  MATH_KERNEL_RESERVED_END,
  MATH_KERNEL_DATA_SEGMENT,
  MATH_KERNEL_STACK_GLOBAL,
  MAX_MEMORY_PAGES,
  DEFAULT_OPTIMIZE_LEVEL,
  ONDA_PROCESS_FULL_BLOCK,
  DELEGATE_BATCH_USED_OFFSET,
  DELEGATE_BATCH_RECORD_COUNT_OFFSET,
  DELEGATE_BATCH_OVERFLOW_OFFSET,
  EXECUTION_OUTPUT_DELEGATE_BATCH_OFFSET,
  EXECUTION_OUTPUT_PRINT_BATCH_OFFSET,
  EXECUTION_OUTPUT_SEQUENCE_OFFSET,
  RUNTIME_FAILURE_GLOBAL,
  INIT_ALL_GLOBAL,
  POINTER_GLOBALS,
  BUFFER_DESCRIPTOR_POINTER_GLOBALS,
  TRAPPING_DESCRIPTOR_UNARY_OPS,
  TRAPPING_DESCRIPTOR_BINARY_OPS,
  collectMathKernelHelpers,
  alignUp,
  encodeScalarValues,
} from "./shared.js";

export class MirCompilerCore {
  constructor(mir, options) {
    this.mir = mir;
    this.options = {
      optimize: options.optimize !== false,
      emitText: options.emitText === true,
      optimizeLevel: options.optimizeLevel ?? DEFAULT_OPTIMIZE_LEVEL,
      shrinkLevel: options.shrinkLevel ?? 0,
      fastMath: options.fastMath === true,
      simd: options.simd !== false,
      allowInliningFunctionsWithLoops:
        options.allowInliningFunctionsWithLoops === true,
    };
    this.module = new binaryen.Module();
    this.functionNames = [];
    this.stateLayout = [];
    this.paramLayout = [];
    this.inputLayout = [];
    this.outputLayout = [];
    this.controlOutputLayout = [];
    this.eventLayout = [];
    this.constLayout = [];
    this.localArrayLayout = [];
    this.localScalarRefLayout = [];
    this.memorySegments = [];
    this.requiredMathHelpers = collectMathKernelHelpers(mir);
    this.nextStaticAddress = this.requiredMathHelpers.size > 0
      ? MATH_KERNEL_RESERVED_END
      : STATIC_BASE;
    this.internalHelpers = new Set();
    this.functionMayFail = [];
    this.bufferMayWrite = [];
    this.fallbackBufferReadAddress = 0;
    this.fallbackBufferWriteAddress = 0;
    this.scalarParameterByValue = [];
    this.nextLabel = 0;
  }

  compile() {
    try {
      this.validateEnvelope();
      this.buildLayouts();
      this.addMathKernel();
      this.addMemoryAndContextGlobals();
      this.addMirFunctions();
      this.addAbiWrappers();

      if (!this.module.validate()) {
        throw new OndaBinaryenError("Binaryen rejected the generated WebAssembly module");
      }
      if (this.options.optimize) {
        const previousOptimizeLevel = binaryen.getOptimizeLevel();
        const previousShrinkLevel = binaryen.getShrinkLevel();
        const previousFastMath = binaryen.getFastMath();
        const previousLoopInlining =
          binaryen.getAllowInliningFunctionsWithLoops();
        try {
          binaryen.setOptimizeLevel(this.options.optimizeLevel);
          binaryen.setShrinkLevel(this.options.shrinkLevel);
          binaryen.setFastMath(this.options.fastMath);
          binaryen.setAllowInliningFunctionsWithLoops(
            this.options.allowInliningFunctionsWithLoops,
          );
          this.module.optimize();
          if (this.hoistInvariantBufferDescriptorLoads()) {
            // The first rewrite makes descriptor provenance explicit in
            // locals. A small cleanup is enough to expose aliases that were
            // shared by Binaryen's original loop body; one final rewrite then
            // catches those without paying for a second full O4 pipeline.
            this.module.runPasses([
              "simplify-locals",
              "optimize-instructions",
              "coalesce-locals",
              "vacuum",
            ]);
            if (this.hoistInvariantBufferDescriptorLoads()) {
              this.module.runPasses(["vacuum"]);
            }
          }
        } finally {
          binaryen.setOptimizeLevel(previousOptimizeLevel);
          binaryen.setShrinkLevel(previousShrinkLevel);
          binaryen.setFastMath(previousFastMath);
          binaryen.setAllowInliningFunctionsWithLoops(previousLoopInlining);
        }
        if (!this.module.validate()) {
          throw new OndaBinaryenError(
            "Binaryen rejected the optimized WebAssembly module",
          );
        }
      }

      const wasm = this.module.emitBinary();
      const result = {
        wasm,
        metadata: this.buildMetadata(),
      };
      if (this.options.emitText) {
        result.wat = this.module.emitText();
      }
      // Binaryen already validated the module above. Validate the descriptor here
      // without asking the JavaScript engine to compile the Wasm a second time.
      validateProcessorMetadata(result.metadata, "webassembly_module");
      return result;
    } finally {
      this.module.dispose();
    }
  }

  hoistInvariantBufferDescriptorLoads() {
    // Binaryen must conservatively assume that arbitrary linear-memory stores
    // can rewrite host descriptor tables. After inlining, recover the stronger
    // processor ABI contract explicitly: descriptor bindings are immutable for
    // one entry-point invocation, so address-invariant loads belong in the loop
    // preheader. Sample-varying addresses remain untouched.
    this.descriptorLoadsHoisted = 0;
    for (let index = 0; index < this.module.getNumFunctions(); index += 1) {
      const func = this.module.getFunctionByIndex(index);
      const body = binaryen.Function.getBody(func);
      // The local-write scan below is part of the safety proof. If Binaryen
      // adds an expression kind that this backend does not know how to walk,
      // leave the whole function untouched rather than silently overlooking
      // a nested local.tee.
      if (!this.visitExpression(body, () => {})) continue;
      const rewritten = this.rewriteDescriptorLoops(body, func);
      if (rewritten !== body) binaryen.Function.setBody(func, rewritten);
    }
    return this.descriptorLoadsHoisted > 0;
  }

  rewriteDescriptorLoops(expression, func) {
    this.rewriteExpressionChildren(expression, (child) =>
      this.rewriteDescriptorLoops(child, func)
    );
    if (binaryen.getExpressionInfo(expression).id !== binaryen.LoopId) {
      return expression;
    }

    const body = binaryen.Loop.getBody(expression);
    const controlPaths = this.descriptorControlPaths(body);
    const definitions = new Map();
    const writtenLocals = new Set();
    this.visitExpression(body, (candidate) => {
      const info = binaryen.getExpressionInfo(candidate);
      if (info.id !== binaryen.LocalSetId) return;
      writtenLocals.add(info.index);
      const entries = definitions.get(info.index) ?? [];
      entries.push(info.value);
      definitions.set(info.index, entries);
    });
    const initializers = [];
    // A pointer-local tee can expose the same invariant address to later
    // descriptor loads. Cache its value in a fresh local: assigning the
    // original local in the preheader would change loop-entry semantics.
    const loopLocalCaches = new Map();

    const rewriteLoad = (candidate) => {
      this.rewriteExpressionChildren(candidate, rewriteLoad);
      const info = binaryen.getExpressionInfo(candidate);
      if (info.id !== binaryen.LoadId || info.isAtomic) return candidate;
      const candidatePath = controlPaths.get(candidate) ?? [];
      const localCache = (local) => {
        const cache = loopLocalCaches.get(local);
        return cache && this.descriptorPathDominates(cache.path, candidatePath)
          ? cache
          : null;
      };
      if (!this.descriptorPointerExpression(
        info.ptr,
        definitions,
        (local) => !writtenLocals.has(local) || localCache(local) !== null,
      )) {
        return candidate;
      }
      this.cacheDescriptorPointerSideEffects(
        info.ptr,
        func,
        initializers,
        loopLocalCaches,
        controlPaths,
        candidatePath,
      );
      // Binaryen exposes FunctionAddVar through its generated C-API surface,
      // but not through the small Function convenience wrapper.
      const cache = binaryen._BinaryenFunctionAddVar(func, info.type);
      this.descriptorLoadsHoisted += 1;
      const hoistedLoad = this.module.copyExpression(candidate);
      const hoistedInfo = binaryen.getExpressionInfo(hoistedLoad);
      binaryen.Load.setPtr(
        hoistedLoad,
        this.descriptorPointerForPreheader(
          hoistedInfo.ptr,
          loopLocalCaches,
          candidatePath,
        ),
      );
      initializers.push(this.module.local.set(cache, hoistedLoad));
      const sideEffects = this.descriptorPointerSideEffects(info.ptr);
      const value = this.module.local.get(cache, info.type);
      return sideEffects.length === 0
        ? value
        : this.module.block(null, [...sideEffects, value], info.type);
    };
    const rewrittenBody = rewriteLoad(body);
    if (rewrittenBody !== body) binaryen.Loop.setBody(expression, rewrittenBody);
    return initializers.length === 0
      ? expression
      : this.module.block(null, [...initializers, expression]);
  }

  descriptorControlPaths(expression) {
    const paths = new Map();
    const visit = (candidate, path) => {
      paths.set(candidate, path);
      const info = binaryen.getExpressionInfo(candidate);
      if (info.id === binaryen.IfId) {
        visit(info.condition, path);
        visit(info.ifTrue, [...path, `if:${candidate}:true`]);
        if (info.ifFalse) {
          visit(info.ifFalse, [...path, `if:${candidate}:false`]);
        }
        return;
      }
      if (info.id === binaryen.LoopId) {
        visit(info.body, [...path, `loop:${candidate}`]);
        return;
      }
      this.rewriteExpressionChildren(candidate, (child) => {
        visit(child, path);
        return child;
      });
    };
    visit(expression, []);
    return paths;
  }

  descriptorPathDominates(dominator, candidate) {
    // Rewrite traversal is in evaluation order, so an available cache is
    // earlier than the candidate. The path prefix additionally proves that
    // it was not produced only in a sibling branch or nested loop.
    return dominator.length <= candidate.length
      && dominator.every((entry, index) => entry === candidate[index]);
  }

  cacheDescriptorPointerSideEffects(
    expression,
    func,
    initializers,
    loopLocalCaches,
    controlPaths,
    candidatePath,
  ) {
    const info = binaryen.getExpressionInfo(expression);
    if (info.id === binaryen.LocalSetId && info.isTee) {
      this.cacheDescriptorPointerSideEffects(
        info.value,
        func,
        initializers,
        loopLocalCaches,
        controlPaths,
        candidatePath,
      );
      const cache = binaryen._BinaryenFunctionAddVar(func, info.type);
      const value = this.descriptorPointerForPreheader(
        this.module.copyExpression(info.value),
        loopLocalCaches,
        candidatePath,
      );
      initializers.push(this.module.local.set(cache, value));
      loopLocalCaches.set(info.index, {
        index: cache,
        type: info.type,
        path: controlPaths.get(expression) ?? candidatePath,
      });
      return;
    }
    if (info.id === binaryen.UnaryId) {
      this.cacheDescriptorPointerSideEffects(
        info.value,
        func,
        initializers,
        loopLocalCaches,
        controlPaths,
        candidatePath,
      );
      return;
    }
    if (info.id === binaryen.BinaryId) {
      for (const child of [info.left, info.right]) {
        this.cacheDescriptorPointerSideEffects(
          child,
          func,
          initializers,
          loopLocalCaches,
          controlPaths,
          candidatePath,
        );
      }
    }
  }

  descriptorPointerForPreheader(expression, loopLocalCaches, candidatePath) {
    this.rewriteExpressionChildren(expression, (child) =>
      this.descriptorPointerForPreheader(
        child,
        loopLocalCaches,
        candidatePath,
      )
    );
    const info = binaryen.getExpressionInfo(expression);
    if (info.id === binaryen.LocalGetId) {
      const cache = loopLocalCaches.get(info.index);
      if (cache && this.descriptorPathDominates(cache.path, candidatePath)) {
        return this.module.local.get(cache.index, cache.type);
      }
    }
    if (info.id === binaryen.LocalSetId && info.isTee) {
      const cache = loopLocalCaches.get(info.index);
      if (cache && this.descriptorPathDominates(cache.path, candidatePath)) {
        return this.module.local.get(cache.index, cache.type);
      }
      return info.value;
    }
    return expression;
  }

  descriptorPointerExpression(expression, definitions, localIsInvariant) {
    if (!this.expressionUsesDescriptorTable(expression, definitions, new Set())) {
      return false;
    }
    const visit = (candidate) => {
      const info = binaryen.getExpressionInfo(candidate);
      if (info.id === binaryen.ConstId) return true;
      if (info.id === binaryen.LocalGetId) return localIsInvariant(info.index);
      if (info.id === binaryen.GlobalGetId) {
        if (BUFFER_DESCRIPTOR_POINTER_GLOBALS.has(info.name)) {
          return true;
        }
        return false;
      }
      if (info.id === binaryen.LocalSetId && info.isTee) {
        return visit(info.value);
      }
      if (info.id === binaryen.UnaryId) {
        return !TRAPPING_DESCRIPTOR_UNARY_OPS.has(info.op)
          && visit(info.value);
      }
      if (info.id === binaryen.BinaryId) {
        return !TRAPPING_DESCRIPTOR_BINARY_OPS.has(info.op)
          && visit(info.left)
          && visit(info.right);
      }
      if (info.id === binaryen.SelectId) {
        if (
          this.expressionContainsTee(info.condition)
          || this.expressionContainsTee(info.ifTrue)
          || this.expressionContainsTee(info.ifFalse)
        ) {
          return false;
        }
        return visit(info.condition) && visit(info.ifTrue) && visit(info.ifFalse);
      }
      return false;
    };
    return visit(expression);
  }

  expressionUsesDescriptorTable(expression, definitions, visitingLocals) {
    const info = binaryen.getExpressionInfo(expression);
    if (info.id === binaryen.GlobalGetId) {
      return BUFFER_DESCRIPTOR_POINTER_GLOBALS.has(info.name);
    }
    if (info.id === binaryen.LocalGetId) {
      const values = definitions.get(info.index);
      if (
        !values
        || values.length !== 1
        || visitingLocals.has(info.index)
      ) {
        return false;
      }
      visitingLocals.add(info.index);
      const result = this.expressionUsesDescriptorTable(
        values[0],
        definitions,
        visitingLocals,
      );
      visitingLocals.delete(info.index);
      return result;
    }
    if (info.id === binaryen.LocalSetId && info.isTee) {
      return this.expressionUsesDescriptorTable(
        info.value,
        definitions,
        visitingLocals,
      );
    }
    if (info.id === binaryen.UnaryId) {
      return this.expressionUsesDescriptorTable(
        info.value,
        definitions,
        visitingLocals,
      );
    }
    if (info.id === binaryen.BinaryId) {
      return this.expressionUsesDescriptorTable(
        info.left,
        definitions,
        visitingLocals,
      ) || this.expressionUsesDescriptorTable(
        info.right,
        definitions,
        visitingLocals,
      );
    }
    if (info.id === binaryen.SelectId) {
      return this.expressionUsesDescriptorTable(
        info.condition,
        definitions,
        visitingLocals,
      ) || this.expressionUsesDescriptorTable(
        info.ifTrue,
        definitions,
        visitingLocals,
      ) || this.expressionUsesDescriptorTable(
        info.ifFalse,
        definitions,
        visitingLocals,
      );
    }
    return false;
  }

  descriptorPointerSideEffects(expression) {
    const result = [];
    const visit = (candidate) => {
      const info = binaryen.getExpressionInfo(candidate);
      if (info.id === binaryen.LocalSetId && info.isTee) {
        result.push(
          this.module.local.set(info.index, this.module.copyExpression(info.value)),
        );
      } else if (info.id === binaryen.UnaryId) {
        visit(info.value);
      } else if (info.id === binaryen.BinaryId) {
        visit(info.left);
        visit(info.right);
      } else if (info.id === binaryen.SelectId) {
        // Pointer selectors generated by this backend are side-effect free.
        // A nested tee would need conditional reconstruction, so leave it to
        // the conservative invariance check instead of moving it here.
      }
    };
    visit(expression);
    return result;
  }

  expressionContainsTee(expression) {
    const info = binaryen.getExpressionInfo(expression);
    if (info.id === binaryen.LocalSetId) return info.isTee;
    if (info.id === binaryen.UnaryId) {
      return this.expressionContainsTee(info.value);
    }
    if (info.id === binaryen.BinaryId) {
      return this.expressionContainsTee(info.left)
        || this.expressionContainsTee(info.right);
    }
    if (info.id === binaryen.SelectId) {
      return this.expressionContainsTee(info.condition)
        || this.expressionContainsTee(info.ifTrue)
        || this.expressionContainsTee(info.ifFalse);
    }
    return false;
  }

  visitExpression(expression, visitor) {
    visitor(expression);
    let complete = true;
    const supported = this.rewriteExpressionChildren(expression, (child) => {
      if (!this.visitExpression(child, visitor)) complete = false;
      return child;
    });
    return complete && supported;
  }

  rewriteExpressionChildren(expression, rewrite) {
    const info = binaryen.getExpressionInfo(expression);
    const replace = (child, setter) => {
      if (child) setter(rewrite(child));
    };
    switch (info.id) {
      case binaryen.BlockId:
        info.children.forEach((child, index) =>
          replace(child, (value) => binaryen.Block.setChildAt(expression, index, value))
        );
        break;
      case binaryen.IfId:
        replace(info.condition, (value) => binaryen.If.setCondition(expression, value));
        replace(info.ifTrue, (value) => binaryen.If.setIfTrue(expression, value));
        replace(info.ifFalse, (value) => binaryen.If.setIfFalse(expression, value));
        break;
      case binaryen.LoopId:
        replace(info.body, (value) => binaryen.Loop.setBody(expression, value));
        break;
      case binaryen.BreakId:
        replace(info.condition, (value) => binaryen.Break.setCondition(expression, value));
        replace(info.value, (value) => binaryen.Break.setValue(expression, value));
        break;
      case binaryen.SwitchId:
        replace(info.condition, (value) => binaryen.Switch.setCondition(expression, value));
        replace(info.value, (value) => binaryen.Switch.setValue(expression, value));
        break;
      case binaryen.CallId:
        info.operands.forEach((child, index) =>
          replace(child, (value) => binaryen.Call.setOperandAt(expression, index, value))
        );
        break;
      case binaryen.CallIndirectId:
        replace(info.target, (value) => binaryen.CallIndirect.setTarget(expression, value));
        info.operands.forEach((child, index) =>
          replace(child, (value) => binaryen.CallIndirect.setOperandAt(expression, index, value))
        );
        break;
      case binaryen.LocalSetId:
        replace(info.value, (value) => binaryen.LocalSet.setValue(expression, value));
        break;
      case binaryen.GlobalSetId:
        replace(info.value, (value) => binaryen.GlobalSet.setValue(expression, value));
        break;
      case binaryen.LoadId:
        replace(info.ptr, (value) => binaryen.Load.setPtr(expression, value));
        break;
      case binaryen.StoreId:
        replace(info.ptr, (value) => binaryen.Store.setPtr(expression, value));
        replace(info.value, (value) => binaryen.Store.setValue(expression, value));
        break;
      case binaryen.UnaryId:
        replace(info.value, (value) => binaryen.Unary.setValue(expression, value));
        break;
      case binaryen.BinaryId:
        replace(info.left, (value) => binaryen.Binary.setLeft(expression, value));
        replace(info.right, (value) => binaryen.Binary.setRight(expression, value));
        break;
      case binaryen.SelectId:
        replace(info.ifTrue, (value) => binaryen.Select.setIfTrue(expression, value));
        replace(info.ifFalse, (value) => binaryen.Select.setIfFalse(expression, value));
        replace(info.condition, (value) => binaryen.Select.setCondition(expression, value));
        break;
      case binaryen.DropId:
        replace(info.value, (value) => binaryen.Drop.setValue(expression, value));
        break;
      case binaryen.ReturnId:
        replace(info.value, (value) => binaryen.Return.setValue(expression, value));
        break;
      case binaryen.MemoryCopyId:
        replace(info.dest, (value) => binaryen.MemoryCopy.setDest(expression, value));
        replace(info.source, (value) => binaryen.MemoryCopy.setSource(expression, value));
        replace(info.size, (value) => binaryen.MemoryCopy.setSize(expression, value));
        break;
      case binaryen.MemoryFillId:
        replace(info.dest, (value) => binaryen.MemoryFill.setDest(expression, value));
        replace(info.value, (value) => binaryen.MemoryFill.setValue(expression, value));
        replace(info.size, (value) => binaryen.MemoryFill.setSize(expression, value));
        break;
      case binaryen.SIMDExtractId:
        replace(info.vec, (value) => binaryen.SIMDExtract.setVec(expression, value));
        break;
      case binaryen.SIMDReplaceId:
        replace(info.vec, (value) => binaryen.SIMDReplace.setVec(expression, value));
        replace(info.value, (value) => binaryen.SIMDReplace.setValue(expression, value));
        break;
      case binaryen.SIMDShuffleId:
        replace(info.left, (value) => binaryen.SIMDShuffle.setLeft(expression, value));
        replace(info.right, (value) => binaryen.SIMDShuffle.setRight(expression, value));
        break;
      case binaryen.SIMDTernaryId:
        replace(info.a, (value) => binaryen.SIMDTernary.setA(expression, value));
        replace(info.b, (value) => binaryen.SIMDTernary.setB(expression, value));
        replace(info.c, (value) => binaryen.SIMDTernary.setC(expression, value));
        break;
      case binaryen.SIMDShiftId:
        replace(info.vec, (value) => binaryen.SIMDShift.setVec(expression, value));
        replace(info.shift, (value) => binaryen.SIMDShift.setShift(expression, value));
        break;
      case binaryen.SIMDLoadId:
        replace(info.ptr, (value) => binaryen.SIMDLoad.setPtr(expression, value));
        break;
      case binaryen.SIMDLoadStoreLaneId:
        replace(info.ptr, (value) => binaryen.SIMDLoadStoreLane.setPtr(expression, value));
        replace(info.vec, (value) => binaryen.SIMDLoadStoreLane.setVec(expression, value));
        break;
      case binaryen.TupleMakeId:
        info.operands.forEach((child, index) =>
          replace(child, (value) => binaryen.TupleMake.setOperandAt(expression, index, value))
        );
        break;
      case binaryen.TupleExtractId:
        replace(info.tuple, (value) => binaryen.TupleExtract.setTuple(expression, value));
        break;
      case binaryen.ConstId:
      case binaryen.LocalGetId:
      case binaryen.GlobalGetId:
      case binaryen.NopId:
      case binaryen.UnreachableId:
      case binaryen.MemorySizeId:
      case binaryen.DataDropId:
        break;
      default:
        return false;
    }
    return true;
  }

  validateEnvelope() {
    const mir = this.mir;
    if (!mir || typeof mir !== "object" || Array.isArray(mir)) {
      this.fail("MIR must be a JSON object");
    }
    if (mir.schema_version !== SUPPORTED_MIR_SCHEMA_VERSION) {
      this.fail(
        `unsupported MIR schema version ${String(mir.schema_version)}; expected ${SUPPORTED_MIR_SCHEMA_VERSION}`,
      );
    }
    for (const field of ["types", "state", "const_data", "functions"]) {
      if (!Array.isArray(mir[field])) {
        this.fail(`MIR field '${field}' must be an array`);
      }
    }
    if (!mir.interface || typeof mir.interface !== "object") {
      this.fail("MIR field 'interface' must be an object");
    }
    for (const field of [
      "inputs",
      "outputs",
      "control_outputs",
      "params",
      "buffers",
      "events",
      "delegates",
    ]) {
      if (!Array.isArray(mir.interface[field])) {
        this.fail(`MIR interface field '${field}' must be an array`);
      }
    }
    if (!mir.entry_points || !Number.isInteger(mir.entry_points.init)) {
      this.fail("MIR entry_points are missing or invalid");
    }
    if (!Number.isInteger(mir.entry_points.process)) {
      this.fail("MIR process entry point is missing or invalid");
    }
    if (!Number.isInteger(mir.config?.block_size) || mir.config.block_size <= 0) {
      this.fail("MIR block size must be a positive integer");
    }
    if (mir.config.block_size > 0x7fff_ffff) {
      this.fail("MIR block size must fit the signed i32 process ABI");
    }
    if (
      !Number.isInteger(this.options.optimizeLevel) ||
      this.options.optimizeLevel < 0 ||
      this.options.optimizeLevel > 4
    ) {
      this.fail("Binaryen optimizeLevel must be an integer from 0 through 4");
    }
    if (
      !Number.isInteger(this.options.shrinkLevel) ||
      this.options.shrinkLevel < 0 ||
      this.options.shrinkLevel > 2
    ) {
      this.fail("Binaryen shrinkLevel must be an integer from 0 through 2");
    }
    this.validateCurrentSchemaEnvelope();
    this.validateProcessEntrySignature();
    this.validateAcyclicCallGraph();
    this.analyzeBufferWrites();
    this.analyzeRecoverableFailures();
    this.analyzeScalarReferenceParameters();
  }

  analyzeScalarReferenceParameters() {
    // MIR reference modes are conservative. Internal scalar references that
    // are never written, never escape to a writable reference, and never alias
    // a writable argument can safely use a value ABI. This removes scratch
    // memory traffic and exposes their values to post-inlining loop analysis.
    const candidates = this.mir.functions.map((func) =>
      func.params.map((parameter) => {
        const type = this.type(parameter.ty);
        return type.kind === "scalar" && parameter.mode !== "value";
      })
    );
    const callSites = this.mir.functions.map(() => []);

    const visitBlock = (functionId, block) => {
      for (const statement of block.statements) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (
          kind === "assign"
          && data.destination?.base?.kind === "parameter"
          && data.destination.projections.length === 0
        ) {
          candidates[functionId][data.destination.base.data] = false;
        } else if (kind === "call") {
          callSites[data.function].push({ caller: functionId, call: data });
        } else if (kind === "if") {
          visitBlock(functionId, data.then_block);
          visitBlock(functionId, data.else_block);
        } else if (kind === "loop") {
          visitBlock(functionId, data.body);
        }
      }
    };
    for (const [functionId, func] of this.mir.functions.entries()) {
      visitBlock(functionId, func.body);
    }

    const unprojectedScalarPlace = (argument, functionId) => {
      if (
        argument?.kind !== "place"
        || argument.data.projections.length !== 0
        || !["local", "parameter"].includes(argument.data.base.kind)
      ) {
        return null;
      }
      const typeId = argument.data.base.kind === "local"
        ? this.mir.functions[functionId].locals[argument.data.base.data]?.ty
        : this.mir.functions[functionId].params[argument.data.base.data]?.ty;
      return Number.isInteger(typeId) && this.type(typeId).kind === "scalar"
        ? argument.data.base
        : null;
    };
    const sameBase = (lhs, rhs) =>
      lhs?.kind === rhs?.kind && lhs?.data === rhs?.data;

    let changed = true;
    while (changed) {
      changed = false;
      for (const [calleeId, sites] of callSites.entries()) {
        for (const { caller: callerId, call } of sites) {
          for (let parameterId = 0; parameterId < candidates[calleeId].length; parameterId += 1) {
            if (!candidates[calleeId][parameterId]) continue;
            const base = unprojectedScalarPlace(call.args[parameterId], callerId);
            const forwardedCandidate =
              base?.kind !== "parameter"
              || candidates[callerId][base.data];
            const aliasesWritableArgument = call.args.some((argument, index) => {
              if (index === parameterId || candidates[calleeId][index]) return false;
              return sameBase(
                base,
                unprojectedScalarPlace(argument, callerId),
              );
            });
            if (!base || !forwardedCandidate || aliasesWritableArgument) {
              candidates[calleeId][parameterId] = false;
              changed = true;
            }
          }
        }
      }

      for (const [callerId, func] of this.mir.functions.entries()) {
        const visitCalls = (block) => {
          for (const statement of block.statements) {
            const kind = statement.kind?.kind;
            const data = statement.kind?.data;
            if (kind === "call") {
              data.args.forEach((argument, parameterId) => {
                const base = unprojectedScalarPlace(argument, callerId);
                if (
                  base?.kind === "parameter"
                  && candidates[callerId][base.data]
                  && !candidates[data.function][parameterId]
                ) {
                  candidates[callerId][base.data] = false;
                  changed = true;
                }
              });
            } else if (kind === "if") {
              visitCalls(data.then_block);
              visitCalls(data.else_block);
            } else if (kind === "loop") {
              visitCalls(data.body);
            }
          }
        };
        visitCalls(func.body);
      }
    }
    this.scalarParameterByValue = candidates;
  }

  parameterPassingMode(functionId, parameterId) {
    return this.scalarParameterByValue[functionId]?.[parameterId]
      ? "value"
      : this.mir.functions[functionId].params[parameterId].mode;
  }

  validateCurrentSchemaEnvelope() {
    const persistenceKinds = new Set([
      "snapshot",
      "instance_scratch",
      "control_mirror",
    ]);
    for (const [stateId, slot] of this.mir.state.entries()) {
      if (typeof slot?.authored !== "boolean") {
        this.fail(`state slot ${stateId} has an invalid authored flag`);
      }
      if (slot?.pinned !== undefined && typeof slot.pinned !== "boolean") {
        this.fail(`state slot ${stateId} has an invalid pinned flag`);
      }
      if (!persistenceKinds.has(slot?.persistence)) {
        this.fail(
          `state slot ${stateId} has invalid persistence '${String(slot?.persistence)}'`,
        );
      }
    }

    const mirrors = new Set();
    for (const [outputId, output] of this.mir.interface.control_outputs.entries()) {
      if (
        !Number.isInteger(output?.mirror) ||
        output.mirror < 0 ||
        output.mirror >= this.mir.state.length
      ) {
        this.fail(`control output ${outputId} has an invalid mirror state id`);
      }
      if (mirrors.has(output.mirror)) {
        this.fail(`control output ${outputId} reuses mirror state ${output.mirror}`);
      }
      mirrors.add(output.mirror);
      const slot = this.mir.state[output.mirror];
      if (slot.persistence !== "control_mirror") {
        this.fail(
          `control output ${outputId} mirror state ${output.mirror} is not control_mirror storage`,
        );
      }
      if (!this.typesEquivalent(output.ty, slot.ty)) {
        this.fail(
          `control output ${outputId} type does not match mirror state ${output.mirror}`,
        );
      }
    }

    const origins = new Set(["source", "compiler_generated"]);
    const inlineHints = new Set(["auto", "always", "never"]);
    for (const [functionId, func] of this.mir.functions.entries()) {
      if (
        !func?.attributes ||
        !origins.has(func.attributes.origin) ||
        !inlineHints.has(func.attributes.inline) ||
        typeof func.attributes.runtime_context !== "boolean"
      ) {
        this.fail(
          `function ${functionId} has invalid schema-${SUPPORTED_MIR_SCHEMA_VERSION} attributes`,
        );
      }
    }
  }

  validateProcessEntrySignature() {
    const processId = this.mir.entry_points.process;
    this.requireFunctionId(processId, "process entry point");
    const process = this.mir.functions[processId];
    if (process?.kind?.kind !== "process") {
      this.fail("MIR process entry point must have process function kind");
    }
    if (!Array.isArray(process.params) || process.params.length !== 3) {
      this.fail(
        "MIR process entry point must have exactly three parameters (start_frame, frames, flags)",
      );
    }
    if (!Array.isArray(process.results) || process.results.length !== 0) {
      this.fail("MIR process entry point must not return values");
    }

    const names = ["start_frame", "frames", "flags"];
    for (const [index, name] of names.entries()) {
      const parameter = process.params[index];
      if (parameter?.name !== name) {
        this.fail(`MIR process parameter ${index} must be named '${name}'`);
      }
      if (parameter.mode !== "value") {
        this.fail(`MIR process parameter '${name}' must use value passing mode`);
      }
      const type = this.mir.types[parameter.ty];
      if (type?.kind !== "scalar" || type.data !== "i32") {
        this.fail(`MIR process parameter '${name}' must have type i32`);
      }
    }
  }

  validateAcyclicCallGraph() {
    const functionCount = this.mir.functions.length;
    const collectCalls = (block, callees) => {
      for (const statement of block?.statements ?? []) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (kind === "call") {
          if (
            Number.isInteger(data?.function)
            && data.function >= 0
            && data.function < functionCount
          ) {
            callees.add(data.function);
          }
        } else if (kind === "if") {
          collectCalls(data?.then_block, callees);
          collectCalls(data?.else_block, callees);
        } else if (kind === "loop") {
          collectCalls(data?.body, callees);
        }
      }
    };

    const edges = this.mir.functions.map((func) => {
      const callees = new Set();
      collectCalls(func.body, callees);
      return [...callees].sort((lhs, rhs) => lhs - rhs);
    });
    const visits = new Uint8Array(functionCount);
    const path = [];
    const visit = (functionId) => {
      if (visits[functionId] === 2) return null;
      if (visits[functionId] === 1) {
        const start = Math.max(0, path.indexOf(functionId));
        return [...path.slice(start), functionId];
      }
      visits[functionId] = 1;
      path.push(functionId);
      for (const callee of edges[functionId]) {
        const cycle = visit(callee);
        if (cycle) return cycle;
      }
      path.pop();
      visits[functionId] = 2;
      return null;
    };

    for (let functionId = 0; functionId < functionCount; functionId += 1) {
      const cycle = visit(functionId);
      if (cycle) {
        const display = cycle
          .map((id) => this.mir.functions[id]?.name ?? `@fn${id}`)
          .join(" -> ");
        this.fail(`recursive call cycle is not realtime-safe: ${display}`);
      }
    }
  }

  analyzeBufferWrites() {
    const bufferOrigin = (id) => `buffer:${id}`;
    const parameterOrigin = (id, slot = 0) => `parameter:${id}:${slot}`;
    const selectedSlots = (selector, len, bounds) => {
      if (!Number.isInteger(len) || len <= 0) return [];
      if (
        selector?.kind === "constant"
        && selector.data?.type === "i32"
        && Number.isInteger(selector.data.value)
      ) {
        let slot = selector.data.value;
        if (bounds === "clamp") {
          slot = Math.min(len - 1, Math.max(0, slot));
          return [slot];
        }
        if (slot >= 0 && slot < len) return [slot];
      }
      return Array.from({ length: len }, (_, slot) => slot);
    };
    const bufferIds = (bufferRef) => {
      if (Number.isInteger(bufferRef)) return [bufferRef];
      if (bufferRef?.kind === "direct" && Number.isInteger(bufferRef.data)) {
        return [bufferRef.data];
      }
      if (
        bufferRef?.kind === "array_element"
        && Number.isInteger(bufferRef.data?.first)
        && Number.isInteger(bufferRef.data?.len)
        && bufferRef.data.len > 0
      ) {
        return selectedSlots(
          bufferRef.data.selector,
          bufferRef.data.len,
          bufferRef.data.bounds,
        ).map(
          (slot) => bufferRef.data.first + slot,
        );
      }
      return [];
    };
    const bufferOrigins = (bufferRef) =>
      new Set(bufferIds(bufferRef).map(bufferOrigin));
    const bufferParamSlots = (parameterRef, func) => {
      if (Number.isInteger(parameterRef)) {
        return [{ parameter: parameterRef, slot: 0 }];
      }
      if (
        parameterRef?.kind === "direct"
        && Number.isInteger(parameterRef.data)
      ) {
        return [{ parameter: parameterRef.data, slot: 0 }];
      }
      if (
        parameterRef?.kind === "array_element"
        && Number.isInteger(parameterRef.data?.span)
      ) {
        const parameter = func.params?.[parameterRef.data.span];
        const type = parameter && this.type(parameter.ty);
        const len = type?.kind === "buffer_span" ? type.data.len : 1;
        return selectedSlots(
          parameterRef.data.selector,
          len,
          parameterRef.data.bounds,
        ).map((slot) => ({ parameter: parameterRef.data.span, slot }));
      }
      return [];
    };
    const bufferParamOrigins = (parameterRef, func) => new Set(
      bufferParamSlots(parameterRef, func).map(({ parameter, slot }) =>
        parameterOrigin(parameter, slot)
      ),
    );
    const localId = (value) =>
      value?.kind === "local" && Number.isInteger(value.data)
        ? value.data
        : null;
    const setEquals = (lhs, rhs) =>
      lhs.size === rhs.size && [...lhs].every((entry) => rhs.has(entry));
    const summaryEquals = (lhs, rhs) =>
      setEquals(lhs.buffers, rhs.buffers)
      && setEquals(lhs.parameters, rhs.parameters);

    const valueOrigins = (value, aliases) => {
      const id = localId(value);
      return id === null ? new Set() : new Set(aliases[id] ?? []);
    };
    const placeOrigins = (place, aliases) => {
      const base = place?.base;
      if (base?.kind === "parameter" && Number.isInteger(base.data)) {
        return new Set([parameterOrigin(base.data, 0)]);
      }
      if (base?.kind === "local" && Number.isInteger(base.data)) {
        return new Set(aliases[base.data] ?? []);
      }
      return new Set();
    };
    const rvalueOrigins = (value, aliases, func) => {
      if (value?.kind === "use") {
        return valueOrigins(value.data, aliases);
      }
      if (value?.kind === "load") {
        return placeOrigins(value.data, aliases);
      }
      if (value?.kind !== "make_slice") {
        return new Set();
      }
      const source = value.data?.source;
      if (source?.kind === "buffer") {
        return bufferOrigins(source.data?.buffer);
      }
      if (source?.kind === "buffer_param") {
        return bufferParamOrigins(source.data?.parameter, func);
      }
      if (source?.kind === "place") {
        return placeOrigins(source.data, aliases);
      }
      return new Set();
    };
    const collectAliases = (func) => {
      const aliases = (func.locals ?? []).map(() => new Set());
      let changed = true;
      const visitBlock = (block) => {
        for (const statement of block?.statements ?? []) {
          const kind = statement.kind?.kind;
          const data = statement.kind?.data;
          if (
            kind === "assign"
            && data?.destination?.projections?.length === 0
            && data.destination.base?.kind === "local"
          ) {
            const destination = data.destination.base.data;
            const origins = rvalueOrigins(data.value, aliases, func);
            for (const origin of origins) {
              if (!aliases[destination].has(origin)) {
                aliases[destination].add(origin);
                changed = true;
              }
            }
          } else if (kind === "if") {
            visitBlock(data?.then_block);
            visitBlock(data?.else_block);
          } else if (kind === "loop") {
            visitBlock(data?.body);
          }
        }
      };
      while (changed) {
        changed = false;
        visitBlock(func.body);
      }
      return aliases;
    };
    const collectUnsupportedResults = (func) => {
      const results = new Set();
      const visitBlock = (block) => {
        for (const statement of block?.statements ?? []) {
          const kind = statement.kind?.kind;
          const data = statement.kind?.data;
          if (kind === "call") {
            for (const result of data?.results ?? []) {
              const type = this.mir.types[func.locals?.[result]?.ty];
              if (type?.kind === "slice" || type?.kind === "buffer") {
                results.add(result);
              }
            }
          } else if (kind === "if") {
            visitBlock(data?.then_block);
            visitBlock(data?.else_block);
          } else if (kind === "loop") {
            visitBlock(data?.body);
          }
        }
      };
      visitBlock(func.body);
      return results;
    };
    const argumentValue = (argument) => {
      switch (argument?.kind) {
        case "value": return argument.data;
        case "slice_element":
        case "slice_window": return argument.data?.slice;
        default: return null;
      }
    };
    const argumentUsesUnsupportedResult = (argument, unsupported) => {
      const value = argumentValue(argument);
      const valueLocal = localId(value);
      if (valueLocal !== null) {
        return unsupported.has(valueLocal);
      }
      if (argument?.kind === "place") {
        const base = argument.data?.base;
        return base?.kind === "local" && unsupported.has(base.data);
      }
      if (argument?.kind === "array_window") {
        const base = argument.data?.array?.base;
        return base?.kind === "local" && unsupported.has(base.data);
      }
      return false;
    };
    const argumentOrigins = (argument, aliases, func, slot) => {
      switch (argument?.kind) {
        case "buffer":
          return bufferOrigins(argument.data);
        case "buffer_param":
          return bufferParamOrigins(argument.data, func);
        case "buffer_span": {
          const span = argument.data;
          if (span?.kind === "interface") {
            const len = span.data?.len ?? 0;
            if (Number.isInteger(slot) && slot >= 0 && slot < len) {
              return new Set([bufferOrigin(span.data.first + slot)]);
            }
            return new Set(Array.from({ length: len }, (_, index) =>
              bufferOrigin(span.data.first + index)
            ));
          }
          if (span?.kind === "parameter") {
            const start = span.data?.start ?? 0;
            const len = span.data?.len ?? 0;
            if (Number.isInteger(slot) && slot >= 0 && slot < len) {
              return new Set([parameterOrigin(span.data.span, start + slot)]);
            }
            return new Set(Array.from({ length: len }, (_, index) =>
              parameterOrigin(span.data.span, start + index)
            ));
          }
          return new Set();
        }
        case "place":
          return placeOrigins(argument.data, aliases);
        case "array_window":
          return placeOrigins(argument.data?.array, aliases);
        case "value":
          return valueOrigins(argument.data, aliases);
        case "slice_element":
        case "slice_window":
          return valueOrigins(argument.data?.slice, aliases);
        default:
          return new Set();
      }
    };
    const markOrigins = (origins, summary) => {
      for (const origin of origins) {
        const [kind, idText] = origin.split(":");
        const id = Number(idText);
        if (kind === "buffer") {
          summary.buffers.add(id);
        } else if (kind === "parameter") {
          summary.parameters.add(origin);
        }
      }
    };

    const aliases = this.mir.functions.map(collectAliases);
    const unsupported = this.mir.functions.map(collectUnsupportedResults);
    let summaries = this.mir.functions.map(() => ({
      buffers: new Set(),
      parameters: new Set(),
    }));
    while (true) {
      const next = this.mir.functions.map((func, functionId) => {
        const summary = { buffers: new Set(), parameters: new Set() };
        const markValueWrite = (value, description) => {
          const id = localId(value);
          if (id !== null && unsupported[functionId].has(id)) {
            this.fail(
              `cannot infer interface-buffer writes for ${description} through a slice returned by a MIR call`,
            );
          }
          markOrigins(valueOrigins(value, aliases[functionId]), summary);
        };
        const visitBlock = (block) => {
          for (const statement of block?.statements ?? []) {
            const kind = statement.kind?.kind;
            const data = statement.kind?.data;
            if (
              kind === "assign"
              && data?.destination?.base?.kind === "parameter"
            ) {
              summary.parameters.add(parameterOrigin(data.destination.base.data, 0));
            } else if (kind === "buffer_store") {
              for (const buffer of bufferIds(data?.buffer)) {
                summary.buffers.add(buffer);
              }
            } else if (kind === "buffer_param_store") {
              markOrigins(
                bufferParamOrigins(data?.parameter, func),
                summary,
              );
            } else if (kind === "slice_store") {
              markValueWrite(data?.slice, "slice store");
            } else if (kind === "slice_fill" || kind === "slice_copy") {
              markValueWrite(data?.destination, "slice write");
            } else if (kind === "call") {
              const callee = summaries[data?.function];
              if (!callee) {
                this.fail(
                  `MIR call references missing function ${String(data?.function)}`,
                );
              }
              for (const buffer of callee.buffers) {
                summary.buffers.add(buffer);
              }
              for (const parameterOriginValue of callee.parameters) {
                const [, parameterText, slotText] = parameterOriginValue.split(":");
                const parameter = Number(parameterText);
                const slot = Number(slotText);
                const argument = data?.args?.[parameter];
                if (!argument) {
                  this.fail(
                    `MIR call to function ${data.function} has no argument for writable parameter ${parameter}`,
                  );
                }
                if (
                  argumentUsesUnsupportedResult(
                    argument,
                    unsupported[functionId],
                  )
                ) {
                  this.fail(
                    "cannot infer interface-buffer writes through a slice or buffer returned by a MIR call",
                  );
                }
                markOrigins(
                  argumentOrigins(argument, aliases[functionId], func, slot),
                  summary,
                );
              }
            } else if (kind === "if") {
              visitBlock(data?.then_block);
              visitBlock(data?.else_block);
            } else if (kind === "loop") {
              visitBlock(data?.body);
            }
          }
        };
        visitBlock(func.body);
        return summary;
      });
      if (next.every((summary, index) => summaryEquals(summary, summaries[index]))) {
        summaries = next;
        break;
      }
      summaries = next;
    }

    const roots = [
      this.mir.entry_points.init,
      this.mir.entry_points.process,
      ...this.mir.interface.events.map((event) => event.handler),
    ];
    this.bufferMayWrite = this.mir.interface.buffers.map(() => false);
    for (const root of roots) {
      const summary = summaries[root];
      if (!summary) {
        this.fail(`MIR buffer-write root function ${String(root)} is missing`);
      }
      for (const bufferId of summary.buffers) {
        if (
          !Number.isInteger(bufferId)
          || bufferId < 0
          || bufferId >= this.bufferMayWrite.length
        ) {
          this.fail(
            `MIR buffer-write analysis references missing buffer ${String(bufferId)}`,
          );
        }
        this.bufferMayWrite[bufferId] = true;
      }
    }
    for (const [bufferId, mayWrite] of this.bufferMayWrite.entries()) {
      if (mayWrite && this.mir.interface.buffers[bufferId].access !== "read_write") {
        this.fail(
          `MIR writes read-only interface buffer '${this.mir.interface.buffers[bufferId].name}'`,
        );
      }
    }
  }

  analyzeRecoverableFailures() {
    const callees = this.mir.functions.map(() => new Set());
    const direct = this.mir.functions.map(() => false);
    const binaryMayFail = (functionId, value) => {
      if (
        value.kind !== "binary"
        || !["divide", "remainder"].includes(value.data?.op)
      ) {
        return false;
      }
      const lhs = value.data.lhs;
      const scalar = lhs.kind === "constant"
        ? lhs.data.type
        : this.requireScalarType(
            this.mir.functions[functionId].locals[lhs.data].ty,
            `binary operand in '${this.mir.functions[functionId].name}'`,
          );
      return scalar === "i32" || scalar === "i64";
    };
    const checkedBoundsKinds = new Set([
      "array_window",
      "buffer_load",
      "buffer_param_load",
      "buffer_param_store",
      "buffer_store",
      "const_data_load",
      "index",
      "input_load",
      "make_slice",
      "output_load",
      "output_store",
      "control_output_store",
    ]);
    const dynamicBoundsKinds = new Set([
      "slice_element",
      "slice_load",
      "slice_store",
      "slice_window",
    ]);
    const scan = (functionId, value) => {
      if (value === null || value === undefined) return;
      if (Array.isArray(value)) {
        for (const entry of value) scan(functionId, entry);
        return;
      }
      if (typeof value !== "object") return;
      if (value.kind === "call" && Number.isInteger(value.data?.function)) {
        callees[functionId].add(value.data.function);
      }
      const fixedDelegatePayloadMayFail = value.kind === "publish_delegate"
        && this.mir.interface.delegates[value.data?.delegate]?.params.some(
          (param) => this.type(param.ty).kind === "array",
        );
      const bounds = value.data?.bounds;
      if (
        value.kind === "process_frame"
        || value.kind === "slice_copy"
        || fixedDelegatePayloadMayFail
        || binaryMayFail(functionId, value)
        || (checkedBoundsKinds.has(value.kind) && bounds === "checked")
        || (dynamicBoundsKinds.has(value.kind) && bounds !== "unchecked")
      ) {
        direct[functionId] = true;
      }
      for (const child of Object.values(value)) scan(functionId, child);
    };
    for (const [functionId, func] of this.mir.functions.entries()) {
      scan(functionId, func.body);
    }
    this.functionMayFail = [...direct];
    let changed = true;
    while (changed) {
      changed = false;
      for (const [functionId, targets] of callees.entries()) {
        if (
          !this.functionMayFail[functionId]
          && [...targets].some((target) => this.functionMayFail[target])
        ) {
          this.functionMayFail[functionId] = true;
          changed = true;
        }
      }
    }
  }

  buildLayouts() {
    this.stateLayout = this.layoutNamedValues(this.mir.state);
    this.paramLayout = this.layoutNamedValues(this.mir.interface.params);
    this.inputLayout = this.layoutPorts(this.mir.interface.inputs);
    this.outputLayout = this.layoutPorts(this.mir.interface.outputs);
    this.controlOutputLayout = this.layoutControlOutputs();
    this.eventLayout = this.mir.interface.events.map((event) =>
      this.layoutEventValues(event.params),
    );
    this.delegateLayout = this.mir.interface.delegates.map((delegate) =>
      this.layoutEventValues(delegate.params),
    );
    this.requireWasm32Extent(
      this.stateLayout.byteLength,
      "MIR physical state storage",
    );
    this.requireWasm32Extent(
      this.paramLayout.byteLength,
      "MIR parameter storage",
    );
    for (const [eventId, layout] of this.eventLayout.entries()) {
      this.requireWasm32Extent(
        layout.minimumByteLength,
        `MIR event ${eventId} fixed payload storage`,
      );
    }
    for (const [delegateId, layout] of this.delegateLayout.entries()) {
      this.requireWasm32Extent(
        layout.minimumByteLength,
        `MIR delegate ${delegateId} fixed payload storage`,
      );
    }

    for (let id = 0; id < this.mir.const_data.length; id += 1) {
      const data = this.mir.const_data[id];
      const scalar = data.element;
      const size = this.scalarSize(scalar);
      this.nextStaticAddress = alignUp(this.nextStaticAddress, size);
      const address = this.nextStaticAddress;
      const bytes = encodeScalarValues(data.values, scalar, this);
      this.memorySegments.push({
        offset: this.module.i32.const(address),
        data: bytes,
      });
      this.constLayout.push({ address, scalar, len: data.values.length });
      this.nextStaticAddress += bytes.byteLength;
    }

    this.localArrayLayout = this.mir.functions.map((func) =>
      func.locals.map((local) => {
        const type = this.type(local.ty);
        if (type.kind !== "array") return null;
        const layout = this.typeLayout(local.ty);
        this.nextStaticAddress = alignUp(this.nextStaticAddress, layout.align);
        const address = this.nextStaticAddress;
        this.nextStaticAddress += layout.size;
        return { ...layout, address };
      }),
    );
    this.localScalarRefLayout = this.mir.functions.map((func, functionId) => {
      const addressTaken = this.collectAddressTakenScalarLocals(functionId);
      return func.locals.map((local, localId) => {
        if (!addressTaken.has(localId)) return null;
        const type = this.type(local.ty);
        if (type.kind !== "scalar") return null;
        const size = this.scalarSize(type.data);
        this.nextStaticAddress = alignUp(this.nextStaticAddress, size);
        const address = this.nextStaticAddress;
        this.nextStaticAddress += size;
        return { address, scalar: type.data, size };
      });
    });
    if (this.mir.interface.buffers.length > 0) {
      const fallbackBytes = Math.max(
        ...this.mir.interface.buffers.map((buffer) =>
          this.scalarSize(buffer.element)),
      );
      this.nextStaticAddress = alignUp(this.nextStaticAddress, 8);
      this.fallbackBufferReadAddress = this.nextStaticAddress;
      this.nextStaticAddress += fallbackBytes;
      this.nextStaticAddress = alignUp(this.nextStaticAddress, 8);
      this.fallbackBufferWriteAddress = this.nextStaticAddress;
      this.nextStaticAddress += fallbackBytes;
    }
    this.nextStaticAddress = alignUp(this.nextStaticAddress, 16);
    this.requireWasm32Extent(this.nextStaticAddress, "MIR static storage");
    this.requireWasm32Extent(
      this.nextStaticAddress
        + this.paramLayout.byteLength
        + this.stateLayout.byteLength,
      "MIR static, parameter, and physical state storage",
    );
  }

  collectAddressTakenScalarLocals(functionId) {
    const result = new Set();
    const visitBlock = (block) => {
      for (const statement of block.statements) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (kind === "call") {
          const target = this.mir.functions[data.function];
          data.args.forEach((argument, index) => {
            const parameter = target?.params[index];
            const type = parameter && this.type(parameter.ty);
            if (
              this.parameterPassingMode(data.function, index) !== "value"
              && type?.kind === "scalar"
              && argument.kind === "place"
              && argument.data.base.kind === "local"
              && argument.data.projections.length === 0
            ) {
              result.add(argument.data.base.data);
            }
          });
        } else if (kind === "if") {
          visitBlock(data.then_block);
          visitBlock(data.else_block);
        } else if (kind === "loop") {
          visitBlock(data.body);
        }
      }
    };
    visitBlock(this.mir.functions[functionId].body);
    return result;
  }

  layoutNamedValues(values) {
    let offset = 0;
    const result = [];
    for (const value of values) {
      const layout = this.typeLayout(value.ty);
      offset = alignUp(offset, layout.align);
      result.push({ ...layout, offset });
      offset += layout.size;
    }
    result.byteLength = alignUp(offset, 16);
    return result;
  }

  layoutControlOutputs() {
    return this.mir.interface.control_outputs.map((output) => {
      const layout = this.typeLayout(output.ty);
      return {
        ...layout,
        offset: this.stateLayout[output.mirror].offset,
      };
    });
  }

  layoutEventValues(values) {
    let offset = 0;
    let dynamic = false;
    const result = values.map((value) => {
      const type = this.type(value.ty);
      if (type.kind === "slice") {
        const entry = {
          offset: dynamic ? null : offset,
          size: null,
          dynamic: true,
          headerSize: 4,
          scalar: type.data.element,
        };
        offset += 4;
        dynamic = true;
        return entry;
      }
      const layout = this.typeLayout(value.ty);
      const entry = { ...layout, offset: dynamic ? null : offset, dynamic: false };
      offset += layout.size;
      return entry;
    });
    result.byteLength = dynamic ? null : offset;
    result.minimumByteLength = offset;
    result.dynamic = dynamic;
    return result;
  }

  layoutPorts(ports) {
    let channel = 0;
    return ports.map((port, portId) => {
      const type = this.type(port.ty);
      const flattened = this.flattenPortType(type);
      this.requireWasm32Extent(
        this.scalarSize(flattened.scalar) * this.mir.config.block_size,
        `MIR audio port ${portId} channel storage`,
      );
      const result = {
        channel,
        channels: flattened.channels,
        scalar: flattened.scalar,
        size: this.scalarSize(flattened.scalar),
        isArray: type.kind === "array",
      };
      channel += flattened.channels;
      return result;
    });
  }

  flattenPortType(type) {
    if (type.kind === "scalar") {
      return { scalar: type.data, channels: 1 };
    }
    if (type.kind === "array") {
      const element = this.type(type.data.element);
      if (element.kind !== "scalar") {
        this.fail("nested aggregate audio ports are not supported yet");
      }
      return { scalar: element.data, channels: type.data.len };
    }
    this.fail(`audio port type '${type.kind}' is not supported yet`);
  }

  addMemoryAndContextGlobals() {
    const initialPages = Math.max(
      1,
      Math.ceil(this.nextStaticAddress / PAGE_BYTES),
    );
    if (initialPages > MAX_MEMORY_PAGES) {
      this.fail(
        `MIR static storage requires ${initialPages} Wasm pages, exceeding the Wasm32 limit`,
      );
    }
    this.module.setMemory(
      initialPages,
      MAX_MEMORY_PAGES,
      "memory",
      this.memorySegments,
    );
    this.module.addGlobal(
      "__heap_base",
      binaryen.i32,
      false,
      this.module.i32.const(this.nextStaticAddress),
    );
    this.module.addGlobalExport("__heap_base", "__heap_base");

    for (const name of Object.values(POINTER_GLOBALS)) {
      this.module.addGlobal(name, binaryen.i32, true, this.module.i32.const(0));
    }
    this.module.addGlobal(
      RUNTIME_FAILURE_GLOBAL,
      binaryen.i32,
      true,
      this.module.i32.const(0),
    );
    this.module.addGlobal(
      INIT_ALL_GLOBAL,
      binaryen.i32,
      true,
      this.module.i32.const(0),
    );
  }

  addMathKernel() {
    if (this.requiredMathHelpers.size === 0) return;

    const source = binaryen.readBinary(ONDA_MATH_KERNEL_WASM);
    try {
      if (
        source.getNumGlobals() !== 1
        || source.getNumTables() !== 0
        || source.getNumDataSegments() !== 1
      ) {
        this.fail("embedded Wasm math kernel has an unsupported module shape");
      }

      for (let index = source.getNumExports() - 1; index >= 0; index -= 1) {
        const exported = binaryen.getExportInfo(source.getExportByIndex(index));
        if (!this.requiredMathHelpers.has(exported.name)) {
          source.removeExport(exported.name);
        }
      }
      source.runPasses(["remove-unused-module-elements"]);

      if (source.getNumGlobals() > 1 || source.getNumDataSegments() > 1) {
        this.fail("optimized Wasm math kernel has an unsupported module shape");
      }
      if (source.getNumGlobals() === 1) {
        const global = binaryen.getGlobalInfo(source.getGlobalByIndex(0));
        if (
          global.module
          || global.name !== MATH_KERNEL_STACK_GLOBAL
          || global.type !== binaryen.i32
          || !global.mutable
        ) {
          this.fail("embedded Wasm math kernel has an invalid stack global");
        }
        this.module.addGlobal(
          global.name,
          global.type,
          global.mutable,
          this.module.copyExpression(global.init),
        );
      }

      if (source.getNumDataSegments() === 1) {
        const segment = source.getDataSegmentInfo(source.getDataSegmentByIndex(0));
        if (
          segment.name !== MATH_KERNEL_DATA_SEGMENT
          || segment.passive
          || !Number.isInteger(segment.offset)
          || segment.offset < STATIC_BASE
          || segment.offset + segment.data.byteLength > MATH_KERNEL_RESERVED_END
        ) {
          this.fail("embedded Wasm math kernel exceeds its reserved memory region");
        }
        this.memorySegments.push({
          offset: this.module.i32.const(segment.offset),
          data: new Uint8Array(segment.data),
        });
      }

      this.module.setFeatures(this.module.getFeatures() | source.getFeatures());
      for (let index = 0; index < source.getNumFunctions(); index += 1) {
        const func = binaryen.getFunctionInfo(source.getFunctionByIndex(index));
        if (func.module || !func.body) {
          this.fail("embedded Wasm math kernel must not import functions");
        }
        this.module.addFunction(
          func.name,
          func.params,
          func.results,
          func.vars,
          this.module.copyExpression(func.body),
        );
      }
    } finally {
      source.dispose();
    }
  }

  addMirFunctions() {
    this.functionNames = this.mir.functions.map((_, id) => `$onda.fn.${id}`);
    for (let id = 0; id < this.mir.functions.length; id += 1) {
      this.addMirFunction(id, this.mir.functions[id]);
    }
  }

  addMirFunction(id, func) {
    let nextIndex = 0;
    const paramLayouts = func.params.map((param, parameterId) => {
      const layout = this.functionValueLayout(
        param.ty,
        nextIndex,
        `parameter '${param.name}'`,
        false,
        this.parameterPassingMode(id, parameterId),
      );
      nextIndex += layout.components.length;
      return layout;
    });
    const paramScalars = paramLayouts.flatMap((layout) => layout.components);
    const localLayouts = func.locals.map((local, localId) => {
      const layout = this.functionValueLayout(
        local.ty,
        nextIndex,
        `local ${localId} of '${func.name}'`,
        true,
      );
      nextIndex += layout.components.length;
      return layout;
    });
    const flatLocalScalars = localLayouts.flatMap((layout) => layout.components);
    const localScalars = func.locals.map((local) => {
      const type = this.type(local.ty);
      return type.kind === "scalar" ? type.data : null;
    });
    const resultScalars = func.results.map((result, resultId) =>
      this.requireScalarType(result, `result ${resultId} of '${func.name}'`),
    );
    const callResultLocals = this.collectCallResultLocals(func);
    const sliceScratch = this.collectSliceScratchLocals(func);
    const processFrameLocals = this.collectProcessFrameLocals(func);
    const generatedLocalBase =
      paramScalars.length +
      flatLocalScalars.length +
      callResultLocals.length +
      sliceScratch.count;
    if (
      resultScalars.length > 1
      || callResultLocals.some((entry) => entry.resultCount > 1)
    ) {
      this.module.setFeatures(
        this.module.getFeatures() | binaryen.Features.Multivalue,
      );
    }
    const context = {
      function: func,
      functionId: id,
      paramScalars,
      paramLayouts,
      localScalars,
      localLayouts,
      flatLocalCount: flatLocalScalars.length,
      callResultLocals: new Map(
        callResultLocals.map((entry, index) => [
          entry.call,
          {
            index: paramScalars.length + flatLocalScalars.length + index,
            type: entry.type,
          },
        ]),
      ),
      sliceScratch: new Map(
        sliceScratch.entries.map((entry, index) => [
          entry.statement,
          {
            index:
              paramScalars.length +
              flatLocalScalars.length +
              callResultLocals.length +
              sliceScratch.offsets[index],
            count: entry.count,
          },
        ]),
      ),
      eventId: func.kind?.kind === "event" ? func.kind.data : null,
      processFrameLocals,
      generatedLocalBase,
      generatedLocals: [],
      entryInitializers: [],
      bufferDescriptorCache: new Map(),
      audioChannelPointerCache: new Map(),
      breakLabels: [],
      continueLabels: [],
    };
    const compiledBody = this.compileBlock(func.body, context);
    const body = context.entryInitializers.length === 0
      ? compiledBody
      : this.module.block(null, [
          ...context.entryInitializers,
          compiledBody,
        ]);
    const functionRef = this.module.addFunction(
      this.functionNames[id],
      binaryen.createType(paramScalars.map((type) => this.wasmType(type))),
      this.wasmResultType(resultScalars),
      [
        ...flatLocalScalars.map((type) => this.wasmType(type)),
        ...callResultLocals.map((entry) => entry.type),
        ...Array.from({ length: sliceScratch.count }, () => binaryen.i32),
        ...context.generatedLocals.map((entry) => this.wasmType(entry.scalar)),
      ],
      body,
    );
    for (let paramId = 0; paramId < func.params.length; paramId += 1) {
      this.setFunctionValueNames(
        functionRef,
        paramLayouts[paramId],
        `${func.params[paramId].name}.arg`,
      );
    }
    for (let localId = 0; localId < func.locals.length; localId += 1) {
      const name = func.locals[localId].name;
      if (name) {
        // Source names can repeat across disjoint lexical scopes while
        // Binaryen requires every debug local name in a function to be unique.
        // Keep the source spelling readable and make identity explicit with
        // the deterministic MIR local ID.
        this.setFunctionValueNames(
          functionRef,
          localLayouts[localId],
          `${name}.local${localId}`,
        );
      }
    }
    for (const local of context.generatedLocals) {
      binaryen.Function.setLocalName(functionRef, local.index, local.name);
    }
  }

  functionValueLayout(
    typeId,
    index,
    description,
    allowStorageOnly = false,
    passingMode = "value",
  ) {
    const type = this.type(typeId);
    if (type.kind === "scalar") {
      if (passingMode !== "value") {
        return {
          index,
          typeId,
          kind: "scalar_ref",
          scalar: type.data,
          components: ["i32"],
        };
      }
      return { index, typeId, kind: "scalar", components: [type.data] };
    }
    if (type.kind === "slice") {
      return {
        index,
        typeId,
        kind: "slice",
        components: ["i32", "i32", "i32", "i32"],
      };
    }
    if (type.kind === "buffer") {
      this.bufferChannelMetadata(type.data.channels, type.data.element);
      return {
        index,
        typeId,
        kind: "buffer",
        components: ["i32", "i32", "i32", "i32", "f32", "i32"],
      };
    }
    if (type.kind === "buffer_span") {
      this.bufferChannelMetadata(type.data.channels, type.data.element);
      if (passingMode !== "value") {
        this.fail(`${description} buffer span must use value passing mode`);
      }
      return {
        index,
        typeId,
        kind: "buffer_span",
        components: ["i32", "i32", "i32", "i32", "i32", "i32"],
      };
    }
    if (type.kind === "array") {
      if (passingMode !== "value") {
        return { index, typeId, kind: "array_ref", components: ["i32"] };
      }
      if (allowStorageOnly) {
        return { index, typeId, kind: "array", components: [] };
      }
    }
    this.fail(`${description} has unsupported function value type '${type.kind}'`);
  }

  setFunctionValueNames(functionRef, layout, name) {
    if (layout.kind === "scalar") {
      binaryen.Function.setLocalName(functionRef, layout.index, name);
      return;
    }
    if (layout.kind === "scalar_ref") {
      binaryen.Function.setLocalName(functionRef, layout.index, `${name}.address`);
      return;
    }
    if (layout.kind === "array_ref") {
      binaryen.Function.setLocalName(functionRef, layout.index, `${name}.address`);
      return;
    }
    if (layout.kind === "array") return;
    const suffixes = layout.kind === "buffer"
      ? ["read_address", "write_address", "frames", "channels", "sample_rate", "bound"]
      : layout.kind === "buffer_span"
        ? ["read_table", "write_table", "frames_table", "channels_table", "sample_rates_table", "bound_table"]
        : ["read_address", "write_address", "length", "stride"];
    for (const [offset, suffix] of suffixes.entries()) {
      binaryen.Function.setLocalName(
        functionRef,
        layout.index + offset,
        `${name}.${suffix}`,
      );
    }
  }

  collectCallResultLocals(func) {
    const result = [];
    const visitBlock = (block) => {
      for (const statement of block.statements) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (kind === "call" && data.results.length > 0) {
          this.requireFunctionId(data.function, "call target");
          const target = this.mir.functions[data.function];
          const aliasesResult = data.args.some((argument, index) =>
            this.parameterPassingMode(data.function, index) !== "value"
              && argument.kind === "place"
              && argument.data.base.kind === "local"
              && argument.data.projections.length === 0
              && this.type(target.params[index].ty).kind === "scalar"
          );
          if (data.results.length === 1 && !aliasesResult) continue;
          const scalars = target.results.map((typeId, resultId) =>
            this.requireScalarType(
              typeId,
              `result ${resultId} of '${target.name}'`,
            ),
          );
          result.push({
            call: data,
            resultCount: scalars.length,
            type: binaryen.createType(
              scalars.map((scalar) => this.wasmType(scalar)),
            ),
          });
        } else if (kind === "if") {
          visitBlock(data.then_block);
          visitBlock(data.else_block);
        } else if (kind === "loop") {
          visitBlock(data.body);
        }
      }
    };
    visitBlock(func.body);
    return result;
  }

  collectSliceScratchLocals(func) {
    const entries = [];
    const offsets = [];
    let count = 0;
    const visitBlock = (block) => {
      for (const statement of block.statements) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (kind === "slice_fill" || kind === "slice_copy") {
          offsets.push(count);
          const scratchCount = kind === "slice_copy" ? 2 : 1;
          entries.push({ statement, count: scratchCount });
          count += scratchCount;
        } else if (kind === "if") {
          visitBlock(data.then_block);
          visitBlock(data.else_block);
        } else if (kind === "loop") {
          visitBlock(data.body);
        }
      }
    };
    visitBlock(func.body);
    return { entries, offsets, count };
  }

  collectProcessFrameLocals(func) {
    const definitions = Array.from({ length: func.locals.length }, () => 0);
    const candidates = new Set();
    const visitBlock = (block) => {
      for (const statement of block.statements) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (kind === "assign" && data.destination?.base?.kind === "local") {
          const localId = data.destination.base.data;
          if (
            Number.isInteger(localId) &&
            localId >= 0 &&
            localId < definitions.length
          ) {
            definitions[localId] += 1;
            if (
              data.destination.projections.length === 0 &&
              data.value?.kind === "process_frame"
            ) {
              candidates.add(localId);
            }
          }
        } else if (kind === "call") {
          for (const localId of data.results) {
            if (
              Number.isInteger(localId) &&
              localId >= 0 &&
              localId < definitions.length
            ) {
              definitions[localId] += 1;
            }
          }
        } else if (kind === "if") {
          visitBlock(data.then_block);
          visitBlock(data.else_block);
        } else if (kind === "loop") {
          visitBlock(data.body);
        }
      }
    };
    visitBlock(func.body);
    return new Set(
      [...candidates].filter((localId) => definitions[localId] === 1),
    );
  }

  defaultFunctionResult(context) {
    const scalars = context.function.results.map((typeId, resultId) =>
      this.requireScalarType(
        typeId,
        `result ${resultId} of '${context.function.name}'`,
      ),
    );
    const values = scalars.map((scalar) => this.zero(scalar));
    if (values.length === 0) return undefined;
    if (values.length === 1) return values[0];
    return this.module.tuple.make(values);
  }

  returnFromCurrentFunction(context) {
    return this.module.return(this.defaultFunctionResult(context));
  }

  raiseRuntimeFailure(context) {
    return this.module.block(null, [
      this.module.global.set(
        RUNTIME_FAILURE_GLOBAL,
        this.module.i32.const(PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE),
      ),
      ...this.resetDelegateBatch(),
      this.returnFromCurrentFunction(context),
    ]);
  }

  propagateRuntimeFailure(calleeId, context) {
    if (!this.functionMayFail[calleeId]) return [];
    return [
      this.module.if(
        this.module.i32.ne(
          this.module.global.get(RUNTIME_FAILURE_GLOBAL, binaryen.i32),
          this.module.i32.const(PROCESSOR_EXECUTION_OK),
        ),
        this.returnFromCurrentFunction(context),
      ),
    ];
  }

  resetRuntimeFailure(functionId) {
    if (!this.functionMayFail[functionId]) return [];
    return [
      this.module.global.set(
        RUNTIME_FAILURE_GLOBAL,
        this.module.i32.const(PROCESSOR_EXECUTION_OK),
      ),
    ];
  }

  executionStatus(functionId) {
    return this.functionMayFail[functionId]
      ? this.module.global.get(RUNTIME_FAILURE_GLOBAL, binaryen.i32)
      : this.module.i32.const(PROCESSOR_EXECUTION_OK);
  }

  resetDelegateBatch() {
    const batch = () =>
      this.module.global.get(POINTER_GLOBALS.delegateBatch, binaryen.i32);
    const stores = [
      DELEGATE_BATCH_USED_OFFSET,
      DELEGATE_BATCH_RECORD_COUNT_OFFSET,
      DELEGATE_BATCH_OVERFLOW_OFFSET,
    ].map((offset) =>
      this.module.i32.store(
        offset,
        4,
        batch(),
        this.module.i32.const(0),
      )
    );
    return [
      this.module.if(
        this.module.i32.ne(batch(), this.module.i32.const(0)),
        this.module.block(null, stores),
      ),
    ];
  }

  executionOutputBatch(outputLocal, fieldOffset) {
    const output = () => this.module.local.get(outputLocal, binaryen.i32);
    return this.module.if(
      this.module.i32.ne(output(), this.module.i32.const(0)),
      this.module.i32.load(fieldOffset, 4, output()),
      this.module.i32.const(0),
    );
  }

  executionOutputSequence(outputLocal) {
    const output = () => this.module.local.get(outputLocal, binaryen.i32);
    return this.module.if(
      this.module.i32.ne(output(), this.module.i32.const(0)),
      this.module.i32.add(
        output(),
        this.module.i32.const(EXECUTION_OUTPUT_SEQUENCE_OFFSET),
      ),
      this.module.i32.const(0),
    );
  }

  advanceOutputSequence(sequenceLocal) {
    const pointer = () =>
      this.module.global.get(POINTER_GLOBALS.outputSequence, binaryen.i32);
    const sequence = () => this.module.local.get(sequenceLocal, binaryen.i32);
    return this.module.block(null, [
      this.module.local.set(
        sequenceLocal,
        this.module.i32.load(0, 4, pointer()),
      ),
      this.module.i32.store(
        0,
        4,
        pointer(),
        this.module.select(
          this.module.i32.eq(sequence(), this.module.i32.const(-1)),
          sequence(),
          this.module.i32.add(sequence(), this.module.i32.const(1)),
        ),
      ),
    ]);
  }

  fullInitClearRanges() {
    const ranges = [];
    let cursor = 0;
    for (const [stateId, slot] of this.mir.state.entries()) {
      if (slot.pinned !== true) continue;
      const layout = this.stateLayout[stateId];
      if (cursor < layout.offset) {
        ranges.push({ offset: cursor, size: layout.offset - cursor });
      }
      cursor = layout.offset + layout.size;
    }
    if (cursor < this.stateLayout.byteLength) {
      ranges.push({
        offset: cursor,
        size: this.stateLayout.byteLength - cursor,
      });
    }
    return ranges;
  }

  addAbiWrappers() {
    const initId = this.mir.entry_points.init;
    const processId = this.mir.entry_points.process;
    this.requireFunctionId(initId, "init entry point");
    this.requireFunctionId(processId, "process entry point");

    this.module.setFeatures(
      this.module.getFeatures() |
        binaryen.Features.BulkMemory |
        binaryen.Features.BulkMemoryOpt |
        (this.options.simd ? binaryen.Features.SIMD128 : 0),
    );
    // Pinned declarations fully initialize their own slots on this path.
    // Clear only the complementary ranges, including layout padding, so large
    // pinned arrays are never written once here and again by their initializer.
    const fullInitClears = this.fullInitClearRanges().map(({ offset, size }) =>
      this.module.memory.fill(
        offset === 0
          ? this.module.local.get(1, binaryen.i32)
          : this.module.i32.add(
            this.module.local.get(1, binaryen.i32),
            this.module.i32.const(offset),
          ),
        this.module.i32.const(0),
        this.module.i32.const(size),
      )
    );
    const initBody = this.module.block(null, [
      ...this.resetRuntimeFailure(initId),
      this.module.global.set(
        POINTER_GLOBALS.params,
        this.module.local.get(0, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.state,
        this.module.local.get(1, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.buffers,
        this.module.local.get(3, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferWrites,
        this.module.local.get(3, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferFrames,
        this.module.local.get(4, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferChannels,
        this.module.local.get(5, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferSampleRates,
        this.module.local.get(6, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.delegateBatch,
        this.executionOutputBatch(7, EXECUTION_OUTPUT_DELEGATE_BATCH_OFFSET),
      ),
      this.module.global.set(
        POINTER_GLOBALS.printBatch,
        this.executionOutputBatch(7, EXECUTION_OUTPUT_PRINT_BATCH_OFFSET),
      ),
      this.module.global.set(
        POINTER_GLOBALS.outputSequence,
        this.executionOutputSequence(7),
      ),
      this.module.global.set(
        INIT_ALL_GLOBAL,
        this.module.i32.ne(
          this.module.local.get(2, binaryen.i32),
          this.module.i32.const(0),
        ),
      ),
      ...(fullInitClears.length === 0
        ? []
        : [
          this.module.if(
            this.module.global.get(INIT_ALL_GLOBAL, binaryen.i32),
            this.module.block(null, fullInitClears),
          ),
        ]),
      this.module.call(this.functionNames[initId], [], binaryen.none),
      this.executionStatus(initId),
    ], binaryen.i32);
    this.module.addFunction(
      "$onda.abi.init",
      binaryen.createType([
        binaryen.i32,
        binaryen.i32,
        binaryen.i32,
        binaryen.i32,
        binaryen.i32,
        binaryen.i32,
        binaryen.i32,
        binaryen.i32,
      ]),
      binaryen.i32,
      [],
      initBody,
    );
    this.module.addFunctionExport("$onda.abi.init", "onda_processor_init");

    const processParams = binaryen.createType(
      Array.from({ length: 12 }, () => binaryen.i32),
    );
    const startFrame = () => this.module.local.get(4, binaryen.i32);
    const frames = () => this.module.local.get(5, binaryen.i32);
    const flags = () => this.module.local.get(6, binaryen.i32);
    const invalidRange = this.module.i32.or(
      this.module.i32.or(
        this.module.i32.or(
          this.module.i32.lt_s(startFrame(), this.module.i32.const(0)),
          this.module.i32.lt_s(frames(), this.module.i32.const(0)),
        ),
        this.module.i32.or(
          this.module.i32.gt_s(
            startFrame(),
            this.module.i32.const(this.mir.config.block_size),
          ),
          this.module.i32.gt_s(
            frames(),
            this.module.i32.sub(
              this.module.i32.const(this.mir.config.block_size),
              startFrame(),
            ),
          ),
        ),
      ),
      this.module.i32.ne(
        this.module.i32.and(
          flags(),
          this.module.i32.const(~ONDA_PROCESS_FULL_BLOCK),
        ),
        this.module.i32.const(0),
      ),
    );
    const processBody = this.module.block(null, [
      this.module.global.set(
        POINTER_GLOBALS.delegateBatch,
        this.executionOutputBatch(11, EXECUTION_OUTPUT_DELEGATE_BATCH_OFFSET),
      ),
      this.module.global.set(
        POINTER_GLOBALS.printBatch,
        this.executionOutputBatch(11, EXECUTION_OUTPUT_PRINT_BATCH_OFFSET),
      ),
      this.module.global.set(
        POINTER_GLOBALS.outputSequence,
        this.executionOutputSequence(11),
      ),
      this.module.if(
        invalidRange,
        this.module.return(
          this.module.i32.const(PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE),
        ),
      ),
      ...this.resetRuntimeFailure(processId),
      this.module.global.set(
        POINTER_GLOBALS.state,
        this.module.local.get(0, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.params,
        this.module.local.get(1, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.inputs,
        this.module.local.get(2, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.outputs,
        this.module.local.get(3, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.buffers,
        this.module.local.get(7, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferWrites,
        this.module.local.get(7, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferFrames,
        this.module.local.get(8, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferChannels,
        this.module.local.get(9, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.bufferSampleRates,
        this.module.local.get(10, binaryen.i32),
      ),
      this.module.call(
        this.functionNames[processId],
        [startFrame(), frames(), flags()],
        binaryen.none,
      ),
      this.executionStatus(processId),
    ], binaryen.i32);
    this.module.addFunction(
      "$onda.abi.process",
      processParams,
      binaryen.i32,
      [],
      processBody,
    );
    this.module.addFunctionExport("$onda.abi.process", "onda_process");

    this.mir.interface.events.forEach((event, eventId) => {
      this.requireFunctionId(event.handler, `event '${event.name}' handler`);
      const handler = this.mir.functions[event.handler];
      if (
        handler.kind?.kind !== "event" ||
        handler.kind.data !== eventId ||
        handler.params.length !== 0 ||
        handler.results.length !== 0
      ) {
        this.fail(`event '${event.name}' has an invalid MIR handler signature`);
      }
      const wrapperName = `$onda.abi.event.${eventId}`;
      const body = this.module.block(null, [
        this.module.global.set(
          POINTER_GLOBALS.delegateBatch,
          this.executionOutputBatch(7, EXECUTION_OUTPUT_DELEGATE_BATCH_OFFSET),
        ),
        this.module.global.set(
          POINTER_GLOBALS.printBatch,
          this.executionOutputBatch(7, EXECUTION_OUTPUT_PRINT_BATCH_OFFSET),
        ),
        this.module.global.set(
          POINTER_GLOBALS.outputSequence,
          this.executionOutputSequence(7),
        ),
        ...this.resetRuntimeFailure(event.handler),
        this.module.global.set(
          POINTER_GLOBALS.eventPayload,
          this.module.local.get(0, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.params,
          this.module.local.get(1, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.state,
          this.module.local.get(2, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.buffers,
          this.module.local.get(3, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.bufferWrites,
          this.module.local.get(3, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.bufferFrames,
          this.module.local.get(4, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.bufferChannels,
          this.module.local.get(5, binaryen.i32),
        ),
        this.module.global.set(
          POINTER_GLOBALS.bufferSampleRates,
          this.module.local.get(6, binaryen.i32),
        ),
        this.module.call(this.functionNames[event.handler], [], binaryen.none),
        this.executionStatus(event.handler),
      ], binaryen.i32);
      this.module.addFunction(
        wrapperName,
        binaryen.createType(Array.from({ length: 8 }, () => binaryen.i32)),
        binaryen.i32,
        [],
        body,
      );
      this.module.addFunctionExport(wrapperName, `onda_event_${eventId}`);
    });
  }
}
