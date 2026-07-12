use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use synapse_core::{
    AiCapability, AiExecutionAuthority, AiProposalUpdate, AuthorizationClock, CreativeAiRuntime,
    DecisionDisposition, HumanDecisionAuthority, HumanDecisionReceipt, HumanDecisionRuntime,
    HumanDecisionUpdate, Repository, RepositoryError,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

const HUMAN_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000001";
const AGENT_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000002";
const OTHER_HUMAN_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000003";
const PROJECT_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000010";
const OTHER_PROJECT_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000011";
const HUMAN_ACTOR_ENTITY_ID: &str = HUMAN_ID;
const AGENT_ACTOR_ENTITY_ID: &str = AGENT_ID;
const POLICY_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000030";
const OTHER_POLICY_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000031";
const GRANT_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000040";
const OTHER_GRANT_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000041";
const CONTEXT_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000050";
const ACTIVITY_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000060";
const FEEDBACK_ENTITY_ID: &str = "urn:uuid:10000000-0000-4000-8000-000000000070";
const DECISION_REF: &str = "decision/main";
const PROPOSAL_REF: &str = "proposal/10000000-0000-4000-8000-000000000002/run-1";
const SECOND_PROPOSAL_REF: &str = "proposal/10000000-0000-4000-8000-000000000002/run-2";
const RECORDED_AT: &str = "1970-01-01T00:00:01.000000000Z";
const FIXED_NOW_NANOS: i128 = 3_000_000_000;
const MESSAGE: &str = "authenticated human decision";

static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct TestClock(Arc<Mutex<TestClockState>>);

enum TestClockState {
    Fixed(std::result::Result<i128, String>),
    Sequence(VecDeque<std::result::Result<i128, String>>),
}

impl TestClock {
    fn fixed(value: std::result::Result<i128, String>) -> Self {
        Self(Arc::new(Mutex::new(TestClockState::Fixed(value))))
    }

    fn sequence(values: impl IntoIterator<Item = std::result::Result<i128, String>>) -> Self {
        Self(Arc::new(Mutex::new(TestClockState::Sequence(
            values.into_iter().collect(),
        ))))
    }
}

impl AuthorizationClock for TestClock {
    fn now_unix_nanos(&self) -> Result<i128, String> {
        match &mut *self.0.lock().unwrap() {
            TestClockState::Fixed(value) => value.clone(),
            TestClockState::Sequence(values) => values
                .pop_front()
                .unwrap_or_else(|| Err("test clock sequence exhausted".to_owned())),
        }
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapsegit-human-decision-test-{}-{id}",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyMode {
    DecisionGate,
    AiProposalAndDecisionGate,
    ExplicitAllow,
    DenyOverGate,
    OtherGate,
    DefaultAllow,
    DefaultDeny,
    ConditionalAllow,
    UnsupportedSelector,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HumanActorMode {
    HumanSelfAsserted,
    AiSelfAsserted,
    HumanAssertedByOther,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BaseOmission {
    HumanActor,
    Policy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProposalChainMode {
    Valid,
    ActivityNotAiRun,
    ActivityNotReady,
    WrongResponsiblePrincipal,
    MissingDecisionGate,
    ContextBaseMismatch,
    ContextGrantMismatch,
    ContextPolicyMismatch,
    GrantProjectMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProposalMode {
    Valid,
    WrongKind,
    WrongParent,
    WrongAuthor,
    DropsBaseBlob,
    AddsProtectedControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityBindingMode {
    Valid,
    MissingAgentRole,
    MissingPrincipalRole,
    WrongContextInput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    DeclaredBlob,
    UndeclaredBlob,
    HumanAssertedClaim,
    DisallowedRecord,
}

struct ScenarioBuilder {
    disposition: &'static str,
    policy_mode: PolicyMode,
    human_actor_mode: HumanActorMode,
    base_omission: Option<BaseOmission>,
    policy_scope_project: &'static str,
    policy_asserted_by: &'static str,
    proposal_chain_mode: ProposalChainMode,
    proposal_mode: ProposalMode,
    activity_binding_mode: ActivityBindingMode,
    side_effect_class: &'static str,
    output_mode: OutputMode,
    max_output_bytes: i64,
    seed_proposal_ref: bool,
}

impl Default for ScenarioBuilder {
    fn default() -> Self {
        Self {
            disposition: "adopted_unchanged",
            policy_mode: PolicyMode::DecisionGate,
            human_actor_mode: HumanActorMode::HumanSelfAsserted,
            base_omission: None,
            policy_scope_project: PROJECT_ID,
            policy_asserted_by: HUMAN_ID,
            proposal_chain_mode: ProposalChainMode::Valid,
            proposal_mode: ProposalMode::Valid,
            activity_binding_mode: ActivityBindingMode::Valid,
            side_effect_class: "none",
            output_mode: OutputMode::DeclaredBlob,
            max_output_bytes: 10_000,
            seed_proposal_ref: true,
        }
    }
}

impl ScenarioBuilder {
    fn build(self) -> Scenario {
        let temporary = TempDirectory::new();
        let repository_path = temporary.join("repo");
        let mut repository = Repository::open(&repository_path).unwrap();

        let (human_kind, human_asserted_by) = match self.human_actor_mode {
            HumanActorMode::HumanSelfAsserted => ("human", HUMAN_ID),
            HumanActorMode::AiSelfAsserted => ("ai_agent", HUMAN_ID),
            HumanActorMode::HumanAssertedByOther => ("human", OTHER_HUMAN_ID),
        };
        let human_actor_oid = put_json(
            &repository,
            actor_record(
                HUMAN_ACTOR_ENTITY_ID,
                human_kind,
                human_asserted_by,
                "Authenticated Human",
            ),
        );
        let agent_actor_oid = put_json(
            &repository,
            actor_record(
                AGENT_ACTOR_ENTITY_ID,
                "ai_agent",
                HUMAN_ID,
                "Proposal Agent",
            ),
        );
        let policy_oid = put_json(
            &repository,
            policy_record(
                POLICY_ENTITY_ID,
                self.policy_scope_project,
                self.policy_asserted_by,
                self.policy_mode,
            ),
        );
        let other_policy_oid = put_json(
            &repository,
            policy_record(
                OTHER_POLICY_ENTITY_ID,
                PROJECT_ID,
                HUMAN_ID,
                PolicyMode::DecisionGate,
            ),
        );
        let grant_project = if self.proposal_chain_mode == ProposalChainMode::GrantProjectMismatch {
            OTHER_PROJECT_ID
        } else {
            PROJECT_ID
        };
        let grant_oid = put_json(
            &repository,
            delegation_grant(GRANT_ENTITY_ID, grant_project, self.max_output_bytes),
        );
        let other_grant_oid = put_json(
            &repository,
            delegation_grant(OTHER_GRANT_ENTITY_ID, PROJECT_ID, 10_000),
        );
        let base_blob_oid = repository
            .put_blob(&b"human decision base evidence"[..])
            .unwrap()
            .oid;
        let subject_oid = put_json(&repository, subject_record());

        let mut base_entries = JsonMap::new();
        if self.base_omission != Some(BaseOmission::HumanActor) {
            base_entries.insert(
                "human.json".to_owned(),
                json!({ "entry_kind": "record", "oid": human_actor_oid }),
            );
        }
        base_entries.insert(
            "agent.json".to_owned(),
            json!({ "entry_kind": "record", "oid": agent_actor_oid }),
        );
        if self.base_omission != Some(BaseOmission::Policy) {
            base_entries.insert(
                "policy.json".to_owned(),
                json!({ "entry_kind": "record", "oid": policy_oid }),
            );
        }
        base_entries.insert(
            "grant.json".to_owned(),
            json!({ "entry_kind": "record", "oid": grant_oid }),
        );
        base_entries.insert(
            "evidence.bin".to_owned(),
            json!({ "entry_kind": "blob", "oid": base_blob_oid }),
        );
        base_entries.insert(
            "subject.json".to_owned(),
            json!({ "entry_kind": "record", "oid": subject_oid }),
        );
        let retained_base_entries = base_entries.clone();
        let base_tree_oid = put_json(&repository, manifest_tree(base_entries));
        let base_commit_oid = put_json(
            &repository,
            commit(
                "checkpoint",
                &[],
                &base_tree_oid,
                &[],
                HUMAN_ID,
                "human decision base",
            ),
        );
        repository
            .update_ref(RefUpdate {
                ref_name: DECISION_REF,
                expected_head: None,
                new_head: &base_commit_oid,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: 1,
                    actor: Some(HUMAN_ID),
                    message: Some("seed canonical decision"),
                },
            })
            .unwrap();

        let unrelated_base_oid = put_json(
            &repository,
            commit(
                "checkpoint",
                &[],
                &base_tree_oid,
                &[],
                HUMAN_ID,
                "unrelated base",
            ),
        );
        let context_base = if self.proposal_chain_mode == ProposalChainMode::ContextBaseMismatch {
            unrelated_base_oid.as_str()
        } else {
            base_commit_oid.as_str()
        };
        let context_grant = if self.proposal_chain_mode == ProposalChainMode::ContextGrantMismatch {
            other_grant_oid.as_str()
        } else {
            grant_oid.as_str()
        };
        let context_policy = if self.proposal_chain_mode == ProposalChainMode::ContextPolicyMismatch
        {
            other_policy_oid.as_str()
        } else {
            policy_oid.as_str()
        };
        let context_oid = put_json(
            &repository,
            context_pack_record(context_base, context_policy, context_grant),
        );
        let (output_oid, output_entry_kind, undeclared_output_oid) = match self.output_mode {
            OutputMode::DeclaredBlob => (
                repository
                    .put_blob(&b"proposal reviewed by a human"[..])
                    .unwrap()
                    .oid,
                "blob",
                None,
            ),
            OutputMode::UndeclaredBlob => (
                repository
                    .put_blob(&b"declared proposal output"[..])
                    .unwrap()
                    .oid,
                "blob",
                Some(
                    repository
                        .put_blob(&b"undeclared proposal snapshot object"[..])
                        .unwrap()
                        .oid,
                ),
            ),
            OutputMode::HumanAssertedClaim => (
                put_json(&repository, claim_record(HUMAN_ID)),
                "record",
                None,
            ),
            OutputMode::DisallowedRecord => (
                put_json(
                    &repository,
                    decision_feedback_record(
                        AGENT_ID,
                        "self_declared",
                        &base_commit_oid,
                        "rejected",
                    ),
                ),
                "record",
                None,
            ),
        };
        let activity_kind = if self.proposal_chain_mode == ProposalChainMode::ActivityNotAiRun {
            "review"
        } else {
            "ai_run"
        };
        let activity_status = if self.proposal_chain_mode == ProposalChainMode::ActivityNotReady {
            "running"
        } else {
            "proposal_ready"
        };
        let responsible_principal =
            if self.proposal_chain_mode == ProposalChainMode::WrongResponsiblePrincipal {
                OTHER_HUMAN_ID
            } else {
                HUMAN_ID
            };
        let required_gates = if self.proposal_chain_mode == ProposalChainMode::MissingDecisionGate {
            vec!["before_release_ref"]
        } else {
            vec!["before_decision_ref"]
        };
        let activity_oid = put_json(
            &repository,
            activity_record(
                activity_kind,
                activity_status,
                responsible_principal,
                &context_oid,
                &grant_oid,
                &output_oid,
                &required_gates,
                self.activity_binding_mode,
                self.side_effect_class,
            ),
        );

        let mut proposal_entries = retained_base_entries;
        if let Some(undeclared) = undeclared_output_oid {
            proposal_entries.insert(
                "undeclared.bin".to_owned(),
                json!({ "entry_kind": "blob", "oid": undeclared }),
            );
        }
        if self.proposal_mode == ProposalMode::DropsBaseBlob {
            proposal_entries.remove("evidence.bin");
        }
        if self.proposal_mode == ProposalMode::AddsProtectedControl {
            let introduced_actor = put_json(
                &repository,
                actor_record(
                    OTHER_HUMAN_ID,
                    "human",
                    OTHER_HUMAN_ID,
                    "Introduced Control Actor",
                ),
            );
            proposal_entries.insert(
                "introduced-actor.json".to_owned(),
                json!({ "entry_kind": "record", "oid": introduced_actor }),
            );
        }
        proposal_entries.insert(
            "context.json".to_owned(),
            json!({ "entry_kind": "record", "oid": context_oid }),
        );
        proposal_entries.insert(
            "proposal.bin".to_owned(),
            json!({ "entry_kind": output_entry_kind, "oid": output_oid }),
        );
        proposal_entries.insert(
            "run.json".to_owned(),
            json!({ "entry_kind": "record", "oid": activity_oid }),
        );
        let proposal_tree_oid = put_json(&repository, manifest_tree(proposal_entries));
        let proposal_kind = if self.proposal_mode == ProposalMode::WrongKind {
            "decision"
        } else {
            "checkpoint"
        };
        let proposal_parent = if self.proposal_mode == ProposalMode::WrongParent {
            unrelated_base_oid.as_str()
        } else {
            base_commit_oid.as_str()
        };
        let proposal_author = if self.proposal_mode == ProposalMode::WrongAuthor {
            OTHER_HUMAN_ID
        } else {
            AGENT_ID
        };
        let proposal_commit_oid = put_json(
            &repository,
            commit(
                proposal_kind,
                &[proposal_parent.to_owned()],
                &proposal_tree_oid,
                std::slice::from_ref(&activity_oid),
                proposal_author,
                "AI proposal awaiting human review",
            ),
        );
        if self.seed_proposal_ref {
            repository
                .update_ref(RefUpdate {
                    ref_name: PROPOSAL_REF,
                    expected_head: None,
                    new_head: &proposal_commit_oid,
                    metadata: ReflogMetadata {
                        occurred_at_unix_nanos: 2,
                        actor: Some(AGENT_ID),
                        message: Some("seed reviewed proposal"),
                    },
                })
                .unwrap();
        }

        let decision_feedback_oid = put_json(
            &repository,
            decision_feedback_record(
                HUMAN_ID,
                "self_declared",
                &proposal_commit_oid,
                self.disposition,
            ),
        );
        let decision_snapshot_oid = if self.disposition == "adopted_unchanged" {
            proposal_tree_oid.clone()
        } else {
            base_tree_oid.clone()
        };
        let new_head = put_json(
            &repository,
            commit(
                "decision",
                std::slice::from_ref(&base_commit_oid),
                &decision_snapshot_oid,
                std::slice::from_ref(&decision_feedback_oid),
                HUMAN_ID,
                "human decision",
            ),
        );

        Scenario {
            _temporary: temporary,
            repository_path,
            repository,
            authenticated_human_id: HUMAN_ID.to_owned(),
            authorized_project_id: PROJECT_ID.to_owned(),
            decision_ref_name: DECISION_REF.to_owned(),
            decision_head: base_commit_oid,
            proposal_ref_name: PROPOSAL_REF.to_owned(),
            proposal_head: proposal_commit_oid,
            human_actor_record_oid: human_actor_oid,
            agent_actor_record_oid: agent_actor_oid,
            policy_record_oid: policy_oid,
            base_tree_oid,
            base_blob_oid,
            subject_oid,
            proposal_tree_oid,
            activity_oid,
            context_oid,
            grant_oid,
            output_oid,
            decision_feedback_oid,
            decision_snapshot_oid,
            new_head,
            clock: TestClock::fixed(Ok(FIXED_NOW_NANOS)),
        }
    }
}

struct Scenario {
    _temporary: TempDirectory,
    repository_path: PathBuf,
    repository: Repository,
    authenticated_human_id: String,
    authorized_project_id: String,
    decision_ref_name: String,
    decision_head: String,
    proposal_ref_name: String,
    proposal_head: String,
    human_actor_record_oid: String,
    agent_actor_record_oid: String,
    policy_record_oid: String,
    base_tree_oid: String,
    base_blob_oid: String,
    subject_oid: String,
    proposal_tree_oid: String,
    activity_oid: String,
    context_oid: String,
    grant_oid: String,
    output_oid: String,
    decision_feedback_oid: String,
    decision_snapshot_oid: String,
    new_head: String,
    clock: TestClock,
}

impl Scenario {
    fn publish(&mut self) -> Result<HumanDecisionReceipt, RepositoryError> {
        let authority = HumanDecisionAuthority::new(
            &self.authenticated_human_id,
            &self.authorized_project_id,
            &self.decision_ref_name,
            &self.decision_head,
            &self.proposal_ref_name,
            &self.proposal_head,
            &self.human_actor_record_oid,
            &self.policy_record_oid,
        );
        HumanDecisionRuntime::with_clock(&mut self.repository, authority, self.clock.clone())
            .publish_decision(HumanDecisionUpdate {
                new_head: &self.new_head,
                decision_feedback_oid: &self.decision_feedback_oid,
                message: Some(MESSAGE),
            })
    }

    fn rebuild_decision(
        &mut self,
        kind: &str,
        parents: &[String],
        snapshot: &str,
        author: &str,
        transitions: &[String],
    ) {
        self.decision_snapshot_oid = snapshot.to_owned();
        self.new_head = put_json(
            &self.repository,
            commit(
                kind,
                parents,
                snapshot,
                transitions,
                author,
                "mutated human decision",
            ),
        );
    }

    fn replace_feedback(
        &mut self,
        asserted_by: &str,
        origin: &str,
        proposal_ref: &str,
        disposition: &str,
    ) {
        self.decision_feedback_oid = put_json(
            &self.repository,
            decision_feedback_record(asserted_by, origin, proposal_ref, disposition),
        );
        let snapshot = if disposition == "adopted_unchanged" {
            self.proposal_tree_oid.clone()
        } else {
            self.base_tree_oid.clone()
        };
        let parent = self.decision_head.clone();
        let feedback = self.decision_feedback_oid.clone();
        let human = self.authenticated_human_id.clone();
        self.rebuild_decision("decision", &[parent], &snapshot, &human, &[feedback]);
    }

    fn advance_decision_ref(&mut self) {
        let advanced = put_json(
            &self.repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&self.decision_head),
                &self.base_tree_oid,
                &[],
                HUMAN_ID,
                "concurrent decision advance",
            ),
        );
        self.repository
            .update_ref(RefUpdate {
                ref_name: &self.decision_ref_name,
                expected_head: Some(&self.decision_head),
                new_head: &advanced,
                metadata: ReflogMetadata::at(4),
            })
            .unwrap();
    }

    fn advance_proposal_ref(&mut self) {
        self.repository
            .update_ref(RefUpdate {
                ref_name: &self.proposal_ref_name,
                expected_head: Some(&self.proposal_head),
                new_head: &self.decision_head,
                metadata: ReflogMetadata::at(4),
            })
            .unwrap();
    }
}

fn actor_record(entity_id: &str, actor_kind: &str, asserted_by: &str, name: &str) -> JsonValue {
    let ai_profile = (actor_kind == "ai_agent").then(|| {
        json!({
            "model_id": "human-decision-fixture",
            "model_version": "1",
            "capabilities": ["propose_branch", "read_context"]
        })
    });
    let mut payload = json!({
        "actor_kind": actor_kind,
        "display_name": name
    });
    if let Some(ai_profile) = ai_profile {
        payload["ai_profile"] = ai_profile;
    }
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "actor",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": asserted_by,
        "origin": "self_declared",
        "source_refs": [],
        "payload": payload,
        "extensions": {}
    })
}

fn policy_record(
    entity_id: &str,
    project_id: &str,
    asserted_by: &str,
    mode: PolicyMode,
) -> JsonValue {
    let gate = json!({
        "rule_id": "gate-decision",
        "effect": "require_human_gate",
        "action": "publish",
        "resource_selector": "decision/**",
        "human_gate": "before_decision_ref"
    });
    let (rules, default_effect) = match mode {
        PolicyMode::DecisionGate => (vec![gate], "deny"),
        PolicyMode::AiProposalAndDecisionGate => (
            vec![
                json!({
                    "rule_id": "allow-context-read",
                    "effect": "allow",
                    "action": "read",
                    "resource_selector": "project/**"
                }),
                json!({
                    "rule_id": "allow-proposal",
                    "effect": "allow",
                    "action": "propose",
                    "resource_selector": "proposal/10000000-0000-4000-8000-000000000002/**"
                }),
                gate,
            ],
            "deny",
        ),
        PolicyMode::ExplicitAllow => (
            vec![json!({
                "rule_id": "allow-decision",
                "effect": "allow",
                "action": "publish",
                "resource_selector": "decision/**"
            })],
            "deny",
        ),
        PolicyMode::DenyOverGate => (
            vec![
                gate,
                json!({
                    "rule_id": "deny-main-decision",
                    "effect": "deny",
                    "action": "publish",
                    "resource_selector": DECISION_REF
                }),
            ],
            "deny",
        ),
        PolicyMode::OtherGate => (
            vec![json!({
                "rule_id": "gate-release",
                "effect": "require_human_gate",
                "action": "publish",
                "resource_selector": "decision/**",
                "human_gate": "before_release_ref"
            })],
            "deny",
        ),
        PolicyMode::DefaultAllow => (
            vec![json!({
                "rule_id": "irrelevant-read",
                "effect": "deny",
                "action": "read",
                "resource_selector": "project/**"
            })],
            "allow",
        ),
        PolicyMode::DefaultDeny => (
            vec![json!({
                "rule_id": "irrelevant-read",
                "effect": "allow",
                "action": "read",
                "resource_selector": "project/**"
            })],
            "deny",
        ),
        PolicyMode::ConditionalAllow => (
            vec![json!({
                "rule_id": "conditional-decision",
                "effect": "allow",
                "action": "publish",
                "resource_selector": "decision/**",
                "condition_text": "Only after an unavailable external condition."
            })],
            "deny",
        ),
        PolicyMode::UnsupportedSelector => (
            vec![json!({
                "rule_id": "unsupported-selector",
                "effect": "allow",
                "action": "publish",
                "resource_selector": "decision/*"
            })],
            "allow",
        ),
    };
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "policy",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": asserted_by,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "scope_refs": [project_id],
            "rules": rules,
            "default_effect": default_effect
        },
        "extensions": {}
    })
}

