// Shared browser-playground buffer loading and validation.
const MAX_BUFFER_SAMPLES = 16 * 1024 * 1024;

export async function prepareBufferBindings(metadata, files, projectApi) {
  const bindings = {};
  for (const buffer of metadata.metadata.buffers) {
    const file = files.get(buffer.name);
    if (!file) continue;
    bindings[buffer.name] = await prepareBufferBinding(buffer, file, projectApi);
  }
  return bindings;
}

export async function prepareBufferBinding(buffer, file, projectApi) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  if (!projectApi?.decodeBufferFile) {
    throw new Error("buffer decoding requires the canonical project API");
  }
  const asset = await projectApi.decodeBufferFile(bytes, file.name);
  if (asset.data.length > MAX_BUFFER_SAMPLES) {
    throw new Error(`buffer exceeds the ${MAX_BUFFER_SAMPLES} sample browser limit`);
  }
  const decoded = { ...asset, scalar: asset.element };
  if (decoded.scalar !== buffer.scalar) {
    throw new Error(
      `Onda buffer '${buffer.name}' requires ${buffer.scalar}, but '${file.name}' contains ${decoded.scalar}`,
    );
  }
  const declaredChannels = Number(buffer.static_channels ?? 0);
  if (declaredChannels && decoded.channels !== declaredChannels) {
    throw new Error(
      `Onda buffer '${buffer.name}' requires ${declaredChannels} channel(s), but '${file.name}' has ${decoded.channels}`,
    );
  }
  return { ...decoded, waveform: bufferWaveform(decoded) };
}

function bufferWaveform({ data, frames, channels }) {
  const columns = Math.min(frames, 128);
  const minimums = [];
  const maximums = [];
  let minValue = Infinity;
  let maxValue = -Infinity;
  for (let column = 0; column < columns; column++) {
    const start = Math.floor(column * frames / columns) * channels;
    const end = Math.min(Math.floor((column + 1) * frames / columns) * channels, data.length);
    let minimum = Infinity;
    let maximum = -Infinity;
    for (let index = start; index < end; index++) {
      const value = Number(data[index]);
      if (!Number.isFinite(value)) continue;
      minimum = Math.min(minimum, value);
      maximum = Math.max(maximum, value);
    }
    if (!Number.isFinite(minimum)) minimum = maximum = 0;
    minimums.push(minimum);
    maximums.push(maximum);
    minValue = Math.min(minValue, minimum);
    maxValue = Math.max(maxValue, maximum);
  }
  return {
    minValue: Number.isFinite(minValue) ? minValue : 0,
    maxValue: Number.isFinite(maxValue) ? maxValue : 0,
    minimums,
    maximums,
  };
}

const ONDA_BUFFER_MAGIC = new TextEncoder().encode("ONDABUF\0");

export function isOndaBuffer(bytes) {
  return bytes.byteLength >= ONDA_BUFFER_MAGIC.byteLength
    && ONDA_BUFFER_MAGIC.every((byte, index) => bytes[index] === byte);
}

export async function decodeOndaBuffer(input, projectApi) {
  if (!projectApi?.decodeBufferAsset) {
    throw new Error("Onda buffer decoding requires the canonical project API");
  }
  const decoded = await projectApi.decodeBufferAsset(input);
  if (decoded.data.length > MAX_BUFFER_SAMPLES) {
    throw new Error(`Onda buffer exceeds the ${MAX_BUFFER_SAMPLES} sample browser limit`);
  }
  return { ...decoded, scalar: decoded.element };
}

export async function encodeOndaBuffer(binding, scalar = "f32", projectApi) {
  if (!projectApi?.encodeBufferAsset) {
    throw new Error("Onda buffer encoding requires the canonical project API");
  }
  return projectApi.encodeBufferAsset({ ...binding, element: scalar });
}
