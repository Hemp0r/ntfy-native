#!/usr/bin/env bash
# Shared app-bundle assembly for macos-dev-bundle.sh and macos-release-bundle.sh.
# Source this file; it only defines functions.

repo_root() { cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd; }

cargo_version() {
  sed -n 's/^version = "\([^"]*\)".*/\1/p' "$(repo_root)/Cargo.toml" | head -n 1
}

# assemble_app <binary> <app-dir>: creates the bundle skeleton (unsigned).
assemble_app() {
  local bin=$1 app=$2 root version
  root=$(repo_root)
  version=$(cargo_version)
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$bin" "$app/Contents/MacOS/ntfy-native"
  cp "$root/assets/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
  sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
}

# sign_app <app-dir> <identity|-> [entitlements-file]
# macOS refuses to launch code that claims the restricted time-sensitive
# entitlement without a matching provisioning profile, so entitlements are
# opt-in. Release builds add --timestamp via MACOS_CODESIGN_TIMESTAMP=1.
sign_app() {
  local app=$1 identity=$2 entitlements=${3:-}
  local args=(--force --options runtime --sign "$identity")
  if [[ -n "$entitlements" ]]; then
    args+=(--entitlements "$entitlements")
  fi
  if [[ "${MACOS_CODESIGN_TIMESTAMP:-0}" == 1 ]]; then
    args+=(--timestamp)
  fi
  codesign "${args[@]}" "$app"
}

# entitlements_from_profile_plist <decoded-profile.plist> <out.plist>
# Keeps only the entitlements we claim, taken from the (decoded) profile so
# application/team identifiers match it. Fails if the profile is for another app.
entitlements_from_profile_plist() {
  local profile=$1 out=$2 all appid team
  all=$(mktemp)
  plutil -extract Entitlements xml1 -o "$all" "$profile"
  appid=$(plutil -extract 'com\.apple\.application-identifier' raw -o - "$all")
  if [[ "$appid" != *.dev.hemp0r.ntfy-native ]]; then
    echo "::error::Provisioning profile is for '$appid', expected *.dev.hemp0r.ntfy-native" >&2
    rm -f "$all"
    return 1
  fi
  team=$(plutil -extract 'com\.apple\.developer\.team-identifier' raw -o - "$all")
  if [[ "$(plutil -extract 'com\.apple\.developer\.usernotifications\.time-sensitive' raw -o - "$all")" != true ]]; then
    echo "::error::Provisioning profile lacks the time-sensitive notifications entitlement" >&2
    rm -f "$all"
    return 1
  fi
  plutil -create xml1 "$out"
  plutil -insert 'com\.apple\.application-identifier' -string "$appid" "$out"
  plutil -insert 'com\.apple\.developer\.team-identifier' -string "$team" "$out"
  plutil -insert 'com\.apple\.developer\.usernotifications\.time-sensitive' -bool true "$out"
  rm -f "$all"
}

# entitlements_from_profile <profile.provisionprofile> <out.plist>
entitlements_from_profile() {
  local decoded
  decoded=$(mktemp)
  security cms -D -i "$1" > "$decoded"
  entitlements_from_profile_plist "$decoded" "$2"
  local rc=$?
  rm -f "$decoded"
  return $rc
}
