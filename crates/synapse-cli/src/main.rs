#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_canonical::{DEFAULT_MAX_STRUCTURED_BYTES, ObjectKind};
use synapse_core::{Repository, RepositoryError};
use synapse_creator::{
    CreatorDisposition, CreatorError, CreatorGenerationNote, CreatorReport, CreatorRunOptions,
    ImportInboxCandidate, creator_report, put_import_inbox_candidate,
    run_creator_session_with_note,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

mod report_json;
use report_json::CreatorReportDocument;

const USAGE: &str = "\
SynapseGit Core Stage 0

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
  synapse inbox put <inbox-dir> <slug> <original> <current> <ai-output> --subject <label> --creator <name> [--generation-note-file <path>] [--format text|json]

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

inbox put writes one candidate for the synapse-local import inbox. It needs no
repository, records no decision, and creates no Proposal: a person reviews the
candidate and decides in the localhost UI started with --import-root. Run
`synapse inbox --help` for details.
";
const INBOX_USAGE: &str = "\
Usage:
  synapse inbox put <inbox-dir> <slug> <original> <current> <ai-output> --subject <label> --creator <name> [--generation-note-file <path>] [--format text|json]

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
";
const VERSION: &str = concat!("synapse ", env!("CARGO_PKG_VERSION"));

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
    FsckFailed,
}

