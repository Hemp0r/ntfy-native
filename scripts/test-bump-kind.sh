#!/usr/bin/env bash
# Tests for scripts/bump-kind.sh.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
script="$here/bump-kind.sh"
fail=0

check() {
  local name=$1 version=$2 commits=$3 expected=$4 actual
  actual=$(printf "%b" "$commits" | "$script" "$version") || actual="<exit $?>"
  if [[ "$actual" == "$expected" ]]; then
    echo "ok   - $name"
  else
    echo "FAIL - $name: expected '$expected', got '$actual'"
    fail=1
  fi
}

check classify_feat_is_minor 0.1.2 'a1\x1ffeat: x\0' minor
check classify_scoped_feat 0.1.2 'a1\x1ffeat(ui): x\0' minor
check classify_uppercase_type 0.1.2 'a1\x1fFeat: x\0' minor
check classify_fix_only_is_patch 0.1.2 'a1\x1ffix: a\0a2\x1ffix(import): b\0' patch
check classify_other_only_is_patch 0.1.2 'a1\x1fdocs: a\0a2\x1fci: b\0a3\x1fUpdate README\0' patch
check classify_breaking_bang_pre_1 0.4.0 'a1\x1ffix!: x\0' minor
check classify_breaking_body_post_1 1.2.3 'a1\x1frefactor: x\n\nBREAKING CHANGE: y\0' major
check classify_breaking_dash_body 1.2.3 'a1\x1frefactor: x\n\nBREAKING-CHANGE: y\0' major
check classify_breaking_bang_post_1 1.0.0 'a1\x1ffeat(api)!: x\0' major
check classify_breaking_text_not_line_start 1.2.3 'a1\x1ffix: x\n\nno BREAKING CHANGE: here\0' patch
check classify_scope_with_hyphen 0.1.2 'a1\x1ffeat(tray-icon): x\0' minor
check classify_revert_is_patch 0.1.2 'a1\x1fRevert "feat: x"\0' patch
check classify_release_commit_ignored 0.1.2 'a1\x1fchore(release): v0.1.2\0' patch
check classify_release_commit_not_breaking 1.0.0 'a1\x1fchore(release)!: v1\0' patch
check classify_empty_range 0.1.2 '' patch
check classify_crlf_breaking_post_1 1.0.0 'a1\x1ffeat: x\r\n\r\nBREAKING CHANGE: y\r\n\0' major
check classify_body_with_record_separator 0.1.2 'a1\x1ffix: x\n\x1efeat: y\0' patch
check classify_release_commit_case_insensitive 0.1.2 'a1\x1fChore(Release): v1\0' patch
check classify_feat_beats_fix 0.1.2 'a1\x1ffix: a\0a2\x1ffeat: b\0' minor

if ! printf '' | "$script" "bad" >/dev/null 2>&1; then
  echo "ok   - invalid version is rejected"
else
  echo "FAIL - invalid version was accepted"
  fail=1
fi

exit "$fail"
