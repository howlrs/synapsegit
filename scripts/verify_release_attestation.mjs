#!/usr/bin/env node
// Enforces release-specific policy on verified `gh attestation` JSON output.
//
// Usage: node scripts/verify_release_attestation.mjs ATTESTATION_JSON ARCHIVE SHA256 TAG
import { readFileSync } from "node:fs";

function fail(message) {
  throw new Error(`release_attestation_error: ${message}`);
}

function requireString(value, name) {
  if (typeof value !== "string" || value.length === 0) {
    fail(`${name} must be a non-empty string`);
  }
  return value;
}

const [, , jsonPath, archive, digest, tag] = process.argv;
if (process.argv.length !== 6) {
  fail("usage: scripts/verify_release_attestation.mjs ATTESTATION_JSON ARCHIVE SHA256 TAG");
}
if (!/^[0-9a-f]{64}$/u.test(digest)) {
  fail("expected SHA-256 digest must be 64 lowercase hexadecimal characters");
}
if (!tag.startsWith("v")) {
  fail("tag must start with v");
}

let document;
try {
  document = JSON.parse(readFileSync(jsonPath, "utf8"));
} catch (error) {
  fail(`cannot read attestation JSON: ${error.message}`);
}

if (!Array.isArray(document) || document.length === 0) {
  fail("expected one or more verified attestations");
}

// A retry can attest the same reproducible archive again. gh returns each
// verified attestation; all must retain the expected tag, runner and digest.
for (const entry of document) {
  if (typeof entry !== "object" || entry === null) {
    fail("verified attestation must be an object");
  }
  const result = entry.verificationResult;
  const certificate = result?.signature?.certificate;
  const subjects = result?.statement?.subject;
  if (typeof certificate !== "object" || certificate === null || !Array.isArray(subjects)) {
    fail("verification result is missing certificate or statement subject");
  }

  const expectedSigner = `https://github.com/howlrs/synapsegit/.github/workflows/release.yml@refs/tags/${tag}`;
  if (requireString(certificate.buildSignerURI, "certificate buildSignerURI") !== expectedSigner) {
    fail("certificate buildSignerURI does not identify the tagged release workflow");
  }
  if (requireString(certificate.subjectAlternativeName, "certificate subjectAlternativeName") !== expectedSigner) {
    fail("certificate subjectAlternativeName does not identify the tagged release workflow");
  }
  if (requireString(certificate.runnerEnvironment, "certificate runnerEnvironment") !== "github-hosted") {
    fail("certificate runnerEnvironment is not github-hosted");
  }
  if (subjects.length !== 1 || typeof subjects[0] !== "object" || subjects[0] === null) {
    fail("expected exactly one statement subject");
  }
  if (subjects[0].name !== archive) {
    fail("statement subject does not identify the selected archive");
  }
  if (typeof subjects[0].digest !== "object" || subjects[0].digest === null) {
    fail("selected statement subject has no digest object");
  }
  if (requireString(subjects[0].digest.sha256, "selected subject SHA-256") !== digest) {
    fail("selected subject SHA-256 does not match SHA256SUMS");
  }
}

console.log(`release attestation policy passed: ${archive}`);
