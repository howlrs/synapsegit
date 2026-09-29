#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const MINIMUM = [0, 11, 1];

export function parseVersion(version) {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(version);
  if (!match) return null;
  return match.slice(1).map(Number);
}

export function compareVersions(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}

export function selectMostRecentAnnotatedBaseline(lines, currentVersion) {
  const current = parseVersion(currentVersion);
  if (!current) throw new Error(`current version is not normal semver: ${currentVersion}`);
  const candidates = lines.map((line) => {
    const [tag, objectType, commit, ancestor] = line.split("\t");
    const version = parseVersion(tag);
    return { tag, objectType, commit, ancestor, version };
  }).filter(({ tag, objectType, commit, version, ancestor }) => (
    /^v/.test(tag ?? "") && objectType === "tag" && /^[0-9a-f]{40}$/.test(commit ?? "")
      && ancestor === "true" && version !== null && compareVersions(version, MINIMUM) >= 0 && compareVersions(version, current) < 0
  ));
  candidates.sort((left, right) => compareVersions(right.version, left.version));
  const selected = candidates[0];
  return selected ? {
    tag: selected.tag,
    objectType: selected.objectType,
    commit: selected.commit,
    version: selected.version,
  } : null;
}

function selfTest() {
  const commit11 = "1".repeat(40);
  const commit12 = "2".repeat(40);
  const commit13 = "3".repeat(40);
  const lines = [
    `v0.11.1\ttag\t${commit11}\ttrue`,
    `v0.12.0\ttag\t${commit12}\ttrue`,
    `v0.13.0\ttag\t${commit13}\ttrue`,
    `v0.13.1\ttag\t${"4".repeat(40)}\ttrue`,
    `v0.10.9\ttag\t${"5".repeat(40)}\ttrue`,
    `v0.13.0-lightweight\tcommit\t${"6".repeat(40)}\ttrue`,
    `v0.14.0\ttag\t${"7".repeat(40)}\ttrue`,
    `v0.13.0\ttag\t${"8".repeat(40)}\tfalse`,
    `0.12.9\ttag\t${"9".repeat(40)}\ttrue`,
  ];
  assert.deepEqual(selectMostRecentAnnotatedBaseline(lines, "0.13.1"), { tag: "v0.13.0", objectType: "tag", commit: commit13, version: [0, 13, 0] });
  assert.deepEqual(selectMostRecentAnnotatedBaseline(lines, "0.12.0"), { tag: "v0.11.1", objectType: "tag", commit: commit11, version: [0, 11, 1] });
  assert.equal(selectMostRecentAnnotatedBaseline([`v0.11.1\tcommit\t${commit11}\ttrue`], "0.13.1"), null);
  assert.equal(selectMostRecentAnnotatedBaseline([`v0.13.1\ttag\t${commit13}\ttrue`, `v0.13.0\ttag\t${commit12}\tfalse`], "0.13.1"), null);
  assert.equal(selectMostRecentAnnotatedBaseline([`0.12.9\ttag\t${commit12}\ttrue`], "0.13.1"), null);
  assert.throws(() => selectMostRecentAnnotatedBaseline(lines, "0.13"), /normal semver/);
  console.log("archive compatibility baseline selector self-test passed");
}

const args = process.argv.slice(2);
try {
  if (args.length === 1 && args[0] === "--self-test") selfTest();
  else if (args.length === 2 && args[0] === "--select") {
    const selected = selectMostRecentAnnotatedBaseline(readFileSync(0, "utf8").trim().split("\n").filter(Boolean), args[1]);
    if (selected) process.stdout.write(`${selected.tag}\t${selected.commit}\n`);
  } else throw new Error("usage: select_archive_compat_baseline.mjs --self-test | --select <current-version>");
} catch (error) {
  console.error(`archive_compatibility_error: ${error.message}`);
  process.exitCode = 1;
}
