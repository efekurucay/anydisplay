import argparse
import importlib.util
import json
from pathlib import Path
import socket
import struct
import threading
import time

root = Path(__file__).parent
spec = importlib.util.spec_from_file_location('ref', str(Path(__file__).resolve().parents[1] / 'anydisplay_ref.py'))
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
p = argparse.ArgumentParser()
p.add_argument('stream')
p.add_argument('--seconds', type=int, default=15)
p.add_argument('--port', type=int, default=47820)
args = p.parse_args()
root = Path(args.stream).resolve().parent
r.state_path = lambda role: str(root / ('pairing-' + role + '.json'))
meta = json.loads(Path(args.stream + '.json').read_text())
data = Path(args.stream).read_bytes()
frames, off = [], 0
for size, key in zip(meta['frame_sizes'], meta['keyframes']):
    frames.append((data[off:off+size], key))
    off += size
sock = socket.create_connection(('127.0.0.1',args.port), timeout=30)
sock.settimeout(None)
conn = r.Conn(sock, 'host')
r.host_handshake(conn, r.load_state('host'), host_name='AnyDisplay codec benchmark', auto_confirm=True)
info = r.Reader(conn.expect(r.DEVICE_INFO))
sw, sh = info.u16(), info.u16()
xdpi, ydpi, refresh = info.f32(), info.f32(), info.f32()
bits, mw, mh = info.u32(), info.u16(), info.u16()
touches, pen, audio = info.u8(), info.u32(), info.u8()
model, os_version = info.str(), info.str()
conn.expect(r.CONFIG_REQ)
codec = r.CODEC_HEVC if meta['codec'] == 'hevc' else r.CODEC_H264
assert bits & (1 << (codec-1)), 'client does not support codec'
conn.send(r.CH_CONTROL, r.CONFIG, r.Writer().u32(1).u16(meta['width']).u16(meta['height']).u16(meta['fps'])
    .u8(codec).u32(meta['target_kbps']).u8(0).u16(meta['width']).u16(meta['height']).u32(0).bytes())
stop = threading.Event()
cond = threading.Condition()
state = {'acked':-1, 'key':False}
submitted, acks, stats = {}, [], []

def reader():
    try:
        while not stop.is_set():
            ch, typ, body = conn.recv()
            if typ == r.PING:
                conn.send(r.CH_CONTROL,r.PONG,body)
            elif typ == r.FRAME_ACK:
                sid, fid, decode = struct.unpack_from('<III',body)
                now = time.monotonic()
                with cond:
                    if fid in submitted:
                        acks.append({'frame':fid,'decode_ms':decode/1000,'ack_ms':(now-submitted.pop(fid))*1000})
                    state['acked'] = max(state['acked'],fid)
                    cond.notify_all()
            elif typ == r.STATS:
                dec, drop, avg, rtt, received = struct.unpack_from('<IIIII',body)
                stats.append({'decoded':dec,'dropped':drop,'decode_ms':avg/1000,'rtt_ms':rtt/1000,'bytes':received})
                print('STATS',stats[-1],flush=True)
            elif typ == r.KEYFRAME_REQ:
                state['key'] = True
            elif typ == r.BYE:
                stop.set()
    except (OSError,EOFError,RuntimeError) as exc:
        if not stop.is_set(): print('reader:',repr(exc),flush=True)
        stop.set()

threading.Thread(target=reader,daemon=True).start()
start = time.monotonic()
deadline = start
fid, index = 0, 0
while time.monotonic()-start < args.seconds and not stop.is_set():
    with cond:
        while fid-state['acked'] > 3 and not stop.is_set():
            cond.wait(timeout=1)
            if time.monotonic()-start > args.seconds: break
    if stop.is_set(): break
    delay = deadline-time.monotonic()
    if delay > 0: time.sleep(delay)
    if state['key']:
        index = 0
        state['key'] = False
    au,key = frames[index]
    submitted[fid] = time.monotonic()
    conn.send(r.CH_VIDEO,r.VIDEO_FRAME,r.Writer().u32(1).u32(fid).u64(int(time.time()*1e6)).u8(int(key)).raw(au).bytes())
    fid += 1
    index = (index+1) % len(frames)
    deadline = max(deadline+1/meta['fps'],time.monotonic())
elapsed = time.monotonic()-start
time.sleep(0.25)
stop.set()
conn.send(r.CH_CONTROL,r.BYE,bytes([0]))
sock.close()
warm = [a for a in acks if a['frame'] >= 60 and a['decode_ms'] > 0]
def percentile(key,q):
    values = sorted(a[key] for a in warm)
    return values[int((len(values)-1)*q)] if values else None
result = {'model':model,'os':os_version,'stream':args.stream,'codec':meta['codec'],'target_kbps':meta['target_kbps'],
    'sent':fid,'acked':len(acks),'elapsed_s':elapsed,'sent_fps':fid/elapsed,'stats':stats,'acks':acks,
    'decode_median_ms':percentile('decode_ms',0.5),'decode_p95_ms':percentile('decode_ms',0.95),
    'ack_median_ms':percentile('ack_ms',0.5),'ack_p95_ms':percentile('ack_ms',0.95)}
Path(args.stream+'.android.json').write_text(json.dumps(result,indent=2))
print('RESULT',json.dumps({k:v for k,v in result.items() if k not in ('stats','acks')}),flush=True)
