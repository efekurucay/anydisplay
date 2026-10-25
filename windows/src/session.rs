//! One host session (PROTOCOL.md 4-5): connect, handshake, stream configuration, flow control,
//! keepalive, input, clipboard, audio, shutdown.
//!
//! Threads: the session loop (caller's thread) owns the platform and all decisions; a reader
//! thread turns socket bytes into frames; a writer thread drains the per-channel queues in
//! priority order and encrypts; one video thread per stream pulls encoded frames.

use crate::negotiate;
use crate::pairing::PairingStore;
use crate::platform::{
    AudioStream, FrameSink, OpenedStream, Platform, StreamRequest, VideoPipeline,
};
use crate::proto::frame::{encode_frame, FrameDecoder, FrameError};
use crate::proto::handshake::{
    Action, AuthMode, HandshakeError, HandshakeResult, HostHandshake, HostIdentity,
};
use crate::proto::messages::*;
use crate::proto::{Cipher, Frame};
use crate::util::{now_us, to_hex};
use anyhow::{anyhow, bail, Context};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// PROTOCOL.md 5.2.
pub const MAX_UNACKED: u32 = 3;
const PING_INTERVAL: Duration = Duration::from_secs(1);
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(5);
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(20);
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);
const PAIRING_RETRIES: u32 = 5;
/// Audio frames waiting beyond this are dropped oldest first (stale audio is worse than a gap).
const AUDIO_QUEUE_LIMIT: usize = 50;

pub type SasPrompt = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct SessionConfig {
    pub host_name: String,
    /// Accept the pairing code without asking.
    pub auto_confirm: bool,
    /// Host codec preference (`--codec`), used when the client can decode it.
    pub codec: Option<Codec>,
    pub clipboard: bool,
    pub audio: bool,
    /// End the session this long after the first CONFIG.
    pub duration: Option<Duration>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            host_name: "AnyDisplay host".into(),
            auto_confirm: false,
            codec: None,
            clipboard: true,
            audio: true,
            duration: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    pub device_name: String,
    pub mode: Option<AuthMode>,
    pub streams: u32,
    pub frames_sent: u64,
    pub last_config: Option<Config>,
    pub notices: Vec<(u16, String)>,
    /// Set when the session ended because of an error.
    pub error: Option<String>,
}

enum Event {
    Frame(Frame),
    Closed(String),
    Stop(u8),
    VideoFailed(u32, String),
}

/// Ends a running session from another thread (Ctrl-C, tray menu).
#[derive(Clone, Default)]
pub struct SessionHandle {
    inner: Arc<Mutex<HandleState>>,
}

#[derive(Default)]
struct HandleState {
    stop: Option<u8>,
    tx: Option<Sender<Event>>,
}

impl SessionHandle {
    pub fn stop(&self, reason: u8) {
        let mut s = self.inner.lock().unwrap();
        s.stop = Some(reason);
        if let Some(tx) = &s.tx {
            let _ = tx.send(Event::Stop(reason));
        }
    }

    fn stop_requested(&self) -> Option<u8> {
        self.inner.lock().unwrap().stop
    }

    fn attach(&self, tx: Sender<Event>) {
        let mut s = self.inner.lock().unwrap();
        if let Some(r) = s.stop {
            let _ = tx.send(Event::Stop(r));
        }
        s.tx = Some(tx);
    }
}

// ---------------------------------------------------------------- outgoing queues (PROTOCOL.md 3)

/// Per-channel queues drained control, input, cursor, audio, then video.
pub struct Outbox {
    state: Mutex<OutState>,
    cv: Condvar,
}

struct OutState {
    queues: [VecDeque<Frame>; 5],
    /// `Some(drain)` once closing; with `drain` the queued frames are still written.
    closing: Option<bool>,
}

fn priority(channel: u8) -> usize {
    match channel {
        channel::CONTROL => 0,
        channel::INPUT => 1,
        channel::CURSOR => 2,
        channel::AUDIO => 3,
        channel::VIDEO => 4,
        _ => 0,
    }
}

