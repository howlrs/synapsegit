//! Authenticated application boundary for Creative AI and Human Decision admission.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use synapse_core::{
    AiCapability, AiExecutionAuthority, AiGeneratedProposal, AiPreflightDecision,
    AiPublicationTarget, AiSideEffectClass, AuthorizationClock, AuthorizationDecision,
    CreativeAiRuntime, Repository, RepositoryError,
};

mod human;

pub use human::{
    AdmittedProposalHandle, DurableProposalBinding, HumanAuthorityProfileConfig,
    HumanAuthorityProfileHandle, HumanDecisionCandidate, HumanDecisionPermit,
    RegisteredHumanDecisionHandle,
};

static NEXT_APPLICATION_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// A credential verifier supplied by the embedding service.
///
/// The application passes only the credential to this trait. Project selectors
/// and execution handles are deliberately resolved after authentication.
pub trait Authenticator: Send + Sync {
    type Credential: ?Sized;

    fn authenticate(
        &self,
        credential: &Self::Credential,
    ) -> std::result::Result<AuthenticatedSession, AuthenticationFailure>;
}

/// A successfully authenticated process-local session.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthenticatedSession {
    actor_id: String,
    session_id: String,
}

impl fmt::Debug for AuthenticatedSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedSession")
            .field("actor_id", &self.actor_id)
            .field("session_id", &"<redacted>")
            .finish()
    }
}

impl AuthenticatedSession {
    pub fn new(
        actor_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> std::result::Result<Self, AuthenticationFailure> {
        let actor_id = actor_id.into();
        let session_id = session_id.into();
        if !valid_control_value(&actor_id) || !valid_control_value(&session_id) {
            return Err(AuthenticationFailure);
        }
        Ok(Self {
            actor_id,
            session_id,
        })
    }

    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// Deliberately detail-free authentication failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticationFailure;

impl fmt::Display for AuthenticationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("authentication failed")
    }
}

impl Error for AuthenticationFailure {}

/// The only generated values a trusted executor may return.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutedAiProposal {
    new_head: String,
    activity_oid: String,
    message: Option<String>,
}

impl ExecutedAiProposal {
    pub fn new(
        new_head: impl Into<String>,
        activity_oid: impl Into<String>,
        message: Option<impl Into<String>>,
    ) -> Self {
        Self {
            new_head: new_head.into(),
            activity_oid: activity_oid.into(),
            message: message.map(Into::into),
        }
    }

    pub fn new_head(&self) -> &str {
        &self.new_head
    }

    pub fn activity_oid(&self) -> &str {
        &self.activity_oid
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// Candidate-independent context passed only to the registered executor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiExecutionContext {
    execution_id: u64,
    actor_id: String,
    project_id: String,
    principal_id: String,
    base_ref_name: String,
    base_head: String,
    target_ref_name: String,
    expected_target_head: Option<String>,
    context_pack_oid: String,
    capabilities: Vec<AiCapability>,
    side_effect_class: AiSideEffectClass,
    not_after_unix_nanos: i128,
}

impl AiExecutionContext {
    pub const fn execution_id(&self) -> u64 {
        self.execution_id
    }

    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub fn base_ref_name(&self) -> &str {
        &self.base_ref_name
    }

    pub fn base_head(&self) -> &str {
        &self.base_head
    }

    pub fn target_ref_name(&self) -> &str {
        &self.target_ref_name
    }

    pub fn expected_target_head(&self) -> Option<&str> {
        self.expected_target_head.as_deref()
    }

    pub fn context_pack_oid(&self) -> &str {
        &self.context_pack_oid
    }

    pub fn capabilities(&self) -> &[AiCapability] {
        &self.capabilities
    }

    pub const fn side_effect_class(&self) -> AiSideEffectClass {
        self.side_effect_class
    }

    pub const fn not_after_unix_nanos(&self) -> i128 {
        self.not_after_unix_nanos
    }
}

/// A single trusted executor selected when the application is constructed.
pub trait AiExecutor: Send + Sync {
    fn execute(
        &self,
        context: &AiExecutionContext,
    ) -> std::result::Result<ExecutedAiProposal, ExecutionFailure>;
}

/// Detail-free failure from the trusted executor boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionFailure;

impl fmt::Display for ExecutionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("execution failed")
    }
}

impl Error for ExecutionFailure {}

/// Caller-selected project key. Validation and lookup occur only after auth.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProjectSelector(String);

