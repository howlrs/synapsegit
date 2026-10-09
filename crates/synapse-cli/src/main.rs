#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_canonical::{DEFAULT_MAX_STRUCTURED_BYTES, ObjectKind};
use synapse_core::{Repository, RepositoryError};
use synapse_creator::{
    CreatorDisposition, CreatorError, CreatorGenerationNote, CreatorReport, CreatorRunOptions,
    CreatorSessionState, ImportInboxCandidate, creator_report, discover_creator_sessions,
    image_metadata_warning, put_import_inbox_candidate, read_creator_session_overview,
    retain_import_inbox_candidate, run_creator_session_with_note, suggested_import_inbox_session,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

mod report_json;
use report_json::CreatorReportDocument;

const USAGE: &str = "\
SynapseGit Core

Usage:
  synapse init <repo>
  synapse put-blob <repo> <file> [--claimed <oid>]
  synapse put-record <repo> <file> [--claimed <oid>]
  synapse build-tree <repo> <file> [--claimed <oid>]
  synapse commit <repo> <file> [--claimed <oid>]
  synapse put-object <repo> <file> [--claimed <oid>]
  synapse update-ref <repo> <ref> <expected-oid|-> <new-oid> [--actor <id>] [--message <text>]
  synapse refs <repo>
  synapse fsck <repo>
  synapse export <repo> <archive-dir>
  synapse restore <archive-dir> <repo>
  synapse creator-run <repo> <session> <original> <current> <ai-output> --subject <label> --creator <name> --decision <adopt|reject|defer> [--rationale <text>] [--generation-note-file <path>]
  synapse creator-report <repo> <session> [--format text|json]
  synapse creator-list <repo> [--format text|json]
  synapse inbox put <inbox-dir> <slug> <original> <current> <ai-output> --subject <label> --creator <name> [--generation-note-file <path>] [--format text|json]
  synapse inbox decide <inbox-dir> <slug> <repo> --decision <adopt|reject|defer> [--rationale <text>] [--session <name>] [--creator <name>]

creator-report --format json prints one private, LOCAL-only JSON document to
stdout, tagged \"format\": \"synapsegit-cli-creator-report-v1\" and
\"scope\": \"private_local\". It may contain rationale text, user-declared
generation notes, decision pins, and internal identifiers, and it is a
separate contract from the public projection bundle. It is not for sharing;
for a shareable bundle use `synapse-present export ... --public`. Omitting
--format, or passing --format text, keeps the existing line-oriented text
output unchanged.

creator-run --generation-note-file accepts a UTF-8 JSON object with optional
\"tool\", \"model\", \"prompt\", and \"intent\" strings. It records a private,
user-declared note; it is not execution or authorship evidence and is never
included in public bundles. Each field is limited to 300, 300, 8192, and 2048
UTF-8 bytes respectively, with a 16 KiB serialized-note limit.

Run `synapse COMMAND --help` or `synapse help COMMAND` for one command.

creator-list prints an unverified overview of every creator session, read
with at most six bounded reads per session. Use creator-report for the
verified record of one session.

inbox put writes one candidate for the synapse-local import inbox. It needs no
repository, records no decision, and creates no Proposal: a person reviews the
candidate and decides in the localhost UI started with --import-root. Run
`synapse inbox --help` for details.
";
const INBOX_USAGE: &str = "\
Usage:
  synapse inbox put <inbox-dir> <slug> <original> <current> <ai-output> --subject <label> --creator <name> [--generation-note-file <path>] [--format text|json]
  synapse inbox decide <inbox-dir> <slug> <repo> --decision <adopt|reject|defer> [--rationale <text>] [--session <name>] [--creator <name>]

Write one candidate for the synapse-local import inbox without recording a
decision. The command copies the three files to <inbox-dir>/<slug>/ as
original, current, and ai-output, records their sizes and SHA-256 digests in a
synapsegit-import-inbox-v1 manifest, and publishes the directory without
replacing an existing one. <inbox-dir> must already exist; it is not created.
It opens no repository, so it is safe while synapse-local is running.

Then start or keep synapse-local running with --import-root PROJECT=<inbox-dir>
and let a person open the project page, review the candidate, and record the
decision.

<slug> must match [a-z][a-z0-9-]{0,63} and must not exist yet. Each file must be
a regular file of at most 64 MiB. --subject is limited to 500 and --creator to
300 UTF-8 bytes. --generation-note-file uses the creator-run note format and
limits. --format json prints one JSON document tagged
\"format\": \"synapsegit-cli-inbox-put-v1\".

`inbox decide` is only for a Human Decision explicitly supplied after the
person reviewed the exact three images. It verifies and retains the Inbox
bytes before opening the repository, then records the manifest subject,
creator, and generation note. `--creator` is the only metadata override.
The default session is the same canonical suggestion as synapse-local:
`inbox-<slug>` when it fits, otherwise a readable prefix plus a SHA-256 suffix.
It never changes Inbox. Do not run
it while synapse-local has the repository open.
";
const VERSION: &str = concat!("synapse ", env!("CARGO_PKG_VERSION"));

/// Write to stdout without panicking.  When the reader has already closed the
/// pipe, as `| head` does, the rest of the output is discarded and the command
/// still finishes with its own exit status.  Any other write failure is a
/// `storage_error`.
fn write_stdout(arguments: fmt::Arguments<'_>) -> Result<(), CliError> {
    match io::stdout().write_fmt(arguments) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(CliError::Stdout(error)),
        _ => Ok(()),
    }
}

