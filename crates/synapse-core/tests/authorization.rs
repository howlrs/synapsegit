use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use synapse_core::{
    AiCapability, AiExecutionAuthority, AiGeneratedProposal, AiPreflightDecision, AiProposalUpdate,
    AiPublicationTarget, AiSideEffectClass, AuthorizationClock, AuthorizationDecision,
    CreativeAiRuntime, Repository, RepositoryError,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

const PRINCIPAL_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000001";
const AGENT_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000002";
const PROJECT_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000010";
const OTHER_ACTOR_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000003";
const OTHER_PROJECT_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000011";
const OTHER_PRINCIPAL_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000012";
const ACTOR_ENTITY_ID: &str = AGENT_ID;
const POLICY_ENTITY_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000030";
const GRANT_ENTITY_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000040";
const ALTERNATE_GRANT_ENTITY_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000041";
const CONTEXT_ENTITY_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000050";
const ACTIVITY_ENTITY_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000060";
const ALTERNATE_ACTIVITY_ENTITY_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000061";
const BASE_REF: &str = "decision/main";
const PROPOSAL_ROOT: &str = "proposal/00000000-0000-4000-8000-000000000002";
const PROPOSAL_REF: &str = "proposal/00000000-0000-4000-8000-000000000002/run-1";
const RECORDED_AT: &str = "1970-01-01T00:00:01.000000000Z";
const VALID_EXPIRES_AT: &str = "9999-12-31T23:59:59.999999999Z";
const EXPIRED_AT: &str = "1970-01-01T00:00:02.000000000Z";
const EXPIRED_AT_NANOS: i128 = 2_000_000_000;
const FIXED_NOW_NANOS: i128 = 3_000_000_000;
const FUTURE_RECORDED_AT: &str = "1970-01-01T00:00:04.000000000Z";
const TRANSACTION_EXPIRES_AT: &str = "1970-01-01T00:00:04.000000000Z";
const TRANSACTION_EXPIRES_AT_NANOS: i128 = 4_000_000_000;
const MESSAGE: &str = "authorized fixture proposal";

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
            "synapsegit-authorization-test-{}-{id}",
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
enum CrosslinkMismatch {
    None,
    CommitToActivity,
    ActivityToContextGrant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyMode {
    ExplicitAllow,
    DefaultDeny,
    DenyOverAllow,
    HumanGateOverAllow,
    ConditionalAllow,
    UnsupportedSelectorDefaultAllow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateMode {
    DeclaredBlob,
    UndeclaredBlob,
    UndeclaredRecord,
    UndeclaredTree,
    MissingDeclaredOutput,
    NestedTreeOutput,
    HumanAssertedClaimOutput,
    AuthorityRecordOutput,
    TombstoneOutput,
    NestedCommitOutput,
    AnalysisResultOutput,
    AnalysisOfSelectedBaseInput,
    AnalysisOfSelectedBaseInputCopiedIntoSnapshot,
    AnalysisOfFixedContextInput,
    CurrentBasePolicyOutput,
    AgentClaimOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BaseBinding {
    Actor,
    Grant,
    Policy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateBaseOmission {
    Actor,
    Principal,
    Grant,
    Policy,
    Blob,
    Record,
}

struct ScenarioBuilder {
    actor_has_capability: bool,
    grant_has_capability: bool,
    runtime_has_capability: bool,
    include_analyze_capability: bool,
    include_submit_claim_capability: bool,
    policy_mode: PolicyMode,
    expires_at: &'static str,
    writable_prefix: String,
    resource_selector: String,
    max_output_bytes: i64,
    side_effect_class: &'static str,
    include_side_effect_class: bool,
    requested_capabilities: Vec<&'static str>,
    grant_recorded_at: &'static str,
    candidate_mode: CandidateMode,
    ref_name: String,
    crosslink_mismatch: CrosslinkMismatch,
    omitted_base_binding: Option<BaseBinding>,
    omitted_candidate_base_entry: Option<CandidateBaseOmission>,
}

impl Default for ScenarioBuilder {
    fn default() -> Self {
        Self {
            actor_has_capability: true,
            grant_has_capability: true,
            runtime_has_capability: true,
            include_analyze_capability: false,
            include_submit_claim_capability: false,
            policy_mode: PolicyMode::ExplicitAllow,
            expires_at: VALID_EXPIRES_AT,
            writable_prefix: PROPOSAL_ROOT.to_owned(),
            resource_selector: "project/**".to_owned(),
            max_output_bytes: 10_000,
            side_effect_class: "none",
            include_side_effect_class: true,
            requested_capabilities: vec!["propose_branch", "read_context"],
            grant_recorded_at: RECORDED_AT,
            candidate_mode: CandidateMode::DeclaredBlob,
            ref_name: PROPOSAL_REF.to_owned(),
            crosslink_mismatch: CrosslinkMismatch::None,
            omitted_base_binding: None,
            omitted_candidate_base_entry: None,
        }
    }
}

impl ScenarioBuilder {
    fn build(self) -> Scenario {
        let temporary = TempDirectory::new();
        let mut repository = Repository::open(temporary.join("repo")).unwrap();

        let actor_capabilities = capability_strings(
            self.actor_has_capability,
            self.include_analyze_capability,
            self.include_submit_claim_capability,
        );
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
                    "display_name": "Authorization Test Agent",
                    "ai_profile": {
                        "model_id": "fixture-model",
                        "model_version": "1",
                        "capabilities": actor_capabilities
                    }
                },
                "extensions": {}
            }),
        );
        let principal_actor_oid = put_json(
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
                    "display_name": "Authorization Test Principal"
                },
                "extensions": {}
            }),
        );

        let allow_proposal = json!({
            "rule_id": "allow-proposal",
            "effect": "allow",
            "action": "propose",
            "resource_selector": format!("{PROPOSAL_ROOT}/**")
        });
        let allow_context_read = json!({
            "rule_id": "allow-context-read",
            "effect": "allow",
            "action": "read",
            "resource_selector": "project/**"
        });
        let (mut policy_rules, policy_default_effect) = match self.policy_mode {
            PolicyMode::ExplicitAllow => (vec![allow_context_read, allow_proposal], "deny"),
            PolicyMode::DefaultDeny => (
                vec![json!({
                    "rule_id": "deny-external-training",
                    "effect": "deny",
                    "action": "train_external",
                    "resource_selector": "project/**"
                })],
                "deny",
            ),
            PolicyMode::DenyOverAllow => (
                vec![
                    allow_context_read,
                    allow_proposal,
                    json!({
                        "rule_id": "deny-proposal",
                        "effect": "deny",
                        "action": "propose",
                        "resource_selector": format!("{PROPOSAL_ROOT}/**")
                    }),
                ],
                "deny",
            ),
            PolicyMode::HumanGateOverAllow => (
                vec![
                    allow_context_read,
                    allow_proposal,
                    json!({
                        "rule_id": "gate-proposal",
                        "effect": "require_human_gate",
                        "action": "propose",
                        "resource_selector": format!("{PROPOSAL_ROOT}/**"),
                        "human_gate": "before_decision_ref"
                    }),
                ],
                "deny",
            ),
            PolicyMode::ConditionalAllow => (
                vec![
                    allow_context_read,
                    json!({
                        "rule_id": "conditional-proposal",
                        "effect": "allow",
                        "action": "propose",
                        "resource_selector": format!("{PROPOSAL_ROOT}/**"),
                        "condition_text": "Only after an unimplemented external condition."
                    }),
                ],
                "deny",
            ),
            PolicyMode::UnsupportedSelectorDefaultAllow => (
                vec![json!({
                    "rule_id": "unsupported-read-selector",
                    "effect": "deny",
                    "action": "read",
                    "resource_selector": "project/*/context"
                })],
                "allow",
            ),
        };
        if self.include_analyze_capability {
            policy_rules.push(json!({
                "rule_id": "allow-analysis",
                "effect": "allow",
                "action": "analyze",
                "resource_selector": "project/**"
            }));
        }
        if self.include_submit_claim_capability {
            policy_rules.push(json!({
                "rule_id": "allow-derived-records",
                "effect": "allow",
                "action": "derive",
                "resource_selector": "project/**"
            }));
        }
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
                    "rules": policy_rules,
                    "default_effect": policy_default_effect
                },
                "extensions": {}
            }),
        );

        let grant_capabilities = capability_strings(
            self.grant_has_capability,
            self.include_analyze_capability,
            self.include_submit_claim_capability,
        );
        let mut grant = delegation_grant(
            GRANT_ENTITY_ID,
            &grant_capabilities,
            &self.writable_prefix,
            &self.resource_selector,
            self.max_output_bytes,
            self.expires_at,
        );
        grant["recorded_at"] = json!(self.grant_recorded_at);
        let grant_oid = put_json(&repository, grant);
        let base_blob_oid = repository
            .put_blob(&b"retained base evidence"[..])
            .unwrap()
            .oid;
        let base_record_oid = put_json(&repository, subject_record());

        let mut base_entries = JsonMap::new();
        if self.omitted_base_binding != Some(BaseBinding::Actor) {
            base_entries.insert(
                "actor.json".to_owned(),
                json!({ "entry_kind": "record", "oid": actor_oid }),
            );
        }
        if self.omitted_base_binding != Some(BaseBinding::Grant) {
            base_entries.insert(
                "grant.json".to_owned(),
                json!({ "entry_kind": "record", "oid": grant_oid }),
            );
        }
        if self.omitted_base_binding != Some(BaseBinding::Policy) {
            base_entries.insert(
                "policy.json".to_owned(),
                json!({ "entry_kind": "record", "oid": policy_oid }),
            );
        }
        base_entries.insert(
            "principal.json".to_owned(),
            json!({ "entry_kind": "record", "oid": principal_actor_oid }),
        );
        base_entries.insert(
            "evidence.bin".to_owned(),
            json!({ "entry_kind": "blob", "oid": base_blob_oid }),
        );
        base_entries.insert(
            "subject.json".to_owned(),
            json!({ "entry_kind": "record", "oid": base_record_oid }),
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
                PRINCIPAL_ID,
                "authorization base",
            ),
        );
        repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: None,
                new_head: &base_commit_oid,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: 1,
                    actor: Some(PRINCIPAL_ID),
                    message: Some("seed authorization base"),
                },
            })
            .unwrap();

        let context_oid = put_json(
            &repository,
            context_pack_record(&base_commit_oid, &policy_oid, &grant_oid),
        );

        let activity_grant_oid =
            if self.crosslink_mismatch == CrosslinkMismatch::ActivityToContextGrant {
                put_json(
                    &repository,
                    delegation_grant(
                        ALTERNATE_GRANT_ENTITY_ID,
                        &["propose_branch", "read_context"],
                        PROPOSAL_ROOT,
                        "project/**",
                        1024,
                        VALID_EXPIRES_AT,
                    ),
                )
            } else {
                grant_oid.clone()
            };
        let mut extra_entries = JsonMap::new();
        let mut nested_output_leaf_oid = None;
        let include_declared_output = self.candidate_mode != CandidateMode::MissingDeclaredOutput;
        let (output_oid, output_entry_kind) = match self.candidate_mode {
            CandidateMode::DeclaredBlob | CandidateMode::MissingDeclaredOutput => (
                repository.put_blob(&b"fixture proposal"[..]).unwrap().oid,
                "blob",
            ),
            CandidateMode::UndeclaredBlob => {
                let output = repository.put_blob(&b"declared output"[..]).unwrap().oid;
                let undeclared = repository
                    .put_blob(&b"undeclared snapshot blob"[..])
                    .unwrap()
                    .oid;
                extra_entries.insert(
                    "undeclared.bin".to_owned(),
                    json!({ "entry_kind": "blob", "oid": undeclared }),
                );
                (output, "blob")
            }
            CandidateMode::UndeclaredRecord => {
                let output = repository.put_blob(&b"declared output"[..]).unwrap().oid;
                let undeclared = put_json(&repository, claim_record(AGENT_ID));
                extra_entries.insert(
                    "undeclared.json".to_owned(),
                    json!({ "entry_kind": "record", "oid": undeclared }),
                );
                (output, "blob")
            }
            CandidateMode::UndeclaredTree => {
                let output = repository.put_blob(&b"declared output"[..]).unwrap().oid;
                let undeclared = put_json(&repository, manifest_tree(JsonMap::new()));
                extra_entries.insert(
                    "undeclared".to_owned(),
                    json!({ "entry_kind": "tree", "oid": undeclared }),
                );
                (output, "blob")
            }
            CandidateMode::NestedTreeOutput => {
                let leaf = repository
                    .put_blob(&b"deduplicated nested output"[..])
                    .unwrap()
                    .oid;
                let mut entries = JsonMap::new();
                entries.insert(
                    "first.bin".to_owned(),
                    json!({ "entry_kind": "blob", "oid": leaf }),
                );
                entries.insert(
                    "second.bin".to_owned(),
                    json!({ "entry_kind": "blob", "oid": leaf }),
                );
                let output_tree = put_json(&repository, manifest_tree(entries));
                nested_output_leaf_oid = Some(leaf);
                (output_tree, "tree")
            }
            CandidateMode::HumanAssertedClaimOutput => {
                (put_json(&repository, claim_record(PRINCIPAL_ID)), "record")
            }
            CandidateMode::AuthorityRecordOutput => {
                (put_json(&repository, authority_actor_record()), "record")
            }
            CandidateMode::TombstoneOutput => {
                let target = repository.put_blob(&b"tombstone target"[..]).unwrap().oid;
                (put_json(&repository, tombstone_record(&target)), "record")
            }
            CandidateMode::NestedCommitOutput => (
                put_json(
                    &repository,
                    commit(
                        "checkpoint",
                        &[],
                        &base_tree_oid,
                        &[],
                        AGENT_ID,
                        "nested output commit",
                    ),
                ),
                "commit",
            ),
            CandidateMode::AnalysisResultOutput => {
                let dependency = repository
                    .put_blob(&b"analysis dependency"[..])
                    .unwrap()
                    .oid;
                (
                    put_json(&repository, analysis_result_record(&dependency)),
                    "record",
                )
            }
            CandidateMode::AnalysisOfSelectedBaseInput
            | CandidateMode::AnalysisOfSelectedBaseInputCopiedIntoSnapshot => {
                if self.candidate_mode
                    == CandidateMode::AnalysisOfSelectedBaseInputCopiedIntoSnapshot
                {
                    extra_entries.insert(
                        "copied-selected-input".to_owned(),
                        json!({ "entry_kind": "commit", "oid": base_commit_oid }),
                    );
                }
                (
                    put_json(&repository, analysis_result_record(&base_commit_oid)),
                    "record",
                )
            }
            CandidateMode::AnalysisOfFixedContextInput => (
                put_json(&repository, analysis_result_record(&context_oid)),
                "record",
            ),
            CandidateMode::CurrentBasePolicyOutput => (policy_oid.clone(), "record"),
            CandidateMode::AgentClaimOutput => {
                (put_json(&repository, claim_record(AGENT_ID)), "record")
            }
        };
        let mut activity_body = activity(
            &context_oid,
            &activity_grant_oid,
            &output_oid,
            self.side_effect_class,
        );
        activity_body["payload"]["ai_run"]["requested_capabilities"] =
            json!(self.requested_capabilities);
        if !self.include_side_effect_class {
            activity_body["payload"]
                .as_object_mut()
                .unwrap()
                .remove("side_effect_class");
        }
        let activity_oid = put_json(&repository, activity_body.clone());
        let request_activity_oid = if self.crosslink_mismatch == CrosslinkMismatch::CommitToActivity
        {
            let mut alternate = activity_body;
            alternate["entity_id"] = json!(ALTERNATE_ACTIVITY_ENTITY_ID);
            put_json(&repository, alternate)
        } else {
            activity_oid.clone()
        };

        let mut proposal_entries = retained_base_entries;
        if let Some(omitted) = self.omitted_candidate_base_entry {
            let name = match omitted {
                CandidateBaseOmission::Actor => "actor.json",
                CandidateBaseOmission::Principal => "principal.json",
                CandidateBaseOmission::Grant => "grant.json",
                CandidateBaseOmission::Policy => "policy.json",
                CandidateBaseOmission::Blob => "evidence.bin",
                CandidateBaseOmission::Record => "subject.json",
            };
            proposal_entries.remove(name);
        }
        proposal_entries.extend(extra_entries);
        proposal_entries.insert(
            "context.json".to_owned(),
            json!({ "entry_kind": "record", "oid": context_oid }),
        );
        proposal_entries.insert(
            "run.json".to_owned(),
            json!({ "entry_kind": "record", "oid": activity_oid }),
        );
        if include_declared_output {
            proposal_entries.insert(
                "proposal".to_owned(),
                json!({ "entry_kind": output_entry_kind, "oid": output_oid }),
            );
        }
        let proposal_tree_oid = put_json(&repository, manifest_tree(proposal_entries));
        let candidate_oid = put_json(
            &repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&base_commit_oid),
                &proposal_tree_oid,
                std::slice::from_ref(&activity_oid),
                AGENT_ID,
                "AI proposal",
            ),
        );

        let mut granted_capabilities = vec![AiCapability::ReadContext];
        if self.include_analyze_capability {
            granted_capabilities.push(AiCapability::Analyze);
        }
        granted_capabilities.push(AiCapability::ProposeBranch);
        if self.include_submit_claim_capability {
            granted_capabilities.push(AiCapability::SubmitClaim);
        }
        let runtime_capabilities = if self.runtime_has_capability {
            granted_capabilities.clone()
        } else {
            Vec::new()
        };
        Scenario {
            _temporary: temporary,
            repository,
            ref_name: self.ref_name,
            expected_head: None,
            new_head: candidate_oid,
            actor_oid,
            principal_actor_oid,
            activity_oid: request_activity_oid,
            context_oid,
            grant_oid,
            policy_oid,
            authorized_capabilities: granted_capabilities,
            runtime_capabilities,
            base_commit_oid,
            base_tree_oid,
            base_blob_oid,
            base_record_oid,
            proposal_tree_oid,
            output_oid,
            nested_output_leaf_oid,
            authenticated_actor_id: AGENT_ID.to_owned(),
            authorized_project_id: PROJECT_ID.to_owned(),
            authorized_principal_id: PRINCIPAL_ID.to_owned(),
            authorized_base_ref: BASE_REF.to_owned(),
            clock: TestClock::fixed(Ok(FIXED_NOW_NANOS)),
        }
    }
}

