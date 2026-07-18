//! Generate two self-checked publication bundles for external comprehension
//! and privacy evaluation.
//!
//! Creator sessions intentionally use the current time and fresh random
//! identifiers. Re-running this example therefore creates a fresh corpus
//! candidate, not a byte-identical regeneration of a previous candidate. The
//! bundles within one candidate remain deterministic projections of their
//! fixed source snapshots.

#![forbid(unsafe_code)]

use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_creator::{
    CreatorBeginOptions, CreatorDisposition, CreatorRunOptions, begin_creator_session,
    run_creator_session,
};
use synapse_publication::{
    ExportOptions, OutputTarget, PresentationInput, ProjectionOptions, PublicationVisibility,
    SessionPresentationInput, export_bundle, verify_bundle,
};

const USAGE: &str = "usage: cargo run -p synapse-publication --example generate_evaluation_corpus -- <new-output-directory>";
const COMPLETE_TEMP_PATH_CANARY: &str = "PRIVATE_COMPLETE_TEMP_PATH_CANARY_91E2";
const INCOMPLETE_TEMP_PATH_CANARY: &str = "PRIVATE_INCOMPLETE_TEMP_PATH_CANARY_6B4D";
const COMPLETE_RAW_ORIGINAL: &str = "RAW_COMPLETE_ORIGINAL_CANARY_C18A";
const COMPLETE_RAW_CURRENT: &str = "RAW_COMPLETE_CURRENT_CANARY_4F2D";
const COMPLETE_RAW_PROPOSAL: &str = "RAW_COMPLETE_PROPOSAL_CANARY_A907";
const COMPLETE_PRIVATE_NAME: &str = "PRIVATE_COMPLETE_CREATOR_NAME_CANARY_3D8C";
const COMPLETE_PRIVATE_SUBJECT: &str = "PRIVATE_COMPLETE_SUBJECT_CANARY_7A51";
const COMPLETE_CREDENTIAL: &str = "GH_TOKEN=PRIVATE_COMPLETE_CREDENTIAL_CANARY_5C14";
const INCOMPLETE_RAW_ORIGINAL: &str = "RAW_INCOMPLETE_ORIGINAL_CANARY_80B3";
const INCOMPLETE_RAW_CURRENT: &str = "RAW_INCOMPLETE_CURRENT_CANARY_1E76";
const INCOMPLETE_RAW_PROPOSAL: &str = "RAW_INCOMPLETE_PROPOSAL_CANARY_D942";
const INCOMPLETE_PRIVATE_NAME: &str = "PRIVATE_INCOMPLETE_CREATOR_NAME_CANARY_27C8";
const INCOMPLETE_PRIVATE_SUBJECT: &str = "PRIVATE_INCOMPLETE_SUBJECT_CANARY_F613";
const INCOMPLETE_CREDENTIAL: &str = "GH_TOKEN=PRIVATE_INCOMPLETE_CREDENTIAL_CANARY_B045";
const INCOMPLETE_PRIVATE_NOTE: &str = "PRIVATE_INCOMPLETE_UNSUBMITTED_RATIONALE_CANARY_62AF";

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct FreshTempDirectory {
    path: PathBuf,
}

impl FreshTempDirectory {
    fn create(private_path_segment: &str) -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                io::Error::other(format!("system clock precedes Unix epoch: {error}"))
            })?
            .as_nanos();
        for _ in 0..64 {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "synapse-publication-evaluation-{private_path_segment}-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            #[cfg(unix)]
            let create_result = {
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder.create(&path)
            };
            #[cfg(not(unix))]
            let create_result = fs::DirBuilder::new().create(&path);
            match create_result {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a fresh evaluation source directory",
        ))
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path.join(path)
    }
}

impl Drop for FreshTempDirectory {
    fn drop(&mut self) {
        // `path` was constructed here and create_dir succeeded for this exact
        // candidate. Never broaden cleanup to its parent or an input path.
        if let Err(error) = fs::remove_dir_all(&self.path) {
            eprintln!(
                "warning: could not remove fresh temporary source {}: {error}",
                self.path.display()
            );
        }
    }
}

#[derive(Serialize)]
struct SchemaIdentity {
    name: &'static str,
    version: u32,
}

#[derive(Serialize)]
struct EvaluationCase {
    must_be_absent: Vec<AbsentCanary>,
    must_be_present: Vec<PresentCanary>,
}

#[derive(Serialize)]
struct AbsentCanary {
    label: String,
    value: String,
    scan_base64: bool,
}

