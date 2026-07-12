use super::*;
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Barrier, Mutex};
use synapse_core::DecisionDisposition;
use synapse_sqlite::{RefUpdate, ReflogMetadata};

const PRINCIPAL_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000001";
const AGENT_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000002";
const OTHER_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000003";
const PROJECT_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000010";
const ACTOR_ENTITY_ID: &str = AGENT_ID;
const POLICY_ENTITY_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000030";
const GRANT_ENTITY_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000040";
const CONTEXT_ENTITY_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000050";
const ACTIVITY_ENTITY_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000060";
const FEEDBACK_ENTITY_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000070";
const BASE_REF: &str = "decision/main";
const PROPOSAL_ROOT: &str = "proposal/20000000-0000-4000-8000-000000000002";
const PROPOSAL_REF: &str = "proposal/20000000-0000-4000-8000-000000000002/application-run-1";
const RECORDED_AT: &str = "1970-01-01T00:00:01.000000000Z";
const NOW: i128 = 3_000_000_000;
const TTL: i128 = 1_000_000_000;
const VALID_EXPIRES_AT: &str = "9999-12-31T23:59:59.999999999Z";

static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapsegit-application-test-{}-{id}",
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

#[derive(Clone)]
struct TestClock(Arc<Mutex<std::result::Result<i128, String>>>);

impl TestClock {
    fn new(now: i128) -> Self {
        Self(Arc::new(Mutex::new(Ok(now))))
    }

    fn set(&self, now: i128) {
        *self.0.lock().unwrap() = Ok(now);
    }

    fn fail(&self) {
        *self.0.lock().unwrap() = Err("test clock failure".to_owned());
    }
}

impl AuthorizationClock for TestClock {
    fn now_unix_nanos(&self) -> std::result::Result<i128, String> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Clone, Default)]
struct TestAuthenticator {
    calls: Arc<AtomicUsize>,
}

impl Authenticator for TestAuthenticator {
    type Credential = str;

    fn authenticate(
        &self,
        credential: &str,
    ) -> std::result::Result<AuthenticatedSession, AuthenticationFailure> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        match credential {
            "agent-session" => AuthenticatedSession::new(AGENT_ID, "session-agent"),
            "principal-session" => AuthenticatedSession::new(PRINCIPAL_ID, "session-principal"),
            "principal-other-session" => {
                AuthenticatedSession::new(PRINCIPAL_ID, "session-principal-other")
            }
            "other-session" => AuthenticatedSession::new(OTHER_ID, "session-other"),
            "panic" => panic!("authenticator panic fixture"),
            _ => Err(AuthenticationFailure),
        }
    }
}

#[derive(Clone)]
struct TestExecutor {
    inner: Arc<TestExecutorInner>,
}

struct TestExecutorInner {
    output: Mutex<ExecutedAiProposal>,
    calls: AtomicUsize,
    contexts: Mutex<Vec<AiExecutionContext>>,
    mode: Mutex<ExecutorMode>,
}

#[derive(Clone)]
enum ExecutorMode {
    Success,
    Failure,
    Panic,
    Block {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    },
}

impl TestExecutor {
    fn new(output: ExecutedAiProposal) -> Self {
        Self {
            inner: Arc::new(TestExecutorInner {
                output: Mutex::new(output),
                calls: AtomicUsize::new(0),
                contexts: Mutex::new(Vec::new()),
                mode: Mutex::new(ExecutorMode::Success),
            }),
        }
    }

    fn set_mode(&self, mode: ExecutorMode) {
        *self.inner.mode.lock().unwrap() = mode;
    }

    fn calls(&self) -> usize {
        self.inner.calls.load(AtomicOrdering::SeqCst)
    }

    fn set_output(&self, output: ExecutedAiProposal) {
        *self.inner.output.lock().unwrap() = output;
    }
}

impl AiExecutor for TestExecutor {
    fn execute(
        &self,
        context: &AiExecutionContext,
    ) -> std::result::Result<ExecutedAiProposal, ExecutionFailure> {
        self.inner.calls.fetch_add(1, AtomicOrdering::SeqCst);
        self.inner.contexts.lock().unwrap().push(context.clone());
        let mode = self.inner.mode.lock().unwrap().clone();
        match mode {
            ExecutorMode::Success => Ok(self.inner.output.lock().unwrap().clone()),
            ExecutorMode::Failure => Err(ExecutionFailure),
            ExecutorMode::Panic => panic!("executor panic fixture"),
            ExecutorMode::Block { entered, release } => {
                entered.wait();
                release.wait();
                Ok(self.inner.output.lock().unwrap().clone())
            }
        }
    }
}

type TestApplication = Application<TestAuthenticator, TestExecutor, TestClock>;

struct Fixture {
    _temporary: TempDirectory,
    application: Arc<TestApplication>,
    selector: ProjectSelector,
    profile: AuthorityProfileHandle,
    execution: RegisteredExecutionHandle,
    clock: TestClock,
    executor: TestExecutor,
    base_commit_oid: String,
    base_tree_oid: String,
    candidate_oid: String,
    activity_oid: String,
    actor_oid: String,
    principal_oid: String,
    context_oid: String,
    grant_oid: String,
    policy_oid: String,
    decision_feedback_oid: String,
    decision_oid: String,
}

#[derive(Debug)]
struct HumanFlow {
    profile: HumanAuthorityProfileHandle,
    registration: RegisteredHumanDecisionHandle,
}