fn delegation_grant(entity_id: &str, project_id: &str, max_output_bytes: i64) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "delegation_grant",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "principal_ref": HUMAN_ID,
            "delegate_ref": AGENT_ID,
            "project_ref": project_id,
            "purpose": "Prepare one proposal for direct human review.",
            "capabilities": ["propose_branch", "read_context"],
            "resource_selectors": ["project/**"],
            "writable_ref_prefixes": ["proposal/10000000-0000-4000-8000-000000000002"],
            "data_classes": ["internal"],
            "allowed_egress": [],
            "may_delegate": false,
            "max_child_depth": 0,
            "max_output_bytes": max_output_bytes,
            "required_human_gates": ["before_decision_ref"],
            "expires_at": "9999-12-31T23:59:59.999999999Z"
        },
        "extensions": {}
    })
}

fn context_pack_record(base_commit: &str, policy_oid: &str, grant_oid: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "context_pack",
        "entity_id": CONTEXT_ENTITY_ID,
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "tool_recorded",
        "source_refs": [],
        "payload": {
            "base_commit": base_commit,
            "base_ref_name": DECISION_REF,
            "expected_ref_head": base_commit,
            "selected_context_refs": [base_commit],
            "must_preserve_constraints": [],
            "allowed_transformations": [],
            "unresolved_questions": [],
            "policy_snapshot_ref": policy_oid,
            "delegation_grant_ref": grant_oid,
            "data_classification": "internal"
        },
        "extensions": {}
    })
}

