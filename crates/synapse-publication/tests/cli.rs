use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        loop {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "synapse-present-cli-{label}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create {}: {error}", path.display()),
            }
        }
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_synapse-present"))
}

fn run(arguments: &[String]) -> Output {
    command().args(arguments).output().unwrap()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn empty_repository(path: &Path) {
    drop(Repository::open(path).unwrap());
}

fn export_arguments(repository: &Path, output: &Path, tail: &[&str]) -> Vec<String> {
    let mut arguments = vec![
        "export".to_owned(),
        repository.to_str().unwrap().to_owned(),
        output.to_str().unwrap().to_owned(),
    ];
    arguments.extend(tail.iter().map(|value| (*value).to_owned()));
    arguments
}

fn bundle_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                visit(root, &path, files);
            } else {
                assert!(metadata.is_file());
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn help_and_version_follow_the_companion_binary_contract() {
    let version = run(&strings(&["--version"]));
    assert_success(&version);
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("synapse-present {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(version.stderr.is_empty());

    let help = run(&strings(&["--help"]));
    assert_success(&help);
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("synapse-present export <repo> <output-dir>"));
    assert!(help.contains("synapse-present preview <bundle-dir>"));
    assert!(!help.contains("publish"));
}

#[test]
fn default_and_explicit_target_aliases_are_deterministic() {
    let temporary = TempDirectory::new("target-aliases");
    let repository = temporary.join("repository");
    empty_repository(&repository);

    let default_output = temporary.join("default");
    let synapse_alias_output = temporary.join("synapse-alias");
    let synapse_target_output = temporary.join("synapse-target");
    for (output, tail) in [
        (&default_output, Vec::<&str>::new()),
        (&synapse_alias_output, vec!["--synapse"]),
        (&synapse_target_output, vec!["--target", "synapse"]),
    ] {
        let result = run(&export_arguments(&repository, output, &tail));
        assert_success(&result);
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .contains("target=synapse\n")
        );
    }
    assert_eq!(
        bundle_files(&default_output),
        bundle_files(&synapse_alias_output)
    );
    assert_eq!(
        bundle_files(&synapse_alias_output),
        bundle_files(&synapse_target_output)
    );

    let github_alias_output = temporary.join("github-alias");
    let github_target_output = temporary.join("github-target");
    for (output, tail) in [
        (&github_alias_output, vec!["--github"]),
        (&github_target_output, vec!["--target", "github"]),
    ] {
        let result = run(&export_arguments(&repository, output, &tail));
        assert_success(&result);
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .contains("target=github\n")
        );
    }
    assert_eq!(
        bundle_files(&github_alias_output),
        bundle_files(&github_target_output)
    );
    assert_eq!(
        fs::read(default_output.join("projection.json")).unwrap(),
        fs::read(github_alias_output.join("projection.json")).unwrap(),
        "provider targets must retain one canonical public projection"
    );
}

#[test]
fn repeated_or_combined_target_selectors_fail_before_source_or_output_access() {
    let temporary = TempDirectory::new("target-conflicts");
    let missing_repository = temporary.join("missing-repository");
    let cases = [
        vec!["--github", "--synapse"],
        vec!["--synapse", "--github"],
        vec!["--github", "--github"],
        vec!["--synapse", "--synapse"],
        vec!["--target", "github", "--github"],
        vec!["--target", "synapse", "--synapse"],
        vec!["--target", "github", "--target", "github"],
        vec!["--target", "synapse", "--target", "github"],
    ];

    for (index, tail) in cases.into_iter().enumerate() {
        let output_path = temporary.join(format!("output-{index}"));
        let output = run(&export_arguments(&missing_repository, &output_path, &tail));
        assert!(
            !output.status.success(),
            "case {tail:?} unexpectedly succeeded"
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with("usage_error:"), "{stderr}");
        assert!(stderr.contains("Usage:"), "{stderr}");
        assert!(!output_path.exists());
        assert!(!missing_repository.exists());
    }
}