impl Fixture {
    fn build(runtime_capabilities: Vec<AiCapability>) -> Self {
        let temporary = TempDirectory::new();
        let mut repository = Repository::open(temporary.join("repo")).unwrap();

        let actor_oid = put_json(
            &repository,
            json!({
                "object_type": "record",
                "schema_version": "0.1.0",
                "record_type": "actor",
                "entity_id": ACTOR_ENTITY_ID,
                "recorded_at": RECORDED_AT,
                "asserted_by": PRINCIPAL_ID,
                "origin": "self_declared",
                "source_refs": [],
                "payload": {
                    "actor_kind": "ai_agent",
                    "display_name": "Application Test Agent",
                    "ai_profile": {
                        "model_id": "fixture-model",
                        "model_version": "1",
                        "capabilities": ["propose_branch", "read_context"]
                    }
                },
                "extensions": {}
            }),
        );
        let principal_oid = put_json(
            &repository,
            json!({
                "object_type": "record",
                "schema_version": "0.1.0",
                "record_type": "actor",
                "entity_id": PRINCIPAL_ID,
                "recorded_at": RECORDED_AT,
                "asserted_by": PRINCIPAL_ID,
                "origin": "self_declared",
                "source_refs": [],
                "payload": {
                    "actor_kind": "human",
                    "display_name": "Application Test Principal"
                },
                "extensions": {}
            }),
        );
        let policy_oid = put_json(
            &repository,
            json!({
                "object_type": "record",
                "schema_version": "0.1.0",
                "record_type": "policy",
                "entity_id": POLICY_ENTITY_ID,
                "recorded_at": RECORDED_AT,
                "asserted_by": PRINCIPAL_ID,
                "origin": "self_declared",
                "source_refs": [],
                "payload": {
                    "scope_refs": [PROJECT_ID],
                    "rules": [
                        {
                            "rule_id": "allow-context-read",
                            "effect": "allow",
                            "action": "read",
                            "resource_selector": "project/**"
                        },
                        {
                            "rule_id": "allow-proposal",
                            "effect": "allow",
                            "action": "propose",
                            "resource_selector": format!("{PROPOSAL_ROOT}/**")
                        },
                        {
                            "rule_id": "gate-decision",
                            "effect": "require_human_gate",
                            "action": "publish",
                            "resource_selector": "decision/**",
                            "human_gate": "before_decision_ref"
                        }
                    ],
                    "default_effect": "deny"
                },
                "extensions": {}
            }),
        );
        let grant_oid = put_json(&repository, delegation_grant());
        let evidence_oid = repository
            .put_blob(&b"retained application evidence"[..])
            .unwrap()
            .oid;
        let subject_oid = put_json(&repository, subject_record());
        let mut base_entries = JsonMap::new();
        base_entries.insert(
            "actor.json".to_owned(),
            json!({ "entry_kind": "record", "oid": actor_oid }),
        );
        base_entries.insert(
            "principal.json".to_owned(),
            json!({ "entry_kind": "record", "oid": principal_oid }),
        );
        base_entries.insert(
            "policy.json".to_owned(),
            json!({ "entry_kind": "record", "oid": policy_oid }),
        );
        base_entries.insert(
            "grant.json".to_owned(),
            json!({ "entry_kind": "record", "oid": grant_oid }),
        );
        base_entries.insert(
            "evidence.bin".to_owned(),
            json!({ "entry_kind": "blob", "oid": evidence_oid }),
        );
        base_entries.insert(
            "subject.json".to_owned(),
            json!({ "entry_kind": "record", "oid": subject_oid }),
        );
        let retained_entries = base_entries.clone();
        let base_tree_oid = put_json(&repository, manifest_tree(base_entries));
        let base_commit_oid = put_json(
            &repository,
            commit(
                "checkpoint",
                &[],
                &base_tree_oid,
                &[],
                PRINCIPAL_ID,
                "application base",
            ),
        );
        repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: None,
                new_head: &base_commit_oid,
                metadata: ReflogMetadata::at(1),
            })
            .unwrap();

        let context_oid = put_json(
            &repository,
            context_pack_record(&base_commit_oid, &policy_oid, &grant_oid),
        );
        let output_oid = repository
            .put_blob(&b"application generated proposal"[..])
            .unwrap()
            .oid;
        let activity_oid = put_json(
            &repository,
            activity_record(&context_oid, &grant_oid, &output_oid),
        );
        let mut candidate_entries = retained_entries;
        candidate_entries.insert(
            "context.json".to_owned(),
            json!({ "entry_kind": "record", "oid": context_oid }),
        );
        candidate_entries.insert(
            "run.json".to_owned(),
            json!({ "entry_kind": "record", "oid": activity_oid }),
        );
        candidate_entries.insert(
            "proposal.bin".to_owned(),
            json!({ "entry_kind": "blob", "oid": output_oid }),
        );
        let candidate_tree_oid = put_json(&repository, manifest_tree(candidate_entries));
        let candidate_oid = put_json(
            &repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&base_commit_oid),
                &candidate_tree_oid,
                std::slice::from_ref(&activity_oid),
                AGENT_ID,
                "application proposal",
            ),
        );
        let decision_feedback_oid = put_json(
            &repository,
            decision_feedback_record(&candidate_oid, "adopted_unchanged"),
        );
        let decision_oid = put_json(
            &repository,
            commit(
                "decision",
                std::slice::from_ref(&base_commit_oid),
                &candidate_tree_oid,
                std::slice::from_ref(&decision_feedback_oid),
                PRINCIPAL_ID,
                "application human decision",
            ),
        );

        let executor = TestExecutor::new(ExecutedAiProposal::new(
            candidate_oid.clone(),
            activity_oid.clone(),
            Some("application publish"),
        ));
        let clock = TestClock::new(NOW);
        let selector = ProjectSelector::new(PROJECT_ID);
        let application = Arc::new(
            Application::new(
                TestAuthenticator::default(),
                executor.clone(),
                clock.clone(),
                TTL,
                [RegisteredProject::new(selector.clone(), repository)],
            )
            .unwrap(),
        );
        application
            .grant_project_access(&selector, AGENT_ID)
            .unwrap();
        application
            .grant_project_access(&selector, PRINCIPAL_ID)
            .unwrap();
        let profile = application
            .register_authority_profile(AiAuthorityProfileConfig::new(
                selector.clone(),
                AGENT_ID,
                PRINCIPAL_ID,
                BASE_REF,
                actor_oid.clone(),
                principal_oid.clone(),
                context_oid.clone(),
                PROPOSAL_REF,
                vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
                runtime_capabilities,
                AiSideEffectClass::None,
            ))
            .unwrap();
        let execution = application.register_execution(&profile).unwrap();
        Self {
            _temporary: temporary,
            application,
            selector,
            profile,
            execution,
            clock,
            executor,
            base_commit_oid,
            base_tree_oid,
            candidate_oid,
            activity_oid,
            actor_oid,
            principal_oid,
            context_oid,
            grant_oid,
            policy_oid,
            decision_feedback_oid,
            decision_oid,
        }
    }

    fn valid() -> Self {
        Self::build(vec![AiCapability::ReadContext, AiCapability::ProposeBranch])
    }

    fn prepare(&self) -> Result<AiExecutionPermit> {
        self.application
            .prepare_ai("agent-session", &self.selector, &self.execution)
    }

    fn target_head(&self) -> Option<String> {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        lock(&slot.repository)
            .unwrap()
            .refs()
            .get(PROPOSAL_REF)
            .unwrap()
            .map(|record| record.head)
    }

    fn occupy_target(&self) {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        let mut repository = lock(&slot.repository).unwrap();
        repository
            .update_ref(RefUpdate {
                ref_name: PROPOSAL_REF,
                expected_head: None,
                new_head: &self.base_commit_oid,
                metadata: ReflogMetadata::at(2),
            })
            .unwrap();
    }

    fn advance_base(&self) {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        let mut repository = lock(&slot.repository).unwrap();
        let next = put_json(
            &repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&self.base_commit_oid),
                &self.base_tree_oid,
                &[],
                PRINCIPAL_ID,
                "advanced application base",
            ),
        );
        repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: Some(&self.base_commit_oid),
                new_head: &next,
                metadata: ReflogMetadata::at(2),
            })
            .unwrap();
    }

    fn admit_proposal(&self) -> AdmittedProposalHandle {
        let permit = self.prepare().unwrap();
        let receipt = self
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap();
        let (decision, admitted) = receipt.into_parts();
        assert_eq!(decision.reflog.new_head, self.candidate_oid);
        admitted
    }

    fn human_candidate(&self) -> HumanDecisionCandidate {
        HumanDecisionCandidate::new(
            self.decision_oid.clone(),
            self.decision_feedback_oid.clone(),
            Some("authenticated application human decision"),
        )
    }

    fn register_human_with(
        &self,
        admitted: AdmittedProposalHandle,
        human_actor_oid: impl Into<String>,
        policy_oid: impl Into<String>,
        candidate: HumanDecisionCandidate,
    ) -> Result<HumanFlow> {
        let profile = self
            .application
            .register_human_profile(self.human_profile_config(human_actor_oid, policy_oid))?;
        let registration = self
            .application
            .register_human_decision(&profile, &admitted, candidate)?;
        Ok(HumanFlow {
            profile,
            registration,
        })
    }

    fn human_profile_config(
        &self,
        human_actor_oid: impl Into<String>,
        policy_oid: impl Into<String>,
    ) -> HumanAuthorityProfileConfig {
        HumanAuthorityProfileConfig::new(
            self.selector.clone(),
            PRINCIPAL_ID,
            BASE_REF,
            human_actor_oid,
            policy_oid,
        )
    }

    fn register_human(&self, admitted: AdmittedProposalHandle) -> Result<HumanFlow> {
        self.register_human_with(
            admitted,
            self.principal_oid.clone(),
            self.policy_oid.clone(),
            self.human_candidate(),
        )
    }

    fn prepare_human(&self, flow: &HumanFlow) -> Result<HumanDecisionPermit> {
        self.application.prepare_human_decision(
            "principal-session",
            &self.selector,
            &flow.registration,
        )
    }

    fn decision_head(&self) -> String {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        lock(&slot.repository)
            .unwrap()
            .refs()
            .get(BASE_REF)
            .unwrap()
            .unwrap()
            .head
    }

    fn advance_admitted_proposal_ref(&self) {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        let mut repository = lock(&slot.repository).unwrap();
        repository
            .update_ref(RefUpdate {
                ref_name: PROPOSAL_REF,
                expected_head: Some(&self.candidate_oid),
                new_head: &self.base_commit_oid,
                metadata: ReflogMetadata::at(5),
            })
            .unwrap();
    }

    fn advance_decision_ref(&self) {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        let mut repository = lock(&slot.repository).unwrap();
        let advanced = put_json(
            &repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&self.base_commit_oid),
                &self.base_tree_oid,
                &[],
                PRINCIPAL_ID,
                "concurrent human decision advance",
            ),
        );
        repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: Some(&self.base_commit_oid),
                new_head: &advanced,
                metadata: ReflogMetadata::at(5),
            })
            .unwrap();
    }

    fn ref_snapshot_and_reflog(
        &self,
    ) -> (
        synapse_sqlite::RefSnapshot,
        Vec<synapse_sqlite::ReflogEntry>,
    ) {
        let slot = self.application.projects.get(PROJECT_ID).unwrap();
        let repository = lock(&slot.repository).unwrap();
        (
            repository.refs().snapshot().unwrap(),
            repository.refs().reflog().unwrap(),
        )
    }
}

