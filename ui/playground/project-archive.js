const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
const MAX_ARCHIVE_BYTES = 256 * 1024 * 1024;
const MAX_PATH_BYTES = 16 * 1024;
const MAX_PATH_COMPONENT_BYTES = 255;
// Shared materialized-file budget for manifests, source documents, and assets.
const MAX_ARCHIVE_FILES = 4096 + 4096 + 1;

export async function createProjectArchive(imageBytes, projectApi, assetFileNames = new Map()) {
  requireProjectApi(projectApi);
  const materialized = await projectApi.materializeProjectImage(imageBytes, assetFileNames);
  return createZip(new Map(
    materialized.files.map((file) => [file.path, file.bytes]),
  ));
}

export async function decodeProjectArchive(input, projectApi, selectProject = null) {
  requireProjectApi(projectApi);
  const files = await readZip(input);
  const projectFilePaths = [...files]
    .filter(([path, bytes]) => isProjectManifest(path, bytes))
    .map(([path]) => path)
    .sort();
  if (projectFilePaths.length === 0) {
    throw new Error("project archive has no valid .ondaproject file");
  }
  let projectFilePath = projectFilePaths[0];
  if (projectFilePaths.length > 1) {
    if (typeof selectProject !== "function") {
      throw new Error("project archive contains multiple projects and requires a selection");
    }
    projectFilePath = await selectProject([...projectFilePaths]);
    if (!projectFilePaths.includes(projectFilePath)) {
      throw new Error(`selected project manifest '${projectFilePath}' is not in the archive`);
    }
  }
  const sourceManifest = JSON.parse(decoder.decode(files.get(projectFilePath)));
  const sourceBufferFiles = manifestBufferFiles(sourceManifest);
  const assetFileNames = new Map(
    [...sourceBufferFiles].map(([name, path]) => [name, path.split("/").at(-1)]),
  );
  const image = await projectApi.loadProjectFiles(files, projectFilePath);
  const materialized = await projectApi.materializeProjectImage(image.bytes, assetFileNames);
  const canonicalFiles = new Map(materialized.files.map((file) => [file.path, file.bytes]));
  const projectFile = materialized.files.find((file) =>
    !file.path.includes("/") && file.path.toLowerCase().endsWith(".ondaproject")
  );
  if (!projectFile) throw new Error("materialized project file is missing");
  const manifest = JSON.parse(decoder.decode(projectFile.bytes));
  const materializedBufferFiles = manifestBufferFiles(manifest);
  const sources = Object.fromEntries(
    image.sourceGraph.documents.map((document) => [document.path, document.contents]),
  );
  const buffers = new Map();
  for (const buffer of image.buffers) {
    const path = materializedBufferFiles.get(buffer.name);
    if (!path) {
      throw new Error(`materialized project buffer '${buffer.name}' has no file binding`);
    }
    const bytes = canonicalFiles.get(path);
    if (!bytes) throw new Error(`materialized project buffer '${buffer.name}' is missing`);
    buffers.set(buffer.name, { path, bytes });
  }
  return {
    project: {
      entry: image.sourceGraph.entry,
      active: image.sourceGraph.entry,
      sources,
    },
    buffers,
    image,
  };
}

function isProjectManifest(path, bytes) {
  if (!path.toLowerCase().endsWith(".ondaproject")) return false;
  try {
    const manifest = JSON.parse(decoder.decode(bytes));
    return manifest !== null
      && typeof manifest === "object"
      && !Array.isArray(manifest)
      && typeof manifest.entry === "string";
  } catch {
    return false;
  }
}

function manifestBufferFiles(manifest) {
  const files = new Map();
  for (const [name, binding] of Object.entries(manifest?.buffers ?? {})) {
    if (Array.isArray(binding)) {
      binding.forEach((element, index) => {
        if (typeof element?.file === "string") files.set(`${name}[${index}]`, element.file);
      });
    } else if (typeof binding?.file === "string") {
      files.set(name, binding.file);
    }
  }
  return files;
}

export function sourceGraphWithWorkspaceDocuments(sourceGraph, sources) {
  if (!sourceGraph || !sources || typeof sources !== "object" || Array.isArray(sources)) {
    throw new Error("project export requires a source graph and workspace sources");
  }
  return {
    ...sourceGraph,
    documents: Object.entries(sources).map(([path, contents]) => ({ path, contents })),
  };
}

function requireProjectApi(projectApi) {
  if (!projectApi?.loadProjectFiles || !projectApi?.materializeProjectImage) {
    throw new Error("project archives require the canonical Onda project API");
  }
}

