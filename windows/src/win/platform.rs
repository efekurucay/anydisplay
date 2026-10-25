//! The Windows [`Platform`]: picks the monitor (the Virtual Display Driver's when installed,
//! else mirroring with NOTICE 5), builds capture + encoder per stream, injects input.

use super::capture::{Capture, Captured};
use super::clipboard::Clipboard;
use super::encoder::{Encoder, EncoderConfig};
use super::input::Injector;
use super::monitors::{self, Monitor};
use super::{audio, ComGuard};
use crate::input::InputTarget;
use crate::negotiate::{auto_bitrate, fit_aspect};
use crate::platform::{
    AudioStream, EncodedFrame, FrameSink, OpenedStream, Platform, StreamRequest, VideoPipeline,
};
use crate::proto::messages::{host_caps, notice, stream_flags, Codec, InputEvent, Message, Notice};
use crate::util::now_us;
use anyhow::{anyhow, bail, Context};
use std::time::{Duration, Instant};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

pub struct WinPlatform {
    monitor_choice: Option<usize>,
    injector: Injector,
    clipboard: Option<Clipboard>,
    /// GDI name of a display whose mode this session changed.
    mode_changed: Option<String>,
    notices: Option<FrameSink>,
    input_notice_sent: bool,
    _com: ComGuard,
}

impl WinPlatform {
    pub fn new(monitor_choice: Option<usize>) -> anyhow::Result<WinPlatform> {
        let com = ComGuard::new();
        let monitors = monitors::list()?;
        if monitors.is_empty() {
            bail!("no monitor attached to the desktop");
        }
        if let Some(i) = monitor_choice {
            if i >= monitors.len() {
                bail!(
                    "--monitor {i}: only {} monitor(s), see `anydisplay-win list`",
                    monitors.len()
                );
            }
        }
        let clipboard = Clipboard::new()
            .map_err(|e| crate::warn!("clipboard unavailable: {e}"))
            .ok();
        Ok(WinPlatform {
            monitor_choice,
            injector: Injector::new(),
            clipboard,
            mode_changed: None,
            notices: None,
            input_notice_sent: false,
            _com: com,
        })
    }

    fn pick(&self, monitors: &[Monitor], mirror: bool) -> usize {
        if let Some(i) = self.monitor_choice {
            return i;
        }
        let primary = monitors.iter().position(|m| m.primary).unwrap_or(0);
        if mirror {
            return primary;
        }
        monitors
            .iter()
            .position(|m| m.virtual_display)
            .unwrap_or(primary)
    }
}

