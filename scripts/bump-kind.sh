#!/usr/bin/env bash
# Decide how big the next version jump is.
#
# Usage: git log -z --no-merges --format='%H%x1f%B' <range> |
#          scripts/bump-kind.sh <current-version>
#
# Prints major|minor|patch:
#   - breaking commit ("type!:" or a BREAKING CHANGE: line)
#                         -> minor before 1.0, major from 1.0
#   - at least one feat   -> minor
#   - anything else       -> patch (also for no commits)
# chore(release): commits are ignored. Unparseable messages count as "other".
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/commits-lib.sh
source "$here/commits-lib.sh"

if [[ $# -ne 1 || ! "$1" =~ ^([0-9]+)\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: bump-kind.sh <MAJOR.MINOR.PATCH> (commits on stdin)" >&2
  exit 2
fi
major=$((10#${BASH_REMATCH[1]}))

has_feat=0
has_breaking=0
while IFS= read -r -d '' record || [[ -n "$record" ]]; do
  message=${record#*$'\x1f'}
  message=${message//$'\r'/}
  subject=${message#"${message%%[![:space:]]*}"}
  subject=${subject%%$'\n'*}
  [[ -z "$subject" ]] && continue
  is_release_commit "$subject" && continue
  parse_subject "$subject"
  [[ "$CC_TYPE" == feat ]] && has_feat=1
  if [[ "$CC_BREAKING" == 1 ]] || is_breaking_body "$message"; then
    has_breaking=1
  fi
done

if ((has_breaking)); then
  if ((major >= 1)); then echo major; else echo minor; fi
elif ((has_feat)); then
  echo minor
else
  echo patch
fi
