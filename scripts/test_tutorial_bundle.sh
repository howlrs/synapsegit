#!/usr/bin/env bash
set -euo pipefail

# Exercises the mural tutorial bundle (three binaries, the runner, its sample
# images, and TUTORIAL.md at the bundle root) the same way package_release.sh
# ships it. Two modes:
#
#   scripts/test_tutorial_bundle.sh SYNAPSE_BIN SYNAPSE_LOCAL_BIN SYNAPSE_PRESENT_BIN
#     Assembles a fresh bundle directory from the given binaries (debug
#     binaries in CI) plus this checkout's runner/assets/guide, so it can run
#     on every push and pull request, not only at tag time.
#
#   scripts/test_tutorial_bundle.sh --bundle BUNDLE_DIR
#     Reuses an already-extracted bundle directory as-is (for example the
#     real packaged archive in the release workflow). The caller is
#     responsible for any file/mode/tag-substitution checks specific to that
#     bundle; this script only exercises the runner.
#
# In both modes, the runner is invoked from a directory that is neither the
# checkout nor the bundle, for adopt, reject, and defer, plus the existing
# argument-validation and refusal checks, and the report's `selected` field
# is checked against the decision (`true` for adopt, `false` otherwise).

usage() {
  echo "tutorial_bundle_error: usage: scripts/test_tutorial_bundle.sh SYNAPSE_BIN SYNAPSE_LOCAL_BIN SYNAPSE_PRESENT_BIN" >&2
  echo "   or: scripts/test_tutorial_bundle.sh --bundle BUNDLE_DIR" >&2
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if [[ "${1:-}" == "--bundle" ]]; then
  bundle="${2:-}"
  if [[ -z "$bundle" ]]; then
    usage
    exit 2
  fi
  if [[ ! -x "$bundle/scripts/run_mural_tutorial.sh" ]]; then
    echo "tutorial_bundle_error: missing or non-executable $bundle/scripts/run_mural_tutorial.sh" >&2
    exit 1
  fi
else
  synapse_bin="${1:-}"
  synapse_local_bin="${2:-}"
  synapse_present_bin="${3:-}"

  if [[ -z "$synapse_bin" || -z "$synapse_local_bin" || -z "$synapse_present_bin" ]]; then
    usage
    exit 2
  fi
  for bin in "$synapse_bin" "$synapse_local_bin" "$synapse_present_bin"; do
    if [[ ! -x "$bin" ]]; then
      echo "tutorial_bundle_error: missing or non-executable $bin" >&2
      exit 1
    fi
  done

  repository_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
  bundle="$work/bundle"
  mkdir -p "$bundle/scripts" "$bundle/docs/tutorial/assets"
  install -m 0755 "$synapse_bin" "$bundle/synapse"
  install -m 0755 "$synapse_local_bin" "$bundle/synapse-local"
  install -m 0755 "$synapse_present_bin" "$bundle/synapse-present"
  install -m 0755 "$repository_root/scripts/run_mural_tutorial.sh" "$bundle/scripts/run_mural_tutorial.sh"
  sed "s/{{RELEASE_TAG}}/ci-test/g" "$repository_root/scripts/ARCHIVE_TUTORIAL.md" > "$bundle/TUTORIAL.md"
  chmod 0644 "$bundle/TUTORIAL.md"
  if grep -q '{{RELEASE_TAG}}' "$bundle/TUTORIAL.md"; then
    echo "tutorial_bundle_error: unresolved {{RELEASE_TAG}} placeholder in $bundle/TUTORIAL.md" >&2
    exit 1
  fi
  for asset in mural-original.png mural-current.png mural-ai-proposal.png; do
    install -m 0644 \
      "$repository_root/docs/tutorial/assets/$asset" \
      "$bundle/docs/tutorial/assets/$asset"
  done
fi

export PATH="$bundle:$PATH"
runner="$bundle/scripts/run_mural_tutorial.sh"

# Run from a directory that is neither the checkout nor the bundle, proving
# the runner resolves its assets relative to itself, not to the caller's cwd.
cd "$work"

for decision in adopt reject defer; do
  repository="$work/repo-$decision"
  output="$("$runner" "$repository" "$decision")"
  if ! grep -q "disposition=$decision" <<<"$output"; then
    echo "tutorial_bundle_error: missing disposition=$decision in runner output" >&2
    exit 1
  fi
  if [[ "$decision" == "adopt" ]]; then
    expected_selected="selected=true"
  else
    expected_selected="selected=false"
  fi
  if ! grep -q "$expected_selected" <<<"$output"; then
    echo "tutorial_bundle_error: expected $expected_selected for decision $decision" >&2
    exit 1
  fi
  if ! grep -q "fsck=clean" <<<"$output"; then
    echo "tutorial_bundle_error: missing fsck=clean for decision $decision" >&2
    exit 1
  fi
done

set +e
"$runner" "$work/repo-bad-decision" not-a-decision >/dev/null 2>&1
bad_decision_status=$?
set -e
if [[ "$bad_decision_status" -ne 2 ]]; then
  echo "tutorial_bundle_error: expected exit 2 for a bad decision argument, got $bad_decision_status" >&2
  exit 1
fi

set +e
"$runner" "$work/repo-adopt" adopt >/dev/null 2>&1
existing_path_status=$?
set -e
if [[ "$existing_path_status" -ne 1 ]]; then
  echo "tutorial_bundle_error: expected exit 1 for an existing repository path, got $existing_path_status" >&2
  exit 1
fi

echo "tutorial_bundle_ok: adopt, reject, defer, and refusal checks passed in $bundle"
