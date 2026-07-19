#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use synapse_application::{AuthenticatedSession, AuthenticationFailure, Authenticator};
use synapse_artifact::{
    ArtifactApprovalRegistry, ArtifactDecisionOptions, ArtifactDecisionReceipt,
    PendingArtifactProposal, WorkflowError, decide_artifact_proposal,
};
use synapse_core::AuthorizationClock;

pub const HOST_CREDENTIAL: &str = "valid-host-reviewer-credential";
pub const HOST_ACTOR: &str = "artifact-host-reviewer";
const HOST_SESSION: &str = "artifact-host-review-session";
const APPROVAL_TTL_NANOS: i128 = 60_000_000_000;

#[derive(Clone, Default)]
pub struct TestHostAuthenticator {
    calls: Arc<AtomicUsize>,
}

impl TestHostAuthenticator {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Authenticator for TestHostAuthenticator {
    type Credential = str;

    fn authenticate(
        &self,
        credential: &Self::Credential,
    ) -> std::result::Result<AuthenticatedSession, AuthenticationFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if credential == HOST_CREDENTIAL {
            AuthenticatedSession::new(HOST_ACTOR, HOST_SESSION)
        } else {
            Err(AuthenticationFailure)
        }
    }
}

#[derive(Clone)]
pub struct TestClock(Arc<Mutex<std::result::Result<i128, String>>>);

impl TestClock {
    pub fn new(now: i128) -> Self {
        Self(Arc::new(Mutex::new(Ok(now))))
    }

    pub fn set(&self, now: i128) {
        *self.0.lock().expect("test clock lock") = Ok(now);
    }

    pub fn fail(&self) {
        *self.0.lock().expect("test clock lock") = Err("test clock failure".into());
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new(1_900_000_000_000_000_000)
    }
}

impl AuthorizationClock for TestClock {
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        self.0
            .lock()
            .map_err(|_| String::from("test clock poisoned"))?
            .clone()
    }
}

pub type TestApprovalRegistry = ArtifactApprovalRegistry<TestHostAuthenticator, TestClock>;

pub fn approval_registry(
    pending: &PendingArtifactProposal,
) -> (TestApprovalRegistry, TestHostAuthenticator, TestClock) {
    let authenticator = TestHostAuthenticator::default();
    let clock = TestClock::default();
    let registry =
        ArtifactApprovalRegistry::new(authenticator.clone(), clock.clone(), APPROVAL_TTL_NANOS)
            .expect("valid test approval registry");
    registry
        .grant_project_access(pending.durable_binding().project(), HOST_ACTOR)
        .expect("grant test reviewer access");
    (registry, authenticator, clock)
}

pub fn approved_decide(
    pending: &mut PendingArtifactProposal,
    options: &ArtifactDecisionOptions,
) -> Result<ArtifactDecisionReceipt, WorkflowError> {
    let (registry, _, _) = approval_registry(pending);
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending, options)
        .map_err(WorkflowError::from)?;
    decide_artifact_proposal(&registry, HOST_CREDENTIAL, &approval, pending, options)
}
