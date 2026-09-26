export function scalarSize(scalar) {
  switch (scalar) {
    case "bool":
      return 1;
    case "i32":
    case "f32":
      return 4;
    case "i64":
    case "f64":
      return 8;
    default:
      throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

export function writeParameterDefaults(memory, paramsPointer, params) {
  for (const param of params) {
    if (param.default_reprs === null) continue;
    if (param.default_reprs.length !== param.array_len) {
      throw new Error(
        `parameter '${param.name}' default has ${param.default_reprs.length} values, expected ${param.array_len}`,
      );
    }
    const elementSize = scalarSize(param.scalar);
    for (const [index, representation] of param.default_reprs.entries()) {
      writeScalar(
        memory,
        paramsPointer + param.byte_offset + index * elementSize,
        param.scalar,
        representation,
      );
    }
  }
}

function writeScalar(memory, pointer, scalar, representation) {
  const view = new DataView(memory.buffer);
  switch (scalar) {
    case "bool":
      view.setUint8(pointer, JSON.parse(representation) ? 1 : 0);
      break;
    case "i32":
      view.setInt32(pointer, JSON.parse(representation), true);
      break;
    case "i64":
      view.setBigInt64(pointer, BigInt(representation), true);
      break;
    case "f32":
      writeFloat(view, pointer, representation, 32);
      break;
    case "f64":
      writeFloat(view, pointer, representation, 64);
      break;
    default:
      throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

function writeFloat(view, pointer, representation, width) {
  if (!representation.startsWith("0x")) {
    const value = JSON.parse(representation);
    if (typeof value !== "number") {
      throw new Error(`invalid f${width} scalar '${representation}'`);
    }
    if (width === 32) view.setFloat32(pointer, value, true);
    else view.setFloat64(pointer, value, true);
    return;
  }
  const digits = representation.slice(2);
  if (digits.length !== width / 4 || !/^[0-9a-f]+$/i.test(digits)) {
    throw new Error(`invalid f${width} bit-pattern scalar '${representation}'`);
  }
  if (width === 32) {
    view.setUint32(pointer, Number.parseInt(digits, 16), true);
  } else {
    view.setBigUint64(pointer, BigInt(representation), true);
  }
}
