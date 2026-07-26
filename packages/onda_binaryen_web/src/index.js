import binaryen from "binaryen";
import { SUPPORTED_MIR_SCHEMA_VERSION } from "./constants.js";
import { OndaBinaryenError } from "./errors.js";
import { decodeMirMessagePack } from "./messagepack.js";
import { supportsMirOperation } from "./operations.js";
import { ONDA_MATH_KERNEL_WASM } from "./math-kernel.generated.js";
import {
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  validateProcessorMetadata,
} from "./artifact.js";

export {
  OndaArtifactError,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  createProcessorArtifactFiles,
  loadProcessorArtifactFiles,
  parseProcessorMetadata,
  serializeProcessorMetadata,
  validateProcessorArtifact,
  validateProcessorMetadata,
  validateProcessorModule,
} from "./artifact.js";
export { SUPPORTED_MIR_SCHEMA_VERSION } from "./constants.js";
export { OndaBinaryenError } from "./errors.js";

const PAGE_BYTES = 64 * 1024;
const STATIC_BASE = 1024;
const MATH_KERNEL_RESERVED_END = 32 * 1024;
const MATH_KERNEL_DATA_SEGMENT = ".rodata";
const MATH_KERNEL_STACK_GLOBAL = "__stack_pointer";
const MAX_MEMORY_PAGES = 65_536;
const WASM32_ADDRESS_SPACE_BYTES = MAX_MEMORY_PAGES * PAGE_BYTES;
const DEFAULT_OPTIMIZE_LEVEL = 4;
const ONDA_PROCESS_FULL_BLOCK = (1 << 0) | (1 << 1);
const MATH_KERNEL_INTRINSICS = new Set([
  "sin",
  "cos",
  "tan",
  "tanh",
  "atan",
  "atan2",
  "exp",
  "log",
  "pow",
  "remainder",
  "fma",
]);

const POINTER_GLOBALS = Object.freeze({
  inputs: "$onda.inputs",
  outputs: "$onda.outputs",
  params: "$onda.params",
  state: "$onda.state",
  eventPayload: "$onda.event_payload",
  buffers: "$onda.buffers",
  bufferFrames: "$onda.buffer_frames",
  bufferChannels: "$onda.buffer_channels",
  bufferSampleRates: "$onda.buffer_sample_rates",
});

// Compiles MIR emitted by Onda's semantic producer. The producer owns proofs
// for operations marked `bounds: "unchecked"` and all other validated MIR
// invariants. This backend deliberately does not expose a partial validator
// for downloaded or hand-authored MIR.
export function compileTrustedMir(mirJson, options = {}) {
  return compileMirInternal(mirJson, options);
}

function compileMirInternal(mirJson, options) {
  const mir = parseMirInput(mirJson);
  const compiler = new MirCompiler(mir, options);
  return compiler.compile();
}

function parseMirInput(input) {
  if (typeof input === "string") return parseMirJson(input);
  if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    try {
      return decodeMirMessagePack(input);
    } catch (error) {
      throw new OndaBinaryenError(`invalid MessagePack MIR: ${error.message}`);
    }
  }
  return input;
}

export function createDefaultImports() {
  return {};
}