impl ProjectSelector {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One project Repository installed by the trusted control plane.
pub struct RegisteredProject {
    selector: ProjectSelector,
    repository: Repository,
}

impl RegisteredProject {
    pub fn new(selector: ProjectSelector, repository: Repository) -> Self {
        Self {
            selector,
            repository,
        }
    }
}

/// Durable-style trusted authority profile. It never crosses the request plane.
#[derive(Clone)]
pub struct AiAuthorityProfileConfig {
    project: ProjectSelector,
    actor_id: String,
    principal_id: String,
    base_ref_name: String,
    actor_record_oid: String,
    principal_actor_record_oid: String,
    context_pack_oid: String,
    target_ref_name: String,
    exact_capabilities: Vec<AiCapability>,
    runtime_capabilities: Vec<AiCapability>,
    side_effect_class: AiSideEffectClass,
}

impl AiAuthorityProfileConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project: ProjectSelector,
        actor_id: impl Into<String>,
        principal_id: impl Into<String>,
        base_ref_name: impl Into<String>,
        actor_record_oid: impl Into<String>,
        principal_actor_record_oid: impl Into<String>,
        context_pack_oid: impl Into<String>,
        target_ref_name: impl Into<String>,
        exact_capabilities: Vec<AiCapability>,
        runtime_capabilities: Vec<AiCapability>,
        side_effect_class: AiSideEffectClass,
    ) -> Self {
        Self {
            project,
            actor_id: actor_id.into(),
            principal_id: principal_id.into(),
            base_ref_name: base_ref_name.into(),
            actor_record_oid: actor_record_oid.into(),
            principal_actor_record_oid: principal_actor_record_oid.into(),
            context_pack_oid: context_pack_oid.into(),
            target_ref_name: target_ref_name.into(),
            exact_capabilities,
            runtime_capabilities,
            side_effect_class,
        }
    }
}

/// Server-side handle for a reusable authority profile.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthorityProfileHandle {
    application_instance: u64,
    project: String,
    profile_serial: u64,
}

impl fmt::Debug for AuthorityProfileHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorityProfileHandle(<opaque>)")
    }
}

/// Opaque one-time execution registration presented after authentication.
#[derive(Eq, PartialEq)]
pub struct RegisteredExecutionHandle {
    application_instance: u64,
    project: String,
    registration_serial: u64,
}

impl fmt::Debug for RegisteredExecutionHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RegisteredExecutionHandle(<opaque>)")
    }
}

/// Opaque process-local one-shot execution permit.
///
/// It is intentionally neither `Clone`, `Copy`, nor serializable. It is passed
/// by reference so an authentication failure does not destroy a still-ready
/// permit. Once an authenticated attempt claims the matching registry entry,
/// every outcome burns it.
#[derive(Eq, PartialEq)]
pub struct AiExecutionPermit {
    application_instance: u64,
    permit_serial: u64,
}

/// A committed AI publication and the opaque capability for its exact proposal.
///
/// The proposal handle is created directly from the successful Core reflog
/// result. No fallible registry update occurs after the Ref transaction.
#[derive(Debug, Eq, PartialEq)]
pub struct AiPublicationReceipt {
    decision: AuthorizationDecision,
    admitted_proposal: AdmittedProposalHandle,
}

impl AiPublicationReceipt {
    /// Return the auditable Core authorization decision.
    pub fn decision(&self) -> &AuthorizationDecision {
        &self.decision
    }

    /// Return the opaque handle for the exact committed proposal.
    pub fn admitted_proposal(&self) -> &AdmittedProposalHandle {
        &self.admitted_proposal
    }

    /// Synonym emphasizing that the returned value is an opaque handle.
    pub fn admitted_proposal_handle(&self) -> &AdmittedProposalHandle {
        &self.admitted_proposal
    }

    /// Consume the receipt into its independently owned audit and handle parts.
    pub fn into_parts(self) -> (AuthorizationDecision, AdmittedProposalHandle) {
        (self.decision, self.admitted_proposal)
    }
}

impl std::ops::Deref for AiPublicationReceipt {
    type Target = AuthorizationDecision;

    fn deref(&self) -> &Self::Target {
        &self.decision
    }
}

impl fmt::Debug for AiExecutionPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AiExecutionPermit(<opaque>)")
    }
}

/// Stable public application errors.
#[derive(Debug)]
pub enum ApplicationError {
    AuthenticationRequired,
    ProjectAccessDenied,
    ExecutionPermitInvalid,
    ExecutionFailed,
    StaleBase,
    RefConflict,
    ConfigInvalid,
    ServiceUnavailable,
    Core(RepositoryError),
}