macro_rules! out {
    ($($arg:tt)*) => {
        write_stdout(format_args!($($arg)*))?
    };
}

macro_rules! outln {
    ($($arg:tt)*) => {
        write_stdout(format_args!("{}\n", format_args!($($arg)*)))?
    };
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    InitTargetNotEmpty(PathBuf),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Core(RepositoryError),
    Creator(CreatorError),
    CreatorReportUnavailableAfterCommit {
        session: String,
        source: CreatorError,
    },
    Clock(String),
    Stdout(io::Error),
    FsckFailed,
}

impl CliError {
    fn code(&self) -> &str {
        match self {
            Self::Usage(_) => "usage_error",
            Self::InitTargetNotEmpty(_) => "repository_not_empty",
            Self::Io { .. } | Self::Clock(_) | Self::Stdout(_) => "storage_error",
            Self::Core(error) => error.code(),
            Self::Creator(error) => error.code(),
            Self::CreatorReportUnavailableAfterCommit { .. } => {
                "creator_report_unavailable_after_commit"
            }
            Self::FsckFailed => "fsck_failed",
        }
    }

    /// One line of advice for errors whose next step is not in the message.
    fn hint(&self) -> Option<&'static str> {
        if let Self::Creator(CreatorError::Io { operation, .. }) = self {
            if operation.contains("input") {
                return Some("check that each input path exists and names a readable regular file");
            }
        }
        match self.code() {
            "creator_session_exists" => Some(
                "choose a new session name; inspect the existing one with `synapse creator-report REPO SESSION`",
            ),
            "creator_session_incomplete" => Some(
                "an earlier attempt left partial history, which is never resumed or rewritten; choose a new session name, or open the session in synapse-local to diagnose it",
            ),
            "creator_session_not_found" => {
                Some("list the sessions with `synapse creator-list REPO`")
            }
            "fsck_failed" => Some(
                "history is never rewritten automatically; review the listed issues and keep a verified archive",
            ),
            _ => None,
        }
    }

    fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::InitTargetNotEmpty(path) => write!(
                formatter,
                "repository path {} is not empty and is not a complete SynapseGit repository; choose an empty directory or inspect the partial layout before retrying",
                path.display()
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
            Self::Core(error) => error.fmt(formatter),
            Self::Creator(error) => error.fmt(formatter),
            Self::CreatorReportUnavailableAfterCommit { session, source } => write!(
                formatter,
                "creator session {session:?} was committed, but its report is unavailable: {source}; rerun creator-report"
            ),
            Self::Clock(message) => formatter.write_str(message),
            Self::Stdout(source) => write!(formatter, "write standard output: {source}"),
            Self::FsckFailed => formatter.write_str("fsck found integrity issues"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Stdout(source) => Some(source),
            Self::Core(error) => Some(error),
            Self::Creator(error) => Some(error),
            Self::CreatorReportUnavailableAfterCommit { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<RepositoryError> for CliError {
    fn from(error: RepositoryError) -> Self {
        Self::Core(error)
    }
}

impl From<CreatorError> for CliError {
    fn from(error: CreatorError) -> Self {
        Self::Creator(error)
    }
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let show_usage = error.code() == "usage_error";
            eprintln!("{}: {error}", error.code());
            if let Some(hint) = error.hint() {
                eprintln!("hint: {hint}");
            }
            if show_usage {
                eprintln!("\n{USAGE}");
            }
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<String>) -> Result<(), CliError> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(CliError::Usage("a command is required".into()));
    };
    if command != "inbox" && matches!(args.get(1).map(String::as_str), Some("--help" | "-h")) {
        if let Some(help) = command_help(command) {
            out!("{help}");
            return Ok(());
        }
    }
    if command == "help" {
        if let Some(topic) = args.get(1) {
            let help = command_help(topic)
                .ok_or_else(|| CliError::Usage(format!("unknown command {topic:?}")))?;
            out!("{help}");
            return Ok(());
        }
    }
    match command {
        "init" => {
            require_len(&args, 2)?;
            init_repository(Path::new(&args[1]))?;
            outln!("initialized {}", args[1]);
        }
        "put-blob" => put_blob(&args)?,
        "put-record" => put_structured(&args, Some(ObjectKind::Record))?,
        "build-tree" => put_structured(&args, Some(ObjectKind::Tree))?,
        "commit" => put_structured(&args, Some(ObjectKind::Commit))?,
        "put-object" => put_structured(&args, None)?,
        "update-ref" => update_ref(&args)?,
        "refs" => {
            require_len(&args, 2)?;
            let repository = Repository::open_existing(&args[1])?;
            for record in repository.refs().list().map_err(RepositoryError::from)? {
                outln!("{}\t{}", record.name, record.head);
            }
        }
        "fsck" => {
            require_len(&args, 2)?;
            let repository = Repository::open_existing(&args[1])?;
            let report = repository.fsck()?;
            outln!(
                "objects={} verified={} closures={} issues={}",
                report.objects_seen,
                report.objects_verified,
                report.closures.len(),
                report.issues.len()
            );
            for issue in &report.issues {
                eprintln!("{:?}", issue.kind);
            }
            if !report.is_clean() {
                return Err(CliError::FsckFailed);
            }
        }
        "export" => {
            require_len(&args, 3)?;
            let mut repository = Repository::open_existing(&args[1])?;
            repository.export_archive(&args[2])?;
            outln!("exported {}", args[2]);
        }
        "restore" => {
            require_len(&args, 3)?;
            Repository::restore_archive(&args[1], &args[2])?;
            outln!("restored {}", args[2]);
        }
        "creator-run" => creator_run(&args)?,
        "creator-report" => creator_report_command(&args)?,
        "inbox" => inbox_command(&args)?,
        "creator-list" => creator_list_command(&args)?,
        "help" | "--help" | "-h" => outln!("{USAGE}"),
        "version" | "--version" | "-V" => outln!("{VERSION}"),
        other => return Err(CliError::Usage(format!("unknown command {other:?}"))),
    }
    Ok(())
}

fn init_repository(path: &Path) -> Result<(), CliError> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(CliError::InitTargetNotEmpty(path.to_path_buf()));
        }
        Ok(_) => match Repository::open_existing(path) {
            Ok(_) => return Ok(()),
            Err(_)
                if std::fs::read_dir(path)
                    .map_err(|source| CliError::io("inspect repository path", path, source))?
                    .next()
                    .is_some() =>
            {
                return Err(CliError::InitTargetNotEmpty(path.to_path_buf()));
            }
            Err(_) => {}
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(CliError::io("inspect repository path", path, source)),
    }
    Repository::open(path)?;
    Ok(())
}

