# Installing SynapseGit

Audience: local single-user users and evaluators
Status: v1.0.0 release installation
Applies to: v1.0.0
Last verified: 2026-10-02

SynapseGit currently has one prebuilt distribution and one source-install path.
It is not published to crates.io, Homebrew, a Linux package repository, or a
container registry.

| Route | Requirements | Installs | Recommended for |
|---|---|---|---|
| GitHub Release archive | Linux x86_64, glibc 2.34+ | `synapse`, `synapse-local`, `synapse-present` | Fastest local setup |
| GitHub Release archive | macOS on Apple Silicon (arm64) | `synapse`, `synapse-local`, `synapse-present` | Creators on a Mac |
| Tagged source build | Rust 1.88+, supported Unix-like host | The selected binary | Other platforms and source review |

Windows is not currently supported by the archive publication path. Linux ARM64
has no release-tested prebuilt artifact yet; use a tagged source build. The Dockerfile
in this repository is for a private, one-shot GCP packaging smoke test; it is
not an end-user SynapseGit image.

The tagged v1.0.0 source also contains the frozen generic-artifact v1
contracts and their sequential, durable, checkout, and local-projection Rust
libraries. Those are workspace libraries for an embedding application. The
release archive still contains exactly the three binaries listed above; it does
not add a generic-artifact HTTP, CLI, browser UI, executable, or remote publish
path.

Use the fixed v1.0.0 commands below for both published archive platforms.

## Install the Linux x86-64 release

After the v1.0.0 tag workflow publishes the release, download the archive
and checksum from its fixed release URL:

```bash
curl -LO https://github.com/howlrs/synapsegit/releases/download/v1.0.0/synapsegit-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/howlrs/synapsegit/releases/download/v1.0.0/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

`SHA256SUMS` lists every archive of the release; `--ignore-missing` checks the
one you downloaded. It detects accidental or malicious byte changes relative to the file
published on the same Release. It does not authenticate the project owner by
itself. Verify the v1.0.0 archive's build provenance with GitHub CLI as well:

```bash
gh attestation verify synapsegit-v1.0.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo howlrs/synapsegit \
  --signer-workflow howlrs/synapsegit/.github/workflows/release.yml \
  --source-ref refs/tags/v1.0.0 \
  --deny-self-hosted-runners
```

An attestation links an artifact to its GitHub Actions build; it is not a claim
that the software is vulnerability-free. Stop if either verification command
fails. Do not extract or install an unverified archive.

Inspect the extracted release notes before installing. Then copy all three
binaries to a user-owned directory:

```bash
tar -xzf synapsegit-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
less synapsegit-v1.0.0-x86_64-unknown-linux-gnu/README.md

mkdir -p "$HOME/.local/bin"
install -m 0755 synapsegit-v1.0.0-x86_64-unknown-linux-gnu/synapse "$HOME/.local/bin/synapse"
install -m 0755 synapsegit-v1.0.0-x86_64-unknown-linux-gnu/synapse-local "$HOME/.local/bin/synapse-local"
install -m 0755 synapsegit-v1.0.0-x86_64-unknown-linux-gnu/synapse-present "$HOME/.local/bin/synapse-present"
export PATH="$HOME/.local/bin:$PATH"

synapse --version
synapse-local --version
synapse-present --version
```

If `synapse` is not found in a new terminal, add this line to the shell profile
used by that terminal:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

The v1.0.0 archive also bundles the mural tutorial runner
(`scripts/run_mural_tutorial.sh`), its three sample images
(`docs/tutorial/assets/`), and a self-contained guide (`TUTORIAL.md` at the
archive root, next to this bundle's `README.md`), so you can try the tutorial
from the extracted archive alone, without cloning the repository. The
v0.9.0 archive does not include them; use a checkout of the v0.9.0 tag for
the tutorial instead.

## Install the macOS arm64 release

`v1.0.0` includes an archive for macOS on Apple Silicon. It is built and
smoke-tested on macOS 14 by the same tag workflow, with the same checksum and
build-provenance attestation:

```bash
TAG=v1.0.0
curl -LO "https://github.com/howlrs/synapsegit/releases/download/$TAG/synapsegit-$TAG-aarch64-apple-darwin.tar.gz"
curl -LO "https://github.com/howlrs/synapsegit/releases/download/$TAG/SHA256SUMS"
grep "synapsegit-$TAG-aarch64-apple-darwin.tar.gz" SHA256SUMS | shasum -a 256 --check
gh attestation verify "synapsegit-$TAG-aarch64-apple-darwin.tar.gz" \
  --repo howlrs/synapsegit \
  --signer-workflow howlrs/synapsegit/.github/workflows/release.yml \
  --source-ref "refs/tags/$TAG" \
  --deny-self-hosted-runners