impl ApplicationError {
    pub fn code(&self) -> &str {
        match self {
            Self::AuthenticationRequired => "authentication_required",
            Self::ProjectAccessDenied => "project_access_denied",
            Self::ExecutionPermitInvalid => "execution_permit_invalid",
            Self::ExecutionFailed => "execution_failed",
            Self::StaleBase => "stale_base",
            Self::RefConflict => "ref_conflict",
            Self::ConfigInvalid => "configuration_invalid",
            Self::ServiceUnavailable => "service_unavailable",
            Self::Core(error) => error.code(),
        }
    }
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthenticationRequired => formatter.write_str("authentication required"),
            Self::ProjectAccessDenied => formatter.write_str("project access denied"),
            Self::ExecutionPermitInvalid => formatter.write_str("execution permit invalid"),
            Self::ExecutionFailed => formatter.write_str("execution failed"),
            Self::StaleBase => formatter.write_str("the accepted base changed"),
            Self::RefConflict => formatter.write_str("the proposal target changed"),
            Self::ConfigInvalid => formatter.write_str("application configuration invalid"),
            Self::ServiceUnavailable => formatter.write_str("application service unavailable"),
            Self::Core(error) => error.fmt(formatter),
        }
    }
}

impl Error for ApplicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, ApplicationError>;

struct ProjectSlot {
    repository: Mutex<Repository>,
    publication_gate: FairGate,
}

#[derive(Default)]
struct ProjectSecurity {
    epoch: u64,
    allowed_actors: BTreeSet<String>,
}

#[derive(Clone)]
struct AuthorityProfileRecord {
    generation: u64,
    suspended: bool,
    config: AiAuthorityProfileConfig,
}

struct ExecutionRegistrationRecord {
    project: String,
    profile_serial: u64,
    profile_generation: u64,
    expected_target_head: Option<String>,
}

struct PermitRecord {
    actor_id: String,
    session_id: String,
    project: String,
    security_epoch: u64,
    profile_serial: u64,
    profile_generation: u64,
    execution_serial: u64,
    not_after_unix_nanos: i128,
    last_observed_unix_nanos: i128,
    preflight: AiPreflightDecision,
}

struct SecurityState {
    next_serial: u64,
    projects: BTreeMap<String, ProjectSecurity>,
    profiles: BTreeMap<u64, AuthorityProfileRecord>,
    registrations: BTreeMap<u64, ExecutionRegistrationRecord>,
    permits: BTreeMap<u64, PermitRecord>,
    human_profiles: BTreeMap<u64, human::HumanAuthorityProfileRecord>,
    human_registrations: BTreeMap<u64, human::HumanDecisionRegistrationRecord>,
    human_permits: BTreeMap<u64, human::HumanDecisionPermitRecord>,
}

impl SecurityState {
    fn allocate_serial(&mut self) -> Result<u64> {
        let serial = self.next_serial;
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(ApplicationError::ServiceUnavailable)?;
        Ok(serial)
    }
}

/// Authenticated, project-scoped application embedding.
pub struct Application<A, E, C> {
    instance: u64,
    authenticator: A,
    executor: E,
    clock: C,
    permit_ttl_nanos: i128,
    projects: BTreeMap<String, Arc<ProjectSlot>>,
    security: Mutex<SecurityState>,
}

impl AiAuthorityProfileConfig {
    fn validate(&self) -> Result<()> {
        let values = [
            self.project.as_str(),
            &self.actor_id,
            &self.principal_id,
            &self.base_ref_name,
            &self.actor_record_oid,
            &self.principal_actor_record_oid,
            &self.context_pack_oid,
            &self.target_ref_name,
        ];
        if values.into_iter().any(|value| !valid_control_value(value))
            || !matches!(
                self.base_ref_name.split('/').next(),
                Some("decision" | "release")
            )
            || self.target_ref_name.split('/').next() != Some("proposal")
            || self.exact_capabilities.is_empty()
            || !self
                .exact_capabilities
                .contains(&AiCapability::ProposeBranch)
            || !unique_capabilities(&self.exact_capabilities)
            || !unique_capabilities(&self.runtime_capabilities)
            || !self
                .exact_capabilities
                .iter()
                .all(|capability| self.runtime_capabilities.contains(capability))
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        Ok(())
    }

    fn authority(&self) -> AiExecutionAuthority<'_> {
        AiExecutionAuthority::new(
            &self.actor_id,
            self.project.as_str(),
            &self.principal_id,
            &self.base_ref_name,
            &self.actor_record_oid,
            &self.principal_actor_record_oid,
            &self.context_pack_oid,
            &self.exact_capabilities,
            &self.runtime_capabilities,
        )
    }
}

