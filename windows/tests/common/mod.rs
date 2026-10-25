//! A minimal AnyDisplay client (PROTOCOL.md client role) and a fake platform for session tests.

#![allow(dead_code)]

use anydisplay::platform::{
    EncodedFrame, FrameSink, OpenedStream, Platform, StreamRequest, VideoPipeline,
};
use anydisplay::proto::crypto::{self, KeyPair};
use anydisplay::proto::frame::{encode_frame, read_frame};
use anydisplay::proto::*;
use std::collections::HashMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Client {
    pub stream: TcpStream,
    send: Option<Cipher>,
    recv: Option<Cipher>,
    pub mode: u8,
    pub host_id: [u8; 16],
}

pub struct ClientIdentity {
    pub device_id: [u8; 16],
    /// Paired hosts: host_id → pair_secret.
    pub secrets: HashMap<[u8; 16], [u8; 32]>,
}

impl Client {
    /// Accepts one host connection and runs the client handshake. `confirm` answers the SAS prompt.
    pub fn accept(
        listener: &TcpListener,
        link: u8,
        id: &mut ClientIdentity,
        confirm: bool,
    ) -> Result<Client, String> {
        let (stream, _) = listener.accept().map_err(|e| e.to_string())?;
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut c = Client {
            stream,
            send: None,
            recv: None,
            mode: 0,
            host_id: [0; 16],
        };
        let key = KeyPair::generate();
        let nonce: [u8; 32] = crypto::random_bytes();
        let auth_required = (link == 1) as u8;
        let hello_c = HelloC {
            magic: MAGIC,
            major: 1,
            minor: 0,
            device_id: id.device_id,
            device_name: "Test Tablet".into(),
            link,
            auth_required,
            client_pub: key.public,
            client_commit: crypto::sha256(&[&nonce]),
        };
        c.send(hello_c.frame());
        let f = c.recv_raw()?;
        if f.ty != ty::HELLO_H {
            return Err(format!("expected HELLO_H, got {f:?}"));
        }
        let hello_h = HelloH::from_body(&f.body).unwrap();
        c.mode = hello_h.auth_mode;
        c.host_id = hello_h.host_id;
        if c.mode == 0 && auth_required == 1 {
            c.send(
                AuthFail {
                    reason: auth_fail::AUTH_REQUIRED,
                }
                .frame(),
            );
            return Err("auth required".into());
        }
        let secret = id.secrets.get(&hello_h.host_id).copied();
        if c.mode == 2 && secret.is_none() {
            c.send(
                AuthFail {
                    reason: auth_fail::UNKNOWN_HOST,
                }
                .frame(),
            );
            return Err("unknown host".into());
        }
        let reveal = Reveal {
            client_nonce: nonce,
        };
        c.send(reveal.frame());
        let transcript = crypto::sha256(&[&hello_c.body(), &f.body, &reveal.body()]);
        if c.mode == 1 {
            if !confirm {
                c.send(
                    AuthFail {
                        reason: auth_fail::USER_REJECTED,
                    }
                    .frame(),
                );
                return Err("rejected".into());
            }
            c.send(SasOk {}.frame());
            let f = c.recv_raw()?;
            if f.ty != ty::SAS_OK {
                return Err(format!("expected SAS_OK, got {f:?} {:?}", f.body));
            }
        }
        let z = key.ecdh(&hello_h.host_pub).unwrap();
        let psk = if c.mode == 2 {
            secret.unwrap().to_vec()
        } else {
            vec![]
        };
        let keys = crypto::key_schedule(&z, &psk, &transcript);
        let f = c.recv_raw()?;
        if f.ty != ty::AUTH_H
            || !crypto::verify_confirm_mac(&keys.k_confirm, b"host", &transcript, &f.body)
        {
            c.send(
                AuthFail {
                    reason: auth_fail::BAD_MAC,
                }
                .frame(),
            );
            return Err(format!("bad AUTH_H {f:?}"));
        }
        c.send(
            AuthC {
                mac: crypto::confirm_mac(&keys.k_confirm, b"client", &transcript),
            }
            .frame(),
        );
        if c.mode != 0 {
            c.send = Some(Cipher::new(&keys.k_c2h));
            c.recv = Some(Cipher::new(&keys.k_h2c));
        }
        if c.mode == 1 {
            let secret: [u8; 32] = crypto::random_bytes();
            c.send(
                Pair {
                    pair_secret: secret,
                }
                .frame(),
            );
            id.secrets.insert(hello_h.host_id, secret);
        }
        Ok(c)
    }

    pub fn send(&mut self, f: Frame) {
        let bytes = encode_frame(&f, self.send.as_mut());
        self.stream.write_all(&bytes).unwrap();
    }

