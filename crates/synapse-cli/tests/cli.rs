use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
const PROPOSAL_HEAD: &str =
    "commit:sg-oid-v1:sha256:21f1e5825721dafad3847c6b0f7d2143f46288fe72082da4dedae62c0db82b00";

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-cli-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_synapse"))
        .args(arguments)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn command_line_drives_ref_fsck_export_and_restore() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let archive_path = temporary.join("archive");
    let restored_path = temporary.join("restored");
    let fixture_directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/core/v0.1/fixtures");

    let repository = Repository::open(&repository_path).unwrap();
    for entry in fs::read_dir(&fixture_directory)
        .unwrap()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && path.file_name().is_some_and(|name| name != "golden.json")
        {
            repository.put_object(&fs::read(path).unwrap()).unwrap();
        }
    }
    repository
        .put_blob(fs::File::open(fixture_directory.join("proposal.txt")).unwrap())
        .unwrap();
    drop(repository);

    let repository_text = repository_path.to_str().unwrap();
    let update = run(&[
        "update-ref",
        repository_text,
        "proposal/agent/cli",
        "-",
        PROPOSAL_HEAD,
        "--message",
        "CLI acceptance",
    ]);
    assert_success(&update);

    let fsck = run(&["fsck", repository_text]);
    assert_success(&fsck);
    assert!(String::from_utf8_lossy(&fsck.stdout).contains("issues=0"));

    let export = run(&["export", repository_text, archive_path.to_str().unwrap()]);
    assert_success(&export);
    let restore = run(&[
        "restore",
        archive_path.to_str().unwrap(),
        restored_path.to_str().unwrap(),
    ]);
    assert_success(&restore);
    let refs = run(&["refs", restored_path.to_str().unwrap()]);
    assert_success(&refs);
    let refs = String::from_utf8(refs.stdout).unwrap();
    assert!(refs.contains("proposal/agent/cli"));
    assert!(refs.contains(PROPOSAL_HEAD));
}
