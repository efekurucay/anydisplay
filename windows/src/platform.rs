//! What a host platform provides to the session: display capture + encoding, input injection,
//! audio, clipboard. `win` implements it for Windows, `testsrc` with an ffmpeg test pattern.

use crate::negotiate::StreamPlan;
use crate::proto::messages::{AudioConfig, Codec, DeviceInfo, InputEvent};
use crate::proto::Frame;
use std::sync::Arc;

/// Queues a frame for the client (thread-safe, never blocks on the network).
pub type FrameSink = Arc<dyn Fn(Frame) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Annex-B access unit; keyframes carry their parameter sets.
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub pts_us: u64,
}

/// Captures and encodes one stream. Runs on its own thread; the session calls `next_frame` only
/// while fewer than 3 frames are unacknowledged, which is how capture skips frames under load.
pub trait VideoPipeline: Send {
    /// Waits up to about one frame interval. `force_keyframe`: make the next output a keyframe
    /// (re-encoding the last image when the screen is static). `Ok(None)`: nothing new.
    fn next_frame(&mut self, force_keyframe: bool) -> anyhow::Result<Option<EncodedFrame>>;
}

pub struct StreamRequest<'a> {
    pub plan: &'a StreamPlan,
    pub device: &'a DeviceInfo,
    pub device_id: [u8; 16],
    pub device_name: &'a str,
}

pub struct OpenedStream {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    pub bitrate_kbps: u32,
    /// Granted `stream_flags` (HIDPI, CLIENT_CURSOR, MIRROR); the session adds AUDIO.
    pub flags: u8,
    pub logical_w: u32,
    pub logical_h: u32,
    pub notices: Vec<(u16, String)>,
    pub hardware_encoder: bool,
    /// For logs: which display is captured and how.
    pub source: String,
    pub pipeline: Box<dyn VideoPipeline>,
}

/// A running audio capture; dropping it stops capture.
pub struct AudioStream {
    pub config: AudioConfig,
    pub handle: Box<dyn Send>,
}

pub trait Platform {
    /// `host_os` for HELLO_H.
    fn host_os(&self) -> String;

    /// `CONFIG.host_caps` bits for touch, pen and clipboard.
    fn host_caps(&self) -> u32;

    /// Sets up display, capture and encoder for a new stream. `cursor` receives CURSOR_POS /
    /// CURSOR_SHAPE frames when the client draws the cursor.
    fn open_stream(
        &mut self,
        req: &StreamRequest<'_>,
        cursor: FrameSink,
    ) -> anyhow::Result<OpenedStream>;

    /// Called after the stream's pipeline has stopped (before a new CONFIG or at the end).
    fn close_stream(&mut self) {}

    fn inject(&mut self, event: InputEvent);

    /// Called about every 20 ms (stationary touch contacts need refreshes).
    fn input_tick(&mut self) {}

    /// Starts loopback capture; samples go to `sink` as AUDIO_DATA frames.
    fn start_audio(&mut self, _sink: FrameSink) -> anyhow::Result<Option<AudioStream>> {
        Ok(None)
    }

    /// Text copied on the host since the last call (only changes made during the session).
    fn clipboard_changed(&mut self) -> Option<String> {
        None
    }

    fn set_clipboard(&mut self, _text: &str) {}

    /// Releases held keys and contacts, restores display modes.
    fn end_session(&mut self) {}
}
