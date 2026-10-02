#!/usr/bin/env node
// Checks the tag grammar of scripts/verify_release_version.sh. Accepted tags
// reach the crate-version comparison, so their failure names a manifest
// instead of the tag format.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { parseReleaseVersion } from "./release-version.mjs";

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

for (const [tag, expected] of [
  ["v0.13.1", { title: "SynapseGit v0.13.1 — Stage 0 preview", isPrerelease: true, isLatest: false }],
  ["v1.0.0-rc.1", { title: "SynapseGit v1.0.0-rc.1 — release candidate", isPrerelease: true, isLatest: false }],
  ["v1.0.0", { title: "SynapseGit v1.0.0", isPrerelease: false, isLatest: true }],
  ["v1.7.3", { title: "SynapseGit v1.7.3", isPrerelease: false, isLatest: true }],
]) {
  const actual = parseReleaseVersion(tag);
  assert.ok(actual, `${tag} should be publishable`);
  assert.deepEqual(
    { title: actual.title, isPrerelease: actual.isPrerelease, isLatest: actual.isLatest },
    expected,
    `${tag} should use the expected publication classification`,
  );
}
for (const tag of ["", "v0.13", "v01.0.0", "v1.0.0-rc.0", "v1.0.0-beta.1"]) {
  assert.equal(parseReleaseVersion(tag), null, `${JSON.stringify(tag)} should not be publishable`);
}

const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "synapsegit-release-version-"));
try {
  const outputPath = path.join(temporaryDirectory, "github-output");
  const output = spawnSync(
    process.execPath,
    ["scripts/release-version.mjs", "v1.0.0", "--github-output", outputPath],
    { encoding: "utf8" },
  );
  assert.equal(output.status, 0, output.stderr);
  assert.equal(output.stdout, "");
  assert.equal(
    fs.readFileSync(outputPath, "utf8"),
    "title=SynapseGit v1.0.0\nprerelease=false\nlatest=true\n",
    "stable release output should include its parsed title and publication flags",
  );

  const invalid = spawnSync(process.execPath, ["scripts/release-version.mjs", "v1.0.0-beta.1"], { encoding: "utf8" });
  assert.equal(invalid.status, 1, "invalid release tags should fail the CLI");
  assert.equal(invalid.stdout, "");
  assert.match(invalid.stderr, /usage: node scripts\/release-version\.mjs/u);
} finally {
  fs.rmSync(temporaryDirectory, { recursive: true, force: true });
}
console.log("release version tag tests passed");
