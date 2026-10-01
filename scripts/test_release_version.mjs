#!/usr/bin/env node
// Checks the tag grammar of scripts/verify_release_version.sh. Accepted tags
// reach the crate-version comparison, so their failure names a manifest
// instead of the tag format.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";

const FORMAT_ERROR = /expected a semantic version tag/u;

function run(tag) {
  const result = spawnSync("bash", ["scripts/verify_release_version.sh", tag], { encoding: "utf8" });
  return { status: result.status, stderr: result.stderr };
}

for (const tag of ["v0.1.0", "v1.0.0", "v1.0.0-rc.1", "v1.2.3-rc.10"]) {
  const { stderr } = run(tag);
  assert.doesNotMatch(stderr, FORMAT_ERROR, `${tag} should pass the tag format check`);
}
for (const tag of ["", "1.0.0", "v1.0", "v01.0.0", "v1.0.0-rc", "v1.0.0-rc.0", "v1.0.0-rc.01", "v1.0.0rc1", "v1.0.0-beta.1", "v1.0.0-rc.1+build"]) {
  const { status, stderr } = run(tag);
  assert.equal(status, 1, `${JSON.stringify(tag)} should fail`);
  assert.match(stderr, FORMAT_ERROR, `${JSON.stringify(tag)} should fail the tag format check`);
}
console.log("release version tag tests passed");
