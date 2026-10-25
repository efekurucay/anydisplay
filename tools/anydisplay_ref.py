#!/usr/bin/env python3
"""Reference implementation of the AnyDisplay protocol (docs/PROTOCOL.md).

Subcommands:
  vectors              print deterministic handshake/crypto test vectors (JSON)
  host  HOST[:PORT]    act as a host: handshake, then stream an ffmpeg test pattern
  client [--port P]    act as a client: listen, handshake, receive video to a file

Run with the venv in tools/.venv (needs `cryptography`).
"""
import argparse
import hashlib
import hmac
import json
import os
import socket
import struct
import subprocess
import sys
import threading
import time
import uuid

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

PORT = 47820
MAGIC = b"ANYD"
MAJOR, MINOR = 1, 0
MAX_FRAME = 32 * 1024 * 1024

CH_CONTROL, CH_VIDEO, CH_AUDIO, CH_CURSOR, CH_INPUT = range(5)

HELLO_C, HELLO_H, AUTH_H, AUTH_C, AUTH_FAIL, SAS_OK, PAIR, REVEAL = 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08
DEVICE_INFO, CONFIG_REQ, CONFIG, PING, PONG, STATS, KEYFRAME_REQ, NOTICE, CLIPBOARD, BYE = range(0x10, 0x1A)
VIDEO_FRAME, FRAME_ACK = 0x20, 0x21
CURSOR_POS, CURSOR_SHAPE = 0x30, 0x31
AUDIO_CONFIG, AUDIO_DATA = 0x40, 0x41
TOUCH, PEN, KEY, TEXT, SCROLL, MOUSE = range(0x50, 0x56)

CODEC_H264, CODEC_HEVC = 1, 2


# ---------------------------------------------------------------- encoding

class Writer:
    def __init__(self):
        self.b = bytearray()

    def u8(self, v): self.b += struct.pack("<B", v); return self
    def u16(self, v): self.b += struct.pack("<H", v); return self
    def i16(self, v): self.b += struct.pack("<h", v); return self
    def u32(self, v): self.b += struct.pack("<I", v); return self
    def u64(self, v): self.b += struct.pack("<Q", v); return self
    def f32(self, v): self.b += struct.pack("<f", v); return self
    def raw(self, v): self.b += v; return self

    def str(self, s):
        e = s.encode("utf-8")
        return self.u16(len(e)).raw(e)

    def blob(self, v):
        return self.u32(len(v)).raw(v)

    def bytes(self):
        return bytes(self.b)


class Reader:
    def __init__(self, b):
        self.b, self.o = b, 0

    def _take(self, fmt):
        v = struct.unpack_from(fmt, self.b, self.o)[0]
        self.o += struct.calcsize(fmt)
        return v

    def u8(self): return self._take("<B")
    def u16(self): return self._take("<H")
    def i16(self): return self._take("<h")
    def u32(self): return self._take("<I")
    def u64(self): return self._take("<Q")
    def f32(self): return self._take("<f")

    def raw(self, n):
        v = self.b[self.o:self.o + n]
        if len(v) != n:
            raise ValueError("short read")
        self.o += n
        return v

    def str(self): return self.raw(self.u16()).decode("utf-8")
    def blob(self): return self.raw(self.u32())
    def rest(self): v = self.b[self.o:]; self.o = len(self.b); return v


# ---------------------------------------------------------------- crypto

def gen_key(private_int=None):
    if private_int is None:
        priv = ec.generate_private_key(ec.SECP256R1())
    else:
        priv = ec.derive_private_key(private_int, ec.SECP256R1())
    pub = priv.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
    return priv, pub


def ecdh(priv, peer_pub):
    peer = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), peer_pub)
    return priv.exchange(ec.ECDH(), peer)


def sha256(*parts):
    h = hashlib.sha256()
    for p in parts:
        h.update(p)
    return h.digest()


