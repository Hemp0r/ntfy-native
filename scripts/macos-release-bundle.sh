#!/usr/bin/env bash
# Builds ntfy-native.app and ntfy-native-<version>-macos-universal.dmg in <out-dir>
# from an already built (universal) binary.
# Usage: scripts/macos-release-bundle.sh <binary> <out-dir>
#
# Environment (all optional):
#   MACOS_SIGN_IDENTITY          Developer ID identity; ad-hoc signature when unset
#   MACOS_PROVISION_PROFILE_FILE path to a provisioning profile (needs an identity);
#                                embedded, and only then the time-sensitive
#                                entitlement is signed in (taken from the profile)
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                                notarize + staple the dmg (needs MACOS_SIGN_IDENTITY)
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/macos-bundle-lib.sh
source scripts/macos-bundle-lib.sh

[[ $# -eq 2 ]] || { echo "usage: $0 <binary> <out-dir>" >&2; exit 2; }
bin=$1
out=$2
mkdir -p "$out"
out=$(cd "$out" && pwd)

version=$(cargo_version)
app="$out/ntfy-native.app"
dmg="$out/ntfy-native-$version-macos-universal.dmg"
identity=${MACOS_SIGN_IDENTITY:-}

assemble_app "$bin" "$app"

entitlements=
if [[ -n "$identity" ]]; then
  if [[ -n "${MACOS_PROVISION_PROFILE_FILE:-}" ]]; then
    cp "$MACOS_PROVISION_PROFILE_FILE" "$app/Contents/embedded.provisionprofile"
    entitlements=$(mktemp)
    entitlements_from_profile "$app/Contents/embedded.provisionprofile" "$entitlements"
  else
    echo "::warning::No provisioning profile: signing without the time-sensitive entitlement"
  fi
  MACOS_CODESIGN_TIMESTAMP=1 sign_app "$app" "$identity" "$entitlements"
else
  if [[ -n "${MACOS_PROVISION_PROFILE_FILE:-}" ]]; then
    echo "::warning::Provisioning profile ignored: it needs a signing identity"
  fi
  echo "::warning::No signing identity: ad-hoc signature, Gatekeeper will warn"
  sign_app "$app" -
fi

staging=$(mktemp -d)
trap 'rm -rf "$staging" "$entitlements"' EXIT
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
rm -f "$dmg"
hdiutil create -volname ntfy-native -srcfolder "$staging" -ov -format UDZO "$dmg"

if [[ -n "$identity" ]]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
  if [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
    submit_out=$(mktemp)
    xcrun notarytool submit "$dmg" --wait --timeout 30m \
      --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" \
      2>&1 | tee "$submit_out" || true
    if ! grep -q 'status: Accepted' "$submit_out"; then
      submission_id=$(sed -n 's/^ *id: *//p' "$submit_out" | head -n 1)
      if [[ -n "$submission_id" ]]; then
        xcrun notarytool log "$submission_id" \
          --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" || true
      fi
      echo "::error::Notarization was not accepted" >&2
      exit 1
    fi
    rm -f "$submit_out"
    xcrun stapler staple "$dmg"
  else
    echo "::warning::Apple notarization credentials missing: dmg is not notarized"
  fi
else
  echo "::warning::Not signed with an identity: dmg is not notarized"
fi

echo "$dmg"
