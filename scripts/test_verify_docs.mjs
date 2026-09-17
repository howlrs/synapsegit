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
  fs.writeFileSync(
    reference,
    fs.readFileSync(reference, "utf8").replace(
      "blob:sg-oid-v1:sha256:SHA256_HEX_HERE",
      "blob:sg-oid-v1:sha256:<64-lowercase-hex>",
    ),
  );
  result = verify();
  assert.notEqual(result.status, 0, "invalid bash syntax must fail");
  assert.match(result.stderr, /docs\/cli_reference\.md:175 invalid bash block/);

  console.log("docs_test_ok");
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