def sas_code(transcript):
    return struct.unpack("<I", sha256(b"anydisplay/1 sas", transcript)[:4])[0] % 1_000_000


def key_schedule(z, psk, transcript):
    okm = HKDF(algorithm=hashes.SHA256(), length=96, salt=transcript, info=b"anydisplay/1 keys").derive(z + psk)
    return okm[0:32], okm[32:64], okm[64:96]


def confirm_mac(k_confirm, who, transcript):
    return hmac.new(k_confirm, who + transcript, hashlib.sha256).digest()


# ---------------------------------------------------------------- framing

class Conn:
    def __init__(self, sock, role):
        self.sock = sock
        self.role = role  # "host" or "client"
        self.send_key = self.recv_key = None
        self.send_seq = self.recv_seq = 0
        self.wlock = threading.Lock()
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def enable_crypto(self, k_h2c, k_c2h):
        if self.role == "host":
            self.send_key, self.recv_key = AESGCM(k_h2c), AESGCM(k_c2h)
        else:
            self.send_key, self.recv_key = AESGCM(k_c2h), AESGCM(k_h2c)

    def send(self, channel, mtype, body=b""):
        with self.wlock:
            if self.send_key is not None:
                nonce = struct.pack("<Q", self.send_seq) + b"\0\0\0\0"
                self.send_seq += 1
                body = self.send_key.encrypt(nonce, body, bytes([channel, mtype]))
            self.sock.sendall(struct.pack("<IBB", len(body) + 2, channel, mtype) + body)

    def _recv_exact(self, n):
        buf = bytearray()
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise EOFError("connection closed")
            buf += chunk
        return bytes(buf)

    def recv(self):
        length, channel, mtype = struct.unpack("<IBB", self._recv_exact(6))
        if length < 2 or length > MAX_FRAME:
            raise ValueError("bad frame length %d" % length)
        body = self._recv_exact(length - 2)
        if self.recv_key is not None:
            nonce = struct.pack("<Q", self.recv_seq) + b"\0\0\0\0"
            self.recv_seq += 1
            body = self.recv_key.decrypt(nonce, body, bytes([channel, mtype]))
        return channel, mtype, body

    def expect(self, mtype):
        channel, t, body = self.recv()
        if t == AUTH_FAIL:
            raise RuntimeError("peer sent AUTH_FAIL reason=%d" % body[0])
        if channel != CH_CONTROL or t != mtype:
            raise RuntimeError("expected type 0x%02x, got ch=%d type=0x%02x" % (mtype, channel, t))
        return body


# ---------------------------------------------------------------- persistent state

def state_path(role):
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), ".anydisplay_ref_%s.json" % role)


def load_state(role):
    try:
        with open(state_path(role)) as f:
            s = json.load(f)
    except (OSError, ValueError):
        s = {}
    if "id" not in s:
        s["id"] = os.urandom(16).hex()
        s["peers"] = {}
        save_state(role, s)
    return s


def save_state(role, s):
    with open(state_path(role), "w") as f:
        json.dump(s, f, indent=2)


# ---------------------------------------------------------------- handshake

