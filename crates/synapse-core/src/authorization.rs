//! Creative AI authorization at the repository publication boundary.
//!
//! Authentication is deliberately supplied by the embedding service. This
//! module consumes that authenticated entity ID and verifies that the immutable
//! Actor, Activity, ContextPack, DelegationGrant, Policy, candidate Commit, and
//! live Ref state all agree before publishing an AI proposal.

use super::{Repository, RepositoryError, Result, validate_prepared_head};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_canonical::{CoreError, ErrorCode, ObjectKind, Value, parse_oid};
use synapse_cas::{ClosureNodeState, PreparedClosureVerifier};
use synapse_schema::validate;
use synapse_sqlite::{
    RefPrecondition, RefStoreError, RefUpdate, ReflogEntry, ReflogMetadata, ValidationError,
    validate_commit_oid, validate_ref_name,
};

/// Capability vocabulary shared by Actor, DelegationGrant, and AI Activity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AiCapability {
    ReadContext,
    Analyze,
    ProposeBranch,
    SubmitClaim,
    RequestReview,
    RenderPreview,
}

impl AiCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadContext => "read_context",
            Self::Analyze => "analyze",
            Self::ProposeBranch => "propose_branch",
            Self::SubmitClaim => "submit_claim",
            Self::RequestReview => "request_review",
            Self::RenderPreview => "render_preview",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "read_context" => Some(Self::ReadContext),
            "analyze" => Some(Self::Analyze),
            "propose_branch" => Some(Self::ProposeBranch),
            "submit_claim" => Some(Self::SubmitClaim),
            "request_review" => Some(Self::RequestReview),
            "render_preview" => Some(Self::RenderPreview),
            _ => None,
        }
    }

    fn policy_action(self) -> &'static str {
        match self {
            Self::ReadContext => "read",
            Self::Analyze => "analyze",
            Self::ProposeBranch | Self::RequestReview => "propose",
            Self::SubmitClaim | Self::RenderPreview => "derive",
        }
    }
}

/// Trusted identity, project, context, and runtime bounds for one AI execution.
///
/// The embedding service constructs this only after authentication and project
/// routing. Keeping it on [`CreativeAiRuntime`] prevents an untrusted publish
/// payload from selecting its own authority snapshots or capabilities.
#[derive(Clone, Copy, Debug)]
pub struct AiExecutionAuthority<'a> {
    authenticated_actor_id: &'a str,
    authorized_project_id: &'a str,
    authorized_principal_id: &'a str,
    authorized_base_ref: &'a str,
    actor_record_oid: &'a str,
    principal_actor_record_oid: &'a str,
    context_pack_oid: &'a str,
    authorized_capabilities: &'a [AiCapability],
    runtime_capabilities: &'a [AiCapability],
}

impl<'a> AiExecutionAuthority<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        authenticated_actor_id: &'a str,
        authorized_project_id: &'a str,
        authorized_principal_id: &'a str,
        authorized_base_ref: &'a str,
        actor_record_oid: &'a str,
        principal_actor_record_oid: &'a str,
        context_pack_oid: &'a str,
        authorized_capabilities: &'a [AiCapability],
        runtime_capabilities: &'a [AiCapability],
    ) -> Self {
        Self {
            authenticated_actor_id,
            authorized_project_id,
            authorized_principal_id,
            authorized_base_ref,
            actor_record_oid,
            principal_actor_record_oid,
            context_pack_oid,
            authorized_capabilities,
            runtime_capabilities,
        }
    }
}

/// Untrusted operation fields for one candidate AI proposal publication.
#[derive(Clone, Copy, Debug)]
pub struct AiProposalUpdate<'a> {
    pub ref_name: &'a str,
    pub expected_head: Option<&'a str>,
    pub new_head: &'a str,
    pub message: Option<&'a str>,
    pub activity_oid: &'a str,
}

/// Server-selected side-effect ceiling for one pre-authorized AI run.
///
/// The Stage 0 profile deliberately has no variant for external or physical
/// effects. Adding such a variant would require a separate enforcement and
/// human-gate contract rather than weakening this enum.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AiSideEffectClass {
    None,
    ProjectInternal,
}

impl AiSideEffectClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ProjectInternal => "project_internal",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "project_internal" => Some(Self::ProjectInternal),
            _ => None,
        }
    }
}

/// Trusted publication target selected by the embedding application.
///
/// These fields are deliberately not part of the generated proposal. The
/// application fixes them before execution and a preflight decision seals the
/// exact expectation used by publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AiPublicationTarget<'a> {
    ref_name: &'a str,
    expected_head: Option<&'a str>,
    side_effect_class: AiSideEffectClass,
}

impl<'a> AiPublicationTarget<'a> {
    pub const fn new(
        ref_name: &'a str,
        expected_head: Option<&'a str>,
        side_effect_class: AiSideEffectClass,
    ) -> Self {
        Self {
            ref_name,
            expected_head,
            side_effect_class,
        }
    }

    pub const fn ref_name(&self) -> &'a str {
        self.ref_name
    }

    pub const fn expected_head(&self) -> Option<&'a str> {
        self.expected_head
    }

    pub const fn side_effect_class(&self) -> AiSideEffectClass {
        self.side_effect_class
    }
}

/// Generated object identifiers returned by a trusted execution adapter.
///
/// Target Ref state, capabilities, identity, and time are intentionally absent
/// and remain fixed by [`AiPreflightDecision`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AiGeneratedProposal<'a> {
    new_head: &'a str,
    activity_oid: &'a str,
    message: Option<&'a str>,
}

impl<'a> AiGeneratedProposal<'a> {
    pub const fn new(new_head: &'a str, activity_oid: &'a str, message: Option<&'a str>) -> Self {
        Self {
            new_head,
            activity_oid,
            message,
        }
    }

    pub const fn new_head(&self) -> &'a str {
        self.new_head
    }

    pub const fn activity_oid(&self) -> &'a str {
        self.activity_oid
    }

    pub const fn message(&self) -> Option<&'a str> {
        self.message
    }
}

/// Candidate-independent authorization evidence for one exact AI publication.
///
/// This is an opaque, consume-on-publication value, but it is not a complete
/// application permit: authentication, project ACLs, revocation, and execution
/// enforcement remain responsibilities of the embedding application. Core
/// re-runs the full immutable authority checks when this value is consumed.
#[derive(Debug, Eq, PartialEq)]
pub struct AiPreflightDecision {
    authenticated_actor_id: String,
    authorized_project_id: String,
    authorized_principal_id: String,
    authorized_base_ref: String,
    actor_record_oid: String,
    principal_actor_record_oid: String,
    context_pack_oid: String,
    delegation_grant_oid: String,
    policy_oid: String,
    base_commit_oid: String,
    target_ref_name: String,
    expected_target_head: Option<String>,
    side_effect_class: AiSideEffectClass,
    exact_capabilities: Vec<AiCapability>,
    runtime_capabilities: Vec<AiCapability>,
    evaluated_at_unix_nanos: i128,
    grant_expires_at_unix_nanos: i128,
}

impl AiPreflightDecision {
    pub fn actor_id(&self) -> &str {
        &self.authenticated_actor_id
    }

    pub fn actor_record_oid(&self) -> &str {
        &self.actor_record_oid
    }

    pub fn project_id(&self) -> &str {
        &self.authorized_project_id
    }

    pub fn principal_id(&self) -> &str {
        &self.authorized_principal_id
    }

    pub fn principal_actor_record_oid(&self) -> &str {
        &self.principal_actor_record_oid
    }

    pub fn base_ref_name(&self) -> &str {
        &self.authorized_base_ref
    }

    pub fn base_head(&self) -> &str {
        &self.base_commit_oid
    }

    pub fn target_ref_name(&self) -> &str {
        &self.target_ref_name
    }

    pub fn expected_target_head(&self) -> Option<&str> {
        self.expected_target_head.as_deref()
    }

    pub const fn side_effect_class(&self) -> AiSideEffectClass {
        self.side_effect_class
    }

    pub fn context_pack_oid(&self) -> &str {
        &self.context_pack_oid
    }

