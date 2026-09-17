#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "synapsegit-workspace-diagrams-"));
const fixture = path.join(temporary, "repository");

try {
  fs.cpSync(root, fixture, {
    recursive: true,
    filter(source) {
      return ![".git", "target", "node_modules"].includes(path.basename(source));
    },
  });

  function verify() {
    return spawnSync("node", ["scripts/verify_workspace_diagrams.mjs"], {
      cwd: fixture,
      encoding: "utf8",
    });
  }

  let result = verify();
  assert.equal(result.status, 0, result.stderr);

  const manifest = path.join(fixture, "crates", "synapse-local-service", "Cargo.toml");
  fs.writeFileSync(
    manifest,
    fs.readFileSync(manifest, "utf8") + "synapse-projection = { path = \"../synapse-projection\" }\n",
  );
  result = verify();
  assert.notEqual(result.status, 0, "an undocumented direct dependency must fail");
  assert.match(result.stderr, /synapse-local-service -> synapse-projection/);

  const architecture = path.join(fixture, "docs", "localhost_application_architecture.md");
  fs.writeFileSync(
    architecture,
    fs.readFileSync(architecture, "utf8").replace(
      "    Service --> Publication[\"synapse-publication<br/>public presentation sidecar\"]",
      "    Service --> App[\"synapse-application<br/>AI + Human routes\"]",
    ),
  );
  result = verify();
  assert.notEqual(result.status, 0, "a false solid dependency edge must fail");
  assert.match(result.stderr, /synapse-local-service -> synapse-application/);

  console.log("workspace_diagram_test_ok");
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