#[allow(clippy::too_many_arguments)]
fn activity_record(
    activity_kind: &str,
    status: &str,
    responsible_principal: &str,
    context_oid: &str,
    grant_oid: &str,
    output_oid: &str,
    required_human_gates: &[&str],
    binding_mode: ActivityBindingMode,
    side_effect_class: &str,
) -> JsonValue {
    let mut actor_refs = match binding_mode {
        ActivityBindingMode::MissingAgentRole => {
            vec![json!({ "role": "responsible_principal", "actor_ref": responsible_principal })]
        }
        ActivityBindingMode::MissingPrincipalRole => {
            vec![json!({ "role": "agent", "actor_ref": AGENT_ID })]
        }
        ActivityBindingMode::Valid | ActivityBindingMode::WrongContextInput => vec![
            json!({ "role": "responsible_principal", "actor_ref": responsible_principal }),
            json!({ "role": "agent", "actor_ref": AGENT_ID }),
        ],
    };
    actor_refs.sort_by(|left, right| {
        left.get("actor_ref")
            .and_then(JsonValue::as_str)
            .cmp(&right.get("actor_ref").and_then(JsonValue::as_str))
    });
    let context_input_oid = if binding_mode == ActivityBindingMode::WrongContextInput {
        output_oid
    } else {
        context_oid
    };
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
            "activity_kind": activity_kind,
            "actor_refs": actor_refs,
            "subject_refs": [],
            "input_refs": [{ "role": "context", "oid": context_input_oid }],
            "output_refs": [{ "role": "proposal", "oid": output_oid }],
            "reversibility": "reversible",
            "side_effect_class": side_effect_class,
            "ai_run": {
                "agent_ref": AGENT_ID,
                "responsible_principal_ref": responsible_principal,
                "context_pack_ref": context_oid,
                "delegation_grant_ref": grant_oid,
                "requested_capabilities": ["propose_branch", "read_context"],
                "required_human_gates": required_human_gates,
                "status": status
            }
        },
        "extensions": {}
    })
}