#[derive(Serialize)]
struct PresentCanary {
    label: String,
    value: String,
    paths: Vec<&'static str>,
}

#[derive(Serialize)]
struct SourceCanaries {
    schema: SchemaIdentity,
    corpus_version: u32,
    cases: BTreeMap<&'static str, EvaluationCase>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = parse_output_directory(env::args_os().skip(1).collect())?;
    create_new_output_directory(&output)?;

    let complete_source = FreshTempDirectory::create(COMPLETE_TEMP_PATH_CANARY)?;
    let incomplete_source = FreshTempDirectory::create(INCOMPLETE_TEMP_PATH_CANARY)?;

    let complete_case = generate_complete_case(&output, &complete_source)?;
    let incomplete_case = generate_incomplete_case(&output, &incomplete_source)?;

    assert_case_canaries(&output.join("bundles/complete"), &complete_case)?;
    assert_case_canaries(&output.join("bundles/incomplete-only"), &incomplete_case)?;

    let source_canaries = SourceCanaries {
        schema: SchemaIdentity {
            name: "org.synapsegit.publication-comprehension-privacy-canaries",
            version: 1,
        },
        corpus_version: 1,
        cases: BTreeMap::from([
            ("complete", complete_case),
            ("incomplete-only", incomplete_case),
        ]),
    };
    let mut canary_bytes = serde_json::to_vec_pretty(&source_canaries)?;
    canary_bytes.push(b'\n');
    fs::write(output.join("source-canaries.json"), canary_bytes)?;

    println!("corpus={}", output.display());
    println!("complete_bundle=bundles/complete");
    println!("incomplete_only_bundle=bundles/incomplete-only");
    println!("source_canaries=source-canaries.json");
    println!("candidate_identity=fresh_not_byte_identical_on_regeneration");
    Ok(())
}

fn parse_output_directory(args: Vec<OsString>) -> io::Result<PathBuf> {
    if args.len() != 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE));
    }
    Ok(PathBuf::from(&args[0]))
}

fn create_new_output_directory(output: &Path) -> io::Result<()> {
    match fs::create_dir(output) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to replace existing evaluation corpus {}",
                    output.display()
                ),
            ));
        }
        Err(error) => return Err(error),
    }
    fs::create_dir(output.join("bundles"))
}

