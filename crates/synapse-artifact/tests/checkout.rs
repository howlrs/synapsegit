mod support;

use serde_json::{Value as JsonValue, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::approved_decide;
use synapse_application::DurableProposalBinding;
use synapse_artifact::{
    ArtifactCheckoutLimits, ArtifactDecisionOptions, ArtifactDisposition, ArtifactLimits,
    ArtifactManifestEntry, ArtifactSourceAttribution, RegularFileManifest,
    TrustedArtifactDecisionBinding, TrustedArtifactProjectConfig, begin_artifact_proposal,
    begin_next_artifact_proposal, checkout_artifact_decision,
};
use synapse_canonical::blob_oid;
use synapse_core::Repository;
use synapse_sqlite::{RefUpdate, ReflogMetadata, SqliteRefStore, ValidationError};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

const ACCEPTED_INDEX: &[u8] = b"<!doctype html><title>accepted</title>";
const ACCEPTED_CSS: &[u8] = b"body { color: blue; }";
const PROPOSED_INDEX: &[u8] = b"<!doctype html><title>proposed</title>";
const PROPOSED_CSS: &[u8] = b"body { color: green; }";
const CONTROL_CANARY: &str = "SECRET-CONTROL-CANARY";

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
                "synapse-artifact-checkout-{label}-{}-{nanos}-{serial}",
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

#[derive(Clone)]
struct CompletedDecision {
    repository: PathBuf,
    project_key: String,
    proposal: DurableProposalBinding,
    decision_head: String,
    disposition: ArtifactDisposition,
    digest: String,
}

impl CompletedDecision {
    fn binding(&self) -> TrustedArtifactDecisionBinding {
        self.binding_with_digest(self.digest.clone())
    }

    fn binding_with_digest(&self, digest: impl Into<String>) -> TrustedArtifactDecisionBinding {
        TrustedArtifactDecisionBinding::new(
            &self.repository,
            self.project_key.clone(),
            self.proposal.clone(),
            self.decision_head.clone(),
            self.disposition,
            digest,
        )
    }
}

fn manifest(index: &[u8], css: &[u8]) -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [
            ArtifactManifestEntry::regular_file("index.html", index.to_vec()),
            ArtifactManifestEntry::regular_file("assets/site.css", css.to_vec()),
        ],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn complete_decision(
    temp: &TempProject,
    project_key: &str,
    disposition: ArtifactDisposition,
) -> CompletedDecision {
    let mut pending = begin_artifact_proposal(
        &temp.config(project_key),
        &manifest(ACCEPTED_INDEX, ACCEPTED_CSS),
        &manifest(PROPOSED_INDEX, PROPOSED_CSS),
        format!(r#"{{"private_control_canary":"{CONTROL_CANARY}"}}"#).as_bytes(),
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let proposal = pending.durable_binding();
    let receipt = approved_decide(
        &mut pending,
        &ArtifactDecisionOptions {
            disposition,
            private_rationale: Some(format!("private rationale {CONTROL_CANARY}")),
        },
    )
    .unwrap();
    let digest = receipt.reviewed_artifact_manifest_sha256().to_owned();
    drop(pending);

    let repository = Repository::open(&temp.path).unwrap();
    let decision_head = repository
        .refs()
        .get(proposal.decision_ref_name())
        .unwrap()
        .unwrap()
        .head;
    drop(repository);

    CompletedDecision {
        repository: temp.path.clone(),
        project_key: project_key.to_owned(),
        proposal,
        decision_head,
        disposition,
        digest,
    }
}

fn source_state(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, current: &Path, state: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let mut entries = fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                state.insert(relative, None);
                visit(root, &path, state);
            } else {
                state.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }

    let mut state = BTreeMap::new();
    visit(root, root, &mut state);
    state
}

fn object_json(repository: &Repository, oid: &str) -> JsonValue {
    serde_json::from_slice(
        &repository
            .objects()
            .read_raw(oid)
            .unwrap()
            .unwrap_or_else(|| panic!("missing fixture object")),
    )
    .unwrap()
}

fn put_unchecked(repository: &Repository, value: &JsonValue) -> String {
    repository
        .objects()
        .put_structured_unchecked(&serde_json::to_vec(value).unwrap())
        .unwrap()
        .oid
}

#[derive(Clone, Copy)]
enum MalformedSite {
    WrongKind,
    UnsupportedRecord,
    MissingBlob,
    Traversal,
    ReservedDeviceSuperscript,
    NonNfc,
    Collision,
    ExtraEntryField,
}

#[derive(Clone, Copy)]
enum OrphanAuthority {
    Policy,
    Grant,
}

fn replace_selected_site(
    completed: &CompletedDecision,
    malformed: MalformedSite,
) -> CompletedDecision {
    let corrupt_with_non_nfc = matches!(malformed, MalformedSite::NonNfc);
    let repository = Repository::open(&completed.repository).unwrap();
    let old_proposal = object_json(&repository, completed.proposal.proposal_head());
    let old_proposal_snapshot_oid = old_proposal["snapshot"].as_str().unwrap();
    let old_proposal_snapshot = object_json(&repository, old_proposal_snapshot_oid);
    let snapshot_entries = old_proposal_snapshot["entries"].as_object().unwrap();
    let base_snapshot = snapshot_entries["base"]["oid"].as_str().unwrap();
    let context_oid = snapshot_entries["context.json"]["oid"].as_str().unwrap();
    let old_activity_oid = snapshot_entries["activity.json"]["oid"].as_str().unwrap();
    let regular_blob = repository
        .put_blob(b"malformed-site-canary".as_slice())
        .unwrap()
        .oid;
    let empty_tree = put_unchecked(
        &repository,
        &json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": {},
            "extensions": {}
        }),
    );

    let entries = match malformed {
        MalformedSite::WrongKind => json!({
            "wrong-kind.txt": {"entry_kind":"blob", "oid":empty_tree}
        }),
        MalformedSite::UnsupportedRecord => json!({
            "metadata.json": {"entry_kind":"record", "oid":context_oid}
        }),
        MalformedSite::MissingBlob => json!({
            "missing.txt": {"entry_kind":"blob", "oid":blob_oid(b"definitely absent blob")}
        }),
        MalformedSite::Traversal => json!({
            "..": {"entry_kind":"blob", "oid":regular_blob}
        }),
        MalformedSite::ReservedDeviceSuperscript => json!({
            "COM¹.txt": {"entry_kind":"blob", "oid":regular_blob}
        }),
        // Synapse Canonical JSON rejects non-NFC keys before they can receive
        // an OID. Store an NFC placeholder, then corrupt its CAS bytes below
        // to prove the verified read still fails closed on a non-NFC key.
        MalformedSite::NonNfc => json!({
            "cafe.txt": {"entry_kind":"blob", "oid":regular_blob}
        }),
        MalformedSite::Collision => json!({
            "README.txt": {"entry_kind":"blob", "oid":regular_blob},
            "Readme.txt": {"entry_kind":"tree", "oid":empty_tree}
        }),
        MalformedSite::ExtraEntryField => json!({
            "extra.txt": {"entry_kind":"blob", "oid":regular_blob, "mode":"executable"}
        }),
    };
    let site_oid = put_unchecked(
        &repository,
        &json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": entries,
            "extensions": {}
        }),
    );

    let mut activity = object_json(&repository, old_activity_oid);
    activity["payload"]["output_refs"] = json!([{"role":"proposal", "oid":site_oid}]);
    let activity_oid = put_unchecked(&repository, &activity);
    let proposal_snapshot_oid = put_unchecked(
        &repository,
        &json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": {
                "activity.json": {"entry_kind":"record", "oid":activity_oid},
                "base": {"entry_kind":"tree", "oid":base_snapshot},
                "context.json": {"entry_kind":"record", "oid":context_oid},
                "site": {"entry_kind":"tree", "oid":site_oid}
            },
            "extensions": {}
        }),
    );
    let mut proposal = old_proposal;
    proposal["snapshot"] = json!(proposal_snapshot_oid);
    proposal["transition_refs"] = json!([activity_oid]);
    let proposal_head = put_unchecked(&repository, &proposal);

    let mut decision = object_json(&repository, &completed.decision_head);
    let old_feedback_oid = decision["transition_refs"][0].as_str().unwrap();
    let mut feedback = object_json(&repository, old_feedback_oid);
    feedback["payload"]["proposal_ref"] = json!(proposal_head);
    let feedback_oid = put_unchecked(&repository, &feedback);
    decision["snapshot"] = json!(proposal_snapshot_oid);
    decision["transition_refs"] = json!([feedback_oid]);
    let decision_head = put_unchecked(&repository, &decision);
    drop(repository);

    let mut refs = SqliteRefStore::open(completed.repository.join("refs.sqlite3")).unwrap();
    let allow = |_head: &str| Ok::<(), ValidationError>(());
    refs.compare_and_swap(
        RefUpdate {
            ref_name: completed.proposal.proposal_ref_name(),
            expected_head: Some(completed.proposal.proposal_head()),
            new_head: &proposal_head,
            metadata: ReflogMetadata::at(i64::MAX - 2),
        },
        &allow,
    )
    .unwrap();
    refs.compare_and_swap(
        RefUpdate {
            ref_name: completed.proposal.decision_ref_name(),
            expected_head: Some(&completed.decision_head),
            new_head: &decision_head,
            metadata: ReflogMetadata::at(i64::MAX - 1),
        },
        &allow,
    )
    .unwrap();
    drop(refs);

    if corrupt_with_non_nfc {
        let digest = site_oid.rsplit(':').next().unwrap();
        let path = completed
            .repository
            .join("cas/objects/tree")
            .join(&digest[..2])
            .join(&digest[2..]);
        fs::write(
            path,
            serde_json::to_vec(&json!({
                "object_type": "tree",
                "schema_version": "0.1.0",
                "entries": {
                    "cafe\u{301}.txt": {"entry_kind":"blob", "oid":regular_blob}
                },
                "extensions": {}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    CompletedDecision {
        repository: completed.repository.clone(),
        project_key: completed.project_key.clone(),
        proposal: DurableProposalBinding::new(
            completed.proposal.project().clone(),
            completed.proposal.proposal_ref_name(),
            proposal_head,
            completed.proposal.decision_ref_name(),
            completed.proposal.decision_head(),
        ),
        decision_head,
        disposition: ArtifactDisposition::AdoptedUnchanged,
        digest: "0".repeat(64),
    }
}

fn replace_with_orphan_authority(
    completed: &CompletedDecision,
    orphan: OrphanAuthority,
) -> CompletedDecision {
    let repository = Repository::open(&completed.repository).unwrap();
    let mut proposal = object_json(&repository, completed.proposal.proposal_head());
    let old_snapshot = object_json(&repository, proposal["snapshot"].as_str().unwrap());
    let old_entries = old_snapshot["entries"].as_object().unwrap();
    let base_snapshot = old_entries["base"]["oid"].as_str().unwrap();
    let site_oid = old_entries["site"]["oid"].as_str().unwrap();
    let old_context_oid = old_entries["context.json"]["oid"].as_str().unwrap();
    let old_activity_oid = old_entries["activity.json"]["oid"].as_str().unwrap();
    let mut context = object_json(&repository, old_context_oid);
    let old_policy_oid = context["payload"]["policy_snapshot_ref"].as_str().unwrap();
    let old_grant_oid = context["payload"]["delegation_grant_ref"].as_str().unwrap();

    let (policy_oid, grant_oid) = match orphan {
        OrphanAuthority::Policy => {
            let mut policy = object_json(&repository, old_policy_oid);
            policy["payload"]["rules"][0]["rule_id"] = json!("orphan-context-read");
            (
                put_unchecked(&repository, &policy),
                old_grant_oid.to_owned(),
            )
        }
        OrphanAuthority::Grant => {
            let mut grant = object_json(&repository, old_grant_oid);
            grant["payload"]["purpose"] =
                json!("Orphan grant copy used only by the negative checkout test.");
            (
                old_policy_oid.to_owned(),
                put_unchecked(&repository, &grant),
            )
        }
    };
    context["payload"]["policy_snapshot_ref"] = json!(policy_oid);
    context["payload"]["delegation_grant_ref"] = json!(grant_oid);
    let context_oid = put_unchecked(&repository, &context);

    let mut activity = object_json(&repository, old_activity_oid);
    for input in activity["payload"]["input_refs"].as_array_mut().unwrap() {
        if input["role"] == "context" {
            input["oid"] = json!(context_oid);
        }
    }
    activity["payload"]["ai_run"]["context_pack_ref"] = json!(context_oid);
    activity["payload"]["ai_run"]["delegation_grant_ref"] = json!(grant_oid);
    let activity_oid = put_unchecked(&repository, &activity);

    let snapshot_oid = put_unchecked(
        &repository,
        &json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": {
                "activity.json": {"entry_kind":"record", "oid":activity_oid},
                "base": {"entry_kind":"tree", "oid":base_snapshot},
                "context.json": {"entry_kind":"record", "oid":context_oid},
                "site": {"entry_kind":"tree", "oid":site_oid}
            },
            "extensions": {}
        }),
    );
    proposal["snapshot"] = json!(snapshot_oid);
    proposal["transition_refs"] = json!([activity_oid]);
    let proposal_head = put_unchecked(&repository, &proposal);

    let mut decision = object_json(&repository, &completed.decision_head);
    let mut feedback = object_json(
        &repository,
        decision["transition_refs"][0].as_str().unwrap(),
    );
    feedback["payload"]["proposal_ref"] = json!(proposal_head);
    let feedback_oid = put_unchecked(&repository, &feedback);
    decision["snapshot"] = json!(snapshot_oid);
    decision["transition_refs"] = json!([feedback_oid]);
    let decision_head = put_unchecked(&repository, &decision);
    drop(repository);

    let mut refs = SqliteRefStore::open(completed.repository.join("refs.sqlite3")).unwrap();
    let allow = |_head: &str| Ok::<(), ValidationError>(());
    refs.compare_and_swap(
        RefUpdate {
            ref_name: completed.proposal.proposal_ref_name(),
            expected_head: Some(completed.proposal.proposal_head()),
            new_head: &proposal_head,
            metadata: ReflogMetadata::at(i64::MAX - 2),
        },
        &allow,
    )
    .unwrap();
    refs.compare_and_swap(
        RefUpdate {
            ref_name: completed.proposal.decision_ref_name(),
            expected_head: Some(&completed.decision_head),
            new_head: &decision_head,
            metadata: ReflogMetadata::at(i64::MAX - 1),
        },
        &allow,
    )
    .unwrap();
    drop(refs);

    CompletedDecision {
        repository: completed.repository.clone(),
        project_key: completed.project_key.clone(),
        proposal: DurableProposalBinding::new(
            completed.proposal.project().clone(),
            completed.proposal.proposal_ref_name(),
            proposal_head,
            completed.proposal.decision_ref_name(),
            completed.proposal.decision_head(),
        ),
        decision_head,
        disposition: ArtifactDisposition::AdoptedUnchanged,
        digest: completed.digest.clone(),
    }
}

#[test]
fn checkout_after_restart_returns_exact_selected_bytes_and_excludes_canaries() {
    for (label, disposition, expected_snapshot, expected_index, expected_css) in [
        (
            "adopt",
            ArtifactDisposition::AdoptedUnchanged,
            "proposal",
            PROPOSED_INDEX,
            PROPOSED_CSS,
        ),
        (
            "reject",
            ArtifactDisposition::Rejected,
            "base",
            ACCEPTED_INDEX,
            ACCEPTED_CSS,
        ),
        (
            "defer",
            ArtifactDisposition::Deferred,
            "base",
            ACCEPTED_INDEX,
            ACCEPTED_CSS,
        ),
    ] {
        let temp = TempProject::new(label);
        let completed = complete_decision(&temp, &format!("checkout-{label}"), disposition);
        let binding = completed.binding();
        assert_eq!(binding.disposition(), disposition);
        let before = source_state(&temp.path);

        let checkout =
            checkout_artifact_decision(&binding, ArtifactCheckoutLimits::default()).unwrap();

        assert_eq!(checkout.disposition(), disposition);
        assert_eq!(checkout.selected_snapshot(), expected_snapshot);
        assert_eq!(checkout.manifest_sha256(), completed.digest);
        assert_eq!(checkout.file_count(), 2);
        assert_eq!(
            checkout.total_bytes(),
            u64::try_from(expected_index.len() + expected_css.len()).unwrap()
        );
        assert_eq!(checkout.bytes("index.html"), Some(expected_index));
        assert_eq!(checkout.bytes("assets/site.css"), Some(expected_css));
        assert_eq!(
            checkout
                .files()
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            vec!["assets/site.css", "index.html"]
        );
        assert!(checkout.files().all(|entry| {
            !entry
                .bytes()
                .windows(CONTROL_CANARY.len())
                .any(|window| window == CONTROL_CANARY.as_bytes())
        }));
        assert_eq!(source_state(&temp.path), before, "{label} mutated source");

        let debug = format!("{checkout:?}");
        assert!(debug.contains("redacted"));
        assert!(!debug.contains("index.html"));
        assert!(!debug.contains(CONTROL_CANARY));
        let binding_debug = format!("{binding:?}");
        assert_eq!(
            binding_debug,
            "TrustedArtifactDecisionBinding(<redacted trusted binding>)"
        );
        assert!(!binding_debug.contains(&temp.path.display().to_string()));
        assert!(!binding_debug.contains(&completed.decision_head));
    }
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn every_checkout_work_limit_fails_closed_without_a_partial_result() {
    let temp = TempProject::new("limits");
    let completed = complete_decision(
        &temp,
        "checkout-limits",
        ArtifactDisposition::AdoptedUnchanged,
    );
    let binding = completed.binding();
    let mut cases = Vec::new();

    let mut limits = ArtifactCheckoutLimits::default();
    limits.artifact.max_files = 1;
    cases.push(("files", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.artifact.max_file_bytes = 1;
    cases.push(("file-bytes", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.artifact.max_total_bytes = 1;
    cases.push(("total-bytes", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.artifact.max_path_bytes = 5;
    cases.push(("path-bytes", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.artifact.max_depth = 1;
    cases.push(("depth", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.max_tree_nodes = 1;
    cases.push(("tree-nodes", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.max_tree_edges = 1;
    cases.push(("tree-edges", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.max_authority_nodes = 1;
    cases.push(("authority-nodes", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.max_authority_edges = 1;
    cases.push(("authority-edges", limits));
    let mut limits = ArtifactCheckoutLimits::default();
    limits.max_ref_snapshot_entries = 1;
    cases.push(("ref-snapshot", limits));

    for (label, limits) in cases {
        let before = source_state(&temp.path);
        let error = checkout_artifact_decision(&binding, limits).unwrap_err();
        assert_eq!(error.code(), "resource_limit", "{label}: {error:?}");
        assert_eq!(source_state(&temp.path), before, "{label} mutated source");
        assert!(!format!("{error:?}").contains(CONTROL_CANARY));
    }

    let mut invalid = ArtifactCheckoutLimits::default();
    invalid.max_tree_nodes = 0;
    assert_eq!(
        checkout_artifact_decision(&binding, invalid)
            .unwrap_err()
            .code(),
        "artifact_limits_invalid"
    );
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn authority_limits_are_shared_across_history_and_control_proof() {
    let temp = TempProject::new("shared-authority-limits");
    let project_key = "checkout-shared-authority-limits";
    let _first = complete_decision(&temp, project_key, ArtifactDisposition::AdoptedUnchanged);
    let accepted = manifest(PROPOSED_INDEX, PROPOSED_CSS);
    let proposed = manifest(
        b"<!doctype html><title>second proposal</title>",
        b"body { color: purple; }",
    );
    let mut pending = begin_next_artifact_proposal(
        &temp.config(project_key),
        &accepted,
        &proposed,
        br#"{"turn":2}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let proposal = pending.durable_binding();
    let receipt = approved_decide(
        &mut pending,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::AdoptedUnchanged,
            private_rationale: None,
        },
    )
    .unwrap();
    drop(pending);
    let repository = Repository::open(&temp.path).unwrap();
    let decision_head = repository
        .refs()
        .get(proposal.decision_ref_name())
        .unwrap()
        .unwrap()
        .head;
    drop(repository);
    let binding = TrustedArtifactDecisionBinding::new(
        &temp.path,
        project_key,
        proposal,
        decision_head,
        ArtifactDisposition::AdoptedUnchanged,
        receipt.reviewed_artifact_manifest_sha256(),
    );

    // Each phase alone fits these values. Their combined work does not, so a
    // reset between history and protected-control verification would pass.
    for (label, limits) in [
        (
            "nodes",
            ArtifactCheckoutLimits {
                max_authority_nodes: 9,
                ..ArtifactCheckoutLimits::default()
            },
        ),
        (
            "edges",
            ArtifactCheckoutLimits {
                max_authority_edges: 12,
                ..ArtifactCheckoutLimits::default()
            },
        ),
    ] {
        let before = source_state(&temp.path);
        let error = checkout_artifact_decision(&binding, limits).unwrap_err();
        assert_eq!(error.code(), "resource_limit", "{label}: {error:?}");
        assert_eq!(source_state(&temp.path), before, "{label} mutated source");
    }
}

#[test]
fn stale_or_digest_mismatched_binding_returns_no_checkout_authority_details() {
    let temp = TempProject::new("binding-errors");
    let completed = complete_decision(
        &temp,
        "checkout-binding-errors",
        ArtifactDisposition::AdoptedUnchanged,
    );
    let mismatch = completed.binding_with_digest("0".repeat(64));
    let error =
        checkout_artifact_decision(&mismatch, ArtifactCheckoutLimits::default()).unwrap_err();
    assert_eq!(error.code(), "artifact_digest_mismatch");
    for rendered in [format!("{error:?}"), error.to_string()] {
        assert!(!rendered.contains(&completed.decision_head));
        assert!(!rendered.contains(&temp.path.display().to_string()));
        assert!(!rendered.contains(CONTROL_CANARY));
    }

    let stale = TrustedArtifactDecisionBinding::new(
        &completed.repository,
        completed.project_key.clone(),
        completed.proposal.clone(),
        completed.proposal.decision_head().to_owned(),
        completed.disposition,
        completed.digest.clone(),
    );
    assert_eq!(
        checkout_artifact_decision(&stale, ArtifactCheckoutLimits::default())
            .unwrap_err()
            .code(),
        "stale_base"
    );
}

#[test]
fn corrupt_selected_blob_is_detected_by_the_bounded_verified_read() {
    let temp = TempProject::new("corrupt-blob");
    let completed = complete_decision(
        &temp,
        "checkout-corrupt-blob",
        ArtifactDisposition::AdoptedUnchanged,
    );
    let oid = blob_oid(PROPOSED_INDEX);
    let digest = oid.rsplit(':').next().unwrap();
    let object_path = temp
        .path
        .join("cas/objects/blob")
        .join(&digest[..2])
        .join(&digest[2..]);
    fs::write(&object_path, b"corrupt replacement bytes").unwrap();

    let error = checkout_artifact_decision(&completed.binding(), ArtifactCheckoutLimits::default())
        .unwrap_err();
    assert_eq!(error.code(), "oid_mismatch");
    assert!(!format!("{error:?}").contains(&oid));
    assert!(!error.to_string().contains("corrupt replacement"));
}

#[test]
fn malformed_site_kinds_paths_collisions_and_missing_objects_fail_closed() {
    for (label, malformed, expected_code) in [
        (
            "wrong-kind",
            MalformedSite::WrongKind,
            "reference_type_mismatch",
        ),
        (
            "unsupported-record",
            MalformedSite::UnsupportedRecord,
            "artifact_entry_unsupported",
        ),
        (
            "missing-blob",
            MalformedSite::MissingBlob,
            "closure_missing",
        ),
        (
            "traversal",
            MalformedSite::Traversal,
            "artifact_path_invalid",
        ),
        (
            "reserved-device-superscript",
            MalformedSite::ReservedDeviceSuperscript,
            "artifact_path_invalid",
        ),
        ("non-nfc", MalformedSite::NonNfc, "oid_mismatch"),
        (
            "collision",
            MalformedSite::Collision,
            "artifact_path_collision",
        ),
        (
            "extra-entry-field",
            MalformedSite::ExtraEntryField,
            "artifact_lineage_invalid",
        ),
    ] {
        let temp = TempProject::new(label);
        let completed = complete_decision(
            &temp,
            &format!("checkout-malformed-{label}"),
            ArtifactDisposition::AdoptedUnchanged,
        );
        let malformed = replace_selected_site(&completed, malformed);
        let before = source_state(&temp.path);

        let error =
            checkout_artifact_decision(&malformed.binding(), ArtifactCheckoutLimits::default())
                .unwrap_err();

        assert_eq!(error.code(), expected_code, "{label}: {error:?}");
        assert_eq!(source_state(&temp.path), before, "{label} mutated source");
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(CONTROL_CANARY));
        assert!(!rendered.contains(&malformed.decision_head));
        assert!(!rendered.contains(&temp.path.display().to_string()));
    }
}

#[test]
fn orphan_policy_or_grant_not_reachable_from_base_snapshot_is_rejected() {
    for (label, orphan) in [
        ("orphan-policy", OrphanAuthority::Policy),
        ("orphan-grant", OrphanAuthority::Grant),
    ] {
        let temp = TempProject::new(label);
        let completed = complete_decision(
            &temp,
            &format!("checkout-{label}"),
            ArtifactDisposition::AdoptedUnchanged,
        );
        let orphaned = replace_with_orphan_authority(&completed, orphan);

        let error =
            checkout_artifact_decision(&orphaned.binding(), ArtifactCheckoutLimits::default())
                .unwrap_err();

        assert_eq!(error.code(), "artifact_lineage_invalid", "{label}");
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(&orphaned.decision_head));
        assert!(!rendered.contains(&temp.path.display().to_string()));
        assert!(!rendered.contains(CONTROL_CANARY));
    }
}