fn delegation_grant() -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "delegation_grant",
        "entity_id": GRANT_ENTITY_ID,
        "recorded_at": RECORDED_AT,
        "asserted_by": PRINCIPAL_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "principal_ref": PRINCIPAL_ID,
            "delegate_ref": AGENT_ID,
            "project_ref": PROJECT_ID,
            "purpose": "Publish one application proposal.",
            "capabilities": ["propose_branch", "read_context"],
            "resource_selectors": ["project/**"],
            "writable_ref_prefixes": [PROPOSAL_ROOT],
            "data_classes": ["internal"],
            "allowed_egress": [],
            "may_delegate": false,
            "max_child_depth": 0,
            "max_output_bytes": 10000,
            "required_human_gates": ["before_decision_ref", "before_release_ref"],
            "expires_at": VALID_EXPIRES_AT
        },
        "extensions": {}
    })
}

fn subject_record() -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "subject",
        "entity_id": "urn:uuid:20000000-0000-4000-8000-000000000080",
        "recorded_at": RECORDED_AT,
        "asserted_by": PRINCIPAL_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "subject_kind": "hybrid",
            "label": "Application retained subject",
            "relation_refs": [],
            "spatial_frame_refs": []
        },
        "extensions": {}
    })
}

fn context_pack_record(base: &str, policy: &str, grant: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "context_pack",
        "entity_id": CONTEXT_ENTITY_ID,
        "recorded_at": RECORDED_AT,
        "asserted_by": PRINCIPAL_ID,
        "origin": "tool_recorded",
        "source_refs": [],
        "payload": {
            "base_commit": base,
            "base_ref_name": BASE_REF,
            "expected_ref_head": base,
            "selected_context_refs": [base],
            "must_preserve_constraints": [],
            "allowed_transformations": [],
            "unresolved_questions": [],
            "policy_snapshot_ref": policy,
            "delegation_grant_ref": grant,
            "data_classification": "internal"
        },
        "extensions": {}
    })
}

