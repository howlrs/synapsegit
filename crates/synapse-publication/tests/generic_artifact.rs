use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_application::{AuthenticatedSession, AuthenticationFailure, Authenticator};
use synapse_artifact::{
    ArtifactApprovalRegistry, ArtifactCheckoutLimits, ArtifactDecisionOptions,
    ArtifactDecisionReceipt, ArtifactDisposition, ArtifactLimits, ArtifactManifestEntry,
    ArtifactSourceAttribution, PendingArtifactProposal, RegularFileManifest,
    TrustedArtifactDecisionBinding, TrustedArtifactProjectConfig, WorkflowError,
    begin_artifact_proposal, decide_artifact_proposal,
};
use synapse_canonical::{canonical_bytes, parse_strict};
use synapse_core::{Repository, SystemAuthorizationClock};
use synapse_publication::{
    GENERIC_ARTIFACT_BUNDLE_SCHEMA, GENERIC_ARTIFACT_BUNDLE_SCHEMA_VERSION,
    GENERIC_ARTIFACT_CHECKSUMS_SCHEMA, GENERIC_ARTIFACT_CHECKSUMS_SCHEMA_VERSION,
    GENERIC_ARTIFACT_PROJECTION_SCHEMA, GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION,
    GENERIC_ARTIFACT_PUBLICATION_PROFILE, GENERIC_ARTIFACT_PUBLICATION_PROFILE_VERSION,
    GENERIC_ARTIFACT_RENDERER_PROFILE, GENERIC_ARTIFACT_RENDERER_PROFILE_VERSION,
    GenericArtifactBundleManifestV1, GenericArtifactBundleOptions, GenericArtifactChecksumsV1,
    GenericArtifactGeneratorIdentity, GenericArtifactHumanDisposition, GenericArtifactOutcomeState,
    GenericArtifactOutputTarget, GenericArtifactPublicationError, GenericArtifactSelectedSnapshot,
    GenericArtifactStatusReason, GenericArtifactVisibility, PublicTargetCaptureSourceV1,
    PublicTargetKindV1, ReviewedPublicTargetV1, TrustedGenericArtifactStatus,
    build_generic_artifact_complete_projection, build_generic_artifact_status_projection,
    export_generic_artifact_bundle, parse_reviewed_public_target_v1,
    validate_generic_artifact_projection, verify_generic_artifact_bundle,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
const PRIVATE_CONTEXT_CANARY: &str = "PRIVATE_CONTEXT_CANARY_ef731";
const PRIVATE_RATIONALE_CANARY: &str = "PRIVATE_RATIONALE_CANARY_a82c1";
const RAW_ACCEPTED_CANARY: &str = "RAW_ACCEPTED_SITE_CANARY_9ed01";
const RAW_PROPOSAL_CANARY: &str = "RAW_PROPOSAL_SITE_CANARY_77b42";
const HOST_CREDENTIAL: &str = "publication-test-host-credential";
const HOST_ACTOR: &str = "publication-test-host-actor";

#[derive(Clone, Copy)]
struct TestAuthenticator;

impl Authenticator for TestAuthenticator {
    type Credential = str;

    fn authenticate(
        &self,
        credential: &Self::Credential,
    ) -> std::result::Result<AuthenticatedSession, AuthenticationFailure> {
        if credential == HOST_CREDENTIAL {
            AuthenticatedSession::new(HOST_ACTOR, "publication-test-session")
        } else {
            Err(AuthenticationFailure)
        }
    }
}

fn approved_decide(
    pending: &mut PendingArtifactProposal,
    options: &ArtifactDecisionOptions,
) -> Result<ArtifactDecisionReceipt, WorkflowError> {
    let registry =
        ArtifactApprovalRegistry::new(TestAuthenticator, SystemAuthorizationClock, 60_000_000_000)
            .unwrap();
    registry
        .grant_project_access(pending.durable_binding().project(), HOST_ACTOR)
        .unwrap();
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending, options)
        .map_err(WorkflowError::from)?;
    decide_artifact_proposal(&registry, HOST_CREDENTIAL, &approval, pending, options)
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-publication-generic-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn manifest(canary: &str) -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [
            ArtifactManifestEntry::regular_file(
                "private/site-secret.html",
                format!("<!doctype html><title>{canary}</title>").into_bytes(),
            ),
            ArtifactManifestEntry::regular_file(
                "private/assets/secret.css",
                format!("/* {canary} */ body {{ color: green; }}").into_bytes(),
            ),
        ],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn complete_binding(
    root: &Path,
    project_key: &str,
    disposition: ArtifactDisposition,
) -> TrustedArtifactDecisionBinding {
    let repository_path = root.join("source-repository");
    let config = TrustedArtifactProjectConfig::new(
        &repository_path,
        project_key,
        "PRIVATE_CREATOR_CANARY_129df",
        "PRIVATE_PROVIDER_CANARY_7a3e2",
        "2026-07-19T00:00:00.000000000Z",
        "2099-01-01T00:00:00.000000000Z",
    );
    let mut pending = begin_artifact_proposal(
        &config,
        &manifest(RAW_ACCEPTED_CANARY),
        &manifest(RAW_PROPOSAL_CANARY),
        format!(r#"{{"private_context":"{PRIVATE_CONTEXT_CANARY}"}}"#).as_bytes(),
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let proposal = pending.durable_binding();
    let options = ArtifactDecisionOptions {
        disposition,
        private_rationale: Some(PRIVATE_RATIONALE_CANARY.into()),
    };
    let receipt = approved_decide(&mut pending, &options).unwrap();
    let digest = receipt.reviewed_artifact_manifest_sha256().to_owned();
    drop(pending);
    let repository = Repository::open(&repository_path).unwrap();
    let decision_head = repository
        .refs()
        .get(proposal.decision_ref_name())
        .unwrap()
        .unwrap()
        .head;
    drop(repository);
    TrustedArtifactDecisionBinding::new(
        repository_path,
        project_key,
        proposal,
        decision_head,
        disposition,
        digest,
    )
}

fn public_target(label: &str) -> ReviewedPublicTargetV1 {
    ReviewedPublicTargetV1::new(
        "0.0.0",
        "public-target-01",
        PublicTargetKindV1::Element,
        label,
        PublicTargetCaptureSourceV1::Accepted,
    )
    .unwrap()
}

fn canonical_json(value: &impl serde::Serialize) -> Vec<u8> {
    let encoded = serde_json::to_vec(value).unwrap();
    canonical_bytes(&parse_strict(&encoded).unwrap()).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, state: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if entry.file_type().unwrap().is_dir() {
                state.insert(relative, None);
                visit(root, &path, state);
            } else {
                state.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut state = BTreeMap::new();
    visit(root, root, &mut state);
    state
}

fn bundle_text(root: &Path) -> String {
    snapshot_tree(root)
        .into_values()
        .flatten()
        .map(|bytes| String::from_utf8(bytes).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn export_projection(
    root: &TempDirectory,
    name: &str,
    projection: &synapse_publication::GenericArtifactPublicProjectionV1,
    target: GenericArtifactOutputTarget,
) -> PathBuf {
    let destination = root.join(name);
    export_generic_artifact_bundle(&GenericArtifactBundleOptions {
        projection,
        destination: destination.clone(),
        target,
    })
    .unwrap();
    destination
}

fn reconcile_checksums(root: &Path) {
    let mut checksums: GenericArtifactChecksumsV1 =
        serde_json::from_slice(&fs::read(root.join("checksums.json")).unwrap()).unwrap();
    for entry in &mut checksums.files {
        let bytes = fs::read(root.join(&entry.path)).unwrap();
        entry.byte_len = bytes.len() as u64;
        entry.sha256 = sha256(&bytes);
    }
    fs::write(root.join("checksums.json"), canonical_json(&checksums)).unwrap();
}

#[test]
fn complete_projection_verifies_each_human_disposition_and_accepted_site_binding() {
    for (label, disposition, public_disposition, selected_snapshot) in [
        (
            "adopt",
            ArtifactDisposition::AdoptedUnchanged,
            GenericArtifactHumanDisposition::AdoptedUnchanged,
            GenericArtifactSelectedSnapshot::Proposal,
        ),
        (
            "reject",
            ArtifactDisposition::Rejected,
            GenericArtifactHumanDisposition::Rejected,
            GenericArtifactSelectedSnapshot::Base,
        ),
        (
            "defer",
            ArtifactDisposition::Deferred,
            GenericArtifactHumanDisposition::Deferred,
            GenericArtifactSelectedSnapshot::Base,
        ),
    ] {
        let temporary = TempDirectory::new(label);
        let binding = complete_binding(&temporary.0, &format!("projection-{label}"), disposition);
        let source = temporary.join("source-repository");
        let before = snapshot_tree(&source);
        let projection = build_generic_artifact_complete_projection(
            &public_target("Reviewed hero target"),
            &binding,
            ArtifactCheckoutLimits::default(),
            GenericArtifactVisibility::Public,
        )
        .unwrap();

        assert_eq!(
            projection.outcome.state,
            GenericArtifactOutcomeState::Complete
        );
        assert_eq!(
            projection.outcome.human_disposition,
            Some(public_disposition)
        );
        assert_eq!(
            projection.outcome.selected_snapshot,
            Some(selected_snapshot)
        );
        let site = projection.outcome.accepted_site.as_ref().unwrap();
        assert_eq!(site.file_count, 2);
        assert!(site.total_bytes > 0);
        assert!(!site.public_core_oid_included);
        assert_eq!(snapshot_tree(&source), before, "{label} mutated source");
    }
}

#[test]
fn bundle_is_private_by_construction_and_human_views_escape_public_text() {
    let temporary = TempDirectory::new("privacy");
    let binding = complete_binding(
        &temporary.0,
        "projection-privacy",
        ArtifactDisposition::AdoptedUnchanged,
    );
    let source_path = temporary.join("source-repository");
    let source_before = snapshot_tree(&source_path);
    let projection = build_generic_artifact_complete_projection(
        &public_target("Hero <script>alert('x')</script> [remote](javascript:x) | table"),
        &binding,
        ArtifactCheckoutLimits::default(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    let bundle = export_projection(
        &temporary,
        "github-bundle",
        &projection,
        GenericArtifactOutputTarget::Github,
    );
    verify_generic_artifact_bundle(&bundle).unwrap();
    let all_text = bundle_text(&bundle);

    for canary in [
        PRIVATE_CONTEXT_CANARY,
        PRIVATE_RATIONALE_CANARY,
        RAW_ACCEPTED_CANARY,
        RAW_PROPOSAL_CANARY,
        "PRIVATE_CREATOR_CANARY_129df",
        "PRIVATE_PROVIDER_CANARY_7a3e2",
        "private/site-secret.html",
        "private/assets/secret.css",
        source_path.to_str().unwrap(),
        "actor:",
        "policy:",
        "record:",
        "commit:",
    ] {
        assert!(!all_text.contains(canary), "bundle leaked {canary:?}");
    }
    let html = fs::read_to_string(bundle.join("index.html")).unwrap();
    let markdown = fs::read_to_string(bundle.join("story.md")).unwrap();
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script"));
    assert!(!html.contains("javascript:"));
    assert!(!html.contains("<iframe"));
    assert!(!html.contains("<form"));
    assert!(markdown.contains("\\<script\\>"));
    assert!(markdown.contains("\\[remote\\]\\(javascript:x\\)"));
    assert!(!markdown.contains("[remote](javascript:"));
    assert_eq!(snapshot_tree(&source_path), source_before);
}

#[test]
fn pending_and_incomplete_status_cannot_claim_decision_or_site_authority() {
    let target = public_target("Status-only target");
    let pending = build_generic_artifact_status_projection(
        &target,
        &TrustedGenericArtifactStatus::pending(),
        GenericArtifactVisibility::PrivateReview,
    )
    .unwrap();
    assert_eq!(pending.outcome.state, GenericArtifactOutcomeState::Pending);
    assert!(pending.outcome.human_disposition.is_none());
    assert!(pending.outcome.selected_snapshot.is_none());
    assert!(pending.outcome.accepted_site.is_none());
    assert_eq!(
        pending.outcome.status_reason,
        Some(GenericArtifactStatusReason::PendingReview)
    );

    for reason in [
        GenericArtifactStatusReason::RetryableFailure,
        GenericArtifactStatusReason::OutcomeUnknown,
        GenericArtifactStatusReason::TerminalDenial,
    ] {
        let status = TrustedGenericArtifactStatus::incomplete(reason).unwrap();
        let incomplete = build_generic_artifact_status_projection(
            &target,
            &status,
            GenericArtifactVisibility::PrivateReview,
        )
        .unwrap();
        assert_eq!(
            incomplete.outcome.state,
            GenericArtifactOutcomeState::Incomplete
        );
        assert!(incomplete.outcome.human_disposition.is_none());
        assert!(incomplete.outcome.selected_snapshot.is_none());
        assert!(incomplete.outcome.accepted_site.is_none());
        assert_eq!(incomplete.outcome.status_reason, Some(reason));
        assert!(
            incomplete
                .limitations
                .iter()
                .any(|limitation| limitation.code == "status_not_authority")
        );
    }
    assert!(
        TrustedGenericArtifactStatus::incomplete(GenericArtifactStatusReason::PendingReview)
            .is_err()
    );
}

#[test]
fn public_target_parser_is_a_strict_bounded_allowlist() {
    let valid = json!({
        "schema": {"name":"org.synapsegit.lp-studio.public-target","version":1},
        "lp_studio": {
            "product":"synapsegit-lp-studio",
            "product_version":"0.0.0",
            "api_version":"v1",
            "api_schema_version":"1",
            "target_schema_version":1
        },
        "target": {
            "target_id":"target-01",
            "kind":"element",
            "label":"Reviewed target",
            "capture_source":"accepted"
        }
    });
    let parsed = parse_reviewed_public_target_v1(&serde_json::to_vec(&valid).unwrap()).unwrap();
    assert_eq!(parsed.target().target_id(), "target-01");

    for forbidden in [
        "dom_quote",
        "dom_path",
        "page_path",
        "prompt",
        "provider_response",
        "private_rationale",
        "raw_site_bytes",
        "repository_path",
        "actor_id",
        "policy_id",
        "grant_id",
        "authority",
    ] {
        let mut injected = valid.clone();
        injected["target"][forbidden] = json!("PRIVATE_INJECTION_CANARY");
        assert!(
            parse_reviewed_public_target_v1(&serde_json::to_vec(&injected).unwrap()).is_err(),
            "accepted forbidden field {forbidden}"
        );
    }
    assert!(
        ReviewedPublicTargetV1::new(
            "0.0.0",
            "x".repeat(129),
            PublicTargetKindV1::Page,
            "label",
            PublicTargetCaptureSourceV1::Accepted,
        )
        .is_err()
    );
    assert!(
        ReviewedPublicTargetV1::new(
            "0.0.0",
            "target",
            PublicTargetKindV1::Page,
            "x".repeat(301),
            PublicTargetCaptureSourceV1::Accepted,
        )
        .is_err()
    );
    let unicode_boundary = ReviewedPublicTargetV1::new(
        "0.0.0",
        "target",
        PublicTargetKindV1::Page,
        "界".repeat(300),
        PublicTargetCaptureSourceV1::Accepted,
    )
    .unwrap();
    let parsed_unicode =
        parse_reviewed_public_target_v1(&serde_json::to_vec(&unicode_boundary).unwrap()).unwrap();
    assert_eq!(parsed_unicode.target().label().chars().count(), 300);
    assert!(
        ReviewedPublicTargetV1::new(
            "0.0.0",
            "target",
            PublicTargetKindV1::Page,
            "界".repeat(301),
            PublicTargetCaptureSourceV1::Accepted,
        )
        .is_err()
    );
    assert!(
        ReviewedPublicTargetV1::new(
            "0.0.0",
            "target",
            PublicTargetKindV1::Page,
            "bidi \u{202e} target",
            PublicTargetCaptureSourceV1::Accepted,
        )
        .is_err()
    );
    let duplicate = br#"{"schema":{"name":"org.synapsegit.lp-studio.public-target","version":1},"schema":{"name":"org.synapsegit.lp-studio.public-target","version":1}}"#;
    assert!(parse_reviewed_public_target_v1(duplicate).is_err());
}

#[test]
fn provider_layouts_share_canonical_deterministic_projection_and_views() {
    let temporary = TempDirectory::new("provider-layouts");
    let projection = build_generic_artifact_status_projection(
        &public_target("Deterministic target"),
        &TrustedGenericArtifactStatus::pending(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    let synapse = export_projection(
        &temporary,
        "synapse-first",
        &projection,
        GenericArtifactOutputTarget::Synapse,
    );
    let synapse_repeat = export_projection(
        &temporary,
        "synapse-repeat",
        &projection,
        GenericArtifactOutputTarget::Synapse,
    );
    let github = export_projection(
        &temporary,
        "github",
        &projection,
        GenericArtifactOutputTarget::Github,
    );

    for path in ["projection.json", "story.md", "index.html"] {
        assert_eq!(
            fs::read(synapse.join(path)).unwrap(),
            fs::read(github.join(path)).unwrap()
        );
    }
    assert_eq!(snapshot_tree(&synapse), snapshot_tree(&synapse_repeat));
    assert_ne!(
        fs::read(synapse.join("manifest.json")).unwrap(),
        fs::read(github.join("manifest.json")).unwrap()
    );
    let projection_bytes = fs::read(synapse.join("projection.json")).unwrap();
    assert_eq!(
        canonical_bytes(&parse_strict(&projection_bytes).unwrap()).unwrap(),
        projection_bytes
    );
    assert_eq!(
        verify_generic_artifact_bundle(&synapse)
            .unwrap()
            .manifest
            .schema
            .name,
        GENERIC_ARTIFACT_BUNDLE_SCHEMA
    );
    assert_eq!(
        verify_generic_artifact_bundle(&github)
            .unwrap()
            .projection
            .schema
            .name,
        GENERIC_ARTIFACT_PROJECTION_SCHEMA
    );
}

#[test]
fn verifier_rejects_reconciled_semantic_claims_and_unknown_profile_versions() {
    let temporary = TempDirectory::new("verification");
    let projection = build_generic_artifact_status_projection(
        &public_target("Verifier target"),
        &TrustedGenericArtifactStatus::pending(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    let authority_bundle = export_projection(
        &temporary,
        "authority-claim",
        &projection,
        GenericArtifactOutputTarget::Synapse,
    );
    let mut projection_json: JsonValue =
        serde_json::from_slice(&fs::read(authority_bundle.join("projection.json")).unwrap())
            .unwrap();
    projection_json["outcome"]["fact_origin"] = json!("verified_from_synapse");
    projection_json["outcome"]["human_disposition"] = json!("adopted_unchanged");
    let projection_bytes = canonical_json(&projection_json);
    fs::write(authority_bundle.join("projection.json"), &projection_bytes).unwrap();
    fs::write(
        authority_bundle.join("target/generic-artifact-public-projection.json"),
        &projection_bytes,
    )
    .unwrap();
    let mut manifest: GenericArtifactBundleManifestV1 =
        serde_json::from_slice(&fs::read(authority_bundle.join("manifest.json")).unwrap()).unwrap();
    manifest.projection_sha256 = sha256(&projection_bytes);
    fs::write(
        authority_bundle.join("manifest.json"),
        canonical_json(&manifest),
    )
    .unwrap();
    reconcile_checksums(&authority_bundle);
    assert!(matches!(
        verify_generic_artifact_bundle(&authority_bundle),
        Err(GenericArtifactPublicationError::InvalidBundle(_))
    ));

    let profile_bundle = export_projection(
        &temporary,
        "unknown-profile",
        &projection,
        GenericArtifactOutputTarget::Github,
    );
    let mut manifest: GenericArtifactBundleManifestV1 =
        serde_json::from_slice(&fs::read(profile_bundle.join("manifest.json")).unwrap()).unwrap();
    manifest.renderer_profile.version += 1;
    fs::write(
        profile_bundle.join("manifest.json"),
        canonical_json(&manifest),
    )
    .unwrap();
    reconcile_checksums(&profile_bundle);
    assert!(matches!(
        verify_generic_artifact_bundle(&profile_bundle),
        Err(GenericArtifactPublicationError::InvalidBundle(_))
    ));
}

#[test]
fn verifier_accepts_a_past_safe_generator_version_only_when_bundle_claims_match() {
    const PAST_SAFE_GENERATOR_VERSION: &str = "0.2.0";

    assert_ne!(PAST_SAFE_GENERATOR_VERSION, env!("CARGO_PKG_VERSION"));
    let temporary = TempDirectory::new("past-generator-version");
    let projection = build_generic_artifact_status_projection(
        &public_target("Past generator target"),
        &TrustedGenericArtifactStatus::pending(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    let bundle = export_projection(
        &temporary,
        "past-generator-version-bundle",
        &projection,
        GenericArtifactOutputTarget::Synapse,
    );

    let mut projection_json: JsonValue =
        serde_json::from_slice(&fs::read(bundle.join("projection.json")).unwrap()).unwrap();
    projection_json["generator"]["version"] = json!(PAST_SAFE_GENERATOR_VERSION);
    let projection_bytes = canonical_json(&projection_json);
    fs::write(bundle.join("projection.json"), &projection_bytes).unwrap();
    fs::write(
        bundle.join("target/generic-artifact-public-projection.json"),
        &projection_bytes,
    )
    .unwrap();

    let mut manifest: GenericArtifactBundleManifestV1 =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    manifest.projection_sha256 = sha256(&projection_bytes);
    fs::write(bundle.join("manifest.json"), canonical_json(&manifest)).unwrap();
    reconcile_checksums(&bundle);
    assert!(matches!(
        verify_generic_artifact_bundle(&bundle),
        Err(GenericArtifactPublicationError::InvalidBundle(_))
    ));

    manifest.generator.version = PAST_SAFE_GENERATOR_VERSION.into();
    fs::write(bundle.join("manifest.json"), canonical_json(&manifest)).unwrap();
    reconcile_checksums(&bundle);
    let verified = verify_generic_artifact_bundle(&bundle).unwrap();
    assert_eq!(
        verified.projection.generator.version,
        PAST_SAFE_GENERATOR_VERSION
    );
    assert_eq!(
        verified.manifest.generator.version,
        PAST_SAFE_GENERATOR_VERSION
    );

    projection_json["generator"]["version"] = json!("0.2.0/unsafe");
    let unsafe_projection_bytes = canonical_json(&projection_json);
    fs::write(bundle.join("projection.json"), &unsafe_projection_bytes).unwrap();
    fs::write(
        bundle.join("target/generic-artifact-public-projection.json"),
        &unsafe_projection_bytes,
    )
    .unwrap();
    manifest.generator.version = "0.2.0/unsafe".into();
    manifest.projection_sha256 = sha256(&unsafe_projection_bytes);
    fs::write(bundle.join("manifest.json"), canonical_json(&manifest)).unwrap();
    reconcile_checksums(&bundle);
    assert!(matches!(
        verify_generic_artifact_bundle(&bundle),
        Err(GenericArtifactPublicationError::InvalidBundle(_))
    ));
}

#[test]
fn public_schemas_accept_all_outcomes_and_golden_identity_constants_are_frozen() {
    let spec_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/application/generic-artifact-publication/v1");
    let projection_schema: JsonValue =
        serde_json::from_slice(&fs::read(spec_root.join("projection.schema.json")).unwrap())
            .unwrap();
    let target_schema: JsonValue =
        serde_json::from_slice(&fs::read(spec_root.join("public-target.schema.json")).unwrap())
            .unwrap();
    let manifest_schema: JsonValue =
        serde_json::from_slice(&fs::read(spec_root.join("bundle-manifest.schema.json")).unwrap())
            .unwrap();
    let checksums_schema: JsonValue =
        serde_json::from_slice(&fs::read(spec_root.join("checksums.schema.json")).unwrap())
            .unwrap();
    let projection_validator = jsonschema::draft202012::new(&projection_schema).unwrap();
    let target_validator = jsonschema::draft202012::new(&target_schema).unwrap();
    let manifest_validator = jsonschema::draft202012::new(&manifest_schema).unwrap();
    let checksums_validator = jsonschema::draft202012::new(&checksums_schema).unwrap();
    let target = public_target("Schema target");
    assert!(target_validator.is_valid(&serde_json::to_value(&target).unwrap()));
    let unicode_boundary_target = ReviewedPublicTargetV1::new(
        "0.0.0",
        "unicode-boundary",
        PublicTargetKindV1::Element,
        "界".repeat(300),
        PublicTargetCaptureSourceV1::Accepted,
    )
    .unwrap();
    let mut oversized_unicode_target = serde_json::to_value(&unicode_boundary_target).unwrap();
    assert!(target_validator.is_valid(&oversized_unicode_target));
    oversized_unicode_target["target"]["label"] = json!("界".repeat(301));
    assert!(!target_validator.is_valid(&oversized_unicode_target));

    let temporary = TempDirectory::new("schemas");
    let complete_binding = complete_binding(
        &temporary.0,
        "schema-complete",
        ArtifactDisposition::AdoptedUnchanged,
    );
    let complete = build_generic_artifact_complete_projection(
        &target,
        &complete_binding,
        ArtifactCheckoutLimits::default(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    assert!(projection_validator.is_valid(&serde_json::to_value(&complete).unwrap()));
    let mut invalid_product_version = serde_json::to_value(&complete).unwrap();
    invalid_product_version["contracts"]["lp_studio"]["product_version"] = json!("0.0.0/unsafe");
    assert!(!projection_validator.is_valid(&invalid_product_version));
    let complete_bundle = export_projection(
        &temporary,
        "schema-complete-bundle",
        &complete,
        GenericArtifactOutputTarget::Synapse,
    );
    let complete_manifest: JsonValue =
        serde_json::from_slice(&fs::read(complete_bundle.join("manifest.json")).unwrap()).unwrap();
    let complete_checksums: JsonValue =
        serde_json::from_slice(&fs::read(complete_bundle.join("checksums.json")).unwrap()).unwrap();
    assert!(manifest_validator.is_valid(&complete_manifest));
    assert!(checksums_validator.is_valid(&complete_checksums));

    let outcomes = [
        TrustedGenericArtifactStatus::pending(),
        TrustedGenericArtifactStatus::incomplete(GenericArtifactStatusReason::OutcomeUnknown)
            .unwrap(),
    ];
    for (index, status) in outcomes.iter().enumerate() {
        let projection = build_generic_artifact_status_projection(
            &target,
            status,
            GenericArtifactVisibility::Public,
        )
        .unwrap();
        assert!(projection_validator.is_valid(&serde_json::to_value(&projection).unwrap()));
        let bundle = export_projection(
            &temporary,
            &format!("schema-{index}"),
            &projection,
            GenericArtifactOutputTarget::Synapse,
        );
        let manifest: JsonValue =
            serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
        let checksums: JsonValue =
            serde_json::from_slice(&fs::read(bundle.join("checksums.json")).unwrap()).unwrap();
        assert!(manifest_validator.is_valid(&manifest));
        assert!(checksums_validator.is_valid(&checksums));
    }

    assert_eq!(GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION, 1);
    assert_eq!(GENERIC_ARTIFACT_BUNDLE_SCHEMA_VERSION, 1);
    assert_eq!(GENERIC_ARTIFACT_CHECKSUMS_SCHEMA_VERSION, 1);
    assert_eq!(
        GENERIC_ARTIFACT_PROJECTION_SCHEMA,
        "org.synapsegit.generic-artifact-public-projection"
    );
    assert_eq!(
        GENERIC_ARTIFACT_BUNDLE_SCHEMA,
        "org.synapsegit.generic-artifact-publication-bundle"
    );
    assert_eq!(
        GENERIC_ARTIFACT_CHECKSUMS_SCHEMA,
        "org.synapsegit.generic-artifact-publication-checksums"
    );
}

#[test]
fn malformed_complete_projection_is_rejected_before_export() {
    let target = public_target("Malformed target");
    let mut projection = build_generic_artifact_status_projection(
        &target,
        &TrustedGenericArtifactStatus::pending(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    projection.outcome.state = GenericArtifactOutcomeState::Complete;
    assert!(validate_generic_artifact_projection(&projection).is_err());
}

#[test]
fn generic_artifact_golden_vectors_are_frozen() {
    let expected: JsonValue = serde_json::from_slice(
        &fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../spec/application/generic-artifact-publication/v1/golden-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let frozen_generator: GenericArtifactGeneratorIdentity =
        serde_json::from_value(expected["generator"].clone()).unwrap();
    let temporary = TempDirectory::new("golden-vectors");
    let target = public_target("Golden target");
    let binding = complete_binding(
        &temporary.0,
        "golden-complete",
        ArtifactDisposition::AdoptedUnchanged,
    );
    let complete = build_generic_artifact_complete_projection(
        &target,
        &binding,
        ArtifactCheckoutLimits::default(),
        GenericArtifactVisibility::Public,
    )
    .unwrap();
    let pending = build_generic_artifact_status_projection(
        &target,
        &TrustedGenericArtifactStatus::pending(),
        GenericArtifactVisibility::PrivateReview,
    )
    .unwrap();
    let incomplete = build_generic_artifact_status_projection(
        &target,
        &TrustedGenericArtifactStatus::incomplete(GenericArtifactStatusReason::OutcomeUnknown)
            .unwrap(),
        GenericArtifactVisibility::PrivateReview,
    )
    .unwrap();

    let cases = [
        ("complete_adopted", complete),
        ("pending", pending),
        ("incomplete_outcome_unknown", incomplete),
    ];
    let mut actual_cases = Vec::new();
    for (index, (name, mut projection)) in cases.into_iter().enumerate() {
        assert_eq!(projection.generator.name, "synapse-publication");
        assert_eq!(projection.generator.version, env!("CARGO_PKG_VERSION"));
        let bundle = export_projection(
            &temporary,
            &format!("golden-{index}"),
            &projection,
            GenericArtifactOutputTarget::Synapse,
        );
        let projection_bytes = fs::read(bundle.join("projection.json")).unwrap();
        assert_eq!(projection_bytes, canonical_json(&projection));
        let manifest: GenericArtifactBundleManifestV1 =
            serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.generator, projection.generator);
        assert_eq!(manifest.projection_sha256, sha256(&projection_bytes));
        projection.generator = frozen_generator.clone();
        actual_cases.push(json!({
            "name": name,
            "projection_sha256": sha256(&canonical_json(&projection)),
            "story_sha256": sha256(&fs::read(bundle.join("story.md")).unwrap()),
            "html_sha256": sha256(&fs::read(bundle.join("index.html")).unwrap())
        }));
    }
    let actual = json!({
        "profile": {
            "name": GENERIC_ARTIFACT_PUBLICATION_PROFILE,
            "version": GENERIC_ARTIFACT_PUBLICATION_PROFILE_VERSION
        },
        "projection_schema": {
            "name": GENERIC_ARTIFACT_PROJECTION_SCHEMA,
            "version": GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION
        },
        "renderer_profile": {
            "name": GENERIC_ARTIFACT_RENDERER_PROFILE,
            "version": GENERIC_ARTIFACT_RENDERER_PROFILE_VERSION
        },
        "generator": frozen_generator,
        "cases": actual_cases
    });
    assert_eq!(actual, expected);
}