def host_handshake(conn, state, host_name="anydisplay-ref", auto_confirm=False):
    hello_c = conn.expect(HELLO_C)
    r = Reader(hello_c)
    if r.raw(4) != MAGIC or r.u16() != MAJOR:
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([1]))
        raise RuntimeError("bad version")
    r.u16()
    device_id = r.raw(16).hex()
    device_name = r.str()
    link, auth_required = r.u8(), r.u8()
    client_pub, client_commit = r.raw(65), r.raw(32)

    secret = state["peers"].get(device_id, {}).get("secret")
    mode = 2 if secret else (0 if auth_required == 0 else 1)
    priv, pub = gen_key()
    host_nonce = os.urandom(32)
    hello_h = (Writer().raw(MAGIC).u16(MAJOR).u16(MINOR).raw(bytes.fromhex(state["id"]))
               .str(host_name).str("anydisplay-ref python").u8(mode).raw(pub).raw(host_nonce).bytes())
    conn.send(CH_CONTROL, HELLO_H, hello_h)

    channel, t, reveal = conn.recv()
    if t == AUTH_FAIL:
        if reveal[0] == 3:
            state["peers"].pop(device_id, None)
            save_state("host", state)
        raise RuntimeError("client rejected: reason %d" % reveal[0])
    if channel != CH_CONTROL or t != REVEAL or len(reveal) != 32:
        raise RuntimeError("expected REVEAL, got ch=%d type=0x%02x" % (channel, t))
    client_nonce = reveal
    if sha256(client_nonce) != client_commit:
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([7]))
        raise RuntimeError("bad commit")
    transcript = sha256(hello_c, hello_h, reveal)

    if mode == 1:
        code = sas_code(transcript)
        print("[host] pairing code: %06d" % code, flush=True)
        if not auto_confirm and input("[host] does the phone show the same code? [y/N] ").strip().lower() != "y":
            conn.send(CH_CONTROL, AUTH_FAIL, bytes([5]))
            raise RuntimeError("user rejected")
        conn.send(CH_CONTROL, SAS_OK)
        conn.expect(SAS_OK)

    psk = bytes.fromhex(secret) if mode == 2 else b""
    k_h2c, k_c2h, k_confirm = key_schedule(ecdh(priv, client_pub), psk, transcript)
    conn.send(CH_CONTROL, AUTH_H, confirm_mac(k_confirm, b"host", transcript))
    mac = conn.expect(AUTH_C)
    if not hmac.compare_digest(mac, confirm_mac(k_confirm, b"client", transcript)):
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([4]))
        raise RuntimeError("bad client mac")
    if mode != 0:
        conn.enable_crypto(k_h2c, k_c2h)
    if mode == 1:
        pair_secret = conn.expect(PAIR)
        state["peers"][device_id] = {"name": device_name, "secret": pair_secret.hex()}
        save_state("host", state)
    print("[host] handshake ok: device=%r link=%d mode=%d" % (device_name, link, mode), flush=True)
    return {"device_name": device_name, "mode": mode, "link": link}


def client_handshake(conn, state, link, device_name="anydisplay-ref-client", auto_confirm=False):
    priv, pub = gen_key()
    client_nonce = os.urandom(32)
    auth_required = 1 if link == 1 else 0
    hello_c = (Writer().raw(MAGIC).u16(MAJOR).u16(MINOR).raw(bytes.fromhex(state["id"])).str(device_name)
               .u8(link).u8(auth_required).raw(pub).raw(sha256(client_nonce)).bytes())
    conn.send(CH_CONTROL, HELLO_C, hello_c)
    hello_h = conn.expect(HELLO_H)
    r = Reader(hello_h)
    if r.raw(4) != MAGIC or r.u16() != MAJOR:
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([1]))
        raise RuntimeError("bad version")
    r.u16()
    host_id = r.raw(16).hex()
    host_name, host_os = r.str(), r.str()
    mode = r.u8()
    host_pub, host_nonce = r.raw(65), r.raw(32)
    if mode == 0 and auth_required:
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([2]))
        raise RuntimeError("host offered no auth on a network link")
    secret = state["peers"].get(host_id, {}).get("secret")
    if mode == 2 and not secret:
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([3]))
        raise RuntimeError("unknown host")
    reveal = client_nonce
    conn.send(CH_CONTROL, REVEAL, reveal)
    transcript = sha256(hello_c, hello_h, reveal)

    if mode == 1:
        code = sas_code(transcript)
        print("[client] pairing code: %06d" % code, flush=True)
        if not auto_confirm and input("[client] does the host show the same code? [y/N] ").strip().lower() != "y":
            conn.send(CH_CONTROL, AUTH_FAIL, bytes([5]))
            raise RuntimeError("user rejected")
        conn.send(CH_CONTROL, SAS_OK)
        conn.expect(SAS_OK)

    psk = bytes.fromhex(secret) if mode == 2 else b""
    k_h2c, k_c2h, k_confirm = key_schedule(ecdh(priv, host_pub), psk, transcript)
    mac = conn.expect(AUTH_H)
    if not hmac.compare_digest(mac, confirm_mac(k_confirm, b"host", transcript)):
        conn.send(CH_CONTROL, AUTH_FAIL, bytes([4]))
        raise RuntimeError("bad host mac")
    conn.send(CH_CONTROL, AUTH_C, confirm_mac(k_confirm, b"client", transcript))
    if mode != 0:
        conn.enable_crypto(k_h2c, k_c2h)
    if mode == 1:
        pair_secret = os.urandom(32)
        conn.send(CH_CONTROL, PAIR, pair_secret)
        state["peers"][host_id] = {"name": host_name, "secret": pair_secret.hex()}
        save_state("client", state)
    print("[client] handshake ok: host=%r os=%r mode=%d" % (host_name, host_os, mode), flush=True)
    return {"host_name": host_name, "mode": mode}


