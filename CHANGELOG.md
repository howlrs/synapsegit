# Changelog

All notable user-visible changes are recorded here. SynapseGit uses semantic
version tags for release identification, but the Core protocol, OID profile,
and archive format remain Stage 0 drafts until explicitly declared stable.

## [Unreleased]

### Added

- `scripts/score_publication_comprehension.mjs` now validates
  `questionnaire.json`/`oracle.json`/`protocol.json` schema identity, that
  `protocol.json`'s `context.track_matrix` exactly matches the scorer's
  fixed evaluator-kind/track group order, that no question names a
  duplicate case or track, and that every case/track combination has at
  least one applicable question, all at corpus-load time. Oracle answer
  integers are now required to be safe integers
  (`Number.isSafeInteger`); response-side integer scoring is unchanged.
  Malformed inputs continue to fail via the existing
  `score_publication_comprehension_error:` stderr prefix and exit code 1 —
  no new machine-readable response error codes were introduced, and the
  frozen v1 corpus and score-report output are unaffected.
- `scripts/test_publication_comprehension_scorer.mjs` gained regression
  coverage for the frozen v1 corpus thresholds, corpus-load rejection of
  each new validation above, `evaluator_metadata`/`run_id`/`notes`
  boundary cases, unknown response/schema properties, and CLI exit-code
  behavior (`--help`, no arguments, an unreadable response file, malformed
  JSON, and a single valid response staying `not_run` with exit code 0).
- `crates/synapse-publication/examples/generate_evaluation_corpus.rs` now
  removes the output directory it created if generation fails partway
  through, instead of leaving a partial corpus behind. An existing output
  directory is still refused outright and is never touched by this
  cleanup.
- localhost browser: read-only archive listing (`GET /archives` plus a
  server-rendered dashboard section) with a new `--archive-root PATH`
  `synapse-local` startup flag. Listing bounded-scans the server-owned
  archive root's direct slug-named entries and reports each as
  `valid` (with its manifest checksum), `invalid`, or
  `staging_or_unknown`, from a manifest-level Core inspection that
  verifies the manifest checksum, structure, and per-object
  presence/length but does not read object content. Archive export and
  restore remain CLI-only.

## [0.5.1] - 2026-08-17

### Changed

- Runtime dependencies updated: serde 1.0.228 → 1.0.229 (serde_derive now
  builds with syn 3), serde_json 1.0.150 → 1.0.151, tokio 1.52.3 → 1.53.1,
  toml 1.1.3+spec-1.1.0 → 1.1.4+spec-1.1.0, and jsonschema 0.47.0 → 0.49.2
  (which introduces the jsonschema-value subcrate).
  `THIRD_PARTY_NOTICES.md` was regenerated for the updated dependency set,
  and the notices generator gained a fallback license entry for
  jsonschema-value, whose crates.io package omits its license file.
  No SynapseGit API or behavior changes are intended by these updates.
- CI and release workflows now run actions/checkout 7.0.1 and
  actions/attest 4.2.1 (SHA-pinned as before).

### Fixed

- Post-v0.5.0 documentation drift: the Japanese README trust-boundary
  paragraph is again a complete translation of the English one, literal
  `\"` artifacts in `docs/quickstart.md` and `docs/cli_reference.md` code
  blocks were removed, the `CONTRIBUTING.md` workspace map and
  `docs/runtime_architecture.md` mermaid diagrams now include
  synapse-local-http / synapse-local-service and the missing dependency
  edges, `SECURITY.md` now lists v0.5.x as supported, and stale v0.4.0
  version references (including the misattributed introduction version of
  localhost diagnostics and background fsck, which shipped in v0.3.0) were
  corrected across six documents.

## [0.5.0] - 2026-07-28

### Changed

- Publication bundle verification now validates canonical timestamps against
  the proleptic Gregorian calendar (previously it checked only the lexical
  wire form, so calendar-invalid values such as month 13 or February 30th
  could pass). Canonical timestamp parsing is now single-sourced in
  `synapse-canonical`, which `synapse-schema` and `synapse-core` also
  delegate to.
- `synapse-creator` and the `synapse` CLI now report the same `Io {
  operation, path, source }` shape as `synapse-core`, `synapse-cas`, and
  `synapse-publication`, so I/O failures from creator sessions and CLI
  commands carry richer operation context (for example, which file was
  being opened or inspected) instead of only a bare path and the OS error.
  Machine-readable error codes are unchanged. The pre-1.0 policy of keeping
  public Rust error enums exhaustive (no new `#[non_exhaustive]`) is now
  documented in `CONTRIBUTING.md`.
