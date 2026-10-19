#!/usr/bin/env bash
# Tests for prev-tag.sh, release-notes.sh and check-commit-subject.sh.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fail=0
repo=Hemp0r/ntfy-native
sha1=a1b2c3d4e5f60718293a4b5c6d7e8f9012345678
sha2=b2c3d4e5f60718293a4b5c6d7e8f901234567890
sha3=c3d4e5f60718293a4b5c6d7e8f90123456789012

report() {
  if [[ "$2" == "$3" ]]; then
    echo "ok   - $1"
  else
    echo "FAIL - $1: expected '$3', got '$2'"
    fail=1
  fi
}

prev() { printf '%b' "$2" | "$here/prev-tag.sh" "$1" || echo "<exit $?>"; }
notes() { printf '%b' "$1" | "$here/release-notes.sh" "$repo" "${@:2}" || echo "<exit $?>"; }
contains() { # name haystack needle
  if [[ "$2" == *"$3"* ]]; then echo "ok   - $1"; else echo "FAIL - $1: '$3' not in output"; fail=1; fi
}
lacks() {
  if [[ "$2" != *"$3"* ]]; then echo "ok   - $1"; else echo "FAIL - $1: '$3' found in output"; fail=1; fi
}

report prev_tag_numeric "$(prev v0.2.0 'v0.1.9\nv0.1.10\nv0.2.0\n')" v0.1.10
report prev_tag_first_release "$(prev v0.1.0 'v0.1.0\n')" ""
report prev_tag_ignores_non_semver "$(prev v1.0.0 'v0.9.0\nv1.0.0-rc1\nv1.0.0\n')" v0.9.0
report prev_tag_rerun_old_tag "$(prev v0.1.9 'v0.1.9\nv0.1.10\nv0.2.0\n')" ""
report prev_tag_unlisted_tag "$(prev v0.3.0 'v0.1.0\nv0.2.0\n')" v0.2.0

three="$sha1\\x1ffix: b\\0$sha2\\x1ffeat: a\\0$sha3\\x1fdocs: c\\0"
out=$(notes "$three" v0.2.0 v0.1.2)
order=$(grep '^### ' <<<"$out" | tr '\n' '|')
report notes_grouped_in_order "$order" '### Features|### Fehlerbehebungen|### Sonstiges|'

out=$(notes "$sha1\\x1ffix: b\\0" v0.2.0 v0.1.2)
lacks notes_skip_empty_groups "$out" '### Features'

out=$(notes "$sha1\\x1ffix(import): robust redaction\\0" v0.2.0 v0.1.2)
contains notes_scope_and_link "$out" "- **import:** robust redaction ([a1b2c3d](https://github.com/$repo/commit/$sha1))"

out=$(notes "$sha1\\x1ffeat!: drop x\\0" v0.2.0 v0.1.2)
contains notes_breaking_mark "$out" '- ⚠️ Breaking: drop x'
out=$(notes "$sha1\\x1frefactor: y\\n\\nBREAKING CHANGE: z\\0" v0.2.0 v0.1.2)
contains notes_breaking_body_mark "$out" '- ⚠️ Breaking: y'

out=$(notes "$sha1\\x1ffeat: first line\\n\\nlong body\\nmore\\0" v0.2.0 v0.1.2)
lacks notes_subject_only "$out" 'long body'
out=$(notes "$sha1\\x1ffeat: crlf\\r\\n\\r\\nbody\\r\\n\\0" v0.2.0 v0.1.2)
lacks notes_crlf_stripped "$out" $'\r'

out=$(notes "$sha1\\x1fRevert \"feat: x\"\\0$sha2\\x1fUpdate README\\0" v0.2.0 v0.1.2)
contains notes_untyped_revert "$out" "- Revert \"feat: x\" ([a1b2c3d]"
contains notes_untyped_other "$out" '- Update README ([b2c3d4e]'
lacks notes_untyped_not_feature "$out" '### Features'

out=$(notes "$sha1\\x1fchore(release): v0.1.2\\0$sha2\\x1fChore(Release): v0.1.3\\0$sha3\\x1ffix: x\\0" v0.2.0 v0.1.2)
lacks notes_excludes_release_commit "$out" 'release'

out=$(notes "$sha1\\x1ffix: x\\0" v0.2.0 v0.1.2)
report notes_compare_link "$(tail -n 1 <<<"$out")" "Alle Änderungen: https://github.com/$repo/compare/v0.1.2...v0.2.0"

out=$(notes "$sha1\\x1ffix: x\\0" v0.1.0)
lacks notes_without_previous_tag "$out" 'Alle Änderungen'
out=$(notes "$sha1\\x1ffix: x\\0" v0.1.0 "")
lacks notes_empty_previous_arg "$out" 'compare'

out=$(notes "" v0.2.0 v0.1.2)
contains notes_empty_range "$out" 'Keine Änderungen.'
out=$(notes "$sha1\\x1fchore(release): v0.1.2\\0" v0.2.0 v0.1.2)
contains notes_only_release_commit_is_empty "$out" 'Keine Änderungen.'

subject() { if "$here/check-commit-subject.sh" "$1" >/dev/null 2>&1; then echo ok; else echo fail; fi; }
report subject_check_scoped "$(subject 'feat(ui): x')" ok
report subject_check_case "$(subject 'Feat: x')" ok
report subject_check_breaking "$(subject 'fix(a-b)!: x')" ok
report subject_check_untyped "$(subject 'added stuff')" fail
report subject_check_no_space "$(subject 'feat:x')" fail
report subject_check_unknown_type "$(subject 'wip: x')" fail
report subject_check_empty "$(subject '')" fail
report subject_check_revert_title "$(subject 'Revert "feat(ui): x"')" ok
report subject_check_revert_nonconforming "$(subject 'Revert "added stuff"')" fail
report subject_check_blank_description "$(subject 'feat:   ')" fail
report subject_check_blank_description_scoped "$(subject 'fix(ui):  ')" fail

exit "$fail"
