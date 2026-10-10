#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    synapse_publication::run_cli(std::env::args().skip(1).collect())
}
