//! Host-authenticated, process-local approval for one artifact Decision.

use crate::{
    ArtifactDecisionOptions, ArtifactDisposition, PendingArtifactProposal, PendingArtifactState,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use synapse_application::{
    AuthenticatedSession, AuthenticationFailure, Authenticator, DurableProposalBinding,
    ProjectSelector,
};
use synapse_core::AuthorizationClock;

static NEXT_APPROVAL_REGISTRY_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// Opaque, process-local approval for one exact generic artifact Decision.
///
/// The handle is neither clonable nor serializable. It is useful only with the
/// registry instance that issued it. After authentication and project access
/// checks, the registry consumes its server-side record before reading time or
/// inspecting caller-supplied Decision data.
#[must_use = "an artifact approval has no effect until it is claimed"]
#[derive(Eq, PartialEq)]
pub struct ArtifactDecisionApproval {
    registry_instance: u64,
    serial: u64,
}

impl fmt::Debug for ArtifactDecisionApproval {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactDecisionApproval(<opaque>)")
    }
}

/// Stable, detail-free failures from the host approval boundary.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ArtifactApprovalError {
    ConfigInvalid,
    AuthenticationFailed,
    ProjectAccessDenied,
    ApprovalInvalid,
    ServiceUnavailable,
}

impl ArtifactApprovalError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "artifact_approval_config_invalid",
            Self::AuthenticationFailed => "authentication_failed",
            Self::ProjectAccessDenied => "project_access_denied",
            Self::ApprovalInvalid => "artifact_approval_invalid",
            Self::ServiceUnavailable => "service_unavailable",
        }
    }
}

impl fmt::Debug for ArtifactApprovalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactApprovalError")
            .field("code", &self.code())
            .finish()
    }
}

impl fmt::Display for ArtifactApprovalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConfigInvalid => "artifact approval configuration is invalid",
            Self::AuthenticationFailed => "authentication failed",
            Self::ProjectAccessDenied => "project access denied",
            Self::ApprovalInvalid => "artifact approval is invalid",
            Self::ServiceUnavailable => "artifact approval service is unavailable",
        })
    }
}

impl Error for ArtifactApprovalError {}

/// Host-owned registry that issues expiring one-shot artifact approvals.
///
/// Authentication is delegated to the embedding host. Project membership is
/// configured independently in this registry and checked both when an approval
/// is issued and when it is claimed. Calling code must never populate that ACL
/// from browser request fields.
pub struct ArtifactApprovalRegistry<A, C> {
    instance: u64,
    authenticator: A,
    clock: C,
    ttl_nanos: i128,
    state: Mutex<ApprovalState>,
}

struct ApprovalState {
    next_serial: u64,
    projects: BTreeMap<String, ProjectApprovalSecurity>,
    approvals: BTreeMap<u64, ApprovalRecord>,
}

struct ProjectApprovalSecurity {
    epoch: u64,
    allowed_actors: BTreeSet<String>,
}

struct ApprovalRecord {
    actor_id: String,
    session_id: String,
    binding: DurableProposalBinding,
    intent_sha256: [u8; 32],
    security_epoch: u64,
    issued_at_unix_nanos: i128,
    not_after_unix_nanos: i128,
}

