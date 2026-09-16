// Logical SoA shape planning. Array extents remain axes; plans never expand
// array elements into metadata. This mirrors the compiler-free Rust planner.
const LIMIT = 0x7fffffff;
const SIZES = Object.freeze({ bool: 1, i32: 4, i64: 8, f32: 4, f64: 8 });
const SIGNED_DECIMAL_INTEGER = /^[+-]?[0-9]+$/;
export const EVENT_INPUT_SIZE_BYTES = 16;
export const PROCESSOR_EXECUTION_INPUT_REJECTED = 2;

/** Return the shortest host number that round-trips to the same f32. */
export function canonicalF32Number(value) {
  const rounded = Math.fround(value);
  if (!Number.isFinite(rounded) || rounded === 0) return rounded;
  for (let precision = 1; precision <= 9; precision += 1) {
    const candidate = Number(rounded.toPrecision(precision));
    if (Math.fround(candidate) === rounded) return candidate;
  }
  return rounded;
}

function freeze(value, seen = new Set()) {
  if (!value || typeof value !== "object" || seen.has(value)) return;
  seen.add(value);
  for (const child of Object.values(value)) freeze(child, seen);
  Object.freeze(value);
}

function extent(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value > LIMIT) {
    throw new RangeError("payload byte extent exceeds i32");
  }
  return value;
}
function align(value, alignment) {
  return Math.floor(extent(value + alignment - 1) / alignment) * alignment;
}
function fields(entries) {
  if (!Array.isArray(entries)) throw new TypeError("invalid payload fields");
  const names = new Set();
  for (const entry of entries) {
    if (typeof entry?.name !== "string" || !entry.name || /[.\[\]]/.test(entry.name) || names.has(entry.name)) {
      throw new TypeError("invalid or duplicate payload field name");
    }
    names.add(entry.name);
  }
  return names;
}
function arrayElement(ty) {
  return ty?.kind === "scalar" || ty?.kind === "struct";
}
function parseSignedDecimalInteger(value, message) {
  if (typeof value !== "string" || !SIGNED_DECIMAL_INTEGER.test(value)) {
    throw new TypeError(message);
  }
  return BigInt(value);
}
function domain(encoding, range) {
  if (range == null) return null;
  if (!["i32", "i64"].includes(encoding) || !["clamp", "wrap"].includes(range.mode)
      || range.min?.type !== encoding || range.max?.type !== encoding) {
    throw new TypeError("invalid payload integer range");
  }
  const bits = encoding === "i32" ? 32 : 64;
  const min = parseSignedDecimalInteger(range.min.value, "invalid payload integer range");
  const max = parseSignedDecimalInteger(range.max.value, "invalid payload integer range");
  if (min > max || BigInt.asIntN(bits, min) !== min || BigInt.asIntN(bits, max) !== max) {
    throw new RangeError("invalid payload integer range");
  }
  return { min, max, wrap: range.mode === "wrap" };
}

export class PayloadPlan {
  #parameterNames;
  #abiDefaults;

