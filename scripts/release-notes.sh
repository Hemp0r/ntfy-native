#!/usr/bin/env bash
# Build grouped release notes (Markdown) from commits.
#
# Usage: git log -z --no-merges --format='%H%x1f%B' <range> |
#          scripts/release-notes.sh <owner/repo> <tag> [<previous-tag>]
#
# Groups (empty ones are omitted): Features (feat), Fehlerbehebungen (fix),
# Sonstiges (everything else, including commits without a type).
# chore(release): commits are skipped. Never fails on odd messages.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/commits-lib.sh
source "$here/commits-lib.sh"

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: release-notes.sh <owner/repo> <tag> [<previous-tag>] (commits on stdin)" >&2
  exit 2
fi
repo=$1
tag=$2
prev=${3-}

features="" fixes="" other=""
while IFS= read -r -d '' record || [[ -n "$record" ]]; do
  sha=${record%%$'\x1f'*}
  message=${record#*$'\x1f'}
  message=${message//$'\r'/}
  sha=${sha//[[:space:]]/}
  subject=${message#"${message%%[![:space:]]*}"}
  subject=${subject%%$'\n'*}
  [[ -z "$subject" ]] && continue
  is_release_commit "$subject" && continue
  parse_subject "$subject"

  entry="- "
  if [[ "$CC_BREAKING" == 1 ]] || is_breaking_body "$message"; then
    entry+="⚠️ Breaking: "
  fi
  [[ -n "$CC_SCOPE" ]] && entry+="**$CC_SCOPE:** "
  entry+="$CC_DESC ([${sha:0:7}](https://github.com/$repo/commit/$sha))"$'\n'

  case "$CC_TYPE" in
    feat) features+=$entry ;;
    fix) fixes+=$entry ;;
    *) other+=$entry ;;
  esac
done

if [[ -z "$features$fixes$other" ]]; then
  echo "Keine Änderungen."
else
  [[ -n "$features" ]] && printf '### Features\n\n%s\n' "$features"
  [[ -n "$fixes" ]] && printf '### Fehlerbehebungen\n\n%s\n' "$fixes"
  [[ -n "$other" ]] && printf '### Sonstiges\n\n%s\n' "$other"
fi
if [[ -n "$prev" ]]; then
  [[ -z "$features$fixes$other" ]] && echo
  echo "Alle Änderungen: https://github.com/$repo/compare/$prev...$tag"
fi
