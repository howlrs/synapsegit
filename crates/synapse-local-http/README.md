# synapse-local-http

Axum/Askama transport and `synapse-local` binary for the single-user localhost
application. It binds only to `127.0.0.1`, embeds its templates/assets in the
binary, applies the localhost Host/Origin/token boundary, and depends on
`synapse-local-service` rather than Core storage primitives directly.

The current UI supports project/history navigation, creator-session reports,
authenticated image viewing, bounded three-file proposal upload, and the
same-process Human `adopt` / `reject` / `defer` review gate. `fsck`, archive
export, and archive restore UI/routes are not yet implemented.

Write forms require the embedded JavaScript module. Native HTML form submission
cannot attach the process-local custom token or normalize each multipart part
to the exact API content type, so a browser with JavaScript disabled remains a
read-only viewer. Uploads are limited to 64 MiB per file and 192 MiB in total;
at most two uploads may own staging space concurrently in one process.

Build and run instructions are in the
[native localhost runbook](../../deploy/local/README.md). Run crate tests with:

```bash
cargo test -p synapse-local-http --locked
```

The HTTP contract and security constraints are described by the
[application architecture](../../docs/localhost_application_architecture.md).
