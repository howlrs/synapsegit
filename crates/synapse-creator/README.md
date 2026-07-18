# synapse-creator

`synapse-creator` is the local, single-creator Stage 0 Pilot orchestration
layer. It accepts an original image, a current image, a caller-supplied AI
output, and one human `adopt` / `reject` / `defer` outcome. It creates the
Subject, imported CaptureProfile, Observations, Activities,
policy/grant/context, Trees, and Commits without caller-authored JSON. Both
Observations reference the same `imported` profile, whose only allowed claim is
`reference_only`; it does not imply repeatable or calibrated capture.

The creator base snapshot also records a deterministic `byte_identity`
AnalysisResult for the ordered original/current Observations. A dedicated
`software_tool` Actor, distinct from the AI Actor, asserts the result. The
adapter verifies every referenced media Blob and compares only the two primary
Blob OIDs. A successful comparison remains `partial` with reason
`byte_identity_only`; it does not decode pixels or EXIF, register viewpoints,
or infer visual or physical change.

AI publication passes through `synapse-application`'s authenticated one-shot
AI route. `begin_creator_session` returns an opaque, non-Clone,
non-serializable pending value that retains the exact `Application` instance
and its admitted proposal handle. `decide_creator_session` borrows that value
to publish `adopt`, `reject`, or `defer` through the narrow Human Decision
route. Persisted Ref/head identifiers cannot recreate this process-local
authority. The existing `run_creator_session` API and `creator-run` CLI remain
compatibility wrappers that invoke both phases consecutively. `creator-report` rebuilds a disposable
ProjectionStore timeline from one captured Ref snapshot and independently
checks the current DecisionFeedback, proposal transition, decision snapshot,
and actor bindings. It also validates the AnalysisResult's ordered inputs,
implementation/configuration evidence, dedicated tool Actor, replay
prerequisites, and reachability from both creator Refs. The report distinguishes
the local agent identity to which the proposal is attributed, the
caller-supplied bytes, and the human reviewer; it is an audit view, not an
authorization source. A legacy-shaped session whose base Tree has no comparison
evidence entries remains reportable with `comparison=unavailable`; this shape
does not prove when the session was created.

Embeddings that need several reports from the same Ref snapshot can prepare one
opaque `PreparedCreatorReportReader`. Preparation performs the bounded snapshot
`fsck` and one disposable ProjectionStore rebuild. Every `report` reuses those
fixed results while still validating that session's Ref pair in the supplied snapshot, lineage,
evidence, and actor bindings. The existing single-report APIs delegate through
the same boundary while preserving their established error order.

Creator begin, Human decision, and report use the bounded Core fsck boundary.
One operation is limited to 10,000 Ref roots, 25,000 complete-inventory CAS
objects, 4 GiB of inventoried raw bytes, 250,000 cumulative closure nodes,
2,500,000 cumulative closure edges, and a 25,000 Record / 512 MiB Tombstone
scan. These values are fixed by the trusted creator integration and cannot be
raised by HTTP input. Each input Blob is also limited to 64 MiB and the three
inputs to 192 MiB in aggregate. Begin reserves its fixed graph growth plus all
eight localhost pending-review decisions, then verifies the exact prospective two-Ref snapshot before
publication. Decision performs the corresponding admission and prospective
snapshot checks before its compare-and-swap. Publication-time head validation
uses the same bounded Tombstone profile instead of the legacy unbounded
inventory scan. A pre-publication limit failure leaves Refs unchanged. If a
committed decision cannot be rebuilt into the full HTTP report after a
concurrent repository change, the service returns its exact durable receipt as
the `committed` success variant, releases the consumed review slot, and never
retries publication.

The prospective capacity check assumes cooperative serialization of creator
mutations for one repository. `synapse-local-service` enforces that assumption
with one process-local writer gate per catalog project across both begin and
decision. Direct crate embeddings must provide the same serialization and must
not run an independent Repository writer concurrently; this is not a
cross-process filesystem lock.

This crate is not a model runner, image decoder, pixel registration/diff adapter,
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

Core directory export/restore preserves the reachable comparison evidence, and
the restored creator report is checked against the same snapshot-bound lineage.
Projection replay readiness only means the recorded prerequisites are present;
it does not promise exact replay. Workstream C pixel registration and visual
difference analysis remain unimplemented.

Creator sessions are create-only; only their `decision/creator/*` and
`proposal/creator-agent/*` Ref names are derived from the session name. A
process failure or loss of the opaque pending value after proposal publication
leaves an incomplete session; a later failure can leave an already
complete session that must be inspected or restarted under a new session name.
Stage 0 does not implement a cross-Ref workflow transaction, resume, or
automatic cleanup.
