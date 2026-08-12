import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { buildExampleProjectCatalog } from "../../scripts/example-projects.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const catalog = await buildExampleProjectCatalog(resolve(repoRoot, "examples"));

test("builds focused browser projects with exact local dependencies", () => {
  assert.equal(catalog.version, 1);
  assert.deepEqual(
    Object.keys(catalog.projects["basic/sine.onda"].sources),
    ["basic/sine.onda"],
  );
  assert.deepEqual(
    Object.keys(catalog.projects["effects/fdn_reverb.onda"].sources),
    [
      "effects/fdn_reverb.onda",
      "effects/processors/effect_audition.onda",
      "effects/processors/fdn_reverb.onda",
    ],
  );
  assert.deepEqual(
    Object.keys(catalog.projects["projects/wavetable_garden/code/main.onda"].sources),
    [
      "projects/wavetable_garden/code/main.onda",
      "projects/wavetable_garden/code/wavetable_voice.onda",
    ],
  );
});

test("every documented playground example exists in the generated catalog", async () => {
  const documents = await Promise.all([
    readFile(resolve(repoRoot, "docs/examples.md"), "utf8"),
    readFile(resolve(repoRoot, "website/index.md"), "utf8"),
  ]);
  const linked = new Set(
    documents.flatMap((document) =>
      [...document.matchAll(/example=([^' }]+)/g)].map((match) => match[1])
    ),
  );
  assert.notEqual(linked.size, 0);
  for (const example of linked) {
    assert.equal(
      Object.hasOwn(catalog.projects, example),
      true,
      `documented playground example '${example}' is missing from the catalog`,
    );
  }
});
