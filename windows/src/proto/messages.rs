//! Message bodies of PROTOCOL.md sections 4 and 5. Decoding ignores trailing bytes (section 6).

use super::frame::Frame;
use super::wire::{DecodeError, Reader, Writer};

pub const MAGIC: [u8; 4] = *b"ANYD";
pub const MAJOR: u16 = 1;
pub const MINOR: u16 = 0;
pub const DEFAULT_PORT: u16 = 47820;

pub mod channel {
    pub const CONTROL: u8 = 0;
    pub const VIDEO: u8 = 1;
    pub const AUDIO: u8 = 2;
    pub const CURSOR: u8 = 3;
    pub const INPUT: u8 = 4;
}

pub mod ty {
    pub const HELLO_C: u8 = 0x01;
    pub const HELLO_H: u8 = 0x02;
    pub const AUTH_H: u8 = 0x03;
    pub const AUTH_C: u8 = 0x04;
    pub const AUTH_FAIL: u8 = 0x05;
    pub const SAS_OK: u8 = 0x06;
    pub const PAIR: u8 = 0x07;
    pub const REVEAL: u8 = 0x08;
    pub const DEVICE_INFO: u8 = 0x10;
    pub const CONFIG_REQ: u8 = 0x11;
    pub const CONFIG: u8 = 0x12;
    pub const PING: u8 = 0x13;
    pub const PONG: u8 = 0x14;
    pub const STATS: u8 = 0x15;
    pub const KEYFRAME_REQ: u8 = 0x16;
    pub const NOTICE: u8 = 0x17;
    pub const CLIPBOARD: u8 = 0x18;
    pub const BYE: u8 = 0x19;
    pub const VIDEO_FRAME: u8 = 0x20;
    pub const FRAME_ACK: u8 = 0x21;
    pub const CURSOR_POS: u8 = 0x30;
    pub const CURSOR_SHAPE: u8 = 0x31;
    pub const AUDIO_CONFIG: u8 = 0x40;
    pub const AUDIO_DATA: u8 = 0x41;
    pub const TOUCH: u8 = 0x50;
    pub const PEN: u8 = 0x51;
    pub const KEY: u8 = 0x52;
    pub const TEXT: u8 = 0x53;
    pub const SCROLL: u8 = 0x54;
    pub const MOUSE: u8 = 0x55;
}

pub mod auth_fail {
    pub const BAD_VERSION: u8 = 1;
    pub const AUTH_REQUIRED: u8 = 2;
    pub const UNKNOWN_HOST: u8 = 3;
    pub const BAD_MAC: u8 = 4;
    pub const USER_REJECTED: u8 = 5;
    pub const BUSY: u8 = 6;
    pub const BAD_COMMIT: u8 = 7;

    pub fn name(reason: u8) -> &'static str {
        match reason {
            BAD_VERSION => "bad_version",
            AUTH_REQUIRED => "auth_required",
            UNKNOWN_HOST => "unknown_host",
            BAD_MAC => "bad_mac",
            USER_REJECTED => "user_rejected",
            BUSY => "busy",
            BAD_COMMIT => "bad_commit",
            _ => "unknown",
        }
    }
}

pub mod notice {
    pub const SOFTWARE_ENCODER: u16 = 1;
    pub const CAPTURE_PERMISSION: u16 = 2;
    pub const INPUT_PERMISSION: u16 = 3;
    pub const RESOLUTION_UNSUPPORTED: u16 = 4;
    pub const VIRTUAL_DISPLAY_UNAVAILABLE: u16 = 5;
}

pub mod bye {
    pub const USER: u8 = 0;
    pub const SHUTDOWN: u8 = 1;
    pub const ERROR: u8 = 2;
}

/// `CONFIG_REQ.flags` / `CONFIG.flags`.
pub mod stream_flags {
    pub const HIDPI: u8 = 1 << 0;
    pub const AUDIO: u8 = 1 << 1;
    pub const CLIENT_CURSOR: u8 = 1 << 2;
    pub const MIRROR: u8 = 1 << 3;
}

