//! Human-gated publication of project decisions.
//!
//! Authentication and project routing remain responsibilities of the embedding
//! service. This module consumes that trusted authority, validates the immutable
//! proposal and decision records, and advances one `decision/*` Ref only while
//! the reviewed `proposal/*` Ref still points at the authorized proposal.
//!
//! The route rechecks static Activity/ContextPack/Grant links, output binding,
//! output Record restrictions, and quotas. It cannot reconstruct the original
//! pre-execution Actor/Grant/Policy/runtime capability intersection because the
//! human authority deliberately does not contain that runtime state. The
//! embedding service must therefore expose only proposals that passed
//! `CreativeAiRuntime` (or an equivalent admission boundary).

use super::{Repository, RepositoryError, Result, validate_prepared_head};
use crate::authorization::{
    AuthorizationClock, CandidateBinding, SystemAuthorizationClock, load_record, load_structured,
    require_array, require_array_contains, require_candidate_output_binding, require_equal,
    require_object, require_oid_kind, require_present_activity_references,
    require_present_oid_array, require_project_resource, require_role_actor, require_role_oid,
    require_string, require_writable_prefix, selector_matches, selector_supported,
    snapshot_object_set,
};
use std::collections::BTreeSet;
use synapse_canonical::{CoreError, ErrorCode, ObjectKind, Value, parse_oid};
use synapse_cas::PreparedClosureVerifier;
use synapse_sqlite::{
    RefPrecondition, RefUpdate, ReflogEntry, ReflogMetadata, ValidationError, validate_ref_name,
};

/// The human review outcome encoded by a `decision_feedback` Record.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DecisionDisposition {
    AdoptedUnchanged,
    AdoptedModified,
    PartiallyAdopted,
    Rejected,
    Deferred,
    ExperimentOnly,
}

impl DecisionDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdoptedUnchanged => "adopted_unchanged",
            Self::AdoptedModified => "adopted_modified",
            Self::PartiallyAdopted => "partially_adopted",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
            Self::ExperimentOnly => "experiment_only",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "adopted_unchanged" => Ok(Self::AdoptedUnchanged),
            "adopted_modified" => Ok(Self::AdoptedModified),
            "partially_adopted" => Ok(Self::PartiallyAdopted),
            "rejected" => Ok(Self::Rejected),
            "deferred" => Ok(Self::Deferred),
            "experiment_only" => Ok(Self::ExperimentOnly),
            _ => Err(denied(format!(
                "unsupported DecisionFeedback disposition {value:?}"
            ))),
        }
    }
}

/// Trusted reviewer, project, policy, and live lineage selected by a service.
///
/// None of these values are accepted from the untrusted decision update. The
/// embedding service constructs the authority only after authenticating the
/// human and routing the request to the intended project repository.
#[derive(Clone, Copy, Debug)]
pub struct HumanDecisionAuthority<'a> {
    authenticated_human_id: &'a str,
    authorized_project_id: &'a str,
    decision_ref_name: &'a str,
    decision_head: &'a str,
    proposal_ref_name: &'a str,
    proposal_head: &'a str,
    human_actor_record_oid: &'a str,
    policy_record_oid: &'a str,
}

impl<'a> HumanDecisionAuthority<'a> {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        authenticated_human_id: &'a str,
        authorized_project_id: &'a str,
        decision_ref_name: &'a str,
        decision_head: &'a str,
        proposal_ref_name: &'a str,
        proposal_head: &'a str,
        human_actor_record_oid: &'a str,
        policy_record_oid: &'a str,
    ) -> Self {
        Self {
            authenticated_human_id,
            authorized_project_id,
            decision_ref_name,
            decision_head,
            proposal_ref_name,
            proposal_head,
            human_actor_record_oid,
            policy_record_oid,
        }
    }
}

/// Untrusted object identifiers and optional message for one human decision.
#[derive(Clone, Copy, Debug)]
pub struct HumanDecisionUpdate<'a> {
    pub new_head: &'a str,
    pub decision_feedback_oid: &'a str,
    pub message: Option<&'a str>,
}

