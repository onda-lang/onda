import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync } from "node:zlib";

import {
  createProjectArchive,
  decodeProjectArchive,
  createZip,
  readZip,
  sourceGraphWithWorkspaceDocuments,
} from "./project-archive.js";

test("project ZIP transport delegates project semantics to the canonical API", async () => {
  const asset = new Uint8Array([7, 8, 9]);
  const source = new TextEncoder().encode("const gain = 0.5\n");
  const materialized = {
    directories: ["assets", "code"],
    files: [
      {
        path: "project.ondaproject",
        bytes: new TextEncoder().encode(JSON.stringify({
          entry: "code/main.onda",
          buffers: { sample: { file: "assets/piano.ondabuffer" } },
        })),
      },
      { path: "code/main.onda", bytes: source },
      { path: "assets/piano.ondabuffer", bytes: asset },
    ],
  };
  let loadedFiles;
  const receivedAssetFileNames = [];
  const projectApi = {
    async materializeProjectImage(_imageBytes, assetFileNames) {
      receivedAssetFileNames.push(assetFileNames);
      return materialized;
    },
    async loadProjectFiles(files) {
      loadedFiles = files;
      return {
        bytes: new Uint8Array([1]),
        sourceGraph: {
          entry: "code/main.onda",
          documents: [{ path: "code/main.onda", contents: "const gain = 0.5\n" }],
        },
        buffers: [{ name: "sample", assetId: `sha256:${"a".repeat(64)}` }],
      };
    },
  };
  const archive = await createProjectArchive(
    new Uint8Array([1]),
    projectApi,
    new Map([["sample", "piano.wav"]]),
  );
  const decoded = await decodeProjectArchive(archive, projectApi);
  assert.equal(decoded.project.entry, "code/main.onda");
  assert.equal(decoded.project.sources["code/main.onda"], "const gain = 0.5\n");
  assert.deepEqual(decoded.buffers.get("sample").bytes, asset);
  assert.equal(loadedFiles.has("project.ondaproject"), true);
  assert.equal(receivedAssetFileNames[0].get("sample"), "piano.wav");
});

test("project ZIP transport resolves canonical buffer array slots", async () => {
  const encoder = new TextEncoder();
  const firstAsset = new Uint8Array([1, 2, 3]);
  const thirdAsset = new Uint8Array([7, 8, 9]);
  const manifestBytes = encoder.encode(JSON.stringify({
    entry: "code/main.onda",
    buffers: {
      bank: [
        { file: "assets/first.ondabuffer" },
        null,
        { file: "assets/third.ondabuffer" },
      ],
    },
  }));
  const materialized = {
    directories: ["assets", "code"],
    files: [
      { path: "project.ondaproject", bytes: manifestBytes },
      { path: "code/main.onda", bytes: encoder.encode("buffers:\n  bank: f32 {3}\n") },
      { path: "assets/first.ondabuffer", bytes: firstAsset },
      { path: "assets/third.ondabuffer", bytes: thirdAsset },
    ],
  };
  let receivedAssetFileNames;
  const projectApi = {
    async loadProjectFiles() {
      return {
        bytes: new Uint8Array([1]),
        sourceGraph: {
          entry: "code/main.onda",
          documents: [{ path: "code/main.onda", contents: "buffers:\n  bank: f32 {3}\n" }],
        },
        buffers: [{ name: "bank[0]" }, { name: "bank[2]" }],
      };
    },
    async materializeProjectImage(_bytes, assetFileNames) {
      receivedAssetFileNames = assetFileNames;
      return materialized;
    },
  };
  const archive = createZip(new Map(
    materialized.files.map((file) => [file.path, file.bytes]),
  ));

  const decoded = await decodeProjectArchive(archive, projectApi);

  assert.deepEqual(decoded.buffers.get("bank[0]").bytes, firstAsset);
  assert.deepEqual(decoded.buffers.get("bank[2]").bytes, thirdAsset);
  assert.equal(receivedAssetFileNames.get("bank[0]"), "first.ondabuffer");
  assert.equal(receivedAssetFileNames.get("bank[2]"), "third.ondabuffer");
});

