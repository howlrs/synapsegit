//! Trusted, bounded checkout of the regular files selected by one generic Decision.
//!
//! This is a control-plane boundary. The binding must be reconstructed only
//! after the embedding service has authenticated and authorized the project;
//! request data must never supply repository paths, Refs, heads, or OIDs.
//! Only the selected snapshot's direct `site` Tree is recursively materialized
//! as output. Protected `base`/`control` Trees and their fixed Records are read
//! solely to prove authority lineage; control Blob bytes are never read.

use crate::{
    ArtifactDisposition, ArtifactLimits, ArtifactManifestEntry, RegularFileManifest,
    artifact_manifest_sha256,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use synapse_application::DurableProposalBinding;
use synapse_canonical::{ObjectKind, Value, parse_oid};
use synapse_core::{RefSnapshot, Repository};
use unicode_normalization::UnicodeNormalization;

const DEFAULT_MAX_REF_SNAPSHOT_ENTRIES: usize = 100_000;
const DEFAULT_MAX_TREE_NODES: usize = 100_000;
const DEFAULT_MAX_TREE_EDGES: usize = 200_000;
const DEFAULT_MAX_AUTHORITY_NODES: usize = 100_000;
const DEFAULT_MAX_AUTHORITY_EDGES: usize = 200_000;

/// Trusted, non-serializable authority for one completed generic Decision.
///
/// The contained proposal binding identifies the exact proposal and its
/// canonical base. `decision_head` is the completed Decision Commit expected
/// at the bound Decision Ref. `reviewed_manifest_sha256` is copied from the
/// trusted Decision outcome journal/receipt and is rechecked over checkout
/// bytes before any result is returned.
#[derive(Clone, Eq, PartialEq)]
pub struct TrustedArtifactDecisionBinding {
    repository: PathBuf,
    project_key: String,
    proposal: DurableProposalBinding,
    decision_head: String,
    disposition: ArtifactDisposition,
    reviewed_manifest_sha256: String,
}

impl TrustedArtifactDecisionBinding {
    pub fn new(
        repository: impl Into<PathBuf>,
        project_key: impl Into<String>,
        proposal: DurableProposalBinding,
        decision_head: impl Into<String>,
        disposition: ArtifactDisposition,
        reviewed_manifest_sha256: impl Into<String>,
    ) -> Self {
        Self {
            repository: repository.into(),
            project_key: project_key.into(),
            proposal,
            decision_head: decision_head.into(),
            disposition,
            reviewed_manifest_sha256: reviewed_manifest_sha256.into(),
        }
    }

    pub const fn disposition(&self) -> ArtifactDisposition {
        self.disposition
    }
}

impl fmt::Debug for TrustedArtifactDecisionBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TrustedArtifactDecisionBinding(<redacted trusted binding>)")
    }
}

/// Independent deployment limits for one whole-result checkout.
///
/// The `artifact` limits retain the frozen regular-file path/byte profile.
/// Tree node and edge limits charge actual recursive work below the selected
/// snapshot's direct `site` entry, including repeated traversal of a shared
/// Tree under distinct paths. Authority node and edge limits are cumulative
/// across prior-Decision lineage and protected-control reachability proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactCheckoutLimits {
    pub artifact: ArtifactLimits,
    pub max_ref_snapshot_entries: usize,
    pub max_tree_nodes: usize,
    pub max_tree_edges: usize,
    pub max_authority_nodes: usize,
    pub max_authority_edges: usize,
}

impl Default for ArtifactCheckoutLimits {
    fn default() -> Self {
        Self {
            artifact: ArtifactLimits::default(),
            max_ref_snapshot_entries: DEFAULT_MAX_REF_SNAPSHOT_ENTRIES,
            max_tree_nodes: DEFAULT_MAX_TREE_NODES,
            max_tree_edges: DEFAULT_MAX_TREE_EDGES,
            max_authority_nodes: DEFAULT_MAX_AUTHORITY_NODES,
            max_authority_edges: DEFAULT_MAX_AUTHORITY_EDGES,
        }
    }
}

/// A complete, digest-checked regular-file checkout.
///
/// Files are retained in normalized bytewise path order. This type is created
/// only after the full traversal, regular-file manifest validation, and trusted
/// digest comparison succeed, so an error can never be mistaken for a partial
/// successful result.
#[derive(Clone, Eq, PartialEq)]
pub struct CheckedOutArtifact {
    disposition: ArtifactDisposition,
    manifest_sha256: String,
    files: Vec<ArtifactManifestEntry>,
    total_bytes: u64,
}

impl CheckedOutArtifact {
    pub const fn disposition(&self) -> ArtifactDisposition {
        self.disposition
    }

    pub const fn selected_snapshot(&self) -> &'static str {
        match self.disposition {
            ArtifactDisposition::AdoptedUnchanged => "proposal",
            ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => "base",
        }
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn files(&self) -> impl ExactSizeIterator<Item = &ArtifactManifestEntry> {
        self.files.iter()
    }

    pub fn bytes(&self, normalized_path: &str) -> Option<&[u8]> {
        self.files
            .binary_search_by(|entry| entry.path().as_bytes().cmp(normalized_path.as_bytes()))
            .ok()
            .map(|index| self.files[index].bytes())
    }
}

impl fmt::Debug for CheckedOutArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckedOutArtifact")
            .field("disposition", &self.disposition)
            .field("manifest_sha256", &self.manifest_sha256)
            .field("file_count", &self.files.len())
            .field("total_bytes", &self.total_bytes)
            .field("paths", &"<redacted>")
            .field("contents", &"<redacted>")
            .finish()
    }
}

/// Stable, redacted checkout failure.
#[derive(Clone, Eq, PartialEq)]
pub struct ArtifactCheckoutError {
    code: &'static str,
    message: &'static str,
}

