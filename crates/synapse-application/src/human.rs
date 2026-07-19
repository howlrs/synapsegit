//! Authenticated, one-shot Human Decision application boundary.

use super::{
    AiExecutor, Application, ApplicationError, Authenticator, PanicSafeClock, ProjectSelector,
    Result, SecurityState, lock, next_epoch, valid_control_value,
};
use std::fmt;
use synapse_core::{
    AuthorizationClock, HumanDecisionAuthority, HumanDecisionReceipt, HumanDecisionRuntime,
    HumanDecisionUpdate,
};

/// Opaque proof that this application instance committed one exact AI proposal.
#[derive(Eq, PartialEq)]
pub struct AdmittedProposalHandle {
    pub(crate) application_instance: u64,
    pub(crate) project: String,
    pub(crate) proposal_ref_name: String,
    pub(crate) proposal_head: String,
}

impl AdmittedProposalHandle {
    pub(crate) fn from_committed(
        application_instance: u64,
        project: String,
        proposal_ref_name: String,
        proposal_head: String,
    ) -> Self {
        Self {
            application_instance,
            project,
            proposal_ref_name,
            proposal_head,
        }
    }
}

impl fmt::Debug for AdmittedProposalHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdmittedProposalHandle(<opaque>)")
    }
}

/// Trusted, durable identifiers used to rebind one admitted proposal after restart.
///
/// This value is control-plane configuration, not a bearer capability. It is
/// intentionally not serializable; an embedding service persists its own
/// versioned journal representation and reconstructs this value only after
/// authenticating and authorizing the project. The application rechecks every
/// Ref/head binding before creating a normal process-local registration, and
/// final publication still performs the complete Core Human Decision validation.
#[derive(Clone, Eq, PartialEq)]
pub struct DurableProposalBinding {
    pub(crate) project: ProjectSelector,
    pub(crate) proposal_ref_name: String,
    pub(crate) proposal_head: String,
    pub(crate) decision_ref_name: String,
    pub(crate) decision_head: String,
}

impl DurableProposalBinding {
    pub fn new(
        project: ProjectSelector,
        proposal_ref_name: impl Into<String>,
        proposal_head: impl Into<String>,
        decision_ref_name: impl Into<String>,
        decision_head: impl Into<String>,
    ) -> Self {
        Self {
            project,
            proposal_ref_name: proposal_ref_name.into(),
            proposal_head: proposal_head.into(),
            decision_ref_name: decision_ref_name.into(),
            decision_head: decision_head.into(),
        }
    }

    pub fn project(&self) -> &ProjectSelector {
        &self.project
    }

    pub fn proposal_ref_name(&self) -> &str {
        &self.proposal_ref_name
    }

    pub fn proposal_head(&self) -> &str {
        &self.proposal_head
    }

    pub fn decision_ref_name(&self) -> &str {
        &self.decision_ref_name
    }

    pub fn decision_head(&self) -> &str {
        &self.decision_head
    }

    fn validate(&self) -> Result<()> {
        if [
            self.project.as_str(),
            &self.proposal_ref_name,
            &self.proposal_head,
            &self.decision_ref_name,
            &self.decision_head,
        ]
        .into_iter()
        .any(|value| !valid_control_value(value))
            || self
                .proposal_ref_name
                .strip_prefix("proposal/")
                .is_none_or(str::is_empty)
            || self
                .decision_ref_name
                .strip_prefix("decision/")
                .is_none_or(str::is_empty)
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        Ok(())
    }
}

impl fmt::Debug for DurableProposalBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DurableProposalBinding(<redacted trusted binding>)")
    }
}

/// Reusable, process-local trusted authority for one direct human reviewer.
#[derive(Clone)]
pub struct HumanAuthorityProfileConfig {
    pub(crate) project: ProjectSelector,
    pub(crate) human_id: String,
    pub(crate) decision_ref_name: String,
    pub(crate) human_actor_record_oid: String,
    pub(crate) policy_record_oid: String,
}

impl HumanAuthorityProfileConfig {
    pub fn new(
        project: ProjectSelector,
        human_id: impl Into<String>,
        decision_ref_name: impl Into<String>,
        human_actor_record_oid: impl Into<String>,
        policy_record_oid: impl Into<String>,
    ) -> Self {
        Self {
            project,
            human_id: human_id.into(),
            decision_ref_name: decision_ref_name.into(),
            human_actor_record_oid: human_actor_record_oid.into(),
            policy_record_oid: policy_record_oid.into(),
        }
    }

