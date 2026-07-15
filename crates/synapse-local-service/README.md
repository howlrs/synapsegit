# synapse-local-service

Transport-neutral, trusted facade for the single-user localhost application.
It owns the exact startup project catalog, bounded read operations, and the
versioned DTOs returned by transports without exposing repository paths or
low-level Ref/object mutation primitives.

The current slice is read-only: projects/status, Refs/reflog, creator-session
discovery/report/timeline/evidence, and bounded verified image reads. Creator
upload, Human review, `fsck`, export, and restore service operations are not yet
implemented.

Run its tests with:

```bash
cargo test -p synapse-local-service --locked
```

See the [native localhost runbook](../../deploy/local/README.md) and
[application architecture](../../docs/localhost_application_architecture.md).