impl<A, E, C> Application<A, E, C>
where
    A: Authenticator,
    E: AiExecutor,
    C: AuthorizationClock + Send + Sync,
{
    /// Construct one process-local application instance.
    pub fn new(
        authenticator: A,
        executor: E,
        clock: C,
        permit_ttl_nanos: i128,
        projects: impl IntoIterator<Item = RegisteredProject>,
    ) -> Result<Self> {
        if permit_ttl_nanos <= 0 {
            return Err(ApplicationError::ConfigInvalid);
        }
        let instance = allocate_application_instance()?;
        let mut slots = BTreeMap::new();
        let mut project_security = BTreeMap::new();
        for registered in projects {
            let name = registered.selector.0;
            if !valid_control_value(&name) || slots.contains_key(&name) {
                return Err(ApplicationError::ConfigInvalid);
            }
            project_security.insert(name.clone(), ProjectSecurity::default());
            slots.insert(
                name,
                Arc::new(ProjectSlot {
                    repository: Mutex::new(registered.repository),
                    publication_gate: FairGate::new(),
                }),
            );
        }
        if slots.is_empty() {
            return Err(ApplicationError::ConfigInvalid);
        }
        Ok(Self {
            instance,
            authenticator,
            executor,
            clock,
            permit_ttl_nanos,
            projects: slots,
            security: Mutex::new(SecurityState {
                next_serial: 1,
                projects: project_security,
                profiles: BTreeMap::new(),
                registrations: BTreeMap::new(),
                permits: BTreeMap::new(),
                human_profiles: BTreeMap::new(),
                human_registrations: BTreeMap::new(),
                human_permits: BTreeMap::new(),
            }),
        })
    }

    /// Grant one authenticated actor access to a project.
    ///
    /// This is a trusted control-plane operation, not a caller route.
    pub fn grant_project_access(
        &self,
        project: &ProjectSelector,
        actor_id: impl Into<String>,
    ) -> Result<()> {
        let actor_id = actor_id.into();
        if !valid_control_value(&actor_id) {
            return Err(ApplicationError::ConfigInvalid);
        }
        let slot = self.control_project(project)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let security = state
            .projects
            .get_mut(project.as_str())
            .ok_or(ApplicationError::ConfigInvalid)?;
        if !security.allowed_actors.contains(&actor_id) {
            let epoch = next_epoch(security.epoch)?;
            security.allowed_actors.insert(actor_id);
            security.epoch = epoch;
        }
        Ok(())
    }

    /// Revoke project access and fence every outstanding permit for it.
    pub fn revoke_project_access(&self, project: &ProjectSelector, actor_id: &str) -> Result<()> {
        let slot = self.control_project(project)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let security = state
            .projects
            .get_mut(project.as_str())
            .ok_or(ApplicationError::ConfigInvalid)?;
        if security.allowed_actors.contains(actor_id) {
            let epoch = next_epoch(security.epoch)?;
            security.allowed_actors.remove(actor_id);
            security.epoch = epoch;
        }
        Ok(())
    }

    /// Install a reusable trusted authority profile.
    pub fn register_authority_profile(
        &self,
        mut config: AiAuthorityProfileConfig,
    ) -> Result<AuthorityProfileHandle> {
        config.exact_capabilities.sort_unstable();
        config.runtime_capabilities.sort_unstable();
        config.validate()?;
        let slot = self.control_project(&config.project)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let serial = state.allocate_serial()?;
        state.profiles.insert(
            serial,
            AuthorityProfileRecord {
                generation: 1,
                suspended: false,
                config: config.clone(),
            },
        );
        Ok(AuthorityProfileHandle {
            application_instance: self.instance,
            project: config.project.0,
            profile_serial: serial,
        })
    }

    /// Replace a profile and invalidate all permits issued from its old generation.
    pub fn replace_authority_profile(
        &self,
        handle: &AuthorityProfileHandle,
        mut replacement: AiAuthorityProfileConfig,
    ) -> Result<()> {
        replacement.exact_capabilities.sort_unstable();
        replacement.runtime_capabilities.sort_unstable();
        replacement.validate()?;
        if handle.application_instance != self.instance
            || handle.project != replacement.project.as_str()
        {
            return Err(ApplicationError::ConfigInvalid);
        }
        let slot = self.control_project(&replacement.project)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let next_generation = {
            let profile = state
                .profiles
                .get(&handle.profile_serial)
                .ok_or(ApplicationError::ConfigInvalid)?;
            next_epoch(profile.generation)?
        };
        let security = state
            .projects
            .get_mut(replacement.project.as_str())
            .ok_or(ApplicationError::ConfigInvalid)?;
        security.epoch = next_epoch(security.epoch)?;
        state.profiles.insert(
            handle.profile_serial,
            AuthorityProfileRecord {
                generation: next_generation,
                suspended: false,
                config: replacement,
            },
        );
        Ok(())
    }

    /// Suspend or resume a profile under the same FIFO publication fence.
    pub fn set_authority_profile_suspended(
        &self,
        handle: &AuthorityProfileHandle,
        suspended: bool,
    ) -> Result<()> {
        if handle.application_instance != self.instance {
            return Err(ApplicationError::ConfigInvalid);
        }
        let selector = ProjectSelector::new(handle.project.clone());
        let slot = self.control_project(&selector)?;
        let _gate = slot.publication_gate.enter()?;
        let mut state = lock(&self.security)?;
        let changed = state
            .profiles
            .get(&handle.profile_serial)
            .is_some_and(|profile| profile.suspended != suspended);
        if changed {
            let next_generation = next_epoch(
                state
                    .profiles
                    .get(&handle.profile_serial)
                    .ok_or(ApplicationError::ConfigInvalid)?
                    .generation,
            )?;
            let next_security_epoch = next_epoch(
                state
                    .projects
                    .get(&handle.project)
                    .ok_or(ApplicationError::ConfigInvalid)?
                    .epoch,
            )?;
            let profile = state
                .profiles
                .get_mut(&handle.profile_serial)
                .ok_or(ApplicationError::ConfigInvalid)?;
            profile.suspended = suspended;
            profile.generation = next_generation;
            let security = state
                .projects
                .get_mut(&handle.project)
                .ok_or(ApplicationError::ConfigInvalid)?;
            security.epoch = next_security_epoch;
        } else if !state.profiles.contains_key(&handle.profile_serial) {
            return Err(ApplicationError::ConfigInvalid);
        }
        Ok(())
    }

    /// Register one execution and seal the current target Ref expectation.
    pub fn register_execution(
        &self,
        profile_handle: &AuthorityProfileHandle,
    ) -> Result<RegisteredExecutionHandle> {
        if profile_handle.application_instance != self.instance {
            return Err(ApplicationError::ConfigInvalid);
        }
        let selector = ProjectSelector::new(profile_handle.project.clone());
        let slot = self.control_project(&selector)?;
        let _gate = slot.publication_gate.enter()?;

        let profile = {
            let state = lock(&self.security)?;
            let profile = state
                .profiles
                .get(&profile_handle.profile_serial)
                .filter(|profile| !profile.suspended)
                .ok_or(ApplicationError::ConfigInvalid)?;
            if profile.config.project.as_str() != profile_handle.project {
                return Err(ApplicationError::ConfigInvalid);
            }
            profile.clone()
        };
        let expected_target_head = {
            let repository = lock(&slot.repository)?;
            repository
                .refs()
                .get(&profile.config.target_ref_name)
                .map_err(|_| ApplicationError::ServiceUnavailable)?
                .map(|record| record.head)
        };
        let mut state = lock(&self.security)?;
        let live = state
            .profiles
            .get(&profile_handle.profile_serial)
            .filter(|live| {
                !live.suspended
                    && live.generation == profile.generation
                    && live.config.project.as_str() == profile_handle.project
            })
            .ok_or(ApplicationError::ConfigInvalid)?;
        let generation = live.generation;
        let serial = state.allocate_serial()?;
        state.registrations.insert(
            serial,
            ExecutionRegistrationRecord {
                project: profile_handle.project.clone(),
                profile_serial: profile_handle.profile_serial,
                profile_generation: generation,
                expected_target_head,
            },
        );
        Ok(RegisteredExecutionHandle {
            application_instance: self.instance,
            project: profile_handle.project.clone(),
            registration_serial: serial,
        })
    }

    /// Authenticate, preflight immutable Core authority, and issue one permit.
    pub fn prepare_ai(
        &self,
        credential: &A::Credential,
        project: &ProjectSelector,
        execution: &RegisteredExecutionHandle,
    ) -> Result<AiExecutionPermit> {
        let session = self.authenticate(credential)?;
        let slot = self.request_project(project)?;
        let _gate = slot.publication_gate.enter()?;

        let (security_epoch, registration, profile) = {
            let state = lock(&self.security)?;
            let security = state
                .projects
                .get(project.as_str())
                .filter(|security| security.allowed_actors.contains(session.actor_id()))
                .ok_or(ApplicationError::ProjectAccessDenied)?;
            if execution.application_instance != self.instance
                || execution.project != project.as_str()
            {
                return Err(ApplicationError::ProjectAccessDenied);
            }
            let registration = state
                .registrations
                .get(&execution.registration_serial)
                .filter(|registration| registration.project == project.as_str())
                .ok_or(ApplicationError::ProjectAccessDenied)?;
            let profile = state
                .profiles
                .get(&registration.profile_serial)
                .filter(|profile| {
                    !profile.suspended
                        && profile.generation == registration.profile_generation
                        && profile.config.actor_id == session.actor_id()
                        && profile.config.project.as_str() == project.as_str()
                })
                .ok_or(ApplicationError::ProjectAccessDenied)?;
            (
                security.epoch,
                ExecutionRegistrationRecord {
                    project: registration.project.clone(),
                    profile_serial: registration.profile_serial,
                    profile_generation: registration.profile_generation,
                    expected_target_head: registration.expected_target_head.clone(),
                },
                profile.clone(),
            )
        };

        let preflight = {
            let mut repository = lock(&slot.repository)?;
            CreativeAiRuntime::with_clock(
                &mut repository,
                profile.config.authority(),
                PanicSafeClock(&self.clock),
            )
            .preflight_proposal(AiPublicationTarget::new(
                &profile.config.target_ref_name,
                registration.expected_target_head.as_deref(),
                profile.config.side_effect_class,
            ))
            .map_err(map_preflight_error)?
        };
        if !preflight_matches_profile(&preflight, &profile.config) {
            return Err(ApplicationError::ConfigInvalid);
        }
        let issued_at = preflight.evaluated_at_unix_nanos();
        let ttl_deadline = issued_at
            .checked_add(self.permit_ttl_nanos)
            .ok_or(ApplicationError::ServiceUnavailable)?;
        let not_after = ttl_deadline.min(preflight.grant_expires_at_unix_nanos());
        if issued_at >= not_after {
            return Err(ApplicationError::ConfigInvalid);
        }

        let mut state = lock(&self.security)?;
        let security = state
            .projects
            .get(project.as_str())
            .filter(|security| {
                security.epoch == security_epoch
                    && security.allowed_actors.contains(session.actor_id())
            })
            .ok_or(ApplicationError::ProjectAccessDenied)?;
        let _ = security;
        let live_registration = state
            .registrations
            .get(&execution.registration_serial)
            .filter(|live| {
                live.project == registration.project
                    && live.profile_serial == registration.profile_serial
                    && live.profile_generation == registration.profile_generation
                    && live.expected_target_head == registration.expected_target_head
            })
            .ok_or(ApplicationError::ProjectAccessDenied)?;
        let _ = live_registration;
        let live_profile = state
            .profiles
            .get(&registration.profile_serial)
            .filter(|live| {
                !live.suspended
                    && live.generation == registration.profile_generation
                    && live.config.actor_id == session.actor_id()
            })
            .ok_or(ApplicationError::ProjectAccessDenied)?;
        let _ = live_profile;

        let permit_serial = state.allocate_serial()?;
        state.registrations.remove(&execution.registration_serial);
        state.permits.insert(
            permit_serial,
            PermitRecord {
                actor_id: session.actor_id,
                session_id: session.session_id,
                project: project.0.clone(),
                security_epoch,
                profile_serial: registration.profile_serial,
                profile_generation: registration.profile_generation,
                execution_serial: execution.registration_serial,
                not_after_unix_nanos: not_after,
                last_observed_unix_nanos: issued_at,
                preflight,
            },
        );
        Ok(AiExecutionPermit {
            application_instance: self.instance,
            permit_serial,
        })
    }

    /// Execute and publish one preflighted proposal.
    ///
    /// Authentication failure occurs before permit lookup and leaves a ready
    /// permit usable. After successful authentication finds a matching ready
    /// registry entry, the entry is removed before every remaining check.
    pub fn execute_and_publish_ai(
        &self,
        credential: &A::Credential,
        permit: &AiExecutionPermit,
    ) -> Result<AiPublicationReceipt> {
        let session = self.authenticate(credential)?;
        if permit.application_instance != self.instance {
            return Err(ApplicationError::ExecutionPermitInvalid);
        }
        let mut record = {
            let mut state = lock(&self.security)?;
            state
                .permits
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
        let started_at = self.permit_time(&record, record.last_observed_unix_nanos)?;
        {
            let state = lock(&self.security)?;
            validate_live_permit(&state, &record, &session)?;
        }
        record.last_observed_unix_nanos = started_at;
        let context = execution_context(&record);
        let generated = catch_unwind(AssertUnwindSafe(|| self.executor.execute(&context)))
            .map_err(|_| ApplicationError::ExecutionFailed)?
            .map_err(|_| ApplicationError::ExecutionFailed)?;

        let final_session = self.authenticate(credential)?;
        if final_session != session {
            return Err(ApplicationError::ExecutionPermitInvalid);
        }
        let _gate = slot.publication_gate.enter()?;
        let checked_at = self.permit_time(&record, started_at)?;
        record.last_observed_unix_nanos = checked_at;
        let live_profile = {
            let state = lock(&self.security)?;
            validate_live_permit(&state, &record, &final_session)?;
            state
                .profiles
                .get(&record.profile_serial)
                .cloned()
                .ok_or(ApplicationError::ExecutionPermitInvalid)?
        };
        let mut repository = lock(&slot.repository)?;
        let deadline_clock = PermitDeadlineClock {
            inner: PanicSafeClock(&self.clock),
            floor_unix_nanos: checked_at,
            not_after_unix_nanos: record.not_after_unix_nanos,
            grant_expires_at_unix_nanos: record.preflight.grant_expires_at_unix_nanos(),
        };
        let core_generated = AiGeneratedProposal::new(
            generated.new_head(),
            generated.activity_oid(),
            generated.message(),
        );
        let decision = CreativeAiRuntime::with_clock(
            &mut repository,
            live_profile.config.authority(),
            deadline_clock,
        )
        .publish_preflighted(record.preflight, core_generated)
        .map_err(ApplicationError::Core)?;
        let admitted_proposal = AdmittedProposalHandle::from_committed(
            self.instance,
            record.project,
            decision.reflog.ref_name.clone(),
            decision.reflog.new_head.clone(),
        );
        Ok(AiPublicationReceipt {
            decision,
            admitted_proposal,
        })
    }

    fn authenticate(&self, credential: &A::Credential) -> Result<AuthenticatedSession> {
        catch_unwind(AssertUnwindSafe(|| {
            self.authenticator.authenticate(credential)
        }))
        .map_err(|_| ApplicationError::ServiceUnavailable)?
        .map_err(|_| ApplicationError::AuthenticationRequired)
    }

    fn request_project(&self, project: &ProjectSelector) -> Result<Arc<ProjectSlot>> {
        self.projects
            .get(project.as_str())
            .cloned()
            .ok_or(ApplicationError::ProjectAccessDenied)
    }

    fn control_project(&self, project: &ProjectSelector) -> Result<Arc<ProjectSlot>> {
        self.projects
            .get(project.as_str())
            .cloned()
            .ok_or(ApplicationError::ConfigInvalid)
    }

    fn permit_time(&self, permit: &PermitRecord, floor: i128) -> Result<i128> {
        let now = PanicSafeClock(&self.clock)
            .now_unix_nanos()
            .map_err(|_| ApplicationError::ExecutionPermitInvalid)?;
        if now < floor || now >= permit.not_after_unix_nanos {
            return Err(ApplicationError::ExecutionPermitInvalid);
        }
        Ok(now)
    }
}

struct PanicSafeClock<'a, C>(&'a C);

