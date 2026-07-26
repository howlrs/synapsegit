#!/usr/bin/env bash
set -euo pipefail

repository="${1:-}"
decision="${2:-adopt}"

if [[ -z "$repository" ]]; then
  echo "usage: scripts/run_mural_tutorial.sh REPOSITORY [adopt|reject|defer]" >&2
  exit 2
fi
if [[ "$decision" != "adopt" && "$decision" != "reject" && "$decision" != "defer" ]]; then
  echo "tutorial_error: decision must be adopt, reject, or defer" >&2
  exit 2
fi
if [[ -e "$repository" ]]; then
  echo "tutorial_error: repository path already exists: $repository" >&2
  exit 1
fi
if ! command -v synapse >/dev/null 2>&1; then
  echo "tutorial_error: synapse is not available on PATH" >&2
  exit 1
fi

script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
project_root="$(cd -- "$script_directory/.." && pwd -P)"
assets="$project_root/docs/tutorial/assets"

synapse creator-run "$repository" mural-treatment-01 \
  "$assets/mural-original.png" \
  "$assets/mural-current.png" \
  "$assets/mural-ai-proposal.png" \
  --subject "Community Hall Coastal Mural" \
  --creator "Tutorial Conservator" \
  --decision "$decision" \
  --rationale "Review the restrained inpainting proposal as the next documented state."

synapse creator-report "$repository" mural-treatment-01

echo
echo "Tutorial complete."
echo "Inspect it with:"
echo "  synapse-local --project \"mural=$repository\" --label \"mural=Community Hall Coastal Mural\""
