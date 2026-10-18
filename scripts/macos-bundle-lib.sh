#!/usr/bin/env bash
# Shared app-bundle assembly for macos-dev-bundle.sh and macos-release-bundle.sh.
# Source this file; it only defines functions.

repo_root() { cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd; }

cargo_version() {
  sed -n 's/^version = "\(.*\)"$/\1/p' "$(repo_root)/Cargo.toml" | head -n 1
}

# assemble_app <binary> <app-dir>: creates the bundle skeleton (unsigned).
assemble_app() {
  local bin=$1 app=$2 root version
  root=$(repo_root)
  version=$(cargo_version)
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS"
  cp "$bin" "$app/Contents/MacOS/ntfy-native"
  sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
}

# sign_app <app-dir> <identity|-> <with-entitlements: 1|0>
# macOS refuses to launch code that claims the restricted time-sensitive
# entitlement without a matching provisioning profile, so entitlements are
# opt-in. Release builds add --timestamp via MACOS_CODESIGN_TIMESTAMP=1.
sign_app() {
  local app=$1 identity=$2 entitlements=$3
  local args=(--force --options runtime --sign "$identity")
  if [[ "$entitlements" == 1 ]]; then
    args+=(--entitlements "$(repo_root)/packaging/macos/ntfy-native.entitlements")
  fi
  if [[ "${MACOS_CODESIGN_TIMESTAMP:-0}" == 1 ]]; then
    args+=(--timestamp)
  fi
  codesign "${args[@]}" "$app"
}
