//! Full host sessions against an in-process client over loopback TCP.

mod common;

use common::*;
use anydisplay::pairing::PairingStore;
use anydisplay::proto::*;
use anydisplay::session::{self, SessionConfig, SessionHandle, SessionSummary};
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

fn temp_store() -> std::path::PathBuf {
    let n: [u8; 8] = anydisplay::proto::crypto::random_bytes();
    std::env::temp_dir()
        .join(format!("anydisplay-test-{}", anydisplay::util::to_hex(&n)))
        .join("host.json")
}

struct Host {
    handle: SessionHandle,
    thread: JoinHandle<anyhow::Result<SessionSummary>>,
    rec: Arc<Mutex<Recorded>>,
}

fn start_host(port: u16, store: std::path::PathBuf, confirm: bool) -> Host {
    let rec = Arc::new(Mutex::new(Recorded::default()));
    let handle = SessionHandle::default();
    let (h, r) = (handle.clone(), rec.clone());
    let thread = std::thread::spawn(move || {
        let mut store = PairingStore::open(store)?;
        let mut platform = FakePlatform { rec: r };
        let cfg = SessionConfig {
            host_name: "Test PC".into(),
            auto_confirm: false,
            ..Default::default()
        };
        let addr = format!("127.0.0.1:{port}").parse().unwrap();
        session::run(
            addr,
            &mut store,
            &mut platform,
            &cfg,
            Arc::new(move |_| confirm),
            &h,
        )
    });
    Host {
        handle,
        thread,
        rec,
    }
}

fn video(f: &Frame) -> VideoFrame {
    assert_eq!((f.channel, f.ty), (channel::VIDEO, ty::VIDEO_FRAME));
    VideoFrame::from_body(&f.body).unwrap()
}

fn ack(c: &mut Client, v: &VideoFrame) {
    c.send(
        FrameAck {
            stream_id: v.stream_id,
            frame_id: v.frame_id,
            decode_us: 100,
        }
        .frame(),
    );
}