/// `CONFIG.host_caps`.
pub mod host_caps {
    pub const NATIVE_TOUCH: u32 = 1 << 0;
    pub const NATIVE_PEN: u32 = 1 << 1;
    pub const CLIPBOARD: u32 = 1 << 2;
    pub const AUDIO: u32 = 1 << 3;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264 = 1,
    Hevc = 2,
    Av1 = 3,
}

impl Codec {
    pub fn from_u8(v: u8) -> Option<Codec> {
        match v {
            1 => Some(Codec::H264),
            2 => Some(Codec::Hevc),
            3 => Some(Codec::Av1),
            _ => None,
        }
    }

    /// Bit in `DEVICE_INFO.codecs`.
    pub fn bit(self) -> u32 {
        1 << (self as u32 - 1)
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }
}

/// A message with a fixed channel and type.
pub trait Message: Sized {
    const CHANNEL: u8;
    const TYPE: u8;
    fn encode(&self, w: &mut Writer);
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError>;

    fn body(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.finish()
    }

    fn frame(&self) -> Frame {
        Frame {
            channel: Self::CHANNEL,
            ty: Self::TYPE,
            body: self.body(),
        }
    }

    fn from_body(body: &[u8]) -> Result<Self, DecodeError> {
        Self::decode(&mut Reader::new(body))
    }
}

macro_rules! message {
    ($name:ident, $ch:expr, $ty:expr, { $($field:ident : $fty:ty => $enc:ident),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name { $(pub $field: $fty),* }

        impl Message for $name {
            const CHANNEL: u8 = $ch;
            const TYPE: u8 = $ty;
            #[allow(unused_variables)]
            fn encode(&self, w: &mut Writer) { $(message!(@enc w, self.$field, $enc);)* }
            #[allow(unused_variables)]
            fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                Ok(Self { $($field: message!(@dec r, $enc)),* })
            }
        }
    };
    (@enc $w:ident, $v:expr, u8) => { $w.u8($v); };
    (@enc $w:ident, $v:expr, u16) => { $w.u16($v); };
    (@enc $w:ident, $v:expr, i16) => { $w.i16($v); };
    (@enc $w:ident, $v:expr, u32) => { $w.u32($v); };
    (@enc $w:ident, $v:expr, u64) => { $w.u64($v); };
    (@enc $w:ident, $v:expr, f32) => { $w.f32($v); };
    (@enc $w:ident, $v:expr, str) => { $w.str(&$v); };
    (@enc $w:ident, $v:expr, blob) => { $w.blob(&$v); };
    (@enc $w:ident, $v:expr, array) => { $w.raw(&$v); };
    (@enc $w:ident, $v:expr, rest) => { $w.raw(&$v); };
    (@dec $r:ident, u8) => { $r.u8()? };
    (@dec $r:ident, u16) => { $r.u16()? };
    (@dec $r:ident, i16) => { $r.i16()? };
    (@dec $r:ident, u32) => { $r.u32()? };
    (@dec $r:ident, u64) => { $r.u64()? };
    (@dec $r:ident, f32) => { $r.f32()? };
    (@dec $r:ident, str) => { $r.str()? };
    (@dec $r:ident, blob) => { $r.blob()?.to_vec() };
    (@dec $r:ident, array) => { $r.array()? };
    (@dec $r:ident, rest) => { $r.rest().to_vec() };
}

use channel::*;

// ---------------------------------------------------------------- handshake (4.1)

message!(HelloC, CONTROL, ty::HELLO_C, {
    magic: [u8; 4] => array,
    major: u16 => u16,
    minor: u16 => u16,
    device_id: [u8; 16] => array,
    device_name: String => str,
    link: u8 => u8,
    auth_required: u8 => u8,
    client_pub: [u8; 65] => array,
    client_commit: [u8; 32] => array,
});

message!(HelloH, CONTROL, ty::HELLO_H, {
    magic: [u8; 4] => array,
    major: u16 => u16,
    minor: u16 => u16,
    host_id: [u8; 16] => array,
    host_name: String => str,
    host_os: String => str,
    auth_mode: u8 => u8,
    host_pub: [u8; 65] => array,
    host_nonce: [u8; 32] => array,
});