    pub fn delegation_grant_oid(&self) -> &str {
        &self.delegation_grant_oid
    }

    pub fn policy_oid(&self) -> &str {
        &self.policy_oid
    }

    pub fn exact_capabilities(&self) -> &[AiCapability] {
        &self.exact_capabilities
    }

    pub const fn evaluated_at_unix_nanos(&self) -> i128 {
        self.evaluated_at_unix_nanos
    }

    pub const fn grant_expires_at_unix_nanos(&self) -> i128 {
        self.grant_expires_at_unix_nanos
    }
}

/// Auditable details returned with a successful authorized publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationDecision {
    pub reflog: ReflogEntry,
    pub actor_record_oid: String,
    pub activity_oid: String,
    pub context_pack_oid: String,
    pub delegation_grant_oid: String,
    pub policy_oid: String,
    pub effective_capabilities: Vec<AiCapability>,
}

/// Trusted time source used for Grant expiry and successful reflog events.
///
/// The embedding runtime owns this dependency. An AI request must never select
/// or override the clock used for its own authorization.
pub trait AuthorizationClock {
    fn now_unix_nanos(&self) -> std::result::Result<i128, String>;
}

impl<T> AuthorizationClock for &T
where
    T: AuthorizationClock + ?Sized,
{
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        (**self).now_unix_nanos()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemAuthorizationClock;

impl AuthorizationClock for SystemAuthorizationClock {
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock is before Unix epoch: {error}"))?;
        i128::try_from(duration.as_nanos())
            .map_err(|_| "system time exceeds supported range".to_owned())
    }
}

/// High-level boundary that exposes only Creative AI proposal publication.
pub struct CreativeAiRuntime<'repository, 'authority, Clock = SystemAuthorizationClock> {
    repository: &'repository mut Repository,
    authority: AiExecutionAuthority<'authority>,
    clock: Clock,
}

impl<'repository, 'authority> CreativeAiRuntime<'repository, 'authority, SystemAuthorizationClock> {
    pub fn new(
        repository: &'repository mut Repository,
        authority: AiExecutionAuthority<'authority>,
    ) -> Self {
        Self {
            repository,
            authority,
            clock: SystemAuthorizationClock,
        }
    }
}

