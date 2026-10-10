#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(status) = synapse_local_http::immediate_exit(
        &arguments,
        "synapse-local",
        "synapse-local",
        "synapse-local",
    ) {
        return status;
    }
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(synapse_local_http::run_cli(arguments)),
        Err(error) => {
            eprintln!("synapse-local: could not start runtime: {error}");
            std::process::ExitCode::from(1)
        }
    }
}