fn activity_record(context: &str, grant: &str, output: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "activity",
        "entity_id": ACTIVITY_ENTITY_ID,
        "valid_time": { "kind": "instant", "at": RECORDED_AT },
        "recorded_at": RECORDED_AT,
        "asserted_by": AGENT_ID,
        "origin": "tool_recorded",
        "source_refs": [],
        "payload": {
            "activity_kind": "ai_run",
            "actor_refs": [
                { "role": "responsible_principal", "actor_ref": PRINCIPAL_ID },
                { "role": "agent", "actor_ref": AGENT_ID }
            ],
            "subject_refs": [],
            "input_refs": [{ "role": "context", "oid": context }],
            "output_refs": [{ "role": "proposal", "oid": output }],
            "reversibility": "reversible",
            "side_effect_class": "none",
            "ai_run": {
                "agent_ref": AGENT_ID,
                "responsible_principal_ref": PRINCIPAL_ID,
                "context_pack_ref": context,
                "delegation_grant_ref": grant,
                "requested_capabilities": ["propose_branch", "read_context"],
                "required_human_gates": ["before_decision_ref", "before_release_ref"],
                "status": "proposal_ready"
            }
        },
        "extensions": {}
    })
}

fn decision_feedback_record(proposal: &str, disposition: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "decision_feedback",
        "entity_id": FEEDBACK_ENTITY_ID,
        "recorded_at": RECORDED_AT,
        "asserted_by": PRINCIPAL_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "proposal_ref": proposal,
            "disposition": disposition,
            "reason_codes": ["goal_fit"],
            "human_rationale": "Application fixture human decision.",
            "visibility": "project",
            "training_use_policy": "project_local_only"
        },
        "extensions": {}
    })
}

fn manifest_tree(entries: JsonMap<String, JsonValue>) -> JsonValue {
    json!({
        "object_type": "tree",
        "schema_version": "0.1.0",
        "entries": entries,
        "extensions": {}
    })
}

fn commit(
    kind: &str,
    parents: &[String],
    snapshot: &str,
    transitions: &[String],
    author: &str,
    message: &str,
) -> JsonValue {
    json!({
        "object_type": "commit",
        "schema_version": "0.1.0",
        "commit_kind": kind,
        "parents": parents,
        "snapshot": snapshot,
        "transition_refs": transitions,
        "bound_declaration_refs": [],
        "author_ref": author,
        "authored_at": RECORDED_AT,
        "message": message,
        "extensions": {}
    })
}

