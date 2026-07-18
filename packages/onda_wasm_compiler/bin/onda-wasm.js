#!/usr/bin/env node

import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, extname, isAbsolute, parse, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import {
  OndaCompileError,
  createCompiler,
  createProcessorArtifactFiles,
} from "../src/index.js";

const HELP = `Usage:
  onda-wasm compile <input.onda> [options]

Options:
  --root <directory>       Project root used to collect .onda sources
  --output, -o <file>      Output Wasm path (default: <input>.wasm)
  --meta-out <file>        Output descriptor path (default: <output>.onda.json)
  --sample-rate <number>   Compile-time sample rate (default: 48000)
  --block-size <integer>   Compile-time block size (default: 128)
  --optimize-level <0..4>  Binaryen optimization level (default: 4)
  --shrink-level <0..2>    Binaryen shrink level (default: 0)
  --fast-math              Enable relaxed floating-point rewrites
  --no-simd                Disable WebAssembly SIMD code generation
  --wat-out <file>         Also write WebAssembly text format
  --help, -h               Show this help
  --version, -V            Show the Onda compiler version
`;

export async function main(argv = process.argv.slice(2)) {
  if (argv.length === 0 || argv.includes("--help") || argv.includes("-h")) {
    process.stdout.write(HELP);
    return 0;
  }
  if (argv.includes("--version") || argv.includes("-V")) {
    const manifest = JSON.parse(
      await readFile(new URL("../package.json", import.meta.url), "utf8"),
    );
    process.stdout.write(`${manifest.version}\n`);
    return 0;
  }
  if (argv[0] !== "compile") {
    throw new Error(`unknown command '${argv[0]}'\n\n${HELP}`);
  }

  const parsed = parseCompileArguments(argv.slice(1));
  const input = resolve(parsed.input);
  const root = resolve(parsed.root ?? dirname(input));
  const entryRelative = relative(root, input);
  if (entryRelative === "" || entryRelative === ".") {
    throw new Error("input must identify an .onda file below the project root");
  }
  if (entryRelative === ".." || entryRelative.startsWith(`..${sep}`) || isAbsolute(entryRelative)) {
    throw new Error(`input '${input}' is outside project root '${root}'`);
  }
  if (extname(input) !== ".onda") {
    throw new Error("input must have an .onda extension");
  }

  const sources = await collectSources(root);
  const entry = portablePath(entryRelative);
  if (!(entry in sources)) {
    throw new Error(`input '${entry}' was not found while collecting project sources`);
  }

  const output = normalizeWasmOutput(parsed.output ?? resolve(dirname(input), parse(input).name));
  const metadataOutput = resolve(
    parsed.metaOut ?? output.slice(0, -".wasm".length) + ".onda.json",
  );
  const watOutput = parsed.watOut === undefined ? undefined : resolve(parsed.watOut);
  const compiler = await createCompiler();
  try {
    const artifact = await compiler.compileProject({ entry, sources }, {
      sampleRate: parsed.sampleRate,
      blockSize: parsed.blockSize,
      codegen: {
        optimizeLevel: parsed.optimizeLevel,
        shrinkLevel: parsed.shrinkLevel,
        fastMath: parsed.fastMath,
        simd: parsed.simd,
        emitText: watOutput !== undefined,
      },
    });
    const files = await createProcessorArtifactFiles(artifact, {
      baseName: parse(output).name,
    });
    await mkdir(dirname(output), { recursive: true });
    await mkdir(dirname(metadataOutput), { recursive: true });
    if (watOutput !== undefined) await mkdir(dirname(watOutput), { recursive: true });
    const writes = [
      writeFile(output, files.wasm.bytes),
      writeFile(metadataOutput, files.metadata.text),
    ];
    if (watOutput !== undefined) {
      if (typeof artifact.wat !== "string") {
        throw new Error("Binaryen did not return the requested WebAssembly text");
      }
      writes.push(writeFile(watOutput, artifact.wat));
    }
    await Promise.all(writes);
    process.stdout.write(`Wrote WebAssembly: ${output}\n`);
    process.stdout.write(`Wrote descriptor: ${metadataOutput}\n`);
    if (watOutput !== undefined) process.stdout.write(`Wrote WebAssembly text: ${watOutput}\n`);
    return 0;
  } finally {
    await compiler.dispose();
  }
}

function parseCompileArguments(args) {
  let input;
  const result = {
    root: undefined,
    output: undefined,
    metaOut: undefined,
    sampleRate: 48_000,
    blockSize: 128,
    optimizeLevel: 4,
    shrinkLevel: 0,
    fastMath: false,
    simd: true,
    watOut: undefined,
  };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (!argument.startsWith("-")) {
      if (input !== undefined) throw new Error(`unexpected argument '${argument}'`);
      input = argument;
      continue;
    }
    switch (argument) {
      case "--root":
        result.root = requiredValue(args, ++index, argument);
        break;
      case "--output":
      case "-o":
        result.output = requiredValue(args, ++index, argument);
        break;
      case "--meta-out":
        result.metaOut = requiredValue(args, ++index, argument);
        break;
      case "--sample-rate":
        result.sampleRate = numberValue(args, ++index, argument);
        break;
      case "--block-size":
        result.blockSize = integerValue(args, ++index, argument, 1, 0xffff_ffff);
        break;
      case "--optimize-level":
        result.optimizeLevel = integerValue(args, ++index, argument, 0, 4);
        break;
      case "--shrink-level":
        result.shrinkLevel = integerValue(args, ++index, argument, 0, 2);
        break;
      case "--fast-math":
        result.fastMath = true;
        break;
      case "--no-simd":
        result.simd = false;
        break;
      case "--wat-out":
        result.watOut = requiredValue(args, ++index, argument);
        break;
      default:
        throw new Error(`unknown option '${argument}'`);
    }
  }
  if (input === undefined) throw new Error("compile requires an input .onda file");
  return { input, ...result };
}

async function collectSources(root) {
  const sources = Object.create(null);
  await visit(root);
  return sources;

  async function visit(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      if (entry.isDirectory()) {
        if ([".git", "node_modules", "target"].includes(entry.name)) continue;
        await visit(resolve(directory, entry.name));
      } else if (entry.isFile() && extname(entry.name) === ".onda") {
        const path = resolve(directory, entry.name);
        sources[portablePath(relative(root, path))] = await readFile(path, "utf8");
      }
    }
  }
}

function normalizeWasmOutput(output) {
  const resolved = resolve(output);
  return extname(resolved) === ".wasm" ? resolved : `${resolved}.wasm`;
}

function portablePath(path) {
  return path.split(sep).join("/");
}

function requiredValue(args, index, option) {
  const value = args[index];
  if (value === undefined || value.startsWith("-")) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

function numberValue(args, index, option) {
  const value = Number(requiredValue(args, index, option));
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${option} requires a finite number greater than zero`);
  }
  return value;
}

function integerValue(args, index, option, minimum, maximum) {
  const value = Number(requiredValue(args, index, option));
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new Error(`${option} requires an integer from ${minimum} to ${maximum}`);
  }
  return value;
}

function printError(error) {
  if (error instanceof OndaCompileError) {
    for (const diagnostic of error.diagnostics) {
      const location = diagnostic.file
        ? `${diagnostic.file}:${diagnostic.line}:${diagnostic.column}: `
        : "";
      process.stderr.write(`${location}[${diagnostic.stage}] ${diagnostic.message}\n`);
    }
    return;
  }
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().then(
    (status) => {
      process.exitCode = status;
    },
    (error) => {
      printError(error);
      process.exitCode = 1;
    },
  );
}
