#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_canonical::{DEFAULT_MAX_STRUCTURED_BYTES, ObjectKind};
use synapse_core::{Repository, RepositoryError};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

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
";

#[derive(Debug)]
enum CliError {
    Usage(String),
    Io { path: String, source: io::Error },
    Core(RepositoryError),
    Clock(String),
    FsckFailed,
}

impl CliError {
    fn code(&self) -> &str {
        match self {
            Self::Usage(_) => "usage_error",
            Self::Io { .. } | Self::Clock(_) => "storage_error",
            Self::Core(error) => error.code(),
            Self::FsckFailed => "fsck_failed",
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::Io { path, source } => write!(formatter, "{path}: {source}"),
            Self::Core(error) => error.fmt(formatter),
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
            _ => None,
        }
    }
}

impl From<RepositoryError> for CliError {
    fn from(error: RepositoryError) -> Self {
        Self::Core(error)
    }
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            if matches!(error, CliError::Usage(_)) {
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
            Repository::open(&args[1])?;
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
            let repository = Repository::open(&args[1])?;
            for record in repository.refs().list().map_err(RepositoryError::from)? {
                println!("{}\t{}", record.name, record.head);
            }
        }
        "fsck" => {
            require_len(&args, 2)?;
            let repository = Repository::open(&args[1])?;
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
            let mut repository = Repository::open(&args[1])?;
            repository.export_archive(&args[2])?;
            println!("exported {}", args[2]);
        }
        "restore" => {
            require_len(&args, 3)?;
            Repository::restore_archive(&args[1], &args[2])?;
            println!("restored {}", args[2]);
        }
        "help" | "--help" | "-h" => println!("{USAGE}"),
        other => return Err(CliError::Usage(format!("unknown command {other:?}"))),
    }
    Ok(())
}

fn put_blob(args: &[String]) -> Result<(), CliError> {
    let (repo, file, claimed) = parse_put_args(args)?;
    let repository = Repository::open(repo)?;
    let input = File::open(file).map_err(|source| CliError::Io {
        path: file.to_owned(),
        source,
    })?;
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
    let repository = Repository::open(repo)?;
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
    let mut repository = Repository::open(&args[1])?;
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
    let file = File::open(path).map_err(|source| CliError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(DEFAULT_MAX_STRUCTURED_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| CliError::Io {
            path: path.display().to_string(),
            source,
        })?;
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
