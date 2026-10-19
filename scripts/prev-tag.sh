#!/usr/bin/env bash
# Print the tag released right before <tag>.
#
# Usage: git tag --list 'v*.*.*' | scripts/prev-tag.sh <vX.Y.Z>
#
# Prints the numerically next-older plain vX.Y.Z tag from stdin, or nothing
# (first release). Other tag names (pre-releases etc.) are ignored.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/commits-lib.sh
source "$here/commits-lib.sh"

if [[ $# -ne 1 || ! "$1" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  echo "usage: prev-tag.sh <vMAJOR.MINOR.PATCH> (tags on stdin)" >&2
  exit 2
fi
current=${BASH_REMATCH[1]}

prev=""
while IFS= read -r tag || [[ -n "$tag" ]]; do
  tag=${tag%$'\r'}
  [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || continue
  candidate=${BASH_REMATCH[1]}
  semver_greater "$current" "$candidate" || continue
  if [[ -z "$prev" ]] || semver_greater "$candidate" "$prev"; then
    prev=$candidate
  fi
done
[[ -z "$prev" ]] || echo "v$prev"