struct Scenario {
    _temporary: TempDirectory,
    repository: Repository,
    ref_name: String,
    expected_head: Option<String>,
    new_head: String,
    actor_oid: String,
    principal_actor_oid: String,
    activity_oid: String,
    context_oid: String,
    grant_oid: String,
    policy_oid: String,
    authorized_capabilities: Vec<AiCapability>,
    runtime_capabilities: Vec<AiCapability>,
    base_commit_oid: String,
    base_tree_oid: String,
    base_blob_oid: String,
    base_record_oid: String,
    proposal_tree_oid: String,
    output_oid: String,
    nested_output_leaf_oid: Option<String>,
    authenticated_actor_id: String,
    authorized_project_id: String,
    authorized_principal_id: String,
    authorized_base_ref: String,
    clock: TestClock,
}

impl Scenario {
    fn preflight(
        &mut self,
        side_effect_class: AiSideEffectClass,
    ) -> Result<AiPreflightDecision, RepositoryError> {
        let authority = AiExecutionAuthority::new(
            &self.authenticated_actor_id,
            &self.authorized_project_id,
            &self.authorized_principal_id,
            &self.authorized_base_ref,
            &self.actor_oid,
            &self.principal_actor_oid,
            &self.context_oid,
            &self.authorized_capabilities,
            &self.runtime_capabilities,
        );
        let target = AiPublicationTarget::new(
            &self.ref_name,
            self.expected_head.as_deref(),
            side_effect_class,
        );
        CreativeAiRuntime::with_clock(&mut self.repository, authority, self.clock.clone())
            .preflight_proposal(target)
    }

