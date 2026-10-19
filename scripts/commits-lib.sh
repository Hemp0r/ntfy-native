#!/usr/bin/env bash
# shellcheck disable=SC2034  # CC_* variables are consumed by the sourcing script
# Shared Conventional Commit helpers. Source this file; do not execute it.
#
# Commit input format (stdin of the callers), one NUL-terminated record per commit:
#   "<sha>\x1f<full message>"
# as produced by: git log -z --no-merges --format='%H%x1f%B' <range>
# Read it with: while IFS= read -r -d '' record; do ...
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
  local line
  line=$(printf '%s' "${1-}" | tr '[:upper:]' '[:lower:]')
  [[ "$line" =~ ^chore\(release\)!?: ]]
}

# semver_greater <a> <b>
# Exit 0 when MAJOR.MINOR.PATCH a is numerically greater than b.
semver_greater() {
  local -a a b
  local i
  IFS=. read -r -a a <<<"$1"
  IFS=. read -r -a b <<<"$2"
  for i in 0 1 2; do
    if ((10#${a[i]} > 10#${b[i]})); then return 0; fi
    if ((10#${a[i]} < 10#${b[i]})); then return 1; fi
  done
  return 1
}

# latest_semver_tag
# Reads tag names on stdin; prints the numerically highest plain vX.Y.Z as
# X.Y.Z (no "v"), or nothing. Other names (pre-releases etc.) are ignored.
latest_semver_tag() {
  local tag latest="" candidate
  while IFS= read -r tag || [[ -n "$tag" ]]; do
    tag=${tag%$'\r'}
    [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || continue
    candidate=${BASH_REMATCH[1]}
    if [[ -z "$latest" ]] || semver_greater "$candidate" "$latest"; then
      latest=$candidate
    fi
  done
  [[ -z "$latest" ]] || printf '%s\n' "$latest"
  return 0
}
