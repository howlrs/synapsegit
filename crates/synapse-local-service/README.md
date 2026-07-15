# synapse-local-service

Transport-neutral, trusted facade for the single-user localhost application.
It owns the exact startup project catalog, bounded read operations, and the
versioned DTOs returned by transports without exposing repository paths or
low-level Ref/object mutation primitives. Creator writes are limited to the
catalog-selected begin/decide workflow.

The current service implements projects/status, Refs/reflog, creator-session
discovery/report/timeline/evidence, bounded verified image reads, proposal-only
creator import, and same-process Human review. Pending authority is opaque,
non-serializable, capacity-bounded, and never reconstructed from Ref/head IDs.
`fsck`, export, restore, restart-durable review, and automatic incomplete-session
recovery are not implemented here.

Run its tests with:

```bash
cargo test -p synapse-local-service --locked
```

See the [native localhost runbook](../../deploy/local/README.md) and
[application architecture](../../docs/localhost_application_architecture.md).
