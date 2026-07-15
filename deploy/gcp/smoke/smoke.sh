#!/bin/sh
set -eu

umask 077

work_dir="$(mktemp -d /tmp/synapse-smoke.XXXXXX)"
repo="${work_dir}/repository"
fixtures="/opt/synapse/smoke-fixtures"

cleanup() {
    rm -rf -- "${work_dir}"
}

trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

/usr/local/bin/synapse creator-run \
    "${repo}" \
    cloud-run-smoke \
    "${fixtures}/original.txt" \
    "${fixtures}/current.txt" \
    "${fixtures}/ai-output.txt" \
    --subject "Cloud Run smoke fixture" \
    --creator "SynapseGit smoke job" \
    --decision adopt \
    --rationale "Bundled, non-sensitive fixture accepted by the smoke test."

/usr/local/bin/synapse creator-report "${repo}" cloud-run-smoke
/usr/local/bin/synapse fsck "${repo}"

printf '%s\n' "SynapseGit Cloud Run smoke test passed."