fn creator_run(args: &[String]) -> Result<(), CliError> {
    if args.len() < 6 {
        return Err(CliError::Usage(
            "creator-run requires <repo> <session> <original> <current> <ai-output> and --subject, --creator, --decision".into(),
        ));
    }
    let mut subject = None;
    let mut creator = None;
    let mut decision = None;
    let mut rationale = None;
    let mut generation_note_file = None;
    let mut index = 6;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("{} requires a value", args[index])))?;
        match args[index].as_str() {
            "--subject" if subject.is_none() => subject = Some(value.clone()),
            "--creator" if creator.is_none() => creator = Some(value.clone()),
            "--decision" if decision.is_none() => {
                decision = Some(CreatorDisposition::parse(value)?)
            }
            "--rationale" if rationale.is_none() => rationale = Some(value.clone()),
            "--generation-note-file" if generation_note_file.is_none() => {
                generation_note_file = Some(PathBuf::from(value));
            }
            other => {
                return Err(CliError::Usage(format!(
                    "invalid or duplicate creator-run option {other:?}"
                )));
            }
        }
        index += 2;
    }
    let generation_note = generation_note_file
        .as_deref()
        .map(read_generation_note_file)
        .transpose()?;
    let options = CreatorRunOptions {
        repository: args[1].as_str().into(),
        session: args[2].clone(),
        original_image: args[3].as_str().into(),
        current_image: args[4].as_str().into(),
        ai_output: args[5].as_str().into(),
        subject_label: subject
            .ok_or_else(|| CliError::Usage("creator-run requires --subject".into()))?,
        creator_name: creator
            .ok_or_else(|| CliError::Usage("creator-run requires --creator".into()))?,
        disposition: decision
            .ok_or_else(|| CliError::Usage("creator-run requires --decision".into()))?,
        rationale,
    };
    let metadata_warnings = [
        image_metadata_warning("original", &options.original_image),
        image_metadata_warning("current", &options.current_image),
        image_metadata_warning("ai_output", &options.ai_output),
    ];
    let receipt = run_creator_session_with_note(&options, generation_note.as_ref())?;
    let report = creator_report(&options.repository, &options.session).map_err(|source| {
        CliError::CreatorReportUnavailableAfterCommit {
            session: options.session.clone(),
            source,
        }
    })?;
    emit_metadata_warnings(&metadata_warnings);
    outln!("session={}", receipt.session);
    outln!("subject={}", receipt.subject_id);
    outln!("original={}", receipt.original_blob_oid);
    outln!("current={}", receipt.current_blob_oid);
    outln!("ai_output={}", receipt.ai_output_blob_oid);
    outln!(
        "proposal_ref={}\t{}",
        receipt.proposal_ref,
        receipt.proposal_head
    );
    outln!(
        "decision_ref={}\t{}",
        receipt.decision_ref,
        receipt.decision_head
    );
    outln!("disposition={}", receipt.disposition.as_cli_str());
    print_creator_report(&report)?;
    Ok(())
}