message!(Reveal, CONTROL, ty::REVEAL, { client_nonce: [u8; 32] => array });
message!(SasOk, CONTROL, ty::SAS_OK, {});
message!(AuthH, CONTROL, ty::AUTH_H, { mac: [u8; 32] => array });
message!(AuthC, CONTROL, ty::AUTH_C, { mac: [u8; 32] => array });
message!(AuthFail, CONTROL, ty::AUTH_FAIL, { reason: u8 => u8 });
message!(Pair, CONTROL, ty::PAIR, { pair_secret: [u8; 32] => array });

// ---------------------------------------------------------------- control (5.1)

message!(DeviceInfo, CONTROL, ty::DEVICE_INFO, {
    screen_w: u16 => u16,
    screen_h: u16 => u16,
    xdpi: f32 => f32,
    ydpi: f32 => f32,
    max_refresh: f32 => f32,
    codecs: u32 => u32,
    max_decode_w: u16 => u16,
    max_decode_h: u16 => u16,
    max_touches: u8 => u8,
    pen_caps: u32 => u32,
    audio: u8 => u8,
    model: String => str,
    os_version: String => str,
});

message!(ConfigReq, CONTROL, ty::CONFIG_REQ, {
    width: u16 => u16,
    height: u16 => u16,
    refresh_hz: u16 => u16,
    codec: u8 => u8,
    bitrate_kbps: u32 => u32,
    quality: u8 => u8,
    flags: u8 => u8,
});

message!(Config, CONTROL, ty::CONFIG, {
    stream_id: u32 => u32,
    width: u16 => u16,
    height: u16 => u16,
    refresh_hz: u16 => u16,
    codec: u8 => u8,
    bitrate_kbps: u32 => u32,
    flags: u8 => u8,
    logical_w: u16 => u16,
    logical_h: u16 => u16,
    host_caps: u32 => u32,
});

message!(Ping, CONTROL, ty::PING, { seq: u32 => u32, sender_time_us: u64 => u64 });
message!(Pong, CONTROL, ty::PONG, { seq: u32 => u32, echoed_time_us: u64 => u64 });

message!(Stats, CONTROL, ty::STATS, {
    frames_decoded: u32 => u32,
    frames_dropped: u32 => u32,
    avg_decode_us: u32 => u32,
    rtt_us: u32 => u32,
    bytes_received: u32 => u32,
});

message!(KeyframeReq, CONTROL, ty::KEYFRAME_REQ, {});
message!(Notice, CONTROL, ty::NOTICE, { code: u16 => u16, message: String => str });
message!(Clipboard, CONTROL, ty::CLIPBOARD, { mime: String => str, data: Vec<u8> => blob });
message!(Bye, CONTROL, ty::BYE, { reason: u8 => u8 });

// ---------------------------------------------------------------- video (5.2)

pub const VIDEO_FLAG_KEYFRAME: u8 = 1;

message!(VideoFrame, VIDEO, ty::VIDEO_FRAME, {
    stream_id: u32 => u32,
    frame_id: u32 => u32,
    pts_us: u64 => u64,
    flags: u8 => u8,
    access_unit: Vec<u8> => rest,
});

message!(FrameAck, VIDEO, ty::FRAME_ACK, { stream_id: u32 => u32, frame_id: u32 => u32, decode_us: u32 => u32 });

// ---------------------------------------------------------------- audio (5.3)

message!(AudioConfig, AUDIO, ty::AUDIO_CONFIG, { sample_rate: u32 => u32, channels: u8 => u8, codec: u8 => u8 });
message!(AudioData, AUDIO, ty::AUDIO_DATA, { pts_us: u64 => u64, samples: Vec<u8> => rest });

// ---------------------------------------------------------------- cursor (5.4)

message!(CursorPos, CURSOR, ty::CURSOR_POS, { x: i16 => i16, y: i16 => i16, visible: u8 => u8 });
message!(CursorShape, CURSOR, ty::CURSOR_SHAPE, {
    w: u16 => u16,
    h: u16 => u16,
    hot_x: u16 => u16,
    hot_y: u16 => u16,
    rgba: Vec<u8> => rest,
});

// ---------------------------------------------------------------- input (5.5)

