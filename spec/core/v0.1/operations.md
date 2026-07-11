# Synapse Core operations and semantic validation v0.1

Status: Stage 0 draft

This document defines the behavior that JSON Schema alone cannot enforce. The
rules apply independently of the database, transport, or programming language.

## 1. Object ingestion

### 1.1 Structured object

`put_object(claimed_oid, input_bytes)` is atomic and performs these steps in
order:

1. Strictly decode and parse the bytes using `oid-profile.md`.
2. Select the concrete schema from `object_type`; for a record, dispatch again
   by `record_type` through `schemas/record.schema.json`.
3. Apply the semantic checks in this document.
4. Produce canonical bytes and calculate the domain-separated OID.
5. Reject an OID prefix/body mismatch or a digest mismatch.
6. Store the canonical bytes under the calculated OID with create-if-absent
   semantics. Existing bytes at that OID must be identical.

The API wrapper `{ "oid": ..., "body": ... }`, ACL, availability, receive time,
and database primary key are not hash input.

### 1.2 Blob

`put_blob(claimed_oid, bytes)` hashes the original bytes without media
transcoding, metadata injection, Unicode handling, or newline conversion.
Filename, media type, privacy policy, and derived previews are separate records.

## 2. Ref update

A Ref is a mutable pointer, not a Commit property. An update request is transport
metadata:

```json
{
  "ref_name": "decision/main",
  "expected_head": "commit:sg-oid-v1:sha256:...",
  "new_head": "commit:sg-oid-v1:sha256:..."
}
```

`expected_head` is `null` only when creating a Ref. The store must:

1. verify that `new_head` is a valid stored Commit;
2. validate the required reference closure from `new_head`;
3. atomically compare the current head with `expected_head`;
4. if equal, append a reflog entry and replace the head in one transaction;
5. otherwise return `ref_conflict` without changing either Ref or reflog.

Servers do not use last-write-wins, implicit rebase, or automatic merge. A
conflict becomes a new proposal Ref or an explicit merge Commit.

`parents` is a sequence and must never be sorted. `parents[0]` is the mainline
or first parent; later entries are additional merge inputs. A root Commit has no
parents, a non-merge Commit has at most one, and a merge Commit has at least two.

## 3. Closure and deletion

Reference resolution returns exactly one availability state:

- `present`: the object is stored and its OID verifies;
- `tombstoned`: a valid Tombstone targets the unavailable OID;
- `missing`: neither object nor applicable Tombstone is available.

Historical closure remains traversable when a payload is `tombstoned`; a UI
must show that absence rather than substitute an empty object. `missing` fails a
Ref update. A newly produced Analysis or Activity may not present a tombstoned
or missing object as an execution input. It may cite the Tombstone using an
explicit redaction/missing-evidence role.

A Tombstone never makes the erased bytes reconstructable and never proves that
every copy was deleted. Derivative purge is an operation over the dependency
graph; `affected_derivative_refs` records what the actor reports having handled.

## 4. Record reference semantics

JSON Schema validates OID syntax, while graph validation resolves the target and
checks its semantic type.

| Source field | Required target |
|---|---|
| `Observation.capture_profile_ref` | `record_type=capture_profile` |
| `Activity.ai_run.context_pack_ref` | `record_type=context_pack` |
| `Activity.ai_run.delegation_grant_ref` | `record_type=delegation_grant` |
| `Claim.ai_run_ref` | `record_type=activity`, `activity_kind=ai_run` |
| `ContextPack.policy_snapshot_ref` | `record_type=policy` |
| `ContextPack.delegation_grant_ref` | `record_type=delegation_grant` |
| `ClaimReaction.claim_ref` | `record_type=claim` |
| `Assurance.target_ref` | the object whose bytes or event are attested |
| `Tombstone.target_ref` | unavailable target OID; self-targeting is invalid |
| `supersedes` | same `entity_id` and `record_type`; no cycle |

The validator also rejects a Manifest entry whose `entry_kind` conflicts with
the referenced OID prefix.

