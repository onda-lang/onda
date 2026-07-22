const SHARE_SCHEMA_VERSION = 1;
const MAX_ENCODED_CHARACTERS = 256_000;
const MAX_DECODED_BYTES = 1_000_000;

export async function encodeSharedSession(session) {
  const source = new TextEncoder().encode(JSON.stringify({
    ...session,
    v: SHARE_SCHEMA_VERSION,
  }));
  if (source.byteLength > MAX_DECODED_BYTES) {
    throw new Error("this project is too large to share in a URL");
  }

  let format = "j";
  let payload = source;
  if (typeof CompressionStream === "function") {
    try {
      const compressed = await transformBytes(
        source,
        new CompressionStream("gzip"),
        MAX_DECODED_BYTES,
      );
      if (compressed.byteLength < source.byteLength) {
        format = "z";
        payload = compressed;
      }
    } catch {
      // Plain JSON remains interoperable if browser compression is unavailable.
    }
  }

  const encoded = `${format}${bytesToBase64Url(payload)}`;
  if (encoded.length > MAX_ENCODED_CHARACTERS) {
    throw new Error("this project is too large to share in a URL");
  }
  return encoded;
}

export async function decodeSharedSession(hash) {
  const encoded = new URLSearchParams(String(hash ?? "").replace(/^#/, "")).get("p");
  if (!encoded) return null;
  if (encoded.length > MAX_ENCODED_CHARACTERS) {
    throw new Error("the shared playground URL is too large");
  }

  const format = encoded[0];
  const data = encoded.slice(1);
  if (!/^[A-Za-z0-9_-]+$/.test(data)) {
    throw new Error("the shared playground URL is malformed");
  }

  let bytes = base64UrlToBytes(data);
  if (format === "z") {
    if (typeof DecompressionStream !== "function") {
      throw new Error("this browser cannot decompress the shared playground URL");
    }
    bytes = await transformBytes(
      bytes,
      new DecompressionStream("gzip"),
      MAX_DECODED_BYTES,
    );
  } else if (format !== "j") {
    throw new Error(`unsupported shared playground format '${format}'`);
  }
  if (bytes.byteLength > MAX_DECODED_BYTES) {
    throw new Error("the shared playground project is too large");
  }

  let session;
  try {
    session = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw new Error("the shared playground URL does not contain a valid project");
  }
  if (!session || typeof session !== "object" || session.v !== SHARE_SCHEMA_VERSION) {
    throw new Error("unsupported shared playground project version");
  }
  return session;
}

export function sharedSessionHash(encoded) {
  return `#p=${encoded}`;
}

async function transformBytes(bytes, transformer, maximumBytes) {
  const readable = new Blob([bytes]).stream().pipeThrough(transformer);
  const reader = readable.getReader();
  const chunks = [];
  let length = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > maximumBytes) {
      await reader.cancel();
      throw new Error("shared playground data exceeds the size limit");
    }
    chunks.push(value);
  }
  const result = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

function bytesToBase64Url(bytes) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
}

function base64UrlToBytes(encoded) {
  const padded = encoded
    .replaceAll("-", "+")
    .replaceAll("_", "/")
    .padEnd(Math.ceil(encoded.length / 4) * 4, "=");
  let binary;
  try {
    binary = atob(padded);
  } catch {
    throw new Error("the shared playground URL is malformed");
  }
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
