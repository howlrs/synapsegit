#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { assertRevisionHistory } from "./local_api_revision_history.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const contract = JSON.parse(fs.readFileSync(path.join(root, "api/local/v1/openapi.json")));
const registry = JSON.parse(fs.readFileSync(path.join(root, "api/local/v1/revisions.json")));
const temporary = fs.mkdtempSync(path.join(tmpdir(), "synapse-api-version-"));
const api = path.join(temporary, "api/local/v1");
fs.mkdirSync(api, { recursive: true });
let checks = 0;
function verify(change = () => {}, changeRegistry = () => {}, expectedError) {
  const document = structuredClone(contract);
  const revisions = structuredClone(registry);
  change(document);
  changeRegistry(revisions, document);
  fs.writeFileSync(path.join(api, "openapi.json"), JSON.stringify(document, null, 4));
  fs.writeFileSync(path.join(api, "revisions.json"), JSON.stringify(revisions));
  const result = spawnSync(process.execPath, [path.join(root, "scripts/verify_local_api.mjs")], {
    cwd: temporary, encoding: "utf8", env: { ...process.env, LOCAL_API_BASE_REF: "" },
  });
  assert.ifError(result.error);
  if (expectedError) {
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stderr, expectedError);
  } else {
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /^local_api_ok: operations=\d+ schemas=\d+ refs=\d+\n$/);
  }
  checks += 1;
}
const drift = /local_api_error: contract revision drift/;
try {
  verify();
  verify((document) => { document.paths["/revision-probe"] = {}; }, undefined, drift);
  verify((document) => { document.components.schemas.RevisionProbe = { type: "string" }; }, undefined, drift);
  verify((document) => { document.paths["/health"].get.responses["200"].headers = { "X-Revision-Probe": { schema: { type: "string" } } }; }, undefined, drift);
  verify((document) => { document.components.schemas.RevisionProbe = { type: "object", properties: { value: { type: "string" } }, required: ["value"] }; }, undefined, drift);
  verify((document) => { document.info.version = "0.4.0-draft"; }, undefined, drift);
  verify((document) => { document.info.description += " Revision probe."; }, undefined, drift);
  verify((document) => {
    document.paths = Object.fromEntries(Object.entries(document.paths).reverse());
    document.info = Object.fromEntries(Object.entries(document.info).reverse());
  });
  verify(undefined, (revisions) => { revisions.revisions.push(revisions.revisions.at(-1)); }, /revision versions must strictly increase/);
  verify(undefined, (revisions) => { revisions.revisions[0].sha256 = "invalid"; }, /numeric X.Y.Z-draft version and SHA-256/);
  verify((document) => {
    const [major, minor, patch] = document.info.version.replace(/-draft$/, "").split(".").map(Number);
    document.info.version = `${major}.${minor}.${patch + 1}-draft`;
    document.components.schemas.RevisionProbe = { type: "string" };
  }, (revisions, document) => {
    const content = structuredClone(document);
    delete content.info.version;
    const sort = (value) => Array.isArray(value) ? value.map(sort) : value !== null && typeof value === "object"
      ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, sort(value[key])])) : value;
    revisions.revisions.push({ version: document.info.version, sha256: createHash("sha256").update(JSON.stringify(sort(content))).digest("hex") });
    assertRevisionHistory(document, revisions, contract, registry);
  });
  const mutatedContract = structuredClone(contract);
  mutatedContract.components.schemas.RevisionProbe = { type: "string" };
  const replacedRegistry = structuredClone(registry);
  replacedRegistry.revisions.at(-1).sha256 = "a".repeat(64);
  assert.throws(() => assertRevisionHistory(mutatedContract, replacedRegistry, contract, registry), /published contract revisions are immutable/);
  assert.throws(() => assertRevisionHistory(mutatedContract, registry, contract, registry), /without a new info.version/);
  assert.throws(() => assertRevisionHistory(mutatedContract, { revisions: [] }, contract, registry), /published contract revisions are immutable/);
  assertRevisionHistory(contract, registry, contract, registry);
  checks += 4;
  console.log(`local_api_version_tests_ok: checks=${checks}`);
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
