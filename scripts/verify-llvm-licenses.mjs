import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const llvmPrefix = process.argv[2];
if (!llvmPrefix) {
  throw new Error("usage: node scripts/verify-llvm-licenses.mjs <llvm-prefix>");
}

for (const [expectedName, installedName] of [
  ["LLVM-LICENSE.txt", "LICENSE.TXT"],
  ["LLVM-BLAKE3-LICENSE.txt", "BLAKE3-LICENSE.txt"],
  ["LLVM-XXHASH-LICENSE.txt", "XXHASH-LICENSE.txt"],
  ["LLVM-MD5-LICENSE.txt", "MD5-LICENSE.txt"],
  ["LLVM-REGEX-LICENSE.txt", "REGEX-LICENSE.txt"],
  ["LLVM-UNICODE-LICENSE.txt", "UNICODE-LICENSE.txt"],
  ["LLVM-MSVCSETUPAPI-LICENSE.txt", "MSVCSETUPAPI-LICENSE.txt"],
]) {
  const [expected, installed] = await Promise.all([
    readFile(resolve(repoRoot, "licenses", expectedName), "utf8"),
    readFile(resolve(llvmPrefix, "share/licenses/llvm", installedName), "utf8"),
  ]);
  if (normalize(expected) !== normalize(installed)) {
    throw new Error(`installed LLVM license differs from licenses/${expectedName}`);
  }
}

function normalize(text) {
  return text.replaceAll("\r\n", "\n");
}