test("project ZIP transport explicitly selects among multiple manifests", async () => {
  const encoder = new TextEncoder();
  const files = new Map([
    ["zeta.ondaproject", encoder.encode(JSON.stringify({ entry: "zeta.onda" }))],
    ["alpha.ondaproject", encoder.encode(JSON.stringify({ entry: "alpha.onda" }))],
    ["zeta.onda", encoder.encode("outs 1\nsample:\n  out1 = 2.0\n")],
    ["alpha.onda", encoder.encode("outs 1\nsample:\n  out1 = 1.0\n")],
  ]);
  const selectedMaterialization = {
    directories: ["assets", "code"],
    files: [
      { path: "project.ondaproject", bytes: encoder.encode(JSON.stringify({ entry: "code/main.onda" })) },
      { path: "code/main.onda", bytes: files.get("zeta.onda") },
    ],
  };
  let selectedPath;
  let offeredPaths;
  const projectApi = {
    async loadProjectFiles(_files, projectFilePath) {
      selectedPath = projectFilePath;
      return {
        bytes: new Uint8Array([1]),
        sourceGraph: {
          entry: "zeta.onda",
          documents: [{ path: "zeta.onda", contents: "outs 1\nsample:\n  out1 = 2.0\n" }],
        },
        buffers: [],
      };
    },
    async materializeProjectImage() {
      return selectedMaterialization;
    },
  };

  const decoded = await decodeProjectArchive(
    createZip(files),
    projectApi,
    async (paths) => {
      offeredPaths = paths;
      return "zeta.ondaproject";
    },
  );

  assert.deepEqual(offeredPaths, ["alpha.ondaproject", "zeta.ondaproject"]);
  assert.equal(selectedPath, "zeta.ondaproject");
  assert.equal(decoded.project.entry, "zeta.onda");
  await assert.rejects(
    decodeProjectArchive(createZip(files), projectApi),
    /requires a selection/,
  );
});

test("project ZIP transport accepts nested project manifests", async () => {
  const encoder = new TextEncoder();
  const files = new Map([
    ["collection/session/session.ondaproject", encoder.encode(JSON.stringify({ entry: "main.onda" }))],
    ["collection/session/main.onda", encoder.encode("outs 1\nsample:\n  out1 = 0.0\n")],
  ]);
  let selectedPath;
  const projectApi = {
    async loadProjectFiles(_files, projectFilePath) {
      selectedPath = projectFilePath;
      return {
        bytes: new Uint8Array([1]),
        sourceGraph: {
          entry: "collection/session/main.onda",
          documents: [{
            path: "collection/session/main.onda",
            contents: "outs 1\nsample:\n  out1 = 0.0\n",
          }],
        },
        buffers: [],
      };
    },
    async materializeProjectImage() {
      return {
        directories: ["assets", "code"],
        files: [
          {
            path: "project.ondaproject",
            bytes: encoder.encode(JSON.stringify({ entry: "code/main.onda" })),
          },
          { path: "code/main.onda", bytes: files.get("collection/session/main.onda") },
        ],
      };
    },
  };

  await decodeProjectArchive(createZip(files), projectApi);
  assert.equal(selectedPath, "collection/session/session.ondaproject");
});

test("project export retains source files outside the reachable graph", () => {
  const graph = sourceGraphWithWorkspaceDocuments({
    entry: "main.onda",
    stdlibDigest: `sha256:${"a".repeat(64)}`,
    documents: [{ path: "main.onda", contents: "import used\n" }],
    resolutions: [{
      source: "main.onda",
      kind: "import",
      specifier: "used",
      target: "used.onda",
    }],
  }, {
    "main.onda": "import used\n",
    "used.onda": "const used = 1\n",
    "scratch.onda": "work in progress\n",
  });

  assert.deepEqual(graph.documents, [
    { path: "main.onda", contents: "import used\n" },
    { path: "used.onda", contents: "const used = 1\n" },
    { path: "scratch.onda", contents: "work in progress\n" },
  ]);
  assert.equal(graph.resolutions.length, 1);
});