/// Auditable identifiers returned after an authorized decision is committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanDecisionReceipt {
    pub reflog: ReflogEntry,
    pub human_actor_record_oid: String,
    pub policy_record_oid: String,
    pub proposal_commit_oid: String,
    pub decision_feedback_oid: String,
    pub disposition: DecisionDisposition,
}

/// Human-gated decision publication boundary.
pub struct HumanDecisionRuntime<'repository, 'authority, Clock = SystemAuthorizationClock> {
    repository: &'repository mut Repository,
    authority: HumanDecisionAuthority<'authority>,
    clock: Clock,
}

impl<'repository, 'authority>
    HumanDecisionRuntime<'repository, 'authority, SystemAuthorizationClock>
{
    pub fn new(
        repository: &'repository mut Repository,
        authority: HumanDecisionAuthority<'authority>,
    ) -> Self {
        Self {
            repository,
            authority,
            clock: SystemAuthorizationClock,
        }
    }
}

impl<'repository, 'authority, Clock> HumanDecisionRuntime<'repository, 'authority, Clock>
where
    Clock: AuthorizationClock,
{
    pub fn with_clock(
        repository: &'repository mut Repository,
        authority: HumanDecisionAuthority<'authority>,
        clock: Clock,
    ) -> Self {
        Self {
            repository,
            authority,
            clock,
        }
    }

    /// Validate and atomically publish one human decision.
    ///
    /// All immutable identity, Policy, proposal, feedback, and snapshot checks
    /// complete before Ref state is read. SQLite then checks the reviewed
    /// proposal head before comparing and advancing the canonical decision Ref.
    pub fn publish_decision(
        &mut self,
        update: HumanDecisionUpdate<'_>,
    ) -> Result<HumanDecisionReceipt> {
        require_ref_namespace(self.authority.decision_ref_name, "decision")?;
        require_ref_namespace(self.authority.proposal_ref_name, "proposal")?;
        require_commit_oid(self.authority.decision_head, "trusted decision head")?;
        require_commit_oid(self.authority.proposal_head, "trusted proposal head")?;
        require_commit_oid(update.new_head, "candidate decision head")?;
        require_oid_kind(
            update.decision_feedback_oid,
            ObjectKind::Record,
            "DecisionFeedback OID",
        )?;

        let closure_verifier = PreparedClosureVerifier::new(
            &self.repository.objects,
            self.repository.graph_limits,
            self.repository.tombstone_scan_limits,
        )?;
        validate_prepared_head(&closure_verifier, self.authority.decision_head)
            .map_err(synapse_sqlite::RefStoreError::Validation)?;
        validate_prepared_head(&closure_verifier, self.authority.proposal_head)
            .map_err(synapse_sqlite::RefStoreError::Validation)?;
        validate_prepared_head(&closure_verifier, update.new_head)
            .map_err(synapse_sqlite::RefStoreError::Validation)?;

        let base_commit = load_structured(
            self.repository,
            self.authority.decision_head,
            ObjectKind::Commit,
            "canonical decision head",
        )?;
        let proposal_commit = load_structured(
            self.repository,
            self.authority.proposal_head,
            ObjectKind::Commit,
            "reviewed proposal Commit",
        )?;
        let decision_commit = load_structured(
            self.repository,
            update.new_head,
            ObjectKind::Commit,
            "candidate decision Commit",
        )?;
        let feedback = load_record(
            self.repository,
            update.decision_feedback_oid,
            "decision_feedback",
            "DecisionFeedback",
        )?;
        let human_actor = load_record(
            self.repository,
            self.authority.human_actor_record_oid,
            "actor",
            "human Actor snapshot",
        )?;
        let policy = load_record(
            self.repository,
            self.authority.policy_record_oid,
            "policy",
            "Policy snapshot",
        )?;

        require_human_actor(&human_actor, self.authority.authenticated_human_id)?;
        require_base_authority_bindings(
            self.repository,
            self.authority.decision_head,
            self.authority.human_actor_record_oid,
            self.authority.policy_record_oid,
        )?;
        let policy_payload = require_object(&policy, "payload", "Policy payload")?;
        require_equal(
            require_string(&policy, "asserted_by", "Policy asserted_by")?,
            self.authority.authenticated_human_id,
            "Policy snapshot was not asserted by the authenticated human reviewer",
        )?;
        require_array_contains(
            policy_payload,
            "scope_refs",
            self.authority.authorized_project_id,
            "Policy snapshot does not cover the authorized project",
        )?;
        authorize_human_publish(policy_payload, self.authority.decision_ref_name)?;
        require_proposal_not_previously_decided(
            self.repository,
            self.authority.decision_head,
            self.authority.proposal_head,
        )?;

        require_equal(
            require_string(
                &proposal_commit,
                "commit_kind",
                "reviewed proposal Commit kind",
            )?,
            "checkpoint",
            "reviewed proposal must be a checkpoint Commit",
        )?;
        require_single_parent(
            &proposal_commit,
            self.authority.decision_head,
            "reviewed proposal",
        )?;
        require_exact_protected_controls(
            self.repository,
            self.authority.decision_head,
            self.authority.proposal_head,
            "reviewed proposal",
        )?;
        require_ai_proposal_chain(
            self.repository,
            self.authority.proposal_head,
            self.authority.proposal_ref_name,
            &proposal_commit,
            self.authority.authenticated_human_id,
            self.authority.authorized_project_id,
            self.authority.decision_ref_name,
            self.authority.decision_head,
            self.authority.policy_record_oid,
        )?;
        require_snapshot_retains_base_non_tree(
            self.repository,
            self.authority.proposal_head,
            self.authority.decision_head,
            "reviewed proposal",
        )?;

        require_equal(
            require_string(
                &decision_commit,
                "commit_kind",
                "candidate decision Commit kind",
            )?,
            "decision",
            "human publication requires a decision Commit",
        )?;
        require_single_parent(
            &decision_commit,
            self.authority.decision_head,
            "candidate decision",
        )?;
        require_equal(
            require_string(
                &decision_commit,
                "author_ref",
                "candidate decision Commit author_ref",
            )?,
            self.authority.authenticated_human_id,
            "candidate decision Commit author does not match the authenticated human",
        )?;
        require_exact_transition(
            &decision_commit,
            update.decision_feedback_oid,
            "candidate decision Commit",
        )?;
        if !require_array(
            &decision_commit,
            "bound_declaration_refs",
            "candidate decision Commit bound_declaration_refs",
        )?
        .is_empty()
        {
            return Err(denied(
                "Stage 0 human decision Commit cannot introduce bound declarations; bound_declaration_refs must be empty",
            ));
        }

        require_equal(
            require_string(&feedback, "asserted_by", "DecisionFeedback asserted_by")?,
            self.authority.authenticated_human_id,
            "DecisionFeedback must be asserted by the authenticated human",
        )?;
        require_equal(
            require_string(&feedback, "origin", "DecisionFeedback origin")?,
            "self_declared",
            "DecisionFeedback must be a self-declared human decision",
        )?;
        let feedback_payload = require_object(&feedback, "payload", "DecisionFeedback payload")?;
        require_equal(
            require_string(
                feedback_payload,
                "proposal_ref",
                "DecisionFeedback proposal_ref",
            )?,
            self.authority.proposal_head,
            "DecisionFeedback does not target the reviewed proposal Commit",
        )?;
        let disposition = DecisionDisposition::parse(require_string(
            feedback_payload,
            "disposition",
            "DecisionFeedback disposition",
        )?)?;
        require_disposition_snapshot(
            &base_commit,
            &proposal_commit,
            &decision_commit,
            disposition,
        )?;
        require_snapshot_retains_base_non_tree(
            self.repository,
            update.new_head,
            self.authority.decision_head,
            "candidate decision",
        )?;
        require_exact_protected_controls(
            self.repository,
            self.authority.decision_head,
            update.new_head,
            "candidate decision",
        )?;
        let evaluated_at = self
            .clock
            .now_unix_nanos()
            .map_err(RepositoryError::Clock)?;
        let occurred_at_unix_nanos = i64::try_from(evaluated_at).map_err(|_| {
            RepositoryError::Clock("current time exceeds reflog i64 nanosecond range".into())
        })?;
        let ref_update = RefUpdate {
            ref_name: self.authority.decision_ref_name,
            expected_head: Some(self.authority.decision_head),
            new_head: update.new_head,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos,
                actor: Some(self.authority.authenticated_human_id),
                message: update.message,
            },
        };
        let proposal_precondition = RefPrecondition {
            ref_name: self.authority.proposal_ref_name,
            expected_head: Some(self.authority.proposal_head),
        };
        let validator = |head: &str| validate_prepared_head(&closure_verifier, head);
        let clock = &self.clock;
        let transaction_guard = || {
            let now = clock
                .now_unix_nanos()
                .map_err(|message| ValidationError::new("storage_error", message))?;
            if now < evaluated_at {
                return Err(ValidationError::new(
                    "storage_error",
                    "trusted human-decision clock moved backwards",
                ));
            }
            Ok(())
        };
        let reflog = self
            .repository
            .refs
            .compare_and_swap_with_preconditions_and_guard(
                ref_update,
                &[proposal_precondition],
                &validator,
                &transaction_guard,
            )?;

        Ok(HumanDecisionReceipt {
            reflog,
            human_actor_record_oid: self.authority.human_actor_record_oid.to_owned(),
            policy_record_oid: self.authority.policy_record_oid.to_owned(),
            proposal_commit_oid: self.authority.proposal_head.to_owned(),
            decision_feedback_oid: update.decision_feedback_oid.to_owned(),
            disposition,
        })
    }
}

