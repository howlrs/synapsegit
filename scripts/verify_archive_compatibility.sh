#!/usr/bin/env bash
# Verifies the fixed v0.11.1 archive and the most recent eligible local release
# archive remain readable by the caller's CLI.
set -euo pipefail

readonly BASELINE_TAG="v0.11.1"
readonly BASELINE_COMMIT="8ff4df29c4817869d47c85967d6548891fbf0846"

fail() {
  echo "archive_compatibility_error: $*" >&2
  exit 1
}

if [[ $# -ne 1 ]]; then
  fail "usage: verify_archive_compatibility.sh <current-synapse-binary>"
fi

current_binary="$1"
if [[ "$current_binary" != /* ]]; then
  current_binary="$(pwd)/$current_binary"
fi
[[ -x "$current_binary" ]] || fail "current synapse binary is not executable: $current_binary"
"$current_binary" --help >/dev/null || fail "current synapse binary cannot run: $current_binary"

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd -- "$script_dir/.." && git rev-parse --show-toplevel)" || fail "script must run from a Git checkout"
comparator="$repository_root/scripts/compare_creator_reports.mjs"
[[ -f "$comparator" ]] || fail "report comparator is missing: $comparator"
baseline_selector="$repository_root/scripts/select_archive_compat_baseline.mjs"
[[ -f "$baseline_selector" ]] || fail "baseline selector is missing: $baseline_selector"
expected_current_version="$(node -e 'const fs = require("node:fs"); const text = fs.readFileSync(process.argv[1], "utf8"); const match = text.match(/^version\s*=\s*"([^"]+)"\s*$/m); if (!match) throw new Error("package version missing"); process.stdout.write(match[1]);' "$repository_root/crates/synapse-cli/Cargo.toml")" \
  || fail "cannot determine current synapse CLI version"
[[ "$("$current_binary" --version)" == "synapse $expected_current_version" ]] \
  || fail "current synapse binary version does not match this checkout's synapse-cli ${expected_current_version}"

actual_baseline="$(git -C "$repository_root" rev-parse "${BASELINE_TAG}^{commit}" 2>/dev/null)" \
  || fail "required local baseline tag ${BASELINE_TAG} is unavailable; fetch it before running this gate"
[[ "$actual_baseline" == "$BASELINE_COMMIT" ]] \
  || fail "baseline tag ${BASELINE_TAG} resolves to ${actual_baseline}, expected ${BASELINE_COMMIT}"
git -C "$repository_root" cat-file -e "${BASELINE_COMMIT}^{commit}" \
  || fail "required local baseline commit ${BASELINE_COMMIT} is unavailable"
git -C "$repository_root" merge-base --is-ancestor "$BASELINE_COMMIT" HEAD \
  || fail "required local baseline ${BASELINE_TAG} (${BASELINE_COMMIT}) is not an ancestor of HEAD"

recent_baseline="$(while IFS=$'\t' read -r tag object_type commit; do
  ancestor=false
  if [[ "$object_type" == "tag" && "$commit" =~ ^[0-9a-f]{40}$ ]] && git -C "$repository_root" merge-base --is-ancestor "$commit" HEAD; then
    ancestor=true
  fi
  printf '%s\t%s\t%s\t%s\n' "$tag" "$object_type" "$commit" "$ancestor"
done < <(git -C "$repository_root" for-each-ref --format='%(refname:strip=2)%09%(objecttype)%09%(*objectname)' refs/tags) | node "$baseline_selector" --select "$expected_current_version")" \
  || fail "cannot select a local annotated archive baseline"
declare -a baseline_tags=("$BASELINE_TAG")
declare -a baseline_commits=("$BASELINE_COMMIT")
if [[ -n "$recent_baseline" ]]; then
  IFS=$'\t' read -r recent_tag recent_commit <<< "$recent_baseline"
  if [[ "$recent_commit" != "$BASELINE_COMMIT" ]]; then
    baseline_tags+=("$recent_tag")
    baseline_commits+=("$recent_commit")
  fi
fi

# Defaulting to offline makes the local gate network-free. A caller may set
# CARGO_NET_OFFLINE=false explicitly when its controlled CI environment needs it.
: "${CARGO_NET_OFFLINE:=true}"
export CARGO_NET_OFFLINE

work="$(mktemp -d "${TMPDIR:-/tmp}/synapsegit-archive-compat.XXXXXX")" || fail "cannot create temporary directory"
cleanup() {
  [[ -n "${work:-}" && -d "$work" ]] && rm -rf -- "$work"
}
trap cleanup EXIT

for index in "${!baseline_tags[@]}"; do
  baseline_tag="${baseline_tags[$index]}"
  baseline_commit="${baseline_commits[$index]}"
  baseline_version="${baseline_tag#v}"
  case_dir="$work/$baseline_tag"
  old_source="$case_dir/source"
  old_target="$case_dir/target"
  old_repo="$case_dir/old repository"
  archive="$case_dir/old archive"
  new_repo="$case_dir/new repository"
  current_archive="$case_dir/current archive"
  second_repo="$case_dir/second repository"

  echo "archive compatibility baseline: ${baseline_tag} ${baseline_commit}"
  mkdir -p -- "$old_source"
  git -C "$repository_root" archive --format=tar "$baseline_commit" | tar -xf - -C "$old_source"
  ( cd "$old_source" && CARGO_TARGET_DIR="$old_target" cargo +1.88.0 build --locked -p synapse-cli --bin synapse )
  old_binary="$old_target/debug/synapse"
  [[ -x "$old_binary" ]] || fail "old synapse binary was not produced: $old_binary"
  [[ "$("$old_binary" --version)" == "synapse $baseline_version" ]] \
    || fail "archived baseline ${baseline_tag} produced an unexpected synapse version"

  printf 'original compatibility fixture\n' > "$case_dir/original.bin"
  printf 'current compatibility fixture\n' > "$case_dir/current.bin"
  printf 'candidate compatibility fixture\n' > "$case_dir/candidate.bin"
  printf '%s\n' '{"tool":"archive compatibility harness","model":"fixture-v1","prompt":"compare the three supplied files","intent":"retain private provenance through restore"}' > "$case_dir/generation-note.json"

  "$old_binary" init "$old_repo" >/dev/null
  "$old_binary" creator-run "$old_repo" archive-compat \
    "$case_dir/original.bin" "$case_dir/current.bin" "$case_dir/candidate.bin" \
    --subject "Archive compatibility fixture" \
    --creator "Archive compatibility harness" \
    --decision defer \
    --rationale "Private compatibility rationale retained across archive restore." \
    --generation-note-file "$case_dir/generation-note.json" >/dev/null
  "$old_binary" fsck "$old_repo" >/dev/null
  "$old_binary" creator-report "$old_repo" archive-compat --format json > "$case_dir/old-report.json"
  "$old_binary" export "$old_repo" "$archive" >/dev/null

  "$current_binary" restore "$archive" "$new_repo" >/dev/null
  "$current_binary" fsck "$new_repo" >/dev/null
  "$current_binary" creator-report "$new_repo" archive-compat --format json > "$case_dir/new-report.json"
  node "$comparator" --backward-compatible "$case_dir/old-report.json" "$case_dir/new-report.json"

  "$current_binary" export "$new_repo" "$current_archive" >/dev/null
  "$current_binary" restore "$current_archive" "$second_repo" >/dev/null
  "$current_binary" fsck "$second_repo" >/dev/null
  "$current_binary" creator-report "$second_repo" archive-compat --format json > "$case_dir/second-report.json"
  node "$comparator" --exact "$case_dir/new-report.json" "$case_dir/second-report.json"
done

echo "archive compatibility passed: ${#baseline_tags[@]} baseline archive(s) restored by current synapse"