# ---------------------------------------------------------------- video source (host role)

def split_access_units(stream, codec):
    """Yield (access_unit_bytes, is_keyframe) from an Annex-B byte stream with AUDs."""
    buf = bytearray()
    aud_type = 9 if codec == CODEC_H264 else 35
    while True:
        chunk = stream.read(65536)
        if not chunk:
            break
        buf += chunk
        while True:
            starts = find_start_codes(buf)
            auds = [s for s in starts if nal_type(buf, s, codec) == aud_type]
            if len(auds) < 2:
                break
            au = bytes(buf[auds[0]:auds[1]])
            del buf[:auds[1]]
            yield au, is_key(au, codec)
    if buf:
        yield bytes(buf), is_key(bytes(buf), codec)


def find_start_codes(b):
    out, i = [], 0
    while True:
        i = b.find(b"\x00\x00\x01", i)
        if i < 0:
            return out
        out.append(i - 1 if i > 0 and b[i - 1] == 0 else i)
        i += 3


def nal_type(b, start, codec):
    p = start + (4 if b[start:start + 4] == b"\x00\x00\x00\x01" else 3)
    if p >= len(b):
        return -1
    return b[p] & 0x1F if codec == CODEC_H264 else (b[p] >> 1) & 0x3F


def is_key(au, codec):
    types = {nal_type(au, s, codec) for s in find_start_codes(au)}
    return bool(types & ({5} if codec == CODEC_H264 else {19, 20, 21}))


def ffmpeg_source(width, height, fps, codec, bitrate_kbps):
    enc = ["-c:v", "libx264", "-x264-params", "aud=1:keyint=120:bframes=0"] if codec == CODEC_H264 else \
          ["-c:v", "libx265", "-x265-params", "aud=1:keyint=120:bframes=0:log-level=error"]
    cmd = ["ffmpeg", "-loglevel", "error", "-re", "-f", "lavfi",
           "-i", "testsrc2=size=%dx%d:rate=%d" % (width, height, fps),
           "-pix_fmt", "yuv420p", "-tune", "zerolatency", "-preset", "ultrafast",
           "-b:v", "%dk" % bitrate_kbps] + enc + ["-f", "h264" if codec == CODEC_H264 else "hevc", "-"]
    return subprocess.Popen(cmd, stdout=subprocess.PIPE)


# ---------------------------------------------------------------- roles