    fn publish_preflighted(
        &mut self,
        decision: AiPreflightDecision,
    ) -> Result<AuthorizationDecision, RepositoryError> {
        let authority = AiExecutionAuthority::new(
            &self.authenticated_actor_id,
            &self.authorized_project_id,
            &self.authorized_principal_id,
            &self.authorized_base_ref,
            &self.actor_oid,
            &self.principal_actor_oid,
            &self.context_oid,
            &self.authorized_capabilities,
            &self.runtime_capabilities,
        );
        let generated = AiGeneratedProposal::new(&self.new_head, &self.activity_oid, Some(MESSAGE));
        CreativeAiRuntime::with_clock(&mut self.repository, authority, self.clock.clone())
            .publish_preflighted(decision, generated)
    }

    fn publish(&mut self) -> Result<AuthorizationDecision, RepositoryError> {
        let authority = AiExecutionAuthority::new(
            &self.authenticated_actor_id,
            &self.authorized_project_id,
            &self.authorized_principal_id,
            &self.authorized_base_ref,
            &self.actor_oid,
            &self.principal_actor_oid,
            &self.context_oid,
            &self.authorized_capabilities,
            &self.runtime_capabilities,
        );
        let request = AiProposalUpdate {
            ref_name: &self.ref_name,
            expected_head: self.expected_head.as_deref(),
            new_head: &self.new_head,
            message: Some(MESSAGE),
            activity_oid: &self.activity_oid,
        };
        CreativeAiRuntime::with_clock(&mut self.repository, authority, self.clock.clone())
            .publish_proposal(request)
    }

