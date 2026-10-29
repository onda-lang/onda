import { cp, copyFile, mkdir, readFile, readdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const source = fileURLToPath(new URL("../ui/gpu/", import.meta.url));

export async function stageGpuUi(destination) {
  try {
    await readFile(resolve(source, "pkg/onda_ui_bg.wasm"));
  } catch {
    throw new Error("GPU assets are missing; run npm run build:gpu-ui first");
  }
  await mkdir(resolve(destination, "pkg"), { recursive: true });
  for (const file of [
    "index.html",
    "host.js",
    "browser.js",
    "protocol.js",
    "text-input.js",
    "pkg/onda_ui.js",
    "pkg/onda_ui_bg.wasm",
  ]) {
    await copyFile(resolve(source, file), resolve(destination, file));
  }
  await cp(resolve(source, "pkg/fonts"), resolve(destination, "pkg/fonts"), { recursive:true });
  for (const file of await readdir(resolve(source,"pkg")))
    if (file.startsWith("OFL-") && file.endsWith(".txt"))
      await copyFile(resolve(source,"pkg",file),resolve(destination,"pkg",file));
}
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  if (!process.argv[2])
    throw new Error("Usage: node scripts/stage-gpu-ui.mjs <destination>");
  await stageGpuUi(resolve(process.argv[2]));
}