impl Outbox {
    pub fn new() -> Arc<Outbox> {
        Arc::new(Outbox {
            state: Mutex::new(OutState {
                queues: Default::default(),
                closing: None,
            }),
            cv: Condvar::new(),
        })
    }

    pub fn send(&self, f: Frame) {
        let mut s = self.state.lock().unwrap();
        if s.closing.is_some() {
            return;
        }
        let q = &mut s.queues[priority(f.channel)];
        q.push_back(f);
        if priority(channel::AUDIO) == priority(q[0].channel) && q.len() > AUDIO_QUEUE_LIMIT {
            q.pop_front();
        }
        self.cv.notify_one();
    }

    pub fn close(&self, drain: bool) {
        let mut s = self.state.lock().unwrap();
        if s.closing != Some(false) {
            s.closing = Some(drain);
        }
        self.cv.notify_all();
    }

    /// Next frame by priority; `None` once closed (after draining when asked to).
    pub fn next(&self) -> Option<Frame> {
        let mut s = self.state.lock().unwrap();
        loop {
            if s.closing == Some(false) {
                return None;
            }
            if let Some(f) = s.queues.iter_mut().find_map(|q| q.pop_front()) {
                return Some(f);
            }
            if s.closing == Some(true) {
                return None;
            }
            s = self.cv.wait(s).unwrap();
        }
    }

    pub fn sink(self: &Arc<Self>) -> FrameSink {
        let me = self.clone();
        Arc::new(move |f| me.send(f))
    }
}

// ---------------------------------------------------------------- flow control (PROTOCOL.md 5.2)

struct Flow {
    state: Mutex<FlowState>,
    cv: Condvar,
}

#[derive(Default)]
struct FlowState {
    next_frame_id: u32,
    /// Highest acknowledged frame_id + 1.
    acked: u32,
    need_keyframe: bool,
    stop: bool,
}

struct VideoStream {
    stream_id: u32,
    flow: Arc<Flow>,
    thread: Option<JoinHandle<()>>,
}

impl VideoStream {
    fn start(
        stream_id: u32,
        pipeline: Box<dyn VideoPipeline>,
        out: Arc<Outbox>,
        events: Sender<Event>,
        sent: Arc<AtomicU64>,
    ) -> VideoStream {
        let flow = Arc::new(Flow {
            state: Mutex::new(FlowState {
                need_keyframe: true,
                ..Default::default()
            }),
            cv: Condvar::new(),
        });
        let f = flow.clone();
        let thread = std::thread::Builder::new()
            .name(format!("anydisplay-video-{stream_id}"))
            .spawn(move || video_loop(stream_id, pipeline, f, out, events, sent))
            .expect("spawn video thread");
        VideoStream {
            stream_id,
            flow,
            thread: Some(thread),
        }
    }

    fn ack(&self, ack: &FrameAck) {
        let mut s = self.flow.state.lock().unwrap();
        if ack.stream_id == self.stream_id && ack.frame_id < s.next_frame_id {
            s.acked = s.acked.max(ack.frame_id + 1);
            self.flow.cv.notify_all();
        }
    }

    fn request_keyframe(&self) {
        self.flow.state.lock().unwrap().need_keyframe = true;
        self.flow.cv.notify_all();
    }

