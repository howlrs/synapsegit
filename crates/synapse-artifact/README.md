# synapse-artifact

`synapse-artifact` is the provider-neutral regular-file mapping and bounded
same-process review boundary for applications that store an artifact tree in
SynapseGit Core.

The crate accepts in-memory manifests selected by a trusted server. It does not
open caller-selected host paths or follow symlinks. All manifest validation
completes before the first CAS write.

The v1 mapper:

- accepts regular-file entries only;
- requires relative NFC paths with portable `/` separators;
- rejects traversal, duplicate, lowercase, file/directory, Windows reserved-name,
  reserved-character, trailing-dot/space, and bidi-control conflicts;
- enforces file, byte, depth, and path limits before CAS mutation; and
- maps the same normalized paths and bytes to the same nested ManifestTree OID.

`begin_artifact_proposal` and `decide_artifact_proposal` additionally provide a
one-empty-repository, one-Proposal workflow. The trusted configuration owns the
repository path and authority metadata; proposal and Decision publication pass
through `synapse-application`, `CreativeAiRuntime`, and
`HumanDecisionRuntime`. The returned pending value is non-serializable,
same-process, and one-shot. Its durable binding is intended only for trusted
server journaling and recovery registration.

This crate does not invoke a model, provide HTTP/CLI/UI transport, integrate the
durable journal with recovery, or resume a workflow after restart. The v1 type
and same-process workflow accept only caller-supplied AI-attributed bytes and
cannot represent verified execution. A future application-integrated executor
requires a separately negotiated contract version.

Invoking `decide_artifact_proposal` is itself trusted-process authority. The
function does not authenticate an operating-system or browser user and must not
be connected directly to an HTTP request. An embedding host must authenticate
and authorize the creator before lookup and before calling it. A mandatory
host-authenticated one-shot approval boundary is tracked in
[issue #24](https://github.com/howlrs/synapsegit/issues/24).

`ArtifactSourceAttribution::CallerSuppliedAiAttributed` must never be presented
as verified model execution. The mapper itself makes no claim about how bytes
were produced and never updates a Ref.
