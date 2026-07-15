#!/usr/bin/env bash
set -euo pipefail

tag="${1:-}"
target="${2:-}"
output_directory="${3:-dist}"

version="$(bash scripts/verify_release_version.sh "$tag")"
if [[ -z "$target" ]]; then
  echo "release_error: a Rust host target is required" >&2
  exit 1
fi

host="$(rustc -vV | awk '$1 == "host:" { print $2 }')"
if [[ "$host" != "$target" ]]; then
  echo "release_error: builder host $host does not match requested target $target" >&2
  exit 1
fi

release_directory="${CARGO_TARGET_DIR:-target}/release"
for binary in synapse synapse-local; do
  if [[ ! -x "$release_directory/$binary" ]]; then
    echo "release_error: missing executable $release_directory/$binary" >&2
    exit 1
  fi
done

bundle="synapsegit-$tag-$target"
bundle_directory="$output_directory/$bundle"
archive="$output_directory/$bundle.tar.gz"
checksums="$output_directory/SHA256SUMS"
for path in "$bundle_directory" "$archive" "$checksums"; do
  if [[ -e "$path" ]]; then
    echo "release_error: refusing to replace existing $path" >&2
    exit 1
  fi
done

mkdir -p "$bundle_directory"
install -m 0755 "$release_directory/synapse" "$bundle_directory/synapse"
install -m 0755 "$release_directory/synapse-local" "$bundle_directory/synapse-local"
install -m 0644 "docs/releases/$tag.md" "$bundle_directory/README.md"

source_date_epoch="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"
if [[ ! "$source_date_epoch" =~ ^[0-9]+$ ]]; then
  echo "release_error: SOURCE_DATE_EPOCH must be an integer" >&2
  exit 1
fi

(
  cd "$output_directory"
  tar \
    --sort=name \
    --mtime="@$source_date_epoch" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -cf - "$bundle" | gzip -n > "$bundle.tar.gz"
  sha256sum "$bundle.tar.gz" > SHA256SUMS
)

printf 'packaged %s (%s)\n' "$archive" "$version"
