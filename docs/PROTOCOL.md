# AnyDisplay Protocol, version 1

AnyDisplay turns an Android device into an extra display and pen tablet for a desktop
computer. Two roles exist:

- **Client**: the Android device. It listens for TCP connections, decodes video,
  renders it, and sends input.
- **Host**: the desktop computer (macOS or Windows). It connects to the client,
  creates or captures a display, encodes video, and injects input.

Every implementation (Android, macOS, Windows, `tools/anydisplay_ref.py`) follows this
document byte for byte. When code and this document disagree, the document wins
and the code is a bug.

## 1. Encoding rules

- All integers are **little-endian**.
- `u8/u16/u32/u64`, `i16/i32/i64` are fixed-size integers. `f32` is IEEE-754.
- `str` is `u16 byteLength` followed by UTF-8 bytes (no terminator).
- `blob` is `u32 byteLength` followed by bytes.
- `bytes[N]` is exactly N raw bytes.
- "rest" means all remaining bytes of the frame body.

## 2. Transport

- TCP. The client listens on port **47820** on all interfaces.
- USB: the host runs `adb forward tcp:<local> tcp:47820` and connects to
  `127.0.0.1:<local>`. The client sees a loopback peer.
- Wi-Fi: the client advertises mDNS/DNS-SD service `_anydisplay._tcp.` with its device
  name as instance name and TXT records `id=<32 hex chars device_id>` and `v=1`.
  Hosts may also connect to a manually entered IP.
- `TCP_NODELAY` on both sides.
- One host session per client at a time. A second connection while a session is
  active is answered with `AUTH_FAIL(reason=busy)` and closed.

## 3. Framing

Every message is one frame:

```
frame := u32 length | u8 channel | u8 type | body
```

`length` counts `channel + type + body` (so `length >= 2`). Maximum `length` is
33 554 432 (32 MiB). A receiver closes the connection on a larger value.

Channels:

| id | name    | carries                         |
|----|---------|---------------------------------|
| 0  | control | handshake, config, ping, misc   |
| 1  | video   | encoded frames, acks            |
| 2  | audio   | audio config and samples        |
| 3  | cursor  | cursor position and shape       |
| 4  | input   | touch, pen, keys, mouse, scroll |

A sender keeps one queue per channel and drains `control`, `input`, `cursor`,
`audio` before `video`, so a large video frame never delays small messages more
than one frame write.

### 3.1 Encryption

After the handshake (section 4) the session is either **plain** (auth mode 0) or
**encrypted** (auth modes 1 and 2).

In encrypted mode every frame body is replaced by
`AES-256-GCM(key_dir, nonce, plaintext=body, aad=channel||type)` with the 16-byte
tag appended. `length` covers `2 + len(ciphertext) + 16`.

- `nonce` (12 bytes) = `u64 seq` little-endian followed by four zero bytes.
- `seq` starts at 0 for the first encrypted frame in each direction and
  increments by one per frame. Each direction has its own key and counter.
- `key_dir` is `k_h2c` for host→client frames and `k_c2h` for client→host frames.
- A frame that fails authentication closes the connection.

## 4. Handshake

All handshake frames use channel 0 and are never encrypted.

### 4.1 Messages

| type | name       | dir   | body |
|------|------------|-------|------|
| 0x01 | HELLO_C    | C→H   | `bytes[4] magic="ANYD"`, `u16 major=1`, `u16 minor=0`, `bytes[16] device_id`, `str device_name`, `u8 link`, `u8 auth_required`, `bytes[65] client_pub`, `bytes[32] client_commit` |
| 0x02 | HELLO_H    | H→C   | `bytes[4] magic="ANYD"`, `u16 major=1`, `u16 minor=0`, `bytes[16] host_id`, `str host_name`, `str host_os`, `u8 auth_mode`, `bytes[65] host_pub`, `bytes[32] host_nonce` |
| 0x08 | REVEAL     | C→H   | `bytes[32] client_nonce` |
| 0x06 | SAS_OK     | both  | empty |
| 0x03 | AUTH_H     | H→C   | `bytes[32] mac` |
| 0x04 | AUTH_C     | C→H   | `bytes[32] mac` |
| 0x05 | AUTH_FAIL  | both  | `u8 reason` (1 bad_version, 2 auth_required, 3 unknown_host, 4 bad_mac, 5 user_rejected, 6 busy, 7 bad_commit) |
| 0x07 | PAIR       | C→H   | `bytes[32] pair_secret` (sent encrypted, see 4.4) |

