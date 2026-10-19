#!/usr/bin/env bash
# Check that a PR title (= future squash commit subject) follows Conventional
# Commits: "type(scope)!: description" with a known type (case-insensitive).
#
# Usage: scripts/check-commit-subject.sh <title>
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/commits-lib.sh
source "$here/commits-lib.sh"

types=" feat fix docs ci test refactor perf build style chore revert "
title=${1-}
# GitHub's default title for reverting a PR: Revert "<subject>".
revert_re='^Revert "(.+)"$'
if [[ "$title" =~ $revert_re ]]; then
  title=${BASH_REMATCH[1]}
fi
parse_subject "$title"
desc_blank_re='^[[:space:]]*$'
if [[ -n "$CC_TYPE" && "$types" == *" $CC_TYPE "* && ! "$CC_DESC" =~ $desc_blank_re ]]; then
  exit 0
fi

echo "::error::Title '${1-}' does not follow 'type(scope): description'." >&2
echo "Allowed types:${types% }. Example: feat(ui): add autostart switch" >&2
exit 1
