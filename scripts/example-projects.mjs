import { readFile, readdir } from "node:fs/promises";
import { extname, posix, relative, resolve } from "node:path";

const ONDA_EXTENSIONS = new Set([".on", ".onda"]);

export async function buildExampleProjectCatalog(examplesRoot) {
  const root = resolve(examplesRoot);
  const files = await filesBelow(root);
  const sources = Object.fromEntries(await Promise.all(
    files
      .filter((path) => ONDA_EXTENSIONS.has(extname(path)))
      .map(async (path) => [projectPath(root, path), await readFile(path, "utf8")]),
  ));
  const projects = {};
  for (const entry of Object.keys(sources).sort()) {
    const projectSources = collectProjectSources(entry, sources);
    projects[entry] = { entry, active: entry, sources: projectSources };
  }
  return { version: 1, projects };
}

function collectProjectSources(entry, allSources) {
  const pending = [entry];
  const selected = new Set();
  while (pending.length) {
    const path = pending.pop();
    if (selected.has(path)) continue;
    const source = allSources[path];
    if (source === undefined) {
      throw new Error(`example project dependency '${path}' is missing`);
    }
    selected.add(path);
    pending.push(...sourceDependencies(path, source, allSources));
  }
  return Object.fromEntries(
    [...selected].sort().map((path) => [path, allSources[path]]),
  );
}

function sourceDependencies(path, source, allSources) {
  const dependencies = [];
  const directory = posix.dirname(path);
  for (const match of source.matchAll(/^\s*include\s+["']([^"']+\.(?:on|onda))["']/gm)) {
    dependencies.push(resolveDependency(directory, match[1], allSources, path));
  }
  for (const match of source.matchAll(/^\s*import\s+([A-Za-z_][A-Za-z0-9_/-]*)\b/gm)) {
    const module = match[1];
    if (module.startsWith("std/")) continue;
    dependencies.push(resolveModule(directory, module, allSources, path));
  }
  return dependencies;
}

function resolveDependency(directory, dependency, allSources, owner) {
  const candidate = posix.normalize(posix.join(directory, dependency));
  if (candidate.startsWith("../") || !Object.hasOwn(allSources, candidate)) {
    throw new Error(`example '${owner}' refers to missing source '${dependency}'`);
  }
  return candidate;
}

function resolveModule(directory, module, allSources, owner) {
  const candidates = [
    posix.join(directory, `${module}.onda`),
    posix.join(directory, `${module}.on`),
    `${module}.onda`,
    `${module}.on`,
  ].map((candidate) => posix.normalize(candidate));
  const found = candidates.find((candidate) => Object.hasOwn(allSources, candidate));
  if (!found) throw new Error(`example '${owner}' imports missing module '${module}'`);
  return found;
}

async function filesBelow(directory) {
  const result = [];
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) result.push(...await filesBelow(path));
    else if (entry.isFile()) result.push(path);
  }
  return result;
}

function projectPath(root, path) {
  return relative(root, path).split("\\").join("/");
}
