//! Media Foundation H.264 / HEVC encoding of NV12 D3D11 textures: hardware MFTs (asynchronous,
//! fed GPU surfaces) first, the synchronous software MFT (fed system-memory NV12) as fallback.
//! Low-latency mode, no B-frames, CBR at the CONFIG bitrate, keyframes on request.

use crate::annexb;
use crate::platform::EncodedFrame;
use crate::proto::messages::Codec;
use anyhow::{anyhow, bail, Context};
use std::sync::Once;
use std::time::{Duration, Instant};
use windows::core::{Interface, GUID};
use windows::Win32::Foundation::{LUID, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::{VARIANT, VARIANT_0_0, VT_BOOL, VT_UI4};

static MF_STARTUP: Once = Once::new();

fn startup() {
    MF_STARTUP.call_once(|| unsafe {
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            crate::warn!("MFStartup failed: {e}");
        }
    });
}

fn variant_u32(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner: &mut VARIANT_0_0 = &mut var.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    var
}

fn variant_bool(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner: &mut VARIANT_0_0 = &mut var.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if v { VARIANT_TRUE } else { VARIANT_FALSE };
    }
    var
}

#[derive(Clone, Copy)]
pub struct EncoderConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
}

pub struct Encoder {
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    codec_api: Option<ICodecAPI>,
    pub hardware: bool,
    pub name: String,
    cfg: EncoderConfig,
    input_id: u32,
    output_id: u32,
    provides_samples: bool,
    output_size: u32,
    /// METransformNeedInput events not yet answered (asynchronous MFTs).
    need_input: u32,
    sequence_header: Vec<u8>,
    context: ID3D11DeviceContext,
    staging: Option<ID3D11Texture2D>,
    _manager: Option<IMFDXGIDeviceManager>,
}

