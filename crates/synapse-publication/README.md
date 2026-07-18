# synapse-publication

`synapse-publication` is the read-only presentation layer for SynapseGit creator
history and provides the companion `synapse-present` binary. It opens the
existing CAS without creating or mutating it, captures the Ref database through
a bounded stable private copy, and generates a reviewable local
`PublicationBundle` for people and machines.

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

This crate does not change the SynapseGit Core protocol or the meaning of
`synapse export`, which remains the verified restorable Core archive command.
It does not publish to GitHub or a hosted Synapse service.
