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

## Test on a physical iPad

1. Install the iPad app from Xcode and put the iPad and computer on the same Wi-Fi.
2. Select **Mirror** on the iPad. Confirm that the computer lists the iPad under
   **Wi-Fi**, then select it and compare the six-digit code on both devices.
3. Move a window on the computer and check that the iPad updates. Rotate the
   iPad and check that the picture still fits.
4. Disconnect and reconnect. Check that the same computer connects without a new code.
5. Disconnect, select **Extend**, and reconnect. Arrange the displays on the
   computer, then drag a window onto the iPad display. On Windows, install
   the virtual display driver first.

[Report the result on GitHub](https://github.com/efekurucay/usethatscreen/issues/new?title=%5BiPad%5D%20Device%20test).
Include the iPad model, iPadOS version, computer OS and host build, iPad build
commit, and the first step that failed or passed.
