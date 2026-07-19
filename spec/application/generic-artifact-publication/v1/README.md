# Generic-artifact public publication profile v1

This directory freezes the provider-neutral, local-only public projection of a
SynapseGit generic-artifact review outcome. It is separate from the creator
publication v1 profile. A conforming implementation must dispatch on both the
profile name and version and must fail closed for unknown versions.

## Contract identities

| Contract | Name | Version |
| --- | --- | ---: |
| Publication profile | `org.synapsegit.generic-artifact-publication` | 1 |
| Projection schema | `org.synapsegit.generic-artifact-public-projection` | 1 |
| Renderer profile | `org.synapsegit.generic-artifact-publication-renderer` | 1 |
| Bundle manifest schema | `org.synapsegit.generic-artifact-publication-bundle` | 1 |
| Checksums schema | `org.synapsegit.generic-artifact-publication-checksums` | 1 |
| LP Studio sidecar | `org.synapsegit.lp-studio.public-target` | 1 |

The sidecar also freezes LP Studio API `v1`, API schema `1`, Target schema `1`,
and carries the concrete LP Studio product version supplied by the caller.

The generator name is fixed to `synapse-publication`. Its version is bounded
ASCII metadata matching `^[A-Za-z0-9._+-]+$`, not a renderer dispatch key. A
verifier requires the generator identity in `manifest.json` and
`projection.json` to match, while the exact publication and renderer profile
name/version pairs select the v1 validator and deterministic renderer. This
keeps bundles from an older safe generator version verifiable without treating
an unknown renderer profile as compatible.

`golden-vectors.json` records the exact `synapse-publication` `0.3.0`
generator identity that emitted the frozen v1 bytes. Later package releases
retain those vectors and digests as a compatibility fixture while separately
checking that newly built projections and manifests carry the current package
version. The pinned generator metadata is fixture provenance, not a renderer
dispatch key and not permission to relabel a newly generated bundle.

## Inputs and trust

A complete projection can be built only from a
`TrustedArtifactDecisionBinding` accepted by
`checkout_artifact_decision`. Checkout validates the canonical Decision
lineage and reconstructs and reads the complete selected site before returning
the Human disposition, selected snapshot, application manifest digest, file
count, and byte count used by the projection. The source repository is opened
read-only.

Pending and incomplete outcomes accept only the bounded,
non-serializable `TrustedGenericArtifactStatus`. These outcomes cannot contain
a Human disposition, selected snapshot, accepted-site binding, digest, OID, or
portable authority. They state that their origin is trusted display input, not
a verified Synapse Decision.

The sole application sidecar is the strict, bounded
`ReviewedPublicTargetV1` described by `public-target.schema.json`. Its allowlist
is limited to schema and LP Studio contract identities, target ID, kind, public
label, and accepted/proposal capture source. The product version and target ID
are bounded ASCII labels. The public label is limited to 300 Unicode scalar
values; control and bidirectional-control characters are rejected. The entire
encoded sidecar remains limited to 16 KiB. Unknown fields fail closed.

## Disclosure boundary

The v1 Rust types and JSON schemas provide no field for:

- DOM anchors, selectors, text quotes, geometry, or page paths;
- prompts, provider responses, or private Human rationale;
- raw selected-site paths or bytes;
- repository paths or credentials; or
- internal Actor, Policy, Grant, Ref, Commit, Tree, or Record identifiers.

The accepted-site manifest SHA-256 binds normalized regular-file paths and
bytes under `synapsegit-generic-artifact-manifest-v1`. A public Core OID is not
included. The digest does not establish authorship, rights, truth, semantic or
visual equivalence, or a physical change.

Attribution is fixed to `caller_supplied_ai_attributed` with
`execution_verified=false`. It records the frozen generic-artifact contract,
not a model invocation performed by SynapseGit.

## Canonical files and local layouts

Every JSON file uses Synapse canonical JSON. Human views are deterministic:
Markdown escapes active Markdown delimiters, and HTML escapes caller text and
contains no script. The common root inventory is:

- `projection.json`
- `story.md`
- `index.html`
- `manifest.json`
- `checksums.json`

The Synapse layout additionally has
`target/generic-artifact-public-projection.json`. The GitHub layout additionally
has `target/README.md`, `target/index.html`, and `target/projection.json`. These
are local staging layouts only; generation performs zero Git and zero network
operations. The provider-neutral root projection and views are byte-identical
across targets.

`checksums.json` covers every file except itself. Verification requires the
exact inventory, canonical encodings, checksums and lengths, profile and schema
identities, manifest/projection correlations, semantic outcome correlations,
deterministic re-rendered views, and exact target copies.

## Outcome correlations

- `complete`: origin is `verified_from_synapse`; disposition and selected
  snapshot are present; adopted selects proposal, while rejected and deferred
  select base; accepted-site binding is present; status reason is absent.
- `pending`: origin is `bounded_trusted_status_input`; reason is
  `pending_review`; Decision and selected-site fields are absent.
- `incomplete`: origin is `bounded_trusted_status_input`; reason is one of
  `retryable_failure`, `outcome_unknown`, or `terminal_denial`; Decision and
  selected-site fields are absent.

JSON Schema validates representation. The Rust semantic validator additionally
enforces the correlations and fixed safety statements above.

## Fixtures

`golden-vectors.json` freezes SHA-256 values for canonical `projection.json`,
escaped `story.md`, and script-free `index.html` for complete, pending, and
incomplete outcomes. Any intentional byte change requires a new version or a
documented compatibility decision; it must not silently rewrite the existing
creator publication v1 corpus. Keeping the original `0.3.0` generator identity
while validating later generators independently is that documented
compatibility decision.
