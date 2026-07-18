use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use synapse_publication::{
    ArtifactRole, OutputTarget, PublicProjection, PublicationVisibility, ValueOrigin, verify_bundle,
};

const CORPUS_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/evaluation/publication-comprehension/v1"
);
const CASES: [&str; 2] = ["complete", "incomplete-only"];

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct SchemaIdentity {
    name: String,
    version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Questionnaire {
    schema: SchemaIdentity,
    corpus_version: u32,
    instructions: String,
    questions: Vec<Question>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Question {
    id: String,
    cases: Vec<String>,
    tracks: Vec<String>,
    answer_type: String,
    #[serde(default)]
    accepted_values: Vec<String>,
    critical: bool,
    prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Oracle {
    schema: SchemaIdentity,
    corpus_version: u32,
    cases: BTreeMap<String, OracleCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleCase {
    bundle: String,
    projection_sha256: String,
    answers: BTreeMap<String, OracleAnswer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleAnswer {
    value: Value,
    critical: bool,
    evidence: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivacyCanaries {
    schema: SchemaIdentity,
    corpus_version: u32,
    cases: BTreeMap<String, PrivacyCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivacyCase {
    must_be_absent: Vec<AbsentCanary>,
    must_be_present: Vec<PresentCanary>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbsentCanary {
    label: String,
    value: String,
    scan_base64: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresentCanary {
    label: String,
    value: String,
    paths: Vec<String>,
}

#[test]
fn frozen_bundles_verify_and_match_the_semantic_oracle() {
    let questionnaire: Questionnaire = read_json("questionnaire.json");
    let oracle: Oracle = read_json("oracle.json");
    assert_contract_identity(
        &questionnaire.schema,
        "org.synapsegit.publication-comprehension-questionnaire",
    );
    assert_contract_identity(
        &oracle.schema,
        "org.synapsegit.publication-comprehension-oracle",
    );
    assert_eq!(questionnaire.corpus_version, 1);
    assert_eq!(oracle.corpus_version, 1);
    assert!(!questionnaire.instructions.trim().is_empty());
    assert_eq!(
        oracle.cases.keys().map(String::as_str).collect::<Vec<_>>(),
        CASES
    );

    let mut questions = BTreeMap::<&str, &Question>::new();
    for question in &questionnaire.questions {
        assert!(!question.prompt.trim().is_empty());
        assert!(
            matches!(
                question.answer_type.as_str(),
                "boolean" | "integer" | "enum"
            ),
            "unsupported answer type for {}",
            question.id
        );
        assert!(
            question
                .cases
                .iter()
                .all(|case| CASES.contains(&case.as_str())),
            "question {} names an unknown case",
            question.id
        );
        assert!(
            !question.tracks.is_empty()
                && question
                    .tracks
                    .iter()
                    .all(|track| matches!(track.as_str(), "json" | "html")),
            "question {} names an unknown or empty track",
            question.id
        );
        assert!(
            questions.insert(&question.id, question).is_none(),
            "duplicate question {}",
            question.id
        );
    }

    for case_id in CASES {
        let expected = &oracle.cases[case_id];
        assert_eq!(expected.bundle, format!("bundles/{case_id}"));
        let bundle = corpus_path(&expected.bundle);
        let verified = verify_bundle(&bundle).unwrap();
        assert_eq!(verified.manifest.target, OutputTarget::Synapse);
        assert_eq!(verified.manifest.visibility, PublicationVisibility::Public);
        assert_eq!(
            verified.manifest.projection_sha256,
            expected.projection_sha256
        );

        let projection_bytes = fs::read(bundle.join("projection.json")).unwrap();
        let projection: PublicProjection = serde_json::from_slice(&projection_bytes).unwrap();
        let projection_value: Value = serde_json::from_slice(&projection_bytes).unwrap();
        assert_eq!(
            fs::read(bundle.join("target/public-projection.json")).unwrap(),
            projection_bytes
        );

        let computed = computed_answers(case_id, &projection);
        let applicable = questionnaire
            .questions
            .iter()
            .filter(|question| question.cases.iter().any(|case| case == case_id))
            .map(|question| question.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            expected
                .answers
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            applicable,
            "oracle and questionnaire differ for {case_id}"
        );
        assert_eq!(
            computed.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            applicable,
            "computed answers and questionnaire differ for {case_id}"
        );

        for (question_id, oracle_answer) in &expected.answers {
            let question = questions[question_id.as_str()];
            assert_eq!(oracle_answer.critical, question.critical);
            assert_answer_type(question, &oracle_answer.value);
            assert_eq!(
                &oracle_answer.value, &computed[question_id],
                "semantic oracle drift for {case_id}/{question_id}"
            );
            assert!(!oracle_answer.evidence.is_empty());
            for evidence in &oracle_answer.evidence {
                let pointer = evidence
                    .strip_prefix("projection.json#")
                    .unwrap_or_else(|| panic!("unsupported evidence path {evidence:?}"));
                assert!(
                    projection_value.pointer(pointer).is_some(),
                    "missing oracle evidence {evidence:?} for {case_id}/{question_id}"
                );
            }
        }
    }
}

#[test]
fn privacy_canaries_are_absent_and_public_controls_are_present() {
    let canaries: PrivacyCanaries = read_json("privacy-canaries.json");
    assert_contract_identity(
        &canaries.schema,
        "org.synapsegit.publication-comprehension-privacy-canaries",
    );
    assert_eq!(canaries.corpus_version, 1);
    assert_eq!(
        canaries
            .cases
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        CASES
    );

    for case_id in CASES {
        let bundle = corpus_path(format!("bundles/{case_id}"));
        let files = collect_regular_files(&bundle);
        for canary in &canaries.cases[case_id].must_be_absent {
            assert!(
                !canary.value.is_empty(),
                "empty absent canary {}",
                canary.label
            );
            let literal = canary.value.as_bytes();
            for path in &files {
                let bytes = fs::read(path).unwrap();
                assert!(
                    !contains_bytes(&bytes, literal),
                    "bundle {case_id} leaked {} in {}",
                    canary.label,
                    path.display()
                );
                if canary.scan_base64 {
                    let encoded = base64(literal);
                    assert!(
                        !contains_bytes(&bytes, encoded.as_bytes()),
                        "bundle {case_id} leaked Base64 {} in {}",
                        canary.label,
                        path.display()
                    );
                }
            }
        }

        for canary in &canaries.cases[case_id].must_be_present {
            assert!(
                !canary.paths.is_empty(),
                "no path for positive canary {}",
                canary.label
            );
            for relative in &canary.paths {
                let bytes = fs::read(bundle.join(relative)).unwrap();
                assert!(
                    contains_bytes(&bytes, canary.value.as_bytes()),
                    "bundle {case_id} lost public control {} from {relative}",
                    canary.label
                );
            }
        }
    }
}

#[test]
fn frozen_html_and_markdown_keep_the_static_accessibility_baseline() {
    for case_id in CASES {
        let bundle = corpus_path(format!("bundles/{case_id}"));
        let html = fs::read_to_string(bundle.join("index.html")).unwrap();
        let story = fs::read_to_string(bundle.join("story.md")).unwrap();
        let lower = html.to_ascii_lowercase();

        assert!(lower.starts_with("<!doctype html>"));
        assert!(lower.contains("<html lang=\"en\">"));
        assert_eq!(lower.matches("<main>").count(), 1);
        assert_eq!(lower.matches("<h1>").count(), 1);
        assert!(lower.contains("name=\"viewport\""));
        assert!(lower.contains("content-security-policy"));
        assert!(lower.contains("href=\"projection.json\""));
        assert!(!lower.contains("<script"));
        assert!(!lower.contains("<iframe"));
        assert!(!lower.contains("<object"));
        assert!(!lower.contains("<embed"));
        assert!(!lower.contains("<form"));
        assert!(!lower.contains("javascript:"));
        assert!(!lower.contains("https://"));
        assert!(!lower.contains("http://"));
        assert!(!lower.contains("href=\"//"));
        assert!(!lower.contains("src="));
        assert!(!lower.contains("@import"));
        assert!(!lower.contains("url("));
        assert!(!lower.contains("data:"));
        assert!(!lower.contains("onclick="));
        assert!(!lower.contains("onload="));
        assert!(story.contains("[`projection.json`](./projection.json)"));

        if case_id == "complete" {
            for value in [
                "adopt",
                "reject",
                "defer",
                "AI-attributed proposal",
                "Human decision",
            ] {
                assert!(story.contains(value), "complete story lost {value:?}");
            }
            assert!(lower.contains("<details>"));
            assert!(lower.contains("aria-label=\"asset bytes omitted\""));
        } else {
            assert!(story.contains("No complete creator session"));
            assert!(story.contains("proposal present: `true`, decision present: `true`"));
            assert!(lower.contains("<h2>no complete session</h2>"));
            assert!(lower.contains("proposal present: true, decision present: true"));
        }
    }
}

#[test]
fn protocol_and_result_template_do_not_claim_unrun_external_evidence() {
    let protocol: Value = read_json("protocol.json");
    let questionnaire: Questionnaire = read_json("questionnaire.json");
    let response_schema: Value = read_json("response.schema.json");
    let result_template: Value = read_json("result-template.json");

    assert_eq!(protocol["status"], "not_run");
    assert_eq!(protocol["case_isolation"]["required"], true);
    assert_eq!(protocol["case_isolation"]["combine_case_scores"], false);
    assert_eq!(protocol["context"]["input_artifact_sha256_required"], true);
    assert_eq!(protocol["human"]["required_track"], "html");
    assert_eq!(
        protocol["scoring"]["duplicate_run_id_invalidates_group"],
        true
    );
    assert_eq!(
        protocol["accessibility"]["automated_static_baseline_is_not_wcag_conformance"],
        true
    );
    assert_eq!(
        response_schema["properties"]["case_id"]["enum"],
        json!(["complete", "incomplete-only"])
    );
    assert_eq!(
        response_schema["properties"]["track"]["enum"],
        json!(["json", "html"])
    );
    assert!(
        response_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|required| required == "input_artifact_sha256")
    );
    assert_eq!(
        questionnaire
            .questions
            .iter()
            .find(|question| question.id == "I06")
            .unwrap()
            .tracks,
        ["json"]
    );
    assert_eq!(result_template["status"], "not_run");
    assert_eq!(result_template["accessibility"]["status"], "not_run");
    for case_id in CASES {
        for track in ["zero_context_ai_json", "zero_context_ai_html", "human_html"] {
            assert_eq!(result_template["cases"][case_id][track], json!([]));
        }
    }
}

fn computed_answers(case_id: &str, projection: &PublicProjection) -> BTreeMap<String, Value> {
    let mut answers = BTreeMap::new();
    answers.insert("P01".into(), json!(projection.sessions.len()));
    answers.insert("P02".into(), json!(projection.incomplete_sessions.len()));
    answers.insert(
        "P03".into(),
        json!(projection.publication.network_operations != 0),
    );
    answers.insert(
        "P04".into(),
        json!(projection.publication.raw_assets_included),
    );
    answers.insert(
        "P05".into(),
        json!(projection.publication.source_private_rationale_included),
    );
    answers.insert(
        "P06".into(),
        json!(projection.publication.training_use_policy != "prohibited"),
    );
    assert!(has_limitation(projection, "byte_identity_only"));
    answers.insert("P07".into(), json!("stored_byte_identity_only"));
    assert!(has_limitation(projection, "bundle_not_signed"));
    answers.insert("P08".into(), json!(false));

    match case_id {
        "complete" => {
            assert_eq!(projection.sessions.len(), 3);
            assert!(projection.incomplete_sessions.is_empty());
            answers.insert(
                "C01".into(),
                json!(selected_role(find_session(projection, "adopt-story"))),
            );
            answers.insert(
                "C02".into(),
                json!(selected_role(find_session(projection, "reject-story"))),
            );
            answers.insert(
                "C03".into(),
                json!(selected_role(find_session(projection, "defer-story"))),
            );
            answers.insert(
                "C04".into(),
                json!(
                    projection
                        .sessions
                        .iter()
                        .filter(|session| {
                            matches!(
                                session.human_decision.disposition.as_str(),
                                "reject" | "defer"
                            )
                        })
                        .all(|session| {
                            session.proposal.retained_when_unselected
                                && session
                                    .history
                                    .iter()
                                    .any(|artifact| artifact.role == ArtifactRole::AiProposal)
                        })
                ),
            );
            assert!(projection.sessions.iter().all(|session| {
                session
                    .proposal
                    .attribution_scope
                    .contains("no model invocation is independently verified")
            }));
            answers.insert(
                "C05".into(),
                json!("workflow_attributed_model_invocation_unverified"),
            );
            assert!(
                projection
                    .source
                    .verification_scope
                    .contains("digest-verified reachable CAS objects")
            );
            answers.insert(
                "C06".into(),
                json!("ref_snapshot_and_reachable_cas_verified"),
            );
        }
        "incomplete-only" => {
            assert!(projection.sessions.is_empty());
            assert_eq!(projection.incomplete_sessions.len(), 1);
            let incomplete = &projection.incomplete_sessions[0];
            assert_eq!(incomplete.origin, ValueOrigin::ObservedFromSynapse);
            answers.insert("I01".into(), json!(incomplete.state));
            answers.insert("I02".into(), json!(incomplete.proposal_present));
            answers.insert("I03".into(), json!(incomplete.decision_present));
            answers.insert(
                "I04".into(),
                json!(
                    projection
                        .sessions
                        .iter()
                        .any(|session| session.session == incomplete.session)
                ),
            );
            assert!(
                projection
                    .incomplete_sessions
                    .iter()
                    .any(|session| session.session == incomplete.session)
            );
            answers.insert("I05".into(), json!("incomplete_sessions_only"));
            assert_eq!(
                projection.source.verification_scope,
                "Deterministic Ref snapshot only; reachable CAS closure remains unverified because no complete creator report was available"
            );
            answers.insert("I06".into(), json!(false));
            answers.insert(
                "I07".into(),
                json!(projection.source.projection_source_fingerprint.is_some()),
            );
        }
        other => panic!("unknown corpus case {other}"),
    }
    answers
}

fn selected_role(session: &synapse_publication::PublicSession) -> &'static str {
    match session.human_decision.selected_artifact {
        ArtifactRole::Original => "original",
        ArtifactRole::Current => "current",
        ArtifactRole::AiProposal => "ai_proposal",
    }
}

fn find_session<'a>(
    projection: &'a PublicProjection,
    name: &str,
) -> &'a synapse_publication::PublicSession {
    projection
        .sessions
        .iter()
        .find(|session| session.session == name)
        .unwrap_or_else(|| panic!("missing session {name}"))
}

fn has_limitation(projection: &PublicProjection, code: &str) -> bool {
    projection
        .limitations
        .iter()
        .any(|limitation| limitation.code == code)
}

fn assert_answer_type(question: &Question, answer: &Value) {
    match question.answer_type.as_str() {
        "boolean" => assert!(answer.is_boolean(), "{} must be boolean", question.id),
        "integer" => assert!(answer.as_i64().is_some(), "{} must be integer", question.id),
        "enum" => {
            let value = answer
                .as_str()
                .unwrap_or_else(|| panic!("{} must be a string enum", question.id));
            assert!(
                question
                    .accepted_values
                    .iter()
                    .any(|accepted| accepted == value),
                "{} has unlisted oracle value {value:?}",
                question.id
            );
        }
        other => panic!("unsupported answer type {other}"),
    }
}

fn assert_contract_identity(identity: &SchemaIdentity, expected: &str) {
    assert_eq!(identity.name, expected);
    assert_eq!(identity.version, 1);
}

fn read_json<T: for<'de> Deserialize<'de>>(relative: impl AsRef<Path>) -> T {
    serde_json::from_slice(&fs::read(corpus_path(relative)).unwrap()).unwrap()
}

fn corpus_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(CORPUS_ROOT).join(relative)
}

fn collect_regular_files(root: &Path) -> Vec<PathBuf> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let file_type = entry.file_type().unwrap();
            assert!(!file_type.is_symlink());
            if file_type.is_dir() {
                visit(&entry.path(), files);
            } else {
                assert!(file_type.is_file());
                files.push(entry.path());
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}