impl<A, C> ArtifactApprovalRegistry<A, C>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    pub fn new(
        authenticator: A,
        clock: C,
        ttl_nanos: i128,
    ) -> std::result::Result<Self, ArtifactApprovalError> {
        if ttl_nanos <= 0 {
            return Err(ArtifactApprovalError::ConfigInvalid);
        }
        let instance = NEXT_APPROVAL_REGISTRY_INSTANCE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1).filter(|next| *next != 0)
            })
            .map_err(|_| ArtifactApprovalError::ServiceUnavailable)?;
        Ok(Self {
            instance,
            authenticator,
            clock,
            ttl_nanos,
            state: Mutex::new(ApprovalState {
                next_serial: 1,
                projects: BTreeMap::new(),
                approvals: BTreeMap::new(),
            }),
        })
    }

    /// Grant one authenticated host actor access to one trusted project.
    pub fn grant_project_access(
        &self,
        project: &ProjectSelector,
        actor_id: impl Into<String>,
    ) -> std::result::Result<(), ArtifactApprovalError> {
        let actor_id = actor_id.into();
        if !valid_control_value(project.as_str()) || !valid_control_value(&actor_id) {
            return Err(ArtifactApprovalError::ConfigInvalid);
        }
        let mut state = lock(&self.state)?;
        let security = state
            .projects
            .entry(project.as_str().to_owned())
            .or_insert_with(|| ProjectApprovalSecurity {
                epoch: 1,
                allowed_actors: BTreeSet::new(),
            });
        if security.allowed_actors.insert(actor_id) {
            security.epoch = next_epoch(security.epoch)?;
        }
        Ok(())
    }

    /// Revoke one actor and invalidate every outstanding approval for project.
    pub fn revoke_project_access(
        &self,
        project: &ProjectSelector,
        actor_id: &str,
    ) -> std::result::Result<(), ArtifactApprovalError> {
        if !valid_control_value(project.as_str()) || !valid_control_value(actor_id) {
            return Err(ArtifactApprovalError::ConfigInvalid);
        }
        let mut state = lock(&self.state)?;
        if let Some(security) = state.projects.get_mut(project.as_str())
            && security.allowed_actors.remove(actor_id)
        {
            security.epoch = next_epoch(security.epoch)?;
        }
        Ok(())
    }

    /// Authenticate and authorize one trusted project before external lookup.
    ///
    /// Durable orchestration uses this anti-oracle preflight before consulting
    /// a `ReviewId`. It issues no approval and returns no session or authority.
    pub(crate) fn authorize_project(
        &self,
        credential: &A::Credential,
        project: &ProjectSelector,
    ) -> std::result::Result<(), ArtifactApprovalError> {
        let session = self.authenticate(credential)?;
        if !valid_control_value(project.as_str()) {
            return Err(ArtifactApprovalError::ConfigInvalid);
        }
        let state = lock(&self.state)?;
        state
            .projects
            .get(project.as_str())
            .filter(|security| security.allowed_actors.contains(session.actor_id()))
            .map(|_| ())
            .ok_or(ArtifactApprovalError::ProjectAccessDenied)
    }

    /// Authenticate first, then issue an approval bound to this pending review.
    pub fn issue_artifact_decision(
        &self,
        credential: &A::Credential,
        pending: &PendingArtifactProposal,
        options: &ArtifactDecisionOptions,
    ) -> std::result::Result<ArtifactDecisionApproval, ArtifactApprovalError> {
        let session = self.authenticate(credential)?;
        let binding = pending.durable_binding();
        let initial_security_epoch = {
            let state = lock(&self.state)?;
            state
                .projects
                .get(binding.project().as_str())
                .filter(|security| security.allowed_actors.contains(session.actor_id()))
                .map(|security| security.epoch)
                .ok_or(ArtifactApprovalError::ProjectAccessDenied)?
        };
        if pending.state() != PendingArtifactState::Ready {
            return Err(ArtifactApprovalError::ApprovalInvalid);
        }
        if options
            .private_rationale
            .as_deref()
            .is_some_and(|rationale| !valid_control_value(rationale))
        {
            return Err(ArtifactApprovalError::ApprovalInvalid);
        }
        let intent_sha256 = decision_intent_sha256(options);
        let issued_at_unix_nanos = self.now()?;
        let not_after_unix_nanos = issued_at_unix_nanos
            .checked_add(self.ttl_nanos)
            .ok_or(ArtifactApprovalError::ServiceUnavailable)?;
        if issued_at_unix_nanos >= not_after_unix_nanos {
            return Err(ArtifactApprovalError::ServiceUnavailable);
        }

        let mut state = lock(&self.state)?;
        let security_epoch = state
            .projects
            .get(binding.project().as_str())
            .filter(|security| {
                security.epoch == initial_security_epoch
                    && security.allowed_actors.contains(session.actor_id())
            })
            .map(|security| security.epoch)
            .ok_or(ArtifactApprovalError::ProjectAccessDenied)?;
        state
            .approvals
            .retain(|_, approval| issued_at_unix_nanos < approval.not_after_unix_nanos);
        let serial = state.next_serial;
        state.next_serial = next_epoch(serial)?;
        state.approvals.insert(
            serial,
            ApprovalRecord {
                actor_id: session.actor_id().to_owned(),
                session_id: session.session_id().to_owned(),
                binding,
                intent_sha256,
                security_epoch,
                issued_at_unix_nanos,
                not_after_unix_nanos,
            },
        );
        Ok(ArtifactDecisionApproval {
            registry_instance: self.instance,
            serial,
        })
    }

    /// Authenticate and burn one approval before the artifact layer touches CAS.
    pub(crate) fn claim_artifact_decision(
        &self,
        credential: &A::Credential,
        approval: &ArtifactDecisionApproval,
        pending: &PendingArtifactProposal,
        options: &ArtifactDecisionOptions,
    ) -> std::result::Result<(), ArtifactApprovalError> {
        let session = self.authenticate(credential)?;
        if approval.registry_instance != self.instance {
            return Err(ArtifactApprovalError::ApprovalInvalid);
        }
        let record = {
            let mut state = lock(&self.state)?;
            let project_access_allowed = state
                .approvals
                .get(&approval.serial)
                .ok_or(ArtifactApprovalError::ApprovalInvalid)
                .and_then(|record| {
                    state
                        .projects
                        .get(record.binding.project().as_str())
                        .filter(|security| security.allowed_actors.contains(session.actor_id()))
                        .map(|_| ())
                        .ok_or(ArtifactApprovalError::ProjectAccessDenied)
                });
            project_access_allowed?;
            state
                .approvals
                .remove(&approval.serial)
                .ok_or(ArtifactApprovalError::ApprovalInvalid)?
        };
        let now = self.now()?;
        let binding = pending.durable_binding();
        if options
            .private_rationale
            .as_deref()
            .is_some_and(|rationale| !valid_control_value(rationale))
        {
            return Err(ArtifactApprovalError::ApprovalInvalid);
        }
        let intent_sha256 = decision_intent_sha256(options);

        let state = lock(&self.state)?;
        let live_security = state.projects.get(record.binding.project().as_str());
        let valid = pending.state() == PendingArtifactState::Ready
            && record.actor_id == session.actor_id()
            && record.session_id == session.session_id()
            && record.binding == binding
            && record.intent_sha256 == intent_sha256
            && now >= record.issued_at_unix_nanos
            && now < record.not_after_unix_nanos
            && live_security.is_some_and(|security| {
                security.epoch == record.security_epoch
                    && security.allowed_actors.contains(session.actor_id())
            });
        if !valid {
            return Err(ArtifactApprovalError::ApprovalInvalid);
        }
        Ok(())
    }

    fn authenticate(
        &self,
        credential: &A::Credential,
    ) -> std::result::Result<AuthenticatedSession, ArtifactApprovalError> {
        catch_unwind(AssertUnwindSafe(|| {
            self.authenticator.authenticate(credential)
        }))
        .map_err(|_| ArtifactApprovalError::AuthenticationFailed)?
        .map_err(|AuthenticationFailure| ArtifactApprovalError::AuthenticationFailed)
    }

    fn now(&self) -> std::result::Result<i128, ArtifactApprovalError> {
        catch_unwind(AssertUnwindSafe(|| self.clock.now_unix_nanos()))
            .map_err(|_| ArtifactApprovalError::ServiceUnavailable)?
            .map_err(|_| ArtifactApprovalError::ServiceUnavailable)
    }
}