test("ZIP reader rejects escaping paths", async () => {
  assert.throws(
    () => createZip(new Map([["../escape.onda", new Uint8Array()]])),
    /not portable/,
  );
  assert.throws(
    () => createZip(new Map([
      ["src", new Uint8Array()],
      ["src/main.onda", new Uint8Array()],
    ])),
    /collides with another path/,
  );
  assert.throws(
    () => createZip(new Map([["e\u0301.onda", new Uint8Array()]])),
    /not portable/,
  );
  assert.doesNotThrow(() => createZip(new Map([
    ["a".repeat(255), new Uint8Array()],
  ])));
  assert.throws(
    () => createZip(new Map([["a".repeat(256), new Uint8Array()]])),
    /not portable/,
  );
  assert.throws(
    () => createZip(new Map([["é".repeat(128), new Uint8Array()]])),
    /not portable/,
  );
  for (const [left, right] of [["σ.onda", "ς.onda"], ["ẞ.onda", "ss.onda"]]) {
    assert.throws(
      () => createZip(new Map([
        [left, new Uint8Array()],
        [right, new Uint8Array()],
      ])),
      /collides with another path/,
    );
  }
});

test("ZIP store codec round-trips arbitrary files", async () => {
  const archive = createZip(new Map([
    ["project.ondaproject", new TextEncoder().encode("{}")],
    ["src/main.onda", new Uint8Array([0, 1, 2, 255])],
  ]));
  const files = await readZip(archive);
  assert.deepEqual([...files.get("src/main.onda")], [0, 1, 2, 255]);
});

test("ZIP reader accepts and omits explicit directory entries", async () => {
  const archive = createZip(new Map([
    ["src/", new Uint8Array()],
    ["src/main.onda", new Uint8Array([1, 2, 3])],
  ]));
  const files = await readZip(archive);
  assert.deepEqual([...files.keys()], ["src/main.onda"]);
});

test("ZIP transport accepts a manifest beyond the maximum source-document count", async () => {
  const files = new Map(Array.from(
    { length: 4096 },
    (_, index) => [`src/${index}.onda`, new Uint8Array()],
  ));
  files.set("project.ondaproject", new Uint8Array());

  const archive = createZip(files);
  const decoded = await readZip(archive);

  assert.equal(decoded.size, 4097);
});

test("ZIP reader streams deflated entries", async () => {
  const contents = new TextEncoder().encode("repeat ".repeat(1024));
  const files = await readZip(deflatedZip("main.onda", contents));
  assert.deepEqual(files.get("main.onda"), contents);
});

test("ZIP reader rejects oversized entries before expanding them", async () => {
  const archive = createZip(new Map([["main.onda", new Uint8Array([1])]]));
  const view = new DataView(archive.buffer, archive.byteOffset, archive.byteLength);
  for (let offset = 0; offset + 46 <= archive.byteLength; offset += 1) {
    if (view.getUint32(offset, true) === 0x02014b50) {
      view.setUint32(offset + 24, 256 * 1024 * 1024 + 1, true);
      break;
    }
  }
  await assert.rejects(readZip(archive), /expanded project archive is too large/);
});

function deflatedZip(path, contents) {
  const stored = createZip(new Map([[path, contents]]));
  const storedView = new DataView(stored.buffer, stored.byteOffset, stored.byteLength);
  const nameLength = storedView.getUint16(26, true);
  const localHeaderLength = 30 + nameLength;
  const storedCentralOffset = localHeaderLength + contents.byteLength;
  const centralLength = 46 + nameLength;
  const compressed = new Uint8Array(deflateRawSync(contents));
  const centralOffset = localHeaderLength + compressed.byteLength;
  const archive = new Uint8Array(centralOffset + centralLength + 22);
  archive.set(stored.subarray(0, localHeaderLength));
  archive.set(compressed, localHeaderLength);
  archive.set(
    stored.subarray(storedCentralOffset, storedCentralOffset + centralLength),
    centralOffset,
  );
  archive.set(stored.subarray(stored.byteLength - 22), centralOffset + centralLength);

  const view = new DataView(archive.buffer);
  view.setUint16(8, 8, true);
  view.setUint32(18, compressed.byteLength, true);
  view.setUint16(centralOffset + 10, 8, true);
  view.setUint32(centralOffset + 20, compressed.byteLength, true);
  view.setUint32(centralOffset + centralLength + 16, centralOffset, true);
  return archive;
}
