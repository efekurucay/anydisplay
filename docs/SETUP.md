# Your first Use That Screen connection

Install [UseThatScreen.apk](https://github.com/efekurucay/usethatscreen/releases/latest/download/UseThatScreen.apk)
on an Android 8.0+ phone or tablet, and download the host for your computer from
[the latest release](https://github.com/efekurucay/usethatscreen/releases/latest).
Install a matching release on both devices. Open the Android app and leave it on screen while
connecting.

## Mac: macOS 14 or later

1. Unzip the download and move **Use That Screen.app** into Applications.
2. Open it. The app is not notarized yet. If macOS blocks it, open **System Settings → Privacy &
   Security → Open Anyway** after the first launch attempt. Only bypass this for a download you trust.
3. Look for the overlapping screens icon in the **menu bar**; there is no main window or Dock icon.
4. Use **Allow Screen Recording…** and **Allow Accessibility…** in its menu when shown.
   Screen Recording enables video; Accessibility enables touch, pen and keyboard input.
   Restart Use That Screen after changing permissions if macOS asks.
5. Choose a connection below. For an extended desktop, arrange the new screen in
   **System Settings → Displays** and drag a window across to it.

To show one Mac window, select **Share a Window…** in the menu bar, choose a window,
then manually connect under **USB** or **Wi-Fi**. The view-only connection shares just
that window's image. **Use Desktop Mode for New Connections** restores desktop sharing
and automatic USB connections. Disconnect and reconnect to change an active stream's source.

## Windows: Windows 10 (1809) or Windows 11

Run the Windows installer and open **Use That Screen** from the Start menu. With the portable ZIP,
extract the files and open `usethatscreen-gui.exe`.

To connect from the command line, open a terminal in the folder containing the executable:

```powershell
.\usethatscreen-win.exe list
.\usethatscreen-win.exe usb
# Or, for Wi-Fi:
.\usethatscreen-win.exe connect 192.168.1.20
```

Replace the IP with the address shown by the Android app. Windows support is experimental.
It mirrors an existing monitor unless you install the optional
[Virtual Display Driver](../windows/README.md#extra-monitor-virtual-display-driver-setup).

## USB

1. Install [Android SDK Platform-Tools](https://developer.android.com/tools/releases/platform-tools)
   on the computer. On macOS with Homebrew, `brew install --cask android-platform-tools` is another
   option. Restart Use That Screen after installing it. For a custom location, set `USETHATSCREEN_ADB`
   to the full executable path when launching the host from a terminal.
2. If **Developer options** is hidden, find **Build number** in your phone's settings and tap it
   seven times. The [Android Developer options guide](https://developer.android.com/studio/debug/dev-options#enable)
   lists common locations. On Xiaomi and Redmi devices, look for **OS/MIUI version** under
   **About phone**, then find **Developer options** in **Additional settings**.
   See [Xiaomi's instructions](https://www.mi.com/uk/support/faq/details/KA-168765/).
3. Open **Developer options** and turn on **USB debugging**. Use a data-capable cable, unlock
   the device, and accept the **Allow USB debugging?** prompt for this computer.
4. Run `adb devices -l`. Your device must say `device`, not `unauthorized` or `offline`.
5. Keep Use That Screen open on Android. The Mac menu bar app connects authorized USB devices
   automatically by default. On Windows, select the device under **USB** in Use That Screen or run
   the USB command above.

For a device listed as `(no serial number)`, connect only one USB Android device at a time.
This case is supported on both hosts starting with v0.1.1. USB uses the existing ADB
computer authorization; it does not show the Wi-Fi pairing code.

## Wi-Fi

Put both devices on the same local network and keep Use That Screen open on Android. On Mac, select
it under **Wi-Fi** in the menu. On Windows, select it under **Wi-Fi** in Use That Screen or use
`list` and `connect <IP>` from the command line.
Compare the six-digit code on both screens and accept only if they match.
Wi-Fi sessions are encrypted after pairing.

If discovery finds nothing, use **Connect to Address…** on Mac or `connect <IP>` on Windows.
Check local-network permission, firewall rules and whether your router isolates wireless clients.
The Android app listens on TCP port **47820**. No router port forwarding is needed.

## Troubleshooting

| Symptom | What to check |
|---|---|
| Mac app seems to do nothing | Find the overlapping screens icon in the menu bar. |
| `adb not found` | Install Platform-Tools, check `adb version`, and restart Use That Screen. |
| No USB device | Try a data cable, unlock Android, enable debugging and run `adb devices -l`. |
| `unauthorized` | Accept Android's computer authorization prompt. Reconnect the cable if needed. |
| `offline` | Reconnect the device and check its debugging prompt before retrying. |
| More than one device | Select a serial with `usb SERIAL`; for an empty serial, unplug other USB devices. |
| Device found but connection fails | Open Use That Screen on Android; verify the app is listening before retrying. |
| Windows USB connects, shows a black screen, then times out | Compare with Wi-Fi and collect the USB diagnostics below. |
| Video is blank on Mac | Enable Screen Recording, restart the host, and check the display arrangement. |
| Picture works, touch does not on Mac | Enable Accessibility for the current Use That Screen app and restart it. |
| Permissions stopped working after an update | The Mac app is ad-hoc signed; re-enable its permissions for the new build. |
| Windows mirrors instead of extending | Install the optional Virtual Display Driver; see the Windows guide. |

### Windows USB connects and then times out

Try Wi-Fi on the same PC and Android device. If it shows video, use it while diagnosing USB.
Record `adb version` and the `usb_backend` field from `adb server-status`, then retry USB
with `usethatscreen-win.exe usb -v` and collect `adb logcat -s UseThatScreen`.

If `adb server-status` shows `LIBADBUSB`, compare with the alternate Windows USB backend
documented in the [Platform-Tools release notes](https://developer.android.com/tools/releases/platform-tools#37.0.1_(july_2026)).
Run this in PowerShell after ending any active ADB sessions.

```powershell
adb kill-server
$env:ADB_USB_LEGACY = '1'
adb start-server
adb server-status
.\usethatscreen-win.exe usb -v
```

To restore the default backend, stop Use That Screen, run `adb kill-server`, remove the override with
`Remove-Item Env:ADB_USB_LEGACY`, and run `adb start-server` again.

Still stuck? [Report the issue](https://github.com/efekurucay/usethatscreen/issues/new?template=bug_report.yml)
with your host OS, Android model/version, connection type and error text. Never include pairing
secrets or signing keys. Successful [device reports](https://github.com/efekurucay/usethatscreen/issues/new?template=device_report.yml)
are welcome too.