    pub fn project(&self) -> &ProjectSelector {
        &self.project
    }

    pub fn human_id(&self) -> &str {
        &self.human_id
    }

    pub fn decision_ref_name(&self) -> &str {
        &self.decision_ref_name
    }

    fn validate(&self) -> Result<()> {
        if [
            self.project.as_str(),
            &self.human_id,
            &self.decision_ref_name,
            &self.human_actor_record_oid,
            &self.policy_record_oid,
        ]
        .into_iter()
        .any(|value| !valid_control_value(value))
            || self.decision_ref_name.split('/').next() != Some("decision")
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        Ok(())
    }

    fn authority<'a>(
        &'a self,
        decision_head: &'a str,
        proposal_ref_name: &'a str,
        proposal_head: &'a str,
    ) -> HumanDecisionAuthority<'a> {
        HumanDecisionAuthority::new(
            &self.human_id,
            self.project.as_str(),
            &self.decision_ref_name,
            decision_head,
            proposal_ref_name,
            proposal_head,
            &self.human_actor_record_oid,
            &self.policy_record_oid,
        )
    }
}

/// Server-side handle for a reusable Human Decision authority profile.
#[derive(Clone, Eq, PartialEq)]
pub struct HumanAuthorityProfileHandle {
    pub(crate) application_instance: u64,
    pub(crate) project: String,
    pub(crate) profile_serial: u64,
}

impl fmt::Debug for HumanAuthorityProfileHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HumanAuthorityProfileHandle(<opaque>)")
    }
}

/// Trusted-control-plane-selected candidate; Core still validates every OID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanDecisionCandidate {
    pub(crate) new_head: String,
    pub(crate) decision_feedback_oid: String,
    pub(crate) message: Option<String>,
}

impl HumanDecisionCandidate {
    pub fn new(
        new_head: impl Into<String>,
        decision_feedback_oid: impl Into<String>,
        message: Option<impl Into<String>>,
    ) -> Self {
        Self {
            new_head: new_head.into(),
            decision_feedback_oid: decision_feedback_oid.into(),
            message: message.map(Into::into),
        }
    }

    pub fn new_head(&self) -> &str {
        &self.new_head
    }

    pub fn decision_feedback_oid(&self) -> &str {
        &self.decision_feedback_oid
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    fn validate(&self) -> Result<()> {
        if !valid_control_value(&self.new_head)
            || !valid_control_value(&self.decision_feedback_oid)
            || self
                .message
                .as_ref()
                .is_some_and(|message| message.len() > 2_000 || !valid_control_value(message))
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        Ok(())
    }
}

/// Opaque one-time Human Decision registration.
#[derive(Eq, PartialEq)]
pub struct RegisteredHumanDecisionHandle {
    pub(crate) application_instance: u64,
    pub(crate) project: String,
    pub(crate) registration_serial: u64,
}

impl fmt::Debug for RegisteredHumanDecisionHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RegisteredHumanDecisionHandle(<opaque>)")
    }
}

/// Opaque process-local, one-shot Human Decision permit.
#[derive(Eq, PartialEq)]
pub struct HumanDecisionPermit {
    pub(crate) application_instance: u64,
    pub(crate) permit_serial: u64,
}

impl fmt::Debug for HumanDecisionPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HumanDecisionPermit(<opaque>)")
    }
}

