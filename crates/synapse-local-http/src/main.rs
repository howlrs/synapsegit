use std::collections::BTreeMap;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use synapse_local_http::build_local_application;
use synapse_local_service::{LocalService, ProjectRegistration};

const DEFAULT_PORT: u16 = 8787;

#[tokio::main]
async fn main() {
    match run().await {
        Ok(()) => {}
        Err(RunError::Help) => print_help(),
        Err(RunError::Version) => print_version(),
        Err(RunError::Failure(error)) => {
            eprintln!("synapse-local: {error}");
            std::process::exit(1);
        }
    }
}

async fn run() -> Result<(), RunError> {
    let cli = parse_args(std::env::args().skip(1))?;
    let registrations = cli
        .projects
        .into_iter()
        .map(|(key, path)| {
            let label = cli.labels.get(&key).cloned().unwrap_or_else(|| key.clone());
            ProjectRegistration::new(key, label, path)
        })
        .collect::<Vec<_>>();
    let mut service =
        LocalService::new(registrations).map_err(|error| RunError::failure(error.to_string()))?;
    if let Some(archive_root) = cli.archive_root {
        // Fail closed at startup, matching the exact-catalog project paths:
        // an archive root that does not exist or is not a directory is a
        // misconfiguration, not "no archives yet" (an existing-but-empty
        // directory is the supported way to say that).
        let metadata = std::fs::metadata(&archive_root).map_err(|error| {
            RunError::failure(format!(
                "--archive-root {} is not accessible: {error}",
                archive_root.display()
            ))
        })?;
        if !metadata.is_dir() {
            return Err(RunError::failure(format!(
                "--archive-root {} is not a directory",
                archive_root.display()
            )));
        }
        let canonical = std::fs::canonicalize(&archive_root).map_err(|error| {
            RunError::failure(format!(
                "--archive-root {} could not be canonicalized: {error}",
                archive_root.display()
            ))
        })?;
        service = service.with_archive_root(canonical);
    }
    let service = Arc::new(service);

    // The host is deliberately not configurable. Port zero is accepted only
    // as an OS-selected development port and is resolved before router setup.
    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, cli.port))
        .await
        .map_err(|_| RunError::failure("could not bind the IPv4 loopback listener"))?;
    let address = listener
        .local_addr()
        .map_err(|_| RunError::failure("could not inspect the loopback listener"))?;
    if !address.ip().is_loopback() {
        return Err(RunError::failure(
            "refusing to start on a non-loopback listener",
        ));
    }
    let application = build_local_application(service, address.port())
        .map_err(|error| RunError::failure(error.to_string()))?;
    eprintln!("SynapseGit Local is available at {}", application.origin());
    eprintln!("Press Ctrl-C to stop. No network sharing is enabled.");

    axum::serve(listener, application.into_router())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|_| RunError::failure("the loopback HTTP server stopped unexpectedly"))?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[derive(Debug, Eq, PartialEq)]
struct Cli {
    port: u16,
    projects: Vec<(String, PathBuf)>,
    labels: BTreeMap<String, String>,
    archive_root: Option<PathBuf>,
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Cli, RunError> {
    let mut arguments = arguments.into_iter();
    let mut port = DEFAULT_PORT;
    let mut projects = Vec::new();
    let mut labels = BTreeMap::new();
    let mut archive_root = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => return Err(RunError::Help),
            "-V" | "--version" => return Err(RunError::Version),
            "--port" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| RunError::failure("--port requires a value"))?;
                port = value
                    .parse()
                    .map_err(|_| RunError::failure("--port must be between 0 and 65535"))?;
            }
            "--project" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| RunError::failure("--project requires key=path"))?;
                let (key, path) = split_assignment(&value, "--project requires key=path")?;
                if projects
                    .iter()
                    .any(|(registered_key, _)| registered_key == key)
                {
                    return Err(RunError::failure("duplicate --project key"));
                }
                projects.push((key.to_owned(), PathBuf::from(path)));
            }
            "--label" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| RunError::failure("--label requires key=display-label"))?;
                let (key, label) = split_assignment(&value, "--label requires key=display-label")?;
                if labels.insert(key.to_owned(), label.to_owned()).is_some() {
                    return Err(RunError::failure("duplicate --label project key"));
                }
            }
            "--archive-root" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| RunError::failure("--archive-root requires a value"))?;
                if archive_root.is_some() {
                    return Err(RunError::failure("--archive-root may only be given once"));
                }
                archive_root = Some(PathBuf::from(value));
            }
            _ => return Err(RunError::failure("unknown command-line option")),
        }
    }
    if projects.is_empty() {
        return Err(RunError::failure(
            "at least one --project key=path registration is required",
        ));
    }
    for key in labels.keys() {
        if !projects.iter().any(|(project, _)| project == key) {
            return Err(RunError::failure(
                "--label refers to an unregistered project key",
            ));
        }
    }
    Ok(Cli {
        port,
        projects,
        labels,
        archive_root,
    })
}