    fn stop(&mut self) {
        self.flow.state.lock().unwrap().stop = true;
        self.flow.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn video_loop(
    stream_id: u32,
    mut pipeline: Box<dyn VideoPipeline>,
    flow: Arc<Flow>,
    out: Arc<Outbox>,
    events: Sender<Event>,
    sent: Arc<AtomicU64>,
) {
    loop {
        let force = {
            let mut s = flow.state.lock().unwrap();
            while !s.stop && s.next_frame_id.wrapping_sub(s.acked) >= MAX_UNACKED {
                s = flow
                    .cv
                    .wait_timeout(s, Duration::from_millis(100))
                    .unwrap()
                    .0;
            }
            if s.stop {
                return;
            }
            s.need_keyframe
        };
        let frame = match pipeline.next_frame(force) {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(e) => {
                let _ = events.send(Event::VideoFailed(stream_id, format!("{e:#}")));
                return;
            }
        };
        let frame_id = {
            let mut s = flow.state.lock().unwrap();
            if s.stop {
                return;
            }
            // The first frame of every stream is a keyframe; anything before it is useless to the decoder.
            if s.next_frame_id == 0 && !frame.keyframe {
                s.need_keyframe = true;
                continue;
            }
            if frame.keyframe {
                s.need_keyframe = false;
            }
            s.next_frame_id += 1;
            s.next_frame_id - 1
        };
        if frame_id == 0 || frame.keyframe {
            debug!(
                "stream {stream_id} keyframe {frame_id} ({} bytes)",
                frame.data.len()
            );
        }
        out.send(
            VideoFrame {
                stream_id,
                frame_id,
                pts_us: frame.pts_us,
                flags: if frame.keyframe {
                    VIDEO_FLAG_KEYFRAME
                } else {
                    0
                },
                access_unit: frame.data,
            }
            .frame(),
        );
        sent.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- connection setup and handshake

/// Resolves `HOST[:PORT]`, `[v6]:PORT` or a bare IPv6 address.
pub fn resolve(target: &str, default_port: u16) -> anyhow::Result<SocketAddr> {
    use std::net::ToSocketAddrs;
    let with_port = if target.starts_with('[') {
        if target.contains("]:") {
            target.to_string()
        } else {
            format!("{target}:{default_port}")
        }
    } else if target.matches(':').count() == 1 {
        target.to_string()
    } else if target.contains(':') {
        format!("[{target}]:{default_port}")
    } else {
        format!("{target}:{default_port}")
    };
    with_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve {target}"))
}

struct Established {
    stream: TcpStream,
    result: HandshakeResult,
    send: Option<Cipher>,
    recv: Option<Cipher>,
    decoder: FrameDecoder,
}

enum HandshakeOutcome {
    Ok(Box<Established>),
    /// With the client's device_id when HELLO_C got that far.
    Failed(HandshakeError, Option<[u8; 16]>),
}

fn run_handshake(
    mut stream: TcpStream,
    mut hs: HostHandshake,
    cfg: &SessionConfig,
    prompt: &SasPrompt,
    handle: &SessionHandle,
) -> anyhow::Result<HandshakeOutcome> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut decoder = FrameDecoder::default();
    let (mut send, mut recv): (Option<Cipher>, Option<Cipher>) = (None, None);
    let mut deadline = Instant::now() + hs.step_timeout();
    let (decision_tx, decision_rx) = mpsc::channel::<bool>();
    let mut buf = vec![0u8; 64 * 1024];
    let mut pending: VecDeque<Action> = VecDeque::new();
    let mut device_id = None;

    loop {
        // Apply actions in order; a frame decoded after EnableEncryption must use the new key.
        while let Some(action) = pending.pop_front() {
            match action {
                Action::Send(f) => {
                    stream
                        .write_all(&encode_frame(&f, send.as_mut()))
                        .context("sending handshake frame")?;
                }
                Action::ShowSas(code) => {
                    info!("pairing code {code}");
                    if cfg.auto_confirm {
                        let _ = decision_tx.send(true);
                    } else {
                        let (p, tx) = (prompt.clone(), decision_tx.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(p(&code));
                        });
                    }
                }
                Action::EnableEncryption { send: s, recv: r } => {
                    send = Some(Cipher::new(&s));
                    recv = Some(Cipher::new(&r));
                }
                Action::Complete(result) => {
                    stream.set_read_timeout(None)?;
                    return Ok(HandshakeOutcome::Ok(Box::new(Established {
                        stream,
                        result,
                        send,
                        recv,
                        decoder,
                    })));
                }
                Action::Fail(e) => return Ok(HandshakeOutcome::Failed(e, device_id)),
            }
            deadline = Instant::now() + hs.step_timeout();
        }
        if let Some(r) = handle.stop_requested() {
            bail!("stopped during handshake (reason {r})");
        }
        if let Ok(ok) = decision_rx.try_recv() {
            pending.extend(hs.user_decision(ok));
            continue;
        }
        match decoder.next(recv.as_mut()) {
            Ok(Some(frame)) => {
                pending.extend(hs.receive(&frame));
                device_id = device_id.or(hs.device_id());
                continue;
            }
            Ok(None) => {}
            Err(e) => bail!("handshake: {e}"),
        }
        if Instant::now() > deadline {
            pending.extend(hs.timed_out());
            continue;
        }
        match stream.read(&mut buf) {
            Ok(0) => bail!("connection closed during handshake"),
            Ok(n) => decoder.push(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e).context("reading handshake"),
        }
    }
}

fn connect_and_handshake(
    addr: SocketAddr,
    store: &mut PairingStore,
    platform: &dyn Platform,
    cfg: &SessionConfig,
    prompt: &SasPrompt,
    handle: &SessionHandle,
) -> anyhow::Result<Box<Established>> {
    let identity = HostIdentity {
        host_id: store.host_id(),
        host_name: cfg.host_name.clone(),
        host_os: platform.host_os(),
    };
    let mut retries = 0;
    loop {
        let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(s) => s,
            // While re-pairing, the client may still be restarting its listener.
            Err(_) if retries > 0 && retries < PAIRING_RETRIES => {
                retries += 1;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("connecting to {addr}")),
        };
        stream.set_nodelay(true)?;
        info!("connected to {addr}");
        let secrets: std::collections::BTreeMap<[u8; 16], [u8; 32]> = store
            .devices()
            .keys()
            .filter_map(|k| crate::util::from_hex(k)?.try_into().ok())
            .filter_map(|id: [u8; 16]| Some((id, store.secret(&id)?)))
            .collect();
        let hs = HostHandshake::new(identity.clone(), move |id| secrets.get(id).copied());
        match run_handshake(stream, hs, cfg, prompt, handle)? {
            HandshakeOutcome::Ok(e) => return Ok(e),
            HandshakeOutcome::Failed(
                HandshakeError::PeerFailed(auth_fail::UNKNOWN_HOST),
                Some(id),
            ) if retries < PAIRING_RETRIES => {
                // The client lost our pairing: forget it and pair again (mode 1).
                retries += 1;
                info!("client does not know this host; forgetting device {} and reconnecting to pair again", to_hex(&id));
                store.forget(&id)?;
                std::thread::sleep(Duration::from_secs(1));
            }
            HandshakeOutcome::Failed(e, _) => return Err(anyhow!(e)),
        }
    }
}

fn reader_loop(
    mut stream: TcpStream,
    mut decoder: FrameDecoder,
    mut cipher: Option<Cipher>,
    tx: Sender<Event>,
) {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        loop {
            match decoder.next(cipher.as_mut()) {
                Ok(Some(f)) => {
                    if tx.send(Event::Frame(f)).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(Event::Closed(e.to_string()));
                    return;
                }
            }
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                let _ = tx.send(Event::Closed("connection closed by client".into()));
                return;
            }
            Ok(n) => decoder.push(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => {
                let _ = tx.send(Event::Closed(FrameError::Io(e).to_string()));
                return;
            }
        }
    }
}