impl<'repository, 'authority, Clock> CreativeAiRuntime<'repository, 'authority, Clock>
where
    Clock: AuthorizationClock,
{
    /// Construct the runtime with an embedding-service-owned trusted clock.
    pub fn with_clock(
        repository: &'repository mut Repository,
        authority: AiExecutionAuthority<'authority>,
        clock: Clock,
    ) -> Self {
        Self {
            repository,
            authority,
            clock,
        }
    }

    /// Validate candidate-independent authority for one exact publication.
    ///
    /// Immutable Actor, principal, ContextPack, Grant, Policy, base-snapshot,
    /// project, capability, and time checks run before either live Ref is read.
    /// The live base and target expectations are then checked without changing
    /// a Ref or appending a reflog event.
    pub fn preflight_proposal(
        &self,
        target: AiPublicationTarget<'_>,
    ) -> Result<AiPreflightDecision> {
        let resolved = resolve_ai_authority(self.repository, &self.authority, &self.clock, target)?;

        let actual_base = self
            .repository
            .refs
            .get(self.authority.authorized_base_ref)?
            .map(|record| record.head);
        if actual_base.as_deref() != Some(resolved.base_commit_oid.as_str()) {
            return Err(stale_base(
                self.authority.authorized_base_ref,
                &resolved.base_commit_oid,
                actual_base.as_deref(),
            ));
        }

        let actual_target = self
            .repository
            .refs
            .get(target.ref_name)?
            .map(|record| record.head);
        if actual_target.as_deref() != target.expected_head {
            return Err(RefStoreError::RefConflict {
                ref_name: target.ref_name.to_owned(),
                expected_head: target.expected_head.map(str::to_owned),
                actual_head: actual_target,
            }
            .into());
        }

        Ok(AiPreflightDecision {
            authenticated_actor_id: self.authority.authenticated_actor_id.to_owned(),
            authorized_project_id: self.authority.authorized_project_id.to_owned(),
            authorized_principal_id: self.authority.authorized_principal_id.to_owned(),
            authorized_base_ref: self.authority.authorized_base_ref.to_owned(),
            actor_record_oid: self.authority.actor_record_oid.to_owned(),
            principal_actor_record_oid: self.authority.principal_actor_record_oid.to_owned(),
            context_pack_oid: self.authority.context_pack_oid.to_owned(),
            delegation_grant_oid: resolved.delegation_grant_oid,
            policy_oid: resolved.policy_oid,
            base_commit_oid: resolved.base_commit_oid,
            target_ref_name: target.ref_name.to_owned(),
            expected_target_head: target.expected_head.map(str::to_owned),
            side_effect_class: target.side_effect_class,
            exact_capabilities: resolved.exact_capabilities,
            runtime_capabilities: normalized_capabilities(self.authority.runtime_capabilities),
            evaluated_at_unix_nanos: resolved.evaluated_at_unix_nanos,
            grant_expires_at_unix_nanos: resolved.grant_expires_at_unix_nanos,
        })
    }

    /// Revalidate and publish generated objects under an exact preflight.
    ///
    /// The preflight value is consumed. Core compares it with this runtime's
    /// trusted authority, re-runs the same candidate-independent resolver, then
    /// performs every candidate/output check and the atomic Ref transaction.
    pub fn publish_preflighted(
        &mut self,
        decision: AiPreflightDecision,
        generated: AiGeneratedProposal<'_>,
    ) -> Result<AuthorizationDecision> {
        require_preflight_authority_binding(&self.authority, &decision)?;
        let update = AiProposalUpdate {
            ref_name: &decision.target_ref_name,
            expected_head: decision.expected_target_head.as_deref(),
            new_head: generated.new_head,
            message: generated.message,
            activity_oid: generated.activity_oid,
        };
        self.publish_proposal_internal(update, Some(decision.side_effect_class), Some(&decision))
    }

    /// Authorize and atomically publish one AI proposal.
    ///
    /// The immutable authorization chain is evaluated before SQLite starts its
    /// write transaction. The ContextPack's live base Ref expectation is then
    /// checked in the same `BEGIN IMMEDIATE` transaction as the proposal Ref
    /// compare-and-swap, eliminating the cross-Ref stale-base race.
    pub fn publish_proposal(
        &mut self,
        request: AiProposalUpdate<'_>,
    ) -> Result<AuthorizationDecision> {
        self.publish_proposal_internal(request, None, None)
    }

    fn publish_proposal_internal(
        &mut self,
        request: AiProposalUpdate<'_>,
        required_side_effect: Option<AiSideEffectClass>,
        preflight: Option<&AiPreflightDecision>,
    ) -> Result<AuthorizationDecision> {
        validate_ref_name(request.ref_name)?;
        require_ai_namespace(request.ref_name)?;
        validate_commit_oid(request.new_head)?;
        if let Some(expected_head) = request.expected_head {
            validate_commit_oid(expected_head)?;
        }
        require_oid_kind(request.activity_oid, ObjectKind::Record, "AI Activity OID")?;
        require_human_gated_base(self.authority.authorized_base_ref)?;
        let closure_verifier = PreparedClosureVerifier::new(
            &self.repository.objects,
            self.repository.graph_limits,
            self.repository.tombstone_scan_limits,
        )?;
        validate_prepared_head(&closure_verifier, request.new_head)
            .map_err(RefStoreError::Validation)?;

        let commit = load_structured(
            self.repository,
            request.new_head,
            ObjectKind::Commit,
            "candidate Commit",
        )?;
        let activity = load_record(
            self.repository,
            request.activity_oid,
            "activity",
            "AI Activity",
        )?;

        require_equal(
            require_string(&commit, "commit_kind", "candidate Commit kind")?,
            "checkpoint",
            "Stage 0 AI proposal Commit must be a checkpoint",
        )?;
        require_string(&commit, "author_ref", "candidate Commit author_ref").and_then(
            |author| {
                require_equal(
                    author,
                    self.authority.authenticated_actor_id,
                    "candidate Commit author does not match authenticated AI actor",
                )
            },
        )?;
        require_array_contains(
            &commit,
            "transition_refs",
            request.activity_oid,
            "candidate Commit does not directly bind the AI Activity",
        )?;

        let activity_payload = require_object(&activity, "payload", "AI Activity payload")?;
        require_equal(
            require_string(activity_payload, "activity_kind", "AI Activity kind")?,
            "ai_run",
            "authorization Activity is not an ai_run",
        )?;
        let ai_run = require_object(activity_payload, "ai_run", "AI Activity ai_run")?;
        require_equal(
            require_string(ai_run, "status", "AI Activity status")?,
            "proposal_ready",
            "AI Activity is not proposal_ready",
        )?;
        let activity_side_effect = activity_payload
            .get("side_effect_class")
            .and_then(Value::as_str)
            .and_then(AiSideEffectClass::parse)
            .ok_or_else(|| {
                denied(
                    "AI proposal publication requires an explicit none or project_internal side effect class",
                )
            })?;
        let authorized_side_effect = required_side_effect.unwrap_or(activity_side_effect);

        let agent_id = require_string(ai_run, "agent_ref", "AI Activity agent_ref")?;
        let principal_id = require_string(
            ai_run,
            "responsible_principal_ref",
            "AI Activity responsible_principal_ref",
        )?;
        require_equal(
            agent_id,
            self.authority.authenticated_actor_id,
            "AI Activity agent does not match authenticated actor",
        )?;
        require_equal(
            principal_id,
            self.authority.authorized_principal_id,
            "AI Activity principal does not match the authenticated runtime principal",
        )?;
        require_role_actor(activity_payload, "agent", agent_id)?;
        require_role_actor(activity_payload, "responsible_principal", principal_id)?;
        require_equal(
            require_string(&activity, "asserted_by", "AI Activity asserted_by")?,
            agent_id,
            "AI Activity must be asserted by its agent",
        )?;

        let context_pack_oid =
            require_string(ai_run, "context_pack_ref", "AI Activity context_pack_ref")?;
        require_equal(
            context_pack_oid,
            self.authority.context_pack_oid,
            "AI Activity ContextPack does not match the runtime-authorized ContextPack",
        )?;
        let grant_oid = require_string(
            ai_run,
            "delegation_grant_ref",
            "AI Activity delegation_grant_ref",
        )?;
        require_role_oid(activity_payload, "input_refs", "context", context_pack_oid)?;

        let target = AiPublicationTarget::new(
            request.ref_name,
            request.expected_head,
            authorized_side_effect,
        );
        let resolved = resolve_ai_authority(self.repository, &self.authority, &self.clock, target)?;
        if let Some(decision) = preflight {
            require_resolved_preflight_binding(&resolved, decision)?;
        }
        if activity_side_effect != authorized_side_effect {
            return Err(denied(
                "AI Activity side effect class does not match the pre-authorized execution class",
            ));
        }
        require_equal(
            grant_oid,
            &resolved.delegation_grant_oid,
            "Activity and ContextPack bind different DelegationGrants",
        )?;

        let context_payload = &resolved.context_payload;
        let grant_payload = &resolved.grant_payload;
        let base_commit = resolved.base_commit_oid.as_str();
        let expected_base = base_commit;
        require_direct_base_parent(&commit, base_commit)?;
        require_candidate_preserves_base_snapshot_objects(
            self.repository,
            request.new_head,
            base_commit,
        )?;
        require_present_activity_references(self.repository, activity_payload)?;
        let required_capabilities = require_candidate_output_binding(
            self.repository,
            CandidateBinding {
                candidate_commit: request.new_head,
                base_commit,
                activity_oid: request.activity_oid,
                context_pack_oid,
                authenticated_actor_id: self.authority.authenticated_actor_id,
                context_payload,
                activity_payload,
                grant_payload,
            },
        )?;

        let requested_capabilities = parse_capabilities(
            require_array(
                ai_run,
                "requested_capabilities",
                "AI Activity requested_capabilities",
            )?,
            "AI Activity requested_capabilities",
        )?;
        let authorized_capabilities = resolved
            .exact_capabilities
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if requested_capabilities != authorized_capabilities {
            return Err(denied(
                "AI Activity requested capabilities do not exactly match the pre-authorized execution capabilities",
            ));
        }
        if !requested_capabilities.contains(&AiCapability::ProposeBranch) {
            return Err(denied(
                "AI Activity did not request the propose_branch capability",
            ));
        }
        for capability in &required_capabilities {
            if !requested_capabilities.contains(capability) {
                return Err(denied(format!(
                    "AI Activity did not declare required capability {}",
                    capability.as_str()
                )));
            }
        }
        let active_from = resolved.grant_active_from_unix_nanos;
        let expires_at = resolved.grant_expires_at_unix_nanos;
        let evaluated_at = resolved.evaluated_at_unix_nanos;
        let base_ref_name = resolved.base_ref_name.as_str();
        let precondition = RefPrecondition {
            ref_name: base_ref_name,
            expected_head: Some(expected_base),
        };
        let update = RefUpdate {
            ref_name: request.ref_name,
            expected_head: request.expected_head,
            new_head: request.new_head,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::try_from(evaluated_at).map_err(|_| {
                    RepositoryError::Clock(
                        "current time exceeds reflog i64 nanosecond range".into(),
                    )
                })?,
                actor: Some(self.authority.authenticated_actor_id),
                message: request.message,
            },
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
                    "trusted authorization clock moved backwards",
                ));
            }
            if now < active_from {
                return Err(ValidationError::new(
                    "authorization_denied",
                    "DelegationGrant is not active yet",
                ));
            }
            if now >= expires_at {
                return Err(ValidationError::new(
                    "authorization_denied",
                    "DelegationGrant expired before the publication transaction",
                ));
            }
            Ok(())
        };
        let reflog = match self
            .repository
            .refs
            .compare_and_swap_with_preconditions_and_guard(
                update,
                &[precondition],
                &validator,
                &transaction_guard,
            ) {
            Ok(entry) => entry,
            Err(RefStoreError::PreconditionFailed {
                ref_name,
                expected_head,
                actual_head,
            }) => {
                return Err(CoreError::new(
                    ErrorCode::StaleBase,
                    format!(
                        "ContextPack base Ref {ref_name:?} is stale: expected {expected_head:?}, actual {actual_head:?}"
                    ),
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        };

        Ok(AuthorizationDecision {
            reflog,
            actor_record_oid: self.authority.actor_record_oid.to_owned(),
            activity_oid: request.activity_oid.to_owned(),
            context_pack_oid: context_pack_oid.to_owned(),
            delegation_grant_oid: grant_oid.to_owned(),
            policy_oid: resolved.policy_oid,
            effective_capabilities: requested_capabilities.into_iter().collect(),
        })
    }
}

pub(crate) fn require_oid_kind(oid: &str, expected: ObjectKind, label: &str) -> Result<()> {
    let actual = parse_oid(oid)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::ReferenceTypeMismatch,
            format!("{label} is not a {} OID: {oid}", expected.prefix()),
        )
        .into())
    }
}

struct ResolvedAiAuthority {
    context_payload: Value,
    grant_payload: Value,
    delegation_grant_oid: String,
    policy_oid: String,
    base_ref_name: String,
    base_commit_oid: String,
    target_ref_name: String,
    expected_target_head: Option<String>,
    side_effect_class: AiSideEffectClass,
    exact_capabilities: Vec<AiCapability>,
    grant_active_from_unix_nanos: i128,
    grant_expires_at_unix_nanos: i128,
    evaluated_at_unix_nanos: i128,
}