## 5. Claim and assurance projections

A Claim is immutable and is `proposed` by its existence. Acknowledgement,
endorsement, dispute, rejection, withdrawal, and moderation are independent
ClaimReaction records. A displayed status is a projection by actor, policy, and
time; reactions never rewrite the Claim. Withdrawal authority and moderation
authority are policy checks, not hash rules.

Integrity and review are likewise not mutable Envelope fields:

- byte integrity is recalculated from the OID;
- signature, server receive time, and external timestamp are detached Assurance
  records that target an existing OID;
- coverage belongs to Observation or Analysis data;
- review of a proposition is a Claim or ClaimReaction.

An Assurance proves only the statement and method it records. It does not by
itself prove truth, authorship, copyright, consent, or the physical capture time.

## 6. Time and fixed-point semantics

In addition to schema validation:

- timestamp dates must exist in the proleptic Gregorian calendar;
- `ValidTime.interval.from` must not be later than `to`;
- temporal precision/uncertainty values must be non-negative and use `ms` or
  `s`;
- an Observation's authoritative event time is `payload.capture_time`; the
  optional Envelope `valid_time` should be omitted for Observation v0.1 to
  avoid two competing values;
- an Activity requires Envelope `valid_time`;
- `DelegationGrant.expires_at` must not precede its `recorded_at`;
- probability confidence uses unit `ratio` and lies from 0 through 1;
- confidence interval bounds use the same unit and lower is not greater than
  upper;
- fixed-point mantissa length is bounded by the schema and its normalized form
  may not contain removable trailing zeroes.

Nine fractional timestamp digits are a lexical interchange rule, not a claim
of nanosecond clock precision. Known resolution or uncertainty belongs in the
typed `precision` value.

## 7. Creative AI execution boundary

An effective AI capability is the intersection of:

1. the Actor's declared capability;
2. the principal's unexpired DelegationGrant;
3. the immutable Policy snapshot in the ContextPack;
4. the runtime's actual sandbox and connector capability.

The most restrictive result wins. Core v0.1 allows AI output only under
`proposal/*`. An AI may produce Artifacts, Analysis, and Claims, but it may not
advance `decision/*` or `release/*`, alter policy, export restricted data, erase
content, or cause a physical effect without the named human gate. `may_delegate`
and `max_child_depth` apply transitively.

Before accepting output, the service compares the live base Ref to the
ContextPack's `expected_ref_head`. A mismatch records `stale_base`; it does not
silently rebase. `generated_by`, `selected_by`, `modified_by`, and `approved_by`
remain distinct relations. DecisionFeedback is project-local memory by default;
external model training requires explicit opt-in outside this Core protocol.

## 8. Archive round trip

A Stage 0 export contains canonical object bytes, original Blob bytes, a Ref
snapshot, reflog, format version, and checksums. A conforming restore into an
empty store must:

1. recalculate every OID rather than trust filenames;
2. reject duplicate OIDs with different bytes;
3. restore objects before Refs;
4. re-run closure validation for each Ref;
5. reproduce the same Commit DAG and `present/tombstoned/missing` states.

Database indexes, search embeddings, previews, and graph projections may be
exported as caches but are never required to recover Core history.

## 9. Stable semantic error codes

Stage 0 fixtures reserve these codes: `invalid_utf8`, `bom_forbidden`,
`duplicate_key`, `number_token_forbidden`, `unsafe_integer`, `lone_surrogate`,
`key_not_nfc`, `identifier_not_nfc`, `set_not_sorted`, `set_duplicate`,
`timestamp_invalid`, `interval_invalid`, `fixed_point_not_normalized`,
`path_segment_invalid`, `schema_invalid`, `reference_type_mismatch`,
`oid_mismatch`, `closure_missing`, `stale_base`, `ref_conflict`, and
`resource_limit`. Implementations return `resource_limit` before allocation or
recursion would exceed a configured ingestion limit; this is an operational
rejection and does not make the object's canonical identity
deployment-specific.
