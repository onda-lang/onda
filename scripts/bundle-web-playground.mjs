import { mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { build } from "esbuild";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export async function bundlePlayground(outfile) {
  const outputPath = resolve(outfile);
  await mkdir(dirname(outputPath), { recursive: true });
  await build({
    entryPoints: [resolve(repoRoot, "ui/playground/live.js")],
    outfile: outputPath,
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "es2022",
    minify: true,
    legalComments: "none",
    loader: { ".onda": "text" },
    external: [
      "@onda-lang/processor-abi",
      "@onda-lang/wasm-compiler",
      "@onda-lang/webaudio",
      "#onda-frontend-loader",
    ],
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const outfile = process.argv[2]
    ?? resolve(repoRoot, "examples/web/onda_wasm_playground/playground.js");
  await bundlePlayground(outfile);
  process.stdout.write(`Bundled browser playground: ${outfile}\n`);
}
