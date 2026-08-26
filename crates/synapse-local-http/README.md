# synapse-local-http

Axum/Askama transport and `synapse-local` binary for the single-user localhost
application. It binds only to `127.0.0.1`, embeds its templates/assets in the
binary, applies the localhost Host/Origin/token boundary, and depends on
`synapse-local-service` rather than Core storage primitives directly.

The current UI supports project/history navigation, creator-session reports,
authenticated image viewing, bounded three-file proposal upload, and the
same-process Human `adopt` / `reject` / `defer` review gate. It also provides a
dedicated authenticated GET diagnostics API and server-rendered read-only
Ref/head/recommended-action view for incomplete sessions. It does not reconstruct
review authority, resume, clean up, or rewrite history. Confirmed maintenance
`fsck` runs as a detached bounded job with a 256-entry / 64-active process-local
registry, pollable states, and project-page result display; dirty is a succeeded
result with `clean=false`. Bounded read-only archive listing (server-owned
archive root, dashboard section, and `GET /api/v1/archives`) is included in
the tagged v0.6.0 binary. Current `main` additionally exposes authenticated,
confirmed `POST /api/v1/projects/{project_key}/archive-exports` as a bounded
no-replace background job when the archive root is configured. Archive export
UI and archive restore UI/route are not yet implemented. The diagnostics and browser `fsck`
additions were introduced in v0.3.0 and remain unchanged in the tagged v0.6.0
binary. The generic-artifact libraries present in the tagged v0.6.0 source do
not add routes, DTOs, UI, or a new binary here.

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
