# Use That Screen for iPad

The iPadOS 17+ client displays a Mac or Windows desktop over the local network.
Choose **Extend** or **Mirror** on the iPad, then select the iPad from the
computer app's **Wi-Fi** list. The first connection asks you to compare six
digits on both devices. Pairing keys are stored in the iPad Keychain.

This build receives H.264 video and supports display viewing. Audio, touch
forwarding, Apple Pencil, keyboard, clipboard and USB are outside this build.
Windows requires a virtual display driver to extend the desktop.

## Build

Install Xcode with the iOS SDK and [XcodeGen](https://github.com/yonaskolb/XcodeGen).
From this directory:

```sh
xcodegen generate
open UseThatScreen.xcodeproj
```

Run the **UseThatScreen** scheme on an iPad simulator, or select your Apple
development team under **Signing & Capabilities** and run it on a connected iPad.
Keep Xcode's simulator signing enabled so the app can use the Keychain. Allow
local-network access when iPadOS asks.

For a simulator video check, keep the app open and run the
[reference host](../tools/usethatscreen_ref.py) against `127.0.0.1:47820` with
Python, `cryptography`, and `ffmpeg` installed.
