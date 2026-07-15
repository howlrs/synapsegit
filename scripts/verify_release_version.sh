#!/usr/bin/env bash
set -euo pipefail

tag="${1:-}"
if [[ ! "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo "release_error: expected a semantic version tag such as v0.1.0, got ${tag:-<empty>}" >&2
  exit 1
fi

version="${tag#v}"
for manifest in crates/*/Cargo.toml; do
  package_version="$(
    awk '
      /^\[package\][[:space:]]*$/ { in_package = 1; next }
      /^\[/ { if (in_package) exit }
      in_package && /^version[[:space:]]*=/ {
        line = $0
        sub(/^[^"]*"/, "", line)
        sub(/".*$/, "", line)
        print line
        exit
      }
    ' "$manifest"
  )"
  if [[ -z "$package_version" ]]; then
    echo "release_error: $manifest has no explicit package version" >&2
    exit 1
  fi
  if [[ "$package_version" != "$version" ]]; then
    echo "release_error: $manifest is $package_version but tag is $tag" >&2
    exit 1
  fi
done

notes="docs/releases/$tag.md"
if [[ ! -f "$notes" ]]; then
  echo "release_error: missing $notes" >&2
  exit 1
fi

echo "$version"
