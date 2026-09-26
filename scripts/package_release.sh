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
if [[ ! -s LICENSE ]]; then
  echo "release_error: missing or empty LICENSE" >&2
  exit 1
fi
if [[ ! -s THIRD_PARTY_NOTICES.md ]]; then
  echo "release_error: missing or empty THIRD_PARTY_NOTICES.md" >&2
  exit 1
fi

tutorial_runner="scripts/run_mural_tutorial.sh"
tutorial_guide="scripts/ARCHIVE_TUTORIAL.md"
tutorial_assets=(
  docs/tutorial/assets/mural-original.png
  docs/tutorial/assets/mural-current.png
  docs/tutorial/assets/mural-ai-proposal.png
)
if [[ ! -x "$tutorial_runner" ]]; then
  echo "release_error: missing or non-executable $tutorial_runner" >&2
  exit 1
fi
if [[ ! -s "$tutorial_guide" ]]; then
  echo "release_error: missing or empty $tutorial_guide" >&2
  exit 1
fi
for asset in "${tutorial_assets[@]}"; do
  if [[ ! -s "$asset" ]]; then
    echo "release_error: missing or empty $asset" >&2
    exit 1
  fi
done

host="$(rustc -vV | awk '$1 == "host:" { print $2 }')"
if [[ "$host" != "$target" ]]; then
  echo "release_error: builder host $host does not match requested target $target" >&2
  exit 1
fi

release_directory="${CARGO_TARGET_DIR:-target}/release"
release_binaries=(synapse synapse-local synapse-present)
for binary in "${release_binaries[@]}"; do
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
for binary in "${release_binaries[@]}"; do
  install -m 0755 "$release_directory/$binary" "$bundle_directory/$binary"
done
install -m 0644 "docs/releases/$tag.md" "$bundle_directory/README.md"
install -m 0644 SECURITY.md "$bundle_directory/SECURITY.md"
install -m 0644 CHANGELOG.md "$bundle_directory/CHANGELOG.md"
install -m 0644 LICENSE "$bundle_directory/LICENSE"
install -m 0644 THIRD_PARTY_NOTICES.md "$bundle_directory/THIRD_PARTY_NOTICES.md"

mkdir -p "$bundle_directory/scripts" "$bundle_directory/docs/tutorial/assets"
install -m 0755 "$tutorial_runner" "$bundle_directory/$tutorial_runner"
for asset in "${tutorial_assets[@]}"; do
  install -m 0644 "$asset" "$bundle_directory/$asset"
done

bundled_guide="$bundle_directory/TUTORIAL.md"
sed "s/{{RELEASE_TAG}}/$tag/g" "$tutorial_guide" > "$bundled_guide"
chmod 0644 "$bundled_guide"
if grep -q '{{RELEASE_TAG}}' "$bundled_guide"; then
  echo "release_error: unresolved {{RELEASE_TAG}} placeholder in $bundled_guide" >&2
  exit 1
fi
if ! grep -q "/blob/$tag/" "$bundled_guide"; then
  echo "release_error: $bundled_guide is missing the substituted tag $tag" >&2
  exit 1
fi

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
