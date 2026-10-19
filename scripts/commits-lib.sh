#!/usr/bin/env bash
# shellcheck disable=SC2034  # CC_* variables are consumed by the sourcing script
# Shared Conventional Commit helpers. Source this file; do not execute it.
#
# Commit input format (stdin of the callers), one record per commit:
#   "<sha>\x1f<full message>\x1e"
# as produced by: git log --no-merges --format='%H%x1f%B%x1e' <range>
#
# Functions never fail on odd input; they only set the CC_* variables.

# parse_subject <first-line>
# Sets CC_TYPE (lower case, empty when the line has no "type: " schema),
# CC_SCOPE, CC_BREAKING (0/1) and CC_DESC (the whole line when no schema).
parse_subject() {
  local line=${1-}
  local re='^([A-Za-z]+)(\(([^)]+)\))?(!)?: (.+)$'
  CC_TYPE="" CC_SCOPE="" CC_BREAKING=0 CC_DESC=$line
  if [[ "$line" =~ $re ]]; then
    CC_TYPE=$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')
    CC_SCOPE=${BASH_REMATCH[3]}
    [[ -n "${BASH_REMATCH[4]}" ]] && CC_BREAKING=1
    CC_DESC=${BASH_REMATCH[5]}
  fi
  return 0
}

# is_breaking_body <message>
# Exit 0 when a line starts with "BREAKING CHANGE:" or "BREAKING-CHANGE:".
is_breaking_body() {
  [[ "${1-}" =~ (^|$'\n')BREAKING[\ -]CHANGE: ]]
}

# is_release_commit <first-line>
# Exit 0 for automatic "chore(release):" commits (with or without "!").
is_release_commit() {
  [[ "${1-}" =~ ^[Cc]hore\(release\)!?: ]]
}
