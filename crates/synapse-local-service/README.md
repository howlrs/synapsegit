# synapse-local-service

Transport-neutral, trusted facade for the single-user localhost application.
It owns the exact startup project catalog, bounded read operations, and the
versioned DTOs returned by transports without exposing repository paths or
low-level Ref/object mutation primitives. Creator writes are limited to the
catalog-selected begin/decide workflow.

The current service implements projects/status, Refs/reflog, creator-session
discovery/report/timeline/evidence, bounded verified image reads, proposal-only
creator import, same-process Human review, and a dedicated read-only creator-session
diagnostic DTO/method. Diagnostics return the current Ref/head shape and a safe
recommended action but never recover, clean up, or mutate a session. Pending
authority is opaque, non-serializable, capacity-bounded, and never reconstructed
from Ref/head IDs. The service also validates exact project confirmation, runs
only `fsck_with_limits` with a server-fixed Core-default-equivalent maintenance
profile, and retains the latest clean or dirty aggregate result in process-local
`last_fsck`. An optional server-owned archive root enables `list_archives`, a
bounded read-only listing of the root's direct entries with an operation-wide
object-count/declared-byte budget shared by every manifest-level inspection
(`valid`/`invalid`/`staging_or_unknown`); it is configured-empty by default and
never accepts a caller-supplied path. With that root configured, the service
also validates exact project confirmation and a logical archive slug, then
runs Core's server-fixed bounded atomic no-replace export for the maintenance
job transport. The same root enables exact target/empty confirmation followed
by Core's server-fixed bounded exact-subset restore with Ref publication last.
Browser archive controls, restart-durable
review, and automatic incomplete-session recovery are not implemented here.
The diagnostics and maintenance `fsck`
additions were introduced in v0.3.0 and remain unchanged in the tagged v0.6.0
binary. Archive listing was added on `main` after v0.5.1 and is included in
the tagged v0.6.0 binary. The generic-artifact workflow in the tagged v0.6.0
source is a separate Rust library boundary; this facade does not expose it
through HTTP/CLI/UI or remote publication.

Run its tests with:

```bash
cargo test -p synapse-local-service --locked
```

See the [native localhost runbook](../../deploy/local/README.md) and
[application architecture](../../docs/localhost_application_architecture.md).
