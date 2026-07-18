import assert from "node:assert/strict";
import { resolve } from "node:path";
import test from "node:test";

import { buildExampleProjectCatalog } from "../../scripts/example-projects.mjs";

test("builds focused browser projects with local example dependencies", async () => {
  const catalog = await buildExampleProjectCatalog(resolve("examples"));
  const sine = catalog.projects["foundations/sine.onda"];
  const reverb = catalog.projects["processors-and-graphs/reverb_graph.onda"];

  assert.equal(catalog.version, 1);
  assert.deepEqual(Object.keys(sine.sources), ["foundations/sine.onda"]);
  assert.deepEqual(Object.keys(reverb.sources), [
    "processors-and-graphs/reverb.onda",
    "processors-and-graphs/reverb_graph.onda",
  ]);
  assert.equal(reverb.entry, "processors-and-graphs/reverb_graph.onda");
  assert.equal(reverb.active, reverb.entry);
});