fn resolve_ai_authority<Clock>(
    repository: &Repository,
    authority: &AiExecutionAuthority<'_>,
    clock: &Clock,
    target: AiPublicationTarget<'_>,
) -> Result<ResolvedAiAuthority>
where
    Clock: AuthorizationClock,
{
    validate_ref_name(target.ref_name)?;
    require_ai_namespace(target.ref_name)?;
    if let Some(expected_head) = target.expected_head {
        validate_commit_oid(expected_head)?;
    }
    require_human_gated_base(authority.authorized_base_ref)?;

    let context = load_record(
        repository,
        authority.context_pack_oid,
        "context_pack",
        "ContextPack",
    )?;
    require_equal(
        require_string(&context, "asserted_by", "ContextPack asserted_by")?,
        authority.authorized_principal_id,
        "ContextPack must be asserted by the responsible principal",
    )?;
    let context_payload = require_object(&context, "payload", "ContextPack payload")?.clone();
    let grant_oid = require_string(
        &context_payload,
        "delegation_grant_ref",
        "ContextPack delegation_grant_ref",
    )?
    .to_owned();
    let policy_oid = require_string(
        &context_payload,
        "policy_snapshot_ref",
        "ContextPack policy_snapshot_ref",
    )?
    .to_owned();

    let grant = load_record(
        repository,
        &grant_oid,
        "delegation_grant",
        "DelegationGrant",
    )?;
    let grant_payload = require_object(&grant, "payload", "DelegationGrant payload")?.clone();
    require_equal(
        require_string(
            &grant_payload,
            "delegate_ref",
            "DelegationGrant delegate_ref",
        )?,
        authority.authenticated_actor_id,
        "DelegationGrant delegate does not match authenticated AI actor",
    )?;
    require_equal(
        require_string(
            &grant_payload,
            "principal_ref",
            "DelegationGrant principal_ref",
        )?,
        authority.authorized_principal_id,
        "DelegationGrant principal does not match the authorized principal",
    )?;
    require_equal(
        require_string(&grant, "asserted_by", "DelegationGrant asserted_by")?,
        authority.authorized_principal_id,
        "DelegationGrant must be asserted by its principal",
    )?;

    let policy = load_record(repository, &policy_oid, "policy", "Policy snapshot")?;
    require_equal(
        require_string(&policy, "asserted_by", "Policy asserted_by")?,
        authority.authorized_principal_id,
        "Policy snapshot must be asserted by the responsible principal",
    )?;

    let actor = load_record(
        repository,
        authority.actor_record_oid,
        "actor",
        "AI Actor snapshot",
    )?;
    require_equal(
        require_string(&actor, "entity_id", "AI Actor entity_id")?,
        authority.authenticated_actor_id,
        "AI Actor snapshot does not describe the authenticated actor",
    )?;
    let actor_payload = require_object(&actor, "payload", "AI Actor payload")?;
    require_equal(
        require_string(actor_payload, "actor_kind", "AI Actor kind")?,
        "ai_agent",
        "authorization Actor is not an ai_agent",
    )?;
    require_equal(
        require_string(&actor, "asserted_by", "AI Actor asserted_by")?,
        authority.authorized_principal_id,
        "AI Actor snapshot must be asserted by the responsible principal",
    )?;

    let principal_actor = load_record(
        repository,
        authority.principal_actor_record_oid,
        "actor",
        "Principal Actor snapshot",
    )?;
    require_equal(
        require_string(&principal_actor, "entity_id", "Principal Actor entity_id")?,
        authority.authorized_principal_id,
        "Principal Actor snapshot does not describe the authorized principal",
    )?;
    require_equal(
        require_string(
            &principal_actor,
            "asserted_by",
            "Principal Actor asserted_by",
        )?,
        authority.authorized_principal_id,
        "Principal Actor snapshot must be self-asserted",
    )?;
    let principal_kind = require_string(
        require_object(&principal_actor, "payload", "Principal Actor payload")?,
        "actor_kind",
        "Principal Actor kind",
    )?;
    if !matches!(principal_kind, "human" | "organization") {
        return Err(denied(
            "direct Stage 0 delegation requires a human or organization principal",
        ));
    }

    let base_commit = require_string(&context_payload, "base_commit", "ContextPack base_commit")?;
    let expected_base = require_string(
        &context_payload,
        "expected_ref_head",
        "ContextPack expected_ref_head",
    )?;
    require_equal(
        base_commit,
        expected_base,
        "ContextPack base_commit and expected_ref_head differ",
    )?;
    require_array_contains(
        &context_payload,
        "selected_context_refs",
        base_commit,
        "ContextPack selected_context_refs does not include its base Commit",
    )?;
    require_present_oid_array(
        repository,
        &context_payload,
        "selected_context_refs",
        "ContextPack selected context",
    )?;
    require_base_snapshot_bindings(
        repository,
        base_commit,
        &[
            authority.actor_record_oid,
            authority.principal_actor_record_oid,
            &grant_oid,
            &policy_oid,
        ],
    )?;

    let base_ref_name = require_string(
        &context_payload,
        "base_ref_name",
        "ContextPack base_ref_name",
    )?;
    require_equal(
        base_ref_name,
        authority.authorized_base_ref,
        "ContextPack base Ref does not match the authenticated runtime base Ref",
    )?;
    let project_id = require_string(&grant_payload, "project_ref", "DelegationGrant project_ref")?;
    require_equal(
        project_id,
        authority.authorized_project_id,
        "DelegationGrant project does not match the authenticated runtime project",
    )?;
    require_array_contains(
        require_object(&policy, "payload", "Policy payload")?,
        "scope_refs",
        project_id,
        "Policy snapshot does not cover the DelegationGrant project",
    )?;
    require_array_contains(
        &grant_payload,
        "data_classes",
        require_string(
            &context_payload,
            "data_classification",
            "ContextPack data_classification",
        )?,
        "ContextPack data classification is outside the DelegationGrant",
    )?;
    require_project_resource(&grant_payload, project_id)?;
    require_writable_prefix(&grant_payload, target.ref_name)?;

    let exact_capabilities = authority
        .authorized_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if !exact_capabilities.contains(&AiCapability::ProposeBranch) {
        return Err(denied(
            "pre-authorized execution capabilities must include propose_branch",
        ));
    }
    let actor_capabilities = parse_capabilities(
        require_array(
            require_object(actor_payload, "ai_profile", "AI Actor ai_profile")?,
            "capabilities",
            "AI Actor capabilities",
        )?,
        "AI Actor capabilities",
    )?;
    let grant_capabilities = parse_capabilities(
        require_array(
            &grant_payload,
            "capabilities",
            "DelegationGrant capabilities",
        )?,
        "DelegationGrant capabilities",
    )?;
    let runtime_capabilities = authority
        .runtime_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for capability in &exact_capabilities {
        if !actor_capabilities.contains(capability)
            || !grant_capabilities.contains(capability)
            || !runtime_capabilities.contains(capability)
        {
            return Err(denied(format!(
                "effective capability intersection excludes {}",
                capability.as_str()
            )));
        }
        authorize_policy_capability(&policy, *capability, project_id, target.ref_name)?;
    }

    let active_from = canonical_timestamp_unix_nanos(require_string(
        &grant,
        "recorded_at",
        "DelegationGrant recorded_at",
    )?)?;
    let expires_at = canonical_timestamp_unix_nanos(require_string(
        &grant_payload,
        "expires_at",
        "DelegationGrant expires_at",
    )?)?;
    let evaluated_at = clock.now_unix_nanos().map_err(RepositoryError::Clock)?;
    if evaluated_at < active_from {
        return Err(denied("DelegationGrant is not active yet"));
    }
    if evaluated_at >= expires_at {
        return Err(denied("DelegationGrant has expired"));
    }

    let base_ref_name = base_ref_name.to_owned();
    let base_commit_oid = base_commit.to_owned();

    Ok(ResolvedAiAuthority {
        context_payload,
        grant_payload,
        delegation_grant_oid: grant_oid,
        policy_oid,
        base_ref_name,
        base_commit_oid,
        target_ref_name: target.ref_name.to_owned(),
        expected_target_head: target.expected_head.map(str::to_owned),
        side_effect_class: target.side_effect_class,
        exact_capabilities: exact_capabilities.into_iter().collect(),
        grant_active_from_unix_nanos: active_from,
        grant_expires_at_unix_nanos: expires_at,
        evaluated_at_unix_nanos: evaluated_at,
    })
}