fn decision_feedback_record(
    asserted_by: &str,
    origin: &str,
    proposal_ref: &str,
    disposition: &str,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "decision_feedback",
        "entity_id": FEEDBACK_ENTITY_ID,
        "recorded_at": RECORDED_AT,
        "asserted_by": asserted_by,
        "origin": origin,
        "source_refs": [],
        "payload": {
            "proposal_ref": proposal_ref,
            "disposition": disposition,
            "reason_codes": ["goal_fit"],
            "human_rationale": "Direct human fixture decision.",
            "visibility": "project",
            "training_use_policy": "project_local_only"
        },
        "extensions": {}
    })
}

fn claim_record(asserted_by: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "claim",
        "entity_id": "urn:uuid:10000000-0000-4000-8000-000000000071",
        "recorded_at": RECORDED_AT,
        "asserted_by": asserted_by,
        "origin": "inferred",
        "source_refs": [],
        "payload": {
            "claim_kind": "interpretation",
            "epistemic_class": "suggested",
            "subject_refs": [PROJECT_ID],
            "predicate": "human_decision_fixture",
            "value_text": "A fixture AI claim.",
            "evidence_refs": []
        },
        "extensions": {}
    })
}

fn subject_record() -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "subject",
        "entity_id": "urn:uuid:10000000-0000-4000-8000-000000000080",
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "subject_kind": "hybrid",
            "label": "Human decision fixture subject",
            "relation_refs": [],
            "spatial_frame_refs": []
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

fn copy_tree_entries(repository: &Repository, tree_oid: &str) -> JsonMap<String, JsonValue> {
    repository
        .objects()
        .get_verified(tree_oid)
        .unwrap()
        .unwrap()
        .structured()
        .unwrap()
        .get("entries")
        .unwrap()
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, entry)| {
            let entry_kind = entry.get("entry_kind").unwrap().as_str().unwrap();
            let oid = entry.get("oid").unwrap().as_str().unwrap();
            (
                name.clone(),
                json!({ "entry_kind": entry_kind, "oid": oid }),
            )
        })
        .collect()
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