def describe_input(t, body):
    r = Reader(body)
    if t == TOUCH:
        n = r.u8()
        pts = [(r.u8(), r.u8(), r.u16(), r.u16(), r.u16(), r.u16()) for _ in range(n)]
        return "TOUCH " + " ".join("id%d a%d (%d,%d) p%d s%d" % p for p in pts)
    if t == PEN:
        n = r.u8()
        s = [(r.u8(), r.u8(), r.u8(), r.u16(), r.u16(), r.u16(), r.i16(), r.i16(), r.u16(), r.u32()) for _ in range(n)]
        return "PEN x%d " % n + " ".join("a%d t%d b%d (%d,%d) p%d tilt(%d,%d) tw%d @%d" % x for x in s)
    if t == KEY:
        return "KEY action=%d usage=0x%02x mods=0x%x" % (r.u8(), r.u16(), r.u16())
    if t == TEXT:
        return "TEXT %r" % r.str()
    if t == SCROLL:
        return "SCROLL (%d,%d) d=(%d,%d) phase=%d" % (r.u16(), r.u16(), r.i16(), r.i16(), r.u8())
    if t == MOUSE:
        return "MOUSE action=%d button=%d (%d,%d)" % (r.u8(), r.u8(), r.u16(), r.u16())
    return "input type 0x%02x len %d" % (t, len(body))


def run_host(args):
    host, _, port = args.target.partition(":")
    sock = socket.create_connection((host, int(port or PORT)), timeout=15)
    sock.settimeout(None)
    conn = Conn(sock, "host")
    state = load_state("host")
    host_handshake(conn, state, auto_confirm=args.yes)

    info = Reader(conn.expect(DEVICE_INFO))
    sw, sh = info.u16(), info.u16()
    xdpi, ydpi, max_refresh = info.f32(), info.f32(), info.f32()
    codecs, mdw, mdh, touches, pen_caps, audio = info.u32(), info.u16(), info.u16(), info.u8(), info.u32(), info.u8()
    model, osv = info.str(), info.str()
    print("[host] DEVICE_INFO %dx%d dpi=%.0f/%.0f refresh=%.0f codecs=0x%x max=%dx%d touches=%d pen=0x%x audio=%d %s %s"
          % (sw, sh, xdpi, ydpi, max_refresh, codecs, mdw, mdh, touches, pen_caps, audio, model, osv), flush=True)
    req = Reader(conn.expect(CONFIG_REQ))
    w, h, hz, codec, kbps, quality, flags = req.u16(), req.u16(), req.u16(), req.u8(), req.u32(), req.u8(), req.u8()
    print("[host] CONFIG_REQ %dx%d@%d codec=%d kbps=%d q=%d flags=0x%x" % (w, h, hz, codec, kbps, quality, flags), flush=True)
    w, h = (w or sw) & ~1, (h or sh) & ~1
    if args.scale != 1.0:
        w, h = int(w * args.scale) & ~1, int(h * args.scale) & ~1
    hz = min(hz or 60, 60)
    codec = codec if codec in (CODEC_H264, CODEC_HEVC) else CODEC_H264
    kbps = kbps or 8000
    stream_id = 1
    conn.send(CH_CONTROL, CONFIG, Writer().u32(stream_id).u16(w).u16(h).u16(hz).u8(codec).u32(kbps)
              .u8(flags & 0).u16(w).u16(h).u32(0).bytes())

    acked = {"last": -1}
    stop = threading.Event()
    cond = threading.Condition()

    def reader():
        try:
            while not stop.is_set():
                ch, t, body = conn.recv()
                if t == PING:
                    seq, ts = struct.unpack_from("<IQ", body)
                    conn.send(CH_CONTROL, PONG, struct.pack("<IQ", seq, ts))
                elif t == FRAME_ACK:
                    _, fid, dec = struct.unpack_from("<III", body)
                    with cond:
                        acked["last"] = max(acked["last"], fid)
                        cond.notify_all()
                elif t == STATS:
                    print("[host] STATS decoded=%d dropped=%d avg_decode=%dus rtt=%dus bytes=%d"
                          % struct.unpack_from("<IIIII", body), flush=True)
                elif ch == CH_INPUT:
                    print("[host] " + describe_input(t, body), flush=True)
                elif t == BYE:
                    print("[host] BYE", flush=True)
                    stop.set()
                elif t not in (PONG,):
                    print("[host] ch=%d type=0x%02x len=%d" % (ch, t, len(body)), flush=True)
        except Exception as e:  # noqa: BLE001 - reference tool reports and exits
            print("[host] reader stopped: %s" % e, flush=True)
            stop.set()

    threading.Thread(target=reader, daemon=True).start()

    def pinger():
        seq = 0
        while not stop.is_set():
            conn.send(CH_CONTROL, PING, struct.pack("<IQ", seq, int(time.time() * 1e6)))
            seq += 1
            time.sleep(1)

    threading.Thread(target=pinger, daemon=True).start()

    proc = ffmpeg_source(w, h, hz, codec, kbps)
    sent = 0
    end = time.time() + args.seconds
    for fid, (au, key) in enumerate(split_access_units(proc.stdout, codec)):
        if stop.is_set() or time.time() > end:
            break
        with cond:
            while fid - acked["last"] > 3 and not stop.is_set():
                if not cond.wait(timeout=2):
                    print("[host] no ack for 2 s (last acked %d)" % acked["last"], flush=True)
                    break
        conn.send(CH_VIDEO, VIDEO_FRAME, Writer().u32(stream_id).u32(fid).u64(int(time.time() * 1e6))
                  .u8(1 if key else 0).raw(au).bytes())
        sent += 1
    proc.kill()
    print("[host] sent %d frames, last acked %d" % (sent, acked["last"]), flush=True)
    try:
        conn.send(CH_CONTROL, BYE, bytes([0]))
    except OSError:
        pass
    stop.set()
    sock.close()