fn normalized_capabilities(capabilities: &[AiCapability]) -> Vec<AiCapability> {
    capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn require_preflight_authority_binding(
    authority: &AiExecutionAuthority<'_>,
    decision: &AiPreflightDecision,
) -> Result<()> {
    let matches = decision.authenticated_actor_id == authority.authenticated_actor_id
        && decision.authorized_project_id == authority.authorized_project_id
        && decision.authorized_principal_id == authority.authorized_principal_id
        && decision.authorized_base_ref == authority.authorized_base_ref
        && decision.actor_record_oid == authority.actor_record_oid
        && decision.principal_actor_record_oid == authority.principal_actor_record_oid
        && decision.context_pack_oid == authority.context_pack_oid
        && decision.exact_capabilities
            == normalized_capabilities(authority.authorized_capabilities)
        && decision.runtime_capabilities == normalized_capabilities(authority.runtime_capabilities);
    if matches {
        Ok(())
    } else {
        Err(denied(
            "preflight decision does not match the publication runtime authority",
        ))
    }
}

fn require_resolved_preflight_binding(
    resolved: &ResolvedAiAuthority,
    decision: &AiPreflightDecision,
) -> Result<()> {
    if resolved.evaluated_at_unix_nanos < decision.evaluated_at_unix_nanos {
        return Err(RepositoryError::Clock(
            "trusted authorization clock moved backwards since preflight".to_owned(),
        ));
    }
    let matches = resolved.delegation_grant_oid == decision.delegation_grant_oid
        && resolved.policy_oid == decision.policy_oid
        && resolved.base_ref_name == decision.authorized_base_ref
        && resolved.base_commit_oid == decision.base_commit_oid
        && resolved.target_ref_name == decision.target_ref_name
        && resolved.expected_target_head == decision.expected_target_head
        && resolved.side_effect_class == decision.side_effect_class
        && resolved.exact_capabilities == decision.exact_capabilities
        && resolved.grant_expires_at_unix_nanos == decision.grant_expires_at_unix_nanos;
    if matches {
        Ok(())
    } else {
        Err(denied(
            "publication authority no longer matches the exact preflight decision",
        ))
    }
}

fn stale_base(ref_name: &str, expected: &str, actual: Option<&str>) -> RepositoryError {
    CoreError::new(
        ErrorCode::StaleBase,
        format!(
            "ContextPack base Ref {ref_name:?} is stale: expected {:?}, actual {:?}",
            Some(expected),
            actual
        ),
    )
    .into()
}

fn require_ai_namespace(ref_name: &str) -> Result<()> {
    match ref_name.split('/').next() {
        Some("proposal") => Ok(()),
        Some("decision" | "release") => Err(CoreError::new(
            ErrorCode::HumanGateRequired,
            format!("AI cannot directly update human-gated Ref {ref_name:?}"),
        )
        .into()),
        _ => Err(denied(format!(
            "AI may publish only under proposal/*, not {ref_name:?}"
        ))),
    }
}

fn require_human_gated_base(ref_name: &str) -> Result<()> {
    validate_ref_name(ref_name)?;
    if matches!(ref_name.split('/').next(), Some("decision" | "release")) {
        Ok(())
    } else {
        Err(denied(format!(
            "Creative AI base Ref must be human-gated, not {ref_name:?}"
        )))
    }
}

pub(crate) fn load_structured(
    repository: &Repository,
    oid: &str,
    expected_kind: ObjectKind,
    label: &str,
) -> Result<Value> {
    if parse_oid(oid)? != expected_kind {
        return Err(CoreError::new(
            ErrorCode::ReferenceTypeMismatch,
            format!("{label} has the wrong OID kind: {oid}"),
        )
        .into());
    }
    let object = repository.objects.get_verified(oid)?.ok_or_else(|| {
        RepositoryError::Core(CoreError::new(
            ErrorCode::ClosureMissing,
            format!("{label} is missing: {oid}"),
        ))
    })?;
    let value = object.structured().cloned().ok_or_else(|| {
        RepositoryError::Core(CoreError::new(
            ErrorCode::ReferenceTypeMismatch,
            format!("{label} is not structured: {oid}"),
        ))
    })?;
    validate(&value)?;
    Ok(value)
}

pub(crate) fn load_record(
    repository: &Repository,
    oid: &str,
    expected_record_type: &str,
    label: &str,
) -> Result<Value> {
    let value = load_structured(repository, oid, ObjectKind::Record, label)?;
    require_equal(
        require_string(&value, "record_type", label)?,
        expected_record_type,
        format!("{label} has the wrong record_type"),
    )?;
    Ok(value)
}

pub(crate) fn require_object<'value>(
    value: &'value Value,
    key: &str,
    label: impl Into<String>,
) -> Result<&'value Value> {
    let label = label.into();
    let child = value
        .get(key)
        .ok_or_else(|| denied(format!("{label} is missing")))?;
    child
        .as_object()
        .ok_or_else(|| denied(format!("{label} is not an object")))?;
    Ok(child)
}

pub(crate) fn require_string<'value>(
    value: &'value Value,
    key: &str,
    label: impl Into<String>,
) -> Result<&'value str> {
    let label = label.into();
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| denied(format!("{label} is missing or not a string")))
}

pub(crate) fn require_array<'value>(
    value: &'value Value,
    key: &str,
    label: impl Into<String>,
) -> Result<&'value [Value]> {
    let label = label.into();
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| denied(format!("{label} is missing or not an array")))
}

pub(crate) fn require_equal(left: &str, right: &str, message: impl Into<String>) -> Result<()> {
    if left == right {
        Ok(())
    } else {
        Err(denied(message))
    }
}

pub(crate) fn require_array_contains(
    value: &Value,
    key: &str,
    expected: &str,
    message: impl Into<String>,
) -> Result<()> {
    if require_array(value, key, key)?
        .iter()
        .any(|value| value.as_str() == Some(expected))
    {
        Ok(())
    } else {
        Err(denied(message))
    }
}

pub(crate) fn require_role_actor(payload: &Value, role: &str, actor_id: &str) -> Result<()> {
    let matching = require_array(payload, "actor_refs", "AI Activity actor_refs")?
        .iter()
        .filter(|entry| entry.get("role").and_then(Value::as_str) == Some(role))
        .collect::<Vec<_>>();
    if matching.len() == 1 && matching[0].get("actor_ref").and_then(Value::as_str) == Some(actor_id)
    {
        Ok(())
    } else {
        Err(denied(format!(
            "AI Activity must bind actor role {role:?} exactly once to {actor_id:?}"
        )))
    }
}

pub(crate) fn require_role_oid(payload: &Value, key: &str, role: &str, oid: &str) -> Result<()> {
    let matching = require_array(payload, key, format!("AI Activity {key}"))?
        .iter()
        .filter(|entry| entry.get("role").and_then(Value::as_str) == Some(role))
        .collect::<Vec<_>>();
    if matching.len() == 1 && matching[0].get("oid").and_then(Value::as_str) == Some(oid) {
        Ok(())
    } else {
        Err(denied(format!(
            "AI Activity must bind {role:?} exactly once to {oid}"
        )))
    }
}

fn parse_capabilities(values: &[Value], label: &str) -> Result<BTreeSet<AiCapability>> {
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| denied(format!("{label} contains a non-string")))?;
            AiCapability::parse(value)
                .ok_or_else(|| denied(format!("{label} contains unknown capability {value:?}")))
        })
        .collect()
}

pub(crate) fn require_writable_prefix(grant_payload: &Value, ref_name: &str) -> Result<()> {
    let allowed = require_array(
        grant_payload,
        "writable_ref_prefixes",
        "DelegationGrant writable_ref_prefixes",
    )?
    .iter()
    .filter_map(Value::as_str)
    .any(|prefix| segment_prefix_matches(prefix, ref_name));
    if allowed {
        Ok(())
    } else {
        Err(denied(format!(
            "DelegationGrant does not allow writing Ref {ref_name:?}"
        )))
    }
}