    fn advance_base_ref(&mut self) {
        let advanced = put_json(
            &self.repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&self.base_commit_oid),
                &self.base_tree_oid,
                &[],
                PRINCIPAL_ID,
                "advance live base",
            ),
        );
        self.repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: Some(&self.base_commit_oid),
                new_head: &advanced,
                metadata: ReflogMetadata::at(2),
            })
            .unwrap();
    }

    fn occupy_target_ref(&mut self) {
        self.repository
            .update_ref(RefUpdate {
                ref_name: &self.ref_name,
                expected_head: None,
                new_head: &self.base_commit_oid,
                metadata: ReflogMetadata::at(2),
            })
            .unwrap();
    }

    fn replace_candidate_commit(&mut self, kind: &str, parents: &[String]) {
        self.new_head = put_json(
            &self.repository,
            commit(
                kind,
                parents,
                &self.proposal_tree_oid,
                std::slice::from_ref(&self.activity_oid),
                AGENT_ID,
                "candidate Commit contract fixture",
            ),
        );
    }

    fn create_unrelated_parent(&self) -> String {
        put_json(
            &self.repository,
            commit(
                "checkpoint",
                &[],
                &self.base_tree_oid,
                &[],
                PRINCIPAL_ID,
                "unrelated candidate parent",
            ),
        )
    }

    fn rebase_context_while_authority_objects_remain_only_in_parent(&mut self) {
        let previous_base = self.base_commit_oid.clone();
        let mut entries = JsonMap::new();
        entries.insert(
            "principal.json".to_owned(),
            json!({ "entry_kind": "record", "oid": self.principal_actor_oid }),
        );
        let retained_entries = entries.clone();
        let next_tree = put_json(&self.repository, manifest_tree(entries));
        let next_base = put_json(
            &self.repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&previous_base),
                &next_tree,
                &[],
                PRINCIPAL_ID,
                "base without inherited authority snapshots",
            ),
        );
        self.repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: Some(&previous_base),
                new_head: &next_base,
                metadata: ReflogMetadata::at(2),
            })
            .unwrap();

        let context_oid = put_json(
            &self.repository,
            context_pack_record(&next_base, &self.policy_oid, &self.grant_oid),
        );
        let output_oid = self
            .repository
            .put_blob(&b"parent-only authority fixture"[..])
            .unwrap()
            .oid;
        let activity_oid = put_json(
            &self.repository,
            activity(&context_oid, &self.grant_oid, &output_oid, "none"),
        );
        let mut proposal_entries = retained_entries;
        proposal_entries.insert(
            "context.json".to_owned(),
            json!({ "entry_kind": "record", "oid": context_oid }),
        );
        proposal_entries.insert(
            "proposal.bin".to_owned(),
            json!({ "entry_kind": "blob", "oid": output_oid }),
        );
        proposal_entries.insert(
            "run.json".to_owned(),
            json!({ "entry_kind": "record", "oid": activity_oid }),
        );
        let proposal_tree_oid = put_json(&self.repository, manifest_tree(proposal_entries));
        let candidate_oid = put_json(
            &self.repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&next_base),
                &proposal_tree_oid,
                std::slice::from_ref(&activity_oid),
                AGENT_ID,
                "proposal selecting parent-only authority",
            ),
        );

        self.base_commit_oid = next_base;
        self.base_tree_oid = next_tree;
        self.context_oid = context_oid;
        self.activity_oid = activity_oid;
        self.proposal_tree_oid = proposal_tree_oid;
        self.output_oid = output_oid;
        self.nested_output_leaf_oid = None;
        self.new_head = candidate_oid;
    }

    fn install_claim_output_referencing_historical_activity(&mut self) {
        let historical_activity_oid = self.activity_oid.clone();
        let previous_base = self.base_commit_oid.clone();
        let mut base_entries = JsonMap::new();
        for (name, oid) in [
            ("actor.json", self.actor_oid.as_str()),
            ("grant.json", self.grant_oid.as_str()),
            ("policy.json", self.policy_oid.as_str()),
            ("principal.json", self.principal_actor_oid.as_str()),
            ("history-run.json", historical_activity_oid.as_str()),
        ] {
            base_entries.insert(
                name.to_owned(),
                json!({ "entry_kind": "record", "oid": oid }),
            );
        }
        let retained_base_entries = base_entries.clone();
        let next_tree = put_json(&self.repository, manifest_tree(base_entries));
        let next_base = put_json(
            &self.repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&previous_base),
                &next_tree,
                &[],
                PRINCIPAL_ID,
                "base with historical AI run",
            ),
        );
        self.repository
            .update_ref(RefUpdate {
                ref_name: BASE_REF,
                expected_head: Some(&previous_base),
                new_head: &next_base,
                metadata: ReflogMetadata::at(2),
            })
            .unwrap();

        let context_oid = put_json(
            &self.repository,
            context_pack_record(&next_base, &self.policy_oid, &self.grant_oid),
        );
        let mut claim = claim_record(AGENT_ID);
        claim["payload"]["ai_run_ref"] = json!(historical_activity_oid);
        let claim_oid = put_json(&self.repository, claim);
        let mut current_activity = activity(&context_oid, &self.grant_oid, &claim_oid, "none");
        current_activity["payload"]["ai_run"]["requested_capabilities"] =
            json!(["propose_branch", "read_context", "submit_claim"]);
        let current_activity_oid = put_json(&self.repository, current_activity);

        let mut proposal_entries = retained_base_entries;
        proposal_entries.insert(
            "claim.json".to_owned(),
            json!({ "entry_kind": "record", "oid": claim_oid }),
        );
        proposal_entries.insert(
            "context.json".to_owned(),
            json!({ "entry_kind": "record", "oid": context_oid }),
        );
        proposal_entries.insert(
            "run.json".to_owned(),
            json!({ "entry_kind": "record", "oid": current_activity_oid }),
        );
        let proposal_tree_oid = put_json(&self.repository, manifest_tree(proposal_entries));
        let candidate_oid = put_json(
            &self.repository,
            commit(
                "checkpoint",
                std::slice::from_ref(&next_base),
                &proposal_tree_oid,
                std::slice::from_ref(&current_activity_oid),
                AGENT_ID,
                "claim with historical AI run provenance",
            ),
        );

        self.base_commit_oid = next_base;
        self.base_tree_oid = next_tree;
        self.context_oid = context_oid;
        self.activity_oid = current_activity_oid;
        self.proposal_tree_oid = proposal_tree_oid;
        self.output_oid = claim_oid;
        self.nested_output_leaf_oid = None;
        self.new_head = candidate_oid;
    }

    fn object_byte_len(&self, oid: &str) -> u64 {
        self.repository
            .objects()
            .get_verified(oid)
            .unwrap()
            .unwrap()
            .byte_len()
    }

    fn nested_output_accounted_bytes(&self) -> u64 {
        let leaf = self
            .nested_output_leaf_oid
            .as_deref()
            .expect("scenario must contain a nested output leaf");
        self.object_byte_len(&self.proposal_tree_oid)
            + self.object_byte_len(&self.output_oid)
            + self.object_byte_len(leaf)
    }

    fn selected_input_analysis_accounted_bytes(&self) -> u64 {
        self.object_byte_len(&self.proposal_tree_oid) + self.object_byte_len(&self.output_oid)
    }
}

fn capability_strings(
    present: bool,
    include_analyze: bool,
    include_submit_claim: bool,
) -> Vec<&'static str> {
    if !present {
        return Vec::new();
    }
    let mut capabilities = Vec::new();
    if include_analyze {
        capabilities.push("analyze");
    }
    capabilities.extend(["propose_branch", "read_context"]);
    if include_submit_claim {
        capabilities.push("submit_claim");
    }
    capabilities
}

fn delegation_grant(
    entity_id: &str,
    capabilities: &[&str],
    writable_prefix: &str,
    resource_selector: &str,
    max_output_bytes: i64,
    expires_at: &str,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "delegation_grant",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": PRINCIPAL_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "principal_ref": PRINCIPAL_ID,
            "delegate_ref": AGENT_ID,
            "project_ref": PROJECT_ID,
            "purpose": "Publish one bounded AI proposal.",
            "capabilities": capabilities,
            "resource_selectors": [resource_selector],
            "writable_ref_prefixes": [writable_prefix],
            "data_classes": ["internal"],
            "allowed_egress": [],
            "may_delegate": false,
            "max_child_depth": 0,
            "max_output_bytes": max_output_bytes,
            "required_human_gates": ["before_decision_ref", "before_release_ref"],
            "expires_at": expires_at
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

fn context_pack_record(base_commit: &str, policy_oid: &str, grant_oid: &str) -> JsonValue {
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
            "base_commit": base_commit,
            "base_ref_name": BASE_REF,
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

fn subject_record() -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "subject",
        "entity_id": "urn:uuid:00000000-0000-4000-8000-000000000080",
        "recorded_at": RECORDED_AT,
        "asserted_by": PRINCIPAL_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "subject_kind": "hybrid",
            "label": "Retained base subject",
            "relation_refs": [],
            "spatial_frame_refs": []
        },
        "extensions": {}
    })
}

fn claim_record(asserted_by: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "claim",
        "entity_id": "urn:uuid:00000000-0000-4000-8000-000000000070",
        "recorded_at": RECORDED_AT,
        "asserted_by": asserted_by,
        "origin": "inferred",
        "source_refs": [],
        "payload": {
            "claim_kind": "interpretation",
            "epistemic_class": "suggested",
            "subject_refs": [PROJECT_ID],
            "predicate": "fixture_interpretation",
            "value_text": "A bounded AI-generated fixture claim.",
            "evidence_refs": []
        },
        "extensions": {}
    })
}

fn analysis_result_record(dependency_oid: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "analysis_result",
        "entity_id": "urn:uuid:00000000-0000-4000-8000-000000000071",
        "recorded_at": RECORDED_AT,
        "asserted_by": AGENT_ID,
        "origin": "inferred",
        "source_refs": [],
        "payload": {
            "analysis_kind": "authorization_fixture",
            "comparison_kind": "revision",
            "inputs": [{ "role": "candidate", "ref": dependency_oid }],
            "adapter": {
                "id": "authorization-fixture-adapter",
                "version": "1",
                "implementation_digest": dependency_oid,
                "configuration_digest": dependency_oid,
                "determinism": "deterministic"
            },
            "status": "succeeded",
            "comparability": "comparable",
            "reason_codes": [],
            "derived_blob_refs": [],
            "warnings": [],
            "limitations": []
        },
        "extensions": {}
    })
}

fn authority_actor_record() -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "actor",
        "entity_id": OTHER_ACTOR_ID,
        "recorded_at": RECORDED_AT,
        "asserted_by": AGENT_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "actor_kind": "human",
            "display_name": "Unauthorized authority output"
        },
        "extensions": {}
    })
}

