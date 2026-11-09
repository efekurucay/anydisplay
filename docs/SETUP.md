# Your first AnyDisplay connection

Install [AnyDisplay.apk](https://github.com/efekurucay/anydisplay/releases/latest/download/AnyDisplay.apk)
on an Android 8.0+ phone or tablet, and download the host for your computer from
[the latest release](https://github.com/efekurucay/anydisplay/releases/latest).
Open the Android app and leave it on screen while connecting.

## Mac: macOS 14 or later

1. Unzip the download and move **AnyDisplay.app** into Applications.
2. Open it. The app is not notarized yet. If macOS blocks it, open **System Settings → Privacy &
   Security → Open Anyway** after the first launch attempt. Only bypass this for a download you trust.
3. Look for the overlapping screens icon in the **menu bar**; there is no main window or Dock icon.
4. Use **Allow Screen Recording…** and **Allow Accessibility…** in its menu when shown.
   Screen Recording enables video; Accessibility enables touch, pen and keyboard input.
   Restart AnyDisplay after changing permissions if macOS asks.
5. Choose a connection below. For an extended desktop, arrange the new screen in
   **System Settings → Displays** and drag a window across to it.

## Windows: Windows 10 (1809) or Windows 11

Unzip the download. Open a terminal in the extracted folder:

```powershell
.\anydisplay-win.exe list
.\anydisplay-win.exe usb
# Or, for Wi-Fi:
.\anydisplay-win.exe connect 192.168.1.20
```

Replace the IP with the address shown by the Android app. Windows support is experimental.
It mirrors an existing monitor unless you install the optional
[Virtual Display Driver](../windows/README.md#extra-monitor-virtual-display-driver-setup).

## USB

1. Install [Android SDK Platform-Tools](https://developer.android.com/tools/releases/platform-tools)
   on the computer. On macOS with Homebrew, `brew install --cask android-platform-tools` is another
   option. Restart AnyDisplay after installing it. For a custom location, set `ANYDISPLAY_ADB`
   to the full executable path when launching the host from a terminal.
2. Enable **Developer options → USB debugging** on Android. Use a data-capable cable, unlock
   the device, and accept the **Allow USB debugging?** prompt for this computer.
3. Run `adb devices -l`. Your device must say `device`, not `unauthorized` or `offline`.
4. Keep AnyDisplay open on Android. The Mac menu bar app connects authorized USB devices
   automatically by default; on Windows run the USB command above.

For a device listed as `(no serial number)`, connect only one USB Android device at a time.
This case is supported on both hosts starting with v0.1.1. USB uses the existing ADB
computer authorization; it does not show the Wi-Fi pairing code.

## Wi-Fi

Put both devices on the same local network and keep AnyDisplay open on Android. On Mac, select
it under **Wi-Fi** in the menu. On Windows, use `list` to discover it, then `connect <IP>`.
Compare the six-digit code on both screens and accept only if they match.
Wi-Fi sessions are encrypted after pairing.

If discovery finds nothing, use **Connect to Address…** on Mac or `connect <IP>` on Windows.
Check local-network permission, firewall rules and whether your router isolates wireless clients.
The Android app listens on TCP port **47820**. No router port forwarding is needed.

## Troubleshooting

| Symptom | What to check |
|---|---|
| Mac app seems to do nothing | Find the overlapping screens icon in the menu bar. |
| `adb not found` | Install Platform-Tools, check `adb version`, and restart AnyDisplay. |
| No USB device | Try a data cable, unlock Android, enable debugging and run `adb devices -l`. |
| `unauthorized` | Accept Android's computer authorization prompt. Reconnect the cable if needed. |
| `offline` | Reconnect the device and check its debugging prompt before retrying. |
| More than one device | Select a serial with `usb SERIAL`; for an empty serial, unplug other USB devices. |
| Device found but connection fails | Open AnyDisplay on Android; verify the app is listening before retrying. |
| Video is blank on Mac | Enable Screen Recording, restart the host, and check the display arrangement. |
| Picture works, touch does not on Mac | Enable Accessibility for the current AnyDisplay app and restart it. |
| Permissions stopped working after an update | The Mac app is ad-hoc signed; re-enable its permissions for the new build. |
| Windows mirrors instead of extending | Install the optional Virtual Display Driver; see the Windows guide. |

Still stuck? [Report the issue](https://github.com/efekurucay/anydisplay/issues/new?template=bug_report.yml)
with your host OS, Android model/version, connection type and error text. Never include pairing
secrets or signing keys. Successful [device reports](https://github.com/efekurucay/anydisplay/issues/new?template=device_report.yml)
are welcome too.
