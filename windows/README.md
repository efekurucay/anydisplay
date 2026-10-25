# AnyDisplay for Windows

The Windows host of AnyDisplay. An Android tablet running the AnyDisplay client becomes an extra monitor
and pen tablet for a Windows PC. It speaks `docs/PROTOCOL.md` (version 1) and interoperates with
`tools/anydisplay_ref.py` and the macOS host.

| Layer | Implementation |
|---|---|
| Capture | DXGI Desktop Duplication of one output, GPU BGRA→NV12 with the D3D11 video processor (BT.709 limited range) |
| Encode | Media Foundation hardware H.264 / HEVC MFT (async, D3D11 surfaces, low-latency, CBR, no B-frames, keyframe on request); software MFT fallback with NOTICE 1 |
| Display | Captures the [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) monitor when installed (extend mode), otherwise mirrors a monitor and sends NOTICE 5 |
| Input | Pen and up to 10 touch contacts via `CreateSyntheticPointerDevice` / `InjectSyntheticPointerInput` (pressure, tilt, twist, eraser, barrel); keys via `SendInput` scan codes from HID usages; TEXT via `KEYEVENTF_UNICODE`; mouse and wheel via `SendInput` |
| Cursor | Composited into the frame (duplication frames never contain it), or sent as CURSOR_POS / CURSOR_SHAPE when the client draws it |
| Audio | WASAPI loopback of the default output → PCM s16le 48 kHz stereo |
| Clipboard | `text/plain` both ways |
| Transport | Wi-Fi (mDNS `_anydisplay._tcp` discovery, or a typed address) and USB (`adb forward`) |

## Build

Needs Rust 1.82 or newer.

```sh
cargo build --release                 # on Windows: target\release\anydisplay-win.exe
cargo test                            # any OS: protocol, crypto vectors, sessions, input translation
cargo check --target x86_64-pc-windows-msvc   # check the Windows code from macOS / Linux
```

For the last one, run `rustup target add x86_64-pc-windows-msvc` first. It only type-checks:
linking a Windows binary needs the MSVC toolchain.

## Use

```text
anydisplay-win list [--seconds N]              monitors, USB (adb) and Wi-Fi (mDNS) clients, paired devices
anydisplay-win usb [SERIAL] [options]          connect to a USB client through adb forward
anydisplay-win connect HOST[:PORT] [options]   connect to a client over the network (default port 47820)

  --monitor N        capture monitor N from `list` (default: the virtual display if installed,
                     else the primary monitor)
  --codec h264|hevc  preferred codec (used when the client can decode it)
  -y, --yes          accept the pairing code without asking
  --seconds N        end the session N seconds after streaming starts
  --no-clipboard     do not share the clipboard
  --no-audio         never stream audio
  --test-source      stream an ffmpeg test pattern instead of the desktop (any OS)
  -v, --verbose      debug logging
```

The first Wi-Fi connection to a tablet pairs. Both screens show a 6-digit code; type `y` in the
console if they match and confirm on the tablet. Later connections use the stored pairing. USB
connections (adb) skip pairing.

Environment variables:

| Variable | Meaning |
|---|---|
| `ANYDISPLAY_STORE` | pairing store path (default `%APPDATA%\AnyDisplay\host.json`, same layout as the macOS host) |
| `ANYDISPLAY_ADB` | `adb` path (otherwise `PATH`, `ANDROID_HOME`, `%LOCALAPPDATA%\Android\Sdk\platform-tools`) |
| `ANYDISPLAY_FFMPEG` | `ffmpeg` path for `--test-source` |

`adb` comes from the Android SDK Platform-Tools. The tablet needs USB debugging enabled.

## Extra monitor: Virtual Display Driver setup

Windows only adds a monitor when a display driver provides one. AnyDisplay does not ship a driver. It
uses the open-source, MIT-licensed Virtual Display Driver (an IddCx driver by VirtualDrivers /
MikeTheTech) when that driver is installed.