fn tombstone_record(target_oid: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "tombstone",
        "entity_id": "urn:uuid:00000000-0000-4000-8000-000000000072",
        "recorded_at": RECORDED_AT,
        "asserted_by": AGENT_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "target_ref": target_oid,
            "erasure_kind": "withheld",
            "reason_code": "project_policy",
            "acted_at": RECORDED_AT
        },
        "extensions": {}
    })
}

fn activity(
    context_oid: &str,
    grant_oid: &str,
    output_oid: &str,
    side_effect_class: &str,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "activity",
        "entity_id": ACTIVITY_ENTITY_ID,
        "valid_time": {
            "kind": "instant",
            "at": RECORDED_AT
        },
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
            "input_refs": [
                { "role": "context", "oid": context_oid }
            ],
            "output_refs": [
                { "role": "proposal", "oid": output_oid }
            ],
            "reversibility": "reversible",
            "side_effect_class": side_effect_class,
            "ai_run": {
                "agent_ref": AGENT_ID,
                "responsible_principal_ref": PRINCIPAL_ID,
                "context_pack_ref": context_oid,
                "delegation_grant_ref": grant_oid,
                "requested_capabilities": ["propose_branch", "read_context"],
                "required_human_gates": ["before_decision_ref", "before_release_ref"],
                "status": "proposal_ready"
            }
        },
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

fn assert_failure_unchanged(mut scenario: Scenario, expected_code: &str) -> RepositoryError {
    let refs_before = scenario.repository.refs().snapshot().unwrap();
    let reflog_before = scenario.repository.refs().reflog().unwrap();
    let error = scenario
        .publish()
        .expect_err("authorization must reject the scenario");
    assert_eq!(error.code(), expected_code, "unexpected error: {error}");
    assert_eq!(scenario.repository.refs().snapshot().unwrap(), refs_before);
    assert_eq!(scenario.repository.refs().reflog().unwrap(), reflog_before);
    error
}

fn assert_preflight_failure_unchanged(
    mut scenario: Scenario,
    expected_code: &str,
) -> RepositoryError {
    let refs_before = scenario.repository.refs().snapshot().unwrap();
    let reflog_before = scenario.repository.refs().reflog().unwrap();
    let error = scenario
        .preflight(AiSideEffectClass::None)
        .expect_err("preflight must reject the scenario");
    assert_eq!(error.code(), expected_code, "unexpected error: {error}");
    assert_eq!(scenario.repository.refs().snapshot().unwrap(), refs_before);
    assert_eq!(scenario.repository.refs().reflog().unwrap(), reflog_before);
    error
}

fn assert_preflighted_failure_unchanged(
    mut scenario: Scenario,
    decision: AiPreflightDecision,
    expected_code: &str,
) -> RepositoryError {
    let refs_before = scenario.repository.refs().snapshot().unwrap();
    let reflog_before = scenario.repository.refs().reflog().unwrap();
    let error = scenario
        .publish_preflighted(decision)
        .expect_err("preflighted publication must reject the scenario");
    assert_eq!(error.code(), expected_code, "unexpected error: {error}");
    assert_eq!(scenario.repository.refs().snapshot().unwrap(), refs_before);
    assert_eq!(scenario.repository.refs().reflog().unwrap(), reflog_before);
    error
}

