#!/usr/bin/env bash
# Collects per-platform release archives and their one-line SHA256SUMS files
# (as written by scripts/package_release.sh) into one asset directory with a
# combined SHA256SUMS, refusing duplicate or unlisted archives.
#
#   scripts/assemble_release_assets.sh PARTS_DIRECTORY OUTPUT_DIRECTORY
set -euo pipefail

fail() {
  echo "release_assets_error: $*" >&2
  exit 1
}

[[ $# -eq 2 ]] || fail "usage: scripts/assemble_release_assets.sh PARTS_DIRECTORY OUTPUT_DIRECTORY"
parts="$1"
output="$2"
[[ -d "$parts" ]] || fail "parts directory not found: $parts"
[[ ! -e "$output" ]] || fail "refusing to replace $output"
mkdir -p "$output"

sums=()
while IFS= read -r -d '' file; do
  sums+=("$file")
done < <(find "$parts" -type f -name SHA256SUMS -print0 | sort -z)
[[ ${#sums[@]} -gt 0 ]] || fail "no SHA256SUMS files under $parts"

for sum in "${sums[@]}"; do
  directory="$(dirname -- "$sum")"
  [[ "$(wc -l < "$sum")" -eq 1 ]] || fail "$sum must list exactly one archive"
  read -r digest name < "$sum"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || fail "$sum has no SHA-256 digest"
  [[ "$name" =~ ^synapsegit-v[^/]+\.tar\.gz$ ]] || fail "$sum names an unexpected file $name"
  [[ -f "$directory/$name" ]] || fail "$sum lists missing $name"
  [[ ! -e "$output/$name" ]] || fail "duplicate archive $name"
  cp "$directory/$name" "$output/$name"
  printf '%s  %s\n' "$digest" "$name" >> "$output/SHA256SUMS.unsorted"
done
sort -k2 "$output/SHA256SUMS.unsorted" > "$output/SHA256SUMS"
rm "$output/SHA256SUMS.unsorted"
for file in "$output"/*.tar.gz; do
  grep -q "  $(basename -- "$file")\$" "$output/SHA256SUMS" || fail "$file is not listed"
done