export function createZip(inputFiles) {
  const archivePaths = new Map();
  const files = [...inputFiles]
    .map(([path, input]) => {
      const directory = path.endsWith("/");
      const portable = directory ? path.slice(0, -1) : path;
      if (!portablePath(portable) || !registerArchivePath(archivePaths, portable, directory)) {
        throw new Error(`archive path '${path}' is not portable or collides with another path`);
      }
      const name = encoder.encode(path);
      const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
      if (directory && bytes.byteLength !== 0) {
        throw new Error(`archive directory '${path}' must be empty`);
      }
      return { path, name, bytes, crc: crc32(bytes) };
    })
    .sort((left, right) => left.path.localeCompare(right.path));
  if (files.length > MAX_ARCHIVE_FILES) throw new Error("project archive has too many files");
  const localParts = [];
  const centralParts = [];
  let offset = 0;
  for (const file of files) {
    const local = new Uint8Array(30 + file.name.byteLength);
    const localView = new DataView(local.buffer);
    localView.setUint32(0, 0x04034b50, true);
    localView.setUint16(4, 20, true);
    localView.setUint16(6, 0x0800, true);
    localView.setUint16(8, 0, true);
    localView.setUint32(14, file.crc, true);
    localView.setUint32(18, file.bytes.byteLength, true);
    localView.setUint32(22, file.bytes.byteLength, true);
    localView.setUint16(26, file.name.byteLength, true);
    local.set(file.name, 30);
    localParts.push(local, file.bytes);

    const central = new Uint8Array(46 + file.name.byteLength);
    const centralView = new DataView(central.buffer);
    centralView.setUint32(0, 0x02014b50, true);
    centralView.setUint16(4, 20, true);
    centralView.setUint16(6, 20, true);
    centralView.setUint16(8, 0x0800, true);
    centralView.setUint16(10, 0, true);
    centralView.setUint32(16, file.crc, true);
    centralView.setUint32(20, file.bytes.byteLength, true);
    centralView.setUint32(24, file.bytes.byteLength, true);
    centralView.setUint16(28, file.name.byteLength, true);
    centralView.setUint32(42, offset, true);
    central.set(file.name, 46);
    centralParts.push(central);
    offset += local.byteLength + file.bytes.byteLength;
    if (offset > MAX_ARCHIVE_BYTES) throw new Error("project archive is too large");
  }
  const centralOffset = offset;
  const centralSize = centralParts.reduce((size, part) => size + part.byteLength, 0);
  const end = new Uint8Array(22);
  const endView = new DataView(end.buffer);
  endView.setUint32(0, 0x06054b50, true);
  endView.setUint16(8, files.length, true);
  endView.setUint16(10, files.length, true);
  endView.setUint32(12, centralSize, true);
  endView.setUint32(16, centralOffset, true);
  const archiveSize = centralOffset + centralSize + end.byteLength;
  if (archiveSize > MAX_ARCHIVE_BYTES) throw new Error("project archive is too large");
  return concatenate([...localParts, ...centralParts, end], archiveSize);
}