  constructor(schema) {
    schema = structuredClone(schema);
    this.#parameterNames = fields(schema?.params);
    this.parameters = [];
    this.tensors = [];
    this.schema = schema;
    const active = new Set();
    const visit = (ty, path, shape, steps) => {
      if (!ty || active.has(ty)) throw new TypeError("invalid recursive payload schema");
      active.add(ty);
      shape.reduce((count, axis) => extent(count * axis), 1);
      switch (ty.kind) {
        case "scalar": {
          const size = SIZES[ty.encoding];
          if (size === undefined) throw new TypeError("invalid payload scalar encoding");
          const elements = shape.reduce((count, axis) => extent(count * axis), 1);
          this.tensors.push({ path, encoding: ty.encoding, size, shape, steps: steps.map((step) => ({ ...step })), elements,
            domain: domain(ty.encoding, ty.integer_range) });
          break;
        }
        case "tuple":
          if (!Array.isArray(ty.elements) || !ty.elements.length
              || ty.elements.some((element) => element?.kind !== "scalar")) {
            throw new TypeError("invalid payload tuple");
          }
          ty.elements.forEach((element, index) =>
            visit(element, `${path}.__${index}`, shape, [...steps, { field: index }]));
          break;
        case "struct":
          if (typeof ty.name !== "string" || !ty.name) throw new TypeError("invalid payload struct name");
          fields(ty.fields);
          if (!ty.fields.length) throw new TypeError("payload structs must contain at least one field");
          for (const field of ty.fields) {
            if (field.default !== undefined) parsePayloadDefault(field.ty, field.default);
            visit(field.ty, `${path}.${field.name}`, shape, [...steps, { field: field.name }]);
          }
          break;
        case "array":
          if (!arrayElement(ty.element) || extent(ty.len) === 0) {
            throw new TypeError("invalid payload array");
          }
          visit(ty.element, path, [...shape, ty.len], [...steps, { axis: ty.len }]);
          break;
        default: throw new TypeError("invalid payload type");
      }
      active.delete(ty);
    };
    let abiParameter = 0;
    for (const field of schema.params) {
      const dynamic = field.ty?.kind === "slice";
      const start = this.tensors.length;
      const ty = dynamic ? field.ty.element : field.ty;
      if (dynamic && !arrayElement(ty)) throw new TypeError("invalid payload slice element");
      const lengthParameter = dynamic && ty.kind === "struct" ? abiParameter++ : null;
      visit(ty, field.name, [], dynamic ? [{ axis: null }] : []);
      if (this.tensors.length === start) throw new TypeError("payload types must contain at least one scalar");
      for (let index = start; index < this.tensors.length; index += 1) {
        this.tensors[index].parameter = abiParameter++;
        this.tensors[index].lengthPrefix = dynamic && lengthParameter === null;
      }
      this.parameters.push({ name: field.name, dynamic, lengthParameter, start, end: this.tensors.length });
    }
    this.abiParameterCount = abiParameter;
    this.dynamicParameters = this.parameters.filter((param) => param.dynamic).length;
    const minimum = this.sizes(Array(this.dynamicParameters).fill(0));
    this.fixedWireSize = this.dynamicParameters === 0 ? minimum.wire : null;
    this.minimumWorkspace = minimum.workspace;
    this.defaults = schema.params.map((field) =>
      field.default === undefined ? undefined : parsePayloadDefault(field.ty, field.default));
    for (const tensor of this.tensors) {
      let stride = 1;
      for (let index = tensor.steps.length - 1; index >= 0; index -= 1) {
        const step = tensor.steps[index];
        if (step.field === undefined) { step.stride = stride; if (step.axis !== null) stride *= step.axis; }
      }
    }
    this.#abiDefaults = Array(this.abiParameterCount).fill(null);
    for (let parameter = 0; parameter < this.parameters.length; parameter += 1) {
      const source = this.defaults[parameter];
      if (source === undefined) continue;
      const group = this.parameters[parameter];
      for (let index = group.start; index < group.end; index += 1) {
        const tensor = this.tensors[index];
        this.#abiDefaults[tensor.parameter] = {
          encoding: tensor.encoding,
          values: Array.from(
            { length: tensor.elements },
            (_, element) => leafValue(source, tensor.steps, element),
          ),
        };
      }
    }
    freeze(this);
  }

  matchesAbiDefault(parameter, defaultReprs) {
    if (!Number.isInteger(parameter) || parameter < 0 || parameter >= this.abiParameterCount) return false;
    const defaults = this.#abiDefaults[parameter];
    if (defaults === null || defaultReprs === null) return defaults === defaultReprs;
    if (!Array.isArray(defaultReprs) || defaultReprs.length !== defaults.values.length) return false;
    try {
      return defaultReprs.every((repr, index) => {
        const actual = parseScalarDefault(defaults.encoding, repr);
        const expected = defaults.values[index];
        return defaults.encoding === "f32"
          ? Object.is(Math.fround(actual), Math.fround(expected))
          : Object.is(actual, expected);
      });
    } catch {
      return false;
    }
  }

  // Read-only preflight: no per-element metadata or temporary payload copies.
  walk(length, tensor, prefix = null) {
    let wire = 0;
    let workspace = 0;
    for (let parameter = 0; parameter < this.parameters.length; parameter += 1) {
      const param = this.parameters[parameter];
      let logicalLength = 1;
      if (param.dynamic) {
        logicalLength = extent(length(wire, parameter));
        workspace = align(workspace, 4);
        if (prefix) prefix(wire, workspace, logicalLength);
        wire = extent(wire + 4);
        workspace = extent(workspace + 4);
      }
      for (let index = param.start; index < param.end; index += 1) {
        const leaf = this.tensors[index];
        const elements = extent(logicalLength * leaf.elements);
        const bytes = extent(elements * leaf.size);
        workspace = align(workspace, leaf.size);
        if (tensor) tensor(index, wire, workspace, elements, parameter);
        wire = extent(wire + bytes);
        workspace = extent(workspace + bytes);
      }
    }
    return { wire, workspace };
  }

  sizes(lengths) {
    if (lengths.length !== this.dynamicParameters) throw new RangeError("payload lengths do not match schema");
    let index = 0;
    return this.walk(() => lengths[index++], null);
  }

  requiredWorkspace(input) {
    const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const sizes = this.walk((offset) => {
      if (offset + 4 > bytes.byteLength) throw new RangeError("truncated payload slice length");
      const length = view.getInt32(offset, true);
      if (length < 0) throw new RangeError("negative payload slice length");
      return length;
    }, null);
    if (sizes.wire > bytes.byteLength) throw new RangeError("truncated payload");
    if (sizes.wire < bytes.byteLength) throw new RangeError("unexpected trailing payload bytes");
    return sizes.workspace;
  }
  /** Encode logical values outside realtime dispatch, preserving exact i64 values.
   * Accept ordered parameter arrays or objects keyed by the declared names. */
  encode(values) {
    const positional = Array.isArray(values);
    const named = !positional && values !== null && typeof values === "object";
    if (!positional && !named) throw new TypeError("payload values must be an ordered array or named object");
    if (positional && values.length > this.parameters.length) {
      throw new RangeError("payload parameter count exceeds schema");
    }
    if (named) {
      const unexpected = Object.keys(values).find((name) => !this.#parameterNames.has(name));
      if (unexpected !== undefined) throw new TypeError(`unexpected payload parameter '${unexpected}'`);
    }
    const roots = this.schema.params.map((field, index) => {
      let value;
      if (positional) value = values[index];
      else if (named && Object.hasOwn(values, field.name)) value = values[field.name];
      const result = value === undefined ? this.defaults[index] : value;
      if (result === undefined) throw new TypeError(`missing payload parameter '${field.name}'`);
      validateValue(field.ty, result);
      return result;
    });
    const sizes = this.walk((_, parameter) => roots[parameter].length, null);
    const bytes = new Uint8Array(sizes.wire);
    const view = new DataView(bytes.buffer);
    this.walk((_, parameter) => roots[parameter].length, (index, offset, _workspace, count, parameter) => {
      const tensor = this.tensors[index];
      for (let element = 0; element < count; element += 1) {
        writeScalar(view, offset + element * tensor.size, tensor.encoding, leafValue(roots[parameter], tensor.steps, element));
      }
    }, (offset, _workspace, length) => view.setInt32(offset, length, true));
    return bytes;
  }

  /** Decode one complete wire payload into nested host values. */
  decode(input) {
    const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
    this.requiredWorkspace(bytes);
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const roots = this.schema.params.map((field) => field.ty.kind === "slice" ? null : emptyValue(field.ty));
    this.walk((offset, parameter) => {
      const length = view.getInt32(offset, true);
      roots[parameter] = emptyValue(this.schema.params[parameter].ty, length);
      return length;
    }, (index, offset, _workspace, count, parameter) => {
      const tensor = this.tensors[index];
      for (let element = 0; element < count; element += 1) {
        roots[parameter] = leafValue(roots[parameter], tensor.steps, element,
          readScalar(view, offset + element * tensor.size, tensor.encoding), true);
      }
    });
    return Object.fromEntries(this.schema.params.map((field, index) => [field.name, roots[index]]));
  }
}

