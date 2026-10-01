#!/usr/bin/env node
// Recomputes the byte-identity implementation OID recorded by each published
// release from its tagged source and checks the audited reader allowlist in
// crates/synapse-creator/src/report.rs. Every annotated release tag that is an
// ancestor of HEAD and older than the current synapse-cli version must be
// listed, and every listed OID must match its tag.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const REPORT_SOURCE = "crates/synapse-creator/src/report.rs";
const CLI_MANIFEST = "crates/synapse-cli/Cargo.toml";
const OBSERVATION_LIBRARY = "crates/synapse-observation/src/lib.rs";
const ALLOWLIST_NAME = "HISTORIC_BYTE_IDENTITY_IMPLEMENTATION_OIDS";
// Mirrors implementation_bundle() in crates/synapse-observation/src/lib.rs.
const BUNDLE_MAGIC = "synapsegit-observation-implementation-bundle-v1\0";
const BUNDLE_MEMBERS = [
  ["Cargo.toml", "crates/synapse-observation/Cargo.toml"],
  ["src/byte_identity.rs", "crates/synapse-observation/src/byte_identity.rs"],
  ["src/lib.rs", "crates/synapse-observation/src/lib.rs"],
];

// X.Y.Z or X.Y.Z-rc.N, with an optional leading "v". The fourth element is the
// release-candidate number, or null for a normal release.
export function parseVersion(version) {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-rc\.([1-9]\d*))?$/.exec(version);
  if (!match) return null;
  return [Number(match[1]), Number(match[2]), Number(match[3]), match[4] === undefined ? null : Number(match[4])];
}

// A release candidate sorts before its normal release, as in SemVer.
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

export function parseAllowlist(source) {
  const start = source.indexOf(`const ${ALLOWLIST_NAME}: &[(&str, &str)] = &[`);
  if (start < 0) throw new Error(`${ALLOWLIST_NAME} is missing or is not a (tag, OID) table`);
  const end = source.indexOf("];", start);
  if (end < 0) throw new Error(`${ALLOWLIST_NAME} is not terminated`);
  const body = source.slice(source.indexOf("= &[", start) + 4, end);
  const entries = [];
  const pattern = /\(\s*"([^"]*)"\s*,\s*"([^"]*)"\s*,?\s*\)\s*,?/gy;
  let cursor = 0;
  const skipComments = () => {
    for (;;) {
      const whitespace = /\s*/y;
      whitespace.lastIndex = cursor;
      whitespace.exec(body);
      cursor = whitespace.lastIndex;
      if (body.startsWith("//", cursor)) {
        const newline = body.indexOf("\n", cursor);
        cursor = newline < 0 ? body.length : newline + 1;
      } else return;
    }
  };
  for (skipComments(); cursor < body.length; skipComments()) {
    pattern.lastIndex = cursor;
    const match = pattern.exec(body);
    if (!match) throw new Error(`${ALLOWLIST_NAME} has an unparseable entry near: ${body.slice(cursor, cursor + 60).trim()}`);
    entries.push({ tag: match[1], oid: match[2] });
    cursor = pattern.lastIndex;
  }
  return entries;
}

export function validateEntries(entries) {
  const failures = [];
  const oids = new Set();
  let previous = null;
  for (const { tag, oid } of entries) {
    const version = parseVersion(tag);
    if (!version || !tag.startsWith("v")) {
      failures.push(`${tag}: entry must name a vX.Y.Z or vX.Y.Z-rc.N release tag`);
      continue;
    }
    if (!/^blob:sg-oid-v1:sha256:[0-9a-f]{64}$/.test(oid)) failures.push(`${tag}: ${oid} is not a Blob OID`);
    if (oids.has(oid)) failures.push(`${tag}: duplicate implementation OID ${oid}`);
    oids.add(oid);
    if (previous && compareVersions(version, previous) <= 0) failures.push(`${tag}: entries must be unique and in ascending version order`);
    previous = version;
  }
  return failures;
}