impl CliError {
    fn code(&self) -> &str {
        match self {
            Self::Usage(_) => "usage_error",
            Self::InitTargetNotEmpty(_) => "repository_not_empty",
            Self::Io { .. } | Self::Clock(_) => "storage_error",
            Self::Core(error) => error.code(),
            Self::Creator(error) => error.code(),
            Self::CreatorReportUnavailableAfterCommit { .. } => {
                "creator_report_unavailable_after_commit"
            }
            Self::FsckFailed => "fsck_failed",
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
            Self::FsckFailed => formatter.write_str("fsck found integrity issues"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
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
    match command {
        "init" => {
            require_len(&args, 2)?;
            init_repository(Path::new(&args[1]))?;
            println!("initialized {}", args[1]);
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
                println!("{}\t{}", record.name, record.head);
            }
        }
        "fsck" => {
            require_len(&args, 2)?;
            let repository = Repository::open_existing(&args[1])?;
            let report = repository.fsck()?;
            println!(
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
            println!("exported {}", args[2]);
        }
        "restore" => {
            require_len(&args, 3)?;
            Repository::restore_archive(&args[1], &args[2])?;
            println!("restored {}", args[2]);
        }
        "creator-run" => creator_run(&args)?,
        "creator-report" => creator_report_command(&args)?,
        "inbox" => inbox_command(&args)?,
        "help" | "--help" | "-h" => println!("{USAGE}"),
        "version" | "--version" | "-V" => println!("{VERSION}"),
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
    let receipt = run_creator_session_with_note(&options, generation_note.as_ref())?;
    let report = creator_report(&options.repository, &options.session).map_err(|source| {
        CliError::CreatorReportUnavailableAfterCommit {
            session: options.session.clone(),
            source,
        }
    })?;
    println!("session={}", receipt.session);
    println!("subject={}", receipt.subject_id);
    println!("original={}", receipt.original_blob_oid);
    println!("current={}", receipt.current_blob_oid);
    println!("ai_output={}", receipt.ai_output_blob_oid);
    println!(
        "proposal_ref={}\t{}",
        receipt.proposal_ref, receipt.proposal_head
    );
    println!(
        "decision_ref={}\t{}",
        receipt.decision_ref, receipt.decision_head
    );
    println!("disposition={}", receipt.disposition.as_cli_str());
    print_creator_report(&report);
    Ok(())
}

const INBOX_PUT_FORMAT: &str = "synapsegit-cli-inbox-put-v1";
const INBOX_NEXT_STEP: &str = "start synapse-local with --import-root PROJECT=<inbox-dir>, open the project page, and let a person review the candidate and record the decision";

fn inbox_command(args: &[String]) -> Result<(), CliError> {
    let is_help = |value: Option<&String>| {
        matches!(value.map(String::as_str), Some("--help" | "-h" | "help"))
    };
    if is_help(args.get(1))
        || (args.get(1).is_some_and(|value| value == "put") && is_help(args.get(2)))
    {
        print!("{INBOX_USAGE}");
        return Ok(());
    }
    if args.get(1).map(String::as_str) != Some("put") {
        return Err(CliError::Usage("inbox requires the put subcommand".into()));
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
    let manifest = &receipt.manifest;
    match format.unwrap_or(CreatorReportFormat::Text) {
        CreatorReportFormat::Text => {
            println!("inbox_candidate={}", args[3]);
            println!("path={}", receipt.directory.display());
            for (role, file) in [
                ("original", &manifest.original),
                ("current", &manifest.current),
                ("ai_output", &manifest.ai_output),
            ] {
                println!("{role}_size={}", file.size);
                println!("{role}_sha256={}", file.sha256.as_deref().unwrap_or(""));
            }
            println!("decision_recorded=false");
            println!("next={INBOX_NEXT_STEP}");
        }
        CreatorReportFormat::Json => {
            let document = serde_json::json!({
                "format": INBOX_PUT_FORMAT,
                "slug": args[3],
                "path": receipt.directory.display().to_string(),
                "manifest": manifest,
                "decision_recorded": false,
                "next": INBOX_NEXT_STEP,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&document).expect("serializable inbox receipt")
            );
        }
    }
    Ok(())
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
        CreatorReportFormat::Text => print_creator_report(&report),
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
            print!("{text}");
        }
    }
    Ok(())
}

fn print_creator_report(report: &CreatorReport) {
    println!("report_session={}", report.session);
    println!("project={}", report.project_id);
    println!("subject={}", report.subject_id);
    if let Some(label) = &report.subject_label {
        println!("subject_label={label:?}");
    }
    println!("proposal_attributed_to_agent={}", report.agent_id);
    println!("ai_output_source=caller_supplied");
    println!("reviewed_by_human={}", report.creator_id);
    if let Some(name) = &report.creator_name {
        println!("creator_name={name:?}");
    }
    println!("selected={}", report.selected_ai_output);
    println!("base_head={}", report.base_head);
    println!("base_snapshot={}", report.base_snapshot);
    println!("proposal_snapshot={}", report.proposal_snapshot);
    println!("decision_snapshot={}", report.decision_snapshot);
    println!(
        "decision_ref={}\t{}",
        report.decision_ref, report.decision_head
    );
    println!(
        "proposal_ref={}\t{}",
        report.proposal_ref, report.proposal_head
    );
    println!("disposition={}", report.disposition.as_cli_str());
    if let Some(recorded_at) = &report.decision_recorded_at {
        println!("decision_recorded_at={recorded_at}");
    }
    if let Some(source) = &report.source {
        println!("reused_reference_source={source:?}");
    }
    if let Some(source) = &report.reuse_source {
        // Keep every field escaped: this is a human-readable report surface,
        // not a shell-safe serialization format.
        println!("reused_three_blob_source_format={:?}", source.format);
        println!("reused_three_blob_source_kind={:?}", source.kind);
        println!("reused_three_blob_source_session={:?}", source.session);
        println!(
            "reused_three_blob_source_proposal_head={:?}",
            source.proposal_head
        );
        println!(
            "reused_three_blob_source_decision_head={:?}",
            source.decision_head
        );
        println!(
            "reused_three_blob_source_original_blob_oid={:?}",
            source.original_blob_oid
        );
        println!(
            "reused_three_blob_source_current_blob_oid={:?}",
            source.current_blob_oid
        );
        println!(
            "reused_three_blob_source_ai_output_blob_oid={:?}",
            source.ai_output_blob_oid
        );
    }
    if report.annotations_unavailable {
        println!("decision_pins=unavailable");
    }
    if let Some(annotations) = &report.annotations {
        println!("decision_pins_private={annotations:?}");
    }
    if let Some(note) = &report.generation_note {
        println!("generation_note_user_declared={note:?}");
    }
    if let Some(rationale) = &report.rationale {
        println!("rationale={rationale:?}");
    }
    println!("original={}", report.original_blob_oid);
    println!("current={}", report.current_blob_oid);
    println!("ai_output={}", report.ai_output_blob_oid);
    if let Some(comparison) = &report.comparison {
        println!("comparison_analysis={}", comparison.analysis_oid);
        println!(
            "comparison_adapter={}@{}",
            comparison.adapter_id, comparison.adapter_version
        );
        println!("comparison_status={}", comparison.status);
        println!("comparison_comparability={}", comparison.comparability);
        println!("byte_identity={}", comparison.outcome);
        println!(
            "comparison_reason_codes={}",
            comparison.reason_codes.join(",")
        );
        println!("comparison_replay_ready={}", comparison.replay_ready);
        for warning in &comparison.warnings {
            println!("comparison_warning={warning:?}");
        }
    } else {
        println!("comparison=unavailable");
    }
    println!("fsck=clean objects={}", report.fsck_objects);
    println!("timeline={}", report.timeline.len());
    for entry in &report.timeline {
        println!(
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
    println!("{}", result.oid);
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
    println!("{}", result.oid);
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
    println!("{}\t{}", args[2], args[4]);
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