    pub fn recv_raw(&mut self) -> Result<Frame, String> {
        read_frame(&mut self.stream, self.recv.as_mut()).map_err(|e| e.to_string())
    }

    /// Next frame that is not a PING (PINGs are answered).
    pub fn recv(&mut self) -> Result<Frame, String> {
        loop {
            let f = self.recv_raw()?;
            if f.ty == ty::PING {
                let p = Ping::from_body(&f.body).unwrap();
                self.send(
                    Pong {
                        seq: p.seq,
                        echoed_time_us: p.sender_time_us,
                    }
                    .frame(),
                );
                continue;
            }
            return Ok(f);
        }
    }

    /// Skips frames until one of type `t` arrives.
    pub fn recv_type(&mut self, t: u8) -> Frame {
        loop {
            let f = self.recv().expect("frame");
            if f.ty == t {
                return f;
            }
        }
    }

    /// Frames received within `d` (PINGs answered and excluded).
    pub fn collect_for(&mut self, d: Duration) -> Vec<Frame> {
        let end = Instant::now() + d;
        let mut out = Vec::new();
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            self.stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .unwrap();
            match self.recv() {
                Ok(f) => out.push(f),
                Err(_) => break,
            }
        }
        self.stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        out
    }

    pub fn start_streaming(&mut self, req: ConfigReq) -> Config {
        self.send(
            DeviceInfo {
                screen_w: 1280,
                screen_h: 800,
                xdpi: 200.0,
                ydpi: 200.0,
                max_refresh: 60.0,
                codecs: 3,
                max_decode_w: 4096,
                max_decode_h: 4096,
                max_touches: 10,
                pen_caps: 0x1f,
                audio: 1,
                model: "Fake".into(),
                os_version: "1".into(),
            }
            .frame(),
        );
        self.send(req.frame());
        Config::from_body(&self.recv_type(ty::CONFIG).body).unwrap()
    }
}

pub fn default_req() -> ConfigReq {
    ConfigReq {
        width: 0,
        height: 0,
        refresh_hz: 60,
        codec: 1,
        bitrate_kbps: 4000,
        quality: 80,
        flags: 0,
    }
}

// ---------------------------------------------------------------- fake platform

#[derive(Default)]
pub struct Recorded {
    pub input: Vec<InputEvent>,
    pub clipboard_set: Vec<String>,
    pub clipboard_to_send: Vec<String>,
    pub streams_opened: u32,
    pub ended: bool,
}

pub struct FakePlatform {
    pub rec: Arc<Mutex<Recorded>>,
}

pub struct FakePipeline {
    n: u64,
}

impl VideoPipeline for FakePipeline {
    fn next_frame(&mut self, force_keyframe: bool) -> anyhow::Result<Option<EncodedFrame>> {
        std::thread::sleep(Duration::from_millis(5));
        self.n += 1;
        // The very first output is a P frame: the session must hold it back until a keyframe.
        let key = force_keyframe && self.n > 1;
        let nal = if key { 0x65 } else { 0x41 };
        Ok(Some(EncodedFrame {
            data: vec![0, 0, 0, 1, nal, self.n as u8],
            keyframe: key,
            pts_us: self.n,
        }))
    }
}

impl Platform for FakePlatform {
    fn host_os(&self) -> String {
        "fake".into()
    }

    fn host_caps(&self) -> u32 {
        host_caps::NATIVE_TOUCH | host_caps::NATIVE_PEN | host_caps::CLIPBOARD
    }

    fn open_stream(
        &mut self,
        req: &StreamRequest<'_>,
        _cursor: FrameSink,
    ) -> anyhow::Result<OpenedStream> {
        self.rec.lock().unwrap().streams_opened += 1;
        let p = req.plan;
        Ok(OpenedStream {
            width: p.width,
            height: p.height,
            fps: p.fps,
            codec: p.codec,
            bitrate_kbps: p.bitrate_kbps,
            flags: 0,
            logical_w: p.width,
            logical_h: p.height,
            notices: vec![(notice::VIRTUAL_DISPLAY_UNAVAILABLE, "mirroring".into())],
            hardware_encoder: true,
            source: "fake".into(),
            pipeline: Box::new(FakePipeline { n: 0 }),
        })
    }

    fn inject(&mut self, event: InputEvent) {
        self.rec.lock().unwrap().input.push(event);
    }

    fn clipboard_changed(&mut self) -> Option<String> {
        self.rec.lock().unwrap().clipboard_to_send.pop()
    }

    fn set_clipboard(&mut self, text: &str) {
        self.rec.lock().unwrap().clipboard_set.push(text.into());
    }

    fn end_session(&mut self) {
        self.rec.lock().unwrap().ended = true;
    }
}