#[test]
fn explicit_allow_publishes_proposal_with_atomic_base_guard_and_trusted_reflog_actor() {
    let mut scenario = ScenarioBuilder::default().build();
    let base = scenario.base_commit_oid.clone();
    let proposal = scenario.new_head.clone();
    let actor = scenario.actor_oid.clone();
    let activity = scenario.activity_oid.clone();
    let context = scenario.context_oid.clone();
    let grant = scenario.grant_oid.clone();
    let policy = scenario.policy_oid.clone();

    let decision = scenario.publish().unwrap();

    assert_eq!(decision.actor_record_oid, actor);
    assert_eq!(decision.activity_oid, activity);
    assert_eq!(decision.context_pack_oid, context);
    assert_eq!(decision.delegation_grant_oid, grant);
    assert_eq!(decision.policy_oid, policy);
    assert_eq!(
        decision.effective_capabilities,
        vec![AiCapability::ReadContext, AiCapability::ProposeBranch]
    );
    assert_eq!(decision.reflog.ref_name, PROPOSAL_REF);
    assert_eq!(decision.reflog.old_head, None);
    assert_eq!(decision.reflog.new_head, proposal);
    assert_eq!(decision.reflog.actor.as_deref(), Some(AGENT_ID));
    assert_eq!(decision.reflog.message.as_deref(), Some(MESSAGE));
    assert_eq!(
        decision.reflog.occurred_at_unix_nanos,
        i64::try_from(FIXED_NOW_NANOS).unwrap()
    );
    assert_eq!(
        scenario
            .repository
            .refs()
            .get(BASE_REF)
            .unwrap()
            .unwrap()
            .head,
        base
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
    assert_eq!(
        scenario
            .repository
            .refs()
            .reflog_for_ref(PROPOSAL_REF)
            .unwrap(),
        vec![decision.reflog]
    );
}

#[test]
fn preflight_is_read_only_and_seals_exact_authority_for_publication() {
    let mut scenario = ScenarioBuilder::default().build();
    let refs_before = scenario.repository.refs().snapshot().unwrap();
    let reflog_before = scenario.repository.refs().reflog().unwrap();

    let decision = scenario.preflight(AiSideEffectClass::None).unwrap();

    assert_eq!(scenario.repository.refs().snapshot().unwrap(), refs_before);
    assert_eq!(scenario.repository.refs().reflog().unwrap(), reflog_before);
    assert_eq!(decision.actor_id(), AGENT_ID);
    assert_eq!(decision.project_id(), PROJECT_ID);
    assert_eq!(decision.principal_id(), PRINCIPAL_ID);
    assert_eq!(decision.base_ref_name(), BASE_REF);
    assert_eq!(decision.base_head(), scenario.base_commit_oid);
    assert_eq!(decision.target_ref_name(), PROPOSAL_REF);
    assert_eq!(decision.expected_target_head(), None);
    assert_eq!(decision.side_effect_class(), AiSideEffectClass::None);
    assert_eq!(decision.context_pack_oid(), scenario.context_oid);
    assert_eq!(decision.delegation_grant_oid(), scenario.grant_oid);
    assert_eq!(decision.policy_oid(), scenario.policy_oid);
    assert_eq!(
        decision.exact_capabilities(),
        &[AiCapability::ReadContext, AiCapability::ProposeBranch]
    );
    assert_eq!(decision.evaluated_at_unix_nanos(), FIXED_NOW_NANOS);
    assert!(decision.grant_expires_at_unix_nanos() > FIXED_NOW_NANOS);

    let publication = scenario.publish_preflighted(decision).unwrap();
    assert_eq!(publication.reflog.ref_name, PROPOSAL_REF);
    assert_eq!(publication.reflog.new_head, scenario.new_head);
    assert_eq!(publication.reflog.actor.as_deref(), Some(AGENT_ID));
}

#[test]
fn preflight_checks_every_static_capability_ceiling_and_policy() {
    for missing in ["actor", "grant", "runtime", "policy"] {
        let mut builder = ScenarioBuilder::default();
        match missing {
            "actor" => builder.actor_has_capability = false,
            "grant" => builder.grant_has_capability = false,
            "runtime" => builder.runtime_has_capability = false,
            "policy" => builder.policy_mode = PolicyMode::DefaultDeny,
            _ => unreachable!(),
        }
        let error = assert_preflight_failure_unchanged(builder.build(), "authorization_denied");
        if missing != "policy" {
            assert!(
                error
                    .to_string()
                    .contains("effective capability intersection"),
                "{missing}: {error}"
            );
        }
    }

    let mut no_propose = ScenarioBuilder::default().build();
    no_propose.authorized_capabilities = vec![AiCapability::ReadContext];
    let error = assert_preflight_failure_unchanged(no_propose, "authorization_denied");
    assert!(error.to_string().contains("must include propose_branch"));
}

#[test]
fn preflight_checks_base_then_exact_target_without_mutation() {
    let mut stale = ScenarioBuilder::default().build();
    stale.advance_base_ref();
    assert_preflight_failure_unchanged(stale, "stale_base");

    let mut occupied = ScenarioBuilder::default().build();
    occupied.occupy_target_ref();
    assert_preflight_failure_unchanged(occupied, "ref_conflict");

    let mut invalid_expected = ScenarioBuilder::default().build();
    invalid_expected.expected_head = Some(invalid_expected.base_blob_oid.clone());
    assert_preflight_failure_unchanged(invalid_expected, "oid_mismatch");

    let mut unauthorized_and_occupied = ScenarioBuilder {
        policy_mode: PolicyMode::DefaultDeny,
        ..ScenarioBuilder::default()
    }
    .build();
    unauthorized_and_occupied.occupy_target_ref();
    assert_preflight_failure_unchanged(unauthorized_and_occupied, "authorization_denied");
}

#[test]
fn preflighted_publication_rejects_side_effect_and_capability_drift() {
    for (activity_side_effect, authorized_side_effect) in [
        ("project_internal", AiSideEffectClass::None),
        ("none", AiSideEffectClass::ProjectInternal),
    ] {
        let mut scenario = ScenarioBuilder {
            side_effect_class: activity_side_effect,
            ..ScenarioBuilder::default()
        }
        .build();
        let decision = scenario.preflight(authorized_side_effect).unwrap();
        let error =
            assert_preflighted_failure_unchanged(scenario, decision, "authorization_denied");
        assert!(error.to_string().contains("side effect class"));
    }

    for requested_capabilities in [
        vec!["propose_branch"],
        vec!["analyze", "propose_branch", "read_context"],
    ] {
        let mut scenario = ScenarioBuilder {
            requested_capabilities,
            ..ScenarioBuilder::default()
        }
        .build();
        let decision = scenario.preflight(AiSideEffectClass::None).unwrap();
        let error =
            assert_preflighted_failure_unchanged(scenario, decision, "authorization_denied");
        assert!(error.to_string().contains("do not exactly match"));
    }
}

#[test]
fn preflighted_publication_rechecks_base_target_expiry_and_clock() {
    let mut stale = ScenarioBuilder::default().build();
    let stale_decision = stale.preflight(AiSideEffectClass::None).unwrap();
    stale.advance_base_ref();
    assert_preflighted_failure_unchanged(stale, stale_decision, "stale_base");

    let mut occupied = ScenarioBuilder::default().build();
    let occupied_decision = occupied.preflight(AiSideEffectClass::None).unwrap();
    occupied.occupy_target_ref();
    assert_preflighted_failure_unchanged(occupied, occupied_decision, "ref_conflict");

    let mut expires = ScenarioBuilder {
        expires_at: TRANSACTION_EXPIRES_AT,
        ..ScenarioBuilder::default()
    }
    .build();
    expires.clock = TestClock::sequence([Ok(FIXED_NOW_NANOS), Ok(TRANSACTION_EXPIRES_AT_NANOS)]);
    let expires_decision = expires.preflight(AiSideEffectClass::None).unwrap();
    let error =
        assert_preflighted_failure_unchanged(expires, expires_decision, "authorization_denied");
    assert!(error.to_string().contains("expired"));

    let mut backwards = ScenarioBuilder::default().build();
    backwards.clock = TestClock::sequence([Ok(FIXED_NOW_NANOS), Ok(EXPIRED_AT_NANOS)]);
    let backwards_decision = backwards.preflight(AiSideEffectClass::None).unwrap();
    let error =
        assert_preflighted_failure_unchanged(backwards, backwards_decision, "storage_error");
    assert!(error.to_string().contains("since preflight"));

    let mut transaction_expiry = ScenarioBuilder {
        expires_at: TRANSACTION_EXPIRES_AT,
        ..ScenarioBuilder::default()
    }
    .build();
    transaction_expiry.clock = TestClock::sequence([
        Ok(FIXED_NOW_NANOS),
        Ok(FIXED_NOW_NANOS),
        Ok(TRANSACTION_EXPIRES_AT_NANOS),
    ]);
    let transaction_decision = transaction_expiry
        .preflight(AiSideEffectClass::None)
        .unwrap();
    let error = assert_preflighted_failure_unchanged(
        transaction_expiry,
        transaction_decision,
        "authorization_denied",
    );
    assert!(error.to_string().contains("publication transaction"));
}

#[test]
fn rebuilt_candidate_snapshot_retains_authority_and_general_base_objects() {
    let mut scenario = ScenarioBuilder::default().build();
    assert_ne!(scenario.proposal_tree_oid, scenario.base_tree_oid);

    let candidate_tree = scenario
        .repository
        .objects()
        .get_verified(&scenario.proposal_tree_oid)
        .unwrap()
        .unwrap();
    let entries = candidate_tree.structured().unwrap().get("entries").unwrap();
    for (name, expected_oid) in [
        ("actor.json", scenario.actor_oid.as_str()),
        ("principal.json", scenario.principal_actor_oid.as_str()),
        ("grant.json", scenario.grant_oid.as_str()),
        ("policy.json", scenario.policy_oid.as_str()),
        ("evidence.bin", scenario.base_blob_oid.as_str()),
        ("subject.json", scenario.base_record_oid.as_str()),
    ] {
        assert_eq!(
            entries
                .get(name)
                .and_then(|entry| entry.get("oid"))
                .and_then(|oid| oid.as_str()),
            Some(expected_oid),
            "candidate snapshot did not retain {name}"
        );
    }

    scenario
        .publish()
        .expect("a reconstructed snapshot retaining every base object must publish");
}

#[test]
fn candidate_snapshot_cannot_drop_any_non_tree_base_object() {
    for omitted in [
        CandidateBaseOmission::Actor,
        CandidateBaseOmission::Principal,
        CandidateBaseOmission::Grant,
        CandidateBaseOmission::Policy,
        CandidateBaseOmission::Blob,
        CandidateBaseOmission::Record,
    ] {
        let scenario = ScenarioBuilder {
            omitted_candidate_base_entry: Some(omitted),
            ..ScenarioBuilder::default()
        }
        .build();

        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error
                .to_string()
                .contains("does not retain base snapshot object"),
            "{omitted:?}: {error}"
        );
    }
}

#[test]
fn policy_default_deny_rejects_without_mutating_refs_or_reflog() {
    let scenario = ScenarioBuilder {
        policy_mode: PolicyMode::DefaultDeny,
        ..ScenarioBuilder::default()
    }
    .build();
    assert_failure_unchanged(scenario, "authorization_denied");
}

#[test]
fn actor_grant_and_runtime_capability_are_each_required() {
    for missing in ["actor", "grant", "runtime"] {
        let mut builder = ScenarioBuilder::default();
        match missing {
            "actor" => builder.actor_has_capability = false,
            "grant" => builder.grant_has_capability = false,
            "runtime" => builder.runtime_has_capability = false,
            _ => unreachable!(),
        }
        let error = assert_failure_unchanged(builder.build(), "authorization_denied");
        assert!(
            error
                .to_string()
                .contains("effective capability intersection"),
            "{missing}: {error}"
        );
    }
}

