# synapse-creator

`synapse-creator` is the local, single-creator Stage 0 Pilot orchestration
layer. It accepts an original image, a current image, a caller-supplied AI
output, and one human `adopt` / `reject` / `defer` outcome. It creates the
Subject, Observations, Activities, policy/grant/context, Trees, and Commits
without caller-authored JSON.

AI publication passes through `synapse-application`'s authenticated one-shot
AI route. The resulting same-instance admitted proposal handle is then passed
through its narrow Human Decision route. `creator-report` rebuilds a disposable
ProjectionStore timeline from one captured Ref snapshot and independently
checks the current DecisionFeedback, proposal transition, decision snapshot,
and actor bindings. The report distinguishes the local agent identity to which
the proposal is attributed, the caller-supplied bytes, and the human reviewer;
it is an audit view, not an authorization source.

This crate is not a model runner, image decoder, registration/diff adapter,
HTTP service, durable authorization service, or production credential store.
All three image files remain opaque immutable Blobs. Imported images do not
claim a capture instant: generated Observations use an explicit unknown
`capture_time`, because filesystem import time is not evidence of capture time.
Activities likewise leave their before/after Observation lists empty instead
of claiming an image-to-image causal relation. Timeline ordering uses strictly
monotonic `recorded_at` values as an explicit fallback, not as capture or model
execution time. Stage 0 fixes Subject kind to `hybrid` and data classification
to `internal`.

Each run creates OS-CSPRNG-backed, session-local EntityIds and stores their
manifest in the reachable Subject extension. They do not prove creator or
Subject continuity across sessions. Human feedback defaults to reason code
`unspecified`, private visibility, and prohibited training use.

Creator sessions are create-only; only their `decision/creator/*` and
`proposal/creator-agent/*` Ref names are derived from the session name. A
process failure after Ref publication can leave an incomplete or already
complete session that must be inspected or restarted under a new session name.
Stage 0 does not implement a cross-Ref workflow transaction, resume, or
automatic cleanup.