fn require_ref_namespace(ref_name: &str, expected: &str) -> Result<()> {
    validate_ref_name(ref_name)?;
    if ref_name.split('/').next() == Some(expected) {
        Ok(())
    } else {
        Err(denied(format!(
            "human decision route requires {expected}/*, not {ref_name:?}"
        )))
    }
}

fn require_commit_oid(oid: &str, label: &str) -> Result<()> {
    if parse_oid(oid)? == ObjectKind::Commit {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::ReferenceTypeMismatch,
            format!("{label} is not a Commit OID: {oid}"),
        )
        .into())
    }
}

fn require_human_actor(actor: &Value, authenticated_human: &str) -> Result<()> {
    require_equal(
        require_string(actor, "entity_id", "human Actor entity_id")?,
        authenticated_human,
        "human Actor snapshot does not describe the authenticated human",
    )?;
    require_equal(
        require_string(actor, "asserted_by", "human Actor asserted_by")?,
        authenticated_human,
        "human Actor snapshot must be self-asserted",
    )?;
    require_equal(
        require_string(
            require_object(actor, "payload", "human Actor payload")?,
            "actor_kind",
            "human Actor kind",
        )?,
        "human",
        "authenticated reviewer Actor must have actor_kind=human",
    )
}

fn require_base_authority_bindings(
    repository: &Repository,
    base_commit: &str,
    human_actor_oid: &str,
    policy_oid: &str,
) -> Result<()> {
    let snapshot = snapshot_object_set(repository, base_commit)?;
    for (oid, label) in [
        (human_actor_oid, "human Actor"),
        (policy_oid, "Policy snapshot"),
    ] {
        if !snapshot.contains(oid) {
            return Err(denied(format!(
                "canonical decision snapshot does not bind the exact {label} OID {oid}"
            )));
        }
    }
    Ok(())
}