def run_client(args):
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", args.port))
    srv.listen(1)
    print("[client] listening on %d" % args.port, flush=True)
    sock, addr = srv.accept()
    srv.close()
    link = 0 if addr[0].startswith("127.") or addr[0] == "::1" else 1
    conn = Conn(sock, "client")
    state = load_state("client")
    client_handshake(conn, state, link, auto_confirm=args.yes)
    conn.send(CH_CONTROL, DEVICE_INFO, Writer().u16(args.width).u16(args.height).f32(400).f32(400).f32(60)
              .u32(0b11).u16(4096).u16(4096).u8(10).u32(0b11111).u8(1).str("anydisplay-ref").str("python").bytes())
    conn.send(CH_CONTROL, CONFIG_REQ, Writer().u16(0).u16(0).u16(60).u8(args.codec).u32(0).u8(80).u8(0).bytes())
    out = open(args.out, "wb") if args.out else None
    frames = 0
    t0 = time.time()
    try:
        while True:
            ch, t, body = conn.recv()
            if t == CONFIG:
                r = Reader(body)
                print("[client] CONFIG stream=%d %dx%d@%d codec=%d kbps=%d flags=0x%x logical=%dx%d caps=0x%x"
                      % (r.u32(), r.u16(), r.u16(), r.u16(), r.u8(), r.u32(), r.u8(), r.u16(), r.u16(), r.u32()), flush=True)
            elif t == VIDEO_FRAME:
                r = Reader(body)
                sid, fid, pts, flags = r.u32(), r.u32(), r.u64(), r.u8()
                data = r.rest()
                if out:
                    out.write(data)
                frames += 1
                conn.send(CH_VIDEO, FRAME_ACK, struct.pack("<III", sid, fid, 0))
                if frames % 60 == 1:
                    print("[client] frame %d key=%d %d bytes" % (fid, flags & 1, len(data)), flush=True)
            elif t == PING:
                conn.send(CH_CONTROL, PONG, body[:12])
            elif t == BYE:
                print("[client] BYE", flush=True)
                break
            elif t == NOTICE:
                r = Reader(body)
                print("[client] NOTICE %d %s" % (r.u16(), r.str()), flush=True)
            elif t not in (PONG,):
                print("[client] ch=%d type=0x%02x len=%d" % (ch, t, len(body)), flush=True)
    except EOFError:
        pass
    dt = time.time() - t0
    print("[client] received %d frames in %.1fs (%.1f fps)" % (frames, dt, frames / max(dt, 1e-6)), flush=True)