#[test]
fn mode0_stream_flow_control_keyframes_reconfig_input_clipboard_bye() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = start_host(listener.local_addr().unwrap().port(), temp_store(), true);
    let mut id = ClientIdentity {
        device_id: [7; 16],
        secrets: HashMap::new(),
    };
    let mut c = Client::accept(&listener, 0, &mut id, true).unwrap();
    assert_eq!(c.mode, 0);

    let notice = {
        c.send(
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
        c.send(default_req().frame());
        Notice::from_body(&c.recv_type(ty::NOTICE).body).unwrap()
    };
    assert_eq!(notice.code, notice::VIRTUAL_DISPLAY_UNAVAILABLE);
    let config = Config::from_body(&c.recv_type(ty::CONFIG).body).unwrap();
    assert_eq!(
        (config.stream_id, config.width, config.height, config.codec),
        (1, 1280, 800, 1)
    );
    assert_eq!(
        config.host_caps,
        host_caps::NATIVE_TOUCH | host_caps::NATIVE_PEN | host_caps::CLIPBOARD
    );

    // Without acks the host stops at 3 frames; the first is a keyframe with frame_id 0.
    let frames: Vec<VideoFrame> = c
        .collect_for(Duration::from_millis(400))
        .iter()
        .filter(|f| f.ty == ty::VIDEO_FRAME)
        .map(video)
        .collect();
    assert_eq!(
        frames.iter().map(|v| v.frame_id).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(frames[0].flags & VIDEO_FLAG_KEYFRAME, 1);
    assert_eq!(frames[1].flags & VIDEO_FLAG_KEYFRAME, 0);
    assert!(frames.iter().all(|v| v.stream_id == 1));

    // One ack frees exactly one slot.
    ack(&mut c, &frames[0]);
    let more: Vec<VideoFrame> = c
        .collect_for(Duration::from_millis(300))
        .iter()
        .filter(|f| f.ty == ty::VIDEO_FRAME)
        .map(video)
        .collect();
    assert_eq!(more.iter().map(|v| v.frame_id).collect::<Vec<_>>(), vec![3]);

    // KEYFRAME_REQ: a keyframe follows once the window opens.
    c.send(KeyframeReq {}.frame());
    for v in frames[1..].iter().chain(&more) {
        ack(&mut c, v);
    }
    let mut got_key = false;
    for _ in 0..10 {
        let v = video(&c.recv_type(ty::VIDEO_FRAME));
        ack(&mut c, &v);
        if v.flags & VIDEO_FLAG_KEYFRAME != 0 {
            got_key = true;
            break;
        }
    }
    assert!(got_key);

    // A new CONFIG_REQ starts stream 2 at frame 0 with a keyframe.
    let mut req = default_req();
    (req.width, req.height) = (800, 1280);
    c.send(req.frame());
    let config2 = Config::from_body(&c.recv_type(ty::CONFIG).body).unwrap();
    assert_eq!(
        (config2.stream_id, config2.width, config2.height),
        (2, 800, 1280)
    );
    let first = loop {
        let v = video(&c.recv_type(ty::VIDEO_FRAME));
        if v.stream_id == 2 {
            break v;
        }
    };
    assert_eq!((first.frame_id, first.flags & VIDEO_FLAG_KEYFRAME), (0, 1));

    // Input, clipboard both ways, ping/pong.
    let touch = Touch {
        contacts: vec![TouchContact {
            id: 1,
            action: touch_action::DOWN,
            x: 10,
            y: 20,
            pressure: 30,
            size: 40,
        }],
    };
    c.send(touch.frame());
    c.send(
        Text {
            text: "merhaba".into(),
        }
        .frame(),
    );
    c.send(
        Clipboard {
            mime: "text/plain".into(),
            data: b"from tablet".to_vec(),
        }
        .frame(),
    );
    host.rec
        .lock()
        .unwrap()
        .clipboard_to_send
        .push("from pc".into());
    let clip = Clipboard::from_body(&c.recv_type(ty::CLIPBOARD).body).unwrap();
    assert_eq!(clip.data, b"from pc");
    c.send(
        Ping {
            seq: 42,
            sender_time_us: 99,
        }
        .frame(),
    );
    let pong = Pong::from_body(&c.recv_type(ty::PONG).body).unwrap();
    assert_eq!((pong.seq, pong.echoed_time_us), (42, 99));
    {
        let rec = host.rec.lock().unwrap();
        assert_eq!(
            rec.input,
            vec![
                InputEvent::Touch(touch),
                InputEvent::Text(Text {
                    text: "merhaba".into()
                })
            ]
        );
        assert_eq!(rec.clipboard_set, vec!["from tablet".to_string()]);
    }

    // Host ends the session: BYE(user), then the connection closes.
    host.handle.stop(bye::USER);
    let b = c.recv_type(ty::BYE);
    assert_eq!(b.body, vec![bye::USER]);
    while c.recv_raw().is_ok() {}
    let summary = host.thread.join().unwrap().unwrap();
    assert_eq!(summary.streams, 2);
    assert!(summary.error.is_none());
    assert!(host.rec.lock().unwrap().ended);
}

#[test]
fn mode1_pairs_then_mode2_is_encrypted_and_unknown_host_repairs() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = temp_store();
    let mut id = ClientIdentity {
        device_id: [9; 16],
        secrets: HashMap::new(),
    };

    // Network link without a pairing: mode 1.
    let host = start_host(port, store.clone(), true);
    let mut c = Client::accept(&listener, 1, &mut id, true).unwrap();
    assert_eq!(c.mode, 1);
    let config = c.start_streaming(default_req());
    assert_eq!(config.stream_id, 1);
    c.send(Bye { reason: bye::USER }.frame());
    let summary = host.thread.join().unwrap().unwrap();
    assert!(summary.error.is_none(), "{:?}", summary.error);
    assert_eq!(
        PairingStore::open(&store).unwrap().secret(&[9; 16]),
        id.secrets.values().next().copied()
    );

    // Same device again: mode 2, still encrypted (CONFIG decrypts with the new keys).
    let host = start_host(port, store.clone(), true);
    let mut c = Client::accept(&listener, 1, &mut id, true).unwrap();
    assert_eq!(c.mode, 2);
    assert_eq!(c.start_streaming(default_req()).stream_id, 1);
    c.send(Bye { reason: bye::USER }.frame());
    host.thread.join().unwrap().unwrap();

    // The client forgot the host: AUTH_FAIL(unknown_host) → host forgets, reconnects, pairs again.
    id.secrets.clear();
    let host = start_host(port, store.clone(), true);
    assert_eq!(
        Client::accept(&listener, 1, &mut id, true).err().as_deref(),
        Some("unknown host")
    );
    let mut c = Client::accept(&listener, 1, &mut id, true).unwrap();
    assert_eq!(c.mode, 1);
    c.start_streaming(default_req());
    c.send(Bye { reason: bye::USER }.frame());
    host.thread.join().unwrap().unwrap();
    assert_eq!(
        PairingStore::open(&store).unwrap().secret(&[9; 16]),
        id.secrets.values().next().copied()
    );
}

#[test]
fn rejected_pairing_code_fails_the_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = start_host(listener.local_addr().unwrap().port(), temp_store(), false);
    let (stream, _) = listener.accept().unwrap();
    drop(stream); // first: make sure a dropped connection is reported, not hung
    let err = host.thread.join().unwrap().unwrap_err();
    assert!(format!("{err:#}").contains("closed"), "{err:#}");

    // Host user rejects: the client sees AUTH_FAIL(user_rejected).
    let host = start_host(listener.local_addr().unwrap().port(), temp_store(), false);
    let mut id = ClientIdentity {
        device_id: [1; 16],
        secrets: HashMap::new(),
    };
    let r = Client::accept(&listener, 1, &mut id, true);
    let err = r.err().unwrap();
    assert!(err.contains("type=0x05"), "client saw {err}");
    let err = host.thread.join().unwrap().unwrap_err();
    assert!(format!("{err:#}").contains("rejected"), "{err:#}");
}

#[test]
fn silent_client_times_out_after_5_seconds() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = start_host(listener.local_addr().unwrap().port(), temp_store(), true);
    let mut id = ClientIdentity {
        device_id: [2; 16],
        secrets: HashMap::new(),
    };
    let mut c = Client::accept(&listener, 0, &mut id, true).unwrap();
    c.start_streaming(default_req());
    // Read without answering pings or acking: the host must give up and say BYE(error).
    let start = std::time::Instant::now();
    let bye = loop {
        let f = c.recv_raw().unwrap();
        if f.ty == ty::BYE {
            break f;
        }
    };
    assert_eq!(bye.body, vec![bye::ERROR]);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_secs(4) && elapsed < Duration::from_secs(8),
        "{elapsed:?}"
    );
    let summary = host.thread.join().unwrap().unwrap();
    assert!(summary.error.unwrap().contains("5 s"));
}
