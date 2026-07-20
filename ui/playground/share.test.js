import assert from "node:assert/strict";
import test from "node:test";

import {
  decodeSharedSession,
  encodeSharedSession,
  sharedSessionHash,
} from "./share.js";

test("round-trips a versioned multi-file playground session through a URL fragment", async () => {
  const session = {
    entry: "main.onda",
    active: "voices/lead.onda",
    sources: {
      "main.onda": "include \"./voices/lead.onda\"\n",
      "voices/lead.onda": "# Unicode remains intact: λ ♪\nsample:\n  out1 = 0.0\n",
    },
    sampleRate: 48_000,
    blockSize: 256,
  };
  const encoded = await encodeSharedSession(session);
  const decoded = await decodeSharedSession(sharedSessionHash(encoded));

  assert.deepEqual(decoded, { ...session, v: 1 });
  assert.match(encoded, /^[zj][A-Za-z0-9_-]+$/);
  assert.doesNotMatch(sharedSessionHash(encoded), /gzip|json|share=/);
});

test("rejects malformed and unsupported shared sessions", async () => {
  await assert.rejects(
    decodeSharedSession("#p=xnot_base64!"),
    /malformed/,
  );
  const unsupported = btoa(JSON.stringify({ v: 99 }))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
  await assert.rejects(
    decodeSharedSession(`#p=j${unsupported}`),
    /unsupported shared playground project version/,
  );
});
