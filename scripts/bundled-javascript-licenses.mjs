import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const licenseName = /^(?:licen[cs]e|copying|notice)(?:\.|$)/i;

export async function writeBundledJavaScriptLicenses(metafiles, output) {
  const packageRoots = new Set();
  for (const metafile of metafiles) {
    for (const input of Object.keys(metafile.inputs)) {
      const packageRoot = bundledPackageRoot(resolve(repoRoot, input));
      if (packageRoot) packageRoots.add(packageRoot);
    }
  }

  const packages = await Promise.all([...packageRoots].map(readPackageLicenses));
  packages.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

  const sections = packages.map(({ name, version, repository, licenses }) => [
    "-------------------------------------------------------------------------------",
    `${name} ${version}${repository ? ` (${repository})` : ""}`,
    "",
    ...licenses.flatMap(({ name: filename, text }) => [filename, "", text.trim(), ""]),
  ].join("\n"));

  const outputPath = resolve(output);
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, [
    "Bundled JavaScript licenses",
    "===========================",
    "",
    "This file is generated from esbuild's bundled-input metadata.",
    "",
    ...sections,
    "",
  ].join("\n"));
}

function bundledPackageRoot(path) {
  const marker = `${sep}node_modules${sep}`;
  const markerIndex = path.lastIndexOf(marker);
  if (markerIndex < 0) return undefined;

  const nodeModules = path.slice(0, markerIndex + marker.length);
  const parts = path.slice(markerIndex + marker.length).split(sep);
  const packageParts = parts[0]?.startsWith("@") ? parts.slice(0, 2) : parts.slice(0, 1);
  return packageParts.length === 0 ? undefined : resolve(nodeModules, ...packageParts);
}

async function readPackageLicenses(packageRoot) {
  const manifest = JSON.parse(await readFile(resolve(packageRoot, "package.json"), "utf8"));
  const filenames = (await readdir(packageRoot)).filter((name) => licenseName.test(name)).sort();
  if (filenames.length === 0) {
    throw new Error(`bundled package ${manifest.name} ${manifest.version} has no license file`);
  }

  return {
    name: manifest.name,
    version: manifest.version,
    repository: repositoryUrl(manifest.repository),
    licenses: await Promise.all(filenames.map(async (name) => ({
      name,
      text: await readFile(resolve(packageRoot, name), "utf8"),
    }))),
  };
}

function repositoryUrl(repository) {
  if (typeof repository === "string") return repository;
  return repository?.url;
}