/// Usage and a short description for one command, or `None` when unknown.
fn command_help(command: &str) -> Option<String> {
    if command == "inbox" {
        return Some(INBOX_USAGE.to_owned());
    }
    let prefix = format!("  synapse {command} ");
    let lines: Vec<&str> = USAGE
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .collect();
    if lines.is_empty() {
        return None;
    }
    let description = match command {
        "init" => {
            "Create an empty repository at <repo>, or succeed when it is already complete. A nonempty directory that is not a repository is refused."
        }
        "put-blob" => {
            "Store a file as one Blob and print its OID. --claimed refuses a different OID."
        }
        "put-record" | "build-tree" | "commit" | "put-object" => {
            "Store one strict structured object and print its OID. --claimed refuses a different OID. This is a low-level trusted-operator command."
        }
        "update-ref" => {
            "Compare-and-swap one Ref from <expected-oid> (or - to create) to <new-oid> and append a reflog entry. This is a low-level trusted-operator command."
        }
        "refs" => "Print the current Refs as <name><TAB><commit-oid>.",
        "fsck" => {
            "Check stored objects and the closure of the current Refs. Exits 1 with fsck_failed when an issue is found."
        }
        "export" => {
            "Write a checksum-bound directory archive to a new <archive-dir>. An existing destination is never replaced."
        }
        "restore" => "Verify an archive and restore it into an empty <repo>.",
        "creator-run" => {
            "Import three files and record the Human Decision in one run. The decision is recorded as made by a person: an AI agent must not choose it, and should place candidates with `synapse inbox put` for a person to decide in synapse-local. --generation-note-file accepts a UTF-8 JSON object with optional tool, model, prompt, and intent strings (300, 300, 8192, and 2048 UTF-8 bytes, 16 KiB in total)."
        }
        "creator-report" => {
            "Rebuild and print the verified report of one creator session. --format json prints one private, local-only document tagged \"format\": \"synapsegit-cli-creator-report-v1\"; it may contain rationale, generation notes, pins, and internal identifiers, so do not share it."
        }
        "creator-list" => {
            "Print an unverified overview of every creator session: name, state, disposition, recorded time, Subject label, and creator name, read with at most six bounded reads per session. A review waiting in a running synapse-local is shown as incomplete. Use creator-report for the verified record. --format json prints one document tagged \"format\": \"synapsegit-cli-creator-list-v1\"."
        }
        _ => "",
    };
    Some(format!("Usage:\n{}\n\n{description}\n", lines.join("\n")))
}

const CREATOR_LIST_FORMAT: &str = "synapsegit-cli-creator-list-v1";
const CREATOR_LIST_MAX_REFS: usize = 100_000;
const CREATOR_LIST_MAX_SESSIONS: usize = 50_000;