fn commit_with_bound_declarations(
    kind: &str,
    parents: &[String],
    snapshot: &str,
    transitions: &[String],
    bound_declarations: &[String],
    author: &str,
    message: &str,
) -> JsonValue {
    let mut value = commit(kind, parents, snapshot, transitions, author, message);
    value["bound_declaration_refs"] = json!(bound_declarations);
    value
}

fn put_json(repository: &Repository, value: JsonValue) -> String {
    repository
        .put_object(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .oid
}

fn assert_failure_unchanged(mut scenario: Scenario, expected_code: &str) -> RepositoryError {
    let refs_before = scenario.repository.refs().snapshot().unwrap();
    let reflog_before = scenario.repository.refs().reflog().unwrap();
    let error = scenario
        .publish()
        .expect_err("human decision admission must reject the scenario");
    assert_eq!(error.code(), expected_code, "unexpected error: {error}");
    assert_eq!(scenario.repository.refs().snapshot().unwrap(), refs_before);
    assert_eq!(scenario.repository.refs().reflog().unwrap(), reflog_before);
    error
}

#[test]
fn adopted_unchanged_publishes_exact_proposal_snapshot_and_authenticated_reflog() {
    let mut scenario = ScenarioBuilder::default().build();
    let expected_head = scenario.new_head.clone();
    let human_actor = scenario.human_actor_record_oid.clone();
    let policy = scenario.policy_record_oid.clone();
    let proposal = scenario.proposal_head.clone();
    let feedback = scenario.decision_feedback_oid.clone();

    let receipt = scenario.publish().unwrap();

    assert_eq!(receipt.disposition, DecisionDisposition::AdoptedUnchanged);
    assert_eq!(receipt.human_actor_record_oid, human_actor);
    assert_eq!(receipt.policy_record_oid, policy);
    assert_eq!(receipt.proposal_commit_oid, proposal);
    assert_eq!(receipt.decision_feedback_oid, feedback);
    assert_eq!(receipt.reflog.ref_name, DECISION_REF);
    assert_eq!(
        receipt.reflog.old_head.as_deref(),
        Some(scenario.decision_head.as_str())
    );
    assert_eq!(receipt.reflog.new_head, expected_head);
    assert_eq!(receipt.reflog.actor.as_deref(), Some(HUMAN_ID));
    assert_eq!(receipt.reflog.message.as_deref(), Some(MESSAGE));
    assert_eq!(
        receipt.reflog.occurred_at_unix_nanos,
        i64::try_from(FIXED_NOW_NANOS).unwrap()
    );
    assert_eq!(scenario.decision_snapshot_oid, scenario.proposal_tree_oid);
    assert_eq!(
        scenario
            .repository
            .refs()
            .get(DECISION_REF)
            .unwrap()
            .unwrap()
            .head,
        expected_head
    );
    assert_eq!(
        scenario
            .repository
            .refs()
            .get(PROPOSAL_REF)
            .unwrap()
            .unwrap()
            .head,
        proposal
    );
}

#[test]
fn creative_ai_proposal_can_be_adopted_then_extend_the_retained_authority_snapshot() {
    let mut scenario = ScenarioBuilder {
        policy_mode: PolicyMode::AiProposalAndDecisionGate,
        seed_proposal_ref: false,
        ..ScenarioBuilder::default()
    }
    .build();
    let capabilities = [AiCapability::ReadContext, AiCapability::ProposeBranch];
    let initial_ai_authority = AiExecutionAuthority::new(
        AGENT_ID,
        PROJECT_ID,
        HUMAN_ID,
        DECISION_REF,
        &scenario.agent_actor_record_oid,
        &scenario.human_actor_record_oid,
        &scenario.context_oid,
        &capabilities,
        &capabilities,
    );
    let proposal_receipt = CreativeAiRuntime::with_clock(
        &mut scenario.repository,
        initial_ai_authority,
        TestClock::fixed(Ok(FIXED_NOW_NANOS)),
    )
    .publish_proposal(AiProposalUpdate {
        ref_name: PROPOSAL_REF,
        expected_head: None,
        new_head: &scenario.proposal_head,
        message: Some("publish reviewed proposal through AI admission"),
        activity_oid: &scenario.activity_oid,
    })
    .expect("the proposal must first pass CreativeAiRuntime");
    assert_eq!(proposal_receipt.policy_oid, scenario.policy_record_oid);
    assert_eq!(proposal_receipt.delegation_grant_oid, scenario.grant_oid);

    let adopted_head = scenario.new_head.clone();
    scenario
        .publish()
        .expect("the exact admitted proposal must be adoptable unchanged");
    assert_eq!(
        scenario
            .repository
            .refs()
            .get(DECISION_REF)
            .unwrap()
            .unwrap()
            .head,
        adopted_head
    );

    let mut second_entries = copy_tree_entries(&scenario.repository, &scenario.proposal_tree_oid);
    for retained_oid in [
        &scenario.human_actor_record_oid,
        &scenario.agent_actor_record_oid,
        &scenario.policy_record_oid,
        &scenario.grant_oid,
        &scenario.base_blob_oid,
        &scenario.subject_oid,
        &scenario.context_oid,
        &scenario.activity_oid,
        &scenario.output_oid,
    ] {
        assert!(
            second_entries.values().any(|entry| {
                entry.get("oid").and_then(JsonValue::as_str) == Some(retained_oid.as_str())
            }),
            "adopted snapshot lost {retained_oid}"
        );
    }

    let second_context_oid = put_json(
        &scenario.repository,
        context_pack_record(
            &adopted_head,
            &scenario.policy_record_oid,
            &scenario.grant_oid,
        ),
    );
    let second_output_oid = scenario
        .repository
        .put_blob(&b"second proposal after adopted authority continuity"[..])
        .unwrap()
        .oid;
    let second_activity_oid = put_json(
        &scenario.repository,
        activity_record(
            "ai_run",
            "proposal_ready",
            HUMAN_ID,
            &second_context_oid,
            &scenario.grant_oid,
            &second_output_oid,
            &["before_decision_ref"],
            ActivityBindingMode::Valid,
            "none",
        ),
    );
    second_entries.insert(
        "context-2.json".to_owned(),
        json!({ "entry_kind": "record", "oid": second_context_oid }),
    );
    second_entries.insert(
        "proposal-2.bin".to_owned(),
        json!({ "entry_kind": "blob", "oid": second_output_oid }),
    );
    second_entries.insert(
        "run-2.json".to_owned(),
        json!({ "entry_kind": "record", "oid": second_activity_oid }),
    );
    let second_tree_oid = put_json(&scenario.repository, manifest_tree(second_entries));
    let second_proposal_oid = put_json(
        &scenario.repository,
        commit(
            "checkpoint",
            std::slice::from_ref(&adopted_head),
            &second_tree_oid,
            std::slice::from_ref(&second_activity_oid),
            AGENT_ID,
            "second AI proposal after adoption",
        ),
    );
    let second_ai_authority = AiExecutionAuthority::new(
        AGENT_ID,
        PROJECT_ID,
        HUMAN_ID,
        DECISION_REF,
        &scenario.agent_actor_record_oid,
        &scenario.human_actor_record_oid,
        &second_context_oid,
        &capabilities,
        &capabilities,
    );
    CreativeAiRuntime::with_clock(
        &mut scenario.repository,
        second_ai_authority,
        TestClock::fixed(Ok(FIXED_NOW_NANOS + 1)),
    )
    .publish_proposal(AiProposalUpdate {
        ref_name: SECOND_PROPOSAL_REF,
        expected_head: None,
        new_head: &second_proposal_oid,
        message: Some("publish a second proposal from adopted base"),
        activity_oid: &second_activity_oid,
    })
    .expect("retained authority objects must authorize the next proposal");
    assert_eq!(
        scenario
            .repository
            .refs()
            .get(SECOND_PROPOSAL_REF)
            .unwrap()
            .unwrap()
            .head,
        second_proposal_oid
    );
}

#[test]
fn non_adopting_dispositions_publish_a_decision_while_preserving_the_base_snapshot() {
    for (text, expected) in [
        ("rejected", DecisionDisposition::Rejected),
        ("deferred", DecisionDisposition::Deferred),
        ("experiment_only", DecisionDisposition::ExperimentOnly),
    ] {
        let mut scenario = ScenarioBuilder {
            disposition: text,
            ..ScenarioBuilder::default()
        }
        .build();
        let proposal = scenario.proposal_head.clone();

        let receipt = scenario.publish().unwrap();

        assert_eq!(receipt.disposition, expected, "{text}");
        assert_eq!(
            scenario.decision_snapshot_oid, scenario.base_tree_oid,
            "{text}"
        );
        assert_eq!(
            scenario
                .repository
                .refs()
                .get(PROPOSAL_REF)
                .unwrap()
                .unwrap()
                .head,
            proposal,
            "{text}"
        );
    }
}

#[test]
fn modified_and_partial_dispositions_are_archival_only_and_fail_atomically() {
    for disposition in ["adopted_modified", "partially_adopted"] {
        let scenario = ScenarioBuilder {
            disposition,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains("modified or partial adoption"),
            "{disposition}: {error}"
        );
    }
}

#[test]
fn policy_allows_a_direct_decision_gate_explicit_allow_or_default_allow() {
    for mode in [
        PolicyMode::DecisionGate,
        PolicyMode::ExplicitAllow,
        PolicyMode::DefaultAllow,
    ] {
        let mut scenario = ScenarioBuilder {
            policy_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        scenario
            .publish()
            .unwrap_or_else(|error| panic!("{mode:?} must authorize: {error}"));
    }
}

#[test]
fn policy_deny_default_deny_conditional_and_unsupported_selectors_fail_closed() {
    for mode in [
        PolicyMode::DenyOverGate,
        PolicyMode::DefaultDeny,
        PolicyMode::ConditionalAllow,
        PolicyMode::UnsupportedSelector,
    ] {
        let scenario = ScenarioBuilder {
            policy_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(match mode {
                PolicyMode::DenyOverGate => "denies",
                PolicyMode::DefaultDeny => "default denies",
                PolicyMode::ConditionalAllow => "conditional",
                PolicyMode::UnsupportedSelector => "unsupported selector",
                _ => unreachable!(),
            }),
            "{mode:?}: {error}"
        );
    }
}

#[test]
fn a_gate_other_than_before_decision_ref_remains_unsatisfied() {
    let scenario = ScenarioBuilder {
        policy_mode: PolicyMode::OtherGate,
        ..ScenarioBuilder::default()
    }
    .build();
    assert_failure_unchanged(scenario, "human_gate_required");
}

#[test]
fn trusted_human_identity_kind_and_self_assertion_are_each_required() {
    let mut wrong_identity = ScenarioBuilder::default().build();
    wrong_identity.authenticated_human_id = OTHER_HUMAN_ID.to_owned();
    let error = assert_failure_unchanged(wrong_identity, "authorization_denied");
    assert!(error.to_string().contains("authenticated human"));

    for (mode, message) in [
        (HumanActorMode::AiSelfAsserted, "actor_kind=human"),
        (
            HumanActorMode::HumanAssertedByOther,
            "must be self-asserted",
        ),
    ] {
        let scenario = ScenarioBuilder {
            human_actor_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(error.to_string().contains(message), "{mode:?}: {error}");
    }
}

#[test]
fn exact_human_actor_and_policy_snapshots_must_be_bound_by_the_base() {
    for omission in [BaseOmission::HumanActor, BaseOmission::Policy] {
        let scenario = ScenarioBuilder {
            base_omission: Some(omission),
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains("does not bind the exact"),
            "{omission:?}: {error}"
        );
    }
}

#[test]
fn policy_scope_and_delegation_grant_project_must_match_trusted_project() {
    let wrong_scope = ScenarioBuilder {
        policy_scope_project: OTHER_PROJECT_ID,
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(wrong_scope, "authorization_denied");
    assert!(error.to_string().contains("authorized project"));

    let wrong_grant_project = ScenarioBuilder {
        proposal_chain_mode: ProposalChainMode::GrantProjectMismatch,
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(wrong_grant_project, "authorization_denied");
    assert!(error.to_string().contains("project_ref"));
}

#[test]
fn policy_snapshot_must_be_asserted_by_the_authenticated_human() {
    let scenario = ScenarioBuilder {
        policy_asserted_by: OTHER_HUMAN_ID,
        ..ScenarioBuilder::default()
    }
    .build();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(error.to_string().contains("Policy snapshot"), "{error}");
    assert!(error.to_string().contains("authenticated human"), "{error}");
}

#[test]
fn proposal_ai_activity_context_grant_and_base_chain_is_revalidated() {
    for mode in [
        ProposalChainMode::ActivityNotAiRun,
        ProposalChainMode::ActivityNotReady,
        ProposalChainMode::WrongResponsiblePrincipal,
        ProposalChainMode::MissingDecisionGate,
        ProposalChainMode::ContextBaseMismatch,
        ProposalChainMode::ContextGrantMismatch,
        ProposalChainMode::ContextPolicyMismatch,
    ] {
        let scenario = ScenarioBuilder {
            proposal_chain_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(match mode {
                ProposalChainMode::ActivityNotAiRun => "not an ai_run",
                ProposalChainMode::ActivityNotReady => "not proposal_ready",
                ProposalChainMode::WrongResponsiblePrincipal => "responsible principal",
                ProposalChainMode::MissingDecisionGate => "does not require",
                ProposalChainMode::ContextBaseMismatch => "base_commit",
                ProposalChainMode::ContextGrantMismatch => "delegation_grant_ref",
                ProposalChainMode::ContextPolicyMismatch => "policy_snapshot_ref",
                _ => unreachable!(),
            }),
            "{mode:?}: {error}"
        );
    }
}

#[test]
fn proposal_activity_requires_safe_side_effect_roles_and_exact_context_input() {
    let unsafe_side_effect = ScenarioBuilder {
        side_effect_class: "external",
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(unsafe_side_effect, "authorization_denied");
    assert!(error.to_string().contains("side effect class"), "{error}");

    for mode in [
        ActivityBindingMode::MissingAgentRole,
        ActivityBindingMode::MissingPrincipalRole,
        ActivityBindingMode::WrongContextInput,
    ] {
        let scenario = ScenarioBuilder {
            activity_binding_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(match mode {
                ActivityBindingMode::MissingAgentRole => "agent",
                ActivityBindingMode::MissingPrincipalRole => "responsible_principal",
                ActivityBindingMode::WrongContextInput => "context",
                ActivityBindingMode::Valid => unreachable!(),
            }),
            "{mode:?}: {error}"
        );
    }
}

#[test]
fn proposal_outputs_are_rechecked_for_declaration_assertion_type_and_quota() {
    for (mode, expected_message) in [
        (OutputMode::UndeclaredBlob, "undeclared object"),
        (OutputMode::HumanAssertedClaim, "authenticated agent"),
        (
            OutputMode::DisallowedRecord,
            "Record type decision_feedback",
        ),
    ] {
        let scenario = ScenarioBuilder {
            output_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(expected_message),
            "{mode:?}: {error}"
        );
    }

    let over_quota = ScenarioBuilder {
        max_output_bytes: 1,
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(over_quota, "authorization_denied");
    assert!(
        error.to_string().contains("output closure totals"),
        "{error}"
    );
}

#[test]
fn reviewed_proposal_commit_and_snapshot_contract_is_revalidated() {
    for mode in [
        ProposalMode::WrongKind,
        ProposalMode::WrongParent,
        ProposalMode::WrongAuthor,
        ProposalMode::DropsBaseBlob,
        ProposalMode::AddsProtectedControl,
    ] {
        let scenario = ScenarioBuilder {
            proposal_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(match mode {
                ProposalMode::WrongKind => "checkpoint",
                ProposalMode::WrongParent => "sole parent",
                ProposalMode::WrongAuthor => "author does not match",
                ProposalMode::DropsBaseBlob => "does not retain canonical base object",
                ProposalMode::AddsProtectedControl => "protected control",
                ProposalMode::Valid => unreachable!(),
            }),
            "{mode:?}: {error}"
        );
    }
}

#[test]
fn supported_dispositions_reject_a_snapshot_other_than_the_exact_required_tree() {
    let mut adopted = ScenarioBuilder::default().build();
    let parent = adopted.decision_head.clone();
    let base_snapshot = adopted.base_tree_oid.clone();
    let human = adopted.authenticated_human_id.clone();
    let feedback = adopted.decision_feedback_oid.clone();
    adopted.rebuild_decision("decision", &[parent], &base_snapshot, &human, &[feedback]);
    let error = assert_failure_unchanged(adopted, "authorization_denied");
    assert!(error.to_string().contains("exact proposal snapshot"));

    for disposition in ["rejected", "deferred", "experiment_only"] {
        let mut scenario = ScenarioBuilder {
            disposition,
            ..ScenarioBuilder::default()
        }
        .build();
        let parent = scenario.decision_head.clone();
        let proposal_snapshot = scenario.proposal_tree_oid.clone();
        let human = scenario.authenticated_human_id.clone();
        let feedback = scenario.decision_feedback_oid.clone();
        scenario.rebuild_decision(
            "decision",
            &[parent],
            &proposal_snapshot,
            &human,
            &[feedback],
        );
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains("exact canonical base snapshot"),
            "{disposition}: {error}"
        );
    }
}

#[test]
fn decision_commit_kind_parent_author_and_exact_feedback_binding_are_enforced() {
    for case in [
        "kind",
        "parent",
        "author",
        "missing-feedback",
        "extra-feedback",
    ] {
        let mut scenario = ScenarioBuilder::default().build();
        let mut kind = "decision";
        let mut parents = vec![scenario.decision_head.clone()];
        let snapshot = scenario.proposal_tree_oid.clone();
        let mut author = HUMAN_ID;
        let mut transitions = vec![scenario.decision_feedback_oid.clone()];
        match case {
            "kind" => kind = "checkpoint",
            "parent" => parents.clear(),
            "author" => author = OTHER_HUMAN_ID,
            "missing-feedback" => transitions.clear(),
            "extra-feedback" => {
                transitions.push(scenario.activity_oid.clone());
                transitions.sort();
            }
            _ => unreachable!(),
        }
        scenario.rebuild_decision(kind, &parents, &snapshot, author, &transitions);

        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(match case {
                "kind" => "decision Commit",
                "parent" => "sole parent",
                "author" => "authenticated human",
                "missing-feedback" | "extra-feedback" => "exactly the requested transition",
                _ => unreachable!(),
            }),
            "{case}: {error}"
        );
    }
}

#[test]
fn feedback_assertion_origin_and_exact_proposal_binding_are_enforced() {
    for case in ["asserted-by", "origin", "proposal"] {
        let mut scenario = ScenarioBuilder::default().build();
        let proposal = scenario.proposal_head.clone();
        let (asserted_by, origin, proposal_ref) = match case {
            "asserted-by" => (OTHER_HUMAN_ID, "self_declared", proposal.as_str()),
            "origin" => (HUMAN_ID, "tool_recorded", proposal.as_str()),
            "proposal" => (HUMAN_ID, "self_declared", scenario.decision_head.as_str()),
            _ => unreachable!(),
        };
        let asserted_by = asserted_by.to_owned();
        let origin = origin.to_owned();
        let proposal_ref = proposal_ref.to_owned();
        scenario.replace_feedback(&asserted_by, &origin, &proposal_ref, "adopted_unchanged");

        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(match case {
                "asserted-by" => "asserted by",
                "origin" => "self-declared",
                "proposal" => "reviewed proposal",
                _ => unreachable!(),
            }),
            "{case}: {error}"
        );
    }
}

#[test]
fn decision_bound_declarations_cannot_smuggle_a_control_record_outside_the_snapshot() {
    let mut scenario = ScenarioBuilder::default().build();
    let introduced_actor = put_json(
        &scenario.repository,
        actor_record(
            OTHER_HUMAN_ID,
            "human",
            OTHER_HUMAN_ID,
            "Out-of-snapshot bound Actor",
        ),
    );
    scenario.new_head = put_json(
        &scenario.repository,
        commit_with_bound_declarations(
            "decision",
            std::slice::from_ref(&scenario.decision_head),
            &scenario.proposal_tree_oid,
            std::slice::from_ref(&scenario.decision_feedback_oid),
            &[introduced_actor],
            HUMAN_ID,
            "decision with forbidden bound declaration",
        ),
    );

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(error.to_string().contains("bound declarations"), "{error}");
    assert!(error.to_string().contains("cannot introduce"), "{error}");
}

#[test]
fn proposal_and_decision_ref_changes_are_atomic_transaction_preconditions() {
    let mut moved_proposal = ScenarioBuilder::default().build();
    moved_proposal.advance_proposal_ref();
    let error = assert_failure_unchanged(moved_proposal, "ref_conflict");
    assert!(error.to_string().contains(PROPOSAL_REF));

    let mut moved_decision = ScenarioBuilder::default().build();
    moved_decision.advance_decision_ref();
    let error = assert_failure_unchanged(moved_decision, "ref_conflict");
    assert!(error.to_string().contains(DECISION_REF));

    let mut both_moved = ScenarioBuilder::default().build();
    both_moved.advance_proposal_ref();
    both_moved.advance_decision_ref();
    let error = assert_failure_unchanged(both_moved, "ref_conflict");
    assert!(
        error.to_string().contains(PROPOSAL_REF),
        "proposal precondition must be checked before the target decision Ref: {error}"
    );
}

#[test]
fn unauthorized_decisions_precede_live_ref_conflicts_without_mutation() {
    let mut scenario = ScenarioBuilder {
        policy_mode: PolicyMode::DefaultDeny,
        ..ScenarioBuilder::default()
    }
    .build();
    scenario.advance_proposal_ref();
    scenario.advance_decision_ref();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(error.to_string().contains("default denies"));
}

#[test]
fn trusted_clock_failure_and_backwards_motion_roll_back_decision_and_reflog() {
    let mut unavailable = ScenarioBuilder::default().build();
    unavailable.clock = TestClock::fixed(Err("trusted reviewer clock unavailable".to_owned()));
    let error = assert_failure_unchanged(unavailable, "storage_error");
    assert!(error.to_string().contains("clock unavailable"));

    let mut transaction_failure = ScenarioBuilder::default().build();
    transaction_failure.clock = TestClock::sequence([
        Ok(FIXED_NOW_NANOS),
        Err("transaction clock unavailable".to_owned()),
    ]);
    let error = assert_failure_unchanged(transaction_failure, "storage_error");
    assert!(error.to_string().contains("transaction clock unavailable"));

    let mut backwards = ScenarioBuilder::default().build();
    backwards.clock = TestClock::sequence([Ok(FIXED_NOW_NANOS), Ok(FIXED_NOW_NANOS - 1)]);
    let error = assert_failure_unchanged(backwards, "storage_error");
    assert!(error.to_string().contains("moved backwards"));
}

#[test]
fn exact_replay_cannot_append_a_second_decision_or_reflog_event() {
    let mut scenario = ScenarioBuilder::default().build();
    scenario.publish().unwrap();
    let refs_after_first = scenario.repository.refs().snapshot().unwrap();
    let reflog_after_first = scenario.repository.refs().reflog().unwrap();

    let error = scenario.publish().unwrap_err();

    assert_eq!(error.code(), "ref_conflict");
    assert_eq!(
        scenario.repository.refs().snapshot().unwrap(),
        refs_after_first
    );
    assert_eq!(
        scenario.repository.refs().reflog().unwrap(),
        reflog_after_first
    );
}

#[test]
fn updated_authority_rejects_feedback_already_present_in_canonical_lineage() {
    let mut scenario = ScenarioBuilder::default().build();
    scenario.publish().unwrap();
    let canonical_decision = scenario.new_head.clone();

    scenario.decision_head = canonical_decision.clone();
    let proposal_snapshot = scenario.proposal_tree_oid.clone();
    let human = scenario.authenticated_human_id.clone();
    let feedback = scenario.decision_feedback_oid.clone();
    scenario.rebuild_decision(
        "decision",
        &[canonical_decision],
        &proposal_snapshot,
        &human,
        &[feedback],
    );

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(
        error.to_string().contains("already contains feedback"),
        "{error}"
    );
}

#[derive(Clone)]
struct ConcurrentCall {
    authenticated_human_id: String,
    authorized_project_id: String,
    decision_ref_name: String,
    decision_head: String,
    proposal_ref_name: String,
    proposal_head: String,
    human_actor_record_oid: String,
    policy_record_oid: String,
    new_head: String,
    decision_feedback_oid: String,
}

impl ConcurrentCall {
    fn from_scenario(scenario: &Scenario) -> Self {
        Self {
            authenticated_human_id: scenario.authenticated_human_id.clone(),
            authorized_project_id: scenario.authorized_project_id.clone(),
            decision_ref_name: scenario.decision_ref_name.clone(),
            decision_head: scenario.decision_head.clone(),
            proposal_ref_name: scenario.proposal_ref_name.clone(),
            proposal_head: scenario.proposal_head.clone(),
            human_actor_record_oid: scenario.human_actor_record_oid.clone(),
            policy_record_oid: scenario.policy_record_oid.clone(),
            new_head: scenario.new_head.clone(),
            decision_feedback_oid: scenario.decision_feedback_oid.clone(),
        }
    }

    fn publish(
        &self,
        repository: &mut Repository,
    ) -> Result<HumanDecisionReceipt, RepositoryError> {
        let authority = HumanDecisionAuthority::new(
            &self.authenticated_human_id,
            &self.authorized_project_id,
            &self.decision_ref_name,
            &self.decision_head,
            &self.proposal_ref_name,
            &self.proposal_head,
            &self.human_actor_record_oid,
            &self.policy_record_oid,
        );
        HumanDecisionRuntime::with_clock(
            repository,
            authority,
            TestClock::fixed(Ok(FIXED_NOW_NANOS)),
        )
        .publish_decision(HumanDecisionUpdate {
            new_head: &self.new_head,
            decision_feedback_oid: &self.decision_feedback_oid,
            message: Some(MESSAGE),
        })
    }
}

#[test]
fn concurrent_decisions_from_the_same_base_allow_exactly_one_winner() {
    let scenario = ScenarioBuilder::default().build();
    let call = ConcurrentCall::from_scenario(&scenario);
    let path = scenario.repository_path.clone();
    let barrier = Arc::new(Barrier::new(2));

    let results = std::thread::scope(|scope| {
        let first_call = call.clone();
        let first_path = path.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = scope.spawn(move || {
            let mut repository = Repository::open(first_path).unwrap();
            first_barrier.wait();
            first_call.publish(&mut repository)
        });

        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            let mut repository = Repository::open(path).unwrap();
            second_barrier.wait();
            call.publish(&mut repository)
        });

        vec![first.join().unwrap(), second.join().unwrap()]
    });

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results.into_iter().find_map(Result::err).unwrap();
    assert_eq!(loser.code(), "ref_conflict", "unexpected loser: {loser}");
    assert_eq!(
        scenario
            .repository
            .refs()
            .get(DECISION_REF)
            .unwrap()
            .unwrap()
            .head,
        scenario.new_head
    );
    assert_eq!(
        scenario
            .repository
            .refs()
            .reflog_for_ref(DECISION_REF)
            .unwrap()
            .len(),
        2
    );
}