impl<C> AuthorizationClock for PanicSafeClock<'_, C>
where
    C: AuthorizationClock,
{
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        catch_unwind(AssertUnwindSafe(|| self.0.now_unix_nanos()))
            .map_err(|_| "trusted application clock panicked".to_owned())?
    }
}

struct PermitDeadlineClock<C> {
    inner: C,
    floor_unix_nanos: i128,
    not_after_unix_nanos: i128,
    grant_expires_at_unix_nanos: i128,
}

impl<C> AuthorizationClock for PermitDeadlineClock<C>
where
    C: AuthorizationClock,
{
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        let now = self.inner.now_unix_nanos()?;
        if now < self.floor_unix_nanos {
            return Err("trusted application clock moved backwards".to_owned());
        }
        if now >= self.not_after_unix_nanos {
            Ok(now.max(self.grant_expires_at_unix_nanos))
        } else {
            Ok(now)
        }
    }
}

struct FairGate {
    state: Mutex<FairGateState>,
    changed: Condvar,
}

#[derive(Default)]
struct FairGateState {
    next_ticket: u64,
    serving: u64,
    occupied: bool,
}

struct FairGateGuard<'a> {
    gate: &'a FairGate,
}

impl FairGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(FairGateState::default()),
            changed: Condvar::new(),
        }
    }

    fn enter(&self) -> Result<FairGateGuard<'_>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApplicationError::ServiceUnavailable)?;
        let ticket = state.next_ticket;
        state.next_ticket = state
            .next_ticket
            .checked_add(1)
            .ok_or(ApplicationError::ServiceUnavailable)?;
        while state.occupied || state.serving != ticket {
            state = self
                .changed
                .wait(state)
                .map_err(|_| ApplicationError::ServiceUnavailable)?;
        }
        state.occupied = true;
        Ok(FairGateGuard { gate: self })
    }
}

