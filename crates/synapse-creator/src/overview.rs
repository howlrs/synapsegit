//! Unverified, bounded display hints for listing creator sessions.
//!
//! A listing must stay useful when a repository has many sessions, so it does
//! not build a report or run `fsck`.  Each session is read with at most six
//! structured CAS reads from its current head: the Commit, its Tree, the
//! creator Actor, the Subject, the image-import Activity, and, for a decision,
//! its DecisionFeedback.  The values are display hints only and never grant
//! authority; opening a session still builds the verified report.

use crate::io::read_json;
use crate::{CREATOR_REUSE_SOURCE_KEY, CREATOR_SOURCE_KEY};
use serde_json::Value as JsonValue;
use synapse_core::Repository;

/// How [`CreatorSessionOverview::recorded_at`] was obtained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatorOverviewTimeBasis {
    /// The DecisionFeedback `recorded_at`.
    FeedbackRecordedAt,
    /// The head Commit `authored_at`, used only when no feedback time exists.
    CommitAuthoredAtFallback,
}

impl CreatorOverviewTimeBasis {
    /// The stable label shown by the localhost dashboard and the CLI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FeedbackRecordedAt => "recorded_at",
            Self::CommitAuthoredAtFallback => "authored_at (unverified fallback)",
        }
    }
}

/// Unverified display hints for one creator session.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreatorSessionOverview {
    /// The head shape shows an unreadable or partial session.
    pub incomplete: bool,
    pub subject_label: Option<String>,
    pub creator_name: Option<String>,
    pub source_session: Option<String>,
    /// `adopt`, `reject`, or `defer` for a readable decision.
    pub disposition: Option<&'static str>,
    pub recorded_at: Option<String>,
    pub recorded_time_basis: Option<CreatorOverviewTimeBasis>,
}

/// Read the display hints of one session from `head`.
///
/// `expect_complete` is the session state from its Ref pair.  A head whose
/// Commit or Tree cannot be read, or a complete-looking session whose head is
/// not a decision Commit, is reported as incomplete.  A malformed field only
/// leaves that value unavailable.
pub fn read_creator_session_overview(
    repository: &Repository,
    head: &str,
    expect_complete: bool,
) -> CreatorSessionOverview {
    let mut overview = CreatorSessionOverview::default();
    let Ok(commit) = read_json(repository, head) else {
        overview.incomplete = true;
        return overview;
    };
    let mut complete = expect_complete;
    if complete && commit.get("commit_kind").and_then(JsonValue::as_str) != Some("decision") {
        overview.incomplete = true;
        complete = false;
    }
    let entries = commit
        .get("snapshot")
        .and_then(JsonValue::as_str)
        .and_then(|tree| read_json(repository, tree).ok())
        .and_then(|tree| tree.get("entries").and_then(JsonValue::as_object).cloned());
    let Some(entries) = entries else {
        overview.incomplete = true;
        return overview;
    };

    let record = |name: &str| -> Option<JsonValue> {
        let oid = entries.get(name)?.get("oid")?.as_str()?;
        read_json(repository, oid).ok()
    };
    if let Some(actor) = record("creator.actor.json") {
        overview.creator_name = payload_text(&actor, "display_name", 300);
    }
    if let Some(subject) = record("subject.json") {
        overview.subject_label = payload_text(&subject, "label", 500);
    }
    if let Some(activity) = record("image-import.activity.json") {
        overview.source_session = source_session(&activity);
    }

    // A completed decision has one feedback transition.  This sixth read does
    // not expose its private rationale.
    if complete && let Some(feedback) = transition_feedback(repository, &commit) {
        overview.disposition =
            payload_text(&feedback, "disposition", 24).and_then(|value| match value.as_str() {
                "adopted_unchanged" => Some("adopt"),
                "rejected" => Some("reject"),
                "deferred" => Some("defer"),
                _ => None,
            });
        if let Some(recorded_at) = top_level_text(&feedback, "recorded_at", 64) {
            overview.recorded_at = Some(recorded_at);
            overview.recorded_time_basis = Some(CreatorOverviewTimeBasis::FeedbackRecordedAt);
        }
    }
    if overview.recorded_at.is_none()
        && let Some(authored_at) = top_level_text(&commit, "authored_at", 64)
    {
        overview.recorded_at = Some(authored_at);
        overview.recorded_time_basis = Some(CreatorOverviewTimeBasis::CommitAuthoredAtFallback);
    }
    overview
}

fn transition_feedback(repository: &Repository, commit: &JsonValue) -> Option<JsonValue> {
    let oid = commit
        .get("transition_refs")?
        .as_array()?
        .first()?
        .as_str()?;
    read_json(repository, oid).ok().filter(|record| {
        record.get("record_type").and_then(JsonValue::as_str) == Some("decision_feedback")
    })
}

fn source_session(activity: &JsonValue) -> Option<String> {
    let extensions = activity.get("extensions")?;
    [CREATOR_SOURCE_KEY, CREATOR_REUSE_SOURCE_KEY]
        .into_iter()
        .find_map(|key| {
            let session = extensions.get(key)?.get("session")?.as_str()?;
            crate::session::validate_session(session)
                .is_ok()
                .then(|| session.to_owned())
        })
}

fn payload_text(record: &JsonValue, field: &str, max_bytes: usize) -> Option<String> {
    bounded_text(record.get("payload")?.get(field)?, max_bytes)
}

fn top_level_text(record: &JsonValue, field: &str, max_bytes: usize) -> Option<String> {
    bounded_text(record.get(field)?, max_bytes)
}

fn bounded_text(value: &JsonValue, max_bytes: usize) -> Option<String> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= max_bytes)
        .map(str::to_owned)
}
