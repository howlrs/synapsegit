#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "synapsegit-docs-"));
const fixture = path.join(temporary, "repository");

try {
  fs.cpSync(root, fixture, {
    recursive: true,
    filter(source) {
      return ![".git", "target", "node_modules"].includes(path.basename(source));
    },
  });

  function verify() {
    return spawnSync("node", ["scripts/verify_docs.mjs"], {
      cwd: fixture,
      encoding: "utf8",
    });
  }

  let result = verify();
  assert.equal(result.status, 0, result.stderr);

  const reference = path.join(fixture, "docs", "cli_reference.md");
  const original = fs.readFileSync(reference, "utf8");
  const marker = "blob:sg-oid-v1:sha256:SHA256_HEX_HERE";
  const markerOffset = original.indexOf(marker);
  assert.notEqual(markerOffset, -1, "the CLI example must contain the fixture marker");
  const fenceLine = original.slice(0, markerOffset).split("\n")
    .findLastIndex(line => /^```bash\s*$/u.test(line)) + 1;
  assert.ok(fenceLine > 0, "the fixture marker must belong to a bash example");
  fs.writeFileSync(
    reference,
    original.replace(
      marker,
      "blob:sg-oid-v1:sha256:<64-lowercase-hex>",
    ),
  );
  result = verify();
  assert.notEqual(result.status, 0, "invalid bash syntax must fail");
  assert.ok(result.stderr.includes(`docs/cli_reference.md:${fenceLine} invalid bash block`), result.stderr);

  console.log("docs_test_ok");
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
