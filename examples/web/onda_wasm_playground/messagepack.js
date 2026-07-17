const utf8 = new TextDecoder("utf-8", { fatal: true });
const MAX_NESTING_DEPTH = 512;

export function decodeMirMessagePack(input) {
  const bytes = asBytes(input);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = 0;

  const requireBytes = (count) => {
    if (offset + count > bytes.byteLength) {
      throw new Error("truncated MessagePack MIR payload");
    }
  };
  const take = (count) => {
    requireBytes(count);
    const start = offset;
    offset += count;
    return start;
  };
  const uint8 = () => view.getUint8(take(1));
  const uint16 = () => view.getUint16(take(2), false);
  const uint32 = () => view.getUint32(take(4), false);
  const length32 = () => {
    const value = uint32();
    if (value > 0x7fff_ffff) throw new Error("MessagePack collection is too large");
    return value;
  };
  const integer64 = (signed) => {
    const start = take(8);
    const value = signed
      ? view.getBigInt64(start, false)
      : view.getBigUint64(start, false);
    const number = Number(value);
    if (!Number.isSafeInteger(number)) {
      throw new Error("MessagePack integer exceeds JavaScript's exact integer range");
    }
    return number;
  };
  const string = (length) => {
    const start = take(length);
    return utf8.decode(bytes.subarray(start, start + length));
  };
  const array = (length, depth) => {
    if (length > bytes.byteLength - offset) {
      throw new Error("truncated MessagePack MIR array");
    }
    const result = new Array(length);
    for (let index = 0; index < length; index += 1) {
      result[index] = read(depth + 1);
    }
    return result;
  };
  const map = (length, depth) => {
    if (length > Math.floor((bytes.byteLength - offset) / 2)) {
      throw new Error("truncated MessagePack MIR map");
    }
    const result = Object.create(null);
    for (let index = 0; index < length; index += 1) {
      const key = read(depth + 1);
      if (typeof key !== "string") throw new Error("MessagePack MIR map key is not a string");
      result[key] = read(depth + 1);
    }
    return result;
  };
  const binary = (length) => {
    const start = take(length);
    return bytes.slice(start, start + length);
  };

  function read(depth) {
    if (depth > MAX_NESTING_DEPTH) {
      throw new Error("MessagePack MIR nesting is too deep");
    }
    const marker = uint8();
    if (marker <= 0x7f) return marker;
    if (marker >= 0xe0) return marker - 0x100;
    if ((marker & 0xf0) === 0x80) return map(marker & 0x0f, depth);
    if ((marker & 0xf0) === 0x90) return array(marker & 0x0f, depth);
    if ((marker & 0xe0) === 0xa0) return string(marker & 0x1f);

    switch (marker) {
      case 0xc0: return null;
      case 0xc2: return false;
      case 0xc3: return true;
      case 0xc4: return binary(uint8());
      case 0xc5: return binary(uint16());
      case 0xc6: return binary(length32());
      case 0xca: return view.getFloat32(take(4), false);
      case 0xcb: return view.getFloat64(take(8), false);
      case 0xcc: return uint8();
      case 0xcd: return uint16();
      case 0xce: return uint32();
      case 0xcf: return integer64(false);
      case 0xd0: return view.getInt8(take(1));
      case 0xd1: return view.getInt16(take(2), false);
      case 0xd2: return view.getInt32(take(4), false);
      case 0xd3: return integer64(true);
      case 0xd9: return string(uint8());
      case 0xda: return string(uint16());
      case 0xdb: return string(length32());
      case 0xdc: return array(uint16(), depth);
      case 0xdd: return array(length32(), depth);
      case 0xde: return map(uint16(), depth);
      case 0xdf: return map(length32(), depth);
      default:
        throw new Error(`unsupported MessagePack marker 0x${marker.toString(16)}`);
    }
  }

  const result = read(0);
  if (offset !== bytes.byteLength) {
    throw new Error("trailing bytes after MessagePack MIR payload");
  }
  return result;
}

function asBytes(input) {
  if (input instanceof Uint8Array) return input;
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  }
  throw new TypeError("MessagePack MIR must be an ArrayBuffer or typed array");
}