- `link`: 0 = loopback (adb forward), 1 = network, 2 = USB accessory.
- `auth_required` = 1 when `link == 1`, else 0.
- `client_pub`, `host_pub`: ephemeral P-256 public keys, uncompressed X9.62
  (`0x04 || X || Y`), freshly generated per connection.
- `client_nonce`, `host_nonce`: 32 random bytes per connection.
- `client_commit` = `SHA-256(client_nonce)`.
- `device_id`, `host_id`: random 16 bytes generated once per installation and
  persisted.
- `auth_mode`: 0 = none, 1 = SAS pairing, 2 = paired secret.

### 4.2 Sequence

1. Host connects. Client sends `HELLO_C`.
2. Host picks `auth_mode`:
   - 2 if it has a stored `pair_secret` for `device_id`,
   - else 0 if `auth_required == 0`,
   - else 1.

   A stored pairing wins even on a loopback link, so a paired device is always
   encrypted. Host sends `HELLO_H`.
3. Client validates:
   - magic or major mismatch → `AUTH_FAIL(bad_version)`;
   - `auth_mode == 0` while `auth_required == 1` → `AUTH_FAIL(auth_required)`;
   - `auth_mode == 2` and no stored secret for `host_id` → `AUTH_FAIL(unknown_host)`.
     The host deletes its stored secret for this device and reconnects with
     mode 1.
4. Client sends `REVEAL` (a frame of any other type is a protocol error). Host checks `SHA-256(client_nonce) == client_commit`,
   else `AUTH_FAIL(bad_commit)`.
5. Both compute `transcript = SHA-256(body(HELLO_C) || body(HELLO_H) || body(REVEAL))`
   where `body(x)` is the frame body without length/channel/type.
6. Mode 1 only (numeric comparison):
   - `sas = u32_le(SHA-256("anydisplay/1 sas" || transcript)[0..4]) mod 1 000 000`,
     shown as 6 digits with leading zeros on **both** screens.
   - Each user confirms the numbers match. On confirmation a side sends `SAS_OK`;
     on rejection it sends `AUTH_FAIL(user_rejected)`.
   - Both proceed only after sending and receiving `SAS_OK`.
   - The commitment in `HELLO_C` stops a man in the middle from picking keys to
     force a matching code.
7. Key schedule:
   - `Z` = ECDH(own private key, peer public key), the 32-byte X coordinate.
   - `psk` = empty for modes 0 and 1, the 32-byte `pair_secret` for mode 2.
   - `okm = HKDF-SHA256(salt=transcript, ikm=Z || psk, info="anydisplay/1 keys", L=96)`.
   - `k_h2c = okm[0..32]`, `k_c2h = okm[32..64]`, `k_confirm = okm[64..96]`.
8. Host sends `AUTH_H` with `mac = HMAC-SHA256(k_confirm, "host" || transcript)`.
   Client verifies (constant time), else `AUTH_FAIL(bad_mac)`.
9. Client sends `AUTH_C` with `mac = HMAC-SHA256(k_confirm, "client" || transcript)`.
   Host verifies, else `AUTH_FAIL(bad_mac)`.
10. Encryption switches on (modes 1 and 2) for every frame after `AUTH_C` in both
    directions.
11. Mode 1 only: client generates 32 random bytes, sends `PAIR` as the first
    encrypted frame, and both sides persist it (client keyed by `host_id` with
    `host_name`, host keyed by `device_id` with `device_name`).

ASCII strings in the formulas (`"anydisplay/1 sas"`, `"host"`, ...) are their raw
ASCII bytes without length prefix.

