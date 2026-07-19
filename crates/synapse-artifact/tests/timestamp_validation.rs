use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use synapse_artifact::{
    ArtifactLimits, ArtifactManifestEntry, ArtifactSourceAttribution, RegularFileManifest,
    TrustedArtifactProjectConfig, begin_artifact_proposal,
};
use synapse_schema::CanonicalTimestamp;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct UnopenedProject {
    path: PathBuf,
}

impl UnopenedProject {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self {
            path: std::env::temp_dir().join(format!(
                "synapse-artifact-timestamp-{label}-{}-{nanos}-{serial}",
                std::process::id()
            )),
        }
    }
}

impl Drop for UnopenedProject {
    fn drop(&mut self) {
        if self.path.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn manifest() -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [ArtifactManifestEntry::regular_file(
            "index.html",
            b"ok".to_vec(),
        )],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn assert_rejected_without_repository(recorded_at: &str, expires_at: &str, label: &str) {
    let project = UnopenedProject::new(label);
    let config = TrustedArtifactProjectConfig::new(
        &project.path,
        format!("timestamp-{label}"),
        "Creator",
        "Agent",
        recorded_at,
        expires_at,
    );
    let error = begin_artifact_proposal(
        &config,
        &manifest(),
        &manifest(),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();

    assert_eq!(error.code(), "invalid_argument", "{label}: {error}");
    assert!(!project.path.exists(), "{label}: repository was created");
    assert!(!error.to_string().contains(recorded_at));
    assert!(!error.to_string().contains(expires_at));
}

#[test]
fn timestamp_validation_precedes_repository_open_and_cas_mutation() {
    let valid_recorded = "2026-07-20T00:00:00.000000000Z";
    let valid_expiry = "2026-07-20T00:01:00.000000000Z";
    for (recorded_at, expires_at, label) in [
        ("2026-07-20T00:00:00Z", valid_expiry, "missing-fraction"),
        (
            "2026-07-20T00:00:00.12000000Z",
            valid_expiry,
            "trimmed-fraction",
        ),
        (
            "2026-02-30T00:00:00.000000000Z",
            valid_expiry,
            "invalid-date",
        ),
        (valid_recorded, "2026-07-20T00:01:00Z", "invalid-expiry"),
        (valid_expiry, valid_recorded, "expiry-before-recorded"),
    ] {
        assert_rejected_without_repository(recorded_at, expires_at, label);
    }
}

#[test]
fn raw_timestamp_try_new_rejects_at_configuration_construction() {
    let project = UnopenedProject::new("try-new");
    let error = TrustedArtifactProjectConfig::try_new(
        &project.path,
        "timestamp-try-new",
        "Creator",
        "Agent",
        "2026-07-20T00:00:00Z",
        "2026-07-20T00:01:00.000000000Z",
    )
    .unwrap_err();

    assert_eq!(error.code(), "invalid_argument");
    assert!(!project.path.exists());
}

#[test]
fn typed_timestamp_constructor_preserves_byte_identical_protocol_records() {
    let raw_project = UnopenedProject::new("raw-constructor");
    let typed_project = UnopenedProject::new("typed-constructor");
    let recorded_at = "2026-07-19T00:00:00.120000000Z";
    let expires_at = "2099-01-01T00:00:00.120000000Z";
    let raw = TrustedArtifactProjectConfig::new(
        &raw_project.path,
        "typed-timestamp",
        "Creator",
        "Agent",
        recorded_at,
        expires_at,
    );
    let typed = TrustedArtifactProjectConfig::new_with_canonical_timestamps(
        &typed_project.path,
        "typed-timestamp",
        "Creator",
        "Agent",
        CanonicalTimestamp::parse(recorded_at).unwrap(),
        CanonicalTimestamp::parse(expires_at).unwrap(),
    );

    let raw_pending = begin_artifact_proposal(
        &raw,
        &manifest(),
        &manifest(),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .expect("raw canonical timestamps are accepted");
    let typed_pending = begin_artifact_proposal(
        &typed,
        &manifest(),
        &manifest(),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .expect("typed canonical timestamps are accepted");
    assert_eq!(
        raw_pending.durable_binding().decision_head(),
        typed_pending.durable_binding().decision_head()
    );
    assert_eq!(
        raw_pending.durable_binding().proposal_head(),
        typed_pending.durable_binding().proposal_head()
    );
}