fn require_single_parent(commit: &Value, expected_parent: &str, label: &str) -> Result<()> {
    let parents = require_array(commit, "parents", format!("{label} parents"))?;
    if parents.len() == 1 && parents[0].as_str() == Some(expected_parent) {
        Ok(())
    } else {
        Err(denied(format!(
            "{label} Commit must have the trusted decision head as its sole parent"
        )))
    }
}

fn require_exact_transition(commit: &Value, expected: &str, label: &str) -> Result<()> {
    let transitions = require_array(
        commit,
        "transition_refs",
        format!("{label} transition_refs"),
    )?;
    if transitions.len() == 1 && transitions[0].as_str() == Some(expected) {
        Ok(())
    } else {
        Err(denied(format!(
            "{label} must directly bind exactly the requested transition Record"
        )))
    }
}

#[allow(clippy::too_many_arguments)]
fn require_ai_proposal_chain(
    repository: &Repository,
    proposal_oid: &str,
    proposal_ref: &str,
    proposal: &Value,
    authenticated_human: &str,
    project_id: &str,
    decision_ref: &str,
    decision_head: &str,
    policy_oid: &str,
) -> Result<()> {
    let transitions = require_array(
        proposal,
        "transition_refs",
        "reviewed proposal transition_refs",
    )?;
    if transitions.len() != 1 {
        return Err(denied(
            "reviewed proposal must directly bind exactly one AI Activity",
        ));
    }
    let activity_oid = transitions[0]
        .as_str()
        .ok_or_else(|| denied("reviewed proposal transition is not an OID"))?;
    let activity = load_record(repository, activity_oid, "activity", "proposal AI Activity")?;
    let activity_payload = require_object(&activity, "payload", "proposal AI Activity payload")?;
    require_equal(
        require_string(
            activity_payload,
            "activity_kind",
            "proposal AI Activity kind",
        )?,
        "ai_run",
        "reviewed proposal transition is not an ai_run Activity",
    )?;
    let ai_run = require_object(activity_payload, "ai_run", "proposal AI Activity ai_run")?;
    require_equal(
        require_string(ai_run, "status", "proposal AI Activity status")?,
        "proposal_ready",
        "reviewed proposal AI Activity is not proposal_ready",
    )?;
    let agent_id = require_string(ai_run, "agent_ref", "proposal AI Activity agent_ref")?;
    require_equal(
        require_string(proposal, "author_ref", "reviewed proposal author_ref")?,
        agent_id,
        "reviewed proposal author does not match its AI Activity agent",
    )?;
    require_equal(
        require_string(&activity, "asserted_by", "proposal AI Activity asserted_by")?,
        agent_id,
        "proposal AI Activity must be asserted by its agent",
    )?;
    require_equal(
        require_string(
            ai_run,
            "responsible_principal_ref",
            "proposal AI Activity responsible principal",
        )?,
        authenticated_human,
        "proposal responsible principal is not the authenticated human reviewer",
    )?;
    require_role_actor(activity_payload, "agent", agent_id)?;
    require_role_actor(
        activity_payload,
        "responsible_principal",
        authenticated_human,
    )?;
    if !matches!(
        activity_payload
            .get("side_effect_class")
            .and_then(Value::as_str),
        Some("none" | "project_internal")
    ) {
        return Err(denied(
            "reviewed AI proposal requires an explicit none or project_internal side effect class",
        ));
    }
    require_array_contains(
        ai_run,
        "required_human_gates",
        "before_decision_ref",
        "proposal AI Activity does not require the decision Human Gate",
    )?;

    let context_oid = require_string(
        ai_run,
        "context_pack_ref",
        "proposal AI Activity ContextPack",
    )?;
    let grant_oid = require_string(
        ai_run,
        "delegation_grant_ref",
        "proposal AI Activity DelegationGrant",
    )?;
    require_role_oid(activity_payload, "input_refs", "context", context_oid)?;
    let context = load_record(
        repository,
        context_oid,
        "context_pack",
        "proposal ContextPack",
    )?;
    require_equal(
        require_string(&context, "asserted_by", "proposal ContextPack asserted_by")?,
        authenticated_human,
        "proposal ContextPack was not asserted by the authenticated human reviewer",
    )?;
    let context_payload = require_object(&context, "payload", "proposal ContextPack payload")?;
    for (field, expected) in [
        ("base_commit", decision_head),
        ("expected_ref_head", decision_head),
        ("base_ref_name", decision_ref),
        ("policy_snapshot_ref", policy_oid),
        ("delegation_grant_ref", grant_oid),
    ] {
        require_equal(
            require_string(context_payload, field, format!("ContextPack {field}"))?,
            expected,
            format!("proposal ContextPack {field} does not match trusted authority"),
        )?;
    }
    require_array_contains(
        context_payload,
        "selected_context_refs",
        decision_head,
        "proposal ContextPack does not select its canonical base Commit",
    )?;
    require_present_oid_array(
        repository,
        context_payload,
        "selected_context_refs",
        "proposal ContextPack selected context",
    )?;

    let grant = load_record(
        repository,
        grant_oid,
        "delegation_grant",
        "proposal DelegationGrant",
    )?;
    if !snapshot_object_set(repository, decision_head)?.contains(grant_oid) {
        return Err(denied(format!(
            "canonical decision snapshot does not bind proposal DelegationGrant {grant_oid}"
        )));
    }
    require_equal(
        require_string(&grant, "asserted_by", "DelegationGrant asserted_by")?,
        authenticated_human,
        "proposal DelegationGrant was not asserted by the authenticated human reviewer",
    )?;
    let grant_payload = require_object(&grant, "payload", "DelegationGrant payload")?;
    for (field, expected) in [
        ("principal_ref", authenticated_human),
        ("delegate_ref", agent_id),
        ("project_ref", project_id),
    ] {
        require_equal(
            require_string(grant_payload, field, format!("DelegationGrant {field}"))?,
            expected,
            format!("proposal DelegationGrant {field} does not match trusted authority"),
        )?;
    }
    require_array_contains(
        grant_payload,
        "data_classes",
        require_string(
            context_payload,
            "data_classification",
            "ContextPack data_classification",
        )?,
        "proposal ContextPack data classification is outside the DelegationGrant",
    )?;
    require_project_resource(grant_payload, project_id)?;
    require_writable_prefix(grant_payload, proposal_ref)?;
    require_array_contains(
        grant_payload,
        "required_human_gates",
        "before_decision_ref",
        "proposal DelegationGrant does not require the decision Human Gate",
    )?;
    require_present_activity_references(repository, activity_payload)?;
    require_candidate_output_binding(
        repository,
        CandidateBinding {
            candidate_commit: proposal_oid,
            base_commit: decision_head,
            activity_oid,
            context_pack_oid: context_oid,
            authenticated_actor_id: agent_id,
            context_payload,
            activity_payload,
            grant_payload,
        },
    )?;
    Ok(())
}

