#!/usr/bin/env bash
# Extracts one packaged release archive and exercises it as a user would:
# bundled files, versions, help, a creator session, an inbox candidate, the
# session list, a local publication bundle, and the archive tutorial.
#
#   scripts/smoke_release_archive.sh ARCHIVE TAG TARGET
set -euo pipefail
unset CDPATH

fail() {
  echo "release_smoke_error: $*" >&2
  exit 1
}

smoke_server() {
  local executable="$1"
  local mode="$2"
  local repository="$3"
  python3 - "$executable" "$mode" "$repository" <<'PY'
import queue
import re
import signal
import subprocess
import sys
import threading
import time
import urllib.request

executable, mode, repository = sys.argv[1:]
command = [executable]
if mode == "canonical":
    command.append("serve")
elif mode != "legacy":
    raise ValueError(f"unknown server mode: {mode}")
command.extend(["--port", "0", "--project", f"smoke={repository}"])

process = subprocess.Popen(
    command,
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.PIPE,
    text=True,
)
assert process.stderr is not None
lines = queue.Queue()

def read_stderr():
    for line in process.stderr:
        lines.put(line)

reader = threading.Thread(target=read_stderr, daemon=True)
reader.start()
deadline = time.monotonic() + 15
origin = None
stderr = []
try:
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited {process.returncode}: {''.join(stderr)}")
        try:
            line = lines.get(timeout=min(0.2, deadline - time.monotonic()))
        except queue.Empty:
            continue
        stderr.append(line)
        match = re.search(r"http://127\.0\.0\.1:\d+", line)
        if match:
            origin = match.group(0)
            break
    if origin is None:
        raise RuntimeError(f"server did not announce a loopback URL: {''.join(stderr)}")
    with urllib.request.urlopen(origin, timeout=2) as response:
        if response.status != 200:
            raise RuntimeError(f"expected HTTP 200 from {origin}, got {response.status}")
finally:
    if process.poll() is None:
        process.send_signal(signal.SIGINT)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    reader.join(timeout=1)
PY
}

[[ $# -eq 3 ]] || fail "usage: scripts/smoke_release_archive.sh ARCHIVE TAG TARGET"
archive="$1"
tag="$2"
target="$3"
bundle="synapsegit-$tag-$target"
[[ "$(basename -- "$archive")" == "$bundle.tar.gz" ]] || fail "expected $bundle.tar.gz, got $archive"
script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

work="$(mktemp -d "${TMPDIR:-/tmp}/synapsegit-release-smoke.XXXXXX")"
# Publication refuses symlinked parents; macOS keeps TMPDIR under /var.
work="$(cd "$work" && pwd -P)"
trap 'rm -rf -- "$work"' EXIT
tar -xzf "$archive" -C "$work"
root="$work/$bundle"
for file in README.md LICENSE THIRD_PARTY_NOTICES.md SECURITY.md CHANGELOG.md \
  AI_AGENT_GUIDE.md AI_AGENT_GUIDE.ja.md TUTORIAL.md scripts/run_mural_tutorial.sh \
  assets/creator-journey.en.png assets/creator-journey.ja.png \
  docs/tutorial/assets/mural-original.png docs/tutorial/assets/mural-current.png \
  docs/tutorial/assets/mural-ai-proposal.png; do
  [[ -s "$root/$file" ]] || fail "missing or empty $file"
done
[[ -x "$root/scripts/run_mural_tutorial.sh" ]] || fail "the tutorial runner is not executable"
if grep -q '{{RELEASE_TAG}}' "$root/TUTORIAL.md"; then
  fail "unresolved RELEASE_TAG placeholder in TUTORIAL.md"
fi
grep -q "/blob/$tag/" "$root/TUTORIAL.md" || fail "TUTORIAL.md does not link the tag $tag"

# The compatibility names must remain relative links after users move an
# extracted bundle, rather than links back to the original extraction path.
relocated_root="$work/relocated-bundle"
mv "$root" "$relocated_root"
root="$relocated_root"

version="${tag#v}"
[[ -f "$root/synapse" && ! -L "$root/synapse" && -x "$root/synapse" ]] \
  || fail "synapse is not a regular executable"
for alias in synapse-local synapse-present; do
  [[ -L "$root/$alias" ]] || fail "$alias is not a symbolic link"
  [[ "$(readlink "$root/$alias")" == synapse ]] || fail "$alias must link relatively to synapse"
done
[[ "$("$root/synapse" --version)" == "synapse $version" ]] || fail "synapse does not report $version"
"$root/synapse" --help >/dev/null
[[ "$("$root/synapse" serve --version)" == "synapse $version" ]] || fail "synapse serve does not report $version"
"$root/synapse" serve --help >/dev/null
[[ "$("$root/synapse" present --version)" == "synapse $version" ]] || fail "synapse present does not report $version"
"$root/synapse" present --help >/dev/null
for binary in synapse-local synapse-present; do
  [[ "$("$root/$binary" --version)" == "$binary $version" ]] || fail "$binary does not report its legacy name and $version"
  "$root/$binary" --help >/dev/null
done

synapse="$root/synapse"
printf 'original\n' > "$work/original.bin"
printf 'current\n' > "$work/current.bin"
printf 'candidate\n' > "$work/candidate.bin"
"$synapse" init "$work/repo" >/dev/null
"$synapse" creator-run "$work/repo" release-smoke \
  "$work/original.bin" "$work/current.bin" "$work/candidate.bin" \
  --subject "Release smoke" --creator "Release workflow" \
  --decision defer --rationale "Private smoke rationale" >/dev/null
"$synapse" creator-report "$work/repo" release-smoke --format json >/dev/null
"$synapse" creator-list "$work/repo" --format json | grep -q '"session": "release-smoke"' \
  || fail "creator-list does not list the smoke session"
mkdir "$work/inbox"
"$synapse" inbox put "$work/inbox" release-smoke \
  "$work/original.bin" "$work/current.bin" "$work/candidate.bin" \
  --subject "Release inbox smoke" --creator "Release workflow" >/dev/null
[[ -s "$work/inbox/release-smoke/manifest.json" ]] || fail "inbox put wrote no manifest"
"$synapse" present export "$work/repo" "$work/view" \
  --session release-smoke --public --github >/dev/null
"$synapse" present preview "$work/view" >/dev/null
"$root/synapse-present" preview "$work/view" >/dev/null
for file in projection.json story.md index.html manifest.json checksums.json target/README.md; do
  [[ -s "$work/view/$file" ]] || fail "the publication bundle is missing $file"
done

bash "$script_directory/test_tutorial_bundle.sh" --bundle "$root"
smoke_server "$synapse" canonical "$work/repo"
smoke_server "$root/synapse-local" legacy "$work/repo"
echo "release archive smoke passed: $bundle"
