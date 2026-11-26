#!/bin/bash
# Builds the signed APK and the macOS app and publishes them as a GitHub release, each under a
# versioned name and a fixed name: releases/latest/download/<fixed name> always serves the newest
# build, so the README and website links never change. The Windows zip is added by
# .github/workflows/release.yml once the release is published.
#
# usage: scripts/release.sh v0.2.0 NOTES.md
set -euo pipefail
TAG="${1:?usage: scripts/release.sh vX.Y.Z NOTES.md}"
NOTES="$(cd "$(dirname "${2:?usage: scripts/release.sh vX.Y.Z NOTES.md}")" && pwd)/$(basename "$2")"
cd "$(dirname "$0")/.."
VERSION="${TAG#v}"
# Resolve the repository before packaging changes the working directory to a temp folder.
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"

grep -q "versionName = \"$VERSION\"" android/app/build.gradle.kts \
    || { echo "android/app/build.gradle.kts versionName is not $VERSION" >&2; exit 1; }

(cd android && ./gradlew -q assembleRelease)
USETHATSCREEN_VERSION="$VERSION" macos/scripts/bundle.sh

OUT="$(mktemp -d)"
cp android/app/build/outputs/apk/release/app-release.apk "$OUT/UseThatScreen-$TAG.apk"
(cd macos/build && ditto -c -k --keepParent "Use That Screen.app" "$OUT/UseThatScreen-$TAG-macOS.zip")
cd "$OUT"
shasum -a 256 "UseThatScreen-$TAG.apk" "UseThatScreen-$TAG-macOS.zip" > SHA256SUMS.txt
cp "UseThatScreen-$TAG.apk" UseThatScreen.apk
cp "UseThatScreen-$TAG-macOS.zip" UseThatScreen-macOS.zip

gh release create "$TAG" --repo "$REPO" --title "Use That Screen $TAG" --notes-file "$NOTES" --latest \
    "UseThatScreen-$TAG.apk" "UseThatScreen-$TAG-macOS.zip" SHA256SUMS.txt UseThatScreen.apk UseThatScreen-macOS.zip
