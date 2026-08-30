import { MirCompiler } from "./compiler/index.js";
import { OndaBinaryenError } from "./errors.js";
import { decodeMirMessagePack } from "./messagepack.js";
export {
  OndaArtifactError,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
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

function parseMirJson(json) {
  try {
    return JSON.parse(json);
  } catch (error) {
    throw new OndaBinaryenError(`invalid MIR JSON: ${error.message}`);
  }
}