fn writer_loop(
    mut stream: TcpStream,
    mut cipher: Option<Cipher>,
    out: Arc<Outbox>,
    tx: Sender<Event>,
) {
    while let Some(f) = out.next() {
        if let Err(e) = stream.write_all(&encode_frame(&f, cipher.as_mut())) {
            out.close(false);
            let _ = tx.send(Event::Closed(format!("send failed: {e}")));
            return;
        }
    }
    let _ = stream.flush();
}

// ---------------------------------------------------------------- the session

/// Connects to a client, runs the handshake and the session until either side ends it.
/// Errors before the session is established are returned as `Err`; a session that ends with an
/// error returns `Ok` with `summary.error` set.
pub fn run(
    addr: SocketAddr,
    store: &mut PairingStore,
    platform: &mut dyn Platform,
    cfg: &SessionConfig,
    prompt: SasPrompt,
    handle: &SessionHandle,
) -> anyhow::Result<SessionSummary> {
    let est = *connect_and_handshake(addr, store, platform, cfg, &prompt, handle)?;
    let r = &est.result;
    if let Some(secret) = r.pair_secret {
        store.pair(&r.device_id, &r.device_name, &secret)?;
        info!("paired with {} ({})", r.device_name, to_hex(&r.device_id));
    }
    info!(
        "handshake ok: device={:?} link={} mode={} encrypted={}",
        r.device_name,
        r.link,
        r.mode as u8,
        est.send.is_some()
    );

    let (tx, rx) = mpsc::channel();
    let out = Outbox::new();
    let stream = est.stream;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let reader = {
        let (s, tx) = (stream.try_clone()?, tx.clone());
        let (decoder, recv) = (est.decoder, est.recv);
        std::thread::Builder::new()
            .name("anydisplay-reader".into())
            .spawn(move || reader_loop(s, decoder, recv, tx))?
    };
    let writer = {
        let (s, out, tx) = (stream.try_clone()?, out.clone(), tx.clone());
        let send = est.send;
        std::thread::Builder::new()
            .name("anydisplay-writer".into())
            .spawn(move || writer_loop(s, send, out, tx))?
    };
    handle.attach(tx.clone());

    let now = Instant::now();
    let mut live = Live {
        platform,
        cfg,
        out: out.clone(),
        tx,
        device_id: est.result.device_id,
        device_info: None,
        phase: Phase::AwaitDeviceInfo,
        phase_deadline: now + SETUP_TIMEOUT,
        last_rx: now,
        next_ping: now,
        ping_seq: 0,
        stream_id: 0,
        video: None,
        audio: None,
        sent: Arc::new(AtomicU64::new(0)),
        end_at: None,
        next_clipboard: now,
        summary: SessionSummary {
            device_name: est.result.device_name.clone(),
            mode: Some(est.result.mode),
            ..Default::default()
        },
    };
    let end = live.run(&rx);

    // Shutdown: stop producers, say BYE if we are the one ending it, drain, close.
    if let Some(mut v) = live.video.take() {
        v.stop();
    }
    live.platform.close_stream();
    live.audio = None;
    live.platform.end_session();
    match end.bye {
        Some(reason) => {
            out.send(Bye { reason }.frame());
            out.close(true);
        }
        None => out.close(false),
    }
    let _ = writer.join();
    let _ = stream.shutdown(Shutdown::Both);
    let _ = reader.join();

    let mut summary = live.summary;
    summary.frames_sent = live.sent.load(Ordering::Relaxed);
    summary.error = end.error;
    match &summary.error {
        Some(e) => warn!("session closed: {e}"),
        None => info!("session closed"),
    }
    info!(
        "sent {} video frames in {} stream(s)",
        summary.frames_sent, summary.streams
    );
    Ok(summary)
}

