# Changelog

All notable user-visible changes are recorded here. SynapseGit uses semantic
version tags for release identification, but the Core protocol, OID profile,
and archive format remain Stage 0 drafts until explicitly declared stable.

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/howlrs/synapsegit/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/howlrs/synapsegit/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/howlrs/synapsegit/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/howlrs/synapsegit/releases/tag/v0.1.0
