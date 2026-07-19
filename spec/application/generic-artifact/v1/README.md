# SynapseGit generic artifact application contract v1

Status: frozen C1 application contract

This directory defines the public-safe JSON boundary for a trusted application
that maps a bounded regular-file artifact, publishes it for Human review, and
queries the resulting review state. It is an application contract, not a new
SynapseGit Core protocol or a browser-facing authority API.

## Negotiation

Every payload carries the exact pair:

```json
{
  "contract": "synapsegit.generic-artifact",
  "contract_version": 1
}
```

A consumer must obtain and validate `capabilities.json` before using the
contract. Version `1` is an exact contract: unknown versions, properties, enum
members, and attribution modes fail closed. A future incompatible surface uses
a different integer version and schema path rather than changing v1 in place.

The frozen capability order is significant for golden comparison. It exposes
only regular files, the three initial Human dispositions, and caller-supplied
AI-attributed bytes whose execution is not verified. The mapper itself does not
update Refs.

The `relative-nfc-portable-v1` path profile requires non-empty relative NFC
paths separated by `/`. It rejects empty, dot, traversal, mixed-separator,
duplicate, Unicode-lowercase, file/directory, Windows reserved-name (including
`COM¹`/`COM²`/`COM³` and `LPT¹`/`LPT²`/`LPT³`), reserved-character,
trailing-dot/space, and bidi-control collisions. Hosts may
apply a stricter application profile but must not weaken these checks.

## Public payloads

| File | Role |
| --- | --- |
| `capabilities.schema.json` | Exact v1 capability negotiation response |
| `proposal-receipt.schema.json` | Proposal successfully published and awaiting review |
| `review-status.schema.json` | Durable review/outcome query result |
| `public-error.schema.json` | Safe failure returned by the application boundary |
| `digest-vectors.json` | Frozen manifest/context digest inputs and outputs |
| `scaled-integer-vectors-v1.json` | Supplemental exact-decimal normalization and context-digest vectors |
| `fixtures/` | Positive frozen instances used by upstream and downstream contract tests |

`review_id` is an opaque, randomly generated 256-bit locator encoded as exactly
64 lowercase hexadecimal characters. It is not a Ref, Commit OID, permit,
credential, or portable authority. Requests must still be authenticated and
project-routed before a service resolves it. Malformed, unknown, and forbidden
identifiers must not become a project or review oracle.
Publishing a Human Decision additionally requires host authentication and
project authorization before the trusted workflow is invoked. Passing a
public review request or `review_id` to `decide_artifact_proposal` is not
authentication. The Rust boundary requires an opaque, expiring one-shot
approval issued by an embedding-host `Authenticator` and server-owned project
ACL. It binds the exact actor/session, ACL epoch, Proposal/Decision heads,
disposition, and rationale presence/bytes, and is consumed before Decision
object or Ref mutation. Credentials and approvals never appear in this public
contract or the durable journal.

The two SHA-256 fields bind application-owned canonical bytes. Exact golden
inputs and outputs are fixed in `digest-vectors.json`:

- `artifact_manifest_sha256` hashes the reviewed canonical regular-file
  manifest, not a Synapse Tree OID. Start SHA-256 with the ASCII domain
  `synapsegit-generic-artifact-manifest-v1` followed by one NUL byte. Visit
  validated files in ascending normalized-path UTF-8 byte order. For each file,
  append the path byte length as unsigned 64-bit big-endian, the path UTF-8
  bytes, the content length as unsigned 64-bit big-endian, and the raw content
  bytes. Encode the final digest as 64 lowercase hexadecimal characters;
- `review_context_sha256` hashes the reviewed, redacted application context,
  not a prompt, credential, provider response, or internal authority object.
  Strictly parse the JSON using the Synapse canonical boundary, serialize those
  canonical JSON bytes, hash the bytes directly with SHA-256, and encode 64
  lowercase hexadecimal characters.

These digests establish byte bindings only. They do not prove authorship,
rights, truth, semantic correctness, visual correctness, or physical change.

## Exact numeric application context

JSON fraction and exponent tokens are outside the Synapse canonical numeric
domain. Values such as dimensions and confidence ratios should be converted
from their original decimal strings to the Core v0.1 `ScaledInteger` shape,
whose value is `mantissa * 10^scale`. For example, `1440.0 px` normalizes to
`{"mantissa":"144","scale":1,"unit":"px"}`, while `0.9200` normalizes to
`{"mantissa":"92","scale":-2,"unit":"ratio"}`.

```json
{
  "geometry": {
    "css_width": { "mantissa": "144", "scale": 1, "unit": "px" }
  },
  "confidence": { "mantissa": "92", "scale": -2, "unit": "ratio" }
}
```

Conversion must operate on the exact decimal string and must not pass through
binary floating point. `scaled-integer-vectors-v1.json` fixes successful
normalization, stable rejection reasons, and the resulting canonical review
context digest for equivalent decimal spellings. This new vector is frozen as
`scaled-integer-decimal-v1`; the existing frozen `digest-vectors.json` file is
unchanged.

For every `equivalent` case, `review_context_input` freezes the exact canonical
UTF-8 bytes hashed by `review_context_sha256`. The conformance wrapper is
exactly `{"measurement":<scaled>}` with no extra members or whitespace, where
`<scaled>` is that case's normalized object. Each listed decimal spelling must
produce the same normalized object, the same wrapper bytes, and the recorded
digest. This single-key wrapper belongs to the
`scaled-integer-decimal-v1` vector profile only; an application may define a
different reviewed-context schema, but it must freeze and hash its own exact
canonical JSON bytes.

## Attribution boundary

`caller_supplied_ai_attributed` always has `execution_verified: false`. It means
trusted application code supplied bytes and attributed them to an AI workflow;
SynapseGit did not execute or verify the model invocation. A verified trusted
executor mode is outside v1 and requires a future negotiated contract version.
An implementation must never upgrade caller-supplied bytes based on a request
field.

## Review states

`pending_review` may progress to `decision_committed`. `terminal_denial` is a
closed failure. `retryable_failure` may be retried only according to the
server-owned durable intent. `outcome_unknown` must be reconciled by querying
trusted local state and must not trigger a blind Decision replay.

The local durable Rust orchestration writes a private Proposal intent before
Proposal CAS and exposes a `review_id` only after verifying exact publication.
It writes an exact Decision intent before Decision CAS and commits a terminal
outcome only after live Ref/reflog reconciliation and bounded selected-site
checkout. After restart it authenticates again and reconstructs fresh
process-local authority from trusted configuration plus the journal and
immutable graph; it never deserializes an old credential, admitted handle,
approval, registration, or permit. This is an implementation of the frozen
state semantics, not a new field or authority mechanism in these JSON schemas.

For a committed Decision, `adopted_unchanged` selects the exact proposal
snapshot. `rejected` and `deferred` retain the exact base snapshot. Modified,
partial, experiment-only, and caller-defined dispositions are outside v1.

Public payloads deliberately omit repository paths, Ref names or heads, all
Core OIDs, Actor/Policy/Grant identifiers, permits, credentials, raw prompts,
provider responses, and private rationale.

## Local validation

From the repository root:

```bash
jq empty spec/application/generic-artifact/v1/*.json \
  spec/application/generic-artifact/v1/fixtures/*.json
jsonschema \
  -i spec/application/generic-artifact/v1/capabilities.json \
  spec/application/generic-artifact/v1/capabilities.schema.json
```

The remaining fixtures are validated against their correspondingly named
schemas by the contract test suite.