fn logical_size(m: &Monitor) -> (u32, u32) {
    let (mut dx, mut dy) = (96u32, 96u32);
    unsafe {
        let _ = GetDpiForMonitor(m.hmonitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    }
    (
        (m.rect.w as u32 * 96) / dx.max(1),
        (m.rect.h as u32 * 96) / dy.max(1),
    )
}

impl Platform for WinPlatform {
    fn host_os(&self) -> String {
        format!("Windows ({})", std::env::consts::ARCH)
    }

    fn host_caps(&self) -> u32 {
        let mut caps = 0;
        if self.injector.has_touch() {
            caps |= host_caps::NATIVE_TOUCH;
        }
        if self.injector.has_pen() {
            caps |= host_caps::NATIVE_PEN;
        }
        if self.clipboard.is_some() {
            caps |= host_caps::CLIPBOARD;
        }
        caps
    }

    fn open_stream(
        &mut self,
        req: &StreamRequest<'_>,
        sink: FrameSink,
    ) -> anyhow::Result<OpenedStream> {
        let plan = req.plan;
        let mirror_requested = plan.wants_mirror();
        let monitors = monitors::list()?;
        if monitors.is_empty() {
            bail!("no monitor attached to the desktop");
        }
        let idx = self
            .pick(&monitors, mirror_requested)
            .min(monitors.len() - 1);
        let mut m = monitors.into_iter().nth(idx).expect("index checked");
        let extend = m.virtual_display && !mirror_requested;
        let mut notices = Vec::new();
        if !mirror_requested && !m.virtual_display {
            notices.push((
                notice::VIRTUAL_DISPLAY_UNAVAILABLE,
                format!(
                    "No virtual display found (install the Virtual Display Driver for an extra monitor); mirroring {}",
                    if m.friendly_name.is_empty() { &m.gdi_name } else { &m.friendly_name }
                ),
            ));
        }

        let (mut w, mut h) = (plan.width, plan.height);
        if extend && (m.rect.w as u32, m.rect.h as u32) != (w, h) {
            if monitors::set_mode(&m.gdi_name, w, h, plan.fps) {
                self.mode_changed = Some(m.gdi_name.clone());
                std::thread::sleep(Duration::from_millis(300));
                let gdi = m.gdi_name.clone();
                m = monitors::list()?
                    .into_iter()
                    .find(|x| x.gdi_name == gdi)
                    .ok_or_else(|| anyhow!("{gdi} vanished after the mode change"))?;
            } else {
                notices.push((
                    notice::RESOLUTION_UNSUPPORTED,
                    format!("The virtual display has no {w}x{h} mode (add it to vdd_settings.xml); using {}x{}", m.rect.w, m.rect.h),
                ));
            }
        }
        if (m.rect.w as u32, m.rect.h as u32) != (w, h) {
            (w, h) = fit_aspect(m.rect.w as u32, m.rect.h as u32, w, h);
        }
        let fps = plan.fps.min(120);
        let client_cursor = plan.wants(stream_flags::CLIENT_CURSOR);

        let mut capture = None;
        let mut last_err = None;
        // Right after a mode change the output may not be ready to duplicate yet.
        for _ in 0..10 {
            match Capture::new(
                &m,
                w,
                h,
                fps,
                !client_cursor,
                client_cursor.then(|| sink.clone()),
            ) {
                Ok(c) => {
                    capture = Some(c);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        let capture = capture
            .ok_or_else(|| last_err.unwrap())
            .with_context(|| format!("capturing {}", m.gdi_name))?;

        let luid = unsafe { m.adapter.GetDesc1() }?.AdapterLuid;
        let mut codec = plan.codec;
        let kbps_for = |c: Codec| {
            if plan.bitrate_kbps == 0 {
                auto_bitrate(w, h, fps, plan.quality, c)
            } else {
                plan.bitrate_kbps
            }
        };
        let encoder = match Encoder::new(
            &capture.device,
            capture.context(),
            luid,
            EncoderConfig {
                codec,
                width: w,
                height: h,
                fps,
                kbps: kbps_for(codec),
            },
        ) {
            Ok(e) => e,
            Err(e) if codec == Codec::Hevc && req.device.codecs & Codec::H264.bit() != 0 => {
                crate::info!("no HEVC encoder ({e:#}); using H.264");
                codec = Codec::H264;
                Encoder::new(
                    &capture.device,
                    capture.context(),
                    luid,
                    EncoderConfig {
                        codec,
                        width: w,
                        height: h,
                        fps,
                        kbps: kbps_for(codec),
                    },
                )?
            }
            Err(e) => return Err(e),
        };
        if !encoder.hardware {
            notices.push((
                notice::SOFTWARE_ENCODER,
                format!(
                    "Hardware {} encoder unavailable; using software",
                    codec.name()
                ),
            ));
        }

        let mut flags = 0;
        if !extend {
            flags |= stream_flags::MIRROR;
        }
        if client_cursor {
            flags |= stream_flags::CLIENT_CURSOR;
        }
        let (logical_w, logical_h) = logical_size(&m);
        self.injector.set_target(Some(InputTarget {
            monitor: m.rect,
            stream_w: w,
            stream_h: h,
        }));
        self.notices = Some(sink);
        let source = format!(
            "{} ({}) via {}",
            m,
            if extend { "extended" } else { "mirrored" },
            encoder.name
        );
        Ok(OpenedStream {
            width: w,
            height: h,
            fps,
            codec,
            bitrate_kbps: kbps_for(codec),
            flags,
            logical_w,
            logical_h,
            notices,
            hardware_encoder: encoder.hardware,
            source,
            pipeline: Box::new(WinPipeline {
                capture,
                encoder,
                interval: Duration::from_secs(1) / fps.max(1),
                last: None,
                _com: None,
            }),
        })
    }

    fn inject(&mut self, event: InputEvent) {
        self.injector.handle(event);
    }

    fn input_tick(&mut self) {
        self.injector.tick();
        if let Some(c) = &self.clipboard {
            c.pump();
        }
        if let (Some(msg), false, Some(sink)) = (
            &self.injector.blocked,
            self.input_notice_sent,
            &self.notices,
        ) {
            self.input_notice_sent = true;
            sink(
                Notice {
                    code: notice::INPUT_PERMISSION,
                    message: format!("{msg}. Windows blocks input into elevated windows unless anydisplay-win runs as administrator."),
                }
                .frame(),
            );
        }
    }

    fn start_audio(&mut self, sink: FrameSink) -> anyhow::Result<Option<AudioStream>> {
        audio::start(sink).map(Some)
    }

    fn clipboard_changed(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.changed()
    }

    fn set_clipboard(&mut self, text: &str) {
        if let Some(c) = &mut self.clipboard {
            c.set(text);
        }
    }

    fn end_session(&mut self) {
        self.injector.reset();
        self.injector.set_target(None);
        if let Some(gdi) = self.mode_changed.take() {
            monitors::restore_mode(&gdi);
        }
    }
}

/// Capture → NV12 → encoder, paced to the stream rate. Runs on the session's video thread.
struct WinPipeline {
    capture: Capture,
    encoder: Encoder,
    interval: Duration,
    last: Option<Instant>,
    /// Joins the MTA on the video thread; declared last so it is released after the COM objects.
    _com: Option<ComGuard>,
}

// All COM objects here live in the MTA (`ComGuard`), so they may be used from the video thread.
unsafe impl Send for WinPipeline {}

impl VideoPipeline for WinPipeline {
    fn next_frame(&mut self, force_keyframe: bool) -> anyhow::Result<Option<EncodedFrame>> {
        if self._com.is_none() {
            self._com = Some(ComGuard::new());
        }
        if let Some(last) = self.last {
            let next = last + self.interval;
            if let Some(wait) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        let timeout = self.interval.as_millis().clamp(1, 100) as u32;
        match self.capture.capture(timeout, force_keyframe)? {
            Captured::Unchanged => Ok(None),
            Captured::Frame(nv12) => {
                self.last = Some(Instant::now());
                self.encoder.encode(&nv12, now_us(), force_keyframe)
            }
        }
    }
}
