#!/usr/bin/env bash
# Decide the next release version.
#
# Usage: git tag --list 'v*.*.*' | scripts/next-version.sh <cargo-version>
#
# Reads existing tags from stdin and prints "<version> <bump|keep>":
#   keep  - release <version> (the Cargo.toml version) unchanged
#   bump  - Cargo.toml must be rewritten to <version> (latest tag, patch + 1)
#
# Rules:
#   - no vMAJOR.MINOR.PATCH tag yet           -> keep the Cargo.toml version
#   - Cargo.toml version > latest tag         -> keep the Cargo.toml version
#   - otherwise                               -> latest tag with patch + 1
# Versions are compared numerically. Tags that are not plain
# vMAJOR.MINOR.PATCH (pre-releases, other names) are ignored.
set -euo pipefail

semver_re='^([0-9]+)\.([0-9]+)\.([0-9]+)$'

if [[ $# -ne 1 || ! "$1" =~ $semver_re ]]; then
  echo "usage: next-version.sh <MAJOR.MINOR.PATCH> (tags on stdin)" >&2
  exit 2
fi
cargo=$1

# Print 1 if version a > version b, else 0 (both MAJOR.MINOR.PATCH).
greater() {
  local -a a b
  IFS=. read -r -a a <<<"$1"
  IFS=. read -r -a b <<<"$2"
  local i
  for i in 0 1 2; do
    if ((10#${a[i]} > 10#${b[i]})); then echo 1; return; fi
    if ((10#${a[i]} < 10#${b[i]})); then echo 0; return; fi
  done
  echo 0
}

latest=""
while IFS= read -r tag || [[ -n "$tag" ]]; do
  tag=${tag%$'\r'}
  [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || continue
  candidate=${BASH_REMATCH[1]}
  if [[ -z "$latest" || $(greater "$candidate" "$latest") == 1 ]]; then
    latest=$candidate
  fi
done

if [[ -z "$latest" || $(greater "$cargo" "$latest") == 1 ]]; then
  echo "$cargo keep"
  exit 0
fi

IFS=. read -r -a parts <<<"$latest"
echo "${parts[0]}.${parts[1]}.$((10#${parts[2]} + 1)) bump"
