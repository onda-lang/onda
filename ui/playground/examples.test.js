import assert from "node:assert/strict";
import test from "node:test";

import { loadExampleProject } from "./examples.js";

const project = {
  entry: "basic/sine.onda",
  active: "basic/sine.onda",
  sources: { "basic/sine.onda": "sample:\n  out1 = 0.0\n" },
};

test("loads an exact project from the versioned example catalog", async () => {
  let requestedUrl = null;
  const loaded = await loadExampleProject("/examples.json", project.entry, async (url) => {
    requestedUrl = url;
    return {
      ok: true,
      json: async () => ({ version: 1, projects: { [project.entry]: project } }),
    };
  });
  assert.equal(requestedUrl, "/examples.json");
  assert.equal(loaded, project);
});

test("rejects invalid and unknown example paths", async () => {
  const fetchCatalog = async () => ({
    ok: true,
    json: async () => ({ version: 1, projects: {} }),
  });
  await assert.rejects(
    loadExampleProject("/examples.json", "../private.onda", fetchCatalog),
    /path is invalid/,
  );
  await assert.rejects(
    loadExampleProject("/examples.json", "basic/missing.onda", fetchCatalog),
    /was not found/,
  );
});
