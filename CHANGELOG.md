# Changelog

All notable user-visible changes are recorded here. SynapseGit uses semantic
version tags for release identification, but the Core protocol, OID profile,
and archive format remain Stage 0 drafts until explicitly declared stable.

## [Unreleased]

### Added

- A concise English entry README and a matching Japanese README.
- Binary-first installation, distribution, project status, support, and
  security documentation.
- Pull request and Issue forms for actionable community feedback.
- Continuous integration for `main` and pull requests.
- Build-provenance attestation for future tagged release archives.
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

[Unreleased]: https://github.com/howlrs/synapsegit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/howlrs/synapsegit/releases/tag/v0.1.0
