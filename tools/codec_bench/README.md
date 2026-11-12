# Codec measurements

These tools feed deterministic 1920×1080, 60 FPS NV12 frames through the application's actual
Mac `VideoEncoder.swift`. They require macOS, Swift, FFmpeg, and Python with Pillow. Android
replay also uses the `cryptography` dependency of `tools/anydisplay_ref.py` and an authorized
USB device with AnyDisplay open.

Keep output outside the repository; raw fixtures need approximately 1.2 GB. From the repository root:

```sh
OUT=/tmp/anydisplay-codecs
python3 tools/codec_bench/generate.py "$OUT"
swiftc -O -swift-version 5 -emit-module -emit-library -module-name AnyDisplayProtocol \
  macos/Sources/AnyDisplayProtocol/*.swift \
  -emit-module-path "$OUT/AnyDisplayProtocol.swiftmodule" -o "$OUT/libAnyDisplayProtocol.dylib"
swiftc -O -swift-version 5 -I "$OUT" -L "$OUT" -lAnyDisplayProtocol \
  -Xlinker -rpath -Xlinker "$OUT" macos/Sources/AnyDisplayHost/Log.swift \
  macos/Sources/AnyDisplayHost/VideoEncoder.swift tools/codec_bench/encode.swift -o "$OUT/encode"
for scene in desktop motion; do
  for rate in 5000 7000 10000 20000; do
    for codec in h264 hevc; do
      "$OUT/encode" "$OUT/$scene.nv12" 1920 1080 60 "$rate" "$codec" "$OUT/$scene-$codec-$rate.$codec"
    done
  done
done
python3 tools/codec_bench/metrics.py "$OUT"
```

`generate.py` creates a scrolling small-text desktop and FFmpeg's moving `testsrc2` pattern.
The NV12 conversion uses BT.709 video range, matching Mac capture. `encode.swift` paces frames
at 60 FPS and records hardware use, bytes, output frame sizes, keyframes, and callback latency.
The first 30 frames are excluded from encoding latency statistics. An incomplete encode fails.
`metrics.py` uses FFmpeg SSIM and PSNR on decoded frames against the exact NV12 reference.
Bitrate is a target; report actual output bitrate as well.

For device measurements, select the intended serial explicitly and create a dedicated forward:

```sh
SERIAL=YOUR_DEVICE_SERIAL
PORT=$(adb -s "$SERIAL" forward tcp:0 tcp:47820)
python3 tools/codec_bench/replay.py "$OUT/desktop-h264-10000.h264" --port "$PORT" --seconds 15
python3 tools/codec_bench/replay.py "$OUT/desktop-hevc-10000.hevc" --port "$PORT" --seconds 15
adb -s "$SERIAL" forward --remove "tcp:$PORT"
```

Use the same device settings and repeat in alternating order. Replay uses the same three-frame
ACK limit as the host and restarts each 180-frame fixture with its keyframe. It saves STATS and
per-frame ACK timings beside the input stream. The first 60 frames are excluded from latency
percentiles; zero-duration drop ACKs are excluded, and STATS retains startup drops. Copy each `.android.json` between repeated runs to
retain them. Pairing state, when needed, stays in the output directory.

The Android `decode_ms` field measures codec queue-to-output time. `ack_ms` includes USB and
application handling. Neither measures physical display scanout, capture, or full input-to-photon
latency. Phone thermal state, startup, USB scheduling, and background host load can affect timing.

The speed-priority experiment changes only `kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality`
to false in a temporary copy of `VideoEncoder.swift`, compiles that copy instead of the production
file, and repeats the 10 Mbps cases. Compare encoded hashes and quality scores before drawing a
conclusion from timing differences.