```

Stop if either check fails. Do not extract or install an unverified archive.
Then install the three binaries:

```bash
tar -xzf "synapsegit-$TAG-aarch64-apple-darwin.tar.gz"
mkdir -p "$HOME/.local/bin"
for binary in synapse synapse-local synapse-present; do
  install -m 0755 "synapsegit-$TAG-aarch64-apple-darwin/$binary" "$HOME/.local/bin/$binary"
done
export PATH="$HOME/.local/bin:$PATH"
synapse --version
```

`synapse-present` refuses a publication path whose parent folders include a
symbolic link. On macOS `/tmp` and `/var` are symbolic links, so keep
publication inputs and outputs under your home folder.

The binaries are not signed or notarized by Apple. Files downloaded with `curl`
carry no quarantine attribute and run directly. If you downloaded the archive
with a browser and macOS blocks a binary, remove the quarantine attribute from
the installed binaries only after both checks above passed:

```bash
xattr -d com.apple.quarantine "$HOME/.local/bin/synapse" "$HOME/.local/bin/synapse-local" "$HOME/.local/bin/synapse-present"
```

## Build from a tagged source release

Install Rust 1.88 or newer, a C toolchain, and SQLite build prerequisites for
the host. Install directly from the immutable v1.0.0 tag:

```bash
cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v1.0.0 \
  --locked \
  synapse-cli

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v1.0.0 \
  --locked \
  synapse-local-http

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v1.0.0 \
  --locked \
  synapse-publication

synapse --version
synapse-local --version
synapse-present --version
```

`--locked` uses the dependency versions recorded by the tag. Use a release tag,
not a moving branch, when installing software you plan to evaluate or retain.

To inspect and test the source before installing:

```bash
git clone --branch v1.0.0 --depth 1 https://github.com/howlrs/synapsegit.git
cd synapsegit
cargo test --workspace --all-targets --locked
cargo install --path crates/synapse-cli --locked
cargo install --path crates/synapse-local-http --locked
cargo install --path crates/synapse-publication --locked
```

The workspace crates are intentionally marked `publish = false` during Stage
0. The commands above build from the repository; they do not use crates.io.

## Tagged sourceからbuildする

日本語でsourceから導入する場合も、moving branchではなくrelease tagを固定します。Rust
1.88以降とhostのC toolchainを用意し、次を実行してください。

```bash
cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v1.0.0 \
  --locked \
  synapse-cli

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v1.0.0 \
  --locked \
  synapse-local-http

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v1.0.0 \
  --locked \
  synapse-publication
```

`--locked`はtagに記録されたdependency versionを使います。Stage 0のworkspace crateは
crates.io配布を意図せず、repository sourceからbuildします。

## Update

v1.x keeps the object, identifier, and archive formats compatible as described
in the [compatibility policy](./compatibility.md). Before updating:

1. read the new release notes and [changelog](../CHANGELOG.md);
2. export important repositories with the currently installed version;
3. keep the old binary and archive until the new version has verified the data;
4. install the new binaries only from a fixed release tag; and
5. read the release notes for operational and supported-version changes.

v0.13.1 and earlier binaries refuse Creator sessions recorded by releases before
v0.11.1 with `creator_report_invalid`, even though `fsck` passes
([#132](https://github.com/howlrs/synapsegit/issues/132)). Releases after v0.13.1
read Creator sessions recorded by every published release, in place or after
archive restore. A session recorded by a newer release or an unreleased source
build is refused with `creator_implementation_unrecognized`. That code does not
indicate damaged data; open the session with the recording build or a later
release.

There is no automatic updater.

## Uninstall

If the binaries were copied to the recommended per-user location:

```bash
rm "$HOME/.local/bin/synapse" "$HOME/.local/bin/synapse-local" \
  "$HOME/.local/bin/synapse-present"
```

Uninstalling does not remove repositories under `$HOME/SynapseGit` or any other
path you supplied. Review and remove those separately only when you no longer
need the recorded data.

## Next steps

- [Read the v1.0.0 release notes](./releases/v1.0.0.md)
- [Complete the illustrated 15-minute mural tutorial](./tutorial/README.md)
- [画像付き15分 壁画チュートリアルを実行する](./tutorial/README.ja.md)
- [Get started from the README](../README.md#get-started)
- [Run the full source Quickstart](./quickstart.md)
- [Read the localhost application runbook](../deploy/local/README.md)
- [Review the security model](./security_model.md)
- [Return to the documentation index](./README.md)

## License notice

Copyright (c) 2026 howlrs and K-Terashima. The custom
[SynapseGit Source-Available License 1.0](../LICENSE) permits these install,
build, and run steps only for non-commercial evaluation or for preparing a
permitted GitHub Fork or pull request. It is not an open-source license.
Commercial, production, or hosted use and redistribution outside the permitted
GitHub Fork workflow require separate written permission. The license applies
to v0.1.0 even though its original archive does not contain a bundled copy; the
root `LICENSE` is authoritative.
Third-party Rust components remain under the terms reproduced in
[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