fn put_json(repository: &Repository, value: JsonValue) -> String {
    repository
        .put_object(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .oid
}

#[test]
fn authenticated_success_uses_only_server_registered_authority() {
    let fixture = Fixture::valid();
    let permit = fixture.prepare().unwrap();
    let decision = fixture
        .application
        .execute_and_publish_ai("agent-session", &permit)
        .unwrap();

    assert_eq!(
        fixture.target_head().as_deref(),
        Some(&*fixture.candidate_oid)
    );
    assert_eq!(decision.actor_record_oid, fixture.actor_oid);
    assert_eq!(decision.activity_oid, fixture.activity_oid);
    assert_eq!(decision.context_pack_oid, fixture.context_oid);
    assert_eq!(decision.delegation_grant_oid, fixture.grant_oid);
    assert_eq!(decision.policy_oid, fixture.policy_oid);
    assert_eq!(
        decision.effective_capabilities,
        vec![AiCapability::ReadContext, AiCapability::ProposeBranch]
    );
    assert_eq!(decision.reflog.actor.as_deref(), Some(AGENT_ID));
    assert_eq!(fixture.executor.calls(), 1);
    let contexts = fixture.executor.inner.contexts.lock().unwrap();
    assert_eq!(contexts[0].project_id(), PROJECT_ID);
    assert_eq!(contexts[0].target_ref_name(), PROPOSAL_REF);
    assert_eq!(format!("{permit:?}"), "AiExecutionPermit(<opaque>)");
}

#[test]
fn authentication_precedes_project_and_permit_lookup_and_does_not_burn() {
    let fixture = Fixture::valid();
    let malformed = ProjectSelector::new("\0unknown-project");
    let error = fixture
        .application
        .prepare_ai("invalid", &malformed, &fixture.execution)
        .unwrap_err();
    assert_eq!(error.code(), "authentication_required");

    let permit = fixture.prepare().unwrap();
    let error = fixture
        .application
        .execute_and_publish_ai("invalid", &permit)
        .unwrap_err();
    assert_eq!(error.code(), "authentication_required");
    assert_eq!(fixture.executor.calls(), 0);
    fixture
        .application
        .execute_and_publish_ai("agent-session", &permit)
        .unwrap();
}

#[test]
fn unknown_forbidden_and_foreign_project_details_share_one_public_error() {
    let fixture = Fixture::valid();
    let unknown = ProjectSelector::new("urn:uuid:ffffffff-ffff-4fff-8fff-ffffffffffff");
    let unknown_error = fixture
        .application
        .prepare_ai("agent-session", &unknown, &fixture.execution)
        .unwrap_err();
    let forbidden_error = fixture
        .application
        .prepare_ai("other-session", &fixture.selector, &fixture.execution)
        .unwrap_err();
    assert_eq!(unknown_error.code(), "project_access_denied");
    assert_eq!(forbidden_error.code(), "project_access_denied");
    assert_eq!(unknown_error.to_string(), forbidden_error.to_string());
    assert_eq!(fixture.executor.calls(), 0);
}

#[test]
fn permit_expiry_is_exclusive_and_burns_before_execution() {
    let fixture = Fixture::valid();
    let permit = fixture.prepare().unwrap();
    let not_after = fixture
        .application
        .security
        .lock()
        .unwrap()
        .permits
        .get(&permit.permit_serial)
        .unwrap()
        .not_after_unix_nanos;
    fixture.clock.set(not_after);
    let error = fixture
        .application
        .execute_and_publish_ai("agent-session", &permit)
        .unwrap_err();
    assert_eq!(error.code(), "execution_permit_invalid");
    assert_eq!(fixture.executor.calls(), 0);
    assert_eq!(fixture.target_head(), None);
    assert_eq!(
        fixture
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );
}

#[test]
fn executor_failure_and_panic_both_burn_the_permit() {
    for mode in [ExecutorMode::Failure, ExecutorMode::Panic] {
        let fixture = Fixture::valid();
        fixture.executor.set_mode(mode);
        let permit = fixture.prepare().unwrap();
        let error = fixture
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap_err();
        assert_eq!(error.code(), "execution_failed");
        assert_eq!(fixture.target_head(), None);
        assert_eq!(
            fixture
                .application
                .execute_and_publish_ai("agent-session", &permit)
                .unwrap_err()
                .code(),
            "execution_permit_invalid"
        );
    }
}

#[test]
fn faulty_executor_cannot_replace_the_registered_activity_binding() {
    let fixture = Fixture::valid();
    fixture.executor.set_output(ExecutedAiProposal::new(
        fixture.candidate_oid.clone(),
        fixture.actor_oid.clone(),
        Some("faulty activity substitution"),
    ));
    let permit = fixture.prepare().unwrap();
    let error = fixture
        .application
        .execute_and_publish_ai("agent-session", &permit)
        .unwrap_err();
    assert_eq!(error.code(), "authorization_denied");
    assert_eq!(fixture.target_head(), None);
    assert_eq!(fixture.executor.calls(), 1);
    assert_eq!(
        fixture
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );
}

#[test]
fn revocation_before_or_during_execution_fences_publication() {
    let before = Fixture::valid();
    let permit = before.prepare().unwrap();
    before
        .application
        .revoke_project_access(&before.selector, AGENT_ID)
        .unwrap();
    assert_eq!(
        before
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );
    assert_eq!(before.executor.calls(), 0);

    let during = Fixture::valid();
    let permit = during.prepare().unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    during.executor.set_mode(ExecutorMode::Block {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    });
    let application = Arc::clone(&during.application);
    let result = std::thread::scope(|scope| {
        let worker = scope.spawn(|| application.execute_and_publish_ai("agent-session", &permit));
        entered.wait();
        during
            .application
            .revoke_project_access(&during.selector, AGENT_ID)
            .unwrap();
        release.wait();
        worker.join().unwrap()
    });
    assert_eq!(result.unwrap_err().code(), "execution_permit_invalid");
    assert_eq!(during.target_head(), None);
}

#[test]
fn base_and_target_changes_after_preflight_are_atomic_core_failures() {
    let stale = Fixture::valid();
    let stale_permit = stale.prepare().unwrap();
    stale.advance_base();
    let error = stale
        .application
        .execute_and_publish_ai("agent-session", &stale_permit)
        .unwrap_err();
    assert_eq!(error.code(), "stale_base");
    assert_eq!(stale.target_head(), None);

    let occupied = Fixture::valid();
    let conflict_permit = occupied.prepare().unwrap();
    occupied.occupy_target();
    let error = occupied
        .application
        .execute_and_publish_ai("agent-session", &conflict_permit)
        .unwrap_err();
    assert_eq!(error.code(), "ref_conflict");
    assert_eq!(occupied.target_head(), Some(occupied.base_commit_oid));
}

#[test]
fn same_permit_parallel_replay_reaches_executor_at_most_once() {
    let fixture = Fixture::valid();
    let permit = fixture.prepare().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let first_application = Arc::clone(&fixture.application);
        let first_barrier = Arc::clone(&barrier);
        let permit_ref = &permit;
        let first = scope.spawn(move || {
            first_barrier.wait();
            first_application.execute_and_publish_ai("agent-session", permit_ref)
        });
        let second_application = Arc::clone(&fixture.application);
        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            second_barrier.wait();
            second_application.execute_and_publish_ai("agent-session", permit_ref)
        });
        vec![first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .next()
            .unwrap()
            .code(),
        "execution_permit_invalid"
    );
    assert_eq!(fixture.executor.calls(), 1);
}

