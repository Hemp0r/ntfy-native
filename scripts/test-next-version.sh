#!/usr/bin/env bash
# Tests for scripts/next-version.sh.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
script="$here/next-version.sh"
fail=0

check() {
  local name=$1 cargo=$2 tags=$3 expected=$4 actual
  actual=$(printf '%s' "$tags" | "$script" "$cargo") || actual="<exit $?>"
  if [[ "$actual" == "$expected" ]]; then
    echo "ok   - $name"
  else
    echo "FAIL - $name: expected '$expected', got '$actual'"
    fail=1
  fi
}

check "no tags keeps cargo version" 0.1.0 "" "0.1.0 keep"
check "single tag equal to cargo bumps patch" 0.1.0 $'v0.1.0\n' "0.1.1 bump"
check "numeric sort of patch versions" 0.1.0 $'v0.1.0\nv0.1.9\nv0.1.10\n' "0.1.11 bump"
check "cargo higher than latest tag is kept" 0.2.0 $'v0.1.5\n' "0.2.0 keep"
check "cargo lower than latest tag bumps latest" 0.1.3 $'v0.1.5\n' "0.1.6 bump"
check "non-semver tags are ignored" 0.1.0 $'v0.1.0\nv9.9.9-rc1\nvfoo\nv1.2\nlatest\nv0.1.0.1\n' "0.1.1 bump"
check "only non-semver tags counts as no tags" 0.1.0 $'v1.0.0-rc1\nfoo\n' "0.1.0 keep"
check "major comparison is numeric" 1.0.0 $'v0.9.9\n' "1.0.0 keep"
check "minor comparison is numeric" 0.10.0 $'v0.9.0\n' "0.10.0 keep"
check "tag order in input does not matter" 0.1.0 $'v0.1.10\nv0.1.2\n' "0.1.11 bump"

if ! printf '' | "$script" "not-a-version" >/dev/null 2>&1; then
  echo "ok   - invalid cargo version is rejected"
else
  echo "FAIL - invalid cargo version was accepted"
  fail=1
fi

exit "$fail"
