#!/usr/bin/env bash
# Builds Xpu Command for macOS and packages a DMG.
#
# Without Apple credentials: ad-hoc signed (users see "Apple could not verify…" once).
# With them: signed with your Developer ID and notarized by Apple (no warning):
#   APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   APPLE_ID=you@example.com  APPLE_PASSWORD=<app-specific password>  APPLE_TEAM_ID=TEAMID
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
ARCH=$(uname -m)
APP="src-tauri/target/release/bundle/macos/Xpu Command.app"
OUT="src-tauri/target/release/bundle/dmg"
DMG="$OUT/Xpu-Command-$VERSION-$ARCH.dmg"

notarize=false
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" && -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
  notarize=true
  echo "Signing as: $APPLE_SIGNING_IDENTITY (will notarize)"
else
  unset APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
  echo "No Apple credentials: building an ad-hoc signed app"
fi

[[ -d node_modules ]] || npm ci
npx tauri build --bundles app

codesign --verify --deep --strict "$APP"

mkdir -p "$OUT"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/Xpu Command.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "Xpu Command" -srcfolder "$STAGE" -ov -format UDZO "$DMG"

if $notarize; then
  codesign --sign "$APPLE_SIGNING_IDENTITY" --timestamp "$DMG"
  xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature -v "$DMG"
fi

hdiutil verify -quiet "$DMG"
echo
echo "Built: $DMG"
echo "SHA-256: $(shasum -a 256 "$DMG" | cut -d' ' -f1)"