// MTA COM objects; see `ComGuard`.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Tries hardware encoders on `adapter_luid`, then software.
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        adapter_luid: LUID,
        cfg: EncoderConfig,
    ) -> anyhow::Result<Encoder> {
        startup();
        let subtype = match cfg.codec {
            Codec::Hevc => MFVideoFormat_HEVC,
            _ => MFVideoFormat_H264,
        };
        let mut last_err = anyhow!("no {} encoder found", cfg.codec.name());
        for hardware in [true, false] {
            for activate in enumerate(subtype, hardware) {
                match Self::open(
                    &activate,
                    device,
                    context,
                    adapter_luid,
                    hardware,
                    &cfg,
                    subtype,
                ) {
                    Ok(e) => return Ok(e),
                    Err(e) => {
                        crate::debug!("encoder candidate rejected: {e:#}");
                        last_err = e;
                    }
                }
            }
        }
        Err(last_err)
    }

    fn open(
        activate: &IMFActivate,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        adapter_luid: LUID,
        hardware: bool,
        cfg: &EncoderConfig,
        subtype: GUID,
    ) -> anyhow::Result<Encoder> {
        unsafe {
            let name = attr_string(activate, &MFT_FRIENDLY_NAME_Attribute)
                .unwrap_or_else(|| "unknown MFT".into());
            if hardware {
                // A hardware encoder must sit on the GPU that owns the captured surfaces.
                let mut luid = [0u8; 8];
                if activate
                    .GetBlob(&MFT_ENUM_ADAPTER_LUID, &mut luid, None)
                    .is_ok()
                {
                    let l = LUID {
                        LowPart: u32::from_le_bytes(luid[0..4].try_into().unwrap()),
                        HighPart: i32::from_le_bytes(luid[4..8].try_into().unwrap()),
                    };
                    if l.LowPart != adapter_luid.LowPart || l.HighPart != adapter_luid.HighPart {
                        bail!("{name} is on another adapter");
                    }
                }
            }
            let mft: IMFTransform = activate
                .ActivateObject()
                .with_context(|| format!("activating {name}"))?;
            let attrs = mft.GetAttributes().ok();
            let is_async = attrs
                .as_ref()
                .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok())
                .unwrap_or(0)
                != 0;
            if let Some(a) = &attrs {
                if is_async {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .context("unlocking async MFT")?;
                }
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }

            let mut manager = None;
            if hardware {
                let mut token = 0u32;
                let mut m: Option<IMFDXGIDeviceManager> = None;
                MFCreateDXGIDeviceManager(&mut token, &mut m)?;
                let m = m.ok_or_else(|| anyhow!("no DXGI device manager"))?;
                m.ResetDevice(device, token)?;
                mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize)
                    .with_context(|| format!("{name} does not take D3D11 surfaces"))?;
                manager = Some(m);
            }

            let (mut input_id, mut output_id) = ([0u32], [0u32]);
            if mft.GetStreamIDs(&mut input_id, &mut output_id).is_err() {
                (input_id, output_id) = ([0], [0]);
            }
            let (input_id, output_id) = (input_id[0], output_id[0]);

            // Rate control before the media types: some drivers read it only at type negotiation.
            let codec_api: Option<ICodecAPI> = mft.cast().ok();
            if let Some(api) = &codec_api {
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
                let _ = api.SetValue(
                    &CODECAPI_AVEncCommonRateControlMode,
                    &variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
                );
                let _ = api.SetValue(
                    &CODECAPI_AVEncCommonMeanBitRate,
                    &variant_u32(cfg.kbps.saturating_mul(1000)),
                );
                let _ = api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &variant_u32(0));
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &variant_u32(cfg.fps * 4));
            }

            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, cfg.kbps.saturating_mul(1000))?;
            set_video_common(&out, cfg)?;
            let profile = match cfg.codec {
                Codec::Hevc => eAVEncH265VProfile_Main_420_8.0 as u32,
                _ => eAVEncH264VProfile_High.0 as u32,
            };
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, profile)?;
            mft.SetOutputType(output_id, &out, 0)
                .with_context(|| format!("{name}: output type"))?;

            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            set_video_common(&input, cfg)?;
            mft.SetInputType(input_id, &input, 0)
                .with_context(|| format!("{name}: NV12 input type"))?;

            let info = mft.GetOutputStreamInfo(output_id)?;
            let provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;

            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            let events = if is_async {
                Some(mft.cast::<IMFMediaEventGenerator>()?)
            } else {
                None
            };

            let mut e = Encoder {
                sequence_header: Vec::new(),
                mft,
                events,
                codec_api,
                hardware,
                name,
                cfg: EncoderConfig { ..*cfg },
                input_id,
                output_id,
                provides_samples,
                output_size: info.cbSize.max(cfg.width * cfg.height * 3 / 2),
                need_input: 0,
                context: context.clone(),
                staging: None,
                _manager: manager,
            };
            e.sequence_header = e.read_sequence_header();
            crate::info!(
                "encoder: {} ({}, {})",
                e.name,
                if hardware { "hardware" } else { "software" },
                if is_async { "async" } else { "sync" }
            );
            Ok(e)
        }
    }

    fn read_sequence_header(&self) -> Vec<u8> {
        unsafe {
            let Ok(t) = self.mft.GetOutputCurrentType(self.output_id) else {
                return vec![];
            };
            let Ok(n) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) else {
                return vec![];
            };
            let mut buf = vec![0u8; n as usize];
            if t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut buf, None)
                .is_err()
            {
                return vec![];
            }
            buf
        }
    }

    fn make_sample(&mut self, nv12: &ID3D11Texture2D, pts_us: u64) -> anyhow::Result<IMFSample> {
        unsafe {
            let buffer = if self.hardware {
                MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12, 0, false)?
            } else {
                self.system_memory_nv12(nv12)?
            };
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts_us as i64 * 10)?;
            sample.SetSampleDuration(10_000_000 / self.cfg.fps.max(1) as i64)?;
            Ok(sample)
        }
    }

    /// Software MFTs read system memory: copy the NV12 surface through a staging texture.
    fn system_memory_nv12(&mut self, nv12: &ID3D11Texture2D) -> anyhow::Result<IMFMediaBuffer> {
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        unsafe {
            if self.staging.is_none() {
                let device = nv12.GetDevice()?;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w as u32,
                    Height: h as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut t = None;
                device.CreateTexture2D(&desc, None, Some(&mut t))?;
                self.staging = t;
            }
            let staging = self.staging.clone().unwrap();
            self.context.CopyResource(&staging, nv12);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let size = w * h * 3 / 2;
            let buffer = MFCreateMemoryBuffer(size as u32)?;
            let mut dst = std::ptr::null_mut();
            buffer.Lock(&mut dst, None, None)?;
            let pitch = mapped.RowPitch as usize;
            let src = mapped.pData as *const u8;
            // Y plane, then the interleaved UV plane, which starts `pitch * height` into the mapping.
            for row in 0..h {
                std::ptr::copy_nonoverlapping(src.add(row * pitch), dst.add(row * w), w);
            }
            for row in 0..h / 2 {
                std::ptr::copy_nonoverlapping(
                    src.add(pitch * h + row * pitch),
                    dst.add(w * h + row * w),
                    w,
                );
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(size as u32)?;
            self.context.Unmap(&staging, 0);
            Ok(buffer)
        }
    }

    /// Encodes one frame. Returns the access unit when the encoder produced one in time.
    pub fn encode(
        &mut self,
        nv12: &ID3D11Texture2D,
        pts_us: u64,
        keyframe: bool,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        if keyframe {
            if let Some(api) = &self.codec_api {
                unsafe {
                    let _ = api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1));
                }
            }
        }
        let sample = self.make_sample(nv12, pts_us)?;
        if self.events.is_some() {
            self.encode_async(&sample)
        } else {
            unsafe { self.mft.ProcessInput(self.input_id, &sample, 0) }.context("ProcessInput")?;
            self.drain_sync()
        }
    }

    fn drain_sync(&mut self) -> anyhow::Result<Option<EncodedFrame>> {
        match self.process_output() {
            Ok(f) => Ok(Some(f)),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(None),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                self.renegotiate_output()?;
                Ok(None)
            }
            Err(e) => Err(e).context("ProcessOutput"),
        }
    }

    /// Asynchronous MFT: feed on METransformNeedInput, collect on METransformHaveOutput.
    fn encode_async(&mut self, sample: &IMFSample) -> anyhow::Result<Option<EncodedFrame>> {
        let events = self.events.clone().unwrap();
        let deadline = Instant::now() + Duration::from_millis(200);
        let mut fed = false;
        loop {
            if !fed && self.need_input > 0 {
                unsafe { self.mft.ProcessInput(self.input_id, sample, 0) }
                    .context("ProcessInput")?;
                self.need_input -= 1;
                fed = true;
            }
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(ev) => ev,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() > deadline {
                        // Not fed: the frame is skipped. Fed: its output arrives with the next call.
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(e) => return Err(e).context("encoder event"),
            };
            let kind = unsafe { event.GetType() }?;
            if kind == METransformNeedInput.0 as u32 {
                self.need_input += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                match self.process_output() {
                    Ok(f) => return Ok(Some(f)),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.renegotiate_output()?
                    }
                    Err(e) => return Err(e).context("ProcessOutput"),
                }
            }
        }
    }

    fn renegotiate_output(&mut self) -> anyhow::Result<()> {
        unsafe {
            let t = self.mft.GetOutputAvailableType(self.output_id, 0)?;
            self.mft.SetOutputType(self.output_id, &t, 0)?;
        }
        let header = self.read_sequence_header();
        if !header.is_empty() {
            self.sequence_header = header;
        }
        Ok(())
    }

    fn process_output(&mut self) -> windows::core::Result<EncodedFrame> {
        unsafe {
            let sample = if self.provides_samples {
                None
            } else {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(self.output_size)?)?;
                Some(s)
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.output_id,
                pSample: std::mem::ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = self.mft.ProcessOutput(0, &mut buffers, &mut status);
            let [mut b] = buffers;
            let sample = std::mem::ManuallyDrop::take(&mut b.pSample);
            drop(std::mem::ManuallyDrop::take(&mut b.pEvents));
            r?;
            let sample = sample
                .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut ptr, None, Some(&mut len))?;
            let mut data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            buffer.Unlock()?;
            let clean_point = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let keyframe = clean_point || annexb::is_keyframe(&data, self.cfg.codec);
            if keyframe {
                data = annexb::with_parameter_sets(data, self.cfg.codec, &self.sequence_header);
            }
            let pts_us = sample.GetSampleTime().map(|t| (t / 10) as u64).unwrap_or(0);
            Ok(EncodedFrame {
                data,
                keyframe,
                pts_us,
            })
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

fn set_video_common(t: &IMFMediaType, cfg: &EncoderConfig) -> windows::core::Result<()> {
    unsafe {
        t.SetUINT64(
            &MF_MT_FRAME_SIZE,
            ((cfg.width as u64) << 32) | cfg.height as u64,
        )?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((cfg.fps as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        // BT.709 limited range, matching the video processor's output color space.
        let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
    }
    Ok(())
}

/// Encoder MFTs for NV12 → `subtype`, best first.
fn enumerate(subtype: GUID, hardware: bool) -> Vec<IMFActivate> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    };
    let flags = MFT_ENUM_FLAG_SORTANDFILTER
        | if hardware {
            MFT_ENUM_FLAG_HARDWARE
        } else {
            MFT_ENUM_FLAG_SYNCMFT
        };
    let mut out = Vec::new();
    unsafe {
        let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input),
            Some(&output),
            &mut array,
            &mut count,
        )
        .is_err()
            || array.is_null()
        {
            return out;
        }
        for i in 0..count as usize {
            if let Some(a) = std::ptr::read(array.add(i)) {
                out.push(a);
            }
        }
        CoTaskMemFree(Some(array as *const _));
    }
    out
}

fn attr_string(a: &IMFActivate, key: &GUID) -> Option<String> {
    unsafe {
        let len = a.GetStringLength(key).ok()?;
        let mut buf = vec![0u16; len as usize + 1];
        a.GetString(key, &mut buf, None).ok()?;
        Some(super::wide(&buf))
    }
}
