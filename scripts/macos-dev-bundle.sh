#!/usr/bin/env bash
# Builds target/ntfy-native.app for local development and signs it
# (ad-hoc unless MACOS_SIGN_IDENTITY is set).
# Usage: scripts/macos-dev-bundle.sh [--example NAME]
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/macos-bundle-lib.sh
source scripts/macos-bundle-lib.sh

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

app=target/ntfy-native.app
assemble_app "$bin" "$app"

if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  sign_app "$app" "$MACOS_SIGN_IDENTITY" packaging/macos/ntfy-native.entitlements
else
  echo "note: ad-hoc signature without time-sensitive entitlement" >&2
  sign_app "$app" -
fi

echo "$app"