fn creator_list_command(args: &[String]) -> Result<(), CliError> {
    let format = match args.len() {
        2 => CreatorReportFormat::Text,
        4 if args[2] == "--format" => match args[3].as_str() {
            "text" => CreatorReportFormat::Text,
            "json" => CreatorReportFormat::Json,
            other => {
                return Err(CliError::Usage(format!(
                    "--format must be text or json, got {other:?}"
                )));
            }
        },
        _ => {
            return Err(CliError::Usage(
                "creator-list requires <repo> and accepts only --format text|json".into(),
            ));
        }
    };
    let repository = Repository::open_existing(&args[1])?;
    let snapshot = repository
        .refs()
        .snapshot_limited(CREATOR_LIST_MAX_REFS)
        .map_err(RepositoryError::from)?;
    let sessions = discover_creator_sessions(&repository, &snapshot, CREATOR_LIST_MAX_SESSIONS)?;
    let rows: Vec<_> = sessions
        .into_iter()
        .map(|session| {
            let complete = session.state == CreatorSessionState::Complete;
            let overview = session
                .decision_head
                .as_deref()
                .or(session.proposal_head.as_deref())
                .map(|head| read_creator_session_overview(&repository, head, complete))
                .unwrap_or_default();
            let state = if complete && !overview.incomplete {
                "complete"
            } else {
                "incomplete"
            };
            (session.session, state, overview)
        })
        .collect();
    match format {
        CreatorReportFormat::Text => {
            outln!(
                "# session\tstate\tdisposition\trecorded_at\tsubject_label\tcreator_name (unverified overview; run creator-report for the verified record)"
            );
            for (session, state, overview) in &rows {
                let quoted = |value: &Option<String>| {
                    value
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), |value| format!("{value:?}"))
                };
                outln!(
                    "{session}\t{state}\t{}\t{}\t{}\t{}",
                    overview.disposition.unwrap_or("-"),
                    overview.recorded_at.as_deref().unwrap_or("-"),
                    quoted(&overview.subject_label),
                    quoted(&overview.creator_name),
                );
            }
        }
        CreatorReportFormat::Json => {
            let sessions: Vec<_> = rows
                .iter()
                .map(|(session, state, overview)| {
                    serde_json::json!({
                        "session": session,
                        "state": state,
                        "disposition": overview.disposition,
                        "recorded_at": overview.recorded_at,
                        "recorded_time_basis": overview.recorded_time_basis.map(|basis| basis.as_str()),
                        "subject_label": overview.subject_label,
                        "creator_name": overview.creator_name,
                        "source_session": overview.source_session,
                    })
                })
                .collect();
            let document = serde_json::json!({
                "format": CREATOR_LIST_FORMAT,
                "scope": "private_local",
                "verified": false,
                "sessions": sessions,
            });
            outln!(
                "{}",
                serde_json::to_string_pretty(&document).expect("serializable creator list")
            );
        }
    }
    Ok(())
}

const INBOX_PUT_FORMAT: &str = "synapsegit-cli-inbox-put-v1";
const INBOX_NEXT_STEP: &str = "start synapse-local with --import-root PROJECT=<inbox-dir>, open the project page, and let a person review the candidate and record the decision";

fn inbox_command(args: &[String]) -> Result<(), CliError> {
    let is_help = |value: Option<&String>| {
        matches!(value.map(String::as_str), Some("--help" | "-h" | "help"))
    };
    if is_help(args.get(1))
        || (args
            .get(1)
            .is_some_and(|value| matches!(value.as_str(), "put" | "decide"))
            && is_help(args.get(2)))
    {
        out!("{INBOX_USAGE}");
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("decide") {
        return inbox_decide(&args[1..]);
    }
    if args.get(1).map(String::as_str) != Some("put") {
        return Err(CliError::Usage("inbox requires put or decide".into()));
    }
    if args.len() < 7 {
        return Err(CliError::Usage(
            "inbox put requires <inbox-dir> <slug> <original> <current> <ai-output> and --subject, --creator".into(),
        ));
    }
    let mut subject = None;
    let mut creator = None;
    let mut generation_note_file = None;
    let mut format = None;
    let mut index = 7;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("{} requires a value", args[index])))?;
        match args[index].as_str() {
            "--subject" if subject.is_none() => subject = Some(value.clone()),
            "--creator" if creator.is_none() => creator = Some(value.clone()),
            "--generation-note-file" if generation_note_file.is_none() => {
                generation_note_file = Some(PathBuf::from(value));
            }
            "--format" if format.is_none() => {
                format = Some(match value.as_str() {
                    "text" => CreatorReportFormat::Text,
                    "json" => CreatorReportFormat::Json,
                    other => {
                        return Err(CliError::Usage(format!(
                            "--format must be text or json, got {other:?}"
                        )));
                    }
                });
            }
            other => {
                return Err(CliError::Usage(format!(
                    "invalid or duplicate inbox put option {other:?}"
                )));
            }
        }
        index += 2;
    }
    let subject = subject.ok_or_else(|| CliError::Usage("inbox put requires --subject".into()))?;
    let creator = creator.ok_or_else(|| CliError::Usage("inbox put requires --creator".into()))?;
    let generation_note = generation_note_file
        .as_deref()
        .map(read_generation_note_file)
        .transpose()?;
    let metadata_warnings = [
        image_metadata_warning("original", Path::new(&args[4])),
        image_metadata_warning("current", Path::new(&args[5])),
        image_metadata_warning("ai_output", Path::new(&args[6])),
    ];
    let receipt = put_import_inbox_candidate(&ImportInboxCandidate {
        inbox_root: Path::new(&args[2]),
        slug: &args[3],
        original: Path::new(&args[4]),
        current: Path::new(&args[5]),
        ai_output: Path::new(&args[6]),
        subject_label: &subject,
        creator_name: &creator,
        generation_note: generation_note.as_ref(),
    })?;
    emit_metadata_warnings(&metadata_warnings);
    let manifest = &receipt.manifest;
    match format.unwrap_or(CreatorReportFormat::Text) {
        CreatorReportFormat::Text => {
            outln!("inbox_candidate={}", args[3]);
            outln!("path={}", receipt.directory.display());
            for (role, file) in [
                ("original", &manifest.original),
                ("current", &manifest.current),
                ("ai_output", &manifest.ai_output),
            ] {
                outln!("{role}_size={}", file.size);
                outln!("{role}_sha256={}", file.sha256.as_deref().unwrap_or(""));
            }
            outln!("decision_recorded=false");
            outln!("next={INBOX_NEXT_STEP}");
        }
        CreatorReportFormat::Json => {
            let document = serde_json::json!({
                "format": INBOX_PUT_FORMAT,
                "slug": args[3],
                "path": receipt.directory.display().to_string(),
                "manifest": manifest,
                "decision_recorded": false,
                "metadata_warnings": metadata_warnings,
                "next": INBOX_NEXT_STEP,
            });
            outln!(
                "{}",
                serde_json::to_string_pretty(&document).expect("serializable inbox receipt")
            );
        }
    }
    Ok(())
}

