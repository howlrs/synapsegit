# synapse-local-http

Axum/Askama transport and `synapse-local` binary for the single-user localhost
application. It binds only to `127.0.0.1`, embeds its templates/assets in the
binary, applies the localhost Host/Origin/token boundary, and depends on
`synapse-local-service` rather than Core storage primitives directly.

The current UI is read-only: project/history navigation and creator-session
report, timeline, evidence, and image viewing. Upload, Human review, `fsck`,
archive export, and archive restore UI/routes are not yet implemented.

Build and run instructions are in the
[native localhost runbook](../../deploy/local/README.md). Run crate tests with:

```bash
cargo test -p synapse-local-http --locked
```

The HTTP contract and security constraints are described by the
[application architecture](../../docs/localhost_application_architecture.md).