impl ArtifactCheckoutError {
    pub const fn code(&self) -> &'static str {
        self.code
    }

    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl fmt::Debug for ArtifactCheckoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactCheckoutError")
            .field("code", &self.code)
            .field("detail", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for ArtifactCheckoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for ArtifactCheckoutError {}

pub type CheckoutResult<T> = std::result::Result<T, ArtifactCheckoutError>;

/// Reopen the bound repository read-only and return exactly the selected
/// generic `site` regular files as one complete verified result.
///
/// Authority verification reads only fixed protected metadata Records. The
/// recursive file walk is confined to `site`, and no control Blob is opened.
pub fn checkout_artifact_decision(
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
) -> CheckoutResult<CheckedOutArtifact> {
    let repository = Repository::open_existing_read_only(&binding.repository)
        .map_err(|error| external_error(Some(error.code())))?;
    checkout_artifact_decision_in_repository(&repository, binding, limits)
}

/// Crate-internal equivalent used while a trusted workflow already owns an
/// open repository. It retains the same one-snapshot Ref and graph checks.
pub(crate) fn checkout_artifact_decision_in_repository(
    repository: &Repository,
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
) -> CheckoutResult<CheckedOutArtifact> {
    validate_limits(limits)?;
    validate_binding(binding)?;
    let refs = repository
        .refs()
        .snapshot_limited(limits.max_ref_snapshot_entries)
        .map_err(|error| external_error(Some(error.code())))?;

    // Every Ref lookup below is against this one immutable SQLite snapshot.
    require_ref_head(
        &refs,
        binding.proposal.proposal_ref_name(),
        binding.proposal.proposal_head(),
    )?;
    require_ref_head(
        &refs,
        binding.proposal.decision_ref_name(),
        &binding.decision_head,
    )?;

    let (manifest, digest) = verify_selected_artifact(repository, binding, limits)?;
    let total_bytes = manifest.total_bytes();
    let files = manifest
        .files
        .into_iter()
        .map(|(path, bytes)| ArtifactManifestEntry::regular_file(path, bytes))
        .collect();
    Ok(CheckedOutArtifact {
        disposition: binding.disposition,
        manifest_sha256: digest,
        files,
        total_bytes,
    })
}

/// Verify a completed historical Decision without returning its selected bytes.
///
/// The caller must first prove, against this exact `refs` snapshot, that the
/// current canonical Decision is a valid descendant of `binding.decision_head`.
/// Unlike public checkout this deliberately does not require the Decision Ref
/// to equal the historical head, but it retains the exact Proposal Ref gate and
/// performs the complete graph, authority, bounded site, and digest checks.
pub(crate) fn verify_historical_artifact_decision_in_repository(
    repository: &Repository,
    refs: &RefSnapshot,
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
) -> CheckoutResult<()> {
    validate_limits(limits)?;
    validate_binding(binding)?;
    if refs.refs.len() > limits.max_ref_snapshot_entries {
        return Err(resource_limit());
    }
    require_ref_head(
        refs,
        binding.proposal.proposal_ref_name(),
        binding.proposal.proposal_head(),
    )?;
    let _ = verify_selected_artifact(repository, binding, limits)?;
    Ok(())
}

fn verify_selected_artifact(
    repository: &Repository,
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
) -> CheckoutResult<(RegularFileManifest, String)> {
    let selected_site = verify_generic_decision(repository, binding, limits)?;
    let mut files = read_site(repository, &selected_site, limits)?;
    files.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));

    // Reuse the mapper's frozen validation/digest profile, then move the
    // validated map directly into the checkout result without duplicating all
    // file contents in memory.
    let manifest =
        RegularFileManifest::from_entries(files, limits.artifact).map_err(artifact_error)?;
    let digest = artifact_manifest_sha256(&manifest);
    if digest != binding.reviewed_manifest_sha256 {
        return Err(ArtifactCheckoutError::new(
            "artifact_digest_mismatch",
            "checked out artifact does not match the trusted Decision digest",
        ));
    }
    Ok((manifest, digest))
}

fn validate_limits(limits: ArtifactCheckoutLimits) -> CheckoutResult<()> {
    if limits.max_ref_snapshot_entries == 0
        || limits.max_tree_nodes == 0
        || limits.max_tree_edges == 0
        || limits.max_authority_nodes == 0
        || limits.max_authority_edges == 0
        || limits.artifact.max_files == 0
        || limits.artifact.max_file_bytes == 0
        || limits.artifact.max_total_bytes == 0
        || limits.artifact.max_path_bytes == 0
        || limits.artifact.max_depth == 0
    {
        return Err(ArtifactCheckoutError::new(
            "artifact_limits_invalid",
            "artifact checkout limits must be positive",
        ));
    }
    Ok(())
}

fn validate_binding(binding: &TrustedArtifactDecisionBinding) -> CheckoutResult<()> {
    let values = [
        binding.proposal.project().as_str(),
        binding.project_key.as_str(),
        binding.proposal.proposal_ref_name(),
        binding.proposal.proposal_head(),
        binding.proposal.decision_ref_name(),
        binding.proposal.decision_head(),
        binding.decision_head.as_str(),
        binding.reviewed_manifest_sha256.as_str(),
    ];
    if binding.repository.as_os_str().is_empty()
        || values.into_iter().any(|value| {
            value.is_empty() || value.len() > 1_024 || value.chars().any(char::is_control)
        })
        || !valid_sha256(&binding.reviewed_manifest_sha256)
    {
        return Err(binding_invalid());
    }

    for oid in [
        binding.proposal.proposal_head(),
        binding.proposal.decision_head(),
        binding.decision_head.as_str(),
    ] {
        if parse_oid(oid).ok() != Some(ObjectKind::Commit) {
            return Err(binding_invalid());
        }
    }

    if !valid_project_key(&binding.project_key) {
        return Err(binding_invalid());
    }
    let expected_decision_ref = format!("decision/artifact/{}", binding.project_key);
    if binding.proposal.decision_ref_name() != expected_decision_ref {
        return Err(binding_invalid());
    }
    let Some(proposal_suffix) = binding
        .proposal
        .proposal_ref_name()
        .strip_prefix("proposal/artifact/")
    else {
        return Err(binding_invalid());
    };
    if proposal_suffix != binding.project_key
        && !proposal_suffix
            .strip_prefix(&binding.project_key)
            .is_some_and(|suffix| suffix.starts_with('/'))
    {
        return Err(binding_invalid());
    }
    Ok(())
}

