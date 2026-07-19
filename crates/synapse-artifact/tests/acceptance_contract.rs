mod support;

use serde_json::{Value as JsonValue, json};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::approved_decide;
use synapse_artifact::{
    ArtifactDecisionOptions, ArtifactDisposition, ArtifactEntryKind, ArtifactLimits,
    ArtifactManifestEntry, ArtifactSourceAttribution, RegularFileManifest,
    TrustedArtifactProjectConfig, begin_artifact_proposal,
};
use synapse_core::Repository;
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self {
            path: std::env::temp_dir().join(format!(
                "synapse-artifact-acceptance-{label}-{}-{nanos}-{serial}",
                std::process::id()
            )),
        }
    }

    fn config(&self, key: &str) -> TrustedArtifactProjectConfig {
        TrustedArtifactProjectConfig::new(
            &self.path,
            key,
            "Artifact Creator",
            "Application-owned AI",
            "2026-07-19T00:00:00.000000000Z",
            "2099-01-01T00:00:00.000000000Z",
        )
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        if self.path.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn manifest(label: &str) -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [
            ArtifactManifestEntry::regular_file(
                "index.html",
                format!("<!doctype html><title>{label}</title>").into_bytes(),
            ),
            ArtifactManifestEntry::regular_file(
                "assets/site.css",
                format!("/* {label} */ body {{ color: #123456; }}").into_bytes(),
            ),
        ],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn begin(temp: &TempProject, key: &str) -> synapse_artifact::PendingArtifactProposal {
    begin_artifact_proposal(
        &temp.config(key),
        &manifest("accepted"),
        &manifest("proposed"),
        br#"{"request":{"kind":"copy_change"},"selection":{"block_id":"hero"}}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap()
}

fn object_json(repository: &Repository, oid: &str) -> JsonValue {
    let bytes = repository
        .objects()
        .read_raw(oid)
        .unwrap_or_else(|error| panic!("read {oid}: {error}"))
        .unwrap_or_else(|| panic!("missing object {oid}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {oid}: {error}"))
}

fn direct_site_oid(repository: &Repository, commit_oid: &str) -> String {
    let commit = object_json(repository, commit_oid);
    assert_eq!(
        commit.get("object_type").and_then(JsonValue::as_str),
        Some("commit")
    );
    let snapshot_oid = commit
        .get("snapshot")
        .and_then(JsonValue::as_str)
        .unwrap_or_else(|| panic!("commit {commit_oid} has no snapshot"));
    let snapshot = object_json(repository, snapshot_oid);
    let site = snapshot
        .get("entries")
        .and_then(JsonValue::as_object)
        .and_then(|entries| entries.get("site"))
        .unwrap_or_else(|| panic!("snapshot {snapshot_oid} has no direct site entry"));
    assert_eq!(
        site.get("entry_kind").and_then(JsonValue::as_str),
        Some("tree")
    );
    site.get("oid")
        .and_then(JsonValue::as_str)
        .unwrap_or_else(|| panic!("snapshot {snapshot_oid} site entry has no OID"))
        .to_owned()
}

fn current_head(repository: &Repository, ref_name: &str) -> String {
    repository
        .refs()
        .get(ref_name)
        .unwrap_or_else(|error| panic!("read {ref_name}: {error}"))
        .unwrap_or_else(|| panic!("missing Ref {ref_name}"))
        .head
}

fn successor_commit(repository: &Repository, parent: &str, label: &str) -> String {
    let mut commit = object_json(repository, parent);
    commit["parents"] = json!([parent]);
    commit["message"] = json!(format!("external {label} advance"));
    repository
        .put_object(&serde_json::to_vec(&commit).unwrap())
        .unwrap_or_else(|error| panic!("store external {label} advance: {error}"))
        .oid
}

fn contract_schema(name: &str) -> JsonValue {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/application/generic-artifact/v1")
        .join(name);
    serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn assert_contract_match(schema: &JsonValue, instance: &JsonValue, expected: bool, label: &str) {
    let validator = jsonschema::draft202012::new(schema)
        .unwrap_or_else(|error| panic!("compile schema for {label}: {error}"));
    if expected && !validator.is_valid(instance) {
        let errors = validator
            .iter_errors(instance)
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        panic!("{label} did not match its frozen schema: {errors}");
    }
    assert_eq!(
        validator.is_valid(instance),
        expected,
        "unexpected schema result for {label}"
    );
}

#[test]
fn identical_fixtures_produce_identical_base_proposal_and_decision_commits() {
    fn run(temp: &TempProject) -> (String, String, String) {
        let mut pending = begin(temp, "deterministic-commit-flow");
        let binding = pending.durable_binding();
        approved_decide(
            &mut pending,
            &ArtifactDecisionOptions {
                disposition: ArtifactDisposition::AdoptedUnchanged,
                private_rationale: Some("Stable deterministic acceptance rationale.".into()),
            },
        )
        .unwrap();
        let repository = Repository::open(&temp.path).unwrap();
        let decision_head = current_head(&repository, binding.decision_ref_name());
        (
            binding.decision_head().to_owned(),
            binding.proposal_head().to_owned(),
            decision_head,
        )
    }

    let first = TempProject::new("deterministic-commits-first");
    let second = TempProject::new("deterministic-commits-second");
    let first_heads = run(&first);
    let second_heads = run(&second);

    assert_eq!(first_heads, second_heads);
    assert_ne!(first_heads.0, first_heads.1);
    assert_ne!(first_heads.0, first_heads.2);
    assert_ne!(first_heads.1, first_heads.2);
}

#[test]
fn committed_decision_graph_selects_the_exact_direct_site_tree() {
    for (label, disposition, expected_snapshot) in [
        (
            "adopt-graph",
            ArtifactDisposition::AdoptedUnchanged,
            "proposal",
        ),
        ("reject-graph", ArtifactDisposition::Rejected, "base"),
        ("defer-graph", ArtifactDisposition::Deferred, "base"),
    ] {
        let temp = TempProject::new(label);
        let mut pending = begin(&temp, label);
        let binding = pending.durable_binding();
        let repository = Repository::open(&temp.path).unwrap();
        let base_site = direct_site_oid(&repository, binding.decision_head());
        let proposal_site = direct_site_oid(&repository, binding.proposal_head());
        assert_ne!(base_site, proposal_site, "{label}");

        let receipt = approved_decide(
            &mut pending,
            &ArtifactDecisionOptions {
                disposition,
                private_rationale: None,
            },
        )
        .unwrap();
        let decision_head = current_head(&repository, binding.decision_ref_name());
        let selected_site = direct_site_oid(&repository, &decision_head);
        let expected_site = match expected_snapshot {
            "proposal" => &proposal_site,
            "base" => &base_site,
            _ => unreachable!(),
        };

        assert_eq!(receipt.selected_snapshot(), expected_snapshot, "{label}");
        assert_eq!(&selected_site, expected_site, "{label}");
    }
}

#[test]
fn externally_advanced_proposal_or_decision_ref_fails_without_further_ref_mutation() {
    for moved in ["proposal", "decision"] {
        let temp = TempProject::new(&format!("moved-{moved}"));
        let key = format!("moved-{moved}-flow");
        let mut pending = begin(&temp, &key);
        let binding = pending.durable_binding();
        let mut repository = Repository::open(&temp.path).unwrap();
        let (ref_name, expected_head) = match moved {
            "proposal" => (binding.proposal_ref_name(), binding.proposal_head()),
            "decision" => (binding.decision_ref_name(), binding.decision_head()),
            _ => unreachable!(),
        };
        let advanced_head = successor_commit(&repository, expected_head, moved);
        repository
            .update_ref(RefUpdate {
                ref_name,
                expected_head: Some(expected_head),
                new_head: &advanced_head,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: i64::MAX - 1,
                    actor: Some("external-test-writer"),
                    message: Some("acceptance-test external advance"),
                },
            })
            .unwrap();
        let refs_after_external_advance = repository.refs().snapshot().unwrap();
        let reflog_after_external_advance = repository.refs().reflog().unwrap();

        let error = approved_decide(
            &mut pending,
            &ArtifactDecisionOptions {
                disposition: ArtifactDisposition::AdoptedUnchanged,
                private_rationale: None,
            },
        )
        .unwrap_err();

        assert!(
            matches!(
                error.code(),
                "configuration_invalid" | "authorization_denied" | "ref_conflict" | "stale_base"
            ),
            "unexpected {moved} error: {}",
            error.code()
        );
        assert_eq!(
            repository.refs().snapshot().unwrap(),
            refs_after_external_advance,
            "{moved} Ref state changed after denial"
        );
        assert_eq!(
            repository.refs().reflog().unwrap(),
            reflog_after_external_advance,
            "{moved} reflog changed after denial"
        );
    }
}

#[test]
fn unsafe_manifest_matrix_cannot_mutate_existing_refs_or_reflog() {
    let temp = TempProject::new("unsafe-manifest-sentinel");
    let _pending = begin(&temp, "unsafe-manifest-sentinel");
    let repository = Repository::open(&temp.path).unwrap();
    let expected_refs = repository.refs().snapshot().unwrap();
    let expected_reflog = repository.refs().reflog().unwrap();
    let default_limits = ArtifactLimits::default();
    let cases = vec![
        (
            "symlink",
            vec![ArtifactManifestEntry::unsupported(
                "unsafe-link",
                ArtifactEntryKind::Symlink,
            )],
            default_limits,
            "artifact_entry_unsupported",
        ),
        (
            "device",
            vec![ArtifactManifestEntry::unsupported(
                "unsafe-device",
                ArtifactEntryKind::Device,
            )],
            default_limits,
            "artifact_entry_unsupported",
        ),
        (
            "absolute",
            vec![ArtifactManifestEntry::regular_file(
                "/etc/passwd",
                b"x".to_vec(),
            )],
            default_limits,
            "artifact_path_invalid",
        ),
        (
            "traversal",
            vec![ArtifactManifestEntry::regular_file(
                "assets/../outside",
                b"x".to_vec(),
            )],
            default_limits,
            "artifact_path_invalid",
        ),
        (
            "unicode-non-nfc",
            vec![ArtifactManifestEntry::regular_file(
                "assets/cafe\u{301}.txt",
                b"x".to_vec(),
            )],
            default_limits,
            "artifact_path_invalid",
        ),
        (
            "case-collision",
            vec![
                ArtifactManifestEntry::regular_file("Assets/a.txt", b"1".to_vec()),
                ArtifactManifestEntry::regular_file("assets/b.txt", b"2".to_vec()),
            ],
            default_limits,
            "artifact_path_collision",
        ),
        (
            "file-count",
            vec![
                ArtifactManifestEntry::regular_file("a", b"1".to_vec()),
                ArtifactManifestEntry::regular_file("b", b"2".to_vec()),
            ],
            ArtifactLimits {
                max_files: 1,
                ..default_limits
            },
            "resource_limit",
        ),
        (
            "per-file-bytes",
            vec![ArtifactManifestEntry::regular_file(
                "large",
                b"12345".to_vec(),
            )],
            ArtifactLimits {
                max_file_bytes: 4,
                ..default_limits
            },
            "resource_limit",
        ),
        (
            "aggregate-bytes",
            vec![
                ArtifactManifestEntry::regular_file("a", b"1234".to_vec()),
                ArtifactManifestEntry::regular_file("b", b"5678".to_vec()),
            ],
            ArtifactLimits {
                max_total_bytes: 6,
                ..default_limits
            },
            "resource_limit",
        ),
    ];

    for (label, entries, limits, expected_code) in cases {
        let error = RegularFileManifest::from_entries(entries, limits).unwrap_err();
        assert_eq!(error.code(), expected_code, "{label}");
        assert_eq!(
            repository.refs().snapshot().unwrap(),
            expected_refs,
            "{label}"
        );
        assert_eq!(
            repository.refs().reflog().unwrap(),
            expected_reflog,
            "{label}"
        );
    }
}

#[test]
fn process_receipts_need_explicit_review_context_to_match_wire_schemas() {
    let proposal_schema = contract_schema("proposal-receipt.schema.json");
    let review_schema = contract_schema("review-status.schema.json");
    let review_id = "a".repeat(64);
    let temp = TempProject::new("wire-receipts");
    let mut pending = begin(&temp, "wire-receipts");

    let mut proposal_wire = json!({
        "contract": pending.receipt().contract(),
        "contract_version": pending.receipt().contract_version(),
        "artifact_manifest_sha256": pending.receipt().artifact_manifest_sha256(),
        "review_context_sha256": pending.receipt().review_context_sha256(),
        "source_attribution": pending.receipt().source_attribution(),
        "execution_verified": pending.receipt().execution_verified(),
    });
    assert_contract_match(
        &proposal_schema,
        &proposal_wire,
        false,
        "process-only Proposal receipt",
    );
    proposal_wire["review_id"] = json!(review_id);
    proposal_wire["state"] = json!("pending_review");
    assert_contract_match(
        &proposal_schema,
        &proposal_wire,
        true,
        "explicitly adapted Proposal wire receipt",
    );

    let decision = approved_decide(
        &mut pending,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::AdoptedUnchanged,
            private_rationale: None,
        },
    )
    .unwrap();
    let mut decision_wire = json!({
        "contract": decision.contract(),
        "contract_version": decision.contract_version(),
        "disposition": decision.disposition(),
        "reviewed_artifact_manifest_sha256": decision.reviewed_artifact_manifest_sha256(),
        "selected_snapshot": decision.selected_snapshot(),
    });
    assert_contract_match(
        &review_schema,
        &decision_wire,
        false,
        "process-only Decision receipt",
    );
    decision_wire["review_id"] = json!(review_id);
    decision_wire["state"] = json!("decision_committed");
    assert_contract_match(
        &review_schema,
        &decision_wire,
        true,
        "explicitly adapted Decision wire receipt",
    );
}