### 4.3 Timeouts

Each handshake step times out after 10 s, except waiting for the user in step 6
(120 s).

## 5. Session messages

After the handshake the client sends `DEVICE_INFO` and then `CONFIG_REQ`. The
host answers with `CONFIG` and starts streaming. The client may send a new
`CONFIG_REQ` at any time (for example after rotation); the host answers with a
new `CONFIG` with a new `stream_id` and starts the new stream with a keyframe.

### 5.1 Control channel (0)

| type | name        | dir  | body |
|------|-------------|------|------|
| 0x10 | DEVICE_INFO | C→H  | `u16 screen_w`, `u16 screen_h` (current orientation, physical px), `f32 xdpi`, `f32 ydpi`, `f32 max_refresh`, `u32 codecs` (bit0 H.264, bit1 HEVC, bit2 AV1), `u16 max_decode_w`, `u16 max_decode_h`, `u8 max_touches`, `u32 pen_caps` (bit0 pressure, bit1 tilt, bit2 hover, bit3 eraser, bit4 barrel button), `u8 audio` (bit0 PCM s16le), `str model`, `str os_version` |
| 0x11 | CONFIG_REQ  | C→H  | `u16 width`, `u16 height` (stream size; 0,0 = screen size), `u16 refresh_hz`, `u8 codec` (1 H.264, 2 HEVC, 3 AV1), `u32 bitrate_kbps` (0 = auto), `u8 quality` (1–100), `u8 flags` (bit0 hidpi, bit1 audio, bit2 client draws cursor, bit3 mirror main display instead of extending) |
| 0x12 | CONFIG      | H→C  | `u32 stream_id`, `u16 width`, `u16 height`, `u16 refresh_hz`, `u8 codec`, `u32 bitrate_kbps`, `u8 flags` (same bits as request, as granted), `u16 logical_w`, `u16 logical_h` (desktop size in points on macOS, 96-DPI units on Windows), `u32 host_caps` (bit0 native touch, bit1 native pen, bit2 clipboard, bit3 audio) |
| 0x13 | PING        | both | `u32 seq`, `u64 sender_time_us` |
| 0x14 | PONG        | both | `u32 seq`, `u64 echoed sender_time_us` |
| 0x15 | STATS       | C→H  | `u32 frames_decoded`, `u32 frames_dropped`, `u32 avg_decode_us`, `u32 rtt_us`, `u32 bytes_received` (since previous STATS, sent every second) |
| 0x16 | KEYFRAME_REQ| C→H  | empty |
| 0x17 | NOTICE      | H→C  | `u16 code`, `str message` (1 software encoder, 2 screen capture permission missing, 3 input permission missing, 4 resolution unsupported, 5 virtual display unavailable, mirroring instead) |
| 0x18 | CLIPBOARD   | both | `str mime` (`text/plain` in v1), `blob data` |
| 0x19 | BYE         | both | `u8 reason` (0 user, 1 shutdown, 2 error) |

Keepalive: each side sends `PING` every 1 s and answers every `PING` with `PONG`.
A side that receives no frame of any kind for 5 s sends `BYE(error)` and closes
the connection; `PONG` and all other traffic count as liveness.

`CLIPBOARD` data is limited to 16 MiB; larger clipboard contents are not sent.

### 5.2 Video channel (1)

| type | name        | dir | body |
|------|-------------|-----|------|
| 0x20 | VIDEO_FRAME | H→C | `u32 stream_id`, `u32 frame_id`, `u64 pts_us`, `u8 flags` (bit0 keyframe), rest = Annex-B access unit |
| 0x21 | FRAME_ACK   | C→H | `u32 stream_id`, `u32 frame_id`, `u32 decode_us` |

- Keyframes carry parameter sets in-band (SPS/PPS, plus VPS for HEVC).
- The first frame of every `stream_id` is a keyframe. `frame_id` starts at 0 for
  every stream. `pts_us` is host wall-clock time in microseconds since the Unix
  epoch (the same clock for audio).
