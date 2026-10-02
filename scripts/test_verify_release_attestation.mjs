#!/usr/bin/env node
// Regression tests for scripts/verify_release_attestation.mjs.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const verify = fileURLToPath(new URL("./verify_release_attestation.mjs", import.meta.url));
const archive = "synapsegit-v1.2.3-x86_64-unknown-linux-gnu.tar.gz";
const digest = "a".repeat(64);
const tag = "v1.2.3";
const signer = `https://github.com/howlrs/synapsegit/.github/workflows/release.yml@refs/tags/${tag}`;

function validAttestation() {
  return [{
    verificationResult: {
      signature: { certificate: { buildSignerURI: signer, subjectAlternativeName: signer, runnerEnvironment: "github-hosted" } },
      statement: { subject: [{ name: archive, digest: { sha256: digest } }] },
    },
  }];
}

function run(document, selectedTag = tag) {
  const directory = mkdtempSync(join(tmpdir(), "synapsegit-attestation-test-"));
  try {
    const path = join(directory, "attestation.json");
    writeFileSync(path, typeof document === "string" ? document : JSON.stringify(document));
    return spawnSync(process.execPath, [verify, path, archive, digest, selectedTag], { encoding: "utf8" });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function expectRejected(name, document, selectedTag = tag) {
  const result = run(document, selectedTag);
  assert.notEqual(result.status, 0, `${name} was accepted`);
}

assert.equal(run(validAttestation()).status, 0, "valid attestation was rejected");

const rcTag = "v1.2.3-rc.1";
const rcAttestation = validAttestation();
const rcSigner = signer.replace(tag, rcTag);
rcAttestation[0].verificationResult.signature.certificate.buildSignerURI = rcSigner;
rcAttestation[0].verificationResult.signature.certificate.subjectAlternativeName = rcSigner;
assert.equal(run(rcAttestation, rcTag).status, 0, "release candidate attestation was rejected");

const alteredDigest = validAttestation();
alteredDigest[0].verificationResult.statement.subject[0].digest.sha256 = "b".repeat(64);
expectRejected("altered digest", alteredDigest);

const alteredWorkflow = validAttestation();
alteredWorkflow[0].verificationResult.signature.certificate.buildSignerURI = signer.replace("release.yml", "other.yml");
expectRejected("altered workflow", alteredWorkflow);

const alteredRef = validAttestation();
alteredRef[0].verificationResult.signature.certificate.subjectAlternativeName = signer.replace(tag, "v9.9.9");
expectRejected("altered ref", alteredRef);
expectRejected("release candidate mismatched ref", rcAttestation, tag);

const selfHosted = validAttestation();
selfHosted[0].verificationResult.signature.certificate.runnerEnvironment = "self-hosted";
expectRejected("self-hosted runner", selfHosted);

expectRejected("malformed JSON", "{");
expectRejected("malformed manifest", [{ verificationResult: {} }]);
expectRejected("empty attestations", []);
assert.equal(run([...validAttestation(), ...validAttestation()]).status, 0, "valid repeated attestations must be accepted");
expectRejected("one changed digest among repeated attestations", [...validAttestation(), ...alteredDigest]);
expectRejected("one changed workflow among repeated attestations", [...validAttestation(), ...alteredWorkflow]);
expectRejected("null attestation among repeated attestations", [...validAttestation(), null]);
const emptySubjects = validAttestation();
emptySubjects[0].verificationResult.statement.subject = [];
expectRejected("empty subjects", emptySubjects);

const multipleSubjects = validAttestation();
multipleSubjects[0].verificationResult.statement.subject.push({ name: "other.tar.gz", digest: { sha256: digest } });
expectRejected("multiple subjects", multipleSubjects);

console.log("verify_release_attestation tests passed");
