# SynapseGit Core Protocol v0.1

Status: Stage 0 draft

This directory turns the architectural decisions in
[`docs/core_concept.md`](../../../docs/core_concept.md) into testable protocol
artifacts.

The four-week implementation and pilot gate is tracked in
[`docs/stage0_execution_plan.md`](../../../docs/stage0_execution_plan.md); the
runtime/storage decision is in
[`docs/runtime_architecture.md`](../../../docs/runtime_architecture.md).

Normative Stage 0 documents are [`oid-profile.md`](./oid-profile.md) and
[`operations.md`](./operations.md). JSON Schema constrains shape; operations
defines graph, time, Ref CAS, deletion, and Creative AI rules that a schema
cannot prove.

## Scope

The v0.1 draft defines:

- deterministic content IDs for structured Core objects;
- an immutable `RecordEnvelope` without a self-referential OID field;
- typed records for actors, subjects, activities, observations, claims,
  capture profiles, analysis results, Creative AI context and delegation,
  policy, detached assurance, evidence gaps, and tombstones;
- content-addressed manifest trees and commits;
- golden fixtures proving canonicalization and OID stability.

It does not yet define:

- HTTP transport or authentication;
- repository federation or cross-repository merge;
- a production JSON Schema registry;
- media adapter execution;
- C2PA, W3C PROV, ODRL, BagIt, or OCFL export profiles.

## Object families

| Object | OID prefix | Canonical content |
|---|---|---|
| Raw blob | `blob:sg-oid-v1:sha256:` | Original bytes, unchanged |
| Immutable record | `record:sg-oid-v1:sha256:` | Synapse Canonical JSON |
| Manifest tree | `tree:sg-oid-v1:sha256:` | Synapse Canonical JSON |
| Commit | `commit:sg-oid-v1:sha256:` | Synapse Canonical JSON |

The OID is transport metadata. It is never embedded in the bytes from which
that same OID is calculated. APIs return objects as `{ "oid": ..., "body":
... }`.

## Schemas

- `schemas/common.schema.json`
- `schemas/record-envelope.schema.json`
- `schemas/record.schema.json` — required concrete-record dispatcher
- `schemas/actor.schema.json`
- `schemas/subject.schema.json`
- `schemas/activity.schema.json`
- `schemas/observation.schema.json`
- `schemas/claim.schema.json`
- `schemas/claim-reaction.schema.json`
- `schemas/capture-profile.schema.json`
- `schemas/analysis-result.schema.json`
- `schemas/context-pack.schema.json`
- `schemas/delegation-grant.schema.json`
- `schemas/decision-feedback.schema.json`
- `schemas/policy.schema.json`
- `schemas/assurance.schema.json`
- `schemas/evidence-gap.schema.json`
- `schemas/tombstone.schema.json`
- `schemas/manifest-tree.schema.json`
- `schemas/commit.schema.json`

## Verification

From the repository root:

```bash
node scripts/verify_core_fixtures.mjs
```

The verifier checks that all schema and fixture files are valid JSON, that only
property order, insignificant whitespace, and equivalent JSON string escapes
collapse to the same canonical bytes, and that SHA-256 OIDs match committed
golden values. Free-text NFC and NFD spellings intentionally remain distinct;
non-NFC path keys are rejected.

The fixture graph is a minimal creator/Creative AI flow:

```text
Creator + AI Actor
  -> Policy + DelegationGrant
  -> base Commit + ContextPack
  -> AI Activity + proposal-only Commit
  -> human DecisionFeedback
  -> optional Tombstone / archive restore check
```

It also covers duplicate keys, invalid UTF-8, BOM, number-token restrictions,
UTF-16 key ordering, set ordering, exact timestamps, normalized fixed-point
values, typed Manifest entries, first-parent direction, OID mismatch, and
`present/tombstoned/missing` closure. The verifier has no package dependency
beyond a current Node.js runtime. `--print-golden` prints candidate values for
review but never changes the committed fixture.

This is a draft profile. Implementations may experiment against it, but OID
values are not declared permanently frozen until the Stage 0 inter-language
test has at least two independent implementations.
