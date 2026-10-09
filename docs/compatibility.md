# SynapseGit compatibility policy

Audience: creators who keep records with SynapseGit, tool builders, and
maintainers.
Applies to: the v1 formats, from SynapseGit v1.0.0 and throughout v1.x.
Decided in: [#147](https://github.com/howlrs/synapsegit/issues/147), implemented in
[#165](https://github.com/howlrs/synapsegit/issues/165).

SynapseGit makes two promises for v1.x:

1. **Read compatibility.** A v1.x binary reads every repository, directory
   archive, and Creator session written by a published release from v0.1.0
   up to and including that binary's own version.
2. **Frozen v1 formats.** The formats below keep their meaning and bytes
   throughout v1.x. A change gets a new identifier, and the old format stays
   readable.

## What is frozen

| Surface | Identifier | Commitment | When a change is needed |
|---|---|---|---|
| Core OID profile | `sg-oid-v1` | Frozen | Add a new profile identifier. Existing OIDs are never recomputed. |
| Core record schemas | [`spec/core/v0.1/schemas`](../spec/core/v0.1/schemas/) | Frozen; new record types and namespaced `extensions` may be added | Keep existing records' meaning; add a record type or an extension. |
| Directory archive | `synapsegit-core-archive-v0.1` ([profile](../spec/core/v0.1/archive-profile.md)) | Frozen | Add a new archive profile and keep restoring the old one. |
| Import inbox manifest | `synapsegit-import-inbox-v1` ([spec](../spec/application/import-inbox/v1/README.md)) | Frozen | Add a new version identifier and keep accepting v1. |
| Private report JSON | `synapsegit-cli-creator-report-v1` | Frozen with additive fields | Add fields within `-v1`; removal, renaming, or a meaning change needs `-v2`. |
| CLI list, inbox, and decision JSON | `synapsegit-cli-creator-list-v1`, `synapsegit-cli-inbox-put-v1`, `synapsegit-cli-creator-decision-v1` | Frozen with additive fields | As above. |
| Creator publication bundle | PublicProjection semantic profile v1; no-flag container/renderer v1 | Frozen | `--locale` uses a distinct container/renderer v2 while retaining semantic profile v1; keep verifying v1 bundles. |
| Generic artifact contracts | `generic-artifact` v1, `generic-artifact-publication` v1 | Frozen | As above. |
| CLI | command names, arguments, exit codes, machine-readable error codes | Stable | Deprecate for at least one minor version before removing or changing. Text output wording may change; use JSON for automation. |

The Core protocol keeps its directory and identifiers (`spec/core/v0.1`,
`sg-oid-v1`, `synapsegit-core-archive-v0.1`); renaming them would change bytes
that existing objects and archives carry.

## What is not promised

An older binary may refuse a Creator session recorded by a newer release
with `creator_implementation_unrecognized`. Open it with the recording release
or a later release. The frozen formats and the promise to read earlier releases
do not require an older binary to recognize a future implementation bundle.

- The localhost HTTP API (`/api/v1`, `info.version` with `-draft`) is the
  browser UI's internal contract.
- Rust crate APIs are not published to crates.io and may change.
- Text output is for people. Its lines and wording may change.

## How it is verified

- **OID freeze basis.** The Rust implementation and the independent
  JavaScript verifier (`scripts/verify_core_fixtures.mjs`) agree on every
  golden fixture's OID, canonical length, and canonical SHA-256. A second
  independent production implementation remains Stage 1 research, not a
  condition of the freeze.
- **Every published release.** `crates/synapse-cli/tests/fixtures/releases`
  holds a repository and an archive written by each published release's own
  binary. CI reads each repository (`fsck`, `creator-report`, `creator-list`),
  restores each archive, and requires identical reports.
  `scripts/verify_release_fixtures.mjs` fails until every annotated release
  tag below the current version has a fixture.
- **Recorded byte-identity bundles.** `scripts/verify_byte_identity_allowlist.mjs`
  requires every published release's byte-identity implementation in the
  reader allowlist.
- **Archive round trips.** `scripts/verify_archive_compatibility.sh` builds
  the pinned v0.11.1 source and the latest earlier normal release and checks
  archive round trips with the current binary.

Release candidates (`vX.Y.Z-rc.N`) are published releases for these checks:
sessions recorded by a candidate stay readable. See the
[distribution guide](./distribution.md) for the release procedure.