#[test]
fn runtime_ceiling_misconfiguration_fails_before_permit_issue() {
    let fixture = Fixture::valid();
    let principal_actor_oid = {
        fixture
            .application
            .security
            .lock()
            .unwrap()
            .profiles
            .get(&fixture.profile.profile_serial)
            .unwrap()
            .config
            .principal_actor_record_oid
            .clone()
    };
    let error = fixture
        .application
        .replace_authority_profile(
            &fixture.profile,
            AiAuthorityProfileConfig::new(
                fixture.selector.clone(),
                AGENT_ID,
                PRINCIPAL_ID,
                BASE_REF,
                fixture.actor_oid.clone(),
                principal_actor_oid,
                fixture.context_oid.clone(),
                PROPOSAL_REF,
                vec![AiCapability::ReadContext, AiCapability::ProposeBranch],
                vec![AiCapability::ProposeBranch],
                AiSideEffectClass::None,
            ),
        )
        .unwrap_err();
    assert_eq!(error.code(), "configuration_invalid");
    assert_eq!(fixture.executor.calls(), 0);
    let state = fixture.application.security.lock().unwrap();
    assert!(state.permits.is_empty());
    assert!(
        state
            .registrations
            .contains_key(&fixture.execution.registration_serial)
    );
}

#[test]
fn clock_failure_and_backward_motion_burn_without_execution() {
    let failure = Fixture::valid();
    let permit = failure.prepare().unwrap();
    failure.clock.fail();
    assert_eq!(
        failure
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );
    assert_eq!(failure.executor.calls(), 0);

    let backward = Fixture::valid();
    let permit = backward.prepare().unwrap();
    backward.clock.set(NOW - 1);
    assert_eq!(
        backward
            .application
            .execute_and_publish_ai("agent-session", &permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );
    assert_eq!(backward.executor.calls(), 0);
}

#[test]
fn epoch_overflow_has_no_partial_acl_or_profile_mutation() {
    let fixture = Fixture::valid();
    {
        let mut state = fixture.application.security.lock().unwrap();
        state.projects.get_mut(PROJECT_ID).unwrap().epoch = u64::MAX;
    }
    let error = fixture
        .application
        .grant_project_access(&fixture.selector, OTHER_ID)
        .unwrap_err();
    assert_eq!(error.code(), "service_unavailable");
    {
        let mut state = fixture.application.security.lock().unwrap();
        let security = state.projects.get_mut(PROJECT_ID).unwrap();
        assert!(!security.allowed_actors.contains(OTHER_ID));
        security.epoch = 7;
        state
            .profiles
            .get_mut(&fixture.profile.profile_serial)
            .unwrap()
            .generation = u64::MAX;
    }
    let error = fixture
        .application
        .set_authority_profile_suspended(&fixture.profile, true)
        .unwrap_err();
    assert_eq!(error.code(), "service_unavailable");
    let state = fixture.application.security.lock().unwrap();
    let profile = state.profiles.get(&fixture.profile.profile_serial).unwrap();
    assert!(!profile.suspended);
    assert_eq!(profile.generation, u64::MAX);
    assert_eq!(state.projects.get(PROJECT_ID).unwrap().epoch, 7);
}

#[test]
fn human_success_uses_exact_admitted_proposal_and_server_authority() {
    let fixture = Fixture::valid();
    let admitted = fixture.admit_proposal();
    let handle_debug = format!("{admitted:?}");
    assert!(!handle_debug.contains(PROJECT_ID));
    assert!(!handle_debug.contains(&fixture.candidate_oid));
    let flow = fixture.register_human(admitted).unwrap();
    let permit = fixture.prepare_human(&flow).unwrap();
    let receipt = fixture
        .application
        .publish_human_decision("principal-session", &permit)
        .unwrap();

    assert_eq!(fixture.decision_head(), fixture.decision_oid);
    assert_eq!(receipt.human_actor_record_oid, fixture.principal_oid);
    assert_eq!(receipt.policy_record_oid, fixture.policy_oid);
    assert_eq!(receipt.proposal_commit_oid, fixture.candidate_oid);
    assert_eq!(receipt.decision_feedback_oid, fixture.decision_feedback_oid);
    assert_eq!(receipt.disposition, DecisionDisposition::AdoptedUnchanged);
    assert_eq!(receipt.reflog.actor.as_deref(), Some(PRINCIPAL_ID));
    assert_eq!(format!("{permit:?}"), "HumanDecisionPermit(<opaque>)");
    assert_eq!(
        fixture
            .application
            .publish_human_decision("principal-session", &permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );
}

#[test]
fn admitted_proposal_proof_is_bound_to_the_issuing_application() {
    let issuing = Fixture::valid();
    let admitted = issuing.admit_proposal();
    let foreign = Fixture::valid();
    let before = foreign.ref_snapshot_and_reflog();
    let error = foreign
        .register_human_with(
            admitted,
            foreign.principal_oid.clone(),
            foreign.policy_oid.clone(),
            foreign.human_candidate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), "configuration_invalid");
    assert_eq!(foreign.ref_snapshot_and_reflog(), before);

    let same_application = Fixture::valid();
    let mut wrong_project = same_application.admit_proposal();
    wrong_project.project = "urn:uuid:eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".to_owned();
    let error = same_application
        .register_human_with(
            wrong_project,
            same_application.principal_oid.clone(),
            same_application.policy_oid.clone(),
            same_application.human_candidate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), "configuration_invalid");

    let stale_proof = Fixture::valid();
    let mut admitted = stale_proof.admit_proposal();
    admitted
        .proposal_head
        .clone_from(&stale_proof.base_commit_oid);
    let before = stale_proof.ref_snapshot_and_reflog();
    let error = stale_proof
        .register_human_with(
            admitted,
            stale_proof.principal_oid.clone(),
            stale_proof.policy_oid.clone(),
            stale_proof.human_candidate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), "configuration_invalid");
    assert_eq!(stale_proof.ref_snapshot_and_reflog(), before);
}

#[test]
fn human_authentication_precedes_lookup_and_pre_auth_does_not_burn() {
    let fixture = Fixture::valid();
    let flow = fixture.register_human(fixture.admit_proposal()).unwrap();
    let malformed = ProjectSelector::new("\0unknown-human-project");
    let error = fixture
        .application
        .prepare_human_decision("invalid", &malformed, &flow.registration)
        .unwrap_err();
    assert_eq!(error.code(), "authentication_required");

    let permit = fixture.prepare_human(&flow).unwrap();
    let error = fixture
        .application
        .publish_human_decision("invalid", &permit)
        .unwrap_err();
    assert_eq!(error.code(), "authentication_required");
    fixture
        .application
        .publish_human_decision("principal-session", &permit)
        .unwrap();
}

