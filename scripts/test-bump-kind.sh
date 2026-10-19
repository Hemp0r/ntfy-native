#!/usr/bin/env bash
# Tests for scripts/bump-kind.sh.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
script="$here/bump-kind.sh"
fail=0

check() {
  local name=$1 version=$2 commits=$3 expected=$4 actual
  actual=$(printf '%s' "$commits" | "$script" "$version") || actual="<exit $?>"
  if [[ "$actual" == "$expected" ]]; then
    echo "ok   - $name"
  else
    echo "FAIL - $name: expected '$expected', got '$actual'"
    fail=1
  fi
}

check classify_feat_is_minor 0.1.2 $'a1\x1ffeat: x\x1e' minor
check classify_scoped_feat 0.1.2 $'a1\x1ffeat(ui): x\x1e' minor
check classify_uppercase_type 0.1.2 $'a1\x1fFeat: x\x1e' minor
check classify_fix_only_is_patch 0.1.2 $'a1\x1ffix: a\x1e\na2\x1ffix(import): b\x1e' patch
check classify_other_only_is_patch 0.1.2 $'a1\x1fdocs: a\x1e\na2\x1fci: b\x1e\na3\x1fUpdate README\x1e' patch
check classify_breaking_bang_pre_1 0.4.0 $'a1\x1ffix!: x\x1e' minor
check classify_breaking_body_post_1 1.2.3 $'a1\x1frefactor: x\n\nBREAKING CHANGE: y\x1e' major
check classify_breaking_dash_body 1.2.3 $'a1\x1frefactor: x\n\nBREAKING-CHANGE: y\x1e' major
check classify_breaking_bang_post_1 1.0.0 $'a1\x1ffeat(api)!: x\x1e' major
check classify_breaking_text_not_line_start 1.2.3 $'a1\x1ffix: x\n\nno BREAKING CHANGE: here\x1e' patch
check classify_scope_with_hyphen 0.1.2 $'a1\x1ffeat(tray-icon): x\x1e' minor
check classify_revert_is_patch 0.1.2 $'a1\x1fRevert "feat: x"\x1e' patch
check classify_release_commit_ignored 0.1.2 $'a1\x1fchore(release): v0.1.2\x1e' patch
check classify_release_commit_not_breaking 1.0.0 $'a1\x1fchore(release)!: v1\x1e' patch
check classify_empty_range 0.1.2 '' patch
check classify_feat_beats_fix 0.1.2 $'a1\x1ffix: a\x1e\na2\x1ffeat: b\x1e' minor

if ! printf '' | "$script" "bad" >/dev/null 2>&1; then
  echo "ok   - invalid version is rejected"
else
  echo "FAIL - invalid version was accepted"
  fail=1
fi

exit "$fail"
