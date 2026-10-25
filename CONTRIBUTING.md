# Contributing to AnyDisplay

Thanks for helping. Bug reports, device test results and pull requests are all useful.

## Reporting a bug

Include:

- the host OS and version, and the device model and Android version
- the link (USB or Wi-Fi) and the codec
- host output with `-v` (`anydisplay -v …` or `anydisplay-win -v …`)
- device logs: `adb logcat -s AnyDisplay`

## Pull requests

- Keep each pull request focused on one change.
- Run the tests for every part you touched:

  ```sh
  (cd macos && swift test)
  (cd windows && cargo test && cargo check --target x86_64-pc-windows-msvc)
  (cd android && ./gradlew testDebugUnitTest)
  ```

- A protocol change touches `docs/PROTOCOL.md`, `tools/anydisplay_ref.py`, `tools/vectors.json`
  and all three implementations in the same pull request. Regenerate the vectors with
  `tools/anydisplay_ref.py vectors > tools/vectors.json`.
- Follow the style of the surrounding code.

## Releasing (maintainers)

1. Set the version in `android/app/build.gradle.kts` (`versionName`, `versionCode`) and
   `windows/Cargo.toml`, and commit.
2. Write the release notes to a file, then run `scripts/release.sh vX.Y.Z NOTES.md`. It needs the
   Android release key configured through Gradle properties (`anydisplay.storeFile`,
   `anydisplay.storePassword`, `anydisplay.keyAlias`, `anydisplay.keyPassword`).
3. The release workflow adds the Windows zip a few minutes later.

## Security issues

Do not open a public issue for a vulnerability. Report it privately through GitHub's
**Security → Report a vulnerability** form on the repository.