pub(crate) fn require_project_resource(grant_payload: &Value, project_id: &str) -> Result<()> {
    let resource = format!("project/{project_id}");
    let allowed = require_array(
        grant_payload,
        "resource_selectors",
        "DelegationGrant resource_selectors",
    )?
    .iter()
    .filter_map(Value::as_str)
    .any(|selector| selector_matches(selector, &resource));
    if allowed {
        Ok(())
    } else {
        Err(denied(format!(
            "DelegationGrant does not cover project resource {resource:?}"
        )))
    }
}

fn segment_prefix_matches(prefix: &str, value: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    value == prefix
        || value
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub(crate) fn selector_matches(selector: &str, resource: &str) -> bool {
    if !selector_supported(selector) {
        return false;
    }
    if let Some(prefix) = selector.strip_suffix("/**") {
        return segment_prefix_matches(prefix, resource);
    }
    selector == resource
}

pub(crate) fn selector_supported(selector: &str) -> bool {
    let base = selector.strip_suffix("/**").unwrap_or(selector);
    !base.is_empty()
        && !base.contains('*')
        && !base.starts_with('/')
        && !base.ends_with('/')
        && base
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PolicyEffect {
    Allow,
    RequireHumanGate,
    Deny,
}

fn authorize_policy_capability(
    policy: &Value,
    capability: AiCapability,
    project_id: &str,
    target_ref: &str,
) -> Result<()> {
    let payload = require_object(policy, "payload", "Policy payload")?;
    let action = capability.policy_action();
    let project_resource = format!("project/{project_id}");
    let resource = if matches!(
        capability,
        AiCapability::ProposeBranch | AiCapability::RequestReview
    ) {
        target_ref
    } else {
        &project_resource
    };
    let mut effect: Option<PolicyEffect> = None;
    for rule in require_array(payload, "rules", "Policy rules")? {
        if rule.get("action").and_then(Value::as_str) != Some(action) {
            continue;
        }
        let Some(selector) = rule.get("resource_selector").and_then(Value::as_str) else {
            continue;
        };
        if !selector_supported(selector) {
            return Err(denied(format!(
                "Policy selector {selector:?} is unsupported by the Stage 0 runtime"
            )));
        }
        if !selector_matches(selector, resource) {
            continue;
        }
        let candidate = match rule.get("effect").and_then(Value::as_str) {
            Some("allow") if rule.get("condition_text").is_some() => {
                // Free-text conditions are not machine-verifiable and cannot
                // safely broaden an AI capability.
                return Err(denied(format!(
                    "Policy conditional allow cannot be evaluated for capability {}",
                    capability.as_str()
                )));
            }
            Some("allow") => PolicyEffect::Allow,
            Some("require_human_gate") => PolicyEffect::RequireHumanGate,
            Some("deny") => PolicyEffect::Deny,
            _ => continue,
        };
        effect = Some(effect.map_or(candidate, |current| current.max(candidate)));
    }
    let effect = effect.unwrap_or_else(|| {
        if payload.get("default_effect").and_then(Value::as_str) == Some("allow") {
            PolicyEffect::Allow
        } else {
            PolicyEffect::Deny
        }
    });
    match effect {
        PolicyEffect::Allow => Ok(()),
        PolicyEffect::RequireHumanGate => Err(CoreError::new(
            ErrorCode::HumanGateRequired,
            format!(
                "Policy requires a human gate for capability {} on {resource:?}",
                capability.as_str()
            ),
        )
        .into()),
        PolicyEffect::Deny => Err(denied(format!(
            "Policy denies capability {} on {resource:?}",
            capability.as_str()
        ))),
    }
}

pub(crate) struct CandidateBinding<'a> {
    pub(crate) candidate_commit: &'a str,
    pub(crate) base_commit: &'a str,
    pub(crate) activity_oid: &'a str,
    pub(crate) context_pack_oid: &'a str,
    pub(crate) authenticated_actor_id: &'a str,
    pub(crate) context_payload: &'a Value,
    pub(crate) activity_payload: &'a Value,
    pub(crate) grant_payload: &'a Value,
}

pub(crate) fn require_candidate_output_binding(
    repository: &Repository,
    binding: CandidateBinding<'_>,
) -> Result<BTreeSet<AiCapability>> {
    let CandidateBinding {
        candidate_commit,
        base_commit,
        activity_oid,
        context_pack_oid,
        authenticated_actor_id,
        context_payload,
        activity_payload,
        grant_payload,
    } = binding;
    let verifier = PreparedClosureVerifier::new(
        &repository.objects,
        repository.graph_limits,
        repository.tombstone_scan_limits,
    )?;
    let candidate = verifier.verify_uncached(candidate_commit)?;
    let base = verifier.verify_uncached(base_commit)?;
    if !candidate.is_complete() || !base.is_complete() {
        return Err(CoreError::new(
            ErrorCode::ClosureMissing,
            "candidate or ContextPack base closure is incomplete",
        )
        .into());
    }

    let candidate_commit_value = load_structured(
        repository,
        candidate_commit,
        ObjectKind::Commit,
        "candidate Commit",
    )?;
    let candidate_snapshot_root = require_string(
        &candidate_commit_value,
        "snapshot",
        "candidate Commit snapshot",
    )?
    .to_owned();
    let candidate_snapshot = snapshot_object_set(repository, candidate_commit)?;
    let base_snapshot = snapshot_object_set(repository, base_commit)?;
    let outputs = require_array(activity_payload, "output_refs", "AI Activity output_refs")?;
    let output_roots = outputs
        .iter()
        .map(|output| {
            output
                .get("oid")
                .and_then(Value::as_str)
                .ok_or_else(|| denied("AI Activity output_ref has no OID"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    for output in &output_roots {
        if !candidate_snapshot.contains(*output) {
            return Err(denied(format!(
                "AI Activity output {output} is not bound by the candidate snapshot"
            )));
        }
    }

    let mut outgoing = BTreeMap::<&str, Vec<&str>>::new();
    for edge in &candidate.edges {
        outgoing
            .entry(edge.source.as_str())
            .or_default()
            .push(edge.target.as_str());
    }
    let mut output_closure = BTreeSet::new();
    let mut pending = output_roots.iter().copied().collect::<Vec<_>>();
    while let Some(oid) = pending.pop() {
        if !output_closure.insert(oid) {
            continue;
        }
        if output_closure.len() > repository.graph_limits.max_objects {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "AI output closure exceeds configured object limit",
            )
            .into());
        }
        if let Some(targets) = outgoing.get(oid) {
            pending.extend(targets.iter().copied());
        }
    }

    let mut input_roots = require_array(
        context_payload,
        "selected_context_refs",
        "ContextPack selected_context_refs",
    )?
    .iter()
    .map(|value| {
        value
            .as_str()
            .ok_or_else(|| denied("ContextPack selected_context_refs contains a non-string OID"))
    })
    .collect::<Result<BTreeSet<_>>>()?;
    input_roots.insert(context_pack_oid);
    let mut input_closure = BTreeSet::new();
    let mut pending = input_roots.iter().copied().collect::<Vec<_>>();
    while let Some(oid) = pending.pop() {
        if !input_closure.insert(oid) {
            continue;
        }
        if input_closure.len() > repository.graph_limits.max_objects {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "ContextPack input closure exceeds configured object limit",
            )
            .into());
        }
        if let Some(targets) = outgoing.get(oid) {
            pending.extend(targets.iter().copied());
        }
    }
    for oid in &input_closure {
        let node = candidate.nodes.get(*oid).ok_or_else(|| {
            RepositoryError::Core(CoreError::new(
                ErrorCode::ClosureMissing,
                format!("ContextPack input is absent from candidate closure: {oid}"),
            ))
        })?;
        let kind = match &node.state {
            ClosureNodeState::Present { kind, .. } => *kind,
            _ => {
                return Err(CoreError::new(
                    ErrorCode::ClosureMissing,
                    format!("ContextPack input closure object is not present: {oid}"),
                )
                .into());
            }
        };
        if kind == ObjectKind::Record {
            let object = repository.objects.get_verified(oid)?.ok_or_else(|| {
                RepositoryError::Core(CoreError::new(
                    ErrorCode::ClosureMissing,
                    format!("ContextPack input Record disappeared: {oid}"),
                ))
            })?;
            let value = object.structured().ok_or_else(|| {
                RepositoryError::Core(CoreError::new(
                    ErrorCode::ReferenceTypeMismatch,
                    format!("ContextPack input Record is not structured: {oid}"),
                ))
            })?;
            validate(value)?;
        }
    }
    let produced_output_closure = output_closure
        .iter()
        .filter(|oid| output_roots.contains(*oid) || !input_closure.contains(*oid))
        .copied()
        .collect::<BTreeSet<_>>();

    let mut snapshot_parents = BTreeMap::<&str, Vec<&str>>::new();
    for edge in &candidate.edges {
        if candidate_snapshot.contains(&edge.source)
            && candidate_snapshot.contains(&edge.target)
            && parse_oid(&edge.source)? == ObjectKind::Tree
        {
            snapshot_parents
                .entry(edge.target.as_str())
                .or_default()
                .push(edge.source.as_str());
        }
    }
    let mut bound_snapshot_objects = candidate_snapshot
        .iter()
        .filter(|oid| {
            base_snapshot.contains(*oid)
                || oid.as_str() == activity_oid
                || oid.as_str() == context_pack_oid
                || produced_output_closure.contains(oid.as_str())
        })
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut pending = bound_snapshot_objects.iter().copied().collect::<Vec<_>>();
    while let Some(oid) = pending.pop() {
        if let Some(parents) = snapshot_parents.get(oid) {
            for parent in parents {
                if bound_snapshot_objects.insert(parent) {
                    pending.push(parent);
                }
            }
        }
    }

    for oid in &candidate_snapshot {
        let kind = parse_oid(oid)?;
        if base_snapshot.contains(oid)
            || (kind == ObjectKind::Tree
                && (oid == &candidate_snapshot_root
                    || bound_snapshot_objects.contains(oid.as_str())))
            || oid == activity_oid
            || oid == context_pack_oid
            || produced_output_closure.contains(oid.as_str())
        {
            continue;
        }
        return Err(denied(format!(
            "candidate snapshot introduces undeclared object {oid} outside the AI Activity outputs"
        )));
    }

    for (oid, node) in &candidate.nodes {
        if base.nodes.contains_key(oid)
            || oid == candidate_commit
            || oid == activity_oid
            || oid == context_pack_oid
            || input_closure.contains(oid.as_str())
            || matches!(
                &node.state,
                ClosureNodeState::Present {
                    kind: ObjectKind::Tree,
                    ..
                }
            )
        {
            continue;
        }
        if !produced_output_closure.contains(oid.as_str()) {
            return Err(denied(format!(
                "candidate introduces undeclared object {oid} outside the AI Activity outputs"
            )));
        }
    }

    let mut output_bytes = 0_u64;
    let mut counted_output_oids = BTreeSet::new();
    for oid in candidate_snapshot.difference(&base_snapshot) {
        if parse_oid(oid)? != ObjectKind::Tree {
            continue;
        }
        let tree = repository.objects.get_verified(oid)?.ok_or_else(|| {
            RepositoryError::Core(CoreError::new(
                ErrorCode::ClosureMissing,
                format!("candidate snapshot Tree is missing: {oid}"),
            ))
        })?;
        if counted_output_oids.insert(oid.to_owned()) {
            output_bytes = output_bytes.checked_add(tree.byte_len()).ok_or_else(|| {
                RepositoryError::Core(CoreError::new(
                    ErrorCode::ResourceLimit,
                    "AI output structure size overflow",
                ))
            })?;
        }
    }
    let mut required_capabilities =
        BTreeSet::from([AiCapability::ReadContext, AiCapability::ProposeBranch]);
    for oid in &produced_output_closure {
        let node = candidate.nodes.get(*oid).ok_or_else(|| {
            RepositoryError::Core(CoreError::new(
                ErrorCode::ClosureMissing,
                format!("AI output closure object is missing from candidate report: {oid}"),
            ))
        })?;
        let (kind, byte_len) = match &node.state {
            ClosureNodeState::Present { kind, byte_len } => (*kind, *byte_len),
            _ => {
                return Err(CoreError::new(
                    ErrorCode::ClosureMissing,
                    format!("AI output closure object is not present: {oid}"),
                )
                .into());
            }
        };
        if !base_snapshot.contains(*oid) && counted_output_oids.insert((*oid).to_owned()) {
            output_bytes = output_bytes.checked_add(byte_len).ok_or_else(|| {
                RepositoryError::Core(CoreError::new(
                    ErrorCode::ResourceLimit,
                    "AI output closure size overflow",
                ))
            })?;
        }
        if kind == ObjectKind::Record {
            let object = repository.objects.get_verified(oid)?.ok_or_else(|| {
                RepositoryError::Core(CoreError::new(
                    ErrorCode::ClosureMissing,
                    format!("AI output Record disappeared during authorization: {oid}"),
                ))
            })?;
            let value = object.structured().ok_or_else(|| {
                RepositoryError::Core(CoreError::new(
                    ErrorCode::ReferenceTypeMismatch,
                    format!("AI output Record is not structured: {oid}"),
                ))
            })?;
            validate(value)?;
            let record_type = value.get("record_type").and_then(Value::as_str);
            require_equal(
                require_string(value, "asserted_by", "AI output Record asserted_by")?,
                authenticated_actor_id,
                "AI output Record must be asserted by the authenticated agent",
            )?;
            match record_type {
                Some("analysis_result") => {
                    required_capabilities.insert(AiCapability::Analyze);
                }
                Some("claim") => {
                    if require_object(value, "payload", "AI output Claim payload")?
                        .get("ai_run_ref")
                        .is_some()
                    {
                        return Err(denied(
                            "Stage 0 AI output Claim must omit ai_run_ref; the current Activity output relation binds provenance without an OID cycle",
                        ));
                    }
                    required_capabilities.insert(AiCapability::SubmitClaim);
                }
                Some(record_type) => {
                    return Err(denied(format!(
                        "Stage 0 AI output cannot publish Record type {record_type}"
                    )));
                }
                None => return Err(denied("AI output Record has no record_type")),
            }
        } else if kind == ObjectKind::Commit {
            return Err(denied(
                "AI Activity outputs cannot introduce nested Commit objects",
            ));
        }
    }
    for output in outputs {
        match output.get("role").and_then(Value::as_str) {
            Some("preview") => {
                required_capabilities.insert(AiCapability::RenderPreview);
            }
            Some("review_request") => {
                required_capabilities.insert(AiCapability::RequestReview);
            }
            _ => {}
        }
    }

    if let Some(limit) = grant_payload
        .get("max_output_bytes")
        .and_then(Value::as_i64)
        && output_bytes > limit as u64
    {
        return Err(denied(format!(
            "AI output closure totals {output_bytes} bytes, exceeding DelegationGrant limit {limit}"
        )));
    }
    Ok(required_capabilities)
}

pub(crate) fn require_present_activity_references(
    repository: &Repository,
    activity_payload: &Value,
) -> Result<()> {
    for field in ["input_refs", "output_refs"] {
        for reference in require_array(activity_payload, field, format!("AI Activity {field}"))? {
            let oid = reference
                .get("oid")
                .and_then(Value::as_str)
                .ok_or_else(|| denied(format!("AI Activity {field} entry has no OID")))?;
            if repository.objects.get_verified(oid)?.is_none() {
                return Err(CoreError::new(
                    ErrorCode::ClosureMissing,
                    format!("AI Activity {field} object is not present: {oid}"),
                )
                .into());
            }
        }
    }
    for field in ["before_observation_refs", "after_observation_refs"] {
        if activity_payload.get(field).is_some() {
            require_present_oid_array(
                repository,
                activity_payload,
                field,
                "AI Activity reference",
            )?;
        }
    }
    let ai_run = require_object(activity_payload, "ai_run", "AI Activity ai_run")?;
    if ai_run.get("prompt_refs").is_some() {
        require_present_oid_array(repository, ai_run, "prompt_refs", "AI Activity prompt")?;
    }
    Ok(())
}

pub(crate) fn require_present_oid_array(
    repository: &Repository,
    value: &Value,
    field: &str,
    label: &str,
) -> Result<()> {
    for oid in require_array(value, field, format!("{label} {field}"))? {
        let oid = oid
            .as_str()
            .ok_or_else(|| denied(format!("{label} {field} contains a non-string OID")))?;
        if repository.objects.get_verified(oid)?.is_none() {
            return Err(CoreError::new(
                ErrorCode::ClosureMissing,
                format!("{label} is not present: {oid}"),
            )
            .into());
        }
    }
    Ok(())
}

fn require_base_snapshot_bindings(
    repository: &Repository,
    base_commit: &str,
    required_oids: &[&str],
) -> Result<()> {
    let snapshot = snapshot_object_set(repository, base_commit)?;
    for oid in required_oids {
        if !snapshot.contains(*oid) {
            return Err(denied(format!(
                "current ContextPack base snapshot does not bind authorization object {oid}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn snapshot_object_set(
    repository: &Repository,
    commit_oid: &str,
) -> Result<BTreeSet<String>> {
    let commit = load_structured(
        repository,
        commit_oid,
        ObjectKind::Commit,
        "snapshot Commit",
    )?;
    let snapshot_oid = require_string(&commit, "snapshot", "Commit snapshot")?;
    let mut objects = BTreeSet::new();
    let mut traversed_trees = BTreeSet::new();
    let mut pending = vec![(snapshot_oid.to_owned(), 0_usize)];
    let mut edges = 0_usize;
    while let Some((tree_oid, depth)) = pending.pop() {
        if !traversed_trees.insert(tree_oid.clone()) {
            continue;
        }
        objects.insert(tree_oid.clone());
        if objects.len() > repository.graph_limits.max_objects
            || depth > repository.graph_limits.max_depth
        {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "snapshot traversal exceeds configured graph limits",
            )
            .into());
        }
        let tree = load_structured(repository, &tree_oid, ObjectKind::Tree, "snapshot Tree")?;
        let entries = require_object(&tree, "entries", "Tree entries")?
            .as_object()
            .ok_or_else(|| denied("Tree entries is not an object"))?;
        for (_, entry) in entries {
            let oid = require_string(entry, "oid", "Tree entry OID")?;
            edges += 1;
            if edges > repository.graph_limits.max_edges {
                return Err(CoreError::new(
                    ErrorCode::ResourceLimit,
                    "snapshot traversal exceeds configured edge limit",
                )
                .into());
            }
            objects.insert(oid.to_owned());
            if parse_oid(oid)? == ObjectKind::Tree {
                pending.push((oid.to_owned(), depth + 1));
            }
        }
    }
    Ok(objects)
}

fn require_candidate_preserves_base_snapshot_objects(
    repository: &Repository,
    candidate_commit: &str,
    base_commit: &str,
) -> Result<()> {
    let base = snapshot_object_set(repository, base_commit)?;
    let candidate = snapshot_object_set(repository, candidate_commit)?;
    for oid in base {
        if parse_oid(&oid)? != ObjectKind::Tree && !candidate.contains(&oid) {
            return Err(denied(format!(
                "AI proposal snapshot does not retain base snapshot object {oid}"
            )));
        }
    }
    Ok(())
}

fn require_direct_base_parent(candidate: &Value, base_commit: &str) -> Result<()> {
    let parents = require_array(candidate, "parents", "candidate Commit parents")?;
    if parents.len() == 1 && parents[0].as_str() == Some(base_commit) {
        Ok(())
    } else {
        Err(denied(
            "Stage 0 AI proposal Commit must have exactly the ContextPack base Commit as its sole parent",
        ))
    }
}

fn canonical_timestamp_unix_nanos(value: &str) -> Result<i128> {
    let bytes = value.as_bytes();
    let lexical = bytes.len() == 30
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[29] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 29) || byte.is_ascii_digit()
        });
    if !lexical {
        return Err(CoreError::new(
            ErrorCode::TimestampInvalid,
            format!("invalid canonical timestamp {value:?}"),
        )
        .into());
    }
    let number = |start: usize, end: usize| -> i128 {
        bytes[start..end].iter().fold(0_i128, |number, digit| {
            number * 10 + i128::from(digit - b'0')
        })
    };
    let year = number(0, 4);
    let month = number(5, 7);
    let day = number(8, 10);
    let hour = number(11, 13);
    let minute = number(14, 16);
    let second = number(17, 19);
    let nanos = number(20, 29);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31_i128,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month)
        || day < 1
        || day > month_days[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(CoreError::new(
            ErrorCode::TimestampInvalid,
            format!("invalid calendar timestamp {value:?}"),
        )
        .into());
    }
    let days_before_year =
        |year: i128| 365 * year + (year + 3) / 4 - (year + 99) / 100 + (year + 399) / 400;
    let preceding_month_days = month_days[..(month - 1) as usize].iter().sum::<i128>();
    let days = days_before_year(year) - days_before_year(1970) + preceding_month_days + day - 1;
    Ok((((days * 24 + hour) * 60 + minute) * 60 + second) * 1_000_000_000 + nanos)
}