#[test]
fn human_prepare_hides_unknown_forbidden_foreign_and_stale_registration_details() {
    let fixture = Fixture::valid();
    let flow = fixture.register_human(fixture.admit_proposal()).unwrap();
    let unknown = ProjectSelector::new("urn:uuid:ffffffff-ffff-4fff-8fff-ffffffffffff");
    let unknown_error = fixture
        .application
        .prepare_human_decision("principal-session", &unknown, &flow.registration)
        .unwrap_err();
    let forbidden_error = fixture
        .application
        .prepare_human_decision("other-session", &fixture.selector, &flow.registration)
        .unwrap_err();

    let foreign = Fixture::valid();
    let foreign_flow = foreign.register_human(foreign.admit_proposal()).unwrap();
    let foreign_error = fixture
        .application
        .prepare_human_decision(
            "principal-session",
            &fixture.selector,
            &foreign_flow.registration,
        )
        .unwrap_err();
    fixture
        .application
        .set_human_profile_suspended(&flow.profile, true)
        .unwrap();
    let stale_error = fixture
        .application
        .prepare_human_decision("principal-session", &fixture.selector, &flow.registration)
        .unwrap_err();

    for error in [
        &unknown_error,
        &forbidden_error,
        &foreign_error,
        &stale_error,
    ] {
        assert_eq!(error.code(), "project_access_denied");
        assert_eq!(error.to_string(), "project access denied");
    }
}

#[test]
fn human_registration_concurrent_prepare_issues_at_most_one_permit() {
    let fixture = Fixture::valid();
    let flow = fixture.register_human(fixture.admit_proposal()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let first_application = Arc::clone(&fixture.application);
        let first_barrier = Arc::clone(&barrier);
        let registration = &flow.registration;
        let selector = &fixture.selector;
        let first = scope.spawn(move || {
            first_barrier.wait();
            first_application.prepare_human_decision("principal-session", selector, registration)
        });
        let second_application = Arc::clone(&fixture.application);
        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            second_barrier.wait();
            second_application.prepare_human_decision("principal-session", selector, registration)
        });
        vec![first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .next()
            .unwrap()
            .code(),
        "project_access_denied"
    );
    let permit = results.into_iter().find_map(|result| result.ok()).unwrap();
    fixture
        .application
        .publish_human_decision("principal-session", &permit)
        .unwrap();
}

#[test]
fn human_permit_subject_ttl_clock_and_replay_fail_closed() {
    for wrong_credential in ["other-session", "principal-other-session"] {
        let subject = Fixture::valid();
        let flow = subject.register_human(subject.admit_proposal()).unwrap();
        let permit = subject.prepare_human(&flow).unwrap();
        assert_eq!(
            subject
                .application
                .publish_human_decision(wrong_credential, &permit)
                .unwrap_err()
                .code(),
            "execution_permit_invalid"
        );
        assert_eq!(
            subject
                .application
                .publish_human_decision("principal-session", &permit)
                .unwrap_err()
                .code(),
            "execution_permit_invalid"
        );
    }

    for clock_state in ["expired", "backward", "failure"] {
        let fixture = Fixture::valid();
        let flow = fixture.register_human(fixture.admit_proposal()).unwrap();
        let permit = fixture.prepare_human(&flow).unwrap();
        match clock_state {
            "expired" => fixture.clock.set(NOW + TTL),
            "backward" => fixture.clock.set(NOW - 1),
            "failure" => fixture.clock.fail(),
            _ => unreachable!(),
        }
        let before = fixture.ref_snapshot_and_reflog();
        let error = fixture
            .application
            .publish_human_decision("principal-session", &permit)
            .unwrap_err();
        assert_eq!(error.code(), "execution_permit_invalid", "{clock_state}");
        assert_eq!(fixture.ref_snapshot_and_reflog(), before);
        assert_eq!(
            fixture
                .application
                .publish_human_decision("principal-session", &permit)
                .unwrap_err()
                .code(),
            "execution_permit_invalid"
        );
    }
}

#[test]
fn human_candidate_actor_policy_and_feedback_denials_burn_without_ref_mutation() {
    for case in ["candidate", "actor", "policy", "feedback"] {
        let fixture = Fixture::valid();
        let admitted = fixture.admit_proposal();
        let candidate = match case {
            "candidate" => HumanDecisionCandidate::new(
                fixture.base_commit_oid.clone(),
                fixture.decision_feedback_oid.clone(),
                Some("not a decision Commit"),
            ),
            "feedback" => HumanDecisionCandidate::new(
                fixture.decision_oid.clone(),
                fixture.actor_oid.clone(),
                Some("not DecisionFeedback"),
            ),
            _ => fixture.human_candidate(),
        };
        let actor = if case == "actor" {
            fixture.actor_oid.clone()
        } else {
            fixture.principal_oid.clone()
        };
        let policy = if case == "policy" {
            fixture.grant_oid.clone()
        } else {
            fixture.policy_oid.clone()
        };
        let flow = fixture
            .register_human_with(admitted, actor, policy, candidate)
            .unwrap();
        let permit = fixture.prepare_human(&flow).unwrap();
        let before = fixture.ref_snapshot_and_reflog();
        let error = fixture
            .application
            .publish_human_decision("principal-session", &permit)
            .unwrap_err();
        assert_eq!(error.code(), "authorization_denied", "{case}: {error}");
        assert_eq!(fixture.ref_snapshot_and_reflog(), before, "{case}");
        assert_eq!(
            fixture
                .application
                .publish_human_decision("principal-session", &permit)
                .unwrap_err()
                .code(),
            "execution_permit_invalid"
        );
    }
}