- Portable-path syntax validation (rejecting a leading `/`, a `\` byte, a
  `.`/`..`/empty path segment, and a NUL byte) is now single-sourced in
  `synapse-canonical`, which `synapse-artifact`, `synapse-publication`,
  `synapse-cas`, and `synapse-schema` all delegate to. Publication bundle
  path validation now also explicitly rejects an ASCII Windows drive-letter
  prefix (for example `C:`); such paths were already rejected end to end by
  an unrelated fixed-character-set check, but this makes the rejection an
  intentional, named rule rather than an incidental side effect. A NUL-byte
  bundle path is still rejected, but now fails with the explicit unsafe-path
  message instead of the unrelated fixed-character-set message it produced
  before. Machine-readable error codes are unchanged.

## [0.4.0] - 2026-07-20

### Added

- A provider-neutral `synapse-artifact` Rust boundary and frozen
  `synapsegit.generic-artifact` v1 application contract for bounded regular
  files. The mapper validates the complete manifest before its first CAS
  write, rejects non-regular entries and unsafe or colliding portable paths,
  builds deterministic nested ManifestTrees, and never updates a Ref. Staged
  source APIs support an initial Proposal and sequential Proposals from exact
  canonical Decision heads, retain prior attempts, enforce one active review,
  and reverify the selected accepted base before each new Proposal. Human
  Decisions require a host-authenticated, project-authorized, expiring one-shot
  approval bound to the exact actor, session, project epoch, Proposal, expected
  Decision head, disposition, and private-rationale bytes. Non-serializable
  pending authority and getter-only receipts omit repository paths, Refs/heads,
  Core OIDs, authority records, permits, and credentials. V1 accepts only
  caller-supplied AI-attributed bytes, always marks execution unverified, and
  cannot represent a trusted executor; verified execution requires a future
  negotiated contract version.
- A trusted `DurableProposalBinding` recovery registration for
  `synapse-application`, a separate `synapse-artifact-journal` SQLite store,
  and an explicit durable artifact orchestrator. The orchestrator records a
  private Proposal intent before CAS, allocates a public opaque `ReviewId` only
  after exact Proposal publication, records an exact Decision intent before
  CAS, and commits an outcome only after bounded live-state reconciliation and
  selected-site checkout. Restart recovery authenticates and authorizes before
  locator lookup, reconstructs fresh application authority without restoring
  credentials, approvals, registrations, handles, or permits, and distinguishes
  current from superseded Decisions without returning stale bytes. Final
  publication still passes through the complete `HumanDecisionRuntime`; the
  journal is neither authentication nor publication authority and is not
  atomic with the Core Ref store.
- A versioned generic-artifact public projection and deterministic local-only
  bundle with canonical JSON, escaped Markdown, script-free HTML, manifests,
  checksums, and Synapse/GitHub staging layouts. Complete projections require
  the bounded verified Decision checkout; pending and incomplete projections
  carry no repository or authority identifiers. The renderer performs no Git,
  network, upload, or credential operation. No HTTP/CLI/UI transport, model
  invocation, multi-process control plane, or production service is added.
- A frozen publication-comprehension corpus with separate complete
  adopt/reject/defer and incomplete-only bundles, a fixed questionnaire and
  semantic oracle, privacy canaries, response/protocol contracts, candidate
  generator, production-path bundle verification, and an executable exact
  scorer. The corpus records external Human, AI, and accessibility evaluation
  as `not_run` until those evaluations are actually completed.

### Changed

- Multi-session publication now shares one snapshot-scoped bounded `fsck` and
  one disposable ProjectionStore rebuild across all complete creator reports.
  Per-session lineage validation remains independent, while repository-wide
  verification work no longer grows linearly with the number of sessions.

## [0.3.0] - 2026-07-18

### Added

- A read-only `synapse-publication` presentation layer and `synapse-present`
  companion CLI for deterministic local publication bundles. The generated
  exports contain canonical JSON, escaped Markdown, JavaScript-free HTML,
  manifests, checksums, and target layouts for Synapse or GitHub without
  uploading or performing network operations. Private rationale, internal Actor
  IDs, repository paths, and raw assets remain omitted; separately supplied
  public presentation notes are labelled as author-supplied. Ref SQLite is
  captured from a checkpointed database of at most 512 MiB into a private
  temporary copy, with copy-time and post-copy source SHA-256 required to match;
  sidecars or concurrent source changes fail as `read_only_source_busy`. The CLI
  discovers at most 100 creator sessions, and remote upload and raw-asset
  rendering remain out of scope.
- A dedicated read-only localhost creator-session diagnostics service/API/UI
  that reports the current Ref/head shape and a safe recommended action without
  reconstructing review authority, resuming, cleaning up, or rewriting history.
- An explicitly confirmed localhost maintenance `fsck` using a server-fixed
  bounded Core profile, a finite process-local background-job registry and poll
  API, clean/dirty aggregate results, `last_fsck`, and project-page UI. Browser
  disconnect does not cancel or retry the job.

### Changed

- The default authorization clock now preserves a process-wide monotonic floor
  across wall-clock regressions, and creator recording uses the same trusted
  clock so freshly issued Grants cannot fail spuriously at startup.
- Documentation now covers the tagged browser diagnostics/`fsck`, CLI-only
  archive export/restore, and planned archive inspection/listing.
- Release packaging now includes `synapse-present`; the already-published
  v0.2.0 archive remains unchanged.

## [0.2.0] - 2026-07-15

### Added

- A concise English entry README and a matching Japanese README.
- Binary-first installation, distribution, project status, support, and
  security documentation.
- Pull request and Issue forms for actionable community feedback.
- Continuous integration for `main` and pull requests.
- Build-provenance attestation for tagged release archives.
- The custom SynapseGit Source-Available License 1.0, held by howlrs and
  K-Terashima, with explicit GitHub Fork and pull-request permissions.
- Generated third-party dependency notices for future release bundles.
- A two-step creator orchestration boundary that can retain the exact admitted
  proposal capability between proposal publication and Human review.
- A bounded localhost creator workflow for staging three caller-supplied files,
  retaining same-process review authority, and publishing Human `adopt`,
  `reject`, or `defer` decisions from the browser UI.

### Changed

- All workspace crates are explicitly excluded from crates.io publication
  while the Stage 0 API and distribution channels remain intentionally bounded.
- Public documentation now distinguishes current technical evaluators from
  the broader future creator audience.
- Stale private-repository and unimplemented-localhost statements were removed.
- The tagged-release workflow now installs the same pinned Node.js major used
  by CI before running protocol and documentation verification scripts.
- Creator begin, decision, and report now use operation-wide bounded fsck
  profiles for Ref roots, CAS objects/raw bytes, cumulative closure work, and
  Tombstone discovery.
- Publication-time closure validation now uses bounded prepared Tombstone
  catalogs. Creator begin reserves its graph and all eight localhost pending
  decisions' headroom,
  validates 64 MiB / 192 MiB input ceilings, and checks the exact prospective
  Ref state before publication; malformed OID references are charged to the
  cumulative edge budget.
- A committed creator decision whose full report cannot be rebuilt now returns
  its exact durable receipt as the HTTP 200 `committed` variant and releases
  the consumed review slot; publication is never retried.
- The localhost facade serializes creator mutations per project so concurrent
  blocking workers cannot race a prospective capacity check, and an empty Ref
  archive restore no longer scans an unused Tombstone inventory.
- Repository-owner merge and security settings now have a versioned,
  idempotent GitHub CLI manager and read-only drift check.
- Pinned GitHub Actions and direct Rust dependencies were refreshed together;
  schema validation and SHA-256 formatting were migrated without changing
  protocol OIDs, and bundled SQLite advanced to the newest release compatible
  with the workspace's Rust 1.88 policy.

## [0.1.0] - 2026-07-15

First Stage 0 preview.

### Added

- Strict canonical JSON and content-addressed Core objects.
- Filesystem object storage, SQLite Refs/reflog, `fsck`, directory export, and
  verified restore.
- A bounded three-file creator Pilot with AI-attributed proposal provenance,
  Human Decision recording, conservative byte-identity comparison, and a
  projection-backed report.
- A loopback-only, read-only local project and creator-session viewer.
- A Linux x86_64 GNU release archive containing `synapse` and `synapse-local`,
  with SHA-256 checksums.

### Known limits

- Stage 0 draft; no stable format or compatibility promise.
- No model invocation, pixel analysis, visual-difference judgment, real-user
  authentication, or production multi-user service.
- The original v0.1.0 archive was published without a bundled `LICENSE`. As of
  2026-07-15, the rights holders offer v0.1.0 under the current custom
  source-available license; the original archive remains unchanged.

[Unreleased]: https://github.com/howlrs/synapsegit/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/howlrs/synapsegit/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/howlrs/synapsegit/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/howlrs/synapsegit/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/howlrs/synapsegit/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/howlrs/synapsegit/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/howlrs/synapsegit/releases/tag/v0.1.0