- Flow control: the host keeps at most **3** frames unacknowledged and skips
  capturing/encoding new frames while at the limit (it never drops already
  encoded frames, so the decoder stays consistent).
- The client acknowledges each frame after `releaseOutputBuffer` (or when it
  drops it). `frame_id` increments by one per frame within a stream.
- `FRAME_ACK` is cumulative: acknowledging frame N acknowledges every earlier
  frame of that stream.

### 5.3 Audio channel (2)

| type | name         | dir | body |
|------|--------------|-----|------|
| 0x40 | AUDIO_CONFIG | H→C | `u32 sample_rate`, `u8 channels`, `u8 codec` (0 = PCM s16le interleaved) |
| 0x41 | AUDIO_DATA   | H→C | `u64 pts_us`, rest = samples |

### 5.4 Cursor channel (3)

Only used when `CONFIG.flags` bit2 is set; otherwise the cursor is part of the
video. Shapes are sent at stream scale (the size the cursor has in the video).

| type | name         | dir | body |
|------|--------------|-----|------|
| 0x30 | CURSOR_POS   | H→C | `i16 x`, `i16 y` (stream pixels, hotspot position), `u8 visible` |
| 0x31 | CURSOR_SHAPE | H→C | `u16 w`, `u16 h`, `u16 hot_x`, `u16 hot_y`, rest = `w*h*4` bytes RGBA8 (straight alpha) |

### 5.5 Input channel (4)

Position coordinates (`x`, `y` in TOUCH, PEN, SCROLL, MOUSE) are `u16`
normalized over the video image: `0` = left/top edge, `65535` = right/bottom
edge. SCROLL `dx`/`dy` are deltas in stream pixels. Pressure is `u16` 0–65535. Angles are hundredths of
a degree.

| type | name   | dir | body |
|------|--------|-----|------|
| 0x50 | TOUCH  | C→H | `u8 count`, then per active contact: `u8 id`, `u8 action` (0 down, 1 move, 2 up, 3 cancel), `u16 x`, `u16 y`, `u16 pressure`, `u16 size` |
| 0x51 | PEN    | C→H | `u8 count`, then per sample: `u8 action` (0 down, 1 move, 2 up, 3 hover_enter, 4 hover_move, 5 hover_exit, 6 cancel), `u8 tool` (0 pen, 1 eraser), `u8 buttons` (bit0 barrel, bit1 secondary), `u16 x`, `u16 y`, `u16 pressure`, `i16 tilt_x`, `i16 tilt_y`, `u16 twist`, `u32 time_ms` |
| 0x52 | KEY    | C→H | `u8 action` (0 up, 1 down), `u16 hid_usage` (USB HID keyboard page), `u16 modifiers` (bit0 ctrl, bit1 shift, bit2 alt, bit3 meta) |
| 0x53 | TEXT   | C→H | `str text` (committed IME text, host types it as Unicode) |
| 0x54 | SCROLL | C→H | `u16 x`, `u16 y`, `i16 dx`, `i16 dy` (stream pixels, positive = content moves right/down), `u8 phase` (0 begin, 1 change, 2 end, 3 momentum) |
| 0x55 | MOUSE  | C→H | `u8 action` (0 move, 1 down, 2 up), `u8 button` (0 left, 1 right, 2 middle), `u16 x`, `u16 y` |

- `KEY.modifiers` is the set of modifiers the user holds for this key. The host
  presses any of them that are not already down before the key and releases
  them after it.
- `TOUCH` always lists every contact that is currently down plus the one whose
  state changed (with action up or cancel), so the host can build a complete
  contact frame.
- `PEN` batches the historical samples of one Android `MotionEvent` (oldest
  first), so fast strokes keep every sample.
- `tilt_x`/`tilt_y` follow the Windows convention: tilt toward +X/+Y in
  hundredths of a degree, range −9000..9000. The client derives them from
  Android `AXIS_TILT` and `AXIS_ORIENTATION`.

## 6. Versioning

`major` changes break compatibility. `minor` additions only append fields at the
end of bodies or add new message types; receivers ignore unknown message types
and trailing bytes.