export function requiredReleaseTags(lines, currentVersion) {
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

export function bundleOid(readMember) {
  const parts = [Buffer.from(BUNDLE_MAGIC)];
  for (const [name, path] of BUNDLE_MEMBERS) {
    const bytes = readMember(path);
    const nameLength = Buffer.alloc(8);
    nameLength.writeBigUInt64BE(BigInt(Buffer.byteLength(name)));
    const byteLength = Buffer.alloc(8);
    byteLength.writeBigUInt64BE(BigInt(bytes.length));
    parts.push(nameLength, Buffer.from(name), byteLength, bytes);
  }
  return `blob:sg-oid-v1:sha256:${createHash("sha256").update(Buffer.concat(parts)).digest("hex")}`;
}

function git(args, options = {}) {
  return execFileSync("git", args, { maxBuffer: 64 * 1024 * 1024, ...options });
}

function tagCommit(tag) {
  try {
    if (git(["cat-file", "-t", `refs/tags/${tag}`], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim() !== "tag") return null;
    return git(["rev-parse", `refs/tags/${tag}^{commit}`], { encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

function currentCliVersion() {
  const match = readFileSync(CLI_MANIFEST, "utf8").match(/^version\s*=\s*"([^"]+)"\s*$/m);
  if (!match) throw new Error(`${CLI_MANIFEST} has no package version`);
  return match[1];
}

function localTagLines() {
  const refs = git(["for-each-ref", "--format=%(refname:strip=2)%09%(objecttype)%09%(*objectname)", "refs/tags"], { encoding: "utf8" })
    .split("\n").filter(Boolean);
  return refs.map((line) => {
    const [tag, objectType, commit] = line.split("\t");
    let ancestor = false;
    if (objectType === "tag" && /^[0-9a-f]{40}$/.test(commit ?? "")) {
      try {
        git(["merge-base", "--is-ancestor", commit, "HEAD"], { stdio: "ignore" });
        ancestor = true;
      } catch {
        ancestor = false;
      }
    }
    return `${tag}\t${objectType}\t${commit ?? ""}\t${ancestor}`;
  });
}

function releaseBundleOid(commit) {
  return bundleOid((path) => git(["cat-file", "blob", `${commit}:${path}`]));
}

function verify() {
  const library = readFileSync(OBSERVATION_LIBRARY, "utf8");
  const formatMarkers = [`b"${BUNDLE_MAGIC.replace("\0", "\\0")}"`, ...BUNDLE_MEMBERS.map(([name]) => `("${name}",`)];
  for (const marker of formatMarkers) {
    if (!library.includes(marker)) {
      throw new Error(`${OBSERVATION_LIBRARY} no longer shows bundle marker ${marker}; update this verifier with the new bundle format`);
    }
  }
  const entries = parseAllowlist(readFileSync(REPORT_SOURCE, "utf8"));
  const failures = validateEntries(entries);
  for (const { tag, oid } of entries) {
    const commit = tagCommit(tag);
    if (!commit) {
      failures.push(`${tag}: annotated tag is unavailable locally; fetch release tags before running this check`);
      continue;
    }
    const expected = releaseBundleOid(commit);
    if (expected !== oid) failures.push(`${tag}: listed ${oid}, but its tagged source bundle is ${expected}`);
  }
  const listed = new Set(entries.map(({ tag }) => tag));
  const required = requiredReleaseTags(localTagLines(), currentCliVersion());
  for (const tag of required) {
    if (!listed.has(tag)) {
      failures.push(`${tag}: published release below the current version is missing; add (${JSON.stringify(tag)}, ${JSON.stringify(releaseBundleOid(tagCommit(tag)))})`);
    }
  }
  if (required.length === 0) failures.push("no annotated release tags are available locally; fetch release tags before running this check");
  if (failures.length > 0) {
    for (const failure of failures) console.error(`byte_identity_allowlist_error: ${failure}`);
    process.exitCode = 1;
    return;
  }
  console.log(`byte-identity allowlist verified: ${entries.length} release bundle(s), ${required.length} required below ${currentCliVersion()}`);
}

function selfTest() {
  const oid = (digit) => `blob:sg-oid-v1:sha256:${digit.repeat(64)}`;
  const source = [
    "// comment",
    `const ${ALLOWLIST_NAME}: &[(&str, &str)] = &[`,
    "    // leading comment",
    `    ("v0.1.0", "${oid("1")}"),`,
    "    (",
    `        "v0.2.0",`,
    `        "${oid("2")}",`,
    "    ),",
    "];",
  ].join("\n");
  assert.deepEqual(parseAllowlist(source), [{ tag: "v0.1.0", oid: oid("1") }, { tag: "v0.2.0", oid: oid("2") }]);
  assert.throws(() => parseAllowlist(`const ${ALLOWLIST_NAME}: &[&str] = &["${oid("1")}"];`), /not a \(tag, OID\) table/);
  assert.throws(() => parseAllowlist(`const ${ALLOWLIST_NAME}: &[(&str, &str)] = &[\n    ("v0.1.0"),\n];`), /unparseable entry/);
  assert.deepEqual(validateEntries([{ tag: "v0.1.0", oid: oid("1") }, { tag: "v0.2.0", oid: oid("2") }]), []);
  assert.match(validateEntries([{ tag: "v0.2.0", oid: oid("2") }, { tag: "v0.1.0", oid: oid("1") }]).join("\n"), /ascending/);
  assert.match(validateEntries([{ tag: "v0.1.0", oid: oid("1") }, { tag: "v0.2.0", oid: oid("1") }]).join("\n"), /duplicate/);
  assert.match(validateEntries([{ tag: "0.1.0", oid: oid("1") }]).join("\n"), /vX\.Y\.Z/);
  assert.deepEqual(validateEntries([{ tag: "v0.14.0", oid: oid("1") }, { tag: "v1.0.0-rc.1", oid: oid("2") }, { tag: "v1.0.0", oid: oid("3") }]), []);
  assert.match(validateEntries([{ tag: "v0.1.0", oid: "blob:sg-oid-v1:sha256:1" }]).join("\n"), /not a Blob OID/);
  const commit = (digit) => digit.repeat(40);
  const lines = [
    `v0.13.0\ttag\t${commit("3")}\ttrue`,
    `v0.1.0\ttag\t${commit("1")}\ttrue`,
    `v0.13.1\ttag\t${commit("4")}\ttrue`,
    `v0.14.0\ttag\t${commit("5")}\ttrue`,
    `v0.12.9\ttag\t${commit("6")}\tfalse`,
    `v0.12.0-lightweight\tcommit\t${commit("7")}\ttrue`,
    `0.12.0\ttag\t${commit("8")}\ttrue`,
  ];
  assert.deepEqual(requiredReleaseTags(lines, "0.13.1"), ["v0.1.0", "v0.13.0"]);
  assert.deepEqual(requiredReleaseTags(lines, "0.14.0"), ["v0.1.0", "v0.13.0", "v0.13.1"]);
  // Sessions recorded by a published release candidate must stay readable, so
  // candidate tags are required like normal releases, in SemVer order.
  const withCandidates = [...lines, `v1.0.0-rc.2\ttag\t${commit("a")}\ttrue`, `v1.0.0-rc.1\ttag\t${commit("b")}\ttrue`];
  assert.deepEqual(requiredReleaseTags(withCandidates, "1.0.0-rc.2"), ["v0.1.0", "v0.13.0", "v0.13.1", "v0.14.0", "v1.0.0-rc.1"]);
  assert.deepEqual(requiredReleaseTags(withCandidates, "1.0.0"), ["v0.1.0", "v0.13.0", "v0.13.1", "v0.14.0", "v1.0.0-rc.1", "v1.0.0-rc.2"]);
  assert.throws(() => requiredReleaseTags(lines, "0.14"), /supported release version/);
  const members = new Map([
    ["crates/synapse-observation/Cargo.toml", Buffer.from("manifest\n")],
    ["crates/synapse-observation/src/byte_identity.rs", Buffer.from("algorithm\n")],
    ["crates/synapse-observation/src/lib.rs", Buffer.from("library\n")],
  ]);
  const framed = Buffer.concat([
    Buffer.from(BUNDLE_MAGIC),
    ...BUNDLE_MEMBERS.flatMap(([name, path]) => {
      const nameLength = Buffer.alloc(8);
      nameLength.writeBigUInt64BE(BigInt(name.length));
      const byteLength = Buffer.alloc(8);
      byteLength.writeBigUInt64BE(BigInt(members.get(path).length));
      return [nameLength, Buffer.from(name), byteLength, members.get(path)];
    }),
  ]);
  assert.equal(bundleOid((path) => members.get(path)), `blob:sg-oid-v1:sha256:${createHash("sha256").update(framed).digest("hex")}`);
  console.log("byte-identity allowlist verifier self-test passed");
}

const args = process.argv.slice(2);
try {
  if (args.length === 1 && args[0] === "--self-test") selfTest();
  else if (args.length === 0) verify();
  else throw new Error("usage: verify_byte_identity_allowlist.mjs [--self-test]");
} catch (error) {
  console.error(`byte_identity_allowlist_error: ${error.message}`);
  process.exitCode = 1;
}