fn valid_project_key(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 128
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn require_ref_head(snapshot: &RefSnapshot, name: &str, expected: &str) -> CheckoutResult<()> {
    match snapshot
        .refs
        .binary_search_by(|record| record.name.as_str().cmp(name))
    {
        Ok(index) if snapshot.refs[index].head == expected => Ok(()),
        _ => Err(ArtifactCheckoutError::new(
            "stale_base",
            "trusted artifact Decision binding is stale",
        )),
    }
}

fn verify_generic_decision(
    repository: &Repository,
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
) -> CheckoutResult<String> {
    let base_head = binding.proposal.decision_head();
    let proposal_head = binding.proposal.proposal_head();
    let decision = load_structured(repository, &binding.decision_head, ObjectKind::Commit)?;
    require_object_type(&decision, "commit")?;
    require_equal(require_string(&decision, "commit_kind")?, "decision")?;
    require_exact_string_array(&decision, "parents", &[base_head])?;
    require_empty_array(&decision, "bound_declaration_refs")?;
    let feedback_oid = require_single_oid(&decision, "transition_refs", ObjectKind::Record)?;
    let selected_snapshot = require_oid(&decision, "snapshot", ObjectKind::Tree)?;

    let feedback = load_structured(repository, feedback_oid, ObjectKind::Record)?;
    require_record_type(&feedback, "decision_feedback")?;
    require_equal(require_string(&feedback, "origin")?, "self_declared")?;
    let feedback_payload = require_object_field(&feedback, "payload")?;
    require_equal(
        require_string(feedback_payload, "proposal_ref")?,
        proposal_head,
    )?;
    require_equal(
        require_string(feedback_payload, "disposition")?,
        disposition_name(binding.disposition),
    )?;
    let human_actor = require_string(&feedback, "asserted_by")?;
    require_equal(require_string(&decision, "author_ref")?, human_actor)?;

    let base = load_structured(repository, base_head, ObjectKind::Commit)?;
    require_object_type(&base, "commit")?;
    require_equal(require_string(&base, "author_ref")?, human_actor)?;
    require_empty_array(&base, "bound_declaration_refs")?;
    match require_string(&base, "commit_kind")? {
        "checkpoint" => {
            require_empty_array(&base, "parents")?;
            require_empty_array(&base, "transition_refs")?;
        }
        "decision" => {
            if require_array(&base, "parents")?.len() != 1 {
                return Err(integrity());
            }
            let _ = require_single_oid(&base, "transition_refs", ObjectKind::Record)?;
        }
        _ => return Err(integrity()),
    }
    let base_snapshot = require_oid(&base, "snapshot", ObjectKind::Tree)?;
    let base_snapshot_value = load_structured(repository, base_snapshot, ObjectKind::Tree)?;
    require_tree(&base_snapshot_value)?;
    let base_site = direct_entry_oid(&base_snapshot_value, "site", ObjectKind::Tree)?;

    let proposal = load_structured(repository, proposal_head, ObjectKind::Commit)?;
    require_object_type(&proposal, "commit")?;
    require_equal(require_string(&proposal, "commit_kind")?, "checkpoint")?;
    require_exact_string_array(&proposal, "parents", &[base_head])?;
    require_empty_array(&proposal, "bound_declaration_refs")?;
    let activity_oid = require_single_oid(&proposal, "transition_refs", ObjectKind::Record)?;
    let proposal_snapshot = require_oid(&proposal, "snapshot", ObjectKind::Tree)?;
    let proposal_snapshot_value = load_structured(repository, proposal_snapshot, ObjectKind::Tree)?;
    require_tree(&proposal_snapshot_value)?;
    require_exact_tree_entries(
        &proposal_snapshot_value,
        &["activity.json", "base", "context.json", "site"],
    )?;
    let proposal_site = direct_entry_oid(&proposal_snapshot_value, "site", ObjectKind::Tree)?;
    require_equal(
        direct_entry_oid(&proposal_snapshot_value, "base", ObjectKind::Tree)?,
        base_snapshot,
    )?;
    let context_oid =
        direct_entry_oid(&proposal_snapshot_value, "context.json", ObjectKind::Record)?;
    require_equal(
        direct_entry_oid(
            &proposal_snapshot_value,
            "activity.json",
            ObjectKind::Record,
        )?,
        activity_oid,
    )?;

    let mut authority_work = AuthorityWork::default();
    verify_context_and_activity(
        repository,
        binding,
        base_head,
        base_snapshot,
        context_oid,
        activity_oid,
        proposal_site,
        &proposal,
        human_actor,
        limits,
        &mut authority_work,
    )?;

    let expected_snapshot = match binding.disposition {
        ArtifactDisposition::AdoptedUnchanged => proposal_snapshot,
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => base_snapshot,
    };
    require_equal(selected_snapshot, expected_snapshot)?;

    Ok(match binding.disposition {
        ArtifactDisposition::AdoptedUnchanged => proposal_site.to_owned(),
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => base_site.to_owned(),
    })
}

#[allow(clippy::too_many_arguments)]
fn verify_context_and_activity(
    repository: &Repository,
    binding: &TrustedArtifactDecisionBinding,
    base_head: &str,
    base_snapshot: &str,
    context_oid: &str,
    activity_oid: &str,
    proposal_site: &str,
    proposal: &Value,
    human_actor: &str,
    limits: ArtifactCheckoutLimits,
    authority_work: &mut AuthorityWork,
) -> CheckoutResult<()> {
    let context = load_structured(repository, context_oid, ObjectKind::Record)?;
    require_record_type(&context, "context_pack")?;
    require_equal(require_string(&context, "origin")?, "tool_recorded")?;
    require_equal(require_string(&context, "asserted_by")?, human_actor)?;
    let context_payload = require_object_field(&context, "payload")?;
    for (field, expected) in [
        ("base_commit", base_head),
        ("expected_ref_head", base_head),
        ("base_ref_name", binding.proposal.decision_ref_name()),
    ] {
        require_equal(require_string(context_payload, field)?, expected)?;
    }
    require_array_contains_string(context_payload, "selected_context_refs", base_head)?;
    if require_array(context_payload, "selected_context_refs")?.len() != 2
        || require_array(context_payload, "subject_refs")?.len() != 1
    {
        return Err(integrity());
    }
    let policy_oid = require_oid(context_payload, "policy_snapshot_ref", ObjectKind::Record)?;
    let grant_oid = require_oid(context_payload, "delegation_grant_ref", ObjectKind::Record)?;

    let policy = load_structured(repository, policy_oid, ObjectKind::Record)?;
    require_record_type(&policy, "policy")?;
    require_equal(require_string(&policy, "origin")?, "self_declared")?;
    require_equal(require_string(&policy, "asserted_by")?, human_actor)?;
    let policy_payload = require_object_field(&policy, "payload")?;
    require_array_contains_string(
        policy_payload,
        "scope_refs",
        binding.proposal.project().as_str(),
    )?;
    require_publish_gate(policy_payload, binding.proposal.decision_ref_name())?;
    require_equal(require_string(policy_payload, "default_effect")?, "deny")?;

    let grant = load_structured(repository, grant_oid, ObjectKind::Record)?;
    require_record_type(&grant, "delegation_grant")?;
    require_equal(require_string(&grant, "origin")?, "self_declared")?;
    require_equal(require_string(&grant, "asserted_by")?, human_actor)?;
    let grant_payload = require_object_field(&grant, "payload")?;
    require_equal(
        require_string(grant_payload, "project_ref")?,
        binding.proposal.project().as_str(),
    )?;
    require_equal(require_string(grant_payload, "principal_ref")?, human_actor)?;
    require_array_contains_string(
        grant_payload,
        "data_classes",
        require_string(context_payload, "data_classification")?,
    )?;
    require_array_contains_string(grant_payload, "resource_selectors", "project/**")?;
    require_array_contains_string(grant_payload, "required_human_gates", "before_decision_ref")?;
    require_writable_ref(grant_payload, binding.proposal.proposal_ref_name())?;

    let activity = load_structured(repository, activity_oid, ObjectKind::Record)?;
    require_record_type(&activity, "activity")?;
    require_equal(require_string(&activity, "origin")?, "tool_recorded")?;
    require_equal(
        require_string(proposal, "author_ref")?,
        require_string(&activity, "asserted_by")?,
    )?;
    let activity_payload = require_object_field(&activity, "payload")?;
    require_equal(require_string(activity_payload, "activity_kind")?, "ai_run")?;
    require_equal(
        require_string(activity_payload, "side_effect_class")?,
        "none",
    )?;
    if require_array(activity_payload, "input_refs")?.len() != 2
        || require_array(activity_payload, "output_refs")?.len() != 1
        || require_array(activity_payload, "actor_refs")?.len() != 2
        || require_array(activity_payload, "subject_refs")?.len() != 1
    {
        return Err(integrity());
    }
    require_role_oid(activity_payload, "input_refs", "context", context_oid)?;
    let outputs = require_array(activity_payload, "output_refs")?;
    if outputs.len() != 1 {
        return Err(integrity());
    }
    let output = outputs[0].as_object().ok_or_else(integrity)?;
    require_equal(object_string(output, "role")?, "proposal")?;
    require_equal(object_string(output, "oid")?, proposal_site)?;

    let ai_run = require_object_field(activity_payload, "ai_run")?;
    require_equal(
        require_oid(ai_run, "context_pack_ref", ObjectKind::Record)?,
        context_oid,
    )?;
    require_equal(
        require_oid(ai_run, "delegation_grant_ref", ObjectKind::Record)?,
        grant_oid,
    )?;
    require_equal(require_string(ai_run, "status")?, "proposal_ready")?;
    require_array_contains_string(ai_run, "required_human_gates", "before_decision_ref")?;
    require_equal(
        require_string(ai_run, "agent_ref")?,
        require_string(&activity, "asserted_by")?,
    )?;
    require_equal(
        require_string(ai_run, "responsible_principal_ref")?,
        human_actor,
    )?;
    require_role_actor(
        activity_payload,
        "agent",
        require_string(ai_run, "agent_ref")?,
    )?;
    require_role_actor(activity_payload, "responsible_principal", human_actor)?;
    require_equal(
        require_string(grant_payload, "delegate_ref")?,
        require_string(&activity, "asserted_by")?,
    )?;
    let ai_actor = require_string(&activity, "asserted_by")?;
    let subject_refs = require_array(context_payload, "subject_refs")?;
    if subject_refs.len() != 1 {
        return Err(integrity());
    }
    let subject = subject_refs[0].as_str().ok_or_else(integrity)?;
    verify_prior_decision_chain(
        repository,
        base_head,
        binding.proposal.decision_ref_name(),
        human_actor,
        ai_actor,
        policy_oid,
        grant_oid,
        limits,
        authority_work,
    )?;
    verify_base_authority(
        repository,
        base_snapshot,
        policy_oid,
        grant_oid,
        human_actor,
        ai_actor,
        subject,
        binding,
        limits,
        authority_work,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_prior_decision_chain(
    repository: &Repository,
    base_head: &str,
    decision_ref: &str,
    human_actor: &str,
    ai_actor: &str,
    policy_oid: &str,
    grant_oid: &str,
    limits: ArtifactCheckoutLimits,
    work: &mut AuthorityWork,
) -> CheckoutResult<()> {
    let mut cursor = base_head.to_owned();
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(cursor.clone()) {
            return Err(integrity());
        }
        work.node(limits)?;
        let commit = load_structured(repository, &cursor, ObjectKind::Commit)?;
        require_object_type(&commit, "commit")?;
        require_empty_array(&commit, "bound_declaration_refs")?;
        require_equal(require_string(&commit, "author_ref")?, human_actor)?;
        let snapshot_oid = require_oid(&commit, "snapshot", ObjectKind::Tree)?;
        match require_string(&commit, "commit_kind")? {
            "checkpoint" => {
                require_empty_array(&commit, "parents")?;
                require_empty_array(&commit, "transition_refs")?;
                work.node(limits)?;
                let root = load_structured(repository, snapshot_oid, ObjectKind::Tree)?;
                require_tree(&root)?;
                require_exact_tree_entries(&root, &["control", "site"])?;
                work.edges(2, limits)?;
                let _ = direct_entry_oid(&root, "control", ObjectKind::Tree)?;
                let _ = direct_entry_oid(&root, "site", ObjectKind::Tree)?;
                return Ok(());
            }
            "decision" => {}
            _ => return Err(integrity()),
        }

        let parents = require_array(&commit, "parents")?;
        if parents.len() != 1 {
            return Err(integrity());
        }
        let parent = parents[0].as_str().ok_or_else(integrity)?;
        if parse_oid(parent).ok() != Some(ObjectKind::Commit) {
            return Err(integrity());
        }
        let feedback_oid = require_single_oid(&commit, "transition_refs", ObjectKind::Record)?;
        work.edges(2, limits)?;
        work.node(limits)?;
        let feedback = load_structured(repository, feedback_oid, ObjectKind::Record)?;
        require_record_type(&feedback, "decision_feedback")?;
        require_equal(require_string(&feedback, "origin")?, "self_declared")?;
        require_equal(require_string(&feedback, "asserted_by")?, human_actor)?;
        let feedback_payload = require_object_field(&feedback, "payload")?;
        let proposal_head = require_string(feedback_payload, "proposal_ref")?;
        if parse_oid(proposal_head).ok() != Some(ObjectKind::Commit) {
            return Err(integrity());
        }
        let disposition = require_string(feedback_payload, "disposition")?;

        work.node(limits)?;
        let proposal = load_structured(repository, proposal_head, ObjectKind::Commit)?;
        require_object_type(&proposal, "commit")?;
        require_equal(require_string(&proposal, "commit_kind")?, "checkpoint")?;
        require_exact_string_array(&proposal, "parents", &[parent])?;
        require_empty_array(&proposal, "bound_declaration_refs")?;
        require_equal(require_string(&proposal, "author_ref")?, ai_actor)?;
        let activity_oid = require_single_oid(&proposal, "transition_refs", ObjectKind::Record)?;
        let proposal_snapshot = require_oid(&proposal, "snapshot", ObjectKind::Tree)?;

        work.node(limits)?;
        let proposal_tree = load_structured(repository, proposal_snapshot, ObjectKind::Tree)?;
        require_tree(&proposal_tree)?;
        require_exact_tree_entries(
            &proposal_tree,
            &["activity.json", "base", "context.json", "site"],
        )?;
        work.edges(4, limits)?;
        require_equal(
            direct_entry_oid(&proposal_tree, "activity.json", ObjectKind::Record)?,
            activity_oid,
        )?;
        let context_oid = direct_entry_oid(&proposal_tree, "context.json", ObjectKind::Record)?;
        let proposal_site = direct_entry_oid(&proposal_tree, "site", ObjectKind::Tree)?;

        work.node(limits)?;
        let parent_commit = load_structured(repository, parent, ObjectKind::Commit)?;
        let parent_snapshot = require_oid(&parent_commit, "snapshot", ObjectKind::Tree)?;
        require_equal(
            direct_entry_oid(&proposal_tree, "base", ObjectKind::Tree)?,
            parent_snapshot,
        )?;
        let expected_selected = match disposition {
            "adopted_unchanged" => proposal_snapshot,
            "rejected" | "deferred" => parent_snapshot,
            _ => return Err(integrity()),
        };
        require_equal(snapshot_oid, expected_selected)?;

        work.node(limits)?;
        let context = load_structured(repository, context_oid, ObjectKind::Record)?;
        require_record_type(&context, "context_pack")?;
        require_equal(require_string(&context, "asserted_by")?, human_actor)?;
        let context_payload = require_object_field(&context, "payload")?;
        for (field, expected) in [
            ("base_commit", parent),
            ("expected_ref_head", parent),
            ("base_ref_name", decision_ref),
            ("policy_snapshot_ref", policy_oid),
            ("delegation_grant_ref", grant_oid),
        ] {
            require_equal(require_string(context_payload, field)?, expected)?;
        }

        work.node(limits)?;
        let activity = load_structured(repository, activity_oid, ObjectKind::Record)?;
        require_record_type(&activity, "activity")?;
        require_equal(require_string(&activity, "asserted_by")?, ai_actor)?;
        let activity_payload = require_object_field(&activity, "payload")?;
        require_role_oid(activity_payload, "input_refs", "context", context_oid)?;
        require_role_oid(activity_payload, "output_refs", "proposal", proposal_site)?;
        let ai_run = require_object_field(activity_payload, "ai_run")?;
        require_equal(
            require_oid(ai_run, "delegation_grant_ref", ObjectKind::Record)?,
            grant_oid,
        )?;
        cursor = parent.to_owned();
    }
}

fn require_role_oid(
    value: &Value,
    field: &str,
    expected_role: &str,
    expected_oid: &str,
) -> CheckoutResult<()> {
    if require_array(value, field)?.iter().any(|entry| {
        entry.get("role").and_then(Value::as_str) == Some(expected_role)
            && entry.get("oid").and_then(Value::as_str) == Some(expected_oid)
    }) {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_role_actor(
    value: &Value,
    expected_role: &str,
    expected_actor: &str,
) -> CheckoutResult<()> {
    if require_array(value, "actor_refs")?.iter().any(|entry| {
        entry.get("role").and_then(Value::as_str) == Some(expected_role)
            && entry.get("actor_ref").and_then(Value::as_str) == Some(expected_actor)
    }) {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_publish_gate(policy_payload: &Value, decision_ref: &str) -> CheckoutResult<()> {
    let rules = require_array(policy_payload, "rules")?;
    if rules.iter().any(|rule| {
        rule.get("effect").and_then(Value::as_str) == Some("require_human_gate")
            && rule.get("action").and_then(Value::as_str) == Some("publish")
            && rule.get("resource_selector").and_then(Value::as_str) == Some(decision_ref)
            && rule.get("human_gate").and_then(Value::as_str) == Some("before_decision_ref")
    }) {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_proposal_rule(policy_payload: &Value, proposal_selector: &str) -> CheckoutResult<()> {
    let rules = require_array(policy_payload, "rules")?;
    if rules.len() == 3
        && rules.iter().any(|rule| {
            rule.get("effect").and_then(Value::as_str) == Some("allow")
                && rule.get("action").and_then(Value::as_str) == Some("propose")
                && rule.get("resource_selector").and_then(Value::as_str) == Some(proposal_selector)
        })
    {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_writable_ref(payload: &Value, proposal_ref: &str) -> CheckoutResult<()> {
    let prefixes = require_array(payload, "writable_ref_prefixes")?;
    if prefixes.iter().filter_map(Value::as_str).any(|prefix| {
        proposal_ref == prefix
            || proposal_ref
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }) {
        Ok(())
    } else {
        Err(integrity())
    }
}

/// Prove that the exact authority records used by the proposal are reachable
/// from the canonical base snapshot. Only structured Tree/Record objects are
/// materialized; Blob contents (including private control canaries) are never
/// returned or retained.
#[allow(clippy::too_many_arguments)]
fn verify_base_authority(
    repository: &Repository,
    base_snapshot: &str,
    policy_oid: &str,
    grant_oid: &str,
    human_actor: &str,
    ai_actor: &str,
    subject: &str,
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
    work: &mut AuthorityWork,
) -> CheckoutResult<()> {
    let mut visited = BTreeSet::new();
    let mut cursor = base_snapshot.to_owned();
    loop {
        if !visited.insert(cursor.clone()) {
            return Err(integrity());
        }
        work.node(limits)?;
        let snapshot = load_structured(repository, &cursor, ObjectKind::Tree)?;
        require_tree(&snapshot)?;
        let entries = require_object_field(&snapshot, "entries")?
            .as_object()
            .ok_or_else(integrity)?;
        if entries.len() == 4 {
            require_exact_tree_entries(
                &snapshot,
                &["activity.json", "base", "context.json", "site"],
            )?;
            work.edges(4, limits)?;
            let _ = direct_entry_oid(&snapshot, "activity.json", ObjectKind::Record)?;
            let _ = direct_entry_oid(&snapshot, "context.json", ObjectKind::Record)?;
            let _ = direct_entry_oid(&snapshot, "site", ObjectKind::Tree)?;
            cursor = direct_entry_oid(&snapshot, "base", ObjectKind::Tree)?.to_owned();
            continue;
        }
        require_exact_tree_entries(&snapshot, &["control", "site"])?;
        work.edges(2, limits)?;
        let _ = direct_entry_oid(&snapshot, "site", ObjectKind::Tree)?;
        let control_oid = direct_entry_oid(&snapshot, "control", ObjectKind::Tree)?;
        work.node(limits)?;
        let control = load_structured(repository, control_oid, ObjectKind::Tree)?;
        require_tree(&control)?;
        require_exact_tree_entries(
            &control,
            &[
                "agent.actor.json",
                "creator.actor.json",
                "grant.json",
                "policy.json",
                "review-context.json",
                "subject.json",
            ],
        )?;
        work.edges(6, limits)?;
        let stored_ai = direct_entry_oid(&control, "agent.actor.json", ObjectKind::Record)?;
        let stored_human = direct_entry_oid(&control, "creator.actor.json", ObjectKind::Record)?;
        let stored_grant = direct_entry_oid(&control, "grant.json", ObjectKind::Record)?;
        let stored_policy = direct_entry_oid(&control, "policy.json", ObjectKind::Record)?;
        let _ = direct_entry_oid(&control, "review-context.json", ObjectKind::Blob)?;
        let stored_subject = direct_entry_oid(&control, "subject.json", ObjectKind::Record)?;
        for _ in 0..5 {
            work.node(limits)?;
        }
        if stored_grant != grant_oid || stored_policy != policy_oid {
            return Err(integrity());
        }

        let human = load_structured(repository, stored_human, ObjectKind::Record)?;
        require_record_type(&human, "actor")?;
        require_equal(require_string(&human, "entity_id")?, human_actor)?;
        require_equal(require_string(&human, "asserted_by")?, human_actor)?;
        require_equal(require_string(&human, "origin")?, "self_declared")?;
        require_equal(
            require_string(require_object_field(&human, "payload")?, "actor_kind")?,
            "human",
        )?;

        let ai = load_structured(repository, stored_ai, ObjectKind::Record)?;
        require_record_type(&ai, "actor")?;
        require_equal(require_string(&ai, "entity_id")?, ai_actor)?;
        require_equal(require_string(&ai, "asserted_by")?, human_actor)?;
        require_equal(require_string(&ai, "origin")?, "tool_recorded")?;
        require_equal(
            require_string(require_object_field(&ai, "payload")?, "actor_kind")?,
            "ai_agent",
        )?;

        let subject_record = load_structured(repository, stored_subject, ObjectKind::Record)?;
        require_record_type(&subject_record, "subject")?;
        require_equal(require_string(&subject_record, "entity_id")?, subject)?;
        require_equal(require_string(&subject_record, "asserted_by")?, human_actor)?;
        require_equal(require_string(&subject_record, "origin")?, "self_declared")?;
        let subject_payload = require_object_field(&subject_record, "payload")?;
        require_equal(require_string(subject_payload, "subject_kind")?, "digital")?;
        require_equal(
            require_string(subject_payload, "label")?,
            &binding.project_key,
        )?;

        let policy = load_structured(repository, stored_policy, ObjectKind::Record)?;
        require_record_type(&policy, "policy")?;
        require_equal(require_string(&policy, "asserted_by")?, human_actor)?;
        let policy_payload = require_object_field(&policy, "payload")?;
        require_exact_string_array(
            policy_payload,
            "scope_refs",
            &[binding.proposal.project().as_str()],
        )?;
        require_equal(require_string(policy_payload, "default_effect")?, "deny")?;
        require_publish_gate(policy_payload, binding.proposal.decision_ref_name())?;
        require_proposal_rule(
            policy_payload,
            &format!("proposal/artifact/{}/**", binding.project_key),
        )?;

        let grant = load_structured(repository, stored_grant, ObjectKind::Record)?;
        require_record_type(&grant, "delegation_grant")?;
        require_equal(require_string(&grant, "asserted_by")?, human_actor)?;
        let grant_payload = require_object_field(&grant, "payload")?;
        require_equal(require_string(grant_payload, "principal_ref")?, human_actor)?;
        require_equal(require_string(grant_payload, "delegate_ref")?, ai_actor)?;
        require_equal(
            require_string(grant_payload, "project_ref")?,
            binding.proposal.project().as_str(),
        )?;
        require_exact_string_array(
            grant_payload,
            "writable_ref_prefixes",
            &[&format!("proposal/artifact/{}", binding.project_key)],
        )?;
        return Ok(());
    }
}

#[derive(Default)]
struct AuthorityWork {
    nodes: usize,
    edges: usize,
}

impl AuthorityWork {
    fn node(&mut self, limits: ArtifactCheckoutLimits) -> CheckoutResult<()> {
        self.nodes = self.nodes.checked_add(1).ok_or_else(resource_limit)?;
        if self.nodes > limits.max_authority_nodes {
            Err(resource_limit())
        } else {
            Ok(())
        }
    }

    fn edges(&mut self, count: usize, limits: ArtifactCheckoutLimits) -> CheckoutResult<()> {
        self.edges = self.edges.checked_add(count).ok_or_else(resource_limit)?;
        if self.edges > limits.max_authority_edges {
            Err(resource_limit())
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct TraversalWork {
    tree_nodes: usize,
    tree_edges: usize,
    files: usize,
    total_bytes: u64,
}

struct PendingTree {
    oid: String,
    prefix: String,
    depth: usize,
}

fn read_site(
    repository: &Repository,
    site_oid: &str,
    limits: ArtifactCheckoutLimits,
) -> CheckoutResult<Vec<ArtifactManifestEntry>> {
    let mut work = TraversalWork::default();
    let mut files = Vec::new();
    let mut pending = vec![PendingTree {
        oid: site_oid.to_owned(),
        prefix: String::new(),
        depth: 0,
    }];

    while let Some(tree) = pending.pop() {
        work.tree_nodes = work.tree_nodes.checked_add(1).ok_or_else(resource_limit)?;
        if work.tree_nodes > limits.max_tree_nodes || tree.depth > limits.artifact.max_depth {
            return Err(resource_limit());
        }

        let value = load_structured(repository, &tree.oid, ObjectKind::Tree)?;
        require_tree(&value)?;
        let entries = require_object_field(&value, "entries")?
            .as_object()
            .ok_or_else(integrity)?;
        let mut ordered = entries.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

        let mut child_spellings = BTreeMap::<String, String>::new();
        for (segment, _) in &ordered {
            validate_segment(segment)?;
            let folded = lowercase_key(segment);
            if child_spellings
                .insert(folded, (*segment).clone())
                .is_some_and(|existing| existing != **segment)
            {
                return Err(ArtifactCheckoutError::new(
                    "artifact_path_collision",
                    "artifact Tree contains colliding paths",
                ));
            }
        }

        // Reverse push preserves bytewise depth-first order without recursion.
        for (segment, entry) in ordered.into_iter().rev() {
            work.tree_edges = work.tree_edges.checked_add(1).ok_or_else(resource_limit)?;
            if work.tree_edges > limits.max_tree_edges {
                return Err(resource_limit());
            }
            let depth = tree.depth.checked_add(1).ok_or_else(resource_limit)?;
            if depth > limits.artifact.max_depth {
                return Err(resource_limit());
            }
            let path = join_path(&tree.prefix, segment, limits.artifact.max_path_bytes)?;
            let fields = entry.as_object().ok_or_else(integrity)?;
            if fields.len() != 2 {
                return Err(integrity());
            }
            let entry_kind = object_string(fields, "entry_kind")?;
            let oid = object_string(fields, "oid")?;
            let expected_kind = match entry_kind {
                "blob" => ObjectKind::Blob,
                "tree" => ObjectKind::Tree,
                _ => {
                    return Err(ArtifactCheckoutError::new(
                        "artifact_entry_unsupported",
                        "selected artifact Tree contains an unsupported entry",
                    ));
                }
            };
            if parse_oid(oid).ok() != Some(expected_kind) {
                return Err(ArtifactCheckoutError::new(
                    "reference_type_mismatch",
                    "selected artifact Tree entry has the wrong object kind",
                ));
            }

            match expected_kind {
                ObjectKind::Tree => pending.push(PendingTree {
                    oid: oid.to_owned(),
                    prefix: path,
                    depth,
                }),
                ObjectKind::Blob => {
                    work.files = work.files.checked_add(1).ok_or_else(resource_limit)?;
                    if work.files > limits.artifact.max_files {
                        return Err(resource_limit());
                    }
                    let remaining = limits
                        .artifact
                        .max_total_bytes
                        .saturating_sub(work.total_bytes);
                    let read_limit = limits.artifact.max_file_bytes.min(remaining).max(1);
                    let bytes = repository
                        .objects()
                        .read_verified_blob_limited(oid, read_limit)
                        .map_err(|error| external_error(error.code().map(|code| code.as_str())))?
                        .ok_or_else(closure_missing)?;
                    let byte_len = u64::try_from(bytes.len()).map_err(|_| resource_limit())?;
                    if byte_len > limits.artifact.max_file_bytes {
                        return Err(resource_limit());
                    }
                    work.total_bytes = work
                        .total_bytes
                        .checked_add(byte_len)
                        .ok_or_else(resource_limit)?;
                    if work.total_bytes > limits.artifact.max_total_bytes {
                        return Err(resource_limit());
                    }
                    files.push(ArtifactManifestEntry::regular_file(path, bytes));
                }
                ObjectKind::Record | ObjectKind::Commit => unreachable!(),
            }
        }
    }
    Ok(files)
}

fn validate_segment(segment: &str) -> CheckoutResult<()> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains(['/', '\\'])
        || segment
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        || segment.nfc().collect::<String>() != segment
        || !portable_component(segment)
    {
        return Err(ArtifactCheckoutError::new(
            "artifact_path_invalid",
            "selected artifact Tree contains an invalid path",
        ));
    }
    Ok(())
}

fn join_path(prefix: &str, segment: &str, max_path_bytes: usize) -> CheckoutResult<String> {
    let separator = usize::from(!prefix.is_empty());
    let byte_len = prefix
        .len()
        .checked_add(separator)
        .and_then(|value| value.checked_add(segment.len()))
        .ok_or_else(resource_limit)?;
    if byte_len > max_path_bytes {
        return Err(resource_limit());
    }
    if prefix.is_empty() {
        Ok(segment.to_owned())
    } else {
        Ok(format!("{prefix}/{segment}"))
    }
}

fn portable_component(component: &str) -> bool {
    if component.ends_with(['.', ' '])
        || component
            .chars()
            .any(|character| matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        || component.chars().any(is_bidi_control)
    {
        return false;
    }
    let stem = component.split('.').next().unwrap_or(component);
    let uppercase = stem.to_ascii_uppercase();
    !matches!(uppercase.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !matches!(
            uppercase.as_bytes(),
            [b'C', b'O', b'M', b'1'..=b'9'] | [b'L', b'P', b'T', b'1'..=b'9']
        )
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn lowercase_key(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

fn load_structured(
    repository: &Repository,
    oid: &str,
    expected: ObjectKind,
) -> CheckoutResult<Value> {
    if parse_oid(oid).ok() != Some(expected) {
        return Err(ArtifactCheckoutError::new(
            "reference_type_mismatch",
            "generic artifact lineage contains the wrong object kind",
        ));
    }
    let object = repository
        .objects()
        .get_verified(oid)
        .map_err(|error| external_error(error.code().map(|code| code.as_str())))?
        .ok_or_else(closure_missing)?;
    if object.kind() != expected {
        return Err(integrity());
    }
    let value = object.structured().cloned().ok_or_else(integrity)?;
    synapse_schema::validate(&value)
        .map_err(|error| external_error(Some(error.code().as_str())))?;
    Ok(value)
}

fn require_tree(value: &Value) -> CheckoutResult<()> {
    require_object_type(value, "tree")?;
    require_equal(require_string(value, "schema_version")?, "0.1.0")?;
    require_object_field(value, "entries")?
        .as_object()
        .ok_or_else(integrity)?;
    Ok(())
}

fn direct_entry_oid<'a>(
    tree: &'a Value,
    name: &str,
    expected: ObjectKind,
) -> CheckoutResult<&'a str> {
    let entries = require_object_field(tree, "entries")?
        .as_object()
        .ok_or_else(integrity)?;
    let entry = entries
        .iter()
        .find_map(|(candidate, value)| (candidate == name).then_some(value))
        .ok_or_else(integrity)?;
    let fields = entry.as_object().ok_or_else(integrity)?;
    if fields.len() != 2 {
        return Err(integrity());
    }
    require_equal(object_string(fields, "entry_kind")?, expected.prefix())?;
    let oid = object_string(fields, "oid")?;
    if parse_oid(oid).ok() != Some(expected) {
        return Err(ArtifactCheckoutError::new(
            "reference_type_mismatch",
            "generic artifact snapshot entry has the wrong object kind",
        ));
    }
    Ok(oid)
}

fn require_exact_tree_entries(tree: &Value, expected: &[&str]) -> CheckoutResult<()> {
    let entries = require_object_field(tree, "entries")?
        .as_object()
        .ok_or_else(integrity)?;
    if entries.len() == expected.len()
        && entries
            .iter()
            .zip(expected)
            .all(|((actual, _), expected)| actual == expected)
    {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_object_type(value: &Value, expected: &str) -> CheckoutResult<()> {
    require_equal(require_string(value, "object_type")?, expected)
}

fn require_record_type(value: &Value, expected: &str) -> CheckoutResult<()> {
    require_object_type(value, "record")?;
    require_equal(require_string(value, "record_type")?, expected)
}

fn require_object_field<'a>(value: &'a Value, field: &str) -> CheckoutResult<&'a Value> {
    value.get(field).ok_or_else(integrity)
}

fn require_string<'a>(value: &'a Value, field: &str) -> CheckoutResult<&'a str> {
    require_object_field(value, field)?
        .as_str()
        .ok_or_else(integrity)
}

fn require_array<'a>(value: &'a Value, field: &str) -> CheckoutResult<&'a [Value]> {
    require_object_field(value, field)?
        .as_array()
        .ok_or_else(integrity)
}

fn object_string<'a>(fields: &'a [(String, Value)], field: &str) -> CheckoutResult<&'a str> {
    fields
        .iter()
        .find_map(|(candidate, value)| (candidate == field).then_some(value))
        .and_then(Value::as_str)
        .ok_or_else(integrity)
}

fn require_oid<'a>(value: &'a Value, field: &str, expected: ObjectKind) -> CheckoutResult<&'a str> {
    let oid = require_string(value, field)?;
    if parse_oid(oid).ok() == Some(expected) {
        Ok(oid)
    } else {
        Err(ArtifactCheckoutError::new(
            "reference_type_mismatch",
            "generic artifact lineage contains the wrong object kind",
        ))
    }
}

fn require_single_oid<'a>(
    value: &'a Value,
    field: &str,
    expected: ObjectKind,
) -> CheckoutResult<&'a str> {
    let values = require_array(value, field)?;
    if values.len() != 1 {
        return Err(integrity());
    }
    let oid = values[0].as_str().ok_or_else(integrity)?;
    if parse_oid(oid).ok() == Some(expected) {
        Ok(oid)
    } else {
        Err(ArtifactCheckoutError::new(
            "reference_type_mismatch",
            "generic artifact lineage contains the wrong object kind",
        ))
    }
}

fn require_exact_string_array(value: &Value, field: &str, expected: &[&str]) -> CheckoutResult<()> {
    let actual = require_array(value, field)?;
    if actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_str() == Some(*expected))
    {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_empty_array(value: &Value, field: &str) -> CheckoutResult<()> {
    if require_array(value, field)?.is_empty() {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_array_contains_string(value: &Value, field: &str, expected: &str) -> CheckoutResult<()> {
    if require_array(value, field)?
        .iter()
        .any(|value| value.as_str() == Some(expected))
    {
        Ok(())
    } else {
        Err(integrity())
    }
}

fn require_equal(actual: &str, expected: &str) -> CheckoutResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(integrity())
    }
}

const fn disposition_name(disposition: ArtifactDisposition) -> &'static str {
    match disposition {
        ArtifactDisposition::AdoptedUnchanged => "adopted_unchanged",
        ArtifactDisposition::Rejected => "rejected",
        ArtifactDisposition::Deferred => "deferred",
    }
}

fn artifact_error(error: crate::ArtifactError) -> ArtifactCheckoutError {
    match error.code() {
        "artifact_limits_invalid" => ArtifactCheckoutError::new(
            "artifact_limits_invalid",
            "artifact checkout limits are invalid",
        ),
        "artifact_path_invalid" => ArtifactCheckoutError::new(
            "artifact_path_invalid",
            "selected artifact Tree contains an invalid path",
        ),
        "artifact_entry_unsupported" => ArtifactCheckoutError::new(
            "artifact_entry_unsupported",
            "selected artifact Tree contains an unsupported entry",
        ),
        "artifact_path_collision" => ArtifactCheckoutError::new(
            "artifact_path_collision",
            "selected artifact Tree contains colliding paths",
        ),
        "resource_limit" => resource_limit(),
        _ => integrity(),
    }
}

fn external_error(code: Option<&str>) -> ArtifactCheckoutError {
    match code {
        Some("resource_limit") => resource_limit(),
        Some("oid_mismatch") => ArtifactCheckoutError::new(
            "oid_mismatch",
            "generic artifact repository contains a corrupt object",
        ),
        Some("reference_type_mismatch") => ArtifactCheckoutError::new(
            "reference_type_mismatch",
            "generic artifact lineage contains the wrong object kind",
        ),
        Some("closure_missing") => closure_missing(),
        Some("key_not_nfc" | "path_segment_invalid") => ArtifactCheckoutError::new(
            "artifact_path_invalid",
            "selected artifact Tree contains an invalid path",
        ),
        Some("schema_invalid") => integrity(),
        Some("stale_base" | "ref_conflict") => {
            ArtifactCheckoutError::new("stale_base", "trusted artifact Decision binding is stale")
        }
        _ => ArtifactCheckoutError::new(
            "storage_error",
            "generic artifact repository could not be read",
        ),
    }
}

const fn binding_invalid() -> ArtifactCheckoutError {
    ArtifactCheckoutError::new(
        "artifact_binding_invalid",
        "trusted artifact Decision binding is invalid",
    )
}

const fn integrity() -> ArtifactCheckoutError {
    ArtifactCheckoutError::new(
        "artifact_lineage_invalid",
        "generic artifact Decision lineage is invalid",
    )
}

const fn closure_missing() -> ArtifactCheckoutError {
    ArtifactCheckoutError::new(
        "closure_missing",
        "generic artifact Decision closure is incomplete",
    )
}

const fn resource_limit() -> ArtifactCheckoutError {
    ArtifactCheckoutError::new(
        "resource_limit",
        "artifact checkout exceeds a configured resource limit",
    )
}