#[derive(PartialEq, Eq)]
enum Phase {
    AwaitDeviceInfo,
    AwaitConfigReq,
    Streaming,
}

struct End {
    error: Option<String>,
    bye: Option<u8>,
}

impl End {
    fn error(e: impl Into<String>) -> End {
        End {
            error: Some(e.into()),
            bye: Some(bye::ERROR),
        }
    }
}

struct Live<'a> {
    platform: &'a mut dyn Platform,
    cfg: &'a SessionConfig,
    out: Arc<Outbox>,
    tx: Sender<Event>,
    device_id: [u8; 16],
    device_info: Option<DeviceInfo>,
    phase: Phase,
    phase_deadline: Instant,
    last_rx: Instant,
    next_ping: Instant,
    ping_seq: u32,
    stream_id: u32,
    video: Option<VideoStream>,
    audio: Option<AudioStream>,
    sent: Arc<AtomicU64>,
    end_at: Option<Instant>,
    next_clipboard: Instant,
    summary: SessionSummary,
}

impl Live<'_> {
    fn run(&mut self, rx: &Receiver<Event>) -> End {
        loop {
            let event = match rx.recv_timeout(TICK) {
                Ok(e) => Some(e),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    return End::error("internal channel closed")
                }
            };
            let end = match event {
                Some(Event::Frame(f)) => {
                    // Any traffic counts as liveness, PONG included.
                    self.last_rx = Instant::now();
                    self.on_frame(f)
                }
                Some(Event::Closed(e)) => Some(End {
                    error: Some(e),
                    bye: None,
                }),
                Some(Event::Stop(reason)) => Some(End {
                    error: None,
                    bye: Some(reason),
                }),
                Some(Event::VideoFailed(sid, e)) if sid == self.stream_id => {
                    Some(End::error(format!("video: {e}")))
                }
                Some(Event::VideoFailed(..)) => None,
                None => None,
            };
            if let Some(end) = end.or_else(|| self.on_tick()) {
                return end;
            }
        }
    }

    fn on_tick(&mut self) -> Option<End> {
        let now = Instant::now();
        if now.duration_since(self.last_rx) > LIVENESS_TIMEOUT {
            return Some(End::error("no data from client for 5 s"));
        }
        if self.phase != Phase::Streaming && now > self.phase_deadline {
            return Some(End::error("timed out waiting for DEVICE_INFO / CONFIG_REQ"));
        }
        if self.end_at.is_some_and(|t| now >= t) {
            return Some(End {
                error: None,
                bye: Some(bye::USER),
            });
        }
        if now >= self.next_ping {
            self.out.send(
                Ping {
                    seq: self.ping_seq,
                    sender_time_us: now_us(),
                }
                .frame(),
            );
            self.ping_seq = self.ping_seq.wrapping_add(1);
            self.next_ping = now + PING_INTERVAL;
        }
        if self.phase == Phase::Streaming {
            self.platform.input_tick();
            if self.cfg.clipboard && now >= self.next_clipboard {
                self.next_clipboard = now + CLIPBOARD_POLL;
                if let Some(text) = self.platform.clipboard_changed() {
                    // Stay well below the 32 MiB frame limit.
                    if text.len() <= 16 << 20 {
                        debug!("clipboard → client ({} bytes)", text.len());
                        self.out.send(
                            Clipboard {
                                mime: "text/plain".into(),
                                data: text.into_bytes(),
                            }
                            .frame(),
                        );
                    }
                }
            }
        }
        None
    }

    fn on_frame(&mut self, f: Frame) -> Option<End> {
        match f.channel {
            channel::CONTROL => self.on_control(f),
            channel::VIDEO if f.ty == ty::FRAME_ACK => {
                if let (Ok(ack), Some(v)) = (FrameAck::from_body(&f.body), &self.video) {
                    v.ack(&ack);
                }
                None
            }
            channel::INPUT => {
                match InputEvent::decode(&f) {
                    Ok(Some(ev)) if self.phase == Phase::Streaming => self.platform.inject(ev),
                    Ok(_) => {}
                    Err(e) => warn!("bad input message 0x{:02x}: {e}", f.ty),
                }
                None
            }
            _ => None,
        }
    }

    fn on_control(&mut self, f: Frame) -> Option<End> {
        let r: Result<Option<End>, crate::proto::wire::DecodeError> = (|| {
            match f.ty {
                ty::DEVICE_INFO => {
                    let i = DeviceInfo::from_body(&f.body)?;
                    info!(
                        "DEVICE_INFO {}x{} dpi={:.0}/{:.0} refresh={:.0} codecs=0x{:x} max_decode={}x{} touches={} pen=0x{:x} audio={} {} {}",
                        i.screen_w, i.screen_h, i.xdpi, i.ydpi, i.max_refresh, i.codecs, i.max_decode_w, i.max_decode_h,
                        i.max_touches, i.pen_caps, i.audio, i.model, i.os_version
                    );
                    self.device_info = Some(i);
                    if self.phase == Phase::AwaitDeviceInfo {
                        self.phase = Phase::AwaitConfigReq;
                        self.phase_deadline = Instant::now() + SETUP_TIMEOUT;
                    }
                }
                ty::CONFIG_REQ => {
                    let req = ConfigReq::from_body(&f.body)?;
                    info!(
                        "CONFIG_REQ {}x{}@{} codec={} kbps={} q={} flags=0x{:x}",
                        req.width,
                        req.height,
                        req.refresh_hz,
                        req.codec,
                        req.bitrate_kbps,
                        req.quality,
                        req.flags
                    );
                    return Ok(self.configure(&req));
                }
                ty::PING => {
                    let p = Ping::from_body(&f.body)?;
                    self.out.send(
                        Pong {
                            seq: p.seq,
                            echoed_time_us: p.sender_time_us,
                        }
                        .frame(),
                    );
                }
                ty::PONG => {
                    let p = Pong::from_body(&f.body)?;
                    debug!(
                        "rtt {:.1} ms",
                        now_us().saturating_sub(p.echoed_time_us) as f64 / 1000.0
                    );
                }
                ty::STATS => {
                    let s = Stats::from_body(&f.body)?;
                    debug!(
                        "STATS decoded={} dropped={} decode={}us rtt={}us bytes={}",
                        s.frames_decoded,
                        s.frames_dropped,
                        s.avg_decode_us,
                        s.rtt_us,
                        s.bytes_received
                    );
                }
                ty::KEYFRAME_REQ => {
                    debug!("keyframe requested");
                    if let Some(v) = &self.video {
                        v.request_keyframe();
                    }
                }
                ty::CLIPBOARD => {
                    let c = Clipboard::from_body(&f.body)?;
                    if self.cfg.clipboard && c.mime == "text/plain" {
                        if let Ok(text) = String::from_utf8(c.data) {
                            debug!("clipboard ← client ({} bytes)", text.len());
                            self.platform.set_clipboard(&text);
                        }
                    }
                }
                ty::BYE => {
                    let reason = f.body.first().copied().unwrap_or(0);
                    info!("client said BYE ({reason})");
                    return Ok(Some(End {
                        error: None,
                        bye: None,
                    }));
                }
                _ => {} // Unknown types are ignored (PROTOCOL.md 6).
            }
            Ok(None)
        })();
        r.unwrap_or_else(|e| {
            warn!("bad control message 0x{:02x}: {e}", f.ty);
            None
        })
    }

    fn notice(&mut self, code: u16, message: &str) {
        info!("NOTICE {code}: {message}");
        self.out.send(
            Notice {
                code,
                message: message.into(),
            }
            .frame(),
        );
        self.summary.notices.push((code, message.into()));
    }

    fn configure(&mut self, req: &ConfigReq) -> Option<End> {
        let info = self.device_info.clone().unwrap_or(DeviceInfo {
            screen_w: 1920,
            screen_h: 1080,
            xdpi: 160.0,
            ydpi: 160.0,
            max_refresh: 60.0,
            codecs: Codec::H264.bit(),
            max_decode_w: 0,
            max_decode_h: 0,
            max_touches: 1,
            pen_caps: 0,
            audio: 0,
            model: String::new(),
            os_version: String::new(),
        });
        let plan = negotiate::plan(&info, req, self.cfg.codec);
        for (code, msg) in &plan.notices {
            self.notice(*code, msg);
        }

        if let Some(mut v) = self.video.take() {
            v.stop();
        }
        self.platform.close_stream();
        self.stream_id += 1;
        let request = StreamRequest {
            plan: &plan,
            device: &info,
            device_id: self.device_id,
            device_name: &self.summary.device_name,
        };
        let opened: OpenedStream = match self.platform.open_stream(&request, self.out.sink()) {
            Ok(o) => o,
            Err(e) => return Some(End::error(format!("cannot start the stream: {e:#}"))),
        };

        let want_audio = self.cfg.audio && plan.wants(stream_flags::AUDIO) && info.audio & 1 != 0;
        if !want_audio {
            self.audio = None;
        } else if self.audio.is_none() {
            match self.platform.start_audio(self.out.sink()) {
                Ok(a) => self.audio = a,
                Err(e) => warn!("audio capture unavailable: {e:#}"),
            }
        }

        let mut flags = opened.flags;
        let mut caps = self.platform.host_caps();
        if !self.cfg.clipboard {
            caps &= !host_caps::CLIPBOARD;
        }
        if self.audio.is_some() {
            flags |= stream_flags::AUDIO;
            caps |= host_caps::AUDIO;
        }
        for (code, msg) in &opened.notices {
            self.notice(*code, msg);
        }
        let config = Config {
            stream_id: self.stream_id,
            width: opened.width as u16,
            height: opened.height as u16,
            refresh_hz: opened.fps as u16,
            codec: opened.codec as u8,
            bitrate_kbps: opened.bitrate_kbps,
            flags,
            logical_w: opened.logical_w as u16,
            logical_h: opened.logical_h as u16,
            host_caps: caps,
        };
        self.out.send(config.frame());
        info!(
            "CONFIG stream={} {}x{}@{} {} {} kbps flags=0x{:x} logical={}x{} caps=0x{:x} hw={} source={}",
            config.stream_id,
            config.width,
            config.height,
            config.refresh_hz,
            opened.codec.name(),
            config.bitrate_kbps,
            flags,
            config.logical_w,
            config.logical_h,
            caps,
            opened.hardware_encoder,
            opened.source
        );
        if let Some(a) = &self.audio {
            self.out.send(a.config.frame());
        }
        self.video = Some(VideoStream::start(
            self.stream_id,
            opened.pipeline,
            self.out.clone(),
            self.tx.clone(),
            self.sent.clone(),
        ));
        self.summary.streams += 1;
        self.summary.last_config = Some(config);
        self.phase = Phase::Streaming;
        if self.end_at.is_none() {
            self.end_at = self.cfg.duration.map(|d| Instant::now() + d);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_priority_and_drain() {
        let out = Outbox::new();
        let f = |ch: u8, t: u8| Frame {
            channel: ch,
            ty: t,
            body: vec![],
        };
        out.send(f(channel::VIDEO, 1));
        out.send(f(channel::AUDIO, 2));
        out.send(f(channel::CURSOR, 3));
        out.send(f(channel::INPUT, 4));
        out.send(f(channel::CONTROL, 5));
        out.close(true);
        out.send(f(channel::CONTROL, 6)); // ignored after close
        let order: Vec<u8> = std::iter::from_fn(|| out.next()).map(|f| f.ty).collect();
        assert_eq!(order, vec![5, 4, 3, 2, 1]);
    }

    #[test]
    fn outbox_drops_stale_audio() {
        let out = Outbox::new();
        for i in 0..(AUDIO_QUEUE_LIMIT + 10) {
            out.send(Frame {
                channel: channel::AUDIO,
                ty: ty::AUDIO_DATA,
                body: vec![i as u8],
            });
        }
        out.close(true);
        let first = out.next().unwrap();
        assert_eq!(first.body, vec![10]);
    }

    #[test]
    fn resolves_targets() {
        assert_eq!(
            resolve("127.0.0.1", 47820).unwrap().to_string(),
            "127.0.0.1:47820"
        );
        assert_eq!(
            resolve("127.0.0.1:5", 47820).unwrap().to_string(),
            "127.0.0.1:5"
        );
        assert_eq!(resolve("::1", 47820).unwrap().to_string(), "[::1]:47820");
        assert_eq!(resolve("[::1]:9", 47820).unwrap().to_string(), "[::1]:9");
    }
}