fn require_disposition_snapshot(
    base_commit: &Value,
    proposal_commit: &Value,
    decision_commit: &Value,
    disposition: DecisionDisposition,
) -> Result<()> {
    let base_snapshot = require_string(base_commit, "snapshot", "base decision snapshot")?;
    let proposal_snapshot = require_string(proposal_commit, "snapshot", "proposal snapshot")?;
    let decision_snapshot =
        require_string(decision_commit, "snapshot", "candidate decision snapshot")?;
    match disposition {
        DecisionDisposition::AdoptedUnchanged => require_equal(
            decision_snapshot,
            proposal_snapshot,
            "adopted_unchanged decision must use the exact proposal snapshot",
        ),
        DecisionDisposition::Rejected
        | DecisionDisposition::Deferred
        | DecisionDisposition::ExperimentOnly => require_equal(
            decision_snapshot,
            base_snapshot,
            "non-adopting decision must retain the exact canonical base snapshot",
        ),
        DecisionDisposition::AdoptedModified | DecisionDisposition::PartiallyAdopted => Err(
            denied("Stage 0 HumanDecisionRuntime does not yet admit modified or partial adoption"),
        ),
    }
}

fn require_snapshot_retains_base_non_tree(
    repository: &Repository,
    candidate_commit: &str,
    base_commit: &str,
    label: &str,
) -> Result<()> {
    let base = snapshot_object_set(repository, base_commit)?;
    let candidate = snapshot_object_set(repository, candidate_commit)?;
    for oid in base {
        if parse_oid(&oid)? != ObjectKind::Tree && !candidate.contains(&oid) {
            return Err(denied(format!(
                "{label} snapshot does not retain canonical base object {oid}"
            )));
        }
    }
    Ok(())
}

