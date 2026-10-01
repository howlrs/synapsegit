#!/usr/bin/env node
// Requires a read-compatibility fixture (Issue #165) for every annotated
// release tag that is an ancestor of HEAD and older than the current
// synapse-cli version, and a matching row in the fixtures README.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const ROOT = "crates/synapse-cli/tests/fixtures/releases";
const CLI_MANIFEST = "crates/synapse-cli/Cargo.toml";

// X.Y.Z or X.Y.Z-rc.N; a release candidate sorts before its release.
export function parseVersion(version) {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-rc\.([1-9]\d*))?$/.exec(version);
  if (!match) return null;
  return [Number(match[1]), Number(match[2]), Number(match[3]), match[4] === undefined ? null : Number(match[4])];
}

export function compareVersions(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  const [leftCandidate, rightCandidate] = [left[3] ?? null, right[3] ?? null];
  if (leftCandidate === rightCandidate) return 0;
  if (leftCandidate === null) return 1;
  if (rightCandidate === null) return -1;
  return leftCandidate - rightCandidate;
}

export function requiredFixtureTags(lines, currentVersion) {
  const current = parseVersion(currentVersion);
  if (!current) throw new Error(`current version is not a supported release version: ${currentVersion}`);
  return lines.map((line) => {
    const [tag, objectType, commit, ancestor] = line.split("\t");
    return { tag, objectType, commit, ancestor, version: parseVersion(tag ?? "") };
  }).filter(({ tag, objectType, commit, ancestor, version }) => (
    /^v/.test(tag ?? "") && objectType === "tag" && /^[0-9a-f]{40}$/.test(commit ?? "")
      && ancestor === "true" && version !== null && compareVersions(version, current) < 0
  )).sort((left, right) => compareVersions(left.version, right.version)).map(({ tag }) => tag);
}

export function fixtureFailures(required, present, readmeTags) {
  const failures = [];
  for (const tag of required) {
    if (!present.has(tag)) failures.push(`${tag}: missing fixture; run scripts/generate_release_fixture.sh with the verified ${tag} release archive`);
    if (!readmeTags.has(tag)) failures.push(`${tag}: missing row in ${ROOT}/README.md`);
  }
  for (const tag of present) {
    if (!parseVersion(tag) || !tag.startsWith("v")) failures.push(`${tag}: fixture directory must be named after a vX.Y.Z or vX.Y.Z-rc.N release tag`);
  }
  return failures;
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

function main() {
  const version = readFileSync(CLI_MANIFEST, "utf8").match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  if (!version) throw new Error(`${CLI_MANIFEST} has no package version`);
  const lines = git(["for-each-ref", "--format=%(refname:strip=2)%09%(objecttype)%09%(*objectname)", "refs/tags"])
    .split("\n").filter(Boolean).map((line) => {
      const [tag, objectType, commit] = line.split("\t");
      let ancestor = "false";
      if (objectType === "tag" && /^[0-9a-f]{40}$/.test(commit ?? "")) {
        try { git(["merge-base", "--is-ancestor", commit, "HEAD"]); ancestor = "true"; } catch { ancestor = "false"; }
      }
      return `${tag}\t${objectType}\t${commit}\t${ancestor}`;
    });
  const required = requiredFixtureTags(lines, version);
  const present = new Set(readdirSync(ROOT, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name));
  for (const tag of present) {
    for (const part of ["repository/refs.sqlite3", "repository/cas/objects", "archive/manifest.json", "archive/manifest.sha256"]) {
      if (!existsSync(path.join(ROOT, tag, part))) throw new Error(`${tag}: fixture is missing ${part}`);
    }
  }
  const readme = readFileSync(path.join(ROOT, "README.md"), "utf8");
  const readmeTags = new Set([...readme.matchAll(/^\| `(v[^`]+)` \|/gmu)].map((match) => match[1]));
  const failures = fixtureFailures(required, present, readmeTags);
  if (failures.length > 0) throw new Error(failures.join("\n"));
  console.log(`release fixtures verified: ${present.size} fixture(s), ${required.length} required below ${version}`);
}

function selfTest() {
  const commit = (digit) => digit.repeat(40);
  const lines = [
    `v0.2.0\ttag\t${commit("2")}\ttrue`,
    `v0.1.0\ttag\t${commit("1")}\ttrue`,
    `v1.0.0-rc.1\ttag\t${commit("3")}\ttrue`,
    `v0.3.0\ttag\t${commit("4")}\tfalse`,
    `v0.4.0-lightweight\tcommit\t${commit("5")}\ttrue`,
  ];
  assert.deepEqual(requiredFixtureTags(lines, "1.0.0"), ["v0.1.0", "v0.2.0", "v1.0.0-rc.1"]);
  assert.deepEqual(requiredFixtureTags(lines, "0.2.0"), ["v0.1.0"]);
  assert.throws(() => requiredFixtureTags(lines, "1.0"), /supported release version/);
  assert.deepEqual(fixtureFailures(["v0.1.0"], new Set(["v0.1.0"]), new Set(["v0.1.0"])), []);
  assert.match(fixtureFailures(["v0.1.0", "v0.2.0"], new Set(["v0.1.0"]), new Set(["v0.1.0"])).join("\n"), /v0\.2\.0: missing fixture/);
  assert.match(fixtureFailures(["v0.1.0"], new Set(["v0.1.0"]), new Set()).join("\n"), /missing row/);
  assert.match(fixtureFailures([], new Set(["notes"]), new Set()).join("\n"), /must be named/);
  console.log("release fixture verifier self-test passed");
}

try {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--self-test") selfTest();
  else if (args.length === 0) main();
  else throw new Error("usage: verify_release_fixtures.mjs [--self-test]");
} catch (error) {
  console.error(`release_fixture_error: ${error.message}`);
  process.exitCode = 1;
}