/// Advisories are emitted only once the command has completed successfully so
/// every failure keeps its established error-code-first stderr contract.
fn emit_metadata_warnings(warnings: &[synapse_creator::ImageMetadataWarning]) {
    for warning in warnings {
        if warning.check != synapse_creator::ImageMetadataCheck::NoGpsFound {
            eprintln!("warning [{}]: {}", warning.role, warning.message);
        }
    }
}

fn read_generation_note_file(path: &Path) -> Result<CreatorGenerationNote, CliError> {
    let bytes = read_structured(path)?;
    let note: CreatorGenerationNote = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::Usage(format!(
            "generation note file {} must be a UTF-8 JSON object with only tool, model, prompt, and intent string fields: {error}",
            path.display()
        ))
    })?;
    note.validate()?;
    Ok(note)
}

fn inbox_decide(args: &[String]) -> Result<(), CliError> {
    if args.len() < 6 {
        return Err(CliError::Usage(
            "inbox decide requires <inbox-dir> <slug> <repo> and --decision".into(),
        ));
    }
    let mut decision = None;
    let mut rationale = None;
    let mut session = None;
    let mut creator = None;
    let mut index = 4;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("{} requires a value", args[index])))?;
        match args[index].as_str() {
            "--decision" if decision.is_none() => {
                decision = Some(CreatorDisposition::parse(value)?)
            }
            "--rationale" if rationale.is_none() => rationale = Some(value.clone()),
            "--session" if session.is_none() => session = Some(value.clone()),
            "--creator" if creator.is_none() => creator = Some(value.clone()),
            other => {
                return Err(CliError::Usage(format!(
                    "invalid or duplicate inbox decide option {other:?}"
                )));
            }
        }
        index += 2;
    }
    let slug = &args[2];
    let session = session.unwrap_or_else(|| suggested_import_inbox_session(slug));
    // This binds the later ingest to exactly these retained bytes. No Inbox
    // path is passed to creator-run and no repository is opened on failure.
    let retained = retain_import_inbox_candidate(Path::new(&args[1]), slug)?;
    let options = CreatorRunOptions {
        repository: args[3].as_str().into(),
        session: session.clone(),
        original_image: retained.original.clone(),
        current_image: retained.current.clone(),
        ai_output: retained.ai_output.clone(),
        subject_label: retained.manifest.metadata.subject_label.clone(),
        creator_name: creator.unwrap_or(retained.manifest.metadata.creator_name.clone()),
        disposition: decision
            .ok_or_else(|| CliError::Usage("inbox decide requires --decision".into()))?,
        rationale,
    };
    let receipt = run_creator_session_with_note(
        &options,
        retained.manifest.metadata.generation_note.as_ref(),
    )?;
    let _report = creator_report(&options.repository, &options.session).map_err(|source| {
        CliError::CreatorReportUnavailableAfterCommit {
            session: options.session.clone(),
            source,
        }
    })?;
    outln!("session={}", receipt.session);
    outln!(
        "decision_ref={}\t{}",
        receipt.decision_ref,
        receipt.decision_head
    );
    outln!("inbox_candidate={slug}");
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreatorReportFormat {
    Text,
    Json,
}