def run_vectors(_args):
    """Deterministic values so other implementations can unit-test their crypto."""
    c_priv, c_pub = gen_key(0x1111111111111111111111111111111111111111111111111111111111111111)
    h_priv, h_pub = gen_key(0x2222222222222222222222222222222222222222222222222222222222222222)
    client_nonce = bytes(range(32))
    host_nonce = bytes(range(32, 64))
    device_id = bytes(range(0xA0, 0xB0))
    host_id = bytes(range(0xB0, 0xC0))
    hello_c = (Writer().raw(MAGIC).u16(1).u16(0).raw(device_id).str("Phone").u8(1).u8(1)
               .raw(c_pub).raw(sha256(client_nonce)).bytes())
    hello_h = (Writer().raw(MAGIC).u16(1).u16(0).raw(host_id).str("Mac").str("macOS").u8(1)
               .raw(h_pub).raw(host_nonce).bytes())
    reveal = client_nonce
    transcript = sha256(hello_c, hello_h, reveal)
    z = ecdh(c_priv, h_pub)
    assert z == ecdh(h_priv, c_pub)
    out = {}
    for name, psk in (("mode1", b""), ("mode2", bytes([0x5A]) * 32)):
        k_h2c, k_c2h, k_confirm = key_schedule(z, psk, transcript)
        nonce0 = struct.pack("<Q", 0) + b"\0\0\0\0"
        out[name] = {
            "psk": psk.hex(),
            "k_h2c": k_h2c.hex(), "k_c2h": k_c2h.hex(), "k_confirm": k_confirm.hex(),
            "auth_h": confirm_mac(k_confirm, b"host", transcript).hex(),
            "auth_c": confirm_mac(k_confirm, b"client", transcript).hex(),
            "encrypted_ping_h2c_seq0": AESGCM(k_h2c).encrypt(nonce0, struct.pack("<IQ", 7, 123456789),
                                                             bytes([CH_CONTROL, PING])).hex(),
        }
    print(json.dumps({
        "client_private": "11" * 32, "host_private": "22" * 32,
        "client_pub": c_pub.hex(), "host_pub": h_pub.hex(),
        "client_nonce": client_nonce.hex(), "host_nonce": host_nonce.hex(),
        "hello_c_body": hello_c.hex(), "hello_h_body": hello_h.hex(), "reveal_body": reveal.hex(),
        "transcript": transcript.hex(), "ecdh_z": z.hex(), "sas": "%06d" % sas_code(transcript),
        "ping_plaintext_body": struct.pack("<IQ", 7, 123456789).hex(),
        **out,
    }, indent=2))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("vectors").set_defaults(fn=run_vectors)
    h = sub.add_parser("host")
    h.add_argument("target", nargs="?", default="127.0.0.1:%d" % PORT)
    h.add_argument("--seconds", type=float, default=10)
    h.add_argument("--scale", type=float, default=1.0, help="scale the requested stream size")
    h.add_argument("-y", "--yes", action="store_true", help="auto-confirm the pairing code")
    h.set_defaults(fn=run_host)
    c = sub.add_parser("client")
    c.add_argument("--port", type=int, default=PORT)
    c.add_argument("--width", type=int, default=1920)
    c.add_argument("--height", type=int, default=1080)
    c.add_argument("--codec", type=int, default=CODEC_H264)
    c.add_argument("--out", help="write received Annex-B stream here")
    c.add_argument("-y", "--yes", action="store_true", help="auto-confirm the pairing code")
    c.set_defaults(fn=run_client)
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