fn generate_complete_case(
    output: &Path,
    source: &FreshTempDirectory,
) -> Result<EvaluationCase, Box<dyn Error>> {
    let inputs = create_inputs(
        source,
        COMPLETE_RAW_ORIGINAL,
        COMPLETE_RAW_CURRENT,
        COMPLETE_RAW_PROPOSAL,
    )?;
    let repository = source.join("repository");
    let mut must_be_absent = vec![
        absent("raw_original_bytes", COMPLETE_RAW_ORIGINAL),
        absent("raw_current_bytes", COMPLETE_RAW_CURRENT),
        absent("raw_proposal_bytes", COMPLETE_RAW_PROPOSAL),
        absent("private_creator_name", COMPLETE_PRIVATE_NAME),
        absent("private_subject_label", COMPLETE_PRIVATE_SUBJECT),
        absent("credential_like_token", COMPLETE_CREDENTIAL),
        absent("private_temp_path_segment", COMPLETE_TEMP_PATH_CANARY),
    ];

    for (session, disposition, rationale_canary) in [
        (
            "adopt-story",
            CreatorDisposition::Adopt,
            "PRIVATE_ADOPT_RATIONALE_CANARY_16E4",
        ),
        (
            "reject-story",
            CreatorDisposition::Reject,
            "PRIVATE_REJECT_RATIONALE_CANARY_B729",
        ),
        (
            "defer-story",
            CreatorDisposition::Defer,
            "PRIVATE_DEFER_RATIONALE_CANARY_4A0C",
        ),
    ] {
        let receipt = run_creator_session(&CreatorRunOptions {
            repository: repository.clone(),
            session: session.into(),
            original_image: inputs.original.clone(),
            current_image: inputs.current.clone(),
            ai_output: inputs.proposal.clone(),
            subject_label: COMPLETE_PRIVATE_SUBJECT.into(),
            creator_name: COMPLETE_PRIVATE_NAME.into(),
            disposition,
            rationale: Some(format!("{rationale_canary} {COMPLETE_CREDENTIAL}")),
        })?;
        must_be_absent.push(absent(
            format!("private_rationale_{session}"),
            rationale_canary,
        ));
        must_be_absent.push(absent(
            format!("internal_creator_id_{session}"),
            receipt.creator_id,
        ));
        must_be_absent.push(absent(
            format!("internal_agent_id_{session}"),
            receipt.agent_id,
        ));
    }

    let public_title = "Public Complete Corpus Title Canary 931A";
    let public_summary = "Public Complete Corpus Summary Canary 2B6F";
    let public_creator = "Public Complete Creator Canary 04D7";
    let public_agent = "Public Complete Agent Canary C520";
    let public_notes = [
        (
            "adopt-story",
            "Public Adopt Decision Note Canary A16D",
            "Public Adopt Session Title Canary 7F35",
        ),
        (
            "reject-story",
            "Public Reject Decision Note Canary D08B",
            "Public Reject Session Title Canary 1C49",
        ),
        (
            "defer-story",
            "Public Defer Decision Note Canary 63E2",
            "Public Defer Session Title Canary B704",
        ),
    ];
    let mut presentation = PresentationInput {
        title: Some(public_title.into()),
        summary: Some(public_summary.into()),
        creator_display_name: Some(public_creator.into()),
        proposal_agent_display_name: Some(public_agent.into()),
        sessions: BTreeMap::new(),
    };
    for (session, note, title) in public_notes {
        presentation.sessions.insert(
            session.into(),
            SessionPresentationInput {
                title: Some(title.into()),
                public_decision_note: Some(note.into()),
                original_caption: None,
                current_caption: None,
                proposal_caption: None,
            },
        );
    }

    let bundle = output.join("bundles/complete");
    export_public_synapse_bundle(repository, bundle.clone(), presentation)?;
    verify_bundle(&bundle)?;

    let mut must_be_present = vec![
        present("public_title", public_title),
        present("public_summary", public_summary),
        present("public_creator_display_name", public_creator),
        present("public_agent_display_name", public_agent),
    ];
    for (session, note, title) in public_notes {
        must_be_present.push(present(format!("public_session_id_{session}"), session));
        must_be_present.push(present(format!("public_decision_note_{session}"), note));
        must_be_present.push(present(format!("public_session_title_{session}"), title));
    }

    Ok(EvaluationCase {
        must_be_absent,
        must_be_present,
    })
}

fn generate_incomplete_case(
    output: &Path,
    source: &FreshTempDirectory,
) -> Result<EvaluationCase, Box<dyn Error>> {
    let inputs = create_inputs(
        source,
        INCOMPLETE_RAW_ORIGINAL,
        INCOMPLETE_RAW_CURRENT,
        INCOMPLETE_RAW_PROPOSAL,
    )?;
    // An incomplete session has no submitted Human DecisionFeedback rationale.
    // Keep an explicit unsubmitted source-side review note so the corpus still
    // carries a literal private-rationale boundary canary without mislabeling
    // it as verified history.
    fs::write(
        source.join("private-review-note.txt"),
        INCOMPLETE_PRIVATE_NOTE,
    )?;
    let repository = source.join("repository");
    let pending = begin_creator_session(&CreatorBeginOptions {
        repository: repository.clone(),
        session: "pending-story".into(),
        original_image: inputs.original,
        current_image: inputs.current,
        ai_output: inputs.proposal,
        subject_label: INCOMPLETE_PRIVATE_SUBJECT.into(),
        creator_name: format!("{INCOMPLETE_PRIVATE_NAME} {INCOMPLETE_CREDENTIAL}"),
    })?;
    let creator_id = pending.receipt().creator_id.clone();
    let agent_id = pending.receipt().agent_id.clone();
    drop(pending);

    let public_title = "Public Incomplete Corpus Title Canary E317";
    let public_summary = "Public Incomplete Corpus Summary Canary 58AC";
    let public_creator = "Public Incomplete Creator Canary 90F2";
    let public_agent = "Public Incomplete Agent Canary 2D61";
    let presentation = PresentationInput {
        title: Some(public_title.into()),
        summary: Some(public_summary.into()),
        creator_display_name: Some(public_creator.into()),
        proposal_agent_display_name: Some(public_agent.into()),
        sessions: BTreeMap::new(),
    };

    let bundle = output.join("bundles/incomplete-only");
    export_public_synapse_bundle(repository, bundle.clone(), presentation)?;
    verify_bundle(&bundle)?;

    Ok(EvaluationCase {
        must_be_absent: vec![
            absent("raw_original_bytes", INCOMPLETE_RAW_ORIGINAL),
            absent("raw_current_bytes", INCOMPLETE_RAW_CURRENT),
            absent("raw_proposal_bytes", INCOMPLETE_RAW_PROPOSAL),
            absent("private_creator_name", INCOMPLETE_PRIVATE_NAME),
            absent("private_subject_label", INCOMPLETE_PRIVATE_SUBJECT),
            absent("credential_like_token", INCOMPLETE_CREDENTIAL),
            absent("private_unsubmitted_rationale", INCOMPLETE_PRIVATE_NOTE),
            absent("internal_creator_id", creator_id),
            absent("internal_agent_id", agent_id),
            absent("private_temp_path_segment", INCOMPLETE_TEMP_PATH_CANARY),
        ],
        must_be_present: vec![
            present("public_title", public_title),
            present("public_summary", public_summary),
            present("public_creator_display_name", public_creator),
            present("public_agent_display_name", public_agent),
            present("public_pending_session_id", "pending-story"),
        ],
    })
}