fn creator_report_command(args: &[String]) -> Result<(), CliError> {
    if args.len() < 3 || (args.len() - 3) % 2 != 0 {
        return Err(CliError::Usage(
            "creator-report expects <repo> <session> [--format text|json]".into(),
        ));
    }
    let mut format = None;
    let mut index = 3;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("{} requires a value", args[index])))?;
        match args[index].as_str() {
            "--format" if format.is_none() => {
                format = Some(match value.as_str() {
                    "text" => CreatorReportFormat::Text,
                    "json" => CreatorReportFormat::Json,
                    other => {
                        return Err(CliError::Usage(format!(
                            "invalid creator-report --format value {other:?}; expected text or json"
                        )));
                    }
                });
            }
            other => {
                return Err(CliError::Usage(format!(
                    "invalid or duplicate creator-report option {other:?}"
                )));
            }
        }
        index += 2;
    }

    // The whole report is verified and built before anything is printed, so
    // a verification failure (returned as `Err` above the match) never
    // leaves a partial line, and never a partial JSON document, on stdout.
    let report = creator_report(&args[1], &args[2])?;
    match format.unwrap_or(CreatorReportFormat::Text) {
        CreatorReportFormat::Text => print_creator_report(&report)?,
        CreatorReportFormat::Json => {
            let document = CreatorReportDocument::from_report(&report);
            // The document is built entirely from primitive, always-valid-UTF8
            // fields copied out of a verified `CreatorReport`, so this cannot
            // practically fail; `expect` keeps a partial document from ever
            // reaching stdout instead of inventing a new error code for an
            // unreachable path.
            let text = document
                .to_pretty_string()
                .expect("creator report JSON document is always serializable");
            out!("{text}");
        }
    }
    Ok(())
}

fn print_creator_report(report: &CreatorReport) -> Result<(), CliError> {
    outln!("report_session={}", report.session);
    outln!("project={}", report.project_id);
    outln!("subject={}", report.subject_id);
    if let Some(label) = &report.subject_label {
        outln!("subject_label={label:?}");
    }
    outln!("proposal_attributed_to_agent={}", report.agent_id);
    outln!("ai_output_source=caller_supplied");
    outln!("reviewed_by_human={}", report.creator_id);
    if let Some(name) = &report.creator_name {
        outln!("creator_name={name:?}");
    }
    outln!("selected={}", report.selected_ai_output);
    outln!("base_head={}", report.base_head);
    outln!("base_snapshot={}", report.base_snapshot);
    outln!("proposal_snapshot={}", report.proposal_snapshot);
    outln!("decision_snapshot={}", report.decision_snapshot);
    outln!(
        "decision_ref={}\t{}",
        report.decision_ref,
        report.decision_head
    );
    outln!(
        "proposal_ref={}\t{}",
        report.proposal_ref,
        report.proposal_head
    );
    outln!("disposition={}", report.disposition.as_cli_str());
    if let Some(recorded_at) = &report.decision_recorded_at {
        outln!("decision_recorded_at={recorded_at}");
    }
    if let Some(source) = &report.source {
        outln!("reused_reference_source={source:?}");
    }
    if let Some(source) = &report.reuse_source {
        // Keep every field escaped: this is a human-readable report surface,
        // not a shell-safe serialization format.
        outln!("reused_three_blob_source_format={:?}", source.format);
        outln!("reused_three_blob_source_kind={:?}", source.kind);
        outln!("reused_three_blob_source_session={:?}", source.session);
        outln!(
            "reused_three_blob_source_proposal_head={:?}",
            source.proposal_head
        );
        outln!(
            "reused_three_blob_source_decision_head={:?}",
            source.decision_head
        );
        outln!(
            "reused_three_blob_source_original_blob_oid={:?}",
            source.original_blob_oid
        );
        outln!(
            "reused_three_blob_source_current_blob_oid={:?}",
            source.current_blob_oid
        );
        outln!(
            "reused_three_blob_source_ai_output_blob_oid={:?}",
            source.ai_output_blob_oid
        );
    }
    if report.annotations_unavailable {
        outln!("decision_pins=unavailable");
    }
    if let Some(annotations) = &report.annotations {
        outln!("decision_pins_private={annotations:?}");
    }
    if let Some(note) = &report.generation_note {
        outln!("generation_note_user_declared={note:?}");
    }
    outln!(
        "rationale={}",
        report.rationale.as_deref().map_or("-", |value| value)
    );
    if let Some(source) = report.rationale_source {
        outln!("rationale_source={}", source.as_str());
    }
    outln!("original={}", report.original_blob_oid);
    outln!("current={}", report.current_blob_oid);
    outln!("ai_output={}", report.ai_output_blob_oid);
    if let Some(comparison) = &report.comparison {
        outln!("comparison_analysis={}", comparison.analysis_oid);
        outln!(
            "comparison_adapter={}@{}",
            comparison.adapter_id,
            comparison.adapter_version
        );
        outln!("comparison_status={}", comparison.status);
        outln!("comparison_comparability={}", comparison.comparability);
        outln!("byte_identity={}", comparison.outcome);
        outln!(
            "comparison_reason_codes={}",
            comparison.reason_codes.join(",")
        );
        outln!("comparison_replay_ready={}", comparison.replay_ready);
        for warning in &comparison.warnings {
            outln!("comparison_warning={warning:?}");
        }
    } else {
        outln!("comparison=unavailable");
    }
    outln!("fsck=clean objects={}", report.fsck_objects);
    outln!("timeline={}", report.timeline.len());
    for entry in &report.timeline {
        outln!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            entry.ordering_time,
            entry.time_basis,
            entry.stage,
            entry.kind,
            entry.entity_id,
            entry.oid,
            entry.reachable_from.join(",")
        );
    }
    Ok(())
}

