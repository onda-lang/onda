import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { createCompiler } from "../packages/onda_wasm_compiler/src/index.js";
import {
  buildExampleProjectCatalog,
  filesBelow,
  relativeProjectPath,
} from "./example-projects.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const examplesRoot = resolve(repoRoot, "examples");
const compileOptions = { sampleRate: 48_000, blockSize: 512 };
const catalog = await buildExampleProjectCatalog(examplesRoot);
const catalogProjects = Object.entries(catalog.projects).sort(([left], [right]) =>
  left.localeCompare(right, "en"),
);
const projectManifests = (await filesBelow(examplesRoot))
  .filter((path) => path.endsWith(".ondaproject"));

if (catalogProjects.length === 0) {
  throw new Error("the checked-in example catalog is empty");
}

const compiler = await createCompiler();
const failures = [];
let compiledCatalogProjects = 0;
let compiledMaterializedProjects = 0;

try {
  for (const [name, project] of catalogProjects) {
    try {
      const compiled = await compiler.compileWorkspace(project, compileOptions);
      assertValidArtifact(compiled.artifact, name);
      assertExactSources(compiled.sourceFiles, project.sources, name);
      compiledCatalogProjects += 1;
    } catch (error) {
      failures.push({ name, error });
    }
  }

  for (const manifest of projectManifests) {
    const name = repoPath(manifest);
    try {
      const projectRoot = dirname(manifest);
      const files = new Map(await Promise.all(
        (await filesBelow(projectRoot)).map(async (path) => [
          relativeProjectPath(projectRoot, path),
          await readFile(path),
        ]),
      ));
      const image = await compiler.loadProjectFiles(
        files,
        relativeProjectPath(projectRoot, manifest),
      );
      const compiled = await compiler.compileProjectImage(image.bytes, compileOptions);
      assertValidArtifact(compiled.artifact, name);
      compiledMaterializedProjects += 1;
    } catch (error) {
      failures.push({ name, error });
    }
  }
} finally {
  await compiler.dispose();
}

if (failures.length > 0) {
  for (const failure of failures) {
    process.stderr.write(`\n--- ${failure.name} ---\n${errorMessage(failure.error)}\n`);
  }
  throw new Error(
    `${failures.length} checked-in example project(s) failed browser compilation`,
  );
}

process.stdout.write(
  `Verified ${compiledCatalogProjects} browser catalog projects and `
    + `${compiledMaterializedProjects} materialized .ondaproject examples\n`,
);

function assertValidArtifact(artifact, name) {
  if (!WebAssembly.validate(artifact.wasm)) {
    throw new Error(`example '${name}' produced invalid WebAssembly`);
  }
}

function assertExactSources(sourceFiles, sources, name) {
  const actual = [...sourceFiles].sort();
  const expected = Object.keys(sources).sort();
  if (
    actual.length !== expected.length
    || actual.some((path, index) => path !== expected[index])
  ) {
    throw new Error(
      `example '${name}' catalog sources do not match compiler dependencies\n`
        + `catalog: ${JSON.stringify(expected)}\n`
        + `compiler: ${JSON.stringify(actual)}`,
    );
  }
}

function repoPath(path) {
  return relativeProjectPath(repoRoot, path);
}

function errorMessage(error) {
  return error instanceof Error ? error.stack ?? error.message : String(error);
}