#[test]
fn invalid_case_sensitive_targets_and_duplicate_options_are_usage_errors() {
    let temporary = TempDirectory::new("usage-errors");
    let missing_repository = temporary.join("missing-repository");
    let presentation_a = temporary.join("a.toml");
    let presentation_b = temporary.join("b.toml");
    let cases = [
        vec!["--target", "GitHub"],
        vec!["--target", "SYNAPSE"],
        vec!["--target"],
        vec!["--target", "--github"],
        vec!["--target=github"],
        vec!["--unknown"],
        vec!["--public", "--public"],
        vec!["--session", "one", "--session", "two"],
        vec![
            "--presentation",
            presentation_a.to_str().unwrap(),
            "--presentation",
            presentation_b.to_str().unwrap(),
        ],
    ];

    for (index, tail) in cases.into_iter().enumerate() {
        let output_path = temporary.join(format!("output-{index}"));
        let output = run(&export_arguments(&missing_repository, &output_path, &tail));
        assert!(
            !output.status.success(),
            "case {tail:?} unexpectedly succeeded"
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with("usage_error:"), "{stderr}");
        assert!(stderr.contains("Usage:"), "{stderr}");
        assert!(!output_path.exists());
        assert!(!missing_repository.exists());
    }
}

#[test]
fn public_presentation_exports_locally_and_preview_prints_only_bundle_metadata() {
    let temporary = TempDirectory::new("preview");
    let repository = temporary.join("repository");
    let destination = temporary.join("bundle");
    let presentation = temporary.join("presentation.toml");
    empty_repository(&repository);
    fs::write(
        &presentation,
        "title = \"Public synthetic history\"\nsummary = \"A local review bundle.\"\n",
    )
    .unwrap();

    let exported = run(&export_arguments(
        &repository,
        &destination,
        &[
            "--presentation",
            presentation.to_str().unwrap(),
            "--public",
            "--github",
        ],
    ));
    assert_success(&exported);
    let exported = String::from_utf8(exported.stdout).unwrap();
    assert!(exported.contains("target=github\n"));
    assert!(exported.contains("visibility=public\n"));
    let digest = exported
        .lines()
        .find_map(|line| line.strip_prefix("projection_sha256="))
        .unwrap();
    assert_eq!(digest.len(), 64);

    let previewed = run(&[
        "preview".to_owned(),
        destination.to_str().unwrap().to_owned(),
    ]);
    assert_success(&previewed);
    assert_eq!(
        String::from_utf8(previewed.stdout).unwrap(),
        format!(
            "target=github\nvisibility=public\nprojection_sha256={digest}\nindex_path={}\n",
            destination.join("index.html").display()
        )
    );
    assert!(previewed.stderr.is_empty());
}

#[test]
fn busy_source_error_is_stable_and_does_not_echo_the_local_repository_path() {
    let temporary = TempDirectory::new("busy-source");
    let repository = temporary.join("private-repository-name");
    let destination = temporary.join("bundle");
    empty_repository(&repository);
    let wal = repository.join("refs.sqlite3-wal");
    fs::write(&wal, b"BUSY_CANARY").unwrap();

    let output = run(&export_arguments(&repository, &destination, &[]));
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("read_only_source_busy:"), "{stderr}");
    assert!(!stderr.contains(repository.to_str().unwrap()), "{stderr}");
    assert_eq!(fs::read(wal).unwrap(), b"BUSY_CANARY");
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn github_export_ignores_credentials_and_never_invokes_git_or_network_tools() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDirectory::new("github-offline");
    let repository = temporary.join("repository");
    let destination = temporary.join("bundle");
    let tools = temporary.join("tools");
    empty_repository(&repository);
    fs::create_dir(&tools).unwrap();
    let mut markers = Vec::new();
    for tool in ["git", "gh", "curl"] {
        let marker = temporary.join(format!("{tool}-invoked"));
        let executable = tools.join(tool);
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf invoked > '{}'\nexit 97\n",
                marker.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        markers.push(marker);
    }

    let output = command()
        .env_clear()
        .env("PATH", &tools)
        .env("GH_TOKEN", "PRIVATE_GH_TOKEN_CANARY")
        .env("GITHUB_TOKEN", "PRIVATE_GITHUB_TOKEN_CANARY")
        .env("SYNAPSE_LOCAL_TOKEN", "PRIVATE_SYNAPSE_TOKEN_CANARY")
        .args(export_arguments(&repository, &destination, &["--github"]))
        .output()
        .unwrap();
    assert_success(&output);
    assert!(markers.iter().all(|marker| !marker.exists()));
    let files = bundle_files(&destination);
    for bytes in files.values() {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains("PRIVATE_GH_TOKEN_CANARY"));
        assert!(!text.contains("PRIVATE_GITHUB_TOKEN_CANARY"));
        assert!(!text.contains("PRIVATE_SYNAPSE_TOKEN_CANARY"));
    }
}