fn put_blob(args: &[String]) -> Result<(), CliError> {
    let (repo, file, claimed) = parse_put_args(args)?;
    let repository = Repository::open_existing(repo)?;
    let input =
        File::open(file).map_err(|source| CliError::io("open blob input file", file, source))?;
    let result = match claimed {
        Some(oid) => repository.put_blob_claimed(oid, input)?,
        None => repository.put_blob(input)?,
    };
    outln!("{}", result.oid);
    Ok(())
}

fn put_structured(args: &[String], expected: Option<ObjectKind>) -> Result<(), CliError> {
    let (repo, file, claimed) = parse_put_args(args)?;
    let bytes = read_structured(Path::new(file))?;
    let repository = Repository::open_existing(repo)?;
    let result = match (expected, claimed) {
        (Some(kind), Some(oid)) => repository.put_object_claimed_as(kind, oid, &bytes)?,
        (Some(kind), None) => repository.put_object_as(kind, &bytes)?,
        (None, Some(oid)) => repository.put_object_claimed(oid, &bytes)?,
        (None, None) => repository.put_object(&bytes)?,
    };
    outln!("{}", result.oid);
    Ok(())
}

fn parse_put_args(args: &[String]) -> Result<(&str, &str, Option<&str>), CliError> {
    if args.len() == 3 {
        return Ok((&args[1], &args[2], None));
    }
    if args.len() == 5 && args[3] == "--claimed" {
        return Ok((&args[1], &args[2], Some(&args[4])));
    }
    Err(CliError::Usage(format!(
        "{} expects <repo> <file> [--claimed <oid>]",
        args.first().map_or("put", String::as_str)
    )))
}

fn update_ref(args: &[String]) -> Result<(), CliError> {
    if args.len() < 5 {
        return Err(CliError::Usage(
            "update-ref expects <repo> <ref> <expected|-> <new>".into(),
        ));
    }
    let mut actor = None;
    let mut message = None;
    let mut index = 5;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("{} requires a value", args[index])))?;
        match args[index].as_str() {
            "--actor" if actor.is_none() => actor = Some(value.as_str()),
            "--message" if message.is_none() => message = Some(value.as_str()),
            other => {
                return Err(CliError::Usage(format!(
                    "invalid update-ref option {other:?}"
                )));
            }
        }
        index += 2;
    }

    let occurred_at = now_unix_nanos()?;
    let expected = (args[3] != "-").then_some(args[3].as_str());
    let mut repository = Repository::open_existing(&args[1])?;
    repository.update_ref(RefUpdate {
        ref_name: &args[2],
        expected_head: expected,
        new_head: &args[4],
        metadata: ReflogMetadata {
            occurred_at_unix_nanos: occurred_at,
            actor,
            message,
        },
    })?;
    outln!("{}\t{}", args[2], args[4]);
    Ok(())
}

fn now_unix_nanos() -> Result<i64, CliError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| CliError::Clock(format!("system clock error: {error}")))?
        .as_nanos();
    i64::try_from(nanos)
        .map_err(|_| CliError::Clock("current time exceeds reflog i64 nanosecond range".into()))
}

fn read_structured(path: &Path) -> Result<Vec<u8>, CliError> {
    let file = File::open(path)
        .map_err(|source| CliError::io("open structured input file", path, source))?;
    let mut bytes = Vec::new();
    file.take(DEFAULT_MAX_STRUCTURED_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| CliError::io("read structured input file", path, source))?;
    if bytes.len() > DEFAULT_MAX_STRUCTURED_BYTES {
        return Err(CliError::Usage(format!(
            "{} exceeds the structured input limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn require_len(args: &[String], expected: usize) -> Result<(), CliError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{} received the wrong number of arguments",
            args.first().map_or("command", String::as_str)
        )))
    }
}