#[test]
fn expired_grant_is_rejected_without_mutating_refs_or_reflog() {
    let scenario = ScenarioBuilder {
        expires_at: EXPIRED_AT,
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(error.to_string().contains("expired"));

    let mut exact_boundary = ScenarioBuilder {
        expires_at: EXPIRED_AT,
        ..ScenarioBuilder::default()
    }
    .build();
    exact_boundary.clock = TestClock::fixed(Ok(EXPIRED_AT_NANOS));
    let error = assert_failure_unchanged(exact_boundary, "authorization_denied");
    assert!(error.to_string().contains("expired"));
}

#[test]
fn writable_prefix_uses_path_segment_boundaries() {
    let scenario = ScenarioBuilder {
        writable_prefix: format!("{PROPOSAL_ROOT}/run-1"),
        ref_name: format!("{PROPOSAL_ROOT}/run-10"),
        ..ScenarioBuilder::default()
    }
    .build();
    assert_failure_unchanged(scenario, "authorization_denied");
}

#[test]
fn decision_and_release_refs_always_require_a_human_gate() {
    for ref_name in ["decision/ai-attempt", "release/ai-attempt"] {
        let mut scenario = ScenarioBuilder::default().build();
        scenario.ref_name = ref_name.to_owned();
        assert_failure_unchanged(scenario, "human_gate_required");
    }
}

#[test]
fn stale_base_and_target_ref_conflict_are_distinct_and_atomic() {
    let mut stale = ScenarioBuilder::default().build();
    stale.advance_base_ref();
    assert_failure_unchanged(stale, "stale_base");

    let mut conflict = ScenarioBuilder::default().build();
    conflict.occupy_target_ref();
    assert_failure_unchanged(conflict, "ref_conflict");
}

#[test]
fn commit_activity_and_activity_context_crosslinks_are_enforced() {
    for mismatch in [
        CrosslinkMismatch::CommitToActivity,
        CrosslinkMismatch::ActivityToContextGrant,
    ] {
        let scenario = ScenarioBuilder {
            crosslink_mismatch: mismatch,
            ..ScenarioBuilder::default()
        }
        .build();
        assert_failure_unchanged(scenario, "authorization_denied");
    }
}

#[test]
fn trusted_actor_project_principal_and_base_ref_must_match_the_immutable_chain() {
    let mut wrong_actor = ScenarioBuilder::default().build();
    wrong_actor.authenticated_actor_id = OTHER_ACTOR_ID.to_owned();
    assert_failure_unchanged(wrong_actor, "authorization_denied");

    let mut wrong_project = ScenarioBuilder::default().build();
    wrong_project.authorized_project_id = OTHER_PROJECT_ID.to_owned();
    assert_failure_unchanged(wrong_project, "authorization_denied");

    let mut wrong_principal = ScenarioBuilder::default().build();
    wrong_principal.authorized_principal_id = OTHER_PRINCIPAL_ID.to_owned();
    assert_failure_unchanged(wrong_principal, "authorization_denied");

    let mut wrong_base_ref = ScenarioBuilder::default().build();
    wrong_base_ref.authorized_base_ref = "decision/other".to_owned();
    assert_failure_unchanged(wrong_base_ref, "authorization_denied");
}

#[test]
fn policy_uses_most_restrictive_matching_effect_and_conditional_allow_fails_closed() {
    for (mode, expected_code) in [
        (PolicyMode::DenyOverAllow, "authorization_denied"),
        (PolicyMode::HumanGateOverAllow, "human_gate_required"),
        (PolicyMode::ConditionalAllow, "authorization_denied"),
    ] {
        let scenario = ScenarioBuilder {
            policy_mode: mode,
            ..ScenarioBuilder::default()
        }
        .build();
        assert_failure_unchanged(scenario, expected_code);
    }
}

#[test]
fn grant_project_resource_selector_uses_path_segment_boundaries() {
    let near_sibling = &PROJECT_ID[..PROJECT_ID.len() - 1];
    let scenario = ScenarioBuilder {
        resource_selector: format!("project/{near_sibling}/**"),
        ..ScenarioBuilder::default()
    }
    .build();
    assert_failure_unchanged(scenario, "authorization_denied");
}

#[test]
fn external_and_physical_activity_side_effects_are_rejected() {
    for side_effect_class in ["external", "physical"] {
        let scenario = ScenarioBuilder {
            side_effect_class,
            ..ScenarioBuilder::default()
        }
        .build();
        assert_failure_unchanged(scenario, "authorization_denied");
    }
}

#[test]
fn grant_output_byte_limit_is_enforced() {
    let scenario = ScenarioBuilder {
        max_output_bytes: 1,
        ..ScenarioBuilder::default()
    }
    .build();
    assert_failure_unchanged(scenario, "authorization_denied");
}

#[test]
fn every_authority_object_must_be_bound_by_the_current_base_snapshot() {
    for omitted in [BaseBinding::Actor, BaseBinding::Grant, BaseBinding::Policy] {
        let scenario = ScenarioBuilder {
            omitted_base_binding: Some(omitted),
            ..ScenarioBuilder::default()
        }
        .build();
        assert_failure_unchanged(scenario, "authorization_denied");
    }
}

#[test]
fn trusted_clock_failure_is_storage_error_and_leaves_refs_unchanged() {
    let mut scenario = ScenarioBuilder::default().build();
    scenario.clock = TestClock::fixed(Err("trusted clock unavailable".to_owned()));
    let error = assert_failure_unchanged(scenario, "storage_error");
    assert!(error.to_string().contains("trusted clock unavailable"));
}

#[test]
fn authority_snapshots_must_be_in_the_current_base_not_only_its_parent() {
    let mut scenario = ScenarioBuilder::default().build();
    scenario.rebase_context_while_authority_objects_remain_only_in_parent();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(
        error
            .to_string()
            .contains("current ContextPack base snapshot")
    );
}

#[test]
fn candidate_snapshot_rejects_every_undeclared_object_kind() {
    for (mode, label) in [
        (CandidateMode::UndeclaredBlob, "Blob"),
        (CandidateMode::UndeclaredRecord, "Record"),
        (CandidateMode::UndeclaredTree, "Tree"),
    ] {
        let scenario = ScenarioBuilder {
            candidate_mode: mode,
            max_output_bytes: 10_000,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(error.to_string().contains("undeclared"), "{label}: {error}");
    }
}

#[test]
fn every_activity_output_must_be_bound_by_the_candidate_snapshot() {
    let scenario = ScenarioBuilder {
        candidate_mode: CandidateMode::MissingDeclaredOutput,
        ..ScenarioBuilder::default()
    }
    .build();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(
        error
            .to_string()
            .contains("not bound by the candidate snapshot")
    );
}

#[test]
fn nested_output_tree_quota_counts_the_transitive_closure_with_oid_deduplication() {
    let over_limit = ScenarioBuilder {
        candidate_mode: CandidateMode::NestedTreeOutput,
        max_output_bytes: 1,
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(over_limit, "authorization_denied");
    assert!(error.to_string().contains("output closure totals"));

    let mut limit = 999_i64;
    let mut exact_boundary = loop {
        let scenario = ScenarioBuilder {
            candidate_mode: CandidateMode::NestedTreeOutput,
            max_output_bytes: limit,
            ..ScenarioBuilder::default()
        }
        .build();
        let measured = scenario.nested_output_accounted_bytes();
        if measured == limit as u64 {
            break scenario;
        }
        limit = i64::try_from(measured).unwrap();
    };
    let expected_head = exact_boundary.new_head.clone();
    let decision = exact_boundary
        .publish()
        .expect("the exact deduplicated output-byte boundary must be accepted");
    assert_eq!(decision.reflog.new_head, expected_head);
}

#[test]
fn ai_outputs_cannot_assert_human_claims_or_introduce_control_records_or_commits() {
    for (mode, expected_message) in [
        (
            CandidateMode::HumanAssertedClaimOutput,
            "asserted by the authenticated agent",
        ),
        (CandidateMode::AuthorityRecordOutput, "Record type actor"),
        (CandidateMode::TombstoneOutput, "Record type tombstone"),
        (
            CandidateMode::NestedCommitOutput,
            "cannot introduce nested Commit",
        ),
    ] {
        let scenario = ScenarioBuilder {
            candidate_mode: mode,
            max_output_bytes: 10_000,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error.to_string().contains(expected_message),
            "{mode:?}: {error}"
        );
    }
}

#[test]
fn activity_capabilities_must_match_authority_and_cover_derived_outputs() {
    let mismatch = ScenarioBuilder {
        requested_capabilities: vec!["propose_branch"],
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(mismatch, "authorization_denied");
    assert!(error.to_string().contains("do not exactly match"));

    for (mode, capability) in [
        (CandidateMode::AnalysisResultOutput, "analyze"),
        (CandidateMode::AgentClaimOutput, "submit_claim"),
    ] {
        let scenario = ScenarioBuilder {
            candidate_mode: mode,
            max_output_bytes: 10_000,
            ..ScenarioBuilder::default()
        }
        .build();
        let error = assert_failure_unchanged(scenario, "authorization_denied");
        assert!(
            error
                .to_string()
                .contains(&format!("required capability {capability}")),
            "{mode:?}: {error}"
        );
    }
}

#[test]
fn ai_activity_requires_an_explicit_safe_side_effect_class() {
    let scenario = ScenarioBuilder {
        include_side_effect_class: false,
        ..ScenarioBuilder::default()
    }
    .build();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(
        error
            .to_string()
            .contains("explicit none or project_internal")
    );
}

#[test]
fn delegation_grant_recorded_in_the_future_is_not_active() {
    let scenario = ScenarioBuilder {
        grant_recorded_at: FUTURE_RECORDED_AT,
        ..ScenarioBuilder::default()
    }
    .build();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(error.to_string().contains("not active yet"));
}

#[test]
fn unsupported_policy_selector_fails_closed_even_when_default_effect_allows() {
    let scenario = ScenarioBuilder {
        policy_mode: PolicyMode::UnsupportedSelectorDefaultAllow,
        ..ScenarioBuilder::default()
    }
    .build();

    let error = assert_failure_unchanged(scenario, "authorization_denied");
    assert!(error.to_string().contains("unsupported"));
}

#[test]
fn transaction_rechecks_expiry_and_rejects_a_clock_that_moves_backwards() {
    let mut expires_while_waiting = ScenarioBuilder {
        expires_at: TRANSACTION_EXPIRES_AT,
        ..ScenarioBuilder::default()
    }
    .build();
    expires_while_waiting.clock =
        TestClock::sequence([Ok(FIXED_NOW_NANOS), Ok(TRANSACTION_EXPIRES_AT_NANOS)]);
    let error = assert_failure_unchanged(expires_while_waiting, "authorization_denied");
    assert!(error.to_string().contains("publication transaction"));

    let mut moves_backwards = ScenarioBuilder::default().build();
    moves_backwards.clock = TestClock::sequence([Ok(FIXED_NOW_NANOS), Ok(EXPIRED_AT_NANOS)]);
    let error = assert_failure_unchanged(moves_backwards, "storage_error");
    assert!(error.to_string().contains("moved backwards"));
}

#[test]
fn selected_context_input_reused_by_analysis_is_not_charged_as_new_output() {
    let requested_capabilities = vec!["analyze", "propose_branch", "read_context"];
    let mut limit = 9_999_i64;
    let mut scenario = loop {
        let scenario = ScenarioBuilder {
            include_analyze_capability: true,
            requested_capabilities: requested_capabilities.clone(),
            candidate_mode: CandidateMode::AnalysisOfSelectedBaseInput,
            max_output_bytes: limit,
            ..ScenarioBuilder::default()
        }
        .build();
        let measured = scenario.selected_input_analysis_accounted_bytes();
        if measured == limit as u64 {
            break scenario;
        }
        limit = i64::try_from(measured).unwrap();
    };
    assert!(scenario.object_byte_len(&scenario.base_commit_oid) > 0);

    let decision = scenario
        .publish()
        .expect("authorized selected input bytes must not be charged as produced output");
    assert_eq!(
        decision.effective_capabilities,
        vec![
            AiCapability::ReadContext,
            AiCapability::Analyze,
            AiCapability::ProposeBranch,
        ]
    );

    let copied_input = ScenarioBuilder {
        include_analyze_capability: true,
        requested_capabilities,
        candidate_mode: CandidateMode::AnalysisOfSelectedBaseInputCopiedIntoSnapshot,
        max_output_bytes: 10_000,
        ..ScenarioBuilder::default()
    }
    .build();
    let error = assert_failure_unchanged(copied_input, "authorization_denied");
    assert!(error.to_string().contains("undeclared"));
}

#[test]
fn explicit_output_root_cannot_relabel_a_base_bound_principal_policy_as_ai_output() {
    let scenario = ScenarioBuilder {
        candidate_mode: CandidateMode::CurrentBasePolicyOutput,
        max_output_bytes: 10_000,
        ..ScenarioBuilder::default()
    }
    .build();

    assert_failure_unchanged(scenario, "authorization_denied");
}

#[test]
fn produced_claim_uses_current_activity_provenance_and_must_omit_historical_ai_run_ref() {
    let requested_capabilities = vec!["propose_branch", "read_context", "submit_claim"];
    let mut historical_reference = ScenarioBuilder {
        include_submit_claim_capability: true,
        requested_capabilities: requested_capabilities.clone(),
        max_output_bytes: 10_000,
        ..ScenarioBuilder::default()
    }
    .build();
    historical_reference.install_claim_output_referencing_historical_activity();

    let error = assert_failure_unchanged(historical_reference, "authorization_denied");
    assert!(error.to_string().contains("must omit ai_run_ref"));

    let mut current_activity_only = ScenarioBuilder {
        include_submit_claim_capability: true,
        requested_capabilities,
        candidate_mode: CandidateMode::AgentClaimOutput,
        max_output_bytes: 10_000,
        ..ScenarioBuilder::default()
    }
    .build();
    let decision = current_activity_only
        .publish()
        .expect("an agent Claim without ai_run_ref must use the current Activity output relation");
    assert_eq!(
        decision.effective_capabilities,
        vec![
            AiCapability::ReadContext,
            AiCapability::ProposeBranch,
            AiCapability::SubmitClaim,
        ]
    );
}

#[test]
fn fixed_context_pack_reused_by_analysis_is_not_charged_as_new_output() {
    let requested_capabilities = vec!["analyze", "propose_branch", "read_context"];
    let mut limit = 9_999_i64;
    let mut scenario = loop {
        let scenario = ScenarioBuilder {
            include_analyze_capability: true,
            requested_capabilities: requested_capabilities.clone(),
            candidate_mode: CandidateMode::AnalysisOfFixedContextInput,
            max_output_bytes: limit,
            ..ScenarioBuilder::default()
        }
        .build();
        let measured = scenario.selected_input_analysis_accounted_bytes();
        if measured == limit as u64 {
            break scenario;
        }
        limit = i64::try_from(measured).unwrap();
    };
    assert!(scenario.object_byte_len(&scenario.context_oid) > 0);

    let decision = scenario
        .publish()
        .expect("the fixed ContextPack input bytes must not be charged as produced output");
    assert_eq!(
        decision.effective_capabilities,
        vec![
            AiCapability::ReadContext,
            AiCapability::Analyze,
            AiCapability::ProposeBranch,
        ]
    );
}

#[test]
fn authorization_and_base_preconditions_precede_target_ref_conflicts() {
    let mut stale_and_occupied = ScenarioBuilder::default().build();
    stale_and_occupied.advance_base_ref();
    stale_and_occupied.occupy_target_ref();
    assert_failure_unchanged(stale_and_occupied, "stale_base");

    let mut denied_and_stale = ScenarioBuilder {
        policy_mode: PolicyMode::DefaultDeny,
        ..ScenarioBuilder::default()
    }
    .build();
    denied_and_stale.advance_base_ref();
    assert_failure_unchanged(denied_and_stale, "authorization_denied");

    let mut expired_and_stale = ScenarioBuilder {
        expires_at: EXPIRED_AT,
        ..ScenarioBuilder::default()
    }
    .build();
    expired_and_stale.advance_base_ref();
    let error = assert_failure_unchanged(expired_and_stale, "authorization_denied");
    assert!(error.to_string().contains("expired"));
}

#[test]
fn candidate_commit_must_be_a_checkpoint_with_the_base_as_its_only_parent() {
    for case in [
        "non-checkpoint",
        "no-parent",
        "base-plus-extra",
        "wrong-parent",
    ] {
        let mut scenario = ScenarioBuilder::default().build();
        let base = scenario.base_commit_oid.clone();
        let extra = scenario.create_unrelated_parent();
        match case {
            "non-checkpoint" => {
                scenario.replace_candidate_commit("decision", std::slice::from_ref(&base));
            }
            "no-parent" => scenario.replace_candidate_commit("checkpoint", &[]),
            "base-plus-extra" => {
                scenario.replace_candidate_commit("merge", &[base, extra]);
            }
            "wrong-parent" => {
                scenario.replace_candidate_commit("checkpoint", std::slice::from_ref(&extra));
            }
            _ => unreachable!(),
        }

        let error = assert_failure_unchanged(scenario, "authorization_denied");
        let expected = if matches!(case, "non-checkpoint" | "base-plus-extra") {
            "checkpoint"
        } else {
            "sole parent"
        };
        assert!(error.to_string().contains(expected), "{case}: {error}");
    }
}
