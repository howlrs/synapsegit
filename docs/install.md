# Installing SynapseGit

Audience: preview users and evaluators
Status: Stage 0 prerelease
Applies to: v0.4.0
Last verified: 2026-07-20

SynapseGit currently has one prebuilt distribution and one source-install path.
It is not published to crates.io, Homebrew, a Linux package repository, or a
container registry.

| Route | Requirements | Installs | Recommended for |
|---|---|---|---|
| GitHub Release archive | Linux x86_64, glibc 2.34+ | `synapse`, `synapse-local`, `synapse-present` | Fastest preview evaluation |
| Tagged source build | Rust 1.88+, supported Unix-like host | The selected binary | Other platforms and source review |

Windows is not currently supported by the archive publication path. macOS and
Linux ARM64 do not have release-tested prebuilt artifacts yet. The Dockerfile
in this repository is for a private, one-shot GCP packaging smoke test; it is
not an end-user SynapseGit image.

The tagged v0.4.0 source also contains the frozen generic-artifact v1
contracts and their sequential, durable, checkout, and local-projection Rust
libraries. Those are workspace libraries for an embedding application. The
release archive still contains exactly the three binaries listed above; it does
not add a generic-artifact HTTP, CLI, browser UI, executable, or remote publish
path.

## Install the Linux x86-64 release

Download the archive and checksum from the fixed v0.4.0 release URL:

```bash
curl -LO https://github.com/howlrs/synapsegit/releases/download/v0.4.0/synapsegit-v0.4.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/howlrs/synapsegit/releases/download/v0.4.0/SHA256SUMS
sha256sum --check SHA256SUMS
```

`SHA256SUMS` detects accidental or malicious byte changes relative to the file
published on the same Release. It does not authenticate the project owner by
itself. Verify the v0.4.0 archive's build provenance with GitHub CLI as well:

```bash
gh attestation verify synapsegit-v0.4.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo howlrs/synapsegit \
  --signer-workflow howlrs/synapsegit/.github/workflows/release.yml \
  --source-ref refs/tags/v0.4.0 \
  --deny-self-hosted-runners
```

An attestation links an artifact to its GitHub Actions build; it is not a claim
that the software is vulnerability-free. Stop if either verification command
fails. Do not extract or install an unverified archive.

Inspect the extracted release notes before installing. Then copy all three
binaries to a user-owned directory:

```bash
tar -xzf synapsegit-v0.4.0-x86_64-unknown-linux-gnu.tar.gz
less synapsegit-v0.4.0-x86_64-unknown-linux-gnu/README.md

mkdir -p "$HOME/.local/bin"
install -m 0755 synapsegit-v0.4.0-x86_64-unknown-linux-gnu/synapse "$HOME/.local/bin/synapse"
install -m 0755 synapsegit-v0.4.0-x86_64-unknown-linux-gnu/synapse-local "$HOME/.local/bin/synapse-local"
install -m 0755 synapsegit-v0.4.0-x86_64-unknown-linux-gnu/synapse-present "$HOME/.local/bin/synapse-present"
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

## Build from a tagged source release

Install Rust 1.88 or newer, a C toolchain, and SQLite build prerequisites for
the host. Install directly from the immutable v0.4.0 tag:

```bash
cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v0.4.0 \
  --locked \
  synapse-cli

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v0.4.0 \
  --locked \
  synapse-local-http

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v0.4.0 \
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
git clone --branch v0.4.0 --depth 1 https://github.com/howlrs/synapsegit.git
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
  --tag v0.4.0 \
  --locked \
  synapse-cli

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v0.4.0 \
  --locked \
  synapse-local-http

cargo install \
  --git https://github.com/howlrs/synapsegit \
  --tag v0.4.0 \
  --locked \
  synapse-publication
```

`--locked`はtagに記録されたdependency versionを使います。Stage 0のworkspace crateは
crates.io配布を意図せず、repository sourceからbuildします。

## Update

Preview releases may change the object, archive, or OID draft. Before updating:

1. read the new release notes and [changelog](../CHANGELOG.md);
2. export important repositories with the currently installed version;
3. keep the old binary and archive until the new version has verified the data;
4. install the new binaries only from a fixed release tag; and
5. do not assume forward or backward compatibility unless the release notes say
   it is supported.

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

- [Read the v0.4.0 release notes](./releases/v0.4.0.md)
- [Run the three-minute Pilot](../README.md#try-it-in-three-minutes)
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
