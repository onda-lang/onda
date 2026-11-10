import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gpuUiFiles, gpuUiVersion } from "./gpu-ui-assets.mjs";

const source = fileURLToPath(new URL("../ui/gpu/", import.meta.url));

export async function stageGpuUi(destination, sourceDirectory = source) {
  const assets = new Map();
  try {
    const manifest = await readFile(resolve(sourceDirectory, "pkg/build.json"));
    const { version, files } = JSON.parse(manifest);
    const expectedVersion = gpuUiVersion();
    if (version !== expectedVersion) {
      throw new Error(`GPU UI is Onda ${version}; expected ${expectedVersion}`);
    }
    if (!files || typeof files !== "object" || Array.isArray(files)) {
      throw new Error("Invalid GPU UI build manifest files");
    }
    const required = new Set(gpuUiFiles());
    for (const name of required) {
      if (!Object.hasOwn(files, name)) {
        throw new Error(`GPU UI build manifest is missing ${name}`);
      }
    }
    for (const [name, hash] of Object.entries(files)) {
      if (!required.has(name) || typeof hash !== "string" || !/^[a-f0-9]{64}$/.test(hash)) {
        throw new Error(`Invalid GPU UI build manifest entry: ${name}`);
      }
      const bytes = await readFile(resolve(sourceDirectory, name));
      if (!bytes.length || createHash("sha256").update(bytes).digest("hex") !== hash) {
        throw new Error(`GPU UI asset changed since build: ${name}`);
      }
      assets.set(name, bytes);
    }
    assets.set("pkg/build.json", manifest);
  } catch (error) {
    throw new Error(`${error.message}; run npm run build:gpu-ui first`, { cause: error });
  }
  // Validate everything before touching the destination, then stage the exact
  // bytes checked above even if a build starts changing the source meanwhile.
  for (const [name, bytes] of assets) {
    const path = resolve(destination, name);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, bytes);
  }
}
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  if (!process.argv[2])
    throw new Error("Usage: node scripts/stage-gpu-ui.mjs <destination>");
  await stageGpuUi(resolve(process.argv[2]));
}
