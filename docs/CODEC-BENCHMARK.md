# H.264 and HEVC measurements

Measured on 2026-11-12 using an Apple M5 Mac and a Huawei P30 Pro (VOG-L09, Android 10)
running AnyDisplay 0.1.2 over USB. The encoder source is commit `72d252f`.
The [machine-readable results](benchmarks/2026-11-12-codecs.json) retain encoder measurements,
Android STATS, repeated-run summaries, and the speed-priority comparison hashes.

## Method

- Feed the application's actual Mac `VideoEncoder.swift` deterministic NV12 frames matching the capture format.
- Use 1920×1080 at 60 FPS, 180 frames per scene: scrolling small text and a moving FFmpeg `testsrc2` pattern.
- Compare H.264 and HEVC at identical target bitrates: 5, 7, 10, and 20 Mbps. Record actual output bitrate too.
- Decode output with FFmpeg and compare against the exact reference using SSIM and PSNR. Higher scores indicate greater signal fidelity; they are not a subjective readability test.
- Exclude the first 30 frames from encoder latency percentiles. All 20 encoding runs produced 180/180 frames without encoder drops, including four speed-priority variants.
- Replay encoded access units through the real Android app with the host's three-frame ACK limit. Desktop 10 Mbps runs alternate codecs and repeat three times (15, 12, and 12 seconds); motion runs last 15 seconds each.
- Exclude the first 60 submitted frames from Android latency percentiles. Keep startup drops in STATS instead of presenting them as steady-state behavior.

Commands and dependencies are in [the benchmark tool instructions](../tools/codec_bench/README.md).
FFmpeg documents the [SSIM and PSNR filters](https://ffmpeg.org/ffmpeg-filters.html).

## Image quality

These are target bitrates. In low-complexity scenes the encoder can use less than the target;
the complete results include actual output bitrate and encoding times.

| Scene | Target Mbps | H.264 SSIM | HEVC SSIM | H.264 PSNR dB | HEVC PSNR dB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Scrolling desktop | 5 | 0.988569 | 0.992260 | 41.76 | 43.62 |
| Scrolling desktop | 7 | 0.994020 | 0.994876 | 45.63 | 45.92 |
| Scrolling desktop | 10 | 0.996189 | 0.996612 | 48.06 | 48.03 |
| Scrolling desktop | 20 | 0.997509 | 0.997760 | 50.61 | 50.49 |
| Motion | 5 | 0.983983 | 0.983826 | 39.07 | 39.62 |
| Motion | 7 | 0.989001 | 0.988437 | 40.74 | 40.86 |
| Motion | 10 | 0.993114 | 0.992617 | 42.82 | 42.90 |
| Motion | 20 | 0.998339 | 0.998166 | 49.94 | 49.88 |

HEVC has a useful advantage for the small-text desktop at 5 Mbps on this encoder. At 10 Mbps
the quality difference is small and depends on the metric/content. Codec choice alone does not
justify a universal quality claim or changing the automatic codec preference.

Reducing H.264 from 10 to 7 Mbps lowers desktop PSNR from 48.06 to 45.63 dB and motion PSNR
from 42.82 to 40.74 dB in these fixtures. Codec fallback must therefore resolve the bitrate for
the codec actually selected rather than applying HEVC's lower automatic budget to H.264.

## Android decoding at 10 Mbps

| Scene | Codec | Median decode ms | p95 decode ms | Median ACK ms |
| --- | --- | ---: | ---: | ---: |
| Desktop, three-run range | H.264 | 4.55–5.03 | 5.96–7.16 | 8.53–9.59 |
| Desktop, three-run range | HEVC | 4.05–4.55 | 5.51–6.38 | 7.43–8.55 |
| Motion, one run | H.264 | 4.65 | 5.87 | 8.18 |
| Motion, one run | HEVC | 4.74 | 7.20 | 9.11 |

Both codecs sustain approximately 60 decoded FPS in steady-state STATS buckets, with no drops
after the first bucket. Startup drops range from 10 to 21 frames across the eight runs. Whole-run
send rates, including startup, range from approximately 59.25 to 59.70 FPS.

Decode time covers queueing a frame into MediaCodec through receiving its output. ACK time
adds USB and application handling; it excludes host capture/encoding because replay uses stored
frames. Neither is input-to-photon latency. This is one phone and one Mac, with synthetic scenes
and uncontrolled thermal/background load; it does not establish results for Windows encoders,
other devices, Wi-Fi, AV1, or VP9.

## App behavior and checks

The Mac resolves automatic bitrate for the final output dimensions and selected codec. If HEVC
creation fails and H.264 is supported, the H.264 encoder and CONFIG both receive the H.264
budget. At 1080p60, quality 80, the automatic budgets are 8,883 kbps for HEVC and 12,690 kbps for
H.264. An explicit user bitrate is preserved through fallback.

Five encoder selection tests cover automatic fallback, explicit bitrate, working HEVC,
unsupported H.264, and encoder failure. The automatic fallback test detects an incorrectly
retained 8,883 kbps budget. All 29 Swift tests pass.

Setting `PrioritizeEncodingSpeedOverQuality` to false produces byte-identical output in all four
10 Mbps scene/codec comparisons on this M5. The production speed-priority setting remains enabled.
Timing variation between those runs does not establish an improvement from toggling that property.