fn require_exact_protected_controls(
    repository: &Repository,
    base_commit: &str,
    candidate_commit: &str,
    label: &str,
) -> Result<()> {
    let base = protected_control_set(repository, base_commit)?;
    let candidate = protected_control_set(repository, candidate_commit)?;
    if base == candidate {
        Ok(())
    } else {
        Err(denied(format!(
            "{label} changes protected control Records: base={base:?}, candidate={candidate:?}"
        )))
    }
}

fn protected_control_set(repository: &Repository, commit_oid: &str) -> Result<BTreeSet<String>> {
    let mut controls = BTreeSet::new();
    for oid in snapshot_object_set(repository, commit_oid)? {
        if parse_oid(&oid)? != ObjectKind::Record {
            continue;
        }
        let record = load_structured(repository, &oid, ObjectKind::Record, "snapshot Record")?;
        if matches!(
            require_string(&record, "record_type", "snapshot Record type")?,
            "actor" | "policy" | "delegation_grant" | "tombstone"
        ) {
            controls.insert(oid);
        }
    }
    Ok(controls)
}

fn require_proposal_not_previously_decided(
    repository: &Repository,
    base_commit: &str,
    proposal_commit: &str,
) -> Result<()> {
    let mut visited = BTreeSet::new();
    let mut pending = vec![(base_commit.to_owned(), 0_usize)];
    let mut edges = 0_usize;
    while let Some((commit_oid, depth)) = pending.pop() {
        if !visited.insert(commit_oid.clone()) {
            continue;
        }
        if visited.len() > repository.graph_limits.max_objects
            || depth > repository.graph_limits.max_depth
        {
            return Err(resource_limit(
                "decision ancestry exceeds configured object/depth limit",
            ));
        }
        let commit = load_structured(
            repository,
            &commit_oid,
            ObjectKind::Commit,
            "canonical decision ancestry Commit",
        )?;
        for transition in require_array(
            &commit,
            "transition_refs",
            "canonical decision transition_refs",
        )? {
            edges += 1;
            if edges > repository.graph_limits.max_edges {
                return Err(resource_limit(
                    "decision ancestry exceeds configured edge limit",
                ));
            }
            let Some(oid) = transition.as_str() else {
                continue;
            };
            let record = load_structured(
                repository,
                oid,
                ObjectKind::Record,
                "canonical decision transition Record",
            )?;
            if record.get("record_type").and_then(Value::as_str) == Some("decision_feedback")
                && record
                    .get("payload")
                    .and_then(|payload| payload.get("proposal_ref"))
                    .and_then(Value::as_str)
                    == Some(proposal_commit)
            {
                return Err(denied(
                    "canonical decision ancestry already contains feedback for this proposal",
                ));
            }
        }
        for parent in require_array(&commit, "parents", "canonical decision parents")? {
            edges += 1;
            if edges > repository.graph_limits.max_edges {
                return Err(resource_limit(
                    "decision ancestry exceeds configured edge limit",
                ));
            }
            let parent = parent
                .as_str()
                .ok_or_else(|| denied("canonical decision parent is not an OID"))?;
            pending.push((parent.to_owned(), depth + 1));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EvaluatedPolicyEffect {
    Allow,
    SatisfiedDecisionGate,
    UnsatisfiedHumanGate,
    Deny,
}

fn authorize_human_publish(policy_payload: &Value, decision_ref: &str) -> Result<()> {
    let mut effects = BTreeSet::new();
    for rule in require_array(policy_payload, "rules", "Policy rules")? {
        if rule.get("action").and_then(Value::as_str) != Some("publish") {
            continue;
        }
        let selector = require_string(rule, "resource_selector", "Policy resource selector")?;
        if !selector_supported(selector) {
            return Err(denied(format!(
                "Policy publish rule uses an unsupported selector {selector:?}"
            )));
        }
        if !selector_matches(selector, decision_ref) {
            continue;
        }
        if rule.get("condition_text").is_some() {
            return Err(denied(
                "matching conditional Policy publish rule cannot be evaluated",
            ));
        }
        let effect = match require_string(rule, "effect", "Policy effect")? {
            "allow" => EvaluatedPolicyEffect::Allow,
            "deny" => EvaluatedPolicyEffect::Deny,
            "require_human_gate"
                if rule.get("human_gate").and_then(Value::as_str)
                    == Some("before_decision_ref") =>
            {
                EvaluatedPolicyEffect::SatisfiedDecisionGate
            }
            "require_human_gate" => EvaluatedPolicyEffect::UnsatisfiedHumanGate,
            effect => return Err(denied(format!("unsupported Policy effect {effect:?}"))),
        };
        effects.insert(effect);
    }

    if effects.contains(&EvaluatedPolicyEffect::Deny) {
        return Err(denied(format!(
            "Policy denies publishing decision Ref {decision_ref:?}"
        )));
    }
    if effects.contains(&EvaluatedPolicyEffect::UnsatisfiedHumanGate) {
        return Err(CoreError::new(
            ErrorCode::HumanGateRequired,
            format!("Policy requires another Human Gate for decision Ref {decision_ref:?}"),
        )
        .into());
    }
    if effects.contains(&EvaluatedPolicyEffect::SatisfiedDecisionGate)
        || effects.contains(&EvaluatedPolicyEffect::Allow)
    {
        return Ok(());
    }
    match require_string(policy_payload, "default_effect", "Policy default_effect")? {
        "allow" => Ok(()),
        "deny" => Err(denied(format!(
            "Policy default denies publishing decision Ref {decision_ref:?}"
        ))),
        effect => Err(denied(format!(
            "unsupported Policy default_effect {effect:?}"
        ))),
    }
}

fn resource_limit(message: impl Into<String>) -> RepositoryError {
    CoreError::new(ErrorCode::ResourceLimit, message).into()
}

fn denied(message: impl Into<String>) -> RepositoryError {
    CoreError::new(ErrorCode::AuthorizationDenied, message).into()
}

#[cfg(test)]
mod tests {
    use super::{DecisionDisposition, authorize_human_publish, require_ref_namespace};
    use synapse_canonical::parse_strict;

    #[test]
    fn disposition_parser_is_exact() {
        for (text, expected) in [
            ("adopted_unchanged", DecisionDisposition::AdoptedUnchanged),
            ("adopted_modified", DecisionDisposition::AdoptedModified),
            ("partially_adopted", DecisionDisposition::PartiallyAdopted),
            ("rejected", DecisionDisposition::Rejected),
            ("deferred", DecisionDisposition::Deferred),
            ("experiment_only", DecisionDisposition::ExperimentOnly),
        ] {
            assert_eq!(DecisionDisposition::parse(text).unwrap(), expected);
            assert_eq!(expected.as_str(), text);
        }
        assert_eq!(
            DecisionDisposition::parse("adopted").unwrap_err().code(),
            "authorization_denied"
        );
    }

    #[test]
    fn route_namespaces_are_not_interchangeable() {
        assert!(require_ref_namespace("decision/main", "decision").is_ok());
        assert!(require_ref_namespace("proposal/agent/run", "proposal").is_ok());
        assert_eq!(
            require_ref_namespace("release/main", "decision")
                .unwrap_err()
                .code(),
            "authorization_denied"
        );
    }

    #[test]
    fn publish_policy_denies_before_satisfied_gate_or_allow() {
        let policy = parse_strict(
            br#"{
                "default_effect":"deny",
                "rules":[
                    {"action":"publish","effect":"allow","resource_selector":"decision/**"},
                    {"action":"publish","effect":"require_human_gate","human_gate":"before_decision_ref","resource_selector":"decision/**"},
                    {"action":"publish","effect":"deny","resource_selector":"decision/main"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            authorize_human_publish(&policy, "decision/main")
                .unwrap_err()
                .code(),
            "authorization_denied"
        );

        let gated = parse_strict(
            br#"{
                "default_effect":"deny",
                "rules":[
                    {"action":"publish","effect":"require_human_gate","human_gate":"before_decision_ref","resource_selector":"decision/**"}
                ]
            }"#,
        )
        .unwrap();
        assert!(authorize_human_publish(&gated, "decision/main").is_ok());
    }
}