#[test]
fn moved_proposal_and_decision_refs_fail_human_publication_atomically() {
    let moved_proposal = Fixture::valid();
    let flow = moved_proposal
        .register_human(moved_proposal.admit_proposal())
        .unwrap();
    let permit = moved_proposal.prepare_human(&flow).unwrap();
    moved_proposal.advance_admitted_proposal_ref();
    let before = moved_proposal.ref_snapshot_and_reflog();
    let error = moved_proposal
        .application
        .publish_human_decision("principal-session", &permit)
        .unwrap_err();
    assert_eq!(error.code(), "ref_conflict");
    assert_eq!(moved_proposal.ref_snapshot_and_reflog(), before);

    let moved_decision = Fixture::valid();
    let flow = moved_decision
        .register_human(moved_decision.admit_proposal())
        .unwrap();
    let permit = moved_decision.prepare_human(&flow).unwrap();
    moved_decision.advance_decision_ref();
    let before = moved_decision.ref_snapshot_and_reflog();
    let error = moved_decision
        .application
        .publish_human_decision("principal-session", &permit)
        .unwrap_err();
    assert_eq!(error.code(), "ref_conflict");
    assert_eq!(moved_decision.ref_snapshot_and_reflog(), before);
}

#[test]
fn revoke_suspend_and_replace_fence_ready_human_permits() {
    for operation in ["revoke", "suspend", "replace"] {
        let fixture = Fixture::valid();
        let flow = fixture.register_human(fixture.admit_proposal()).unwrap();
        let permit = fixture.prepare_human(&flow).unwrap();
        match operation {
            "revoke" => fixture
                .application
                .revoke_project_access(&fixture.selector, PRINCIPAL_ID)
                .unwrap(),
            "suspend" => fixture
                .application
                .set_human_profile_suspended(&flow.profile, true)
                .unwrap(),
            "replace" => fixture
                .application
                .replace_human_profile(
                    &flow.profile,
                    fixture.human_profile_config(
                        fixture.principal_oid.clone(),
                        fixture.policy_oid.clone(),
                    ),
                )
                .unwrap(),
            _ => unreachable!(),
        }
        let before = fixture.ref_snapshot_and_reflog();
        let error = fixture
            .application
            .publish_human_decision("principal-session", &permit)
            .unwrap_err();
        assert_eq!(error.code(), "execution_permit_invalid", "{operation}");
        assert_eq!(fixture.ref_snapshot_and_reflog(), before, "{operation}");
    }
}

#[test]
fn human_profile_epoch_overflow_has_no_partial_suspension() {
    let fixture = Fixture::valid();
    let flow = fixture.register_human(fixture.admit_proposal()).unwrap();
    let original_epoch = {
        let mut state = fixture.application.security.lock().unwrap();
        let security = state.projects.get_mut(PROJECT_ID).unwrap();
        let original = security.epoch;
        security.epoch = u64::MAX;
        original
    };
    let error = fixture
        .application
        .set_human_profile_suspended(&flow.profile, true)
        .unwrap_err();
    assert_eq!(error.code(), "service_unavailable");
    fixture
        .application
        .security
        .lock()
        .unwrap()
        .projects
        .get_mut(PROJECT_ID)
        .unwrap()
        .epoch = original_epoch;
    let permit = fixture.prepare_human(&flow).unwrap();
    fixture
        .application
        .publish_human_decision("principal-session", &permit)
        .unwrap();

    let generation = Fixture::valid();
    let flow = generation
        .register_human(generation.admit_proposal())
        .unwrap();
    let project_epoch = {
        let mut state = generation.application.security.lock().unwrap();
        state
            .human_profiles
            .get_mut(&flow.profile.profile_serial)
            .unwrap()
            .generation = u64::MAX;
        state.projects.get(PROJECT_ID).unwrap().epoch
    };
    let error = generation
        .application
        .set_human_profile_suspended(&flow.profile, true)
        .unwrap_err();
    assert_eq!(error.code(), "service_unavailable");
    let state = generation.application.security.lock().unwrap();
    let profile = state
        .human_profiles
        .get(&flow.profile.profile_serial)
        .unwrap();
    assert!(!profile.suspended);
    assert_eq!(profile.generation, u64::MAX);
    assert_eq!(state.projects.get(PROJECT_ID).unwrap().epoch, project_epoch);
}

#[test]
fn admitted_proposal_proof_can_register_a_corrected_candidate_after_core_denial() {
    let fixture = Fixture::valid();
    let admitted = fixture.admit_proposal();
    let profile = fixture
        .application
        .register_human_profile(
            fixture.human_profile_config(fixture.principal_oid.clone(), fixture.policy_oid.clone()),
        )
        .unwrap();
    let denied_registration = fixture
        .application
        .register_human_decision(
            &profile,
            &admitted,
            HumanDecisionCandidate::new(
                fixture.base_commit_oid.clone(),
                fixture.decision_feedback_oid.clone(),
                Some("invalid first candidate"),
            ),
        )
        .unwrap();
    let denied_permit = fixture
        .application
        .prepare_human_decision("principal-session", &fixture.selector, &denied_registration)
        .unwrap();
    let before = fixture.ref_snapshot_and_reflog();
    assert_eq!(
        fixture
            .application
            .publish_human_decision("principal-session", &denied_permit)
            .unwrap_err()
            .code(),
        "authorization_denied"
    );
    assert_eq!(fixture.ref_snapshot_and_reflog(), before);
    assert_eq!(
        fixture
            .application
            .publish_human_decision("principal-session", &denied_permit)
            .unwrap_err()
            .code(),
        "execution_permit_invalid"
    );

    let corrected_registration = fixture
        .application
        .register_human_decision(&profile, &admitted, fixture.human_candidate())
        .unwrap();
    let corrected_permit = fixture
        .application
        .prepare_human_decision(
            "principal-session",
            &fixture.selector,
            &corrected_registration,
        )
        .unwrap();
    fixture
        .application
        .publish_human_decision("principal-session", &corrected_permit)
        .unwrap();
    assert_eq!(fixture.decision_head(), fixture.decision_oid);
}
