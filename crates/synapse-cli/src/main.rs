#![forbid(unsafe_code)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let argv0 = arguments
        .next()
        .unwrap_or_else(|| OsString::from("synapse"));
    let arguments = match arguments
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(arguments) => arguments,
        Err(_) => {
            eprintln!("usage_error: command-line arguments must be valid Unicode");
            return ExitCode::from(1);
        }
    };
    let program = Path::new(&argv0).file_name().unwrap_or(argv0.as_os_str());
    match program {
        name if name == OsStr::new("synapse-local") => {
            run_local(arguments, "synapse-local", "synapse-local", "synapse-local")
        }
        name if name == OsStr::new("synapse-present") => {
            synapse_publication::run_cli_as(arguments, "synapse-present", "synapse-present")
        }
        _ => run_synapse(arguments),
    }
}

fn run_synapse(mut arguments: Vec<String>) -> ExitCode {
    match arguments.first().map(String::as_str) {
        Some("serve") => {
            arguments.remove(0);
            run_local(arguments, "synapse serve", "synapse", "synapse serve")
        }
        Some("present") => {
            arguments.remove(0);
            synapse_publication::run_cli_as(arguments, "synapse present", "synapse")
        }
        _ => synapse_cli::run_cli(arguments),
    }
}

fn run_local(
    arguments: Vec<String>,
    help_program: &str,
    version_program: &str,
    error_program: &str,
) -> ExitCode {
    if let Some(status) =
        synapse_local_http::immediate_exit(&arguments, help_program, version_program, error_program)
    {
        return status;
    }
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(synapse_local_http::run_cli_as(
            arguments,
            help_program,
            version_program,
            error_program,
        )),
        Err(error) => {
            eprintln!("{error_program}: could not start runtime: {error}");
            ExitCode::from(1)
        }
    }
}
