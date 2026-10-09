# Public bundle contract

`synapse-present export` creates a local, read-only presentation bundle. It does not upload, run Git, or change the source repository. This document is the current creator-publication contract; it is separate from unresolved semantic-projection v2 work in [#163](https://github.com/howlrs/synapsegit/issues/163).

## Frozen v1 and explicit localized v2 containers

Without `--locale`, export writes the frozen creator bundle v1: the v1 `PublicProjection` semantic profile, v1 manifest/container schema, and existing English renderer bytes. Existing v1 bundles remain verifiable.

`--locale en` or `--locale ja` opts into a localized **container schema 2** and matching renderer profile version 2. It changes only rendered `story.md` and `index.html`; `projection.json` keeps PublicProjection semantic profile v1. A localized manifest carries the selected locale and matching v2 renderer identity. Verification rejects a missing, unknown, or mismatched locale/renderer identity. This is not semantic projection v2.

Localized renderer v2 is still unreleased development output. Its rendered views
may change before the first release, so a localized bundle exported from an
earlier development revision may need re-export before `preview` can verify it.
Released renderer profiles remain selected by their manifest identity.

## Public boundary

The bundle includes canonical projection facts, session identifiers, dispositions, selected-role facts, OIDs/Refs and checksums. Those identifiers may be correlatable. It excludes raw asset bytes, repository paths, internal Actor IDs, private generation notes, rationales, and decision pins.

`presentation.toml` can add author-supplied public text. It does not promote that text into verified source history. Review the generated bundle before any external copy.

## Replacing a destination

Export normally refuses an existing destination. Replacing an existing destination with `--replace` requires Linux or macOS. When its destination is absent, it publishes a new bundle through the same staged atomic no-replace path as ordinary export. If a file, directory, or symlink appears before publication, the operation fails without overwriting it. An existing destination must be a real strict bundle with the same checked parent. The exporter verifies its identity, stages and verifies the new bundle, then uses the platform atomic directory exchange (`renameat2(RENAME_EXCHANGE)` on Linux and `renameatx_np(RENAME_SWAP)` on macOS). The former bundle is then deleted only when it keeps the verified identity and still has exactly the strict bundle inventory: files are unlinked by name and directories removed non-recursively, so unexpected content is never deleted. Otherwise it is retained at the hidden staging/recovery path printed as `replacement_recovery_path` for human review and manual cleanup; remove it before `git add` when the bundle lives in a Git working tree. If the post-exchange directory sync fails, the new bundle is already visible; the command reports a sync warning and retains the recovery copy. A filesystem that does not support the exchange fails without changing the old bundle and removes the unpublished staging directory.

On platforms other than Linux and macOS, an absent destination uses the ordinary export path and succeeds only where atomic no-replace directory publication is supported. An existing destination is rejected before staging begins because it needs an atomic exchange. A platform without either required primitive reports an unsupported operation. Symlinks, a changed destination identity, unsafe parent paths, source descendants, and non-directory targets are rejected. Use a new destination where ordinary export is supported and replacement is unavailable or unsafe.

Run replacement with exclusive access to the destination and its parent. The final inventory and inode checks detect changes visible before exchange; they are not a filesystem compare-and-swap or a lock against another process running as the same user. Never edit either bundle during export. The exporter never deletes the exchanged directory recursively, and it retains it after a sync warning.

## Verify

Run `synapse-present preview BUNDLE`. It checks the fixed inventory, checksums, container schema, renderer/locale identity, and semantic links before showing the local HTML entry point.
