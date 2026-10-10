#![forbid(unsafe_code)]

use crate::{
    ExportOptions, OutputTarget, ProjectionOptions, PublicationError, PublicationLocale,
    PublicationVisibility, export_bundle, load_presentation, verify_bundle,
};
use std::env;
use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
SynapseGit local publication bundle

Usage:
  synapse-present export <repo> <output-dir> [--session <id>] [--presentation <presentation.toml>] [--public] [--locale <en|ja>] [--replace] [--target <synapse|github> | --synapse | --github]
  synapse-present preview <bundle-dir>
";

/// Write one stdout line without panicking.  When the reader has already
/// closed the pipe, the rest of the output is discarded and the command still
/// finishes with its own exit status.  Any other write failure is a
/// `storage_error`.
fn write_stdout_line(arguments: fmt::Arguments<'_>) -> Result<(), CliError> {
    match writeln!(io::stdout(), "{arguments}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(CliError::Stdout(error)),
        _ => Ok(()),
    }
}

macro_rules! outln {
    ($($arg:tt)*) => {
        write_stdout_line(format_args!($($arg)*))?
    };
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Publication(PublicationError),
    Stdout(io::Error),
}

impl CliError {
    fn code(&self) -> &str {
        match self {
            Self::Usage(_) => "usage_error",
            Self::Publication(error) => error.code(),
            Self::Stdout(_) => "storage_error",
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::Publication(error) => error.fmt(formatter),
            Self::Stdout(source) => write!(formatter, "write standard output: {source}"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Publication(error) => Some(error),
            Self::Stdout(source) => Some(source),
            Self::Usage(_) => None,
        }
    }
}

impl From<PublicationError> for CliError {
    fn from(error: PublicationError) -> Self {
        Self::Publication(error)
    }
}

pub fn run_cli(args: Vec<String>) -> ExitCode {
    run_cli_as(args, "synapse-present", "synapse-present")
}

pub fn run_cli_as(args: Vec<String>, help_program: &str, version_program: &str) -> ExitCode {
    match run(args, help_program, version_program) {
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

fn run(args: Vec<String>, help_program: &str, version_program: &str) -> Result<(), CliError> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(CliError::Usage("a command is required".into()));
    };
    match command {
        "export" => export(&args),
        "preview" => preview(&args),
        "help" | "--help" | "-h" => {
            if args.len() != 1 {
                return Err(CliError::Usage(
                    "help does not accept additional arguments".into(),
                ));
            }
            outln!("{}", USAGE.replace("synapse-present", help_program));
            Ok(())
        }
        "version" | "--version" | "-V" => {
            if args.len() != 1 {
                return Err(CliError::Usage(
                    "version does not accept additional arguments".into(),
                ));
            }
            outln!("{version_program} {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(CliError::Usage(format!("unknown command {other:?}"))),
    }
}

fn export(args: &[String]) -> Result<(), CliError> {
    if args.len() < 3 {
        return Err(CliError::Usage(
            "export requires <repo> and <output-dir>".into(),
        ));
    }

    let mut session = None::<String>;
    let mut presentation = None::<PathBuf>;
    let mut visibility = PublicationVisibility::PrivateReview;
    let mut public_selected = false;
    let mut target = None::<OutputTarget>;
    let mut locale = None::<PublicationLocale>;
    let mut replace = false;
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--session" => {
                let value = option_value(args, index, "--session")?;
                if session.replace(value.to_owned()).is_some() {
                    return Err(CliError::Usage(
                        "duplicate export option \"--session\"".into(),
                    ));
                }
                index += 2;
            }
            "--presentation" => {
                let value = option_value(args, index, "--presentation")?;
                if presentation.replace(PathBuf::from(value)).is_some() {
                    return Err(CliError::Usage(
                        "duplicate export option \"--presentation\"".into(),
                    ));
                }
                index += 2;
            }
            "--public" => {
                if public_selected {
                    return Err(CliError::Usage(
                        "duplicate export option \"--public\"".into(),
                    ));
                }
                public_selected = true;
                visibility = PublicationVisibility::Public;
                index += 1;
            }
            "--target" => {
                let value = option_value(args, index, "--target")?;
                select_target(&mut target, parse_target(value)?, "--target")?;
                index += 2;
            }
            "--synapse" => {
                select_target(&mut target, OutputTarget::Synapse, "--synapse")?;
                index += 1;
            }
            "--github" => {
                select_target(&mut target, OutputTarget::Github, "--github")?;
                index += 1;
            }
            "--locale" => {
                let value = option_value(args, index, "--locale")?;
                let parsed = match value {
                    "en" => PublicationLocale::En,
                    "ja" => PublicationLocale::Ja,
                    _ => return Err(CliError::Usage("--locale must be en or ja".into())),
                };
                if locale.replace(parsed).is_some() {
                    return Err(CliError::Usage(
                        "duplicate export option \"--locale\"".into(),
                    ));
                }
                index += 2;
            }
            "--replace" => {
                if replace {
                    return Err(CliError::Usage(
                        "duplicate export option \"--replace\"".into(),
                    ));
                }
                replace = true;
                index += 1;
            }
            other => {
                return Err(CliError::Usage(format!("unknown export option {other:?}")));
            }
        }
    }

