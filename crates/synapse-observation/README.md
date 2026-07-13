# synapse-observation

`synapse-observation` contains conservative, first-party Observation analysis
adapters. The initial `byte_identity` adapter validates two ordered Observation
Records and every referenced media Blob, records whether the two primary
content-addressed Blob OIDs are equal, and emits a schema-valid AnalysisResult.

This adapter does not decode media, inspect EXIF, register viewpoints, compare
pixels, or infer visual or physical change. Its result is always `partial` with
the `byte_identity_only` reason when comparison succeeds. Equal bytes do not
prove that a physical subject was unchanged; different bytes do not prove that
it changed.

The adapter writes immutable implementation/configuration Blobs and the
AnalysisResult to CAS, but never updates a Ref. The caller decides whether and
how to bind the result into a reachable snapshot. The implementation digest is
the Blob OID of a deterministic bundle containing the semantic Rust sources and
crate manifest compiled into this crate. It still does not capture Cargo.lock,
transitive dependency sources, the compiler, target, or runtime environment.

Before writing, it verifies the two Observation Records, referenced
CaptureProfiles, and every media Blob. Optional non-media dependencies of an
Observation remain the Ref-publishing caller's closure-validation
responsibility.