export function writeEventInput(memory, address, payloadAddress, payloadBytes, workspaceAddress, workspaceCapacityBytes) {
  const view = new DataView(memory.buffer ?? memory);
  for (const value of [address, payloadAddress, payloadBytes, workspaceAddress, workspaceCapacityBytes]) extent(value);
  if (address % 4 || workspaceAddress % 8) throw new RangeError("misaligned event input storage");
  if (address + EVENT_INPUT_SIZE_BYTES > view.byteLength
      || payloadAddress + payloadBytes > view.byteLength
      || workspaceAddress + workspaceCapacityBytes > view.byteLength) throw new RangeError("event input exceeds memory");
  view.setUint32(address, payloadAddress, true);
  view.setUint32(address + 4, payloadBytes, true);
  view.setUint32(address + 8, workspaceAddress, true);
  view.setUint32(address + 12, workspaceCapacityBytes, true);
}

function scalarValue(encoding, value) {
  if (encoding === "bool") {
    if (typeof value !== "boolean") throw new TypeError("bool payload value must be a boolean");
    return value;
  }
  if (encoding === "i64") {
    if (typeof value === "number" && !Number.isSafeInteger(value)) throw new TypeError("i64 payload value must be an exact integer");
    let integer;
    if (typeof value === "string") {
      integer = parseSignedDecimalInteger(value, "invalid i64 payload value");
    } else {
      if (typeof value !== "bigint" && typeof value !== "number") {
        throw new TypeError("invalid i64 payload value");
      }
      integer = BigInt(value);
    }
    if (BigInt.asIntN(64, integer) !== integer) throw new RangeError("i64 payload value is outside the signed 64-bit range");
    return integer;
  }
  if (typeof value !== "number") throw new TypeError(`${encoding} payload value must be a number`);
  if (encoding === "i32" && (!Number.isInteger(value) || value < -2147483648 || value > 2147483647)) {
    throw new RangeError("i32 payload value is outside the signed 32-bit range");
  }
  return value;
}
function parseScalarDefault(encoding, value) {
  if (typeof value !== "string") throw new TypeError("invalid scalar payload default");
  switch (encoding) {
    case "bool":
      if (value !== "true" && value !== "false") throw new TypeError("invalid bool payload default");
      return value === "true";
    case "i32":
      return scalarValue(encoding, Number(parseSignedDecimalInteger(value, "invalid i32 payload default")));
    case "i64":
      return scalarValue(encoding, parseSignedDecimalInteger(value, "invalid i64 payload default"));
    case "f32": case "f64":
      if (!/^[+-]?(?:(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?|inf(?:inity)?|nan)$/i.test(value)) {
        throw new TypeError("invalid floating-point payload default");
      }
      return /^[+-]?inf(?:inity)?$/i.test(value)
        ? (value.startsWith("-") ? -Infinity : Infinity)
        : Number(value);
    default: throw new TypeError("invalid payload scalar encoding");
  }
}
function parsePayloadDefault(ty, value) {
  if (ty.kind === "scalar") return parseScalarDefault(ty.encoding, value);
  if (!Array.isArray(value)) throw new TypeError("invalid aggregate payload default");
  if (ty.kind === "struct") {
    if (value.length !== ty.fields.length) throw new TypeError("payload default field count mismatch");
    return Object.fromEntries(ty.fields.map((field, index) =>
      [field.name, parsePayloadDefault(field.ty, value[index])]));
  }
  if (ty.kind === "tuple") {
    if (value.length !== ty.elements.length) throw new TypeError("payload default tuple length mismatch");
    return value.map((entry, index) => parsePayloadDefault(ty.elements[index], entry));
  }
  if (ty.kind === "array" && value.length === ty.len) {
    return value.map((entry) => parsePayloadDefault(ty.element, entry));
  }
  throw new TypeError("invalid payload default shape");
}
function validateValue(ty, value) {
  switch (ty.kind) {
    case "scalar": scalarValue(ty.encoding, value); return;
    case "struct": {
      const keys = value && typeof value === "object" && !Array.isArray(value) ? Object.keys(value) : [];
      if (!value || typeof value !== "object" || Array.isArray(value) || keys.length !== ty.fields.length
          || ty.fields.some((field) => !Object.hasOwn(value, field.name))) {
        throw new TypeError(`payload '${ty.name}' must be an object with exactly its declared fields`);
      }
      for (const field of ty.fields) validateValue(field.ty, value[field.name]);
      return;
    }
    case "tuple":
      if (!Array.isArray(value) || value.length !== ty.elements.length) throw new TypeError("payload tuple length mismatch");
      for (let index = 0; index < value.length; index += 1) validateValue(ty.elements[index], value[index]);
      return;
    case "slice": case "array":
      if (!Array.isArray(value) && !(ArrayBuffer.isView(value) && !(value instanceof DataView))) throw new TypeError("payload array must be a sequence");
      extent(value.length);
      if (ty.kind === "array" && value.length !== ty.len) throw new RangeError("payload fixed array length mismatch");
      for (let index = 0; index < value.length; index += 1) validateValue(ty.element, value[index]);
      return;
    default: throw new TypeError("invalid payload value type");
  }
}
function emptyValue(ty, length = 0) {
  switch (ty.kind) {
    case "scalar": return null;
    case "struct": return Object.fromEntries(ty.fields.map((field) => [field.name, emptyValue(field.ty)]));
    case "tuple": return ty.elements.map((element) => emptyValue(element));
    case "array": return Array.from({ length: ty.len }, () => emptyValue(ty.element));
    case "slice": return Array.from({ length }, () => emptyValue(ty.element));
    default: throw new TypeError("invalid payload type");
  }
}
function leafValue(root, steps, index, replacement, write) {
  if (steps.length === 0) return write ? replacement : root;
  let current = root;
  for (let part = 0; part < steps.length; part += 1) {
    const step = steps[part];
    const key = step.field !== undefined ? step.field : (step.axis === null ? Math.floor(index / step.stride) : Math.floor(index / step.stride) % step.axis);
    if (write && part === steps.length - 1) current[key] = replacement;
    else current = current[key];
  }
  return write ? root : current;
}
function readScalar(view, offset, encoding) {
  switch (encoding) {
    case "bool": return view.getUint8(offset) !== 0;
    case "i32": return view.getInt32(offset, true);
    case "i64": return view.getBigInt64(offset, true);
    case "f32": return canonicalF32Number(view.getFloat32(offset, true));
    case "f64": return view.getFloat64(offset, true);
  }
}
function writeScalar(view, offset, encoding, value) {
  switch (encoding) {
    case "bool": view.setUint8(offset, value ? 1 : 0); break;
    case "i32": view.setInt32(offset, value, true); break;
    case "i64": view.setBigInt64(offset, scalarValue(encoding, value), true); break;
    case "f32": view.setFloat32(offset, value, true); break;
    case "f64": view.setFloat64(offset, value, true); break;
  }
}
