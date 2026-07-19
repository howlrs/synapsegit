# synapse-publication

`synapse-publication` is the read-only presentation layer for SynapseGit creator
history and generic-artifact Decisions, and provides the companion
`synapse-present` binary. The creator-history path opens the existing CAS
without creating or mutating it, captures the Ref database through a bounded
stable private copy, and generates a reviewable local `PublicationBundle` for
people and machines.

The bundle contains canonical `projection.json`, escaped `story.md`, a
JavaScript-free `index.html`, `manifest.json`, `checksums.json`, and a
target-specific directory. The default `synapse` target and the `github` target
share the same provider-neutral projection semantics. `--github` only prepares
local GitHub-ready files; neither export nor preview performs an upload, Git
operation, or network request.

Source-private Human rationale, internal Actor IDs, repository paths, and raw
asset bytes are omitted. A bounded regular `presentation.toml` may add title,
summary, captions, display names, and a public decision note; every such value
is marked `author_supplied`. Raw-asset rendering and thumbnails are not
implemented. Machine readability does not grant training permission, and
generated bundles declare training use prohibited.

Build and inspect the CLI with:

```bash
cargo build -p synapse-publication --bin synapse-present --locked
target/debug/synapse-present --help
```

Stop `synapse-local` and every writer for the source repository before export.
The source `refs.sqlite3` must be checkpointed and no larger than 512 MiB. The
opener never gives SQLite the source path: it copies the main database into a
private temporary file while computing SHA-256, hashes the source again after
the copy, and opens only the matching temporary copy. An existing
`refs.sqlite3-wal`／`refs.sqlite3-shm`／`refs.sqlite3-journal` or a
digest-changing concurrent write
fails closed with `read_only_source_busy`; an oversized database is refused.
The CLI discovers at most 100 creator sessions. The output
parent must be an existing real directory and the destination itself must not
exist or be inside the source repository. Export uses staged atomic no-replace
directory publication. `preview` verifies the fixed inventory, checksums,
schemas, and semantic links before printing the local HTML path.

When a snapshot contains multiple complete sessions, publication performs one
bounded snapshot `fsck` and one in-memory ProjectionStore rebuild, then reuses
that prepared read context for each independently validated session report.
Incomplete-only projections do not claim verified CAS lineage and do not build
the report context.

The repository includes a frozen
[publication-comprehension corpus](../../docs/evaluation/publication-comprehension/v1/)
with separate complete adopt/reject/defer and incomplete-only bundles. Its
production-path verifier, semantic oracle, and privacy canaries are automated;
Human, zero-context AI, axe, keyboard, zoom, and screen-reader evaluations
remain explicitly `not_run` until performed.

This crate does not change the SynapseGit Core protocol or the meaning of
`synapse export`, which remains the verified restorable Core archive command.
It does not publish to GitHub or a hosted Synapse service.

## Generic-artifact public projection v1

The generic-artifact API is a separate, explicitly dispatched profile. It does
not reuse or change the creator publication v1 schemas, renderer, bundle bytes,
or verifier.

- Profile: `org.synapsegit.generic-artifact-publication`, version `1`
- Projection: `org.synapsegit.generic-artifact-public-projection`, version `1`
- Renderer: `org.synapsegit.generic-artifact-publication-renderer`, version `1`
- Bundle: `org.synapsegit.generic-artifact-publication-bundle`, version `1`

`build_generic_artifact_complete_projection` accepts a
`TrustedArtifactDecisionBinding` and calls the artifact checkout boundary. A
complete projection is returned only after canonical Decision lineage, the
Human disposition, selected snapshot, every selected regular file, and the
application manifest digest verify. The public accepted-site binding contains
the manifest SHA-256, file count, and byte count, but no Core OID. Its identity
is byte-and-path identity only; it is not proof of authorship, rights, truth,
semantic equivalence, visual equivalence, or physical change.

Pending and incomplete projections require a non-serializable
`TrustedGenericArtifactStatus`. They contain no digest, OID, repository
locator, Decision receipt, selected site, or Human disposition and explicitly
identify their facts as bounded trusted display input rather than Synapse
authority.

The only application metadata accepted by this profile is a strict, bounded
`ReviewedPublicTargetV1` sidecar. It carries LP Studio product/API/schema
versions plus a reviewed target ID, kind, label, and accepted/proposal capture
source. It has no field for DOM paths or quotes, page paths, geometry, prompts,
provider responses, private rationale, raw site paths or bytes, repository
paths, or internal Actor, Policy, Grant, Ref, Commit, and Tree identifiers.

`export_generic_artifact_bundle` writes an atomic local bundle from an already
detached projection. Synapse and GitHub layouts reuse byte-identical canonical
`projection.json`, escaped `story.md`, and script-free `index.html`; only their
local `target/` copies differ. The API has no Git, credential, remote, or
network input and performs no such operation. The caller must select a new
destination outside every source repository; the detached public projection
intentionally contains no private repository path with which the export phase
could infer that boundary.

The schemas, semantic rules, and golden vectors are in
[`spec/application/generic-artifact-publication/v1`](../../spec/application/generic-artifact-publication/v1/).
Future remote publication, Git import, identity mapping, GitHub App, and hosted
service work is isolated in
[`docs/generic_artifact_publication_roadmap.md`](../../docs/generic_artifact_publication_roadmap.md).