fn split_assignment<'a>(value: &'a str, message: &str) -> Result<(&'a str, &'a str), RunError> {
    let (key, assigned) = value
        .split_once('=')
        .ok_or_else(|| RunError::failure(message))?;
    if key.is_empty() || assigned.is_empty() {
        return Err(RunError::failure(message));
    }
    Ok((key, assigned))
}

#[derive(Debug)]
enum RunError {
    Help,
    Version,
    Failure(Box<dyn Error + Send + Sync>),
}

impl RunError {
    fn failure(message: impl Into<String>) -> Self {
        Self::Failure(std::io::Error::other(message.into()).into())
    }
}

fn print_help() {
    println!(
        "SynapseGit Local\n\nUsage:\n  synapse-local --project KEY=PATH [--label KEY=LABEL] [--archive-root PATH] [--port PORT]\n\nThe server always binds to 127.0.0.1. --project may be repeated.\n--archive-root PATH enables read-only GET /api/v1/archives listing of the\ndirectories directly under PATH; the path must already exist and be a\ndirectory. Without it, archive listing always returns an empty list."
    );
}

fn print_version() {
    println!("synapse-local {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn cli_accepts_exact_project_registrations_without_a_host_option() {
        let cli = parse_args(strings(&[
            "--project",
            "demo=/tmp/demo",
            "--label",
            "demo=Demo project",
            "--port",
            "0",
        ]))
        .unwrap();
        assert_eq!(cli.port, 0);
        assert_eq!(cli.projects, [("demo".into(), PathBuf::from("/tmp/demo"))]);
        assert_eq!(cli.labels.get("demo").unwrap(), "Demo project");
        assert_eq!(cli.archive_root, None);
    }

    #[test]
    fn cli_accepts_an_archive_root_and_rejects_a_repeated_one() {
        let cli = parse_args(strings(&[
            "--project",
            "demo=/tmp/demo",
            "--archive-root",
            "/tmp/archives",
        ]))
        .unwrap();
        assert_eq!(cli.archive_root, Some(PathBuf::from("/tmp/archives")));

        assert!(matches!(
            parse_args(strings(&[
                "--project",
                "demo=/tmp/demo",
                "--archive-root",
                "/tmp/archives",
                "--archive-root",
                "/tmp/other-archives",
            ])),
            Err(RunError::Failure(_))
        ));

        assert!(matches!(
            parse_args(strings(&["--project", "demo=/tmp/demo", "--archive-root"])),
            Err(RunError::Failure(_))
        ));
    }

    #[test]
    fn cli_recognizes_the_version_flag_without_a_project() {
        assert!(matches!(
            parse_args(strings(&["--version"])),
            Err(RunError::Version)
        ));
        assert!(matches!(
            parse_args(strings(&["-V"])),
            Err(RunError::Version)
        ));
    }

    #[test]
    fn cli_rejects_missing_projects_unknown_options_and_orphan_labels() {
        assert!(matches!(parse_args(Vec::new()), Err(RunError::Failure(_))));
        assert!(matches!(
            parse_args(strings(&["--host", "0.0.0.0"])),
            Err(RunError::Failure(_))
        ));
        assert!(matches!(
            parse_args(strings(&[
                "--project",
                "demo=/tmp/demo",
                "--label",
                "other=Other"
            ])),
            Err(RunError::Failure(_))
        ));
    }

    #[test]
    fn cli_rejects_duplicate_project_keys() {
        assert!(matches!(
            parse_args(strings(&[
                "--project",
                "demo=/tmp/first",
                "--project",
                "demo=/tmp/second"
            ])),
            Err(RunError::Failure(_))
        ));
    }
}
