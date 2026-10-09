use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;
use synapse_local_service::{
    BeginCreatorSessionRequest, CreatorDecision, CreatorDecisionRequest, LocalService,
    ProjectRegistration,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);
impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "synapse-presentation-suggestions-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(
    temp: &TempDirectory,
    session: &str,
    subject: &str,
    creator: &str,
) -> BeginCreatorSessionRequest {
    let original = temp.join("original");
    let current = temp.join("current");
    let proposal = temp.join("proposal");
    fs::write(&original, b"original").unwrap();
    fs::write(&current, b"current").unwrap();
    fs::write(&proposal, b"proposal").unwrap();
    BeginCreatorSessionRequest {
        session: session.into(),
        subject_label: subject.into(),
        creator_name: creator.into(),
        original_image: original,
        current_image: current,
        ai_output: proposal,
        generation_note: Some(synapse_creator::CreatorGenerationNote {
            tool: "private tool".into(),
            model: String::new(),
            prompt: "private prompt".into(),
            intent: String::new(),
        }),
    }
}

#[test]
fn suggestions_are_two_fields_verified_complete_and_read_only() {
    let temp = TempDirectory::new();
    let repository = temp.join("repo");
    fs::create_dir(&repository).unwrap();
    let service =
        LocalService::new([ProjectRegistration::new("project", "Project", &repository)]).unwrap();
    let pending = service
        .begin_creator_session(
            "project",
            "instance",
            request(&temp, "complete", "Public title", "Public creator"),
        )
        .unwrap();
    service
        .decide_creator_session(
            "project",
            "complete",
            "instance",
            CreatorDecisionRequest {
                review_id: pending.review_id,
                disposition: CreatorDecision::Adopt,
                rationale: Some("private rationale".into()),
                annotations: None,
            },
        )
        .unwrap();
    let storage = Repository::open(&repository).unwrap();
    let refs_before = storage.refs().snapshot().unwrap();
    let reflog_before = storage.refs().reflog().unwrap();
    drop(storage);
    let suggestions = service
        .presentation_suggestions("project", "complete")
        .unwrap();
    assert_eq!(
        suggestions.creator_display_name.as_deref(),
        Some("Public creator")
    );
    assert_eq!(suggestions.title.as_deref(), Some("Public title"));
    let json = serde_json::to_value(&suggestions).unwrap();
    let object = json.as_object().unwrap();
    assert_eq!(object.len(), 2);
    assert!(!json.to_string().contains("private"));
    let storage = Repository::open(&repository).unwrap();
    assert_eq!(storage.refs().snapshot().unwrap(), refs_before);
    assert_eq!(storage.refs().reflog().unwrap(), reflog_before);
}

#[test]
fn suggestions_reject_pending_and_omit_oversized_values_without_truncation() {
    let temp = TempDirectory::new();
    let repository = temp.join("repo");
    fs::create_dir(&repository).unwrap();
    let service =
        LocalService::new([ProjectRegistration::new("project", "Project", &repository)]).unwrap();
    service
        .begin_creator_session(
            "project",
            "instance",
            request(&temp, "pending", "Title", "Creator"),
        )
        .unwrap();
    assert!(
        service
            .presentation_suggestions("project", "pending")
            .is_err()
    );
    let subject = "x".repeat(301);
    let pending = service
        .begin_creator_session(
            "project",
            "instance",
            request(&temp, "large", &subject, "Creator"),
        )
        .unwrap();
    service
        .decide_creator_session(
            "project",
            "large",
            "instance",
            CreatorDecisionRequest {
                review_id: pending.review_id,
                disposition: CreatorDecision::Reject,
                rationale: None,
                annotations: None,
            },
        )
        .unwrap();
    let suggestions = service
        .presentation_suggestions("project", "large")
        .unwrap();
    assert_eq!(suggestions.creator_display_name.as_deref(), Some("Creator"));
    assert_eq!(suggestions.title, None);
}

#[test]
fn blank_local_rationale_is_absent_from_the_committed_report() {
    let temp = TempDirectory::new();
    let repository = temp.join("repo");
    fs::create_dir(&repository).unwrap();
    let service =
        LocalService::new([ProjectRegistration::new("project", "Project", &repository)]).unwrap();
    let pending = service
        .begin_creator_session(
            "project",
            "instance",
            request(&temp, "blank", "Title", "Creator"),
        )
        .unwrap();
    let complete = service
        .decide_creator_session(
            "project",
            "blank",
            "instance",
            CreatorDecisionRequest {
                review_id: pending.review_id,
                disposition: CreatorDecision::Adopt,
                rationale: Some(String::new()),
                annotations: None,
            },
        )
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(complete.report.rationale, None);
    assert_eq!(complete.report.rationale_source, None);
}
