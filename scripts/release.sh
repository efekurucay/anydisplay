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

grep -q "versionName = \"$VERSION\"" android/app/build.gradle.kts \
    || { echo "android/app/build.gradle.kts versionName is not $VERSION" >&2; exit 1; }

(cd android && ./gradlew -q assembleRelease)
ANYDISPLAY_VERSION="$VERSION" macos/scripts/bundle.sh

OUT="$(mktemp -d)"
cp android/app/build/outputs/apk/release/app-release.apk "$OUT/AnyDisplay-$TAG.apk"
(cd macos/build && ditto -c -k --keepParent AnyDisplay.app "$OUT/AnyDisplay-$TAG-macOS.zip")
cd "$OUT"
shasum -a 256 "AnyDisplay-$TAG.apk" "AnyDisplay-$TAG-macOS.zip" > SHA256SUMS.txt
cp "AnyDisplay-$TAG.apk" AnyDisplay.apk
cp "AnyDisplay-$TAG-macOS.zip" AnyDisplay-macOS.zip

gh release create "$TAG" --title "AnyDisplay $TAG" --notes-file "$NOTES" --latest \
    "AnyDisplay-$TAG.apk" "AnyDisplay-$TAG-macOS.zip" SHA256SUMS.txt AnyDisplay.apk AnyDisplay-macOS.zip