    let mut projection = ProjectionOptions::new(&args[1]);
    projection.session = session;
    projection.visibility = visibility;
    if let Some(path) = presentation {
        projection.presentation = load_presentation(path)?;
    }
    let receipt = export_bundle(&ExportOptions {
        projection,
        destination: PathBuf::from(&args[2]),
        target: target.unwrap_or(OutputTarget::Synapse),
        locale,
        replace,
    })?;
    outln!("exported={}", receipt.destination.display());
    outln!("target={}", receipt.target.as_str());
    outln!("visibility={}", receipt.visibility.as_str());
    outln!("projection_sha256={}", receipt.projection_sha256);
    outln!("sessions={}", receipt.sessions_exported);
    outln!("incomplete_sessions={}", receipt.incomplete_sessions);
    if let Some(path) = receipt.replacement_recovery_path {
        outln!("replacement_recovery_path={}", path.display());
    }
    if let Some(warning) = receipt.sync_warning {
        outln!("sync_warning={warning}");
    }
    Ok(())
}

fn preview(args: &[String]) -> Result<(), CliError> {
    if args.len() != 2 {
        return Err(CliError::Usage(
            "preview requires exactly one <bundle-dir>".into(),
        ));
    }
    let root = Path::new(&args[1]);
    let verified = verify_bundle(root)?;
    outln!("target={}", verified.manifest.target.as_str());
    outln!("visibility={}", verified.manifest.visibility.as_str());
    outln!("projection_sha256={}", verified.manifest.projection_sha256);
    outln!(
        "index_path={}",
        root.join(&verified.manifest.html_path).display()
    );
    Ok(())
}

fn option_value<'a>(
    args: &'a [String],
    option_index: usize,
    option: &str,
) -> Result<&'a str, CliError> {
    let value = args
        .get(option_index + 1)
        .ok_or_else(|| CliError::Usage(format!("{option} requires a value")))?;
    if value.starts_with("--") {
        return Err(CliError::Usage(format!("{option} requires a value")));
    }
    Ok(value)
}

fn parse_target(value: &str) -> Result<OutputTarget, CliError> {
    match value {
        "synapse" => Ok(OutputTarget::Synapse),
        "github" => Ok(OutputTarget::Github),
        _ => Err(CliError::Usage(
            "--target must be exactly \"synapse\" or \"github\"".into(),
        )),
    }
}

fn select_target(
    selected: &mut Option<OutputTarget>,
    target: OutputTarget,
    option: &str,
) -> Result<(), CliError> {
    if selected.is_some() {
        return Err(CliError::Usage(format!(
            "output target selectors are mutually exclusive and may appear only once; repeated at {option:?}"
        )));
    }
    *selected = Some(target);
    Ok(())
}