impl Drop for FairGateGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.occupied = false;
        state.serving = state
            .serving
            .checked_add(1)
            .expect("fair gate serving counter invariant");
        self.gate.changed.notify_all();
    }
}

fn valid_control_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

fn unique_capabilities(capabilities: &[AiCapability]) -> bool {
    capabilities.iter().copied().collect::<BTreeSet<_>>().len() == capabilities.len()
}

fn next_epoch(epoch: u64) -> Result<u64> {
    epoch
        .checked_add(1)
        .ok_or(ApplicationError::ServiceUnavailable)
}

fn allocate_application_instance() -> Result<u64> {
    NEXT_APPLICATION_INSTANCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| ApplicationError::ServiceUnavailable)
}

fn map_preflight_error(error: RepositoryError) -> ApplicationError {
    match error.code() {
        "stale_base" => ApplicationError::StaleBase,
        "ref_conflict" => ApplicationError::RefConflict,
        "storage_error" | "resource_limit" => ApplicationError::ServiceUnavailable,
        _ => ApplicationError::ConfigInvalid,
    }
}

fn preflight_matches_profile(
    preflight: &AiPreflightDecision,
    profile: &AiAuthorityProfileConfig,
) -> bool {
    preflight.actor_id() == profile.actor_id
        && preflight.project_id() == profile.project.as_str()
        && preflight.principal_id() == profile.principal_id
        && preflight.base_ref_name() == profile.base_ref_name
        && preflight.target_ref_name() == profile.target_ref_name
        && preflight.context_pack_oid() == profile.context_pack_oid
        && preflight.side_effect_class() == profile.side_effect_class
        && preflight.exact_capabilities() == profile.exact_capabilities
}

