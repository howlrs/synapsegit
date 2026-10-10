#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use synapse_core::Repository;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "synapse-unified-cli-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_synapse"))
}

fn run(arguments: &[&str]) -> Output {
    Command::new(binary()).args(arguments).output().unwrap()
}

#[test]
fn canonical_dispatch_uses_canonical_help_and_one_synapse_version() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("synapse serve --project"));
    assert!(help.contains("synapse present export"));

    for arguments in [
        ["--version"].as_slice(),
        ["serve", "--version"].as_slice(),
        ["present", "--version"].as_slice(),
    ] {
        let output = run(arguments);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("synapse {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    let serve_help = run(&["serve", "--help"]);
    assert!(serve_help.status.success());
    assert!(
        String::from_utf8(serve_help.stdout)
            .unwrap()
            .contains("synapse serve --project")
    );
    let present_help = run(&["present", "--help"]);
    assert!(present_help.status.success());
    assert!(
        String::from_utf8(present_help.stdout)
            .unwrap()
            .contains("synapse present export")
    );
}

#[test]
fn basename_aliases_keep_the_legacy_contract_without_resolving_symlinks() {
    let temporary = TempDirectory::new();
    let local = temporary.join("synapse-local");
    let present = temporary.join("synapse-present");
    symlink(binary(), &local).unwrap();
    symlink(binary(), &present).unwrap();

    let local_help = Command::new(&local).arg("--help").output().unwrap();
    assert!(local_help.status.success());
    assert!(
        String::from_utf8(local_help.stdout)
            .unwrap()
            .contains("synapse-local --project")
    );
    let local_version = Command::new(&local).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8(local_version.stdout).unwrap(),
        format!("synapse-local {}\n", env!("CARGO_PKG_VERSION"))
    );
    let local_error = Command::new("./synapse-local")
        .current_dir(&temporary.0)
        .arg("--unknown")
        .output()
        .unwrap();
    assert!(
        String::from_utf8(local_error.stderr)
            .unwrap()
            .starts_with("synapse-local: unknown command-line option")
    );
    let path_local = Command::new("synapse-local")
        .env("PATH", &temporary.0)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(path_local.stdout).unwrap(),
        format!("synapse-local {}\n", env!("CARGO_PKG_VERSION"))
    );

    let present_help = Command::new(&present).arg("--help").output().unwrap();
    assert!(present_help.status.success());
    assert!(
        String::from_utf8(present_help.stdout)
            .unwrap()
            .contains("synapse-present export")
    );
    let present_version = Command::new(&present).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8(present_version.stdout).unwrap(),
        format!("synapse-present {}\n", env!("CARGO_PKG_VERSION"))
    );
    let present_error = Command::new(&present).arg("unknown").output().unwrap();
    assert!(
        String::from_utf8(present_error.stderr)
            .unwrap()
            .starts_with("usage_error: unknown command \"unknown\"")
    );
}

#[test]
fn canonical_present_export_and_preview_are_dispatched_separately_from_core_export() {
    let temporary = TempDirectory::new();
    let repository = temporary.join("repository");
    Repository::open(&repository).unwrap();
    let bundle = temporary.join("bundle");

    let export = run(&[
        "present",
        "export",
        repository.to_str().unwrap(),
        bundle.to_str().unwrap(),
    ]);
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(bundle.join("projection.json").is_file());
    let preview = run(&["present", "preview", bundle.to_str().unwrap()]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );

    let core_export = run(&[
        "export",
        repository.to_str().unwrap(),
        temporary.join("archive").to_str().unwrap(),
    ]);
    assert!(
        core_export.status.success(),
        "{}",
        String::from_utf8_lossy(&core_export.stderr)
    );
    assert!(temporary.join("archive/manifest.json").is_file());
}

#[test]
fn local_errors_use_the_invocation_label() {
    let canonical = run(&["serve", "--unknown"]);
    assert_eq!(canonical.status.code(), Some(1));
    assert!(
        String::from_utf8(canonical.stderr)
            .unwrap()
            .starts_with("synapse serve: unknown command-line option")
    );
}

#[test]
fn closed_stdout_is_successful_for_canonical_help_routes() {
    for arguments in [
        &["--help"][..],
        &["serve", "--help"],
        &["present", "--help"],
    ] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let output = Command::new(binary())
            .args(arguments)
            .stdout(writer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let temporary = TempDirectory::new();
    let local = temporary.join("synapse-local");
    let present = temporary.join("synapse-present");
    symlink(binary(), &local).unwrap();
    symlink(binary(), &present).unwrap();
    for program in [&local, &present] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let output = Command::new(program)
            .arg("--help")
            .stdout(writer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn serve_starts_only_on_ipv4_loopback() {
    let temporary = TempDirectory::new();
    let repository = temporary.join("repository");
    Repository::open(&repository).unwrap();
    let mut child = Command::new(binary())
        .args([
            "serve",
            "--project",
            &format!("demo={}", repository.display()),
            "--port",
            "0",
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (origin_sender, origin_receiver) = mpsc::channel();
    let stderr_reader = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            if let Some(origin) = line.strip_prefix("SynapseGit Local is available at ") {
                let _ = origin_sender.send(origin.to_owned());
            }
        }
    });

    let origin = origin_receiver.recv_timeout(Duration::from_secs(10));
    let probe = origin
        .as_ref()
        .ok()
        .and_then(|origin| origin.strip_prefix("http://"))
        .map(TcpStream::connect);

    let _ = child.kill();
    let status = child.wait().unwrap();
    stderr_reader.join().unwrap();

    let origin = origin.expect("server did not report its loopback origin within ten seconds");
    assert!(
        origin.strip_prefix("http://127.0.0.1:").is_some(),
        "unexpected server origin {origin:?}"
    );
    assert!(
        probe.expect("origin must be an HTTP URL").is_ok(),
        "server did not accept a TCP connection at {origin} (status: {status})"
    );
}
