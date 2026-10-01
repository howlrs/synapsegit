#!/usr/bin/env bash
# Records one published release's repository and directory archive as a
# read-compatibility fixture for the current CLI (Issue #165).
#
#   scripts/generate_release_fixture.sh TAG VERIFIED_RELEASE_ARCHIVE
#
# Download the Linux x86_64 archive and SHA256SUMS of TAG from the GitHub
# Release and verify them first, for example:
#
#   gh release download TAG --pattern 'synapsegit-TAG-x86_64-unknown-linux-gnu.tar.gz' --pattern SHA256SUMS
#   sha256sum --check --ignore-missing SHA256SUMS
#
# The released `synapse` binary records one deferred session from fixed
# synthetic inputs and exports it. The fixture is written to
# crates/synapse-cli/tests/fixtures/releases/TAG and is never overwritten.
set -euo pipefail

fail() {
  echo "release_fixture_error: $*" >&2
  exit 1
}

[[ $# -eq 2 ]] || fail "usage: scripts/generate_release_fixture.sh TAG VERIFIED_RELEASE_ARCHIVE"
tag="$1"
archive="$2"
[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$ ]] \
  || fail "expected a release tag such as v0.1.0, got $tag"
[[ "$(basename -- "$archive")" == "synapsegit-$tag-x86_64-unknown-linux-gnu.tar.gz" ]] \
  || fail "expected synapsegit-$tag-x86_64-unknown-linux-gnu.tar.gz, got $archive"
[[ -f "$archive" ]] || fail "release archive not found: $archive"

root="$(git rev-parse --show-toplevel)"
output="$root/crates/synapse-cli/tests/fixtures/releases/$tag"
[[ ! -e "$output" ]] || fail "refusing to replace existing fixture $output"

work="$(mktemp -d "${TMPDIR:-/tmp}/synapsegit-release-fixture.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT
tar -xzf "$archive" -C "$work"
synapse="$work/synapsegit-$tag-x86_64-unknown-linux-gnu/synapse"
[[ -x "$synapse" ]] || fail "the release archive has no synapse binary"
[[ "$("$synapse" --version)" == "synapse ${tag#v}" ]] || fail "binary version does not match $tag"

printf 'release fixture original\n' > "$work/original.bin"
printf 'release fixture current\n' > "$work/current.bin"
printf 'release fixture candidate\n' > "$work/candidate.bin"
note=()
if "$synapse" --help | grep -q -- '--generation-note-file'; then
  printf '{"tool":"Fixture tool","model":"fixture-model","prompt":"Synthetic fixture prompt","intent":"Read compatibility"}' > "$work/note.json"
  note=(--generation-note-file "$work/note.json")
fi
"$synapse" init "$work/repository" >/dev/null
"$synapse" creator-run "$work/repository" release-fixture \
  "$work/original.bin" "$work/current.bin" "$work/candidate.bin" \
  --subject "Release fixture subject" \
  --creator "Release fixture creator" \
  --decision defer \
  --rationale "Private fixture rationale." \
  "${note[@]}" >/dev/null
"$synapse" export "$work/repository" "$work/archive" >/dev/null
for sidecar in "$work/repository/refs.sqlite3-wal" "$work/repository/refs.sqlite3-shm"; do
  [[ ! -e "$sidecar" ]] || fail "the repository was left with SQLite sidecar $sidecar"
done

mkdir -p "$output"
cp -R "$work/repository" "$output/repository"
cp -R "$work/archive" "$output/archive"
printf '%s\t%s\t%s\n' "$tag" "$(sha256sum "$archive" | cut -d' ' -f1)" "${#note[@]}"