fn validate_live_permit(
    state: &SecurityState,
    permit: &PermitRecord,
    session: &AuthenticatedSession,
) -> Result<()> {
    let security = state
        .projects
        .get(&permit.project)
        .filter(|security| {
            security.epoch == permit.security_epoch
                && security.allowed_actors.contains(session.actor_id())
        })
        .ok_or(ApplicationError::ExecutionPermitInvalid)?;
    let _ = security;
    state
        .profiles
        .get(&permit.profile_serial)
        .filter(|profile| {
            !profile.suspended
                && profile.generation == permit.profile_generation
                && profile.config.project.as_str() == permit.project
                && profile.config.actor_id == session.actor_id()
        })
        .ok_or(ApplicationError::ExecutionPermitInvalid)?;
    Ok(())
}

fn execution_context(permit: &PermitRecord) -> AiExecutionContext {
    AiExecutionContext {
        execution_id: permit.execution_serial,
        actor_id: permit.preflight.actor_id().to_owned(),
        project_id: permit.preflight.project_id().to_owned(),
        principal_id: permit.preflight.principal_id().to_owned(),
        base_ref_name: permit.preflight.base_ref_name().to_owned(),
        base_head: permit.preflight.base_head().to_owned(),
        target_ref_name: permit.preflight.target_ref_name().to_owned(),
        expected_target_head: permit.preflight.expected_target_head().map(str::to_owned),
        context_pack_oid: permit.preflight.context_pack_oid().to_owned(),
        capabilities: permit.preflight.exact_capabilities().to_vec(),
        side_effect_class: permit.preflight.side_effect_class(),
        not_after_unix_nanos: permit.not_after_unix_nanos,
    }
}

fn lock<'a, T>(mutex: &'a Mutex<T>) -> Result<MutexGuard<'a, T>> {
    mutex
        .lock()
        .map_err(|_| ApplicationError::ServiceUnavailable)
}

#[cfg(test)]
mod tests;