pub mod touch_action {
    pub const DOWN: u8 = 0;
    pub const MOVE: u8 = 1;
    pub const UP: u8 = 2;
    pub const CANCEL: u8 = 3;
}

pub mod pen_action {
    pub const DOWN: u8 = 0;
    pub const MOVE: u8 = 1;
    pub const UP: u8 = 2;
    pub const HOVER_ENTER: u8 = 3;
    pub const HOVER_MOVE: u8 = 4;
    pub const HOVER_EXIT: u8 = 5;
    pub const CANCEL: u8 = 6;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchContact {
    pub id: u8,
    pub action: u8,
    pub x: u16,
    pub y: u16,
    pub pressure: u16,
    pub size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    pub contacts: Vec<TouchContact>,
}

impl Message for Touch {
    const CHANNEL: u8 = INPUT;
    const TYPE: u8 = ty::TOUCH;
    fn encode(&self, w: &mut Writer) {
        w.u8(self.contacts.len() as u8);
        for c in &self.contacts {
            w.u8(c.id)
                .u8(c.action)
                .u16(c.x)
                .u16(c.y)
                .u16(c.pressure)
                .u16(c.size);
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let n = r.u8()?;
        let contacts = (0..n)
            .map(|_| {
                Ok(TouchContact {
                    id: r.u8()?,
                    action: r.u8()?,
                    x: r.u16()?,
                    y: r.u16()?,
                    pressure: r.u16()?,
                    size: r.u16()?,
                })
            })
            .collect::<Result<_, DecodeError>>()?;
        Ok(Touch { contacts })
    }
}

pub const PEN_TOOL_PEN: u8 = 0;
pub const PEN_TOOL_ERASER: u8 = 1;
pub const PEN_BUTTON_BARREL: u8 = 1 << 0;
pub const PEN_BUTTON_SECONDARY: u8 = 1 << 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PenSample {
    pub action: u8,
    pub tool: u8,
    pub buttons: u8,
    pub x: u16,
    pub y: u16,
    pub pressure: u16,
    pub tilt_x: i16,
    pub tilt_y: i16,
    pub twist: u16,
    pub time_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pen {
    pub samples: Vec<PenSample>,
}

impl Message for Pen {
    const CHANNEL: u8 = INPUT;
    const TYPE: u8 = ty::PEN;
    fn encode(&self, w: &mut Writer) {
        w.u8(self.samples.len() as u8);
        for s in &self.samples {
            w.u8(s.action)
                .u8(s.tool)
                .u8(s.buttons)
                .u16(s.x)
                .u16(s.y)
                .u16(s.pressure);
            w.i16(s.tilt_x).i16(s.tilt_y).u16(s.twist).u32(s.time_ms);
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let n = r.u8()?;
        let samples = (0..n)
            .map(|_| {
                Ok(PenSample {
                    action: r.u8()?,
                    tool: r.u8()?,
                    buttons: r.u8()?,
                    x: r.u16()?,
                    y: r.u16()?,
                    pressure: r.u16()?,
                    tilt_x: r.i16()?,
                    tilt_y: r.i16()?,
                    twist: r.u16()?,
                    time_ms: r.u32()?,
                })
            })
            .collect::<Result<_, DecodeError>>()?;
        Ok(Pen { samples })
    }
}

pub mod modifiers {
    pub const CTRL: u16 = 1 << 0;
    pub const SHIFT: u16 = 1 << 1;
    pub const ALT: u16 = 1 << 2;
    pub const META: u16 = 1 << 3;
}

message!(Key, INPUT, ty::KEY, { action: u8 => u8, hid_usage: u16 => u16, modifiers: u16 => u16 });
message!(Text, INPUT, ty::TEXT, { text: String => str });
message!(Scroll, INPUT, ty::SCROLL, { x: u16 => u16, y: u16 => u16, dx: i16 => i16, dy: i16 => i16, phase: u8 => u8 });

pub mod mouse_action {
    pub const MOVE: u8 = 0;
    pub const DOWN: u8 = 1;
    pub const UP: u8 = 2;
}

message!(Mouse, INPUT, ty::MOUSE, { action: u8 => u8, button: u8 => u8, x: u16 => u16, y: u16 => u16 });

/// A decoded message on the input channel.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Touch(Touch),
    Pen(Pen),
    Key(Key),
    Text(Text),
    Scroll(Scroll),
    Mouse(Mouse),
}

impl InputEvent {
    /// `Ok(None)` for unknown input types, which receivers ignore.
    pub fn decode(frame: &Frame) -> Result<Option<InputEvent>, DecodeError> {
        let b = &frame.body[..];
        Ok(Some(match frame.ty {
            ty::TOUCH => InputEvent::Touch(Touch::from_body(b)?),
            ty::PEN => InputEvent::Pen(Pen::from_body(b)?),
            ty::KEY => InputEvent::Key(Key::from_body(b)?),
            ty::TEXT => InputEvent::Text(Text::from_body(b)?),
            ty::SCROLL => InputEvent::Scroll(Scroll::from_body(b)?),
            ty::MOUSE => InputEvent::Mouse(Mouse::from_body(b)?),
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_layout() {
        // Same bytes anydisplay_ref.py's client sends.
        let info = DeviceInfo {
            screen_w: 1920,
            screen_h: 1080,
            xdpi: 400.0,
            ydpi: 400.0,
            max_refresh: 60.0,
            codecs: 3,
            max_decode_w: 4096,
            max_decode_h: 4096,
            max_touches: 10,
            pen_caps: 0x1f,
            audio: 1,
            model: "anydisplay-ref".into(),
            os_version: "python".into(),
        };
        let b = info.body();
        assert_eq!(b.len(), 2 + 2 + 12 + 4 + 4 + 1 + 4 + 1 + 2 + 14 + 2 + 6);
        assert_eq!(&b[..4], &[0x80, 0x07, 0x38, 0x04]);
        assert_eq!(DeviceInfo::from_body(&b).unwrap(), info);
    }

    #[test]
    fn config_is_24_bytes() {
        let c = Config {
            stream_id: 1,
            width: 2,
            height: 3,
            refresh_hz: 60,
            codec: 1,
            bitrate_kbps: 8000,
            flags: 0,
            logical_w: 2,
            logical_h: 3,
            host_caps: 3,
        };
        assert_eq!(c.body().len(), 24);
        assert_eq!(Config::from_body(&c.body()).unwrap(), c);
    }

    #[test]
    fn trailing_bytes_are_ignored() {
        let mut b = Ping {
            seq: 7,
            sender_time_us: 123_456_789,
        }
        .body();
        assert_eq!(b, hex("0700000015cd5b0700000000"));
        b.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            Ping::from_body(&b).unwrap(),
            Ping {
                seq: 7,
                sender_time_us: 123_456_789
            }
        );
    }

    #[test]
    fn input_round_trip() {
        let pen = Pen {
            samples: vec![PenSample {
                action: pen_action::MOVE,
                tool: PEN_TOOL_ERASER,
                buttons: PEN_BUTTON_BARREL,
                x: 100,
                y: 65535,
                pressure: 30000,
                tilt_x: -4500,
                tilt_y: 9000,
                twist: 18000,
                time_ms: 99,
            }],
        };
        assert_eq!(pen.body().len(), 1 + 19);
        let f = pen.frame();
        assert_eq!(InputEvent::decode(&f).unwrap(), Some(InputEvent::Pen(pen)));
        let touch = Touch {
            contacts: vec![
                TouchContact {
                    id: 3,
                    action: touch_action::DOWN,
                    x: 1,
                    y: 2,
                    pressure: 3,
                    size: 4,
                },
                TouchContact {
                    id: 4,
                    action: touch_action::UP,
                    x: 5,
                    y: 6,
                    pressure: 7,
                    size: 8,
                },
            ],
        };
        assert_eq!(touch.body().len(), 1 + 2 * 10);
        assert_eq!(
            InputEvent::decode(&touch.frame()).unwrap(),
            Some(InputEvent::Touch(touch))
        );
        let unknown = Frame {
            channel: INPUT,
            ty: 0x5f,
            body: vec![],
        };
        assert_eq!(InputEvent::decode(&unknown).unwrap(), None);
    }

    pub(crate) fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
