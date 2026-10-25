//! Choosing the stream parameters from DEVICE_INFO and CONFIG_REQ (PROTOCOL.md 5.1).

use crate::proto::messages::{notice, stream_flags, Codec, ConfigReq, DeviceInfo};

/// Largest edge the hardware encoders accept.
pub const ENCODER_MAX_EDGE: u32 = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct StreamPlan {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    /// 0 = choose with [`auto_bitrate`] once the final size is known.
    pub bitrate_kbps: u32,
    pub quality: u8,
    pub flags: u8,
    pub notices: Vec<(u16, String)>,
}

/// `prefer` is the host's `--codec` choice; it applies when the client can decode it.
pub fn plan(info: &DeviceInfo, req: &ConfigReq, prefer: Option<Codec>) -> StreamPlan {
    let mut notices = Vec::new();
    let (mut w, mut h) = if req.width == 0 || req.height == 0 {
        (info.screen_w as u32, info.screen_h as u32)
    } else {
        (req.width as u32, req.height as u32)
    };
    if w == 0 || h == 0 {
        (w, h) = (1920, 1080);
    }
    let decode_edge = info.max_decode_w.max(info.max_decode_h) as u32;
    let limit = if decode_edge > 0 {
        decode_edge.min(ENCODER_MAX_EDGE)
    } else {
        ENCODER_MAX_EDGE
    };
    if w > limit || h > limit {
        let k = (limit as f64 / w as f64).min(limit as f64 / h as f64);
        let (nw, nh) = ((w as f64 * k) as u32, (h as f64 * k) as u32);
        notices.push((
            notice::RESOLUTION_UNSUPPORTED,
            format!("{w}x{h} exceeds the decoder or encoder limit; streaming {nw}x{nh}"),
        ));
        (w, h) = (nw, nh);
    }
    let w = (w & !1).max(320);
    let h = (h & !1).max(240);

    let mut fps = if req.refresh_hz == 0 {
        60
    } else {
        req.refresh_hz as u32
    };
    if info.max_refresh >= 1.0 {
        fps = fps.min(info.max_refresh.round() as u32);
    }
    let fps = fps.clamp(1, 120);

    let can_decode = |c: Codec| info.codecs & c.bit() != 0;
    let wanted = prefer
        .or(Codec::from_u8(req.codec))
        .filter(|&c| c != Codec::Av1)
        .unwrap_or(Codec::H264);
    let codec = if can_decode(wanted) {
        wanted
    } else if can_decode(Codec::H264) {
        Codec::H264
    } else {
        Codec::Hevc
    };

    StreamPlan {
        width: w,
        height: h,
        fps,
        codec,
        bitrate_kbps: req.bitrate_kbps,
        quality: req.quality,
        flags: req.flags,
        notices,
    }
}

impl StreamPlan {
    pub fn wants(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub fn wants_mirror(&self) -> bool {
        self.wants(stream_flags::MIRROR)
    }
}

/// Bitrate when CONFIG_REQ leaves it at 0.
pub fn auto_bitrate(width: u32, height: u32, fps: u32, quality: u8, codec: Codec) -> u32 {
    let q = if quality == 0 {
        80
    } else {
        quality.clamp(1, 100)
    } as f64
        / 100.0;
    let bits_per_pixel = 0.03 + 0.09 * q;
    let mut kbps = (width * height) as f64 * fps.min(60) as f64 * bits_per_pixel / 1000.0;
    if codec == Codec::Hevc {
        kbps *= 0.7;
    }
    kbps.clamp(2000.0, 60_000.0) as u32
}

/// Largest even size with the source's aspect ratio inside `max_w x max_h` (mirroring).
pub fn fit_aspect(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (max_w & !1, max_h & !1);
    }
    let k = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    (
        ((src_w as f64 * k) as u32 & !1).max(2),
        ((src_h as f64 * k) as u32 & !1).max(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> DeviceInfo {
        DeviceInfo {
            screen_w: 2560,
            screen_h: 1600,
            xdpi: 274.0,
            ydpi: 274.0,
            max_refresh: 120.0,
            codecs: 0b011,
            max_decode_w: 3840,
            max_decode_h: 2160,
            max_touches: 10,
            pen_caps: 0x1f,
            audio: 1,
            model: "Tab".into(),
            os_version: "15".into(),
        }
    }

    fn req() -> ConfigReq {
        ConfigReq {
            width: 0,
            height: 0,
            refresh_hz: 0,
            codec: 1,
            bitrate_kbps: 0,
            quality: 80,
            flags: 0,
        }
    }

    #[test]
    fn defaults_to_screen_size_and_60_hz() {
        let p = plan(&info(), &req(), None);
        assert_eq!(
            (p.width, p.height, p.fps, p.codec),
            (2560, 1600, 60, Codec::H264)
        );
        assert!(p.notices.is_empty());
    }

    #[test]
    fn clamps_to_decoder_and_rounds_even() {
        let mut r = req();
        (r.width, r.height, r.refresh_hz) = (7681, 4321, 144);
        let p = plan(&info(), &r, None);
        assert_eq!((p.width, p.height, p.fps), (3840, 2160, 120));
        assert_eq!(p.notices[0].0, notice::RESOLUTION_UNSUPPORTED);
    }

    #[test]
    fn codec_choice() {
        let mut r = req();
        r.codec = 2;
        assert_eq!(plan(&info(), &r, None).codec, Codec::Hevc);
        assert_eq!(plan(&info(), &r, Some(Codec::H264)).codec, Codec::H264);
        let mut i = info();
        i.codecs = 0b001;
        assert_eq!(plan(&i, &r, Some(Codec::Hevc)).codec, Codec::H264);
        r.codec = 3;
        assert_eq!(plan(&info(), &r, None).codec, Codec::H264);
    }

    #[test]
    fn bitrate_and_aspect() {
        assert_eq!(auto_bitrate(1920, 1080, 60, 80, Codec::H264), 12_690);
        assert_eq!(auto_bitrate(320, 240, 30, 1, Codec::H264), 2000);
        assert_eq!(fit_aspect(1920, 1080, 2560, 1600), (2560, 1440));
        assert_eq!(fit_aspect(1080, 1920, 2560, 1600), (900, 1600));
    }
}