1. Install it with `winget install --id=VirtualDrivers.Virtual-Display-Driver -e`, or run the
   *Virtual Driver Control* app from the project's
   [releases](https://github.com/VirtualDrivers/Virtual-Display-Driver/releases) and click
   Install. If you see a `vcruntime140.dll` error, install the Microsoft Visual C++ Redistributable.
2. Make the driver offer your tablet's resolution in both orientations, for example
   `2560x1600` and `1600x2560` at 60 Hz. Resolutions and the monitor count are configured in
   `C:\VirtualDisplayDriver\vdd_settings.xml` or through the Control app; the project wiki has
   the file format. Restart the driver after editing.
3. In *Settings → System → Display*, place the new monitor next to your main display.
4. Run `anydisplay-win list`. The driver's monitor is marked `[Virtual Display Driver]`.

On each stream, anydisplay-win:

- picks that monitor automatically (unless the client asks to mirror or `--monitor` is given);
- switches it to the stream size with `ChangeDisplaySettingsEx` if the driver offers that mode,
  without saving the change to the registry (the registry mode comes back when the session ends);
- if the driver lacks the mode, keeps the monitor's current mode, scales into the stream and sends
  NOTICE 4.

The monitor is recognized by its default EDID: PnP id `MTT`, product `0x1337`, name
"VDD by MTT". A custom EDID loaded into the driver hides it, so use `--monitor N` in that case.

Without the driver, anydisplay-win mirrors the primary monitor (or `--monitor N`), keeps that monitor's
aspect ratio inside the requested size, sets the `mirror` flag in CONFIG and sends NOTICE 5.

## Running as administrator

Windows does not let a normal process inject input into elevated windows (UIPI). When an
injection is refused, anydisplay-win sends NOTICE 3 once. Run it elevated to control elevated windows.
Desktop Duplication pauses on the secure desktop (UAC prompt, lock screen); capture resumes
on its own when the normal desktop comes back.

## Demo on macOS or Linux

Off Windows, the same CLI streams an ffmpeg `testsrc2` pattern (libx264 / libx265) through the
real session code. The reference client in `tools/` plays the tablet:

```sh
# terminal 1: fake tablet on port 47831, saves the received stream
tools/.venv/bin/python tools/anydisplay_ref.py client --port 47831 -y --out /tmp/anydisplay.h264

# terminal 2: loopback = mode 0; a LAN address = mode 1 pairing, then mode 2 on reconnect
cargo run -- connect 127.0.0.1:47831 --seconds 5 -y

ffprobe -count_frames -show_entries stream=codec_name,width,height,nb_read_frames /tmp/anydisplay.h264
```

A KEYFRAME_REQ restarts ffmpeg, whose first output is an IDR frame with SPS/PPS.

## Code map

```text
src/
  main.rs            anydisplay-win CLI (list / usb / connect, SAS prompt, Ctrl-C → BYE)
  lib.rs
  proto/             PROTOCOL.md, platform-independent
    wire.rs          little-endian primitives, str / blob
    messages.rs      every message body, channel / type constants
    frame.rs         framing, 32 MiB limit, AES-256-GCM per direction
    crypto.rs        P-256 ECDH, SHA-256, HKDF key schedule, confirmation MACs, SAS
    handshake.rs     host handshake state machine (sans-IO)
  session.rs         connect, handshake driver, unknown_host re-pair, keepalive, priority queues,
                     flow control (3 unacked), CONFIG negotiation, input / clipboard / audio routing
  negotiate.rs       stream size / rate / codec / bitrate from DEVICE_INFO + CONFIG_REQ
  platform.rs        Platform / VideoPipeline traits the session drives
  testsrc.rs         ffmpeg test-pattern platform (any OS)
  pairing.rs         host identity + paired devices (JSON)
  adb.rs             adb discovery and port forwarding
  discovery.rs       mDNS browse for _anydisplay._tcp
  input.rs           coordinate mapping, touch slots, pen states, modifier sync, wheel deltas
  keymap.rs          HID usage → scan code / virtual key
  cursor.rs          DXGI pointer shapes → RGBA, CPU compositing
  audio_convert.rs   any mix format → s16le 48 kHz stereo
  annexb.rs          NAL scanning, keyframes, in-band parameter sets
  vdd.rs             Virtual Display Driver monitor recognition
  win/               #[cfg(windows)]
    platform.rs      WinPlatform: monitor choice, VDD mode, capture + encoder per stream
    monitors.rs      DXGI outputs + DisplayConfig names, display mode switching
    capture.rs       Desktop Duplication, video processor NV12, cursor
    encoder.rs       Media Foundation encoder MFTs
    input.rs         synthetic pointer + SendInput injection
    audio.rs         WASAPI loopback
    clipboard.rs     Win32 clipboard
tests/
  vectors.rs         every value in tools/vectors.json, plus the host state machine on them
  session.rs         full sessions against an in-process client (modes 0/1/2, flow control,
                     keyframes, re-CONFIG, input, clipboard, BYE, 5 s liveness, rejected SAS)
  testsrc.rs         ffmpeg pipeline keyframes
```

## Not yet verified on Windows hardware

The Windows layers type-check with the `windows` crate but have not run on a Windows machine:
capture, the video processor, the encoders (hardware and software), input injection, audio,
clipboard and the virtual display mode switch. Known limits:

- **Rotated monitors.** On a rotated monitor the video processor rotates the frame, but the
  cursor is not drawn into it.
- **Hybrid-GPU laptops.** A hardware encoder must belong to the GPU that owns the duplicated
  monitor. If it doesn't, anydisplay-win falls back to software encoding.
- **Stationary touch.** Contacts that don't move are re-sent every 100 ms. This keeps them alive;
  the exact Windows timeout was not measured.