fn denied(message: impl Into<String>) -> RepositoryError {
    CoreError::new(ErrorCode::AuthorizationDenied, message).into()
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_timestamp_unix_nanos, segment_prefix_matches, selector_matches,
        selector_supported,
    };

    #[test]
    fn segment_prefixes_do_not_match_nearby_siblings() {
        assert!(segment_prefix_matches(
            "proposal/agent",
            "proposal/agent/run-1"
        ));
        assert!(segment_prefix_matches(
            "proposal/agent/",
            "proposal/agent/run-1"
        ));
        assert!(segment_prefix_matches("proposal/agent", "proposal/agent"));
        assert!(!segment_prefix_matches(
            "proposal/agent",
            "proposal/agent-evil/run-1"
        ));
        assert!(!segment_prefix_matches(
            "proposal/agent/2",
            "proposal/agent/20"
        ));
    }

    #[test]
    fn policy_selectors_support_only_exact_or_terminal_subtrees() {
        assert!(selector_matches("project/**", "project/urn:uuid:1"));
        assert!(selector_matches(
            "proposal/agent/**",
            "proposal/agent/run-1"
        ));
        assert!(selector_matches("proposal/agent", "proposal/agent"));
        assert!(!selector_matches(
            "proposal/agent/**",
            "proposal/agent-evil/run-1"
        ));
        assert!(!selector_matches("project/1/**", "project/12/item"));
        assert!(!selector_matches("proposal/*/run", "proposal/agent/run"));
        assert!(selector_supported("proposal/agent/**"));
        assert!(selector_supported("project/exact"));
        assert!(!selector_supported("proposal/*/run"));
        assert!(!selector_supported("project/../secret"));
        assert!(!selector_supported("/project/**"));
    }

    #[test]
    fn canonical_timestamp_conversion_handles_epoch_and_leap_days() {
        assert_eq!(
            canonical_timestamp_unix_nanos("1970-01-01T00:00:00.000000000Z").unwrap(),
            0
        );
        assert_eq!(
            canonical_timestamp_unix_nanos("1969-12-31T23:59:59.000000000Z").unwrap(),
            -1_000_000_000
        );
        let leap_day = canonical_timestamp_unix_nanos("2000-02-29T00:00:00.000000000Z").unwrap();
        let next_day = canonical_timestamp_unix_nanos("2000-03-01T00:00:00.000000000Z").unwrap();
        assert_eq!(next_day - leap_day, 86_400_000_000_000);
        assert!(canonical_timestamp_unix_nanos("2000-00-01T00:00:00.000000000Z").is_err());
        assert!(canonical_timestamp_unix_nanos("2001-02-29T00:00:00.000000000Z").is_err());
    }
}
