#!/usr/bin/env bash
set -euo pipefail

# Assembles the same bundle layout package_release.sh ships (three binaries,
# the mural tutorial runner, its sample images, and the bundled guide) into a
# fresh directory outside this checkout, then runs the runner from a
# different current directory for adopt, reject, and defer, plus the
# existing argument-validation and refusal checks. This is the CI-reachable
# counterpart of the release workflow's packaged-archive smoke: it uses
# whatever binaries the caller already built (debug binaries in CI) so it can
# run on every push and pull request, not only at tag time.
#
# usage: scripts/test_tutorial_bundle.sh SYNAPSE_BIN SYNAPSE_LOCAL_BIN SYNAPSE_PRESENT_BIN

synapse_bin="${1:-}"
synapse_local_bin="${2:-}"
synapse_present_bin="${3:-}"

if [[ -z "$synapse_bin" || -z "$synapse_local_bin" || -z "$synapse_present_bin" ]]; then
  echo "tutorial_bundle_error: usage: scripts/test_tutorial_bundle.sh SYNAPSE_BIN SYNAPSE_LOCAL_BIN SYNAPSE_PRESENT_BIN" >&2
  exit 2
fi
for bin in "$synapse_bin" "$synapse_local_bin" "$synapse_present_bin"; do
  if [[ ! -x "$bin" ]]; then
    echo "tutorial_bundle_error: missing or non-executable $bin" >&2
    exit 1
  fi
done

repository_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

bundle="$work/bundle"
mkdir -p "$bundle/scripts" "$bundle/docs/tutorial/assets"
install -m 0755 "$synapse_bin" "$bundle/synapse"
install -m 0755 "$synapse_local_bin" "$bundle/synapse-local"
install -m 0755 "$synapse_present_bin" "$bundle/synapse-present"
install -m 0755 "$repository_root/scripts/run_mural_tutorial.sh" "$bundle/scripts/run_mural_tutorial.sh"
install -m 0644 "$repository_root/scripts/ARCHIVE_TUTORIAL.md" "$bundle/scripts/ARCHIVE_TUTORIAL.md"
for asset in mural-original.png mural-current.png mural-ai-proposal.png; do
  install -m 0644 \
    "$repository_root/docs/tutorial/assets/$asset" \
    "$bundle/docs/tutorial/assets/$asset"
done

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
