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

  fs.writeFileSync(reference, original);

  // SECURITY.md must describe the current stable major or prerelease minor.
  const security = path.join(fixture, "SECURITY.md");
  const securityOriginal = fs.readFileSync(security, "utf8");
  const supportedRow = /^\| Latest v[^|]+ \|/mu;
  assert.match(securityOriginal, supportedRow);
  fs.writeFileSync(security, securityOriginal.replace(supportedRow, "| Latest v0.1.x prerelease |"));
  result = verify();
  assert.notEqual(result.status, 0, "a stale SECURITY.md support row must fail");
  assert.match(result.stderr, /SECURITY\.md: Supported versions names Latest v0\.1\.x prerelease, but synapse-cli is /u);
  fs.writeFileSync(security, securityOriginal);

  // Release notes after v0.13.1 double as the archive README and must keep the
  // verification, backup, and license essentials.
  const cliManifest = path.join(fixture, "crates", "synapse-cli", "Cargo.toml");
  const manifestOriginal = fs.readFileSync(cliManifest, "utf8");
  const versionLine = /^version = "(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-rc\.[1-9]\d*)?"$/mu.exec(manifestOriginal);
  assert.ok(versionLine, "synapse-cli must use X.Y.Z or X.Y.Z-rc.N");
  const major = Number(versionLine[1]);
  const minor = Number(versionLine[2]) + 1;
  const nextVersion = `${major}.${minor}.0`;
  const nextTag = `v${nextVersion}`;
  fs.writeFileSync(cliManifest, manifestOriginal.replace(versionLine[0], `version = "${nextVersion}"`));
  const stableSupport = major === 0 ? `Latest v${major}.${minor}.x prerelease` : `Latest v${major}.x`;
  fs.writeFileSync(security, securityOriginal.replace(supportedRow, `| ${stableSupport} |`));
  const notes = path.join(fixture, "docs", "releases", `${nextTag}.md`);
  result = verify();
  assert.notEqual(result.status, 0, "missing release notes must fail");
  assert.ok(result.stderr.includes(`docs/releases/${nextTag}.md: release notes for the current synapse-cli version are missing`), result.stderr);

  fs.writeFileSync(notes, [
    `# SynapseGit ${nextTag}`,
    "",
    "After the tag workflow publishes the prerelease, read the installation guide.",
    "",
  ].join("\n"));
  result = verify();
  assert.notEqual(result.status, 0, "incomplete release notes must fail");
  for (const expected of [
    "missing the rule not to extract or install an unverified archive",
    "missing the export-before-update backup precaution",
    "missing that the license is not OSI-approved",
    "missing the separate written permission requirement",
    "missing the THIRD_PARTY_NOTICES.md reference",
    `missing the tag-pinned installation guide link https://github.com/howlrs/synapsegit/blob/${nextTag}/docs/install.md`,
    "contains pre-publication wording",
  ]) {
    assert.ok(result.stderr.includes(`docs/releases/${nextTag}.md: ${expected}`), `${expected}\n${result.stderr}`);
  }

  fs.writeFileSync(notes, [
    `# SynapseGit ${nextTag}`,
    "",
    "Stop if either verification command fails. Do not extract or install an",
    "unverified archive.",
    `See the [installation guide](https://github.com/howlrs/synapsegit/blob/${nextTag}/docs/install.md).`,
    "",
    "Export important repositories with the old binary before updating so that a",
    "known-good recovery copy remains available.",
    "",
    "It is not an OSI-approved open-source license. Commercial use requires separate written permission.",
    "Third-party terms are reproduced in `THIRD_PARTY_NOTICES.md`.",
    "",
  ].join("\n"));
  result = verify();
  assert.equal(result.status, 0, `wrapped required prose must pass\n${result.stderr}`);

  // Candidates use the same required prose, with their own tag-pinned link.
  const candidateTag = `${nextTag}-rc.1`;
  const candidateNotes = path.join(fixture, "docs", "releases", `${candidateTag}.md`);
  fs.writeFileSync(cliManifest, manifestOriginal.replace(versionLine[0], `version = "${nextVersion}-rc.1"`));
  // A stable support row cannot describe a candidate. Also verify the inverse.
  if (major > 0) {
    fs.writeFileSync(candidateNotes, fs.readFileSync(notes, "utf8").replaceAll(nextTag, candidateTag));
    result = verify();
    assert.notEqual(result.status, 0, "a candidate must require prerelease support wording");
    assert.ok(result.stderr.includes(`use Latest v${major}.${minor}.x prerelease`), result.stderr);
    fs.writeFileSync(cliManifest, manifestOriginal.replace(versionLine[0], `version = "${nextVersion}"`));
    fs.writeFileSync(security, securityOriginal.replace(supportedRow, `| Latest v${major}.${minor}.x prerelease |`));
    result = verify();
    assert.notEqual(result.status, 0, "a stable version must reject prerelease support wording");
    assert.ok(result.stderr.includes(`use Latest v${major}.x`), result.stderr);
    fs.writeFileSync(cliManifest, manifestOriginal.replace(versionLine[0], `version = "${nextVersion}-rc.1"`));
  }
  fs.writeFileSync(security, securityOriginal.replace(supportedRow, `| Latest v${major}.${minor}.x prerelease |`));
  const completeNotes = fs.readFileSync(notes, "utf8");
  fs.writeFileSync(candidateNotes, completeNotes.replaceAll(nextTag, candidateTag));
  result = verify();
  assert.equal(result.status, 0, `candidate release notes must pass\n${result.stderr}`);
  fs.writeFileSync(candidateNotes, completeNotes);
  result = verify();
  assert.notEqual(result.status, 0, "a candidate must not use the normal release's pinned guide link");
  assert.ok(result.stderr.includes(`missing the tag-pinned installation guide link https://github.com/howlrs/synapsegit/blob/${candidateTag}/docs/install.md`), result.stderr);

  console.log("docs_test_ok");
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