function collectMathKernelHelpers(mir) {
  const result = new Set();
  for (const func of mir?.functions ?? []) {
    const localScalars = (func.locals ?? []).map((local) => {
      const type = mir.types?.[local.ty];
      return type?.kind === "scalar" ? type.data : null;
    });
    const valueScalar = (value) => {
      if (value?.kind === "constant") return value.data?.type;
      if (value?.kind === "local") return localScalars[value.data];
      return null;
    };
    const visitBlock = (block) => {
      for (const statement of block?.statements ?? []) {
        const kind = statement.kind?.kind;
        const data = statement.kind?.data;
        if (kind === "assign" && data?.value?.kind === "intrinsic") {
          const intrinsic = data.value.data?.intrinsic;
          const scalar = valueScalar(data.value.data?.args?.[0]);
          if (
            MATH_KERNEL_INTRINSICS.has(intrinsic)
            && (scalar === "f32" || scalar === "f64")
          ) {
            result.add(`onda_math_${intrinsic}_${scalar}`);
          }
        } else if (
          kind === "assign"
          && data?.value?.kind === "binary"
          && data.value.data?.op === "remainder"
        ) {
          const scalar = valueScalar(data.value.data?.lhs);
          if (scalar === "f32" || scalar === "f64") {
            result.add(`onda_math_remainder_${scalar}`);
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
  }
  return result;
}

function parseMirJson(json) {
  try {
    return JSON.parse(json);
  } catch (error) {
    throw new OndaBinaryenError(`invalid MIR JSON: ${error.message}`);
  }
}

class MirCompiler {
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
  }

  validateCurrentSchemaEnvelope() {
    const persistenceKinds = new Set([
      "snapshot",
      "instance_scratch",
      "control_mirror",
    ]);
    for (const [stateId, slot] of this.mir.state.entries()) {
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
        !inlineHints.has(func.attributes.inline)
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

  buildLayouts() {
    this.stateLayout = this.layoutNamedValues(this.mir.state);
    this.paramLayout = this.layoutNamedValues(this.mir.interface.params);
    this.inputLayout = this.layoutPorts(this.mir.interface.inputs);
    this.outputLayout = this.layoutPorts(this.mir.interface.outputs);
    this.controlOutputLayout = this.layoutControlOutputs();
    this.eventLayout = this.mir.interface.events.map((event) =>
      this.layoutEventValues(event.params),
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
              parameter?.mode !== "value"
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
    const paramLayouts = func.params.map((param) => {
      const layout = this.functionValueLayout(
        param.ty,
        nextIndex,
        `parameter '${param.name}'`,
        false,
        param.mode,
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
      breakLabels: [],
      continueLabels: [],
    };
    const body = this.compileBlock(func.body, context);
    const functionRef = this.module.addFunction(
      this.functionNames[id],
      binaryen.createType(paramScalars.map((type) => this.wasmType(type))),
      this.wasmResultType(resultScalars),
      [
        ...flatLocalScalars.map((type) => this.wasmType(type)),
        ...callResultLocals.map((entry) => entry.type),
        ...Array.from({ length: sliceScratch.count }, () => binaryen.i32),
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
        components: ["i32", "i32", "i32"],
      };
    }
    if (type.kind === "buffer") {
      return {
        index,
        typeId,
        kind: "buffer",
        components: ["i32", "i32", "i32", "f32"],
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
      ? ["address", "frames", "channels", "sample_rate"]
      : ["address", "length", "stride"];
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
            target.params[index]?.mode !== "value"
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
    const initBody = this.module.block(null, [
      this.module.global.set(
        POINTER_GLOBALS.params,
        this.module.local.get(0, binaryen.i32),
      ),
      this.module.global.set(
        POINTER_GLOBALS.state,
        this.module.local.get(1, binaryen.i32),
      ),
      this.module.memory.fill(
        this.module.local.get(1, binaryen.i32),
        this.module.i32.const(0),
        this.module.i32.const(this.stateLayout.byteLength ?? 0),
      ),
      this.module.call(this.functionNames[initId], [], binaryen.none),
    ]);
    this.module.addFunction(
      "$onda.abi.init",
      binaryen.createType([binaryen.i32, binaryen.i32]),
      binaryen.none,
      [],
      initBody,
    );
    this.module.addFunctionExport("$onda.abi.init", "onda_init");

    const processParams = binaryen.createType(
      Array.from({ length: 11 }, () => binaryen.i32),
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
      this.module.if(invalidRange, this.module.unreachable()),
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
    ]);
    this.module.addFunction(
      "$onda.abi.process",
      processParams,
      binaryen.none,
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
      ]);
      this.module.addFunction(
        wrapperName,
        binaryen.createType(Array.from({ length: 7 }, () => binaryen.i32)),
        binaryen.none,
        [],
        body,
      );
      this.module.addFunctionExport(wrapperName, `onda_event_${eventId}`);
    });
  }

  compileBlock(block, context) {
    return this.module.block(
      null,
      block.statements.map((statement) =>
        this.compileStatement(statement, context),
      ),
    );
  }

  compileStatement(statement, context) {
    const kind = statement.kind?.kind;
    const data = statement.kind?.data;
    switch (kind) {
      case "assign": {
        if (data.value?.kind === "process_frame") {
          const localId =
            data.destination?.base?.kind === "local" &&
            data.destination.projections.length === 0
              ? data.destination.base.data
              : null;
          if (!context.processFrameLocals.has(localId)) {
            this.fail(
              "process_frame must be the unique definition of an unprojected local",
            );
          }
        }
        if (this.type(this.placeTypeId(data.destination, context)).kind === "slice") {
          return this.storeSlicePlace(
            data.destination,
            this.compileSliceRvalue(data.value, context),
            context,
          );
        }
        const scalar = this.placeScalarType(data.destination, context);
        const value = this.compileRvalue(data.value, scalar, context);
        return this.storePlace(data.destination, value, scalar, context);
      }
      case "call":
        return this.compileCall(data, context);
      case "output_store":
        return this.compileOutputStore(data, context);
      case "control_output_store":
        return this.compileControlOutputStore(data, context);
      case "if":
        return this.module.if(
          this.compileValue(data.condition, context),
          this.compileBlock(data.then_block, context),
          this.compileBlock(data.else_block, context),
        );
      case "loop": {
        const id = this.nextLabel++;
        const breakLabel = `$onda.break.${id}`;
        const continueLabel = `$onda.loop.${id}`;
        context.breakLabels.push(breakLabel);
        context.continueLabels.push(continueLabel);
        const body = this.compileBlock(data.body, context);
        context.breakLabels.pop();
        context.continueLabels.pop();
        return this.module.block(breakLabel, [
          this.module.loop(
            continueLabel,
            this.module.block(null, [body, this.module.br(continueLabel)]),
          ),
        ]);
      }
      case "break":
        return this.module.br(this.currentLabel(context.breakLabels, "break"));
      case "continue":
        return this.module.br(
          this.currentLabel(context.continueLabels, "continue"),
        );
      case "return": {
        const values = data.values.map((value) =>
          this.compileValue(value, context),
        );
        return this.module.return(
          values.length > 1 ? this.module.tuple.make(values) : values[0],
        );
      }
      case "buffer_store":
        return this.compileBufferStore(data, context);
      case "buffer_param_store":
        return this.compileBufferParamStore(data, context);
      case "slice_store":
        return this.compileSliceStore(data, context);
      case "slice_fill":
        return this.compileSliceFill(statement, data, context);
      case "slice_copy":
        return this.compileSliceCopy(statement, data, context);
      default:
        this.fail(`unknown MIR statement '${String(kind)}'`);
    }
  }

  compileCall(data, context) {
    this.requireFunctionId(data.function, "call target");
    const target = this.mir.functions[data.function];
    if (data.args.length !== target.params.length) {
      this.fail(`call to '${target.name}' has the wrong number of arguments`);
    }
    const args = data.args.flatMap((argument, index) => {
      const parameterType = this.type(target.params[index].ty);
      if (parameterType.kind === "scalar") {
        if (target.params[index].mode === "value") {
          if (argument.kind !== "value") {
            this.fail(`scalar call argument ${index} of '${target.name}' is not a value`);
          }
          return [this.compileValue(argument.data, context)];
        }
        if (!["place", "slice_element"].includes(argument.kind)) {
          this.fail(
            `reference call argument ${index} of '${target.name}' is not addressable`,
          );
        }
        return [
          argument.kind === "place"
            ? this.placeAddress(argument.data, context)
            : this.compileSliceAddress(
                argument.data.slice,
                argument.data.index,
                argument.data.bounds,
                context,
              ),
        ];
      }
      if (parameterType.kind === "slice") {
        if (argument.kind !== "value") {
          this.fail(`slice call argument ${index} of '${target.name}' is not a value`);
        }
        return this.compileSliceValue(argument.data, context);
      }
      if (parameterType.kind === "array") {
        if (
          target.params[index].mode === "value" ||
          !["place", "array_window", "slice_window"].includes(argument.kind)
        ) {
          this.fail(`array reference argument ${index} of '${target.name}' is invalid`);
        }
        if (argument.kind === "place") {
          return [this.placeAddress(argument.data, context)];
        }
        if (argument.kind === "array_window") {
          return [
            this.compileArrayWindowAddress(
              argument.data,
              parameterType,
              context,
            ),
          ];
        }
        return [
          this.compileSliceWindowAddress(
            argument.data,
            parameterType,
            context,
          ),
        ];
      }
      if (parameterType.kind === "buffer") {
        if (argument.kind === "buffer") {
          return this.compileInterfaceBufferValue(argument.data);
        }
        if (argument.kind === "place") {
          return this.loadBufferPlace(argument.data, context);
        }
        this.fail(`buffer call argument ${index} of '${target.name}' is invalid`);
      }
      this.fail(
        `call argument ${index} of '${target.name}' has unsupported type '${parameterType.kind}'`,
      );
    });
    if (data.results.length !== target.results.length) {
      this.fail(`call result arity for '${target.name}' does not match its signature`);
    }
    const resultScalars = target.results.map((result, resultId) =>
      this.requireScalarType(result, `result ${resultId} of '${target.name}'`),
    );
    const resultType = this.wasmResultType(resultScalars);
    const call = this.module.call(this.functionNames[data.function], args, resultType);
    const localReferenceSync = data.args.flatMap((argument, index) => {
      const parameter = target.params[index];
      if (
        parameter.mode === "value"
        || argument.kind !== "place"
        || argument.data.base.kind !== "local"
        || argument.data.projections.length !== 0
      ) {
        return [];
      }
      const layout =
        this.localScalarRefLayout[context.functionId]?.[argument.data.base.data];
      if (!layout) return [];
      return [{
        localId: argument.data.base.data,
        address: layout.address,
        scalar: layout.scalar,
        writeBack: parameter.mode === "read_write_reference",
      }];
    });
    const beforeCall = localReferenceSync.map((sync) =>
      this.storeScalar(
        sync.scalar,
        this.module.i32.const(sync.address),
        this.module.local.get(
          this.localIndex(sync.localId, context),
          this.wasmType(sync.scalar),
        ),
      ),
    );
    const afterCall = localReferenceSync
      .filter((sync) => sync.writeBack)
      .map((sync) =>
        this.module.local.set(
          this.localIndex(sync.localId, context),
          this.loadScalar(sync.scalar, this.module.i32.const(sync.address)),
        ),
      );
    const resultSpill = context.callResultLocals.get(data);
    if (localReferenceSync.length > 0 && data.results.length > 0) {
      if (!resultSpill) {
        this.fail(`internal result spill is missing for call to '${target.name}'`);
      }
      const spilledValue = () =>
        this.module.local.get(resultSpill.index, resultSpill.type);
      const assignResults = data.results.length === 1
        ? [
            this.module.local.set(
              this.localIndex(data.results[0], context),
              spilledValue(),
            ),
          ]
        : data.results.map((localId, index) =>
            this.module.local.set(
              this.localIndex(localId, context),
              this.module.tuple.extract(spilledValue(), index),
            ),
          );
      return this.module.block(null, [
        ...beforeCall,
        this.module.local.set(resultSpill.index, call),
        ...afterCall,
        ...assignResults,
      ]);
    }
    let compiledCall;
    if (data.results.length === 0) {
      compiledCall = call;
    } else if (data.results.length === 1) {
      compiledCall = this.module.local.set(
        this.localIndex(data.results[0], context),
        call,
      );
    } else {
      const tupleLocal = context.callResultLocals.get(data);
      if (!tupleLocal) {
        this.fail(`internal tuple spill is missing for call to '${target.name}'`);
      }
      const tupleValue = () =>
        this.module.local.get(tupleLocal.index, tupleLocal.type);
      compiledCall = this.module.block(null, [
        this.module.local.set(tupleLocal.index, call),
        ...data.results.map((localId, index) =>
          this.module.local.set(
            this.localIndex(localId, context),
            this.module.tuple.extract(tupleValue(), index),
          ),
        ),
      ]);
    }
    if (localReferenceSync.length === 0) return compiledCall;
    return this.module.block(null, [...beforeCall, compiledCall, ...afterCall]);
  }

  compileOutputStore(data, context) {
    this.requireProcessFrame(data.frame, context, "audio output store");
    const port = this.outputLayout[data.output];
    if (!port) {
      this.fail(`output id ${data.output} is out of range`);
    }
    const channel = this.compilePortChannel(port, data.element, data.bounds, context);
    const tableAddress = this.module.i32.add(
      this.module.global.get(POINTER_GLOBALS.outputs, binaryen.i32),
      this.module.i32.mul(channel, this.module.i32.const(4)),
    );
    const channelPointer = this.module.i32.load(0, 4, tableAddress);
    const sampleAddress = this.module.i32.add(
      channelPointer,
      this.module.i32.mul(
        this.compileValue(data.frame, context),
        this.module.i32.const(port.size),
      ),
    );
    return this.storeScalar(
      port.scalar,
      sampleAddress,
      this.compileValue(data.value, context),
    );
  }

  compileControlOutputStore(data, context) {
    const output = this.mir.interface.control_outputs[data.output];
    const layout = this.controlOutputLayout[data.output];
    if (!output || !layout) {
      this.fail(`control output id ${data.output} is out of range`);
    }
    const flattened = this.flattenPortType(this.type(output.ty));
    let elementOffset = this.module.i32.const(0);
    if (this.type(output.ty).kind !== "array") {
      if (data.element !== null) {
        this.fail("scalar control output unexpectedly has an element index");
      }
    } else {
      if (data.element === null) {
        this.fail("array control output is missing its element index");
      }
      const index = this.compileBoundedIndex(
        data.element,
        flattened.channels,
        data.bounds,
        context,
      );
      elementOffset = this.module.i32.mul(
        index,
        this.module.i32.const(layout.size / flattened.channels),
      );
    }
    const address = this.module.i32.add(
      this.module.i32.add(
        this.module.global.get(POINTER_GLOBALS.state, binaryen.i32),
        this.module.i32.const(layout.offset),
      ),
      elementOffset,
    );
    return this.storeScalar(
      flattened.scalar,
      address,
      this.compileValue(data.value, context),
    );
  }

  compileBufferStore(data, context) {
    const buffer = this.requireBuffer(data.buffer);
    if (buffer.access !== "read_write") {
      this.fail(`buffer '${buffer.name}' is read-only`);
    }
    return this.storeScalar(
      buffer.element,
      this.compileBufferAddress(data, context),
      this.compileValue(data.value, context),
    );
  }

  compileBufferParamStore(data, context) {
    const type = this.bufferParamType(data.parameter, context);
    if (type.data.access !== "read_write") {
      this.fail(`buffer parameter ${data.parameter} is read-only`);
    }
    return this.storeScalar(
      type.data.element,
      this.compileBufferParamAddress(data, context),
      this.compileValue(data.value, context),
    );
  }

  compileRvalue(rvalue, expectedScalar, context) {
    const kind = rvalue.kind;
    const data = rvalue.data;
    switch (kind) {
      case "use":
        return this.compileValue(data, context);
      case "load":
        return this.loadPlace(data, context);
      case "unary":
        return this.compileUnary(
          data.op,
          this.valueScalarType(data.operand, context),
          this.compileValue(data.operand, context),
        );
      case "binary": {
        const scalar = this.valueScalarType(data.lhs, context);
        return this.compileBinary(
          data.op,
          scalar,
          () => this.compileValue(data.lhs, context),
          () => this.compileValue(data.rhs, context),
        );
      }
      case "compare": {
        const scalar = this.valueScalarType(data.lhs, context);
        return this.compileCompare(
          data.op,
          scalar,
          this.compileValue(data.lhs, context),
          this.compileValue(data.rhs, context),
        );
      }
      case "cast":
        return this.compileCast(
          this.valueScalarType(data.value, context),
          data.to,
          this.compileValue(data.value, context),
        );
      case "intrinsic":
        return this.compileIntrinsic(data, expectedScalar, context);
      case "process_frame":
        return this.compileProcessFrame(data, context);
      case "input_load":
        return this.compileInputLoad(data, context);
      case "output_load":
        return this.compileOutputLoad(data, context);
      case "const_data_load":
        return this.compileConstDataLoad(data, context);
      case "buffer_load":
        return this.compileBufferLoad(data, context);
      case "buffer_param_load":
        return this.compileBufferParamLoad(data, context);
      case "buffer_len":
        return this.compileBufferLen(data);
      case "buffer_param_len":
        return this.compileBufferParamLen(data, context);
      case "buffer_channels":
        return this.loadBufferTableValue(
          POINTER_GLOBALS.bufferChannels,
          data,
          "i32",
        );
      case "buffer_param_channels":
        return this.loadBufferParamComponent(data, 2, "i32", context);
      case "buffer_sample_rate":
        return this.loadBufferTableValue(
          POINTER_GLOBALS.bufferSampleRates,
          data,
          "f32",
        );
      case "buffer_param_sample_rate":
        return this.loadBufferParamComponent(data, 3, "f32", context);
      case "slice_len":
        return this.compileSliceValue(data, context)[1];
      case "slice_load":
        return this.compileSliceLoad(data, context);
      case "make_slice":
        this.fail("make_slice must be assigned to a slice-typed destination");
        break;
      default:
        this.fail(`unknown MIR rvalue '${String(kind)}'`);
    }
  }

  compileSliceRvalue(rvalue, context) {
    switch (rvalue.kind) {
      case "use":
        return this.compileSliceValue(rvalue.data, context);
      case "load":
        return this.loadSlicePlace(rvalue.data, context);
      case "make_slice":
        return this.compileMakeSlice(rvalue.data, context);
      default:
        this.fail(`rvalue '${String(rvalue.kind)}' does not produce a slice`);
    }
  }

  compileInputLoad(data, context) {
    this.requireProcessFrame(data.frame, context, "audio input load");
    const port = this.inputLayout[data.input];
    if (!port) {
      this.fail(`input id ${data.input} is out of range`);
    }
    const channel = this.compilePortChannel(port, data.element, data.bounds, context);
    const tableAddress = this.module.i32.add(
      this.module.global.get(POINTER_GLOBALS.inputs, binaryen.i32),
      this.module.i32.mul(channel, this.module.i32.const(4)),
    );
    const channelPointer = this.module.i32.load(0, 4, tableAddress);
    const sampleAddress = this.module.i32.add(
      channelPointer,
      this.module.i32.mul(
        this.compileValue(data.frame, context),
        this.module.i32.const(port.size),
      ),
    );
    return this.loadScalar(port.scalar, sampleAddress);
  }

  compileProcessFrame(data, context) {
    if (context.function.kind?.kind !== "process") {
      this.fail("process_frame is only valid in the process entry point");
    }
    const offset = () => this.compileValue(data.offset, context);
    const startFrame = () =>
      this.module.local.get(context.paramLayouts[0].index, binaryen.i32);
    const frames = () =>
      this.module.local.get(context.paramLayouts[1].index, binaryen.i32);
    const invalid = this.module.i32.ge_u(offset(), frames());
    return this.module.if(
      invalid,
      this.module.unreachable(),
      this.module.i32.add(startFrame(), offset()),
    );
  }

  requireProcessFrame(value, context, operation) {
    if (
      context.function.kind?.kind !== "process" ||
      value?.kind !== "local" ||
      !context.processFrameLocals.has(value.data)
    ) {
      this.fail(`${operation} frame must come directly from process_frame`);
    }
  }

  compileOutputLoad(data, context) {
    this.requireProcessFrame(data.frame, context, "audio output load");
    const port = this.outputLayout[data.output];
    if (!port) {
      this.fail(`output id ${data.output} is out of range`);
    }
    const channel = this.compilePortChannel(port, data.element, data.bounds, context);
    const tableAddress = this.module.i32.add(
      this.module.global.get(POINTER_GLOBALS.outputs, binaryen.i32),
      this.module.i32.mul(channel, this.module.i32.const(4)),
    );
    const channelPointer = this.module.i32.load(0, 4, tableAddress);
    const sampleAddress = this.module.i32.add(
      channelPointer,
      this.module.i32.mul(
        this.compileValue(data.frame, context),
        this.module.i32.const(port.size),
      ),
    );
    return this.loadScalar(port.scalar, sampleAddress);
  }

  compilePortChannel(port, element, bounds, context) {
    if (!port.isArray) {
      if (element !== null) {
        this.fail("scalar audio port unexpectedly has an element index");
      }
      return this.module.i32.const(port.channel);
    }
    if (element === null) {
      this.fail("array audio port is missing its element index");
    }
    const index = this.compileBoundedIndex(element, port.channels, bounds, context);
    return this.module.i32.add(this.module.i32.const(port.channel), index);
  }

  compileConstDataLoad(data, context) {
    const item = this.constLayout[data.data];
    if (!item) {
      this.fail(`const data id ${data.data} is out of range`);
    }
    const index = this.compileBoundedIndex(data.index, item.len, data.bounds, context);
    const address = this.module.i32.add(
      this.module.i32.const(item.address),
      this.module.i32.mul(index, this.module.i32.const(this.scalarSize(item.scalar))),
    );
    return this.loadScalar(item.scalar, address);
  }

  compileBufferLoad(data, context) {
    const buffer = this.requireBuffer(data.buffer);
    return this.loadScalar(
      buffer.element,
      this.compileBufferAddress(data, context),
    );
  }

  compileBufferParamLoad(data, context) {
    const type = this.bufferParamType(data.parameter, context);
    return this.loadScalar(
      type.data.element,
      this.compileBufferParamAddress(data, context),
    );
  }

  compileBufferParamAddress(data, context) {
    const type = this.bufferParamType(data.parameter, context);
    const component = (offset, scalar) => () =>
      this.loadBufferParamComponent(data.parameter, offset, scalar, context);
    const channels = component(2, "i32");
    const rawIndex = () => {
      if (data.channel === null) {
        return this.compileValue(data.index, context);
      }
      return this.module.i32.add(
        this.module.i32.mul(this.compileValue(data.index, context), channels()),
        this.compileValue(data.channel, context),
      );
    };
    const index = this.compileDynamicBoundedIndex(
      rawIndex,
      () => this.compileBufferParamTotalScalarLen(data.parameter, context),
      data.bounds,
      true,
    );
    return this.module.i32.add(
      this.loadBufferParamComponent(data.parameter, 0, "i32", context),
      this.module.i32.mul(
        index,
        this.module.i32.const(this.scalarSize(type.data.element)),
      ),
    );
  }

  compileBufferAddress(data, context) {
    const buffer = this.requireBuffer(data.buffer);
    const rawIndex = () => {
      if (data.channel === null) {
        return this.compileValue(data.index, context);
      }
      return this.module.i32.add(
        this.module.i32.mul(
          this.compileValue(data.index, context),
          this.loadBufferTableValue(
            POINTER_GLOBALS.bufferChannels,
            data.buffer,
            "i32",
          ),
        ),
        this.compileValue(data.channel, context),
      );
    };
    const index = this.compileDynamicBoundedIndex(
      rawIndex,
      () => this.compileBufferTotalScalarLen(data.buffer),
      data.bounds,
      true,
    );
    const pointer = this.loadBufferTableValue(
      POINTER_GLOBALS.buffers,
      data.buffer,
      "i32",
    );
    return this.module.i32.add(
      pointer,
      this.module.i32.mul(
        index,
        this.module.i32.const(this.scalarSize(buffer.element)),
      ),
    );
  }

  compileBufferLen(bufferId) {
    this.requireBuffer(bufferId);
    return this.loadBufferTableValue(
      POINTER_GLOBALS.bufferFrames,
      bufferId,
      "i32",
    );
  }

  compileBufferTotalScalarLen(bufferId) {
    this.requireBuffer(bufferId);
    return this.module.i32.mul(
      this.compileBufferLen(bufferId),
      this.loadBufferTableValue(
        POINTER_GLOBALS.bufferChannels,
        bufferId,
        "i32",
      ),
    );
  }

  compileBufferParamLen(parameterId, context) {
    this.bufferParamType(parameterId, context);
    return this.loadBufferParamComponent(parameterId, 1, "i32", context);
  }

  compileBufferParamTotalScalarLen(parameterId, context) {
    this.bufferParamType(parameterId, context);
    return this.module.i32.mul(
      this.compileBufferParamLen(parameterId, context),
      this.loadBufferParamComponent(parameterId, 2, "i32", context),
    );
  }

  bufferParamType(parameterId, context) {
    const parameter = context.function.params[parameterId];
    const type = parameter && this.type(parameter.ty);
    if (!type || type.kind !== "buffer") {
      this.fail(`parameter id ${parameterId} is not a buffer`);
    }
    return type;
  }

  bufferParamLayout(parameterId, context) {
    const layout = context.paramLayouts[parameterId];
    if (!layout || layout.kind !== "buffer") {
      this.fail(`parameter id ${parameterId} has no buffer descriptor`);
    }
    return layout;
  }

  loadBufferParamComponent(parameterId, offset, scalar, context) {
    const layout = this.bufferParamLayout(parameterId, context);
    return this.module.local.get(layout.index + offset, this.wasmType(scalar));
  }

  loadBufferPlace(place, context) {
    if (place.base.kind !== "parameter" || place.projections.length !== 0) {
      this.fail("buffer call arguments must be unprojected buffer parameters");
    }
    const layout = this.bufferParamLayout(place.base.data, context);
    return layout.components.map((scalar, offset) =>
      this.module.local.get(layout.index + offset, this.wasmType(scalar)),
    );
  }

  compileInterfaceBufferValue(bufferId) {
    this.requireBuffer(bufferId);
    return [
      this.loadBufferTableValue(POINTER_GLOBALS.buffers, bufferId, "i32"),
      this.loadBufferTableValue(POINTER_GLOBALS.bufferFrames, bufferId, "i32"),
      this.loadBufferTableValue(POINTER_GLOBALS.bufferChannels, bufferId, "i32"),
      this.loadBufferTableValue(POINTER_GLOBALS.bufferSampleRates, bufferId, "f32"),
    ];
  }

  loadBufferTableValue(globalName, bufferId, scalar) {
    this.requireBuffer(bufferId);
    const size = this.scalarSize(scalar);
    const address = this.module.i32.add(
      this.module.global.get(globalName, binaryen.i32),
      this.module.i32.const(bufferId * size),
    );
    return this.loadScalar(scalar, address);
  }

  compileSliceValue(value, context) {
    if (value.kind !== "local") {
      this.fail("slice values must reside in MIR locals");
    }
    const layout = context.localLayouts[value.data];
    if (!layout || layout.kind !== "slice") {
      this.fail(`local id ${value.data} is not a slice`);
    }
    return layout.components.map((scalar, offset) =>
      this.module.local.get(layout.index + offset, this.wasmType(scalar)),
    );
  }

  loadSlicePlace(place, context) {
    if (place.projections.length !== 0) {
      this.fail("slice places cannot have projections");
    }
    let layout;
    if (place.base.kind === "local") {
      layout = context.localLayouts[place.base.data];
    } else if (place.base.kind === "parameter") {
      layout = context.paramLayouts[place.base.data];
    } else if (place.base.kind === "event_param") {
      const event = this.mir.interface.events[context.eventId];
      const parameter = event?.params[place.base.data];
      const type = parameter && this.type(parameter.ty);
      if (!type || type.kind !== "slice") {
        this.fail(`event parameter id ${place.base.data} is not a slice`);
      }
      const header = () =>
        this.compileEventParamAddress(context.eventId, place.base.data);
      return [
        this.module.i32.add(header(), this.module.i32.const(4)),
        this.module.i32.load(0, 4, header()),
        this.module.i32.const(this.scalarSize(type.data.element)),
      ];
    } else {
      this.fail(`slice place base '${place.base.kind}' is not supported yet`);
    }
    if (!layout || layout.kind !== "slice") {
      this.fail(`place base '${place.base.kind}' id ${place.base.data} is not a slice`);
    }
    return layout.components.map((scalar, offset) =>
      this.module.local.get(layout.index + offset, this.wasmType(scalar)),
    );
  }

  compileEventParamAddress(eventId, paramId) {
    const event = this.mir.interface.events[eventId];
    if (!event || !Number.isInteger(paramId) || paramId < 0 || paramId >= event.params.length) {
      this.fail(`event parameter id ${paramId} is invalid for event ${eventId}`);
    }
    let offset = () => this.module.i32.const(0);
    for (let index = 0; index < paramId; index += 1) {
      const previous = offset;
      const type = this.type(event.params[index].ty);
      if (type.kind === "slice") {
        const elementSize = this.scalarSize(type.data.element);
        offset = () =>
          this.module.i32.add(
            previous(),
            this.module.i32.add(
              this.module.i32.const(4),
              this.module.i32.mul(
                this.module.i32.load(
                  0,
                  4,
                  this.module.i32.add(
                    this.module.global.get(POINTER_GLOBALS.eventPayload, binaryen.i32),
                    previous(),
                  ),
                ),
                this.module.i32.const(elementSize),
              ),
            ),
          );
      } else {
        const size = this.typeLayout(event.params[index].ty).size;
        offset = () => this.module.i32.add(previous(), this.module.i32.const(size));
      }
    }
    return this.module.i32.add(
      this.module.global.get(POINTER_GLOBALS.eventPayload, binaryen.i32),
      offset(),
    );
  }

  storeSlicePlace(place, components, context) {
    if (place.base.kind !== "local" || place.projections.length !== 0) {
      this.fail("slice assignment destination must be an unprojected local");
    }
    const layout = context.localLayouts[place.base.data];
    if (!layout || layout.kind !== "slice" || components.length !== 3) {
      this.fail(`local id ${place.base.data} is not a valid slice destination`);
    }
    return this.module.block(
      null,
      components.map((component, offset) =>
        this.module.local.set(layout.index + offset, component),
      ),
    );
  }

  compileMakeSlice(data, context) {
    const source = () => this.compileSliceSource(data.source, context);
    const range = this.compileSliceRange(
      () => this.compileValue(data.start, context),
      () => this.compileValue(data.len, context),
      () => source()[1],
      data.bounds,
    );
    return [
      this.module.i32.add(
        source()[0],
        this.module.i32.mul(
          range.start(),
          source()[2],
        ),
      ),
      range.len(),
      source()[2],
    ];
  }

  compileSliceRange(start, len, sourceLen, bounds) {
    const zero = () => this.module.i32.const(0);
    if (bounds === "unchecked") {
      return { start, len };
    }
    if (bounds === "clamp") {
      const normalizedStart = () => {
        const low = () =>
          this.module.select(
            this.module.i32.lt_s(start(), zero()),
            zero(),
            start(),
          );
        return this.module.select(
          this.module.i32.gt_s(low(), sourceLen()),
          sourceLen(),
          low(),
        );
      };
      const normalizedLen = () => {
        const low = () =>
          this.module.select(
            this.module.i32.lt_s(len(), zero()),
            zero(),
            len(),
          );
        const remaining = () =>
          this.module.i32.sub(sourceLen(), normalizedStart());
        return this.module.select(
          this.module.i32.gt_s(low(), remaining()),
          remaining(),
          low(),
        );
      };
      return { start: normalizedStart, len: normalizedLen };
    }
    if (bounds === "trap") {
      const invalid = () => {
        const remaining = () => this.module.i32.sub(sourceLen(), start());
        return this.module.i32.or(
          this.module.i32.or(
            this.module.i32.lt_s(start(), zero()),
            this.module.i32.gt_s(start(), sourceLen()),
          ),
          this.module.i32.or(
            this.module.i32.lt_s(len(), zero()),
            this.module.i32.gt_s(len(), remaining()),
          ),
        );
      };
      return {
        start: () =>
          this.module.if(invalid(), this.module.unreachable(), start()),
        len,
      };
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileSliceSource(source, context) {
    if (source.kind === "place") {
      const typeId = this.placeTypeId(source.data, context);
      const type = this.type(typeId);
      if (type.kind === "slice") {
        return this.loadSlicePlace(source.data, context);
      }
      if (type.kind !== "array") {
        this.fail(`slice place source has unsupported type '${type.kind}'`);
      }
      const element = this.type(type.data.element);
      if (element.kind !== "scalar") {
        this.fail("slice array source must have primitive elements");
      }
      return [
        this.placeAddress(source.data, context),
        this.module.i32.const(type.data.len),
        this.module.i32.const(this.scalarSize(element.data)),
      ];
    }
    if (source.kind === "const_data") {
      const item = this.constLayout[source.data];
      if (!item) this.fail(`const data id ${source.data} is out of range`);
      return [
        this.module.i32.const(item.address),
        this.module.i32.const(item.len),
        this.module.i32.const(this.scalarSize(item.scalar)),
      ];
    }
    if (source.kind === "buffer") {
      const buffer = this.requireBuffer(source.data.buffer);
      const elementSize = this.scalarSize(buffer.element);
      const address = this.loadBufferTableValue(
        POINTER_GLOBALS.buffers,
        source.data.buffer,
        "i32",
      );
      if (source.data.channel === null) {
        return [
          address,
          this.compileBufferLen(source.data.buffer),
          this.module.i32.const(elementSize),
        ];
      }
      const channels = () =>
        this.loadBufferTableValue(
          POINTER_GLOBALS.bufferChannels,
          source.data.buffer,
          "i32",
        );
      const channel = this.compileDynamicBoundedIndex(
        () => this.compileValue(source.data.channel, context),
        channels,
        "clamp",
        true,
      );
      return [
        this.module.i32.add(
          address,
          this.module.i32.mul(channel, this.module.i32.const(elementSize)),
        ),
        this.loadBufferTableValue(
          POINTER_GLOBALS.bufferFrames,
          source.data.buffer,
          "i32",
        ),
        this.module.i32.mul(channels(), this.module.i32.const(elementSize)),
      ];
    }
    if (source.kind === "buffer_param") {
      const type = this.bufferParamType(source.data.parameter, context);
      const elementSize = this.scalarSize(type.data.element);
      const address = () =>
        this.loadBufferParamComponent(source.data.parameter, 0, "i32", context);
      if (source.data.channel === null) {
        return [
          address(),
          this.compileBufferParamLen(source.data.parameter, context),
          this.module.i32.const(elementSize),
        ];
      }
      const channels = () =>
        this.loadBufferParamComponent(source.data.parameter, 2, "i32", context);
      const channel = this.compileDynamicBoundedIndex(
        () => this.compileValue(source.data.channel, context),
        channels,
        "clamp",
        true,
      );
      return [
        this.module.i32.add(
          address(),
          this.module.i32.mul(channel, this.module.i32.const(elementSize)),
        ),
        this.loadBufferParamComponent(source.data.parameter, 1, "i32", context),
        this.module.i32.mul(channels(), this.module.i32.const(elementSize)),
      ];
    }
    this.fail(`unsupported slice source '${String(source.kind)}'`);
  }

  sliceElementScalar(value, context) {
    if (value.kind !== "local") this.fail("slice value is not a local");
    const local = context.function.locals[value.data];
    const type = local && this.type(local.ty);
    if (!type || type.kind !== "slice") {
      this.fail(`local id ${value.data} is not slice-typed`);
    }
    return type.data.element;
  }

  sliceAccess(value, context) {
    if (value.kind !== "local") this.fail("slice value is not a local");
    const local = context.function.locals[value.data];
    const type = local && this.type(local.ty);
    if (!type || type.kind !== "slice") {
      this.fail(`local id ${value.data} is not slice-typed`);
    }
    return type.data.access;
  }

  compileSliceAddress(slice, index, bounds, context) {
    return this.compileSliceAddressWithFactories(
      () => this.compileSliceValue(slice, context),
      () => this.compileValue(index, context),
      bounds,
    );
  }

  compileSliceAddressWithFactories(slice, index, bounds) {
    const bounded = this.compileDynamicBoundedIndex(
      index,
      () => slice()[1],
      bounds,
    );
    return this.module.i32.add(
      slice()[0],
      this.module.i32.mul(bounded, slice()[2]),
    );
  }

  compileArrayWindowAddress(data, parameterType, context) {
    const sourceTypeId = this.placeTypeId(data.array, context);
    const sourceType = this.type(sourceTypeId);
    if (sourceType.kind !== "array") {
      this.fail("array-window source is not a fixed array");
    }
    if (
      !this.typesEquivalent(sourceType.data.element, parameterType.data.element) ||
      sourceType.data.len < parameterType.data.len
    ) {
      this.fail("array-window source does not contain the required parameter shape");
    }
    const elementSize = this.typeLayout(parameterType.data.element).size;
    const start = this.compileWindowStart(
      () => this.compileValue(data.start, context),
      () =>
        this.module.i32.const(
          sourceType.data.len - parameterType.data.len,
        ),
      data.bounds,
    );
    return this.module.i32.add(
      this.placeAddress(data.array, context),
      this.module.i32.mul(start(), this.module.i32.const(elementSize)),
    );
  }

  compileSliceWindowAddress(data, parameterType, context) {
    const elementType = this.type(parameterType.data.element);
    if (elementType.kind !== "scalar") {
      this.fail("slice-window fixed-array parameter element is not scalar");
    }
    const slice = () => this.compileSliceValue(data.slice, context);
    const elementSize = this.scalarSize(elementType.data);
    const requiredLen = parameterType.data.len;
    const start = this.compileWindowStart(
      () => this.compileValue(data.start, context),
      () =>
        this.module.i32.sub(
          slice()[1],
          this.module.i32.const(requiredLen),
        ),
      data.bounds,
    );
    const address = () =>
      this.module.i32.add(
        slice()[0],
        this.module.i32.mul(start(), slice()[2]),
      );
    if (data.bounds === "unchecked") {
      return address();
    }
    const invalidShape = () =>
      this.module.i32.or(
        this.module.i32.ne(
          slice()[2],
          this.module.i32.const(elementSize),
        ),
        this.module.i32.lt_s(
          slice()[1],
          this.module.i32.const(requiredLen),
        ),
      );
    return this.module.if(
      invalidShape(),
      this.module.unreachable(),
      address(),
    );
  }

  compileWindowStart(start, maximum, bounds) {
    const zero = () => this.module.i32.const(0);
    if (bounds === "unchecked") {
      return start;
    }
    if (bounds === "clamp") {
      return () => {
        const low = () =>
          this.module.select(
            this.module.i32.lt_s(start(), zero()),
            zero(),
            start(),
          );
        return this.module.select(
          this.module.i32.gt_s(low(), maximum()),
          maximum(),
          low(),
        );
      };
    }
    if (bounds === "trap") {
      return () =>
        this.module.if(
          this.module.i32.or(
            this.module.i32.lt_s(start(), zero()),
            this.module.i32.gt_s(start(), maximum()),
          ),
          this.module.unreachable(),
          start(),
        );
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileSliceLoad(data, context) {
    const scalar = this.sliceElementScalar(data.slice, context);
    return this.loadScalar(
      scalar,
      this.compileSliceAddress(data.slice, data.index, data.bounds, context),
    );
  }

  compileSliceStore(data, context) {
    if (this.sliceAccess(data.slice, context) !== "read_write") {
      this.fail("slice store destination is read-only");
    }
    const scalar = this.sliceElementScalar(data.slice, context);
    return this.storeScalar(
      scalar,
      this.compileSliceAddress(data.slice, data.index, data.bounds, context),
      this.compileValue(data.value, context),
    );
  }

  compileSliceFill(statement, data, context) {
    if (this.sliceAccess(data.destination, context) !== "read_write") {
      this.fail("slice fill destination is read-only");
    }
    const scratch = context.sliceScratch.get(statement);
    if (!scratch || scratch.count !== 1) {
      this.fail("internal slice-fill scratch local is missing");
    }
    const counter = scratch.index;
    const id = this.nextLabel++;
    const vectorLoopLabel = `$onda.slice.fill.vector.${id}`;
    const scalarLoopLabel = `$onda.slice.fill.scalar.${id}`;
    const destination = () => this.compileSliceValue(data.destination, context);
    const counterValue = () => this.module.local.get(counter, binaryen.i32);
    const scalar = this.sliceElementScalar(data.destination, context);
    const scalarSize = this.scalarSize(scalar);
    const address = () =>
      this.module.i32.add(
        destination()[0],
        this.module.i32.mul(counterValue(), destination()[2]),
      );
    const scalarLoop = () =>
      this.module.loop(
        scalarLoopLabel,
        this.module.if(
          this.module.i32.lt_s(counterValue(), destination()[1]),
          this.module.block(null, [
            this.storeScalar(
              scalar,
              address(),
              this.compileValue(data.value, context),
            ),
            this.module.local.set(
              counter,
              this.module.i32.add(counterValue(), this.module.i32.const(1)),
            ),
            this.module.br(scalarLoopLabel),
          ]),
        ),
      );
    const body = [];
    if (this.options.simd) {
      const lanes = 16 / scalarSize;
      const vectorCondition = this.module.i32.and(
        this.module.i32.eq(
          destination()[2],
          this.module.i32.const(scalarSize),
        ),
        this.module.i32.and(
          this.module.i32.ge_u(
            destination()[1],
            this.module.i32.const(lanes),
          ),
          this.module.i32.le_u(
            counterValue(),
            this.module.i32.sub(
              destination()[1],
              this.module.i32.const(lanes),
            ),
          ),
        ),
      );
      body.push(
        this.module.loop(
          vectorLoopLabel,
          this.module.if(
            vectorCondition,
            this.module.block(null, [
              this.module.v128.store(
                0,
                scalarSize,
                address(),
                this.compileVectorSplat(
                  scalar,
                  this.compileValue(data.value, context),
                ),
              ),
              this.module.local.set(
                counter,
                this.module.i32.add(
                  counterValue(),
                  this.module.i32.const(lanes),
                ),
              ),
              this.module.br(vectorLoopLabel),
            ]),
          ),
        ),
      );
    }
    body.push(scalarLoop());
    return this.module.block(null, [
      this.module.local.set(counter, this.module.i32.const(0)),
      ...body,
    ]);
  }

  compileSliceCopy(statement, data, context) {
    if (this.sliceAccess(data.destination, context) !== "read_write") {
      this.fail("slice copy destination is read-only");
    }
    const scratch = context.sliceScratch.get(statement);
    if (!scratch || scratch.count !== 2) {
      this.fail("internal slice-copy scratch locals are missing");
    }
    const count = scratch.index;
    const counter = scratch.index + 1;
    const id = this.nextLabel++;
    const loopLabel = `$onda.slice.copy.${id}`;
    const destination = () => this.compileSliceValue(data.destination, context);
    const source = () => this.compileSliceValue(data.source, context);
    const countValue = () => this.module.local.get(count, binaryen.i32);
    const counterValue = () => this.module.local.get(counter, binaryen.i32);
    const copyIndex = () =>
      this.module.select(
        this.module.i32.and(
          this.module.i32.eq(destination()[2], source()[2]),
          this.module.i32.gt_u(destination()[0], source()[0]),
        ),
        this.module.i32.sub(
          this.module.i32.sub(countValue(), this.module.i32.const(1)),
          counterValue(),
        ),
        counterValue(),
      );
    const sourceAddress = () =>
      this.module.i32.add(
        source()[0],
        this.module.i32.mul(copyIndex(), source()[2]),
      );
    const destinationAddress = () =>
      this.module.i32.add(
        destination()[0],
        this.module.i32.mul(copyIndex(), destination()[2]),
      );
    const sourceScalar = this.sliceElementScalar(data.source, context);
    const destinationScalar = this.sliceElementScalar(data.destination, context);
    const copiedValue = () => {
      const loaded = this.loadScalar(sourceScalar, sourceAddress());
      return sourceScalar === destinationScalar
        ? loaded
        : this.compileCast(sourceScalar, destinationScalar, loaded);
    };
    const nonEmpty = () =>
      this.module.i32.gt_s(countValue(), this.module.i32.const(0));
    const sourceEnd = () =>
      this.module.i32.add(
        this.module.i32.add(
          source()[0],
          this.module.i32.mul(
            this.module.i32.sub(countValue(), this.module.i32.const(1)),
            source()[2],
          ),
        ),
        this.module.i32.const(this.scalarSize(sourceScalar)),
      );
    const destinationEnd = () =>
      this.module.i32.add(
        this.module.i32.add(
          destination()[0],
          this.module.i32.mul(
            this.module.i32.sub(countValue(), this.module.i32.const(1)),
            destination()[2],
          ),
        ),
        this.module.i32.const(this.scalarSize(destinationScalar)),
      );
    const overlaps = () =>
      this.module.i32.and(
        nonEmpty(),
        this.module.i32.and(
          this.module.i32.lt_u(destination()[0], sourceEnd()),
          this.module.i32.lt_u(source()[0], destinationEnd()),
        ),
      );
    const invalidOverlap = () =>
      this.module.i32.and(
        this.module.i32.ne(destination()[2], source()[2]),
        overlaps(),
      );
    const scalarCopy = () =>
      this.module.loop(
        loopLabel,
        this.module.if(
          this.module.i32.lt_s(counterValue(), countValue()),
          this.module.block(null, [
            this.storeScalar(destinationScalar, destinationAddress(), copiedValue()),
            this.module.local.set(
              counter,
              this.module.i32.add(counterValue(), this.module.i32.const(1)),
            ),
            this.module.br(loopLabel),
          ]),
        ),
      );
    const sameRepresentation = sourceScalar === destinationScalar;
    const copy = sameRepresentation
      ? this.module.if(
          this.module.i32.and(
            this.module.i32.eq(
              destination()[2],
              this.module.i32.const(this.scalarSize(destinationScalar)),
            ),
            this.module.i32.eq(
              source()[2],
              this.module.i32.const(this.scalarSize(sourceScalar)),
            ),
          ),
          // memory.copy has memmove overlap semantics and lets engines use
          // their tuned bulk-memory implementation for contiguous slices.
          this.module.memory.copy(
            destination()[0],
            source()[0],
            this.module.i32.mul(
              countValue(),
              this.module.i32.const(this.scalarSize(sourceScalar)),
            ),
          ),
          scalarCopy(),
        )
      : scalarCopy();
    return this.module.block(null, [
      this.module.local.set(
        count,
        this.module.select(
          this.module.i32.lt_s(destination()[1], source()[1]),
          destination()[1],
          source()[1],
        ),
      ),
      this.module.if(invalidOverlap(), this.module.unreachable()),
      this.module.local.set(counter, this.module.i32.const(0)),
      copy,
    ]);
  }

  compileVectorSplat(scalar, value) {
    switch (scalar) {
      case "bool": return this.module.i8x16.splat(value);
      case "i32": return this.module.i32x4.splat(value);
      case "i64": return this.module.i64x2.splat(value);
      case "f32": return this.module.f32x4.splat(value);
      case "f64": return this.module.f64x2.splat(value);
      default: this.fail(`unknown SIMD scalar type '${String(scalar)}'`);
    }
  }

  compileValue(value, context) {
    switch (value.kind) {
      case "local": {
        const scalar = context.localScalars[value.data];
        if (!scalar) {
          this.fail(`local id ${value.data} is not a scalar or is out of range`);
        }
        return this.module.local.get(
          this.localIndex(value.data, context),
          this.wasmType(scalar),
        );
      }
      case "constant":
        return this.compileConstant(value.data);
      default:
        this.fail(`unknown MIR value '${String(value.kind)}'`);
    }
  }

  compileConstant(value) {
    switch (value.type) {
      case "f32":
        return this.module.f32.const(decodeFloatLiteral(value.value, "f32", this));
      case "f64":
        return this.module.f64.const(decodeFloatLiteral(value.value, "f64", this));
      case "i32":
        return this.module.i32.const(value.value);
      case "i64":
        return this.module.i64.const(decodeI64Literal(value.value, this));
      case "bool":
        return this.module.i32.const(value.value ? 1 : 0);
      default:
        this.fail(`unknown scalar constant type '${String(value.type)}'`);
    }
  }

  valueScalarType(value, context) {
    if (value.kind === "constant") {
      return value.data.type;
    }
    if (value.kind === "local") {
      const scalar = context.localScalars[value.data];
      if (!scalar) {
        this.fail(`local id ${value.data} is not a scalar or is out of range`);
      }
      return scalar;
    }
    this.fail(`unknown MIR value '${String(value.kind)}'`);
  }

  placeScalarType(place, context) {
    const typeId = this.placeTypeId(place, context);
    return this.requireScalarType(typeId, "place");
  }

  placeTypeId(place, context) {
    let typeId;
    switch (place.base.kind) {
      case "local":
        typeId = context.function.locals[place.base.data]?.ty;
        break;
      case "parameter":
        typeId = context.function.params[place.base.data]?.ty;
        break;
      case "state":
        typeId = this.mir.state[place.base.data]?.ty;
        break;
      case "param":
        typeId = this.mir.interface.params[place.base.data]?.ty;
        break;
      case "event_param": {
        const event = this.mir.interface.events[context.eventId];
        typeId = event?.params[place.base.data]?.ty;
        break;
      }
      default:
        this.fail(`place base '${place.base.kind}' is not supported yet`);
    }
    if (!Number.isInteger(typeId)) {
      this.fail(`place base '${place.base.kind}' id ${place.base.data} is out of range`);
    }
    for (const projection of place.projections) {
      const type = this.type(typeId);
      if (projection.kind === "index" && type.kind === "array") {
        typeId = type.data.element;
      } else {
        this.fail(`projection '${projection.kind}' on '${type.kind}' is not supported yet`);
      }
    }
    return typeId;
  }

  loadPlace(place, context) {
    if (place.base.kind === "local" && place.projections.length === 0) {
      const scalar = this.placeScalarType(place, context);
      return this.module.local.get(
        this.localIndex(place.base.data, context),
        this.wasmType(scalar),
      );
    }
    if (place.base.kind === "parameter" && place.projections.length === 0) {
      const scalar = this.placeScalarType(place, context);
      const layout = context.paramLayouts[place.base.data];
      if (!layout || !["scalar", "scalar_ref"].includes(layout.kind)) {
        this.fail(`parameter id ${place.base.data} is not a scalar`);
      }
      if (layout.kind === "scalar_ref") {
        return this.loadScalar(
          scalar,
          this.module.local.get(layout.index, binaryen.i32),
        );
      }
      return this.module.local.get(layout.index, this.wasmType(scalar));
    }
    const scalar = this.placeScalarType(place, context);
    return this.loadScalar(scalar, this.placeAddress(place, context));
  }

  storePlace(place, value, scalar, context) {
    if (place.base.kind === "local" && place.projections.length === 0) {
      return this.module.local.set(this.localIndex(place.base.data, context), value);
    }
    if (place.base.kind === "parameter" && place.projections.length === 0) {
      const layout = context.paramLayouts[place.base.data];
      if (layout?.kind !== "scalar_ref") {
        this.fail("assignment to a by-value function parameter is not supported");
      }
      return this.storeScalar(
        scalar,
        this.module.local.get(layout.index, binaryen.i32),
        value,
      );
    }
    return this.storeScalar(scalar, this.placeAddress(place, context), value);
  }

  placeAddress(place, context) {
    let typeId;
    let address;
    switch (place.base.kind) {
      case "parameter": {
        const layout = context.paramLayouts[place.base.data];
        if (!layout || !["scalar_ref", "array_ref"].includes(layout.kind)) {
          this.fail(`parameter id ${place.base.data} is not an addressable reference`);
        }
        typeId = context.function.params[place.base.data].ty;
        address = this.module.local.get(layout.index, binaryen.i32);
        break;
      }
      case "local": {
        const layout =
          this.localArrayLayout[context.functionId]?.[place.base.data]
          ?? this.localScalarRefLayout[context.functionId]?.[place.base.data];
        if (!layout) {
          this.fail(`local id ${place.base.data} is not addressable`);
        }
        typeId = context.function.locals[place.base.data].ty;
        address = this.module.i32.const(layout.address);
        break;
      }
      case "state": {
        const layout = this.stateLayout[place.base.data];
        if (!layout) this.fail(`state id ${place.base.data} is out of range`);
        typeId = this.mir.state[place.base.data].ty;
        address = this.module.i32.add(
          this.module.global.get(POINTER_GLOBALS.state, binaryen.i32),
          this.module.i32.const(layout.offset),
        );
        break;
      }
      case "param": {
        const layout = this.paramLayout[place.base.data];
        if (!layout) this.fail(`param id ${place.base.data} is out of range`);
        typeId = this.mir.interface.params[place.base.data].ty;
        address = this.module.i32.add(
          this.module.global.get(POINTER_GLOBALS.params, binaryen.i32),
          this.module.i32.const(layout.offset),
        );
        break;
      }
      case "event_param": {
        const event = this.mir.interface.events[context.eventId];
        const layout = this.eventLayout[context.eventId]?.[place.base.data];
        if (!event || !layout) {
          this.fail(
            `event parameter id ${place.base.data} is invalid for function '${context.function.name}'`,
          );
        }
        typeId = event.params[place.base.data].ty;
        if (this.type(typeId).kind === "slice") {
          this.fail("slice event parameters require slice-value lowering");
        }
        address = this.compileEventParamAddress(context.eventId, place.base.data);
        break;
      }
      default:
        this.fail(
          `addressable place base '${place.base.kind}' is not in the first Binaryen slice`,
        );
    }

    for (const projection of place.projections) {
      const type = this.type(typeId);
      if (projection.kind !== "index" || type.kind !== "array") {
        this.fail(`projection '${projection.kind}' on '${type.kind}' is not supported yet`);
      }
      const elementLayout = this.typeLayout(type.data.element);
      const index = this.compileBoundedIndex(
        projection.data.index,
        type.data.len,
        projection.data.bounds,
        context,
      );
      address = this.module.i32.add(
        address,
        this.module.i32.mul(index, this.module.i32.const(elementLayout.size)),
      );
      typeId = type.data.element;
    }
    return address;
  }

  compileBoundedIndex(value, length, bounds, context) {
    if (!Number.isInteger(length) || length <= 0) {
      this.fail("array and port lengths must be positive integers");
    }
    if (bounds === "unchecked") {
      return this.compileValue(value, context);
    }
    if (bounds === "clamp") {
      return this.module.select(
        this.module.i32.lt_s(
          this.compileValue(value, context),
          this.module.i32.const(0),
        ),
        this.module.i32.const(0),
        this.module.select(
          this.module.i32.ge_s(
            this.compileValue(value, context),
            this.module.i32.const(length),
          ),
          this.module.i32.const(length - 1),
          this.compileValue(value, context),
        ),
      );
    }
    if (bounds === "trap") {
      const outOfBounds = this.module.i32.or(
        this.module.i32.lt_s(
          this.compileValue(value, context),
          this.module.i32.const(0),
        ),
        this.module.i32.ge_s(
          this.compileValue(value, context),
          this.module.i32.const(length),
        ),
      );
      return this.module.if(
        outOfBounds,
        this.module.unreachable(),
        this.compileValue(value, context),
      );
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileDynamicBoundedIndex(index, length, bounds, clampLengthKnownPositive = false) {
    if (bounds === "unchecked") {
      return index();
    }
    if (bounds === "clamp") {
      const maximum = () =>
        this.module.i32.sub(length(), this.module.i32.const(1));
      const clamped = () =>
        this.module.select(
          this.module.i32.lt_s(index(), this.module.i32.const(0)),
          this.module.i32.const(0),
          this.module.select(
            this.module.i32.gt_s(index(), maximum()),
            maximum(),
            index(),
          ),
        );
      if (clampLengthKnownPositive) return clamped();
      return this.module.if(
        this.module.i32.le_s(length(), this.module.i32.const(0)),
        this.module.unreachable(),
        clamped(),
      );
    }
    if (bounds === "trap") {
      return this.module.if(
        this.module.i32.or(
          this.module.i32.lt_s(index(), this.module.i32.const(0)),
          this.module.i32.ge_s(index(), length()),
        ),
        this.module.unreachable(),
        index(),
      );
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileUnary(op, scalar, value) {
    if (!supportsMirOperation("unary", op, scalar)) {
      this.fail(`unary operation '${String(op)}' does not support scalar '${scalar}'`);
    }
    switch (op) {
      case "negate":
        if (scalar === "f32" || scalar === "f64") return this.module[scalar].neg(value);
        return this.module[scalar].sub(this.zero(scalar), value);
      case "logical_not":
        return this.module.i32.eqz(value);
      case "bit_not":
        return this.module[scalar].xor(value, this.minusOne(scalar));
      default:
        this.fail(`unknown unary operation '${String(op)}'`);
    }
  }

  compileBinary(op, scalar, lhs, rhs) {
    if (!supportsMirOperation("binary", op, scalar)) {
      this.fail(`binary operation '${String(op)}' does not support scalar '${scalar}'`);
    }
    const wasm = this.module[scalar === "bool" ? "i32" : scalar];
    const integer = scalar === "i32" || scalar === "i64" || scalar === "bool";
    switch (op) {
      case "add": return wasm.add(lhs(), rhs());
      case "subtract": return wasm.sub(lhs(), rhs());
      case "multiply": return wasm.mul(lhs(), rhs());
      case "divide": {
        if (!integer) return wasm.div(lhs(), rhs());
        const minimum = () =>
          scalar === "i64"
            ? this.module.i64.const(-(1n << 63n))
            : this.module.i32.const(-0x8000_0000);
        const negativeOne = () =>
          scalar === "i64"
            ? this.module.i64.const(-1n)
            : this.module.i32.const(-1);
        const overflow = this.module.i32.and(
          wasm.eq(lhs(), minimum()),
          wasm.eq(rhs(), negativeOne()),
        );
        return this.module.if(overflow, minimum(), wasm.div_s(lhs(), rhs()));
      }
      case "remainder":
        if (!integer) {
          return this.compileMathKernelCall("remainder", scalar, [lhs(), rhs()]);
        }
        return wasm.rem_s(lhs(), rhs());
      case "bit_and": return wasm.and(lhs(), rhs());
      case "bit_or": return wasm.or(lhs(), rhs());
      case "bit_xor": return wasm.xor(lhs(), rhs());
      case "shift_left": return wasm.shl(lhs(), rhs());
      case "shift_right": return wasm.shr_s(lhs(), rhs());
      default:
        this.fail(`unknown binary operation '${String(op)}'`);
    }
  }

  compileCompare(op, scalar, lhs, rhs) {
    if (!supportsMirOperation("compare", op, scalar)) {
      this.fail(`comparison '${String(op)}' does not support scalar '${scalar}'`);
    }
    const type = scalar === "bool" ? "i32" : scalar;
    const wasm = this.module[type];
    const integer = type === "i32" || type === "i64";
    switch (op) {
      case "equal": return wasm.eq(lhs, rhs);
      case "not_equal": return wasm.ne(lhs, rhs);
      case "less": return integer ? wasm.lt_s(lhs, rhs) : wasm.lt(lhs, rhs);
      case "less_equal": return integer ? wasm.le_s(lhs, rhs) : wasm.le(lhs, rhs);
      case "greater": return integer ? wasm.gt_s(lhs, rhs) : wasm.gt(lhs, rhs);
      case "greater_equal": return integer ? wasm.ge_s(lhs, rhs) : wasm.ge(lhs, rhs);
      default:
        this.fail(`unknown comparison '${String(op)}'`);
    }
  }

  compileCast(from, to, value) {
    const source = from === "bool" ? "i32" : from;
    const target = to === "bool" ? "i32" : to;
    if (source === target) return value;
    if (target === "bool") return this.module[source].ne(value, this.zero(source));
    if (source === "f32" && target === "f64") return this.module.f64.promote(value);
    if (source === "f64" && target === "f32") return this.module.f32.demote(value);
    if (source === "i32" && target === "i64") return this.module.i64.extend_s(value);
    if (source === "i64" && target === "i32") return this.module.i32.wrap(value);
    if ((source === "i32" || source === "i64") && target === "f32") {
      return this.module.f32.convert_s[source](value);
    }
    if ((source === "i32" || source === "i64") && target === "f64") {
      return this.module.f64.convert_s[source](value);
    }
    if ((source === "f32" || source === "f64") && target === "i32") {
      this.module.setFeatures(
        this.module.getFeatures() | binaryen.Features.NontrappingFPToInt,
      );
      return this.module.i32.trunc_s_sat[source](value);
    }
    if ((source === "f32" || source === "f64") && target === "i64") {
      this.module.setFeatures(
        this.module.getFeatures() | binaryen.Features.NontrappingFPToInt,
      );
      return this.module.i64.trunc_s_sat[source](value);
    }
    this.fail(`unsupported scalar cast from '${from}' to '${to}'`);
  }

  compileIntrinsic(data, expectedScalar, context) {
    const scalar = data.args.length
      ? this.valueScalarType(data.args[0], context)
      : expectedScalar;
    const args = data.args.map((value) => this.compileValue(value, context));
    const isFloat = scalar === "f32" || scalar === "f64";
    const isInteger = scalar === "i32" || scalar === "i64";
    if (!isFloat && !isInteger) {
      this.fail(`intrinsic '${data.intrinsic}' requires numeric operands`);
    }
    const wasm = this.module[scalar];

    if (isInteger) {
      // Binaryen expressions are tree nodes, so each use needs its own local
      // get/constant node even when the MIR value is the same.
      const arg = (index) => this.compileValue(data.args[index], context);
      switch (data.intrinsic) {
        case "abs":
          return this.module.select(
            wasm.lt_s(arg(0), this.zero(scalar)),
            wasm.sub(this.zero(scalar), arg(0)),
            arg(0),
          );
        case "min":
          return this.module.select(wasm.lt_s(arg(0), arg(1)), arg(0), arg(1));
        case "max":
          return this.module.select(wasm.gt_s(arg(0), arg(1)), arg(0), arg(1));
        default:
          this.fail(`intrinsic '${data.intrinsic}' requires f32 or f64 operands`);
      }
    }

    switch (data.intrinsic) {
      case "sqrt": return wasm.sqrt(args[0]);
      case "abs": return wasm.abs(args[0]);
      case "floor": return wasm.floor(args[0]);
      case "ceil": return wasm.ceil(args[0]);
      case "trunc": return wasm.trunc(args[0]);
      case "min": return wasm.min(args[0], args[1]);
      case "max": return wasm.max(args[0], args[1]);
      case "fma": return this.compileMathKernelCall(data.intrinsic, scalar, args);
      case "sin":
      case "cos":
      case "tan":
      case "tanh":
      case "atan":
      case "atan2":
      case "exp":
      case "log":
      case "pow":
        return this.compileMathKernelCall(data.intrinsic, scalar, args);
      case "round":
        return this.compileRoundHelper(scalar, args);
      default:
        this.fail(`unknown intrinsic '${String(data.intrinsic)}'`);
    }
  }

  compileMathKernelCall(intrinsic, scalar, args) {
    const name = `onda_math_${intrinsic}_${scalar}`;
    if (!this.requiredMathHelpers.has(name)) {
      this.fail(`math kernel was not reserved for helper '${name}'`);
    }
    return this.module.call(
      name,
      args,
      this.wasmType(scalar),
    );
  }

  compileRoundHelper(scalar, args) {
    const name = `$onda.math.round.${scalar}`;
    if (!this.internalHelpers.has(name)) {
      const wasm = this.module[scalar];
      const get = () => this.module.local.get(0, this.wasmType(scalar));
      const trunc = () => wasm.trunc(get());
      const magnitude = wasm.abs(wasm.sub(get(), trunc()));
      const rounded = wasm.add(
        trunc(),
        wasm.copysign(wasm.const(1), get()),
      );
      this.module.addFunction(
        name,
        this.wasmType(scalar),
        this.wasmType(scalar),
        [],
        this.module.select(
          wasm.ge(magnitude, wasm.const(0.5)),
          rounded,
          trunc(),
        ),
      );
      this.internalHelpers.add(name);
    }
    return this.module.call(name, args, this.wasmType(scalar));
  }

  loadScalar(scalar, address) {
    switch (scalar) {
      case "bool": return this.module.i32.load8_u(0, 1, address);
      case "i32": return this.module.i32.load(0, 4, address);
      case "i64": return this.module.i64.load(0, 8, address);
      case "f32": return this.module.f32.load(0, 4, address);
      case "f64": return this.module.f64.load(0, 8, address);
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  storeScalar(scalar, address, value) {
    switch (scalar) {
      case "bool": return this.module.i32.store8(0, 1, address, value);
      case "i32": return this.module.i32.store(0, 4, address, value);
      case "i64": return this.module.i64.store(0, 8, address, value);
      case "f32": return this.module.f32.store(0, 4, address, value);
      case "f64": return this.module.f64.store(0, 8, address, value);
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  type(typeId) {
    const type = this.mir.types[typeId];
    if (!type) this.fail(`type id ${typeId} is out of range`);
    return type;
  }

  typesEquivalent(lhsId, rhsId, visiting = new Set()) {
    if (lhsId === rhsId) return true;
    const key = `${lhsId}:${rhsId}`;
    if (visiting.has(key)) return true;
    const lhs = this.mir.types[lhsId];
    const rhs = this.mir.types[rhsId];
    if (!lhs || !rhs || lhs.kind !== rhs.kind) return false;
    visiting.add(key);
    let equivalent = false;
    if (lhs.kind === "scalar") {
      equivalent = lhs.data === rhs.data;
    } else if (lhs.kind === "array") {
      equivalent =
        lhs.data.len === rhs.data.len &&
        this.typesEquivalent(lhs.data.element, rhs.data.element, visiting);
    } else if (lhs.kind === "slice") {
      equivalent =
        lhs.data.element === rhs.data.element &&
        lhs.data.access === rhs.data.access;
    } else if (lhs.kind === "buffer") {
      equivalent =
        lhs.data.element === rhs.data.element &&
        JSON.stringify(lhs.data.channels) === JSON.stringify(rhs.data.channels) &&
        lhs.data.access === rhs.data.access;
    } else if (lhs.kind === "tuple") {
      equivalent =
        lhs.data.length === rhs.data.length &&
        lhs.data.every((element, index) =>
          this.typesEquivalent(element, rhs.data[index], visiting),
        );
    } else if (lhs.kind === "struct") {
      equivalent = lhs.data === rhs.data;
    }
    visiting.delete(key);
    return equivalent;
  }

  typeLayout(typeId) {
    const type = this.type(typeId);
    if (type.kind === "scalar") {
      const size = this.scalarSize(type.data);
      return { size, align: size, scalar: type.data };
    }
    if (type.kind === "array") {
      const element = this.typeLayout(type.data.element);
      if (!Number.isInteger(type.data.len) || type.data.len <= 0) {
        this.fail("fixed array length must be a positive integer");
      }
      return {
        size: element.size * type.data.len,
        align: element.align,
        scalar: element.scalar,
      };
    }
    this.fail(`storage layout for MIR type '${type.kind}' is not supported yet`);
  }

  requireScalarType(typeId, description) {
    const type = this.type(typeId);
    if (type.kind !== "scalar") {
      this.fail(`${description} has unsupported non-scalar type '${type.kind}'`);
    }
    return type.data;
  }

  scalarSize(scalar) {
    switch (scalar) {
      case "bool": return 1;
      case "i32":
      case "f32": return 4;
      case "i64":
      case "f64": return 8;
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  requireWasm32Extent(byteLength, description) {
    if (
      !Number.isSafeInteger(byteLength)
      || byteLength < 0
      || byteLength >= WASM32_ADDRESS_SPACE_BYTES
    ) {
      this.fail(`${description} must fit within the wasm32 4 GiB address space`);
    }
  }

  wasmType(scalar) {
    return binaryen[scalar === "bool" ? "i32" : scalar];
  }

  wasmResultType(scalars) {
    if (scalars.length === 0) return binaryen.none;
    if (scalars.length === 1) return this.wasmType(scalars[0]);
    return binaryen.createType(scalars.map((scalar) => this.wasmType(scalar)));
  }

  zero(scalar) {
    const type = scalar === "bool" ? "i32" : scalar;
    return type === "i64"
      ? this.module.i64.const(0n)
      : this.module[type].const(0);
  }

  minusOne(scalar) {
    const type = scalar === "bool" ? "i32" : scalar;
    return type === "i64"
      ? this.module.i64.const(-1n)
      : this.module.i32.const(-1);
  }

  localIndex(localId, context) {
    if (!Number.isInteger(localId) || localId < 0 || localId >= context.localScalars.length) {
      this.fail(`local id ${localId} is out of range`);
    }
    return context.localLayouts[localId].index;
  }

  requireFunctionId(id, description) {
    if (!Number.isInteger(id) || id < 0 || id >= this.mir.functions.length) {
      this.fail(`${description} function id ${id} is out of range`);
    }
  }

  requireBuffer(id) {
    if (!Number.isInteger(id) || id < 0 || id >= this.mir.interface.buffers.length) {
      this.fail(`buffer id ${id} is out of range`);
    }
    return this.mir.interface.buffers[id];
  }

  currentLabel(labels, statement) {
    const label = labels.at(-1);
    if (!label) this.fail(`'${statement}' appears outside a MIR loop`);
    return label;
  }

  buildMetadata() {
    const stateSize = this.stateLayout.byteLength ?? 0;
    const paramSize = this.paramLayout.byteLength ?? 0;
    const snapshot = this.stateSnapshotMetadata();
    const eventExports = this.mir.interface.events.map(
      (_, id) => `onda_event_${id}`,
    );
    const requiredExports = [
      "memory",
      "__heap_base",
      "onda_init",
      "onda_process",
      ...eventExports,
    ];
    const targetFeatures = ["bulk-memory"];
    if (this.options.simd) targetFeatures.push("simd128");
    return {
      format: PROCESSOR_ARTIFACT_FORMAT,
      format_version: PROCESSOR_ARTIFACT_FORMAT_VERSION,
      artifact_kind: "webassembly_module",
      abi_version: PROCESSOR_ABI_VERSION,
      backend: "binaryen-js",
      mir_schema_version: this.mir.schema_version,
      target: {
        triple: "wasm32-unknown-unknown",
        cpu: "generic",
        features: targetFeatures.map((feature) => `+${feature}`).join(","),
        reloc_model: "static",
        code_model: "default",
        opt_level: String(this.options.optimizeLevel),
        abi_name: null,
        data_layout: "e-m:e-p:32:32-i64:64-n32:64-S128",
        pointer_width_bits: 32,
        byte_order: "little_endian",
        pointer_model: "linear_memory_offset",
        calling_convention: "core_webassembly",
      },
      integration: {
        required_symbols: requiredExports,
        one_processor_per_artifact: true,
        profile: {
          kind: "core_webassembly_module",
          imports: [],
          memory_export: "memory",
          heap_base_export: "__heap_base",
        },
      },
      required_features: targetFeatures,
      optimization: {
        enabled: this.options.optimize,
        level: this.options.optimizeLevel,
        shrink_level: this.options.shrinkLevel,
        fast_math: this.options.fastMath,
        simd: this.options.simd,
        inline_functions_with_loops:
          this.options.allowInliningFunctionsWithLoops,
      },
      compile: {
        sample_rate: this.mir.config.sample_rate,
        block_size: this.mir.config.block_size,
        fast_math: this.options.fastMath,
      },
      exports: {
        memory: "memory",
        heap_base: "__heap_base",
        init: "onda_init",
        process: "onda_process",
        events: eventExports,
      },
      runtime: {
        state_size_bytes: stateSize,
        state_align_bytes: 16,
        state_initialization: "zeroed",
        snapshot_size_bytes: snapshot.byteLength,
        snapshot_format_version: PROCESSOR_SNAPSHOT_FORMAT_VERSION,
        snapshot_byte_order: "little_endian",
        snapshot_restore_base: "post_init_physical_state_image",
        param_size_bytes: paramSize,
        param_align_bytes: 16,
        requires_full_blocks: false,
      },
      metadata: {
        states: snapshot.entries,
        inputs: this.portMetadata(this.mir.interface.inputs, this.inputLayout),
        outputs: this.portMetadata(this.mir.interface.outputs, this.outputLayout),
        control_outputs: this.mir.interface.control_outputs.map((output, id) => ({
          name: output.name,
          type_repr: typeName(this.type(output.ty), this),
          scalar: this.storageShape(output.ty).scalar,
          array_len: this.storageShape(output.ty).length,
          element_size_bytes: this.scalarSize(this.storageShape(output.ty).scalar),
          slot_offset: this.interfaceSlotOffset(
            this.mir.interface.control_outputs,
            id,
          ),
          byte_offset: null,
          state_byte_offset: this.controlOutputLayout[id].offset,
          byte_size: this.controlOutputLayout[id].size,
          default_reprs: null,
          range_min_repr: null,
          range_max_repr: null,
          param_control: null,
        })),
        params: this.mir.interface.params.map((param, id) => ({
          name: param.name,
          type_repr: typeName(this.type(param.ty), this),
          scalar: this.storageShape(param.ty).scalar,
          array_len: this.storageShape(param.ty).length,
          element_size_bytes: this.scalarSize(this.storageShape(param.ty).scalar),
          slot_offset: this.interfaceSlotOffset(this.mir.interface.params, id),
          byte_offset: this.paramLayout[id].offset,
          state_byte_offset: null,
          byte_size: this.paramLayout[id].size,
          default_reprs: this.constantReprs(param.default),
          range_min_repr: this.scalarRepr(param.range?.min),
          range_max_repr: this.scalarRepr(param.range?.max),
          param_control: this.storageShape(param.ty).length === 1 && param.range
            ? {
                scale: param.control.scale,
                curve: param.control.curve,
                unit: param.control.unit,
                step_repr: this.scalarRepr(param.control.step),
                step_count: param.control.step_count,
              }
            : null,
        })),
        buffers: this.mir.interface.buffers.map((buffer) => {
          const channels = this.bufferChannelMetadata(buffer.channels);
          return {
            name: buffer.name,
            type_repr: this.bufferTypeRepr(buffer, channels),
            scalar: buffer.element,
            element_size_bytes: this.scalarSize(buffer.element),
            channels: channels.kind,
            static_channels: channels.count,
            access: buffer.access,
            may_write: buffer.access === "read_write",
          };
        }),
        events: this.mir.interface.events.map((event, eventId) => ({
          name: event.name,
          export: `onda_event_${eventId}`,
          payload_size_bytes: this.eventLayout[eventId].byteLength,
          payload_min_size_bytes: this.eventLayout[eventId].minimumByteLength,
          has_dynamic_payload: this.eventLayout[eventId].dynamic,
          params: event.params.map((param, paramId) => ({
            name: param.name,
            type_repr: typeName(this.type(param.ty), this),
            scalar: this.storageShape(param.ty).scalar,
            array_len: this.storageShape(param.ty).length ?? 0,
            is_slice: this.storageShape(param.ty).isSlice === true,
            byte_offset: this.eventLayout[eventId][paramId].offset,
            byte_size: this.eventLayout[eventId][paramId].size,
            element_size_bytes: this.scalarSize(
              this.storageShape(param.ty).scalar,
            ),
            has_default: param.default !== null && param.default !== undefined,
            default_reprs: this.constantReprs(param.default),
          })),
        })),
      },
    };
  }

  stateSnapshotMetadata() {
    let byteOffset = 0;
    const entries = [];
    for (const [id, slot] of this.mir.state.entries()) {
      if (slot.persistence !== "snapshot") {
        continue;
      }
      const shape = this.storageShape(slot.ty);
      const layout = this.stateLayout[id];
      entries.push({
        name: slot.name,
        type_repr: typeName(this.type(slot.ty), this),
        scalar: shape.scalar,
        array_len: shape.length,
        element_size_bytes: this.scalarSize(shape.scalar),
        packed_snapshot_byte_offset: byteOffset,
        physical_state_byte_offset: layout.offset,
        byte_size: layout.size,
      });
      byteOffset += layout.size;
    }
    return { entries, byteLength: byteOffset };
  }

  portMetadata(ports, layouts) {
    return ports.map((port, id) => ({
      name: port.name,
      type_repr: typeName(this.type(port.ty), this),
      scalar: layouts[id].scalar,
      array_len: layouts[id].channels,
      element_size_bytes: layouts[id].size,
      slot_offset: layouts[id].channel,
      byte_offset: null,
      state_byte_offset: null,
      byte_size: layouts[id].size * layouts[id].channels,
      default_reprs: null,
      range_min_repr: null,
      range_max_repr: null,
      param_control: null,
    }));
  }

  interfaceSlotOffset(values, end) {
    let offset = 0;
    for (let id = 0; id < end; id += 1) {
      offset += this.storageShape(values[id].ty).length;
    }
    return offset;
  }

  constantReprs(value) {
    if (value === null || value === undefined) return null;
    if (value.kind === "scalar") return [this.scalarRepr(value.data)];
    if (value.kind === "aggregate") {
      return value.data.flatMap((entry) => this.constantReprs(entry) ?? []);
    }
    this.fail(`unknown MIR constant kind '${String(value.kind)}'`);
  }

  scalarRepr(value) {
    if (value === null || value === undefined) return null;
    if (Object.is(value.value, -0)) return "-0";
    return String(value.value);
  }

  bufferTypeRepr(buffer, channels) {
    if (channels.kind === "mono") return `buffer[${buffer.element}]`;
    if (channels.kind === "static") {
      return `buffer[${buffer.element}[${channels.count}]]`;
    }
    return `buffer[${buffer.element}[]]`;
  }

  storageShape(typeId) {
    const type = this.type(typeId);
    if (type.kind === "scalar") {
      return { scalar: type.data, length: 1, isArray: false };
    }
    if (type.kind === "array") {
      const element = this.type(type.data.element);
      if (element.kind !== "scalar") {
        this.fail("nested aggregate storage metadata is not supported yet");
      }
      return { scalar: element.data, length: type.data.len, isArray: true };
    }
    if (type.kind === "slice") {
      return {
        scalar: type.data.element,
        length: null,
        isArray: true,
        isSlice: true,
      };
    }
    this.fail(`storage metadata for MIR type '${type.kind}' is not supported yet`);
  }

  bufferChannelMetadata(channels) {
    if (channels === "mono") {
      return { kind: "mono", count: 1 };
    }
    if (channels === "dynamic") {
      return { kind: "dynamic", count: null };
    }
    if (
      channels &&
      typeof channels === "object" &&
      Number.isInteger(channels.static) &&
      channels.static > 0
    ) {
      return { kind: "static", count: channels.static };
    }
    this.fail(`invalid MIR buffer channel descriptor '${JSON.stringify(channels)}'`);
  }

  fail(message) {
    throw new OndaBinaryenError(message);
  }
}

function alignUp(value, alignment) {
  return Math.ceil(value / alignment) * alignment;
}

function typeName(type, compiler) {
  if (type.kind === "scalar") return type.data;
  if (type.kind === "array") {
    return `${typeName(compiler.type(type.data.element), compiler)}[${type.data.len}]`;
  }
  if (type.kind === "slice") return `${type.data.element}[]`;
  return type.kind;
}

function encodeScalarValues(values, scalar, compiler) {
  const size = compiler.scalarSize(scalar);
  const bytes = new Uint8Array(values.length * size);
  const view = new DataView(bytes.buffer);
  values.forEach((value, index) => {
    if (value.type !== scalar) {
      compiler.fail(`const data scalar '${value.type}' does not match '${scalar}'`);
    }
    const offset = index * size;
    switch (scalar) {
      case "bool": view.setUint8(offset, value.value ? 1 : 0); break;
      case "i32": view.setInt32(offset, value.value, true); break;
      case "i64":
        view.setBigInt64(offset, decodeI64Literal(value.value, compiler), true);
        break;
      case "f32":
        view.setFloat32(offset, decodeFloatLiteral(value.value, "f32", compiler), true);
        break;
      case "f64":
        view.setFloat64(offset, decodeFloatLiteral(value.value, "f64", compiler), true);
        break;
      default: compiler.fail(`unknown const data scalar '${String(scalar)}'`);
    }
  });
  return bytes;
}

const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;

function decodeI64Literal(value, compiler) {
  if (typeof value !== "string" || !/^-?(0|[1-9][0-9]*)$/.test(value)) {
    compiler.fail(
      `MIR schema ${SUPPORTED_MIR_SCHEMA_VERSION} i64 values must be canonical decimal strings`,
    );
  }
  let decoded;
  try {
    decoded = BigInt(value);
  } catch {
    compiler.fail(`invalid MIR i64 value '${String(value)}'`);
  }
  if (decoded < I64_MIN || decoded > I64_MAX) {
    compiler.fail(`MIR i64 value '${value}' is outside the signed 64-bit range`);
  }
  return decoded;
}

function decodeFloatLiteral(value, scalar, compiler) {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      compiler.fail(`${scalar} JSON number must be finite`);
    }
    return value;
  }

  const digits = scalar === "f32" ? 8 : 16;
  if (
    typeof value !== "string"
    || !new RegExp(`^0x[0-9a-f]{${digits}}$`).test(value)
  ) {
    compiler.fail(
      `${scalar} value must be a finite JSON number or an exact ${digits}-digit IEEE bit pattern`,
    );
  }

  const bytes = new ArrayBuffer(8);
  const view = new DataView(bytes);
  if (scalar === "f32") {
    view.setUint32(0, Number.parseInt(value.slice(2), 16), true);
    return view.getFloat32(0, true);
  }
  view.setBigUint64(0, BigInt(value), true);
  return view.getFloat64(0, true);
}