#[derive(Clone)]
pub(crate) struct HumanAuthorityProfileRecord {
    pub(crate) generation: u64,
    pub(crate) suspended: bool,
    pub(crate) config: HumanAuthorityProfileConfig,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct HumanDecisionRegistrationRecord {
    pub(crate) project: String,
    pub(crate) profile_serial: u64,
    pub(crate) profile_generation: u64,
    pub(crate) decision_head: String,
    pub(crate) proposal_ref_name: String,
    pub(crate) proposal_head: String,
    pub(crate) candidate: HumanDecisionCandidate,
}

pub(crate) struct HumanDecisionPermitRecord {
    pub(crate) actor_id: String,
    pub(crate) session_id: String,
    pub(crate) project: String,
    pub(crate) security_epoch: u64,
    pub(crate) profile_serial: u64,
    pub(crate) profile_generation: u64,
    pub(crate) decision_head: String,
    pub(crate) proposal_ref_name: String,
    pub(crate) proposal_head: String,
    pub(crate) candidate: HumanDecisionCandidate,
    pub(crate) not_after_unix_nanos: i128,
    pub(crate) last_observed_unix_nanos: i128,
}

impl<A, E, C> Application<A, E, C>
where
    A: Authenticator,
    E: AiExecutor,
    C: AuthorizationClock + Send + Sync,
{
    /// Install reusable, process-local authority for one direct human reviewer.
    pub fn register_human_profile(
        &self,
        config: HumanAuthorityProfileConfig,
    ) -> Result<HumanAuthorityProfileHandle> {
        config.validate()?;
        let slot = self.control_project(&config.project)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let serial = state.allocate_serial()?;
        state.human_profiles.insert(
            serial,
            HumanAuthorityProfileRecord {
                generation: 1,
                suspended: false,
                config: config.clone(),
            },
        );
        Ok(HumanAuthorityProfileHandle {
            application_instance: self.instance,
            project: config.project.0,
            profile_serial: serial,
        })
    }

    /// Replace a Human Decision profile and fence every old-generation permit.
    pub fn replace_human_profile(
        &self,
        handle: &HumanAuthorityProfileHandle,
        replacement: HumanAuthorityProfileConfig,
    ) -> Result<()> {
        replacement.validate()?;
        if handle.application_instance != self.instance
            || handle.project != replacement.project.as_str()
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        let slot = self.control_project(&replacement.project)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let next_generation = state
            .human_profiles
            .get(&handle.profile_serial)
            .filter(|profile| profile.config.project.as_str() == handle.project)
            .map(|profile| next_epoch(profile.generation))
            .transpose()?
            .ok_or(ApplicationError::ConfigInvalid)?;
        let next_security_epoch = state
            .projects
            .get(replacement.project.as_str())
            .map(|security| next_epoch(security.epoch))
            .transpose()?
            .ok_or(ApplicationError::ConfigInvalid)?;
        state.human_profiles.insert(
            handle.profile_serial,
            HumanAuthorityProfileRecord {
                generation: next_generation,
                suspended: false,
                config: replacement,
            },
        );
        state
            .projects
            .get_mut(&handle.project)
            .ok_or(ApplicationError::ConfigInvalid)?
            .epoch = next_security_epoch;
        Ok(())
    }

    /// Suspend or resume Human Decision authority under the publication fence.
    pub fn set_human_profile_suspended(
        &self,
        handle: &HumanAuthorityProfileHandle,
        suspended: bool,
    ) -> Result<()> {
        if handle.application_instance != self.instance {
            return Err(ApplicationError::ConfigInvalid);
        }
        let selector = ProjectSelector::new(handle.project.clone());
        let slot = self.control_project(&selector)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let profile = state
            .human_profiles
            .get(&handle.profile_serial)
            .filter(|profile| profile.config.project.as_str() == handle.project)
            .ok_or(ApplicationError::ConfigInvalid)?;
        if profile.suspended == suspended {
            return Ok(());
        }
        let next_generation = next_epoch(profile.generation)?;
        let next_security_epoch = state
            .projects
            .get(&handle.project)
            .map(|security| next_epoch(security.epoch))
            .transpose()?
            .ok_or(ApplicationError::ConfigInvalid)?;
        let profile = state
            .human_profiles
            .get_mut(&handle.profile_serial)
            .ok_or(ApplicationError::ConfigInvalid)?;
        profile.suspended = suspended;
        profile.generation = next_generation;
        state
            .projects
            .get_mut(&handle.project)
            .ok_or(ApplicationError::ConfigInvalid)?
            .epoch = next_security_epoch;
        Ok(())
    }

    /// Register one exact proposal decision and seal the current decision head.
    ///
    /// The admitted proposal handle can only originate from this application's
    /// successful AI publication path. Candidate OIDs remain untrusted and are
    /// validated only by Core when the one-shot permit is published.
    pub fn register_human_decision(
        &self,
        profile_handle: &HumanAuthorityProfileHandle,
        admitted_proposal: &AdmittedProposalHandle,
        candidate: HumanDecisionCandidate,
    ) -> Result<RegisteredHumanDecisionHandle> {
        candidate.validate()?;
        if profile_handle.application_instance != self.instance
            || admitted_proposal.application_instance != self.instance
            || profile_handle.project != admitted_proposal.project
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        let selector = ProjectSelector::new(profile_handle.project.clone());
        let slot = self.control_project(&selector)?;
        let _gate = slot.publication_gate.enter()?;

        let profile = {
            let state = lock(&self.security)?;
            state
                .human_profiles
                .get(&profile_handle.profile_serial)
                .filter(|profile| {
                    !profile.suspended && profile.config.project.as_str() == profile_handle.project
                })
                .cloned()
                .ok_or(ApplicationError::ConfigInvalid)?
        };
        let decision_head = {
            let repository = lock(&slot.repository)?;
            let decision_head = repository
                .refs()
                .get(&profile.config.decision_ref_name)
                .map_err(|_| ApplicationError::ServiceUnavailable)?
                .map(|record| record.head)
                .ok_or(ApplicationError::ConfigInvalid)?;
            let current_proposal = repository
                .refs()
                .get(&admitted_proposal.proposal_ref_name)
                .map_err(|_| ApplicationError::ServiceUnavailable)?
                .map(|record| record.head);
            if current_proposal.as_deref() != Some(admitted_proposal.proposal_head.as_str()) {
                return Err(ApplicationError::ConfigInvalid);
            }
            decision_head
        };

        let mut state = lock(&self.security)?;
        state
            .human_profiles
            .get(&profile_handle.profile_serial)
            .filter(|live| {
                !live.suspended
                    && live.generation == profile.generation
                    && live.config.project.as_str() == profile_handle.project
            })
            .ok_or(ApplicationError::ConfigInvalid)?;
        let serial = state.allocate_serial()?;
        state.human_registrations.insert(
            serial,
            HumanDecisionRegistrationRecord {
                project: profile_handle.project.clone(),
                profile_serial: profile_handle.profile_serial,
                profile_generation: profile.generation,
                decision_head,
                proposal_ref_name: admitted_proposal.proposal_ref_name.clone(),
                proposal_head: admitted_proposal.proposal_head.clone(),
                candidate,
            },
        );
        Ok(RegisteredHumanDecisionHandle {
            application_instance: self.instance,
            project: profile_handle.project.clone(),
            registration_serial: serial,
        })
    }

    /// Rebind a journaled proposal to this process and register one Human decision.
    ///
    /// The durable binding is accepted only from the trusted control plane. It
    /// never restores an old handle or permit. Under the project publication
    /// fence, this method checks the live Human profile, exact Proposal Ref/head,
    /// and exact canonical Decision Ref/head before creating the same ordinary
    /// one-shot registration used by [`Self::register_human_decision`].
    pub fn register_recovered_human_decision(
        &self,
        profile_handle: &HumanAuthorityProfileHandle,
        binding: &DurableProposalBinding,
        candidate: HumanDecisionCandidate,
    ) -> Result<RegisteredHumanDecisionHandle> {
        candidate.validate()?;
        binding.validate()?;
        if profile_handle.application_instance != self.instance
            || profile_handle.project != binding.project.as_str()
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        let slot = self.control_project(&binding.project)?;
        let _gate = slot.publication_gate.enter()?;

        let profile = {
            let state = lock(&self.security)?;
            state
                .human_profiles
                .get(&profile_handle.profile_serial)
                .filter(|profile| {
                    !profile.suspended
                        && profile.config.project.as_str() == binding.project.as_str()
                        && profile.config.decision_ref_name == binding.decision_ref_name
                })
                .cloned()
                .ok_or(ApplicationError::ConfigInvalid)?
        };
        {
            let repository = lock(&slot.repository)?;
            let current_proposal = repository
                .refs()
                .get(&binding.proposal_ref_name)
                .map_err(|_| ApplicationError::ServiceUnavailable)?
                .map(|record| record.head);
            let current_decision = repository
                .refs()
                .get(&binding.decision_ref_name)
                .map_err(|_| ApplicationError::ServiceUnavailable)?
                .map(|record| record.head);
            if current_proposal.as_deref() != Some(binding.proposal_head.as_str()) {
                return Err(ApplicationError::RefConflict);
            }
            if current_decision.as_deref() != Some(binding.decision_head.as_str()) {
                return Err(ApplicationError::StaleBase);
            }
        }

        let mut state = lock(&self.security)?;
        state
            .human_profiles
            .get(&profile_handle.profile_serial)
            .filter(|live| {
                !live.suspended
                    && live.generation == profile.generation
                    && live.config.project.as_str() == binding.project.as_str()
                    && live.config.decision_ref_name == binding.decision_ref_name
            })
            .ok_or(ApplicationError::ConfigInvalid)?;
        let serial = state.allocate_serial()?;
        state.human_registrations.insert(
            serial,
            HumanDecisionRegistrationRecord {
                project: binding.project.0.clone(),
                profile_serial: profile_handle.profile_serial,
                profile_generation: profile.generation,
                decision_head: binding.decision_head.clone(),
                proposal_ref_name: binding.proposal_ref_name.clone(),
                proposal_head: binding.proposal_head.clone(),
                candidate,
            },
        );
        Ok(RegisteredHumanDecisionHandle {
            application_instance: self.instance,
            project: binding.project.0.clone(),
            registration_serial: serial,
        })
    }

    /// Authenticate and exchange one registration for a short-lived permit.
    ///
    /// Authentication occurs before selector, handle, state, or repository
    /// lookup. This phase deliberately performs no Core or repository work.
    pub fn prepare_human_decision(
        &self,
        credential: &A::Credential,
        project: &ProjectSelector,
        registration_handle: &RegisteredHumanDecisionHandle,
    ) -> Result<HumanDecisionPermit> {
        let session = self.authenticate(credential)?;
        let slot = self.request_project(project)?;
        let _gate = slot.publication_gate.enter()?;

        let (security_epoch, registration) = {
            let state = lock(&self.security)?;
            let security = state
                .projects
                .get(project.as_str())
                .filter(|security| security.allowed_actors.contains(session.actor_id()))
                .ok_or(ApplicationError::ProjectAccessDenied)?;
            if registration_handle.application_instance != self.instance
                || registration_handle.project != project.as_str()
            {
                return Err(ApplicationError::ProjectAccessDenied);
            }
            let registration = state
                .human_registrations
                .get(&registration_handle.registration_serial)
                .filter(|registration| registration.project == project.as_str())
                .ok_or(ApplicationError::ProjectAccessDenied)?;
            state
                .human_profiles
                .get(&registration.profile_serial)
                .filter(|profile| {
                    !profile.suspended
                        && profile.generation == registration.profile_generation
                        && profile.config.project.as_str() == project.as_str()
                        && profile.config.human_id == session.actor_id()
                })
                .ok_or(ApplicationError::ProjectAccessDenied)?;
            (security.epoch, registration.clone())
        };

        let issued_at = PanicSafeClock(&self.clock)
            .now_unix_nanos()
            .map_err(|_| ApplicationError::ServiceUnavailable)?;
        let not_after = issued_at
            .checked_add(self.permit_ttl_nanos)
            .ok_or(ApplicationError::ServiceUnavailable)?;
        if issued_at >= not_after {
            return Err(ApplicationError::ServiceUnavailable);
        }

        let mut state = lock(&self.security)?;
        state
            .projects
            .get(project.as_str())
            .filter(|security| {
                security.epoch == security_epoch
                    && security.allowed_actors.contains(session.actor_id())
            })
            .ok_or(ApplicationError::ProjectAccessDenied)?;
        state
            .human_registrations
            .get(&registration_handle.registration_serial)
            .filter(|live| *live == &registration)
            .ok_or(ApplicationError::ProjectAccessDenied)?;
        state
            .human_profiles
            .get(&registration.profile_serial)
            .filter(|profile| {
                !profile.suspended
                    && profile.generation == registration.profile_generation
                    && profile.config.project.as_str() == project.as_str()
                    && profile.config.human_id == session.actor_id()
            })
            .ok_or(ApplicationError::ProjectAccessDenied)?;

        let permit_serial = state.allocate_serial()?;
        state
            .human_registrations
            .remove(&registration_handle.registration_serial)
            .ok_or(ApplicationError::ProjectAccessDenied)?;
        state.human_permits.insert(
            permit_serial,
            HumanDecisionPermitRecord {
                actor_id: session.actor_id,
                session_id: session.session_id,
                project: project.0.clone(),
                security_epoch,
                profile_serial: registration.profile_serial,
                profile_generation: registration.profile_generation,
                decision_head: registration.decision_head,
                proposal_ref_name: registration.proposal_ref_name,
                proposal_head: registration.proposal_head,
                candidate: registration.candidate,
                not_after_unix_nanos: not_after,
                last_observed_unix_nanos: issued_at,
            },
        );
        Ok(HumanDecisionPermit {
            application_instance: self.instance,
            permit_serial,
        })
    }

    /// Burn one ready permit and publish its server-fixed decision through Core.
    ///
    /// Authentication failure happens before permit lookup. Once a matching
    /// same-application record is removed, every later outcome remains burned.
    /// There is no external executor, so the application does not invoke the
    /// authenticator again while holding or waiting for the project fence;
    /// process-local ACL and profile changes are the linearized revocation path.
    pub fn publish_human_decision(
        &self,
        credential: &A::Credential,
        permit: &HumanDecisionPermit,
    ) -> Result<HumanDecisionReceipt> {
        let session = self.authenticate(credential)?;
        if permit.application_instance != self.instance {
            return Err(ApplicationError::ExecutionPermitInvalid);
        }
        let mut record = {
            let mut state = lock(&self.security)?;
            state
                .human_permits
                .remove(&permit.permit_serial)
                .ok_or(ApplicationError::ExecutionPermitInvalid)?
        };
        if record.actor_id != session.actor_id || record.session_id != session.session_id {
            return Err(ApplicationError::ExecutionPermitInvalid);
        }
        let slot = self
            .projects
            .get(&record.project)
            .cloned()
            .ok_or(ApplicationError::ExecutionPermitInvalid)?;
        let started_at = human_permit_time(&self.clock, &record, record.last_observed_unix_nanos)?;
        record.last_observed_unix_nanos = started_at;

        let _gate = slot.publication_gate.enter()?;
        let checked_at = human_permit_time(&self.clock, &record, started_at)?;
        record.last_observed_unix_nanos = checked_at;
        let live_profile = {
            let state = lock(&self.security)?;
            validate_live_human_permit(&state, &record, &session)?;
            state
                .human_profiles
                .get(&record.profile_serial)
                .cloned()
                .ok_or(ApplicationError::ExecutionPermitInvalid)?
        };

        let mut repository = lock(&slot.repository)?;
        let deadline_clock = HumanPermitDeadlineClock {
            inner: PanicSafeClock(&self.clock),
            floor_unix_nanos: checked_at,
            not_after_unix_nanos: record.not_after_unix_nanos,
        };
        HumanDecisionRuntime::with_clock(
            &mut repository,
            live_profile.config.authority(
                &record.decision_head,
                &record.proposal_ref_name,
                &record.proposal_head,
            ),
            deadline_clock,
        )
        .publish_decision(HumanDecisionUpdate {
            new_head: &record.candidate.new_head,
            decision_feedback_oid: &record.candidate.decision_feedback_oid,
            message: record.candidate.message.as_deref(),
        })
        .map_err(ApplicationError::Core)
    }
}

fn human_permit_time<C>(clock: &C, permit: &HumanDecisionPermitRecord, floor: i128) -> Result<i128>
where
    C: AuthorizationClock,
{
    let now = PanicSafeClock(clock)
        .now_unix_nanos()
        .map_err(|_| ApplicationError::ExecutionPermitInvalid)?;
    if now < floor || now >= permit.not_after_unix_nanos {
        return Err(ApplicationError::ExecutionPermitInvalid);
    }
    Ok(now)
}

fn validate_live_human_permit(
    state: &SecurityState,
    permit: &HumanDecisionPermitRecord,
    session: &super::AuthenticatedSession,
) -> Result<()> {
    state
        .projects
        .get(&permit.project)
        .filter(|security| {
            security.epoch == permit.security_epoch
                && security.allowed_actors.contains(session.actor_id())
        })
        .ok_or(ApplicationError::ExecutionPermitInvalid)?;
    state
        .human_profiles
        .get(&permit.profile_serial)
        .filter(|profile| {
            !profile.suspended
                && profile.generation == permit.profile_generation
                && profile.config.project.as_str() == permit.project
                && profile.config.human_id == session.actor_id()
        })
        .ok_or(ApplicationError::ExecutionPermitInvalid)?;
    Ok(())
}

struct HumanPermitDeadlineClock<C> {
    inner: C,
    floor_unix_nanos: i128,
    not_after_unix_nanos: i128,
}

impl<C> AuthorizationClock for HumanPermitDeadlineClock<C>
where
    C: AuthorizationClock,
{
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        let now = self.inner.now_unix_nanos()?;
        if now < self.floor_unix_nanos {
            return Err("trusted application clock moved backwards".to_owned());
        }
        if now >= self.not_after_unix_nanos {
            return Err("Human Decision permit expired before publication".to_owned());
        }
        Ok(now)
    }
}