export async function readZip(input) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  if (bytes.byteLength > MAX_ARCHIVE_BYTES) throw new Error("project archive is too large");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const endOffset = findEndRecord(view);
  const fileCount = view.getUint16(endOffset + 10, true);
  const centralSize = view.getUint32(endOffset + 12, true);
  const centralOffset = view.getUint32(endOffset + 16, true);
  if (fileCount > MAX_ARCHIVE_FILES) throw new Error("project archive has too many files");
  if (centralOffset + centralSize > endOffset) {
    throw new Error("project ZIP central directory is invalid");
  }
  const files = new Map();
  const archivePaths = new Map();
  let cursor = centralOffset;
  let totalBytes = 0;
  for (let index = 0; index < fileCount; index += 1) {
    if (cursor + 46 > bytes.byteLength || view.getUint32(cursor, true) !== 0x02014b50) {
      throw new Error("project ZIP central directory is invalid");
    }
    const method = view.getUint16(cursor + 10, true);
    const expectedCrc = view.getUint32(cursor + 16, true);
    const compressedSize = view.getUint32(cursor + 20, true);
    const uncompressedSize = view.getUint32(cursor + 24, true);
    const nameLength = view.getUint16(cursor + 28, true);
    const extraLength = view.getUint16(cursor + 30, true);
    const commentLength = view.getUint16(cursor + 32, true);
    const localOffset = view.getUint32(cursor + 42, true);
    const recordEnd = cursor + 46 + nameLength + extraLength + commentLength;
    if (recordEnd > bytes.byteLength) {
      throw new Error("project ZIP central directory is truncated");
    }
    const name = decoder.decode(bytes.subarray(cursor + 46, cursor + 46 + nameLength));
    const directory = name.endsWith("/");
    const path = directory ? name.slice(0, -1) : name;
    if (!portablePath(path) || !registerArchivePath(archivePaths, path, directory)) {
      throw new Error(`project ZIP contains invalid or duplicate path '${name}'`);
    }
    if (localOffset + 30 > bytes.byteLength || view.getUint32(localOffset, true) !== 0x04034b50) {
      throw new Error(`project ZIP entry '${name}' has an invalid local header`);
    }
    const localNameLength = view.getUint16(localOffset + 26, true);
    const localExtraLength = view.getUint16(localOffset + 28, true);
    const dataOffset = localOffset + 30 + localNameLength + localExtraLength;
    const compressed = bytes.subarray(dataOffset, dataOffset + compressedSize);
    if (compressed.byteLength !== compressedSize) {
      throw new Error(`project ZIP entry '${name}' is truncated`);
    }
    const remainingBytes = MAX_ARCHIVE_BYTES - totalBytes;
    if (uncompressedSize > remainingBytes) {
      throw new Error("expanded project archive is too large");
    }
    const contents = method === 0
      ? compressed.slice()
      : method === 8
        ? await inflateRaw(compressed, remainingBytes, uncompressedSize)
        : null;
    if (!contents) throw new Error(`project ZIP entry '${name}' uses compression method ${method}`);
    if (contents.byteLength !== uncompressedSize || crc32(contents) !== expectedCrc) {
      throw new Error(`project ZIP entry '${name}' failed integrity validation`);
    }
    totalBytes += contents.byteLength;
    if (directory) {
      if (contents.byteLength !== 0) {
        throw new Error(`project ZIP directory '${name}' is not empty`);
      }
    } else {
      files.set(path, contents);
    }
    cursor = recordEnd;
  }
  return files;
}

async function inflateRaw(bytes, maxBytes, expectedBytes) {
  if (typeof DecompressionStream !== "function") {
    throw new Error("this browser cannot decompress ZIP files");
  }
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  const reader = stream.getReader();
  const output = new Uint8Array(expectedBytes);
  let totalBytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      totalBytes += value.byteLength;
      if (totalBytes > maxBytes) {
        await reader.cancel();
        throw new Error("expanded project archive is too large");
      }
      if (totalBytes > expectedBytes) {
        await reader.cancel();
        throw new Error("project ZIP entry expands beyond its declared size");
      }
      output.set(value, totalBytes - value.byteLength);
    }
  } finally {
    reader.releaseLock();
  }
  return totalBytes === expectedBytes ? output : output.subarray(0, totalBytes);
}

function findEndRecord(view) {
  const minimum = Math.max(0, view.byteLength - 65_557);
  for (let offset = view.byteLength - 22; offset >= minimum; offset -= 1) {
    if (view.getUint32(offset, true) === 0x06054b50) return offset;
  }
  throw new Error("file is not a ZIP archive");
}

function portablePath(path) {
  return typeof path === "string"
    && path.length > 0
    && encoder.encode(path).byteLength <= MAX_PATH_BYTES
    && path.normalize("NFC") === path
    && !path.startsWith("/")
    && !path.includes("\\")
    && !/^[A-Za-z]:/.test(path)
    && !path.split("/").some((part) =>
      !part
      || part === "."
      || part === ".."
      || encoder.encode(part).byteLength > MAX_PATH_COMPONENT_BYTES
    );
}

function registerArchivePath(paths, path, directory) {
  const key = portablePathCollisionKey(path);
  if (paths.has(key)) return false;
  for (let separator = key.indexOf("/"); separator >= 0; separator = key.indexOf("/", separator + 1)) {
    const ancestor = paths.get(key.slice(0, separator));
    if (ancestor && !ancestor.directory) return false;
  }
  if (!directory) {
    const descendantPrefix = `${key}/`;
    if ([...paths.keys()].some((existing) => existing.startsWith(descendantPrefix))) return false;
  }
  paths.set(key, { directory });
  return true;
}

function portablePathCollisionKey(path) {
  // Closing over lower/upper/lower catches Unicode caseless equivalences which
  // a single lowercase pass misses (for example Greek final sigma and capital
  // sharp S). NFC keeps the comparison aligned with the canonical path format.
  return path.toLowerCase().toUpperCase().toLowerCase().normalize("NFC");
}

function concatenate(parts, totalBytes = parts.reduce((size, part) => size + part.byteLength, 0)) {
  const output = new Uint8Array(totalBytes);
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.byteLength;
  }
  return output;
}

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}