struct InputPaths {
    original: PathBuf,
    current: PathBuf,
    proposal: PathBuf,
}

fn create_inputs(
    source: &FreshTempDirectory,
    original_bytes: &str,
    current_bytes: &str,
    proposal_bytes: &str,
) -> io::Result<InputPaths> {
    let inputs = source.join("inputs");
    fs::create_dir(&inputs)?;
    let original = inputs.join("original.bin");
    let current = inputs.join("current.bin");
    let proposal = inputs.join("proposal.bin");
    fs::write(&original, original_bytes)?;
    fs::write(&current, current_bytes)?;
    fs::write(&proposal, proposal_bytes)?;
    Ok(InputPaths {
        original,
        current,
        proposal,
    })
}

fn export_public_synapse_bundle(
    repository: PathBuf,
    destination: PathBuf,
    presentation: PresentationInput,
) -> synapse_publication::Result<()> {
    let mut projection = ProjectionOptions::new(repository);
    projection.visibility = PublicationVisibility::Public;
    projection.presentation = presentation;
    export_bundle(&ExportOptions {
        projection,
        destination,
        target: OutputTarget::Synapse,
    })?;
    Ok(())
}

fn assert_case_canaries(bundle: &Path, case: &EvaluationCase) -> io::Result<()> {
    for canary in &case.must_be_absent {
        if bundle_contains_literal(bundle, canary.value.as_bytes())? {
            return Err(io::Error::other(format!(
                "{} leaked must_be_absent canary {:?}",
                bundle.display(),
                canary.label
            )));
        }
        if canary.scan_base64
            && bundle_contains_literal(bundle, base64(canary.value.as_bytes()).as_bytes())?
        {
            return Err(io::Error::other(format!(
                "{} leaked Base64 must_be_absent canary {:?}",
                bundle.display(),
                canary.label
            )));
        }
    }
    for canary in &case.must_be_present {
        for relative in &canary.paths {
            let bytes = fs::read(bundle.join(relative))?;
            if !contains_bytes(&bytes, canary.value.as_bytes()) {
                return Err(io::Error::other(format!(
                    "{} omitted must_be_present canary {:?} from {relative}",
                    bundle.display(),
                    canary.label
                )));
            }
        }
    }
    Ok(())
}

fn bundle_contains_literal(root: &Path, needle: &[u8]) -> io::Result<bool> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if bundle_contains_literal(&entry.path(), needle)? {
                return Ok(true);
            }
        } else if file_type.is_file() {
            let bytes = fs::read(entry.path())?;
            if contains_bytes(&bytes, needle) {
                return Ok(true);
            }
        } else {
            return Err(io::Error::other(format!(
                "unexpected non-file bundle entry {}",
                entry.path().display()
            )));
        }
    }
    Ok(false)
}

fn absent(label: impl Into<String>, value: impl Into<String>) -> AbsentCanary {
    AbsentCanary {
        label: label.into(),
        value: value.into(),
        scan_base64: true,
    }
}

fn present(label: impl Into<String>, value: impl Into<String>) -> PresentCanary {
    PresentCanary {
        label: label.into(),
        value: value.into(),
        paths: vec![
            "projection.json",
            "story.md",
            "index.html",
            "target/public-projection.json",
        ],
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_source_directory_is_created_with_owner_only_permissions() {
        let directory = FreshTempDirectory::create("permission-test").unwrap();
        let mode = fs::metadata(&directory.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
