import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

await verifyHostedC();
await verifyProcessorC();
await verifyWebPackages();

async function verifyHostedC() {
  const header = await read("include/onda.h");
  const declarations = [...withoutComments(header).matchAll(
    /(?:^|\n)(?![ \t]*typedef\b)[^;{}#]*?\b(onda_[A-Za-z0-9_]+)\s*\([^;{}]*\)\s*;/g,
  )].map((match) => match[1]);
  const documented = await documentedIndex(
    "docs/api.md",
    "<!-- BEGIN C API FUNCTION INDEX -->",
    "<!-- END C API FUNCTION INDEX -->",
    /^onda_[A-Za-z0-9_]+$/,
  );
  verifySameSet("docs/api.md", "include/onda.h", declarations, documented);
  process.stdout.write(`Verified ${declarations.length} documented libonda C functions\n`);
}

async function verifyProcessorC() {
  const header = withoutComments(await read("include/onda_processor_abi.h"));
  const declarations = [
    ...header.matchAll(
      /^ONDA_PROCESSOR_STATIC_INLINE\s+[^\n(]+\s+(onda_[A-Za-z0-9_]+)\s*\(/gm,
    ),
    ...header.matchAll(/^uint32_t\s+(onda_[A-Za-z0-9_]+)\s*\(/gm),
  ].map((match) => match[1]);
  const documented = await documentedIndex(
    "docs/processor-abi.md",
    "<!-- BEGIN PROCESSOR C API FUNCTION INDEX -->",
    "<!-- END PROCESSOR C API FUNCTION INDEX -->",
    /^onda_[A-Za-z0-9_]+$/,
  );
  verifySameSet(
    "docs/processor-abi.md",
    "include/onda_processor_abi.h",
    declarations,
    documented,
  );
  process.stdout.write(`Verified ${declarations.length} documented processor C functions\n`);
}

async function verifyWebPackages() {
  const packages = [
    "onda_processor_abi",
    "onda_binaryen_web",
    "onda_wasm_compiler",
    "onda_webaudio",
  ];
  let total = 0;
  for (const packageDirectory of packages) {
    const declarationPath = `packages/${packageDirectory}/src/index.d.ts`;
    const implementationPath = `packages/${packageDirectory}/src/index.js`;
    const declarationsSource = await read(declarationPath);
    const declarations = exportedTypeScriptNames(declarationsSource);
    const documented = await documentedIndex(
      "docs/web-api.md",
      `<!-- BEGIN WEB API ${packageDirectory} -->`,
      `<!-- END WEB API ${packageDirectory} -->`,
      /^[A-Za-z_$][A-Za-z0-9_$]*$/,
    );
    verifySameSet(
      "docs/web-api.md",
      declarationPath,
      declarations,
      documented,
    );
    verifySameSet(
      declarationPath,
      implementationPath,
      exportedJavaScriptNames(await read(implementationPath)),
      exportedTypeScriptValueNames(declarationsSource),
    );
    total += declarations.length;
  }
  process.stdout.write(`Verified ${total} documented web package exports\n`);
}

function exportedJavaScriptNames(source) {
  source = withoutComments(source);
  const names = [...source.matchAll(
    /\bexport\s+(?:async\s+)?(?:const|class|function)\s+([A-Za-z_$][A-Za-z0-9_$]*)/g,
  )].map((match) => match[1]);
  for (const match of source.matchAll(/\bexport\s+const\s+\{([^}]*)\}\s*=/g)) {
    names.push(...exportBlockNames(match[1]));
  }
  for (const match of source.matchAll(
    /\bexport\s+\{([^}]*)\}(?:\s+from\s+[^;]+)?\s*;/g,
  )) {
    names.push(...exportBlockNames(match[1]));
  }
  return [...new Set(names)].sort();
}

function exportedTypeScriptValueNames(source) {
  source = withoutComments(source);
  const names = [...source.matchAll(
    /\bexport\s+(?:declare\s+)?(?:const|class|function)\s+([A-Za-z_$][A-Za-z0-9_$]*)/g,
  )].map((match) => match[1]);
  for (const match of source.matchAll(
    /\bexport\s+(?!type\b)\{([^}]*)\}(?:\s+from\s+[^;]+)?\s*;/g,
  )) {
    names.push(...exportBlockNames(match[1]));
  }
  return [...new Set(names)].sort();
}

function exportedTypeScriptNames(source) {
  source = withoutComments(source);
  const names = [...source.matchAll(
    /\bexport\s+(?:declare\s+)?(?:const|class|function|interface|type)\s+([A-Za-z_$][A-Za-z0-9_$]*)/g,
  )].map((match) => match[1]);
  for (const match of source.matchAll(
    /\bexport\s+(?:type\s+)?\{([^}]*)\}(?:\s+from\s+[^;]+)?\s*;/g,
  )) {
    names.push(...exportBlockNames(match[1]));
  }
  return [...new Set(names)].sort();
}

function exportBlockNames(block) {
  return block.split(",").map((entry) => entry.trim().split(/\s+as\s+/).at(-1)).filter(
    (name) => /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name),
  );
}

async function documentedIndex(path, beginMarker, endMarker, identifierPattern) {
  const docs = await read(path);
  const begin = docs.indexOf(beginMarker);
  const end = docs.indexOf(endMarker);
  if (begin < 0 || end < begin) {
    throw new Error(`${path} is missing the index markers ${beginMarker} / ${endMarker}`);
  }
  return docs
    .slice(begin + beginMarker.length, end)
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => identifierPattern.test(line));
}

function verifySameSet(docsPath, declarationPath, declarations, documented) {
  const declarationSet = new Set(declarations);
  const documentedSet = new Set(documented);
  const missing = [...declarationSet].filter((name) => !documentedSet.has(name));
  const stale = [...documentedSet].filter((name) => !declarationSet.has(name));
  const duplicateDeclarations = duplicates(declarations);
  const duplicateDocs = duplicates(documented);
  if (
    missing.length > 0
    || stale.length > 0
    || duplicateDeclarations.length > 0
    || duplicateDocs.length > 0
  ) {
    throw new Error([
      `${docsPath} does not match ${declarationPath}`,
      formatList("missing", missing),
      formatList("stale", stale),
      formatList("duplicate declarations", duplicateDeclarations),
      formatList("duplicate documentation entries", duplicateDocs),
    ].filter(Boolean).join("\n"));
  }
}

function withoutComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

async function read(path) {
  return readFile(resolve(repoRoot, path), "utf8");
}

function duplicates(values) {
  const seen = new Set();
  const repeated = new Set();
  for (const value of values) {
    if (seen.has(value)) repeated.add(value);
    seen.add(value);
  }
  return [...repeated].sort();
}

function formatList(label, values) {
  return values.length === 0 ? "" : `${label}: ${values.join(", ")}`;
}
