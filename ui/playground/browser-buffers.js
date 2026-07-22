// Shared browser-playground buffer loading and validation.
const MAX_BUFFER_SAMPLES = 16 * 1024 * 1024;
export const UNBOUND_BUFFERS_MESSAGE = "Bind all buffers to start processing";

export async function prepareBufferBindings(metadata, files) {
  if (metadata.metadata.buffers.some((buffer) => !files.has(buffer.name))) {
    throw new Error(UNBOUND_BUFFERS_MESSAGE);
  }
  const bindings = {};
  for (const buffer of metadata.metadata.buffers) {
    const file = files.get(buffer.name);
    bindings[buffer.name] = await prepareBufferBinding(buffer, file);
  }
  return bindings;
}

export async function prepareBufferBinding(buffer, file) {
  if (buffer.scalar !== "f32") {
    throw new Error(
      `Onda buffer '${buffer.name}' is ${buffer.scalar}; WAV loading supports f32 buffers`,
    );
  }
  const decoded = decodeWav(await file.arrayBuffer());
  const declaredChannels = Number(buffer.static_channels ?? 0);
  if (declaredChannels && decoded.channels !== declaredChannels) {
    throw new Error(
      `Onda buffer '${buffer.name}' requires ${declaredChannels} channel(s), but '${file.name}' has ${decoded.channels}`,
    );
  }
  return decoded;
}

export function decodeWav(input) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (bytes.byteLength < 12 || ascii(bytes, 0, 4) !== "RIFF" || ascii(bytes, 8, 4) !== "WAVE") {
    throw new Error("buffer files must be little-endian RIFF/WAVE audio");
  }

  let format = null;
  let dataOffset = -1;
  let dataLength = 0;
  for (let offset = 12; offset + 8 <= bytes.byteLength;) {
    const id = ascii(bytes, offset, 4);
    const length = view.getUint32(offset + 4, true);
    const body = offset + 8;
    if (body + length > bytes.byteLength) throw new Error(`WAV chunk '${id}' is truncated`);
    if (id === "fmt ") format = readFormat(view, body, length);
    if (id === "data" && dataOffset < 0) {
      dataOffset = body;
      dataLength = length;
    }
    offset = body + length + (length & 1);
  }
  if (!format) throw new Error("WAV file has no format chunk");
  if (dataOffset < 0) throw new Error("WAV file has no audio data chunk");
  const frames = Math.floor(dataLength / format.blockAlign);
  const sampleCount = frames * format.channels;
  if (frames <= 0) throw new Error("WAV file contains no complete audio frames");
  if (sampleCount > MAX_BUFFER_SAMPLES) {
    throw new Error(
      `WAV file has ${sampleCount.toLocaleString()} samples; the browser limit is ${MAX_BUFFER_SAMPLES.toLocaleString()}`,
    );
  }

  const data = new Float32Array(sampleCount);
  const bytesPerSample = format.bitsPerSample / 8;
  for (let frame = 0; frame < frames; frame += 1) {
    const frameOffset = dataOffset + frame * format.blockAlign;
    for (let channel = 0; channel < format.channels; channel += 1) {
      const offset = frameOffset + channel * bytesPerSample;
      data[frame * format.channels + channel] = readSample(
        view,
        offset,
        format.encoding,
        format.bitsPerSample,
      );
    }
  }
  return {
    data,
    frames,
    channels: format.channels,
    sampleRate: format.sampleRate,
  };
}

function readFormat(view, offset, length) {
  if (length < 16) throw new Error("WAV format chunk is too short");
  let encoding = view.getUint16(offset, true);
  const channels = view.getUint16(offset + 2, true);
  const sampleRate = view.getUint32(offset + 4, true);
  const blockAlign = view.getUint16(offset + 12, true);
  const bitsPerSample = view.getUint16(offset + 14, true);
  if (encoding === 0xfffe && length >= 40) encoding = view.getUint16(offset + 24, true);
  if (encoding !== 1 && encoding !== 3) {
    throw new Error(`WAV encoding ${encoding} is unsupported; use PCM or IEEE float`);
  }
  const supportedBits = encoding === 1
    ? new Set([8, 16, 24, 32])
    : new Set([32, 64]);
  if (!supportedBits.has(bitsPerSample)) {
    throw new Error(`WAV ${bitsPerSample}-bit ${encoding === 1 ? "PCM" : "float"} audio is unsupported`);
  }
  const minimumBlockAlign = channels * (bitsPerSample / 8);
  if (
    !Number.isInteger(channels) || channels <= 0
    || !Number.isFinite(sampleRate) || sampleRate <= 0
    || blockAlign < minimumBlockAlign
  ) {
    throw new Error("WAV format metadata is invalid");
  }
  return { encoding, channels, sampleRate, blockAlign, bitsPerSample };
}

function readSample(view, offset, encoding, bits) {
  if (encoding === 3) {
    const value = bits === 32
      ? view.getFloat32(offset, true)
      : view.getFloat64(offset, true);
    return Number.isFinite(value) ? value : 0;
  }
  if (bits === 8) return (view.getUint8(offset) - 128) / 128;
  if (bits === 16) return view.getInt16(offset, true) / 32768;
  if (bits === 24) {
    let value = view.getUint8(offset)
      | (view.getUint8(offset + 1) << 8)
      | (view.getUint8(offset + 2) << 16);
    if (value & 0x800000) value |= 0xff000000;
    return value / 8388608;
  }
  return view.getInt32(offset, true) / 2147483648;
}

function ascii(bytes, offset, length) {
  return String.fromCharCode(...bytes.subarray(offset, offset + length));
}
