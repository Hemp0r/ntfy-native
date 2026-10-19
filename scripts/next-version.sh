#!/usr/bin/env bash
# Decide the next release version.
#
# Usage: git tag --list 'v*.*.*' | scripts/next-version.sh <cargo-version> <kind>
# <kind> is major|minor|patch (see scripts/bump-kind.sh).
#
# Also: git tag --list 'v*.*.*' | scripts/next-version.sh --latest
# prints the latest plain vX.Y.Z tag as X.Y.Z (nothing without such a tag).
#
# Reads existing tags from stdin and prints "<version> <bump|keep>":
#   keep  - release <version> (the Cargo.toml version) unchanged
#   bump  - Cargo.toml must be rewritten to <version> (latest tag, jumped by <kind>)
#
# Rules:
#   - no vMAJOR.MINOR.PATCH tag yet           -> keep the Cargo.toml version
#   - Cargo.toml version > latest tag         -> keep the Cargo.toml version
#   - otherwise                               -> latest tag jumped by <kind>
#     (patch: X.Y.Z+1, minor: X.Y+1.0, major: X+1.0.0)
# Versions are compared numerically. Tags that are not plain
# vMAJOR.MINOR.PATCH (pre-releases, other names) are ignored.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/commits-lib.sh
source "$here/commits-lib.sh"

# --latest: print the latest plain vX.Y.Z tag (as X.Y.Z) from stdin, or nothing.
if [[ "${1-}" == "--latest" && $# -eq 1 ]]; then
  latest_semver_tag
  exit 0
fi

semver_re='^([0-9]+)\.([0-9]+)\.([0-9]+)$'

if [[ $# -ne 2 || ! "$1" =~ $semver_re || ! "$2" =~ ^(major|minor|patch)$ ]]; then
  echo "usage: next-version.sh <MAJOR.MINOR.PATCH> <major|minor|patch> (tags on stdin)" >&2
  exit 2
fi
cargo=$1
kind=$2

latest=$(latest_semver_tag)

if [[ -z "$latest" ]] || semver_greater "$cargo" "$latest"; then
  echo "$cargo keep"
  exit 0
fi

IFS=. read -r -a parts <<<"$latest"
major=$((10#${parts[0]}))
minor=$((10#${parts[1]}))
patch=$((10#${parts[2]}))
case "$kind" in
  major) echo "$((major + 1)).0.0 bump" ;;
  minor) echo "$major.$((minor + 1)).0 bump" ;;
  patch) echo "$major.$minor.$((patch + 1)) bump" ;;
esac
