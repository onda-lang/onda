import binaryen from "binaryen";
import { SUPPORTED_MIR_SCHEMA_VERSION } from "../constants.js";
import { OndaBinaryenError } from "../errors.js";
import { supportsMirOperation } from "../operations.js";
import { ONDA_MATH_KERNEL_WASM } from "../math-kernel.generated.js";
import {
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  validateProcessorMetadata,
} from "../artifact.js";

export {
  SUPPORTED_MIR_SCHEMA_VERSION,
  OndaBinaryenError,
  supportsMirOperation,
  ONDA_MATH_KERNEL_WASM,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  validateProcessorMetadata,
};

export const PAGE_BYTES = 64 * 1024;
export const STATIC_BASE = 1024;
export const MATH_KERNEL_RESERVED_END = 32 * 1024;
export const MATH_KERNEL_DATA_SEGMENT = ".rodata";
export const MATH_KERNEL_STACK_GLOBAL = "__stack_pointer";
export const MAX_MEMORY_PAGES = 65_536;
export const WASM32_ADDRESS_SPACE_BYTES = MAX_MEMORY_PAGES * PAGE_BYTES;
export const DEFAULT_OPTIMIZE_LEVEL = 4;
export const ONDA_PROCESS_FULL_BLOCK = (1 << 0) | (1 << 1);
export const DELEGATE_RECORD_HEADER_SIZE = 12;
export const DELEGATE_BATCH_STORAGE_OFFSET = 0;
export const DELEGATE_BATCH_CAPACITY_OFFSET = 4;
export const DELEGATE_BATCH_USED_OFFSET = 8;
export const DELEGATE_BATCH_RECORD_COUNT_OFFSET = 12;
export const DELEGATE_BATCH_OVERFLOW_OFFSET = 16;
export const PRINT_RECORD_HEADER_SIZE = 12;
export const PRINT_BATCH_STORAGE_OFFSET = 0;
export const PRINT_BATCH_CAPACITY_OFFSET = 4;
export const PRINT_BATCH_USED_OFFSET = 8;
export const PRINT_BATCH_RECORD_COUNT_OFFSET = 12;
export const PRINT_BATCH_OVERFLOW_OFFSET = 16;
export const EXECUTION_OUTPUT_DELEGATE_BATCH_OFFSET = 0;
export const EXECUTION_OUTPUT_PRINT_BATCH_OFFSET = 4;
export const EXECUTION_OUTPUT_SEQUENCE_OFFSET = 8;
export const RUNTIME_FAILURE_GLOBAL = "$onda.runtime_failure";
export const INIT_ALL_GLOBAL = "$onda.init_all";
export const MATH_KERNEL_INTRINSICS = new Set([
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

export const POINTER_GLOBALS = Object.freeze({
  inputs: "$onda.inputs",
  outputs: "$onda.outputs",
  params: "$onda.params",
  state: "$onda.state",
  eventPayload: "$onda.event_payload",
  delegateBatch: "$onda.delegate_batch",
  printBatch: "$onda.print_batch",
  outputSequence: "$onda.output_sequence",
  buffers: "$onda.buffers",
  bufferWrites: "$onda.buffer_writes",
  bufferFrames: "$onda.buffer_frames",
  bufferChannels: "$onda.buffer_channels",
  bufferSampleRates: "$onda.buffer_sample_rates",
});
export const BUFFER_DESCRIPTOR_POINTER_GLOBALS = new Set([
  POINTER_GLOBALS.buffers,
  POINTER_GLOBALS.bufferWrites,
  POINTER_GLOBALS.bufferFrames,
  POINTER_GLOBALS.bufferChannels,
  POINTER_GLOBALS.bufferSampleRates,
]);
export const TRAPPING_DESCRIPTOR_UNARY_OPS = new Set([
  binaryen.TruncSFloat32ToInt32,
  binaryen.TruncSFloat32ToInt64,
  binaryen.TruncSFloat64ToInt32,
  binaryen.TruncSFloat64ToInt64,
  binaryen.TruncUFloat32ToInt32,
  binaryen.TruncUFloat32ToInt64,
  binaryen.TruncUFloat64ToInt32,
  binaryen.TruncUFloat64ToInt64,
]);
export const TRAPPING_DESCRIPTOR_BINARY_OPS = new Set([
  binaryen.DivSInt32,
  binaryen.DivSInt64,
  binaryen.DivUInt32,
  binaryen.DivUInt64,
  binaryen.RemSInt32,
  binaryen.RemSInt64,
  binaryen.RemUInt32,
  binaryen.RemUInt64,
]);

export function collectMathKernelHelpers(mir) {
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

export function alignUp(value, alignment) {
  return Math.ceil(value / alignment) * alignment;
}

export function typeName(type, compiler) {
  if (type.kind === "scalar") return type.data;
  if (type.kind === "array") {
    return `${typeName(compiler.type(type.data.element), compiler)}[${type.data.len}]`;
  }
  if (type.kind === "slice") return `${type.data.element}[]`;
  return type.kind;
}

export function encodeScalarValues(values, scalar, compiler) {
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

export const I64_MIN = -(1n << 63n);
export const I64_MAX = (1n << 63n) - 1n;

export function decodeI64Literal(value, compiler) {
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

export function decodeFloatLiteral(value, scalar, compiler) {
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
