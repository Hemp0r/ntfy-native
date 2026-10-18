#!/usr/bin/env bash
# Builds target/ntfy-native.app for local development and signs it
# (ad-hoc unless MACOS_SIGN_IDENTITY is set).
# Usage: scripts/macos-dev-bundle.sh [--example NAME]
set -euo pipefail
cd "$(dirname "$0")/.."

build_args=(--release)
bin=target/release/ntfy-native
case $# in
  0) ;;
  2)
    [[ "$1" == "--example" ]] || { echo "usage: $0 [--example NAME]" >&2; exit 2; }
    build_args+=(--example "$2")
    bin="target/release/examples/$2"
    ;;
  *) echo "usage: $0 [--example NAME]" >&2; exit 2 ;;
esac

cargo build "${build_args[@]}"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
app=target/ntfy-native.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$bin" "$app/Contents/MacOS/ntfy-native"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# macOS refuses to launch ad-hoc signed code that claims the restricted
# time-sensitive entitlement, so it is only added for a real identity.
sign_args=(--force --options runtime --sign "${MACOS_SIGN_IDENTITY:--}")
if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  sign_args+=(--entitlements packaging/macos/ntfy-native.entitlements)
else
  echo "note: ad-hoc signature without time-sensitive entitlement" >&2
fi
codesign "${sign_args[@]}" "$app"

echo "$app"