fn decision_intent_sha256(options: &ArtifactDecisionOptions) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"synapsegit.generic-artifact-decision-intent.v1\0");
    hasher.update(match options.disposition {
        ArtifactDisposition::AdoptedUnchanged => b"adopted_unchanged".as_slice(),
        ArtifactDisposition::Rejected => b"rejected".as_slice(),
        ArtifactDisposition::Deferred => b"deferred".as_slice(),
    });
    match options.private_rationale.as_deref() {
        None => hasher.update(b"\0rationale:none"),
        Some(rationale) => {
            hasher.update(b"\0rationale:some\0");
            hasher.update(
                u64::try_from(rationale.len())
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            hasher.update(rationale.as_bytes());
        }
    }
    hasher.finalize().into()
}

fn valid_control_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2_000 && !value.chars().any(char::is_control)
}

fn next_epoch(value: u64) -> std::result::Result<u64, ArtifactApprovalError> {
    value
        .checked_add(1)
        .filter(|next| *next != 0)
        .ok_or(ArtifactApprovalError::ServiceUnavailable)
}

fn lock<T>(mutex: &Mutex<T>) -> std::result::Result<MutexGuard<'_, T>, ArtifactApprovalError> {
    mutex
        .lock()
        .map_err(|_| ArtifactApprovalError::ServiceUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_intent_hash_binds_option_presence_and_rationale_length() {
        let options = |private_rationale| ArtifactDecisionOptions {
            disposition: ArtifactDisposition::Rejected,
            private_rationale,
        };

        assert_ne!(
            decision_intent_sha256(&options(None)),
            decision_intent_sha256(&options(Some(String::new())))
        );
        assert_ne!(
            decision_intent_sha256(&options(Some("a".into()))),
            decision_intent_sha256(&options(Some("a\0".into())))
        );
    }
}
